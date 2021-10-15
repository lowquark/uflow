
mod rtt;

use std::collections::VecDeque;
use std::time;
use std::ops::Range;

use super::frame;
use super::channel;
use super::ChannelId;
use super::DataSink;
use super::MAX_CHANNELS;
use super::PROTOCOL_VERSION;
use super::SendMode;
use super::transfer;

#[derive(Clone,Debug)]
pub struct Params {
    pub num_channels: u32,
    pub max_tx_bandwidth: u32,
    pub max_rx_bandwidth: u32,
    pub priority_channels: Range<u32>,
    pub max_packet_size: u32,
}

#[derive(Clone,Copy,Debug,PartialEq)]
enum State {
    Connecting,       // Send Connect until acked (Connect & ConnectAck => Connected, Bad Connect => SendDisconnect)
    Connected,        // Send/receive data (disconnect() => SendDisconnect, after all pending data transferred)
    SendDisconnect,   // Send disconnect (DisconnectAck -> Disconnected)
    Disconnected,     // Continue to acknowledge Disconnect (30s => Zombie)
    Zombie,           // Nothing
}

#[derive(Clone,Debug,PartialEq)]
pub enum Event {
    Connect,
    Disconnect,
    Receive(Box<[u8]>),
    Timeout,
}

const PING_INTERVAL_MS: u64 = 1000;
const CONNECT_INTERVAL_MS: u64 = 500;
const DISCONNECT_INTERVAL_MS: u64 = 500;
const WATCHDOG_TIMEOUT_MS: u64 = 20000;

static PING_INTERVAL: time::Duration = time::Duration::from_millis(PING_INTERVAL_MS);
static CONNECT_INTERVAL: time::Duration = time::Duration::from_millis(CONNECT_INTERVAL_MS);
static DISCONNECT_INTERVAL: time::Duration = time::Duration::from_millis(DISCONNECT_INTERVAL_MS);
static WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(WATCHDOG_TIMEOUT_MS);

pub struct Peer {
    state: State,
    params: Params,

    watchdog_time: time::Instant,
    meta_send_time: Option<time::Instant>,

    // Connect, Disconnect, Ping
    meta_queue: VecDeque<Box<[u8]>>,
    event_queue: VecDeque<Event>,

    // Connecting stuff
    connect_frame: frame::Connect,
    connect_ack_received: bool,
    connect_frame_remote: Option<frame::Connect>,

    // Connected stuff
    ping_rtt: rtt::PingRtt,
    was_connected: bool,
    disconnect_flush: bool,

    channels: Vec<channel::Channel>,
    frame_io: transfer::FrameIO,
}

impl Peer {
    pub fn new(params: Params) -> Self {
        assert!(params.num_channels > 0, "Must have at least one channel");
        assert!(params.num_channels <= MAX_CHANNELS, "Number of channels exceeds maximum");

        let connect_frame = frame::Connect {
            version: PROTOCOL_VERSION,
            num_channels: (params.num_channels - 1) as u8,
            max_rx_bandwidth: params.max_rx_bandwidth,
            sequence_id: rand::random::<u32>(),
        };

        // Preallocate channels so that data may be enqueued before a connection is established
        let mut channels = Vec::new();
        for _ in 0..params.num_channels {
            channels.push(channel::Channel::new(params.max_packet_size as usize));
        }

        Self {
            state: State::Connecting,
            params: params,

            watchdog_time: time::Instant::now(),
            meta_send_time: None,

            meta_queue: VecDeque::new(),
            event_queue: VecDeque::new(),

            connect_frame: connect_frame,
            connect_ack_received: false,
            connect_frame_remote: None,

            ping_rtt: rtt::PingRtt::new(),
            was_connected: false,
            disconnect_flush: false,

            channels: channels,
            frame_io: transfer::FrameIO::new(0, 0, 0),
        }
    }

    fn enqueue_meta(&mut self, frame: frame::Frame) {
        self.meta_queue.push_back(frame.to_bytes());
    }

    // State::Connecting
    fn verify_connect_info(&self, remote_info: &frame::Connect, local_info: &frame::Connect) -> bool {
        remote_info.num_channels == local_info.num_channels && remote_info.version == local_info.version
    }

