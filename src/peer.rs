
use std::collections::VecDeque;
use std::collections::HashMap;
use std::net;
use std::time;

use super::frame;

const MAX_CHANNELS: usize = 256;

struct SendEntry {
    data: Box<[u8]>,
    sequence_id: u32,
    reliable: bool,
}

impl SendEntry {
    fn new(data: Box<[u8]>, sequence_id: u32, reliable: bool) -> Self {
        Self {
            data: data, 
            sequence_id: sequence_id,
            reliable: reliable,
        }
    }
}

struct ResendEntry {
    data: Box<[u8]>,
    last_send_time: Option<time::Instant>,
    sequence_id: u32,
}

impl ResendEntry {
    fn new(data: Box<[u8]>, sequence_id: u32) -> Self {
        Self {
            data: data, 
            last_send_time: None,
            sequence_id: sequence_id,
        }
    }

    fn mark_sent(&mut self, now: time::Instant) {
        self.last_send_time = Some(now);
    }

    fn should_resend(&self, now: time::Instant, timeout: time::Duration) -> bool {
        match self.last_send_time {
            Some(send_time) => now - send_time > timeout,
            None => true,
        }
    }
}

#[derive(Clone,Debug)]
pub struct Params {
    pub num_channels: u32,
    pub max_tx_bandwidth: u32,
    pub max_rx_bandwidth: u32,
}

#[derive(Clone,Copy,Debug,PartialEq)]
enum State {
    AwaitConnect,     // Host: Do nothing (Connect => AwaitConnectAck, Bad Connect => SendDisconnect)
    AwaitConnectAck,  // Host: Send Connect (ConnectAck -> Connected)
    SendConnect,      // Client: Send Connect (Connect -> Connected, Bad Connect => SendDisconnect)
    Connected,        // Send/receive data (disconnect() => SendDisconnect)
    SendDisconnect,   // Send disconnect (DisconnectAck -> Disconnected)
    Disconnected,     // Continue to acknowledge Disconnect (30s => Zombie)
    Zombie,           // Nothing
}

#[derive(Clone,Copy,Debug,PartialEq)]
enum Mode {
    Passive,
    Active,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Event {
    Connect,
    Disconnect,
    Receive(Box<[u8]>),
    Timeout,
}

pub struct Peer {
    state: State,

    watchdog_time: time::Instant,
    meta_send_time: Option<time::Instant>,

    rto_ms: f32,

    // Connect, Disconnect, Ping
    meta_queue: VecDeque<Box<[u8]>>,
    event_queue: VecDeque<Event>,

    was_connected: bool,
    disconnect_flush: bool,

    next_reliable_id: u32,
    base_sequence_id: u32,
    send_queue: VecDeque<Box<[u8]>>,
    resend_queue: VecDeque<ResendEntry>,
}

pub trait DataSink {
    fn send(&self, data: &[u8]);
}

fn verify_connect_info(remote_info: frame::Connect) -> bool {
    true
}

impl Peer {
    fn new(mode: Mode, params: Params) -> Self {
        Self {
            state: match mode { Mode::Passive => State::AwaitConnect, Mode::Active => State::SendConnect },

            watchdog_time: time::Instant::now(),
            meta_send_time: None,

            rto_ms: 100.0,

            meta_queue: VecDeque::new(),
            event_queue: VecDeque::new(),

            was_connected: false,
            disconnect_flush: false,

            next_reliable_id: 0,
            base_sequence_id: 0,
            send_queue: VecDeque::new(),
            resend_queue: VecDeque::new(),
        }
    }

    pub fn new_passive(params: Params) -> Self {
        Self::new(Mode::Passive, params)
    }

    pub fn new_active(params: Params) -> Self {
        Self::new(Mode::Active, params)
    }

    fn enqueue_meta(&mut self, frame: frame::Frame) {
        self.meta_queue.push_back(frame.to_bytes());
    }

    // State::AwaitConnect
    fn await_connect_enter(&mut self) {
        self.state = State::AwaitConnect;
        self.watchdog_time = time::Instant::now();

        self.meta_send_time = None;
    }

    fn await_connect_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::Connect(frame) => {
                if verify_connect_info(frame) {
                    self.await_connect_ack_enter();
                } else {
                    // No likey connect
                    self.send_disconnect_enter();
                }
            }
            _ => ()
        }
    }

    // State::AwaitConnectAck
    fn await_connect_ack_enter(&mut self) {
        self.state = State::AwaitConnectAck;
        self.watchdog_time = time::Instant::now();

        self.meta_send_time = None;
    }

    fn await_connect_ack_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::ConnectAck(frame) => {
                self.connected_enter();
            }
            frame::Frame::Disconnect(_) => {
                // Client must not have liked our connect info
                self.enqueue_meta(frame::Frame::DisconnectAck(frame::DisconnectAck { }));
                self.disconnected_enter();
            }
            _ => ()
        }
    }

    fn await_connect_ack_step(&mut self) {
        let now = time::Instant::now();
        let timeout = time::Duration::from_millis(500);

        if self.meta_send_time.map_or(true, |time| now - time > timeout) {
            self.meta_send_time = Some(now);
            self.enqueue_meta(frame::Frame::Connect(frame::Connect {
                version: 0,
                num_channels: 5,
                rx_bandwidth_max: 100000,
            }));
        }
    }

    // State::SendConnect
    fn send_connect_enter(&mut self) {
        self.state = State::SendConnect;
        self.watchdog_time = time::Instant::now();

        self.meta_send_time = None;
    }

    fn send_connect_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::Connect(frame) => {
                if verify_connect_info(frame) {
                    self.enqueue_meta(frame::Frame::ConnectAck(frame::ConnectAck { }));
                    self.connected_enter();
                } else {
                    // No likey connect
                    self.send_disconnect_enter();
                }
            }
            frame::Frame::Disconnect(_) => {
                // Server must not have liked our connect info
                self.enqueue_meta(frame::Frame::DisconnectAck(frame::DisconnectAck { }));
                self.disconnected_enter();
            }
            _ => ()
        }
    }

    fn send_connect_step(&mut self) {
        let now = time::Instant::now();
        let timeout = time::Duration::from_millis(500);

        if self.meta_send_time.map_or(true, |time| now - time > timeout) {
            self.meta_send_time = Some(now);
            self.enqueue_meta(frame::Frame::Connect(frame::Connect {
                version: 0,
                num_channels: 5,
                rx_bandwidth_max: 100000,
            }));
        }
    }

    // State::Connected
    fn connected_enter(&mut self) {
        self.state = State::Connected;
        self.watchdog_time = time::Instant::now();

        self.was_connected = true;
        self.disconnect_flush = false;
        self.event_queue.push_back(Event::Connect);
    }

    fn connected_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::Connect(frame) => {
                // In this state, we've already verified any connection parameters, just ack
                // If both client & host ack here, NAT punchthrough might work
                self.enqueue_meta(frame::Frame::ConnectAck(frame::ConnectAck { }));
            }
            frame::Frame::Disconnect(_) => {
                // Welp
                println!("welp.");
                self.enqueue_meta(frame::Frame::DisconnectAck(frame::DisconnectAck { }));
                self.disconnected_enter();
            }
            frame::Frame::PingAck(_) => {
                // Our ping has been received, pet watchdog
                self.watchdog_time = time::Instant::now();
            }
            _ => ()
        }
    }

    fn connected_step(&mut self) {
        if self.disconnect_flush {
            // TODO: Verify channel tx queues and rstream queue are empty
            self.send_disconnect_enter();
        }
    }

    // State::SendDisconnect
    fn send_disconnect_enter(&mut self) {
        self.state = State::SendDisconnect;
        self.watchdog_time = time::Instant::now();

        self.meta_send_time = None;
    }

    fn send_disconnect_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::Disconnect(_) => {
                // The remote is also disconnecting
                self.enqueue_meta(frame::Frame::DisconnectAck(frame::DisconnectAck { }));
                self.disconnected_enter();
            }
            frame::Frame::DisconnectAck(_) => {
                // Our disconnect has been received
                self.disconnected_enter();
            }
            _ => ()
        }
    }

    fn send_disconnect_step(&mut self) {
        let now = time::Instant::now();
        let timeout = time::Duration::from_millis(500);

        if self.meta_send_time.map_or(true, |time| now - time > timeout) {
            self.meta_send_time = Some(now);
            self.enqueue_meta(frame::Frame::Disconnect(frame::Disconnect { }));
        }
    }

    // State::Disconnected
    fn disconnected_enter(&mut self) {
        self.state = State::Disconnected;
        self.watchdog_time = time::Instant::now();

        // Enqueue disconnected event only if previously connected
        if self.was_connected {
            self.event_queue.push_back(Event::Disconnect);
        }
    }

    fn disconnected_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::Disconnect(_) => {
                self.enqueue_meta(frame::Frame::DisconnectAck(frame::DisconnectAck { }));
            }
            _ => ()
        }
    }

    fn zombie_enter(&mut self) {
        self.state = State::Zombie;

        self.event_queue.push_back(Event::Timeout);
    }

    pub fn handle_frame(&mut self, frame: frame::Frame) {
        match self.state {
            State::AwaitConnect => self.await_connect_handle_frame(frame),
            State::AwaitConnectAck => self.await_connect_ack_handle_frame(frame),
            State::SendConnect => self.send_connect_handle_frame(frame),
            State::Connected => self.connected_handle_frame(frame),
            State::SendDisconnect => self.send_disconnect_handle_frame(frame),
            State::Disconnected => self.disconnected_handle_frame(frame),
            State::Zombie => (),
        }
    }

    pub fn step(&mut self) {
        let now = time::Instant::now();

        if self.state != State::Zombie {
            let watchdog_timeout = time::Duration::from_millis(20000);

            if now - self.watchdog_time > watchdog_timeout {
                self.zombie_enter();
            }
        }

        match self.state {
            State::AwaitConnect => (),
            State::AwaitConnectAck => self.await_connect_ack_step(),
            State::SendConnect => self.send_connect_step(),
            State::Connected => self.connected_step(),
            State::SendDisconnect => self.send_disconnect_step(),
            State::Disconnected => (),
            State::Zombie => (),
        }
    }

    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        std::mem::take(&mut self.event_queue).into_iter()
    }

    fn enqueue_reliable_frame(&mut self, data: Box<[u8]>) {
        self.resend_queue.push_back(ResendEntry { data: data, last_send_time: None, sequence_id: self.next_reliable_id });
        self.next_reliable_id = self.next_reliable_id.wrapping_add(1);
    }

    fn flush_data(&mut self, now: time::Instant, sink: & dyn DataSink) {
        let timeout = time::Duration::from_millis(self.rto_ms.round() as u64);

        for entry in self.resend_queue.iter_mut() {
            if entry.should_resend(now, timeout) {
                entry.mark_sent(now);
                sink.send(&entry.data);
            }
        }

        /*
        for entry in self.send_queue.iter_mut() {
            sink.send(&entry.data);
            if entry.reliable {
                self.resend_queue.push_back(ResendEntry {
                    data: entry.data,
                    last_send_time: Some(time::Instant::now()),
                    sequence_id: entry.sequence_id,
                }
            }
        }

        self.send_queue.clear();
        */
    }

    fn flush_meta(&mut self, now: time::Instant, sink: & dyn DataSink) {
        for data in self.meta_queue.iter() {
            sink.send(&data);
        }
        self.meta_queue.clear();
    }

    pub fn flush(&mut self, sink: & dyn DataSink) {
        let now = time::Instant::now();
        self.flush_meta(now, sink);
        self.flush_data(now, sink);
    }

    pub fn disconnect(&mut self) {
        if self.state == State::Connected {
            self.disconnect_flush = true;
        }
    }

    pub fn is_zombie(&self) -> bool {
        return self.state == State::Zombie;
    }
}


    /*
    let channel = self.channels.get_mut(channel_id as usize).expect("No such channel");
    if self.state == State::Connected {
        channel.tx.enqueue(data, mode);
    }
    */