    fn try_enter_connected(&mut self) {
        if self.connect_ack_received {
            if let Some(connect_frame) = &self.connect_frame_remote {
                let tx_sequence_id = connect_frame.sequence_id;
                let rx_sequence_id = self.connect_frame.sequence_id;
                let max_tx_bandwidth = self.params.max_tx_bandwidth.min(connect_frame.max_rx_bandwidth);
                self.connected_enter(tx_sequence_id, rx_sequence_id, max_tx_bandwidth as usize);
            }
        }
    }

    fn connecting_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::ConnectAck(connect_ack_frame) => {
                if connect_ack_frame.sequence_id == self.connect_frame.sequence_id {
                    // The remote peer acknowledged our connection request, stop sending more
                    self.connect_ack_received = true;
                    // Try to enter connected state
                    self.try_enter_connected();
                } else {
                    // The acknowledgement doesn't match our connection request id, fail lol
                    self.send_disconnect_enter();
                }
            }
            frame::Frame::Connect(connect_frame) => {
                if self.verify_connect_info(&connect_frame, &self.connect_frame) {
                    // Connection is possible, acknowledge request
                    self.enqueue_meta(frame::Frame::ConnectAck(frame::ConnectAck { sequence_id: connect_frame.sequence_id }));
                    // Save remote connection request
                    self.connect_frame_remote = Some(connect_frame);
                    // Try to enter connected state
                    self.try_enter_connected();
                } else {
                    // The connection is not possible
                    self.send_disconnect_enter();
                }
            }
            frame::Frame::Disconnect(_) => {
                // Peer must not have liked our connect info
                self.enqueue_meta(frame::Frame::DisconnectAck(frame::DisconnectAck { }));
                self.disconnected_enter();
            }
            _ => ()
        }
    }

    fn connecting_step(&mut self, now: time::Instant) {
        // Send connection requests until acked
        if !self.connect_ack_received {
            if self.meta_send_time.map_or(true, |time| now - time > CONNECT_INTERVAL) {
                self.meta_send_time = Some(now);
                self.enqueue_meta(frame::Frame::Connect(self.connect_frame.clone()));
            }
        }
    }

    // State::Connected
    fn connected_enter(&mut self, tx_sequence_id: u32, rx_sequence_id: u32, max_tx_bandwidth: usize) {
        self.state = State::Connected;
        self.watchdog_time = time::Instant::now();

        self.meta_send_time = None;
        self.was_connected = true;
        self.disconnect_flush = false;
        self.event_queue.push_back(Event::Connect);

        // TODO: This is icky, should an entire connected state object be created here?
        self.frame_io = transfer::FrameIO::new(tx_sequence_id, rx_sequence_id, max_tx_bandwidth);
    }

    fn connected_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::Connect(connect_frame) => {
                // In this state, we've already verified any connection parameters, just ack
                self.enqueue_meta(frame::Frame::ConnectAck(frame::ConnectAck { sequence_id: connect_frame.sequence_id }));
            }
            frame::Frame::Disconnect(_) => {
                // Welp
                println!("welp.");
                self.enqueue_meta(frame::Frame::DisconnectAck(frame::DisconnectAck { }));
                self.disconnected_enter();
            }
            frame::Frame::Ping(ping_frame) => {
                // A ping has been received, acknowledge
                self.enqueue_meta(frame::Frame::PingAck(frame::PingAck { sequence_id: ping_frame.sequence_id }));
            }
            frame::Frame::PingAck(ping_ack_frame) => {
                // Our ping has been acknowledged, update RTT and pet watchdog
                let now = time::Instant::now();
                self.ping_rtt.handle_ack(now, ping_ack_frame.sequence_id);
                self.watchdog_time = now;
            }
            frame::Frame::Data(data_frame) => {
                if let Some(entries) = self.frame_io.handle_data_frame(data_frame) {
                    for entry in entries.into_iter() {
                        if let Some(channel) = self.channels.get_mut(entry.channel_id as usize) {
                            match entry.message {
                                frame::Message::Datagram(dg) => {
                                    channel.rx.handle_datagram(dg);
                                }
                                frame::Message::WindowAck(wa) => {
                                    channel.tx.handle_window_ack(wa);
                                }
                            }
                        }
                    }
                }
            }
            frame::Frame::DataAck(data_ack_frame) => {
                self.frame_io.handle_data_ack_frame(data_ack_frame);
            }
            _ => ()
        }
    }

    fn connected_step(&mut self, now: time::Instant) {
        // Try to send a ping
        if self.meta_send_time.map_or(true, |time| now - time > PING_INTERVAL) {
            self.meta_send_time = Some(now);
            let sequence_id = self.ping_rtt.new_ping(now);
            self.enqueue_meta(frame::Frame::Ping(frame::Ping {
                sequence_id: sequence_id,
            }));
        }

        // Deliver packets from rx channels to application
        for channel in self.channels.iter_mut() {
            while let Some(packet) = channel.rx.receive() {
                self.event_queue.push_back(Event::Receive(packet));
            }
        }

        //self.frame_io.step(now);

        if self.disconnect_flush {
            // Disconnect if there's no pending data --the application is expected to stop calling send()!
            if self.frame_io.is_idle() && !self.channels.iter().any(|channel| !channel.tx.is_empty()) {
                self.send_disconnect_enter();
            }
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

    fn send_disconnect_step(&mut self, now: time::Instant) {
        if self.meta_send_time.map_or(true, |time| now - time > DISCONNECT_INTERVAL) {
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

    // State::Zombie
    fn zombie_enter(&mut self) {
        self.state = State::Zombie;

        self.event_queue.push_back(Event::Timeout);
    }

    pub fn handle_frame(&mut self, frame: frame::Frame) {
        match self.state {
            State::Connecting => self.connecting_handle_frame(frame),
            State::Connected => self.connected_handle_frame(frame),
            State::SendDisconnect => self.send_disconnect_handle_frame(frame),
            State::Disconnected => self.disconnected_handle_frame(frame),
            State::Zombie => (),
        }
    }

    pub fn step(&mut self) {
        let now = time::Instant::now();

        if self.state != State::Zombie {
            if now - self.watchdog_time > WATCHDOG_TIMEOUT {
                self.zombie_enter();
            }
        }

        match self.state {
            State::Connecting => self.connecting_step(now),
            State::Connected => self.connected_step(now),
            State::SendDisconnect => self.send_disconnect_step(now),
            State::Disconnected => (),
            State::Zombie => (),
        }
    }

    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        std::mem::take(&mut self.event_queue).into_iter()
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: ChannelId, mode: SendMode) {
        let channel = self.channels.get_mut(channel_id as usize).expect("No such channel");
        match self.state {
            State::Connecting | State::Connected => channel.tx.enqueue(data, mode),
            _ => (),
        }
    }

    fn flush_data(&mut self, now: time::Instant, sink: & dyn DataSink) {
        // Take pending messages from tx channels and enqueue them for delivery
        // TODO: Should this be round-robin?
        for (channel_id, channel) in self.channels.iter_mut().enumerate() {
            while let Some((datagram, is_reliable)) = channel.tx.try_send() {
                let data_entry = frame::DataEntry::new(channel_id as ChannelId, frame::Message::Datagram(datagram));
                self.frame_io.enqueue_datagram(data_entry, is_reliable, self.params.priority_channels.contains(&(channel_id as u32)));
            }
            if let Some(window_ack) = channel.rx.take_window_ack() {
                let data_entry = frame::DataEntry::new(channel_id as ChannelId, frame::Message::WindowAck(window_ack));
                self.frame_io.enqueue_datagram(data_entry, true, self.params.priority_channels.contains(&(channel_id as u32)));
            }
        }

        // Flush any pending frames using a resend timeout computed by the ping mechanism
        let timeout = time::Duration::from_millis(self.ping_rtt.rto_ms().round() as u64);

        self.frame_io.flush(now, timeout, sink);
    }

    fn flush_meta(&mut self, sink: & dyn DataSink) {
        for data in self.meta_queue.iter() {
            sink.send(&data);
        }
        self.meta_queue.clear();
    }

    pub fn flush(&mut self, sink: & dyn DataSink) {
        let now = time::Instant::now();
        self.flush_meta(sink);

        if self.state == State::Connected {
            self.flush_data(now, sink);
        }
    }

    pub fn disconnect(&mut self) {
        if self.state == State::Connected {
            self.disconnect_flush = true;
        }
    }

    pub fn is_zombie(&self) -> bool {
        self.state == State::Zombie
    }

    pub fn rtt_ms(&self) -> f64 {
        self.ping_rtt.rtt_ms()
    }
}

