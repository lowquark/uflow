
use std::collections::VecDeque;
use std::time;
use std::ops::Range;

use super::frame;
use super::channel;
use super::ChannelId;
use super::DataSink;
use super::MAX_CHANNELS;
use super::SendMode;
use super::transport;

#[derive(Clone,Debug)]
pub struct Params {
    pub num_channels: u32,
    pub min_tx_bandwidth: u32,
    pub max_tx_bandwidth: u32,
    pub max_rx_bandwidth: u32,
    pub priority_channels: Range<u32>,
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

#[derive(Clone,Debug,PartialEq)]
pub enum Event {
    Connect,
    Disconnect,
    Receive(Box<[u8]>),
    Timeout,
}

struct Ping {
    sequence_id: u16,
    instant: time::Instant,
}

struct PingRtt {
    ping_history: Vec<Ping>,
    next_sequence_id: u16,
    srtt_ms: f64,
    sdrtt_ms: f64,
    ping_received: bool,
}

impl PingRtt {
    const DEFAULT_RTT_MS: f64 = 100.0;
    const PING_TIMEOUT_MS: u64 = 3000;
    const RTT_SMOOTH_ALPHA: f64 = 0.875;
    const DRTT_SMOOTH_ALPHA: f64 = 0.875;
    const RTO_SDRTT_U: f64 = 4.0;

    pub fn new() -> Self {
        Self {
            ping_history: Vec::new(),
            next_sequence_id: 0,
            srtt_ms: Self::DEFAULT_RTT_MS,
            sdrtt_ms: 0.0,
            ping_received: false,
        }
    }

    fn update_rtt(&mut self, rtt_ms: f64) {
        if !self.ping_received {
            self.ping_received = true;
            self.srtt_ms = rtt_ms;
            self.sdrtt_ms = 0.0;
        } else {
            self.srtt_ms = Self::RTT_SMOOTH_ALPHA*self.srtt_ms + (1.0 - Self::RTT_SMOOTH_ALPHA)*rtt_ms;
            self.sdrtt_ms = Self::DRTT_SMOOTH_ALPHA*self.sdrtt_ms + (1.0 - Self::DRTT_SMOOTH_ALPHA)*(rtt_ms - self.srtt_ms).abs();
        }
    }

    pub fn new_ping(&mut self, now: time::Instant) -> u16 {
        let sequence_id = self.next_sequence_id;
        self.ping_history.push(Ping { sequence_id: sequence_id, instant: now } );
        self.next_sequence_id = self.next_sequence_id.wrapping_add(1);
        sequence_id
    }

    pub fn handle_ack(&mut self, now: time::Instant, sequence_id: u16) {
        for (idx, ping) in self.ping_history.iter().enumerate() {
            if sequence_id == ping.sequence_id {
                let rtt_ms = (now - ping.instant).as_secs_f64()*1_000.0;
                self.ping_history.remove(idx);
                self.update_rtt(rtt_ms);
                break;
            }
        }

        self.ping_history.retain(|ping| (now - ping.instant).as_millis() < Self::PING_TIMEOUT_MS as u128);
    }

    pub fn rtt_ms(&self) -> f64 {
        self.srtt_ms
    }

    pub fn rto_ms(&self) -> f64 {
        self.srtt_ms + Self::RTO_SDRTT_U*self.sdrtt_ms
    }
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

    watchdog_time: time::Instant,
    meta_send_time: Option<time::Instant>,

    ping_rtt: PingRtt,

    // Connect, Disconnect, Ping
    meta_queue: VecDeque<Box<[u8]>>,
    event_queue: VecDeque<Event>,

    was_connected: bool,
    disconnect_flush: bool,

    channels: Vec<channel::Channel>,
    priority_channels: Range<u32>,
    frame_io: transport::FrameIO,
}

impl Peer {
    fn new(params: Params) -> Self {
        let mut channels = Vec::new();

        assert!(params.num_channels <= MAX_CHANNELS, "Number of channels exceeds maximum");
        for _ in 0..params.num_channels {
            channels.push(channel::Channel::new());
        }

        Self {
            state: State::Zombie,

            watchdog_time: time::Instant::now(),
            meta_send_time: None,

            ping_rtt: PingRtt::new(),

            meta_queue: VecDeque::new(),
            event_queue: VecDeque::new(),

            was_connected: false,
            disconnect_flush: false,

            channels: channels,
            priority_channels: params.priority_channels,
            frame_io: transport::FrameIO::new(),
        }
    }

    pub fn new_passive(params: Params) -> Self {
        let mut peer = Self::new(params);
        peer.await_connect_enter();
        peer
    }

    pub fn new_active(params: Params) -> Self {
        let mut peer = Self::new(params);
        peer.send_connect_enter();
        peer
    }

    fn enqueue_meta(&mut self, frame: frame::Frame) {
        self.meta_queue.push_back(frame.to_bytes());
    }

    fn verify_connect_info(&self, _remote_info: frame::Connect) -> bool {
        true
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
                if self.verify_connect_info(frame) {
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
            frame::Frame::ConnectAck(_) => {
                // TODO: Exchange random connection ids
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

    fn await_connect_ack_step(&mut self, now: time::Instant) {
        if self.meta_send_time.map_or(true, |time| now - time > CONNECT_INTERVAL) {
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
                // TODO: Exchange random connection ids
                if self.verify_connect_info(frame) {
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

    fn send_connect_step(&mut self, now: time::Instant) {
        if self.meta_send_time.map_or(true, |time| now - time > CONNECT_INTERVAL) {
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

        self.meta_send_time = None;
        self.was_connected = true;
        self.disconnect_flush = false;
        self.event_queue.push_back(Event::Connect);
    }

    fn connected_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::Connect(_) => {
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
                self.frame_io.acknowledge_data_frame(&data_frame);
                for entry in data_frame.entries.into_iter() {
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
            frame::Frame::DataAck(data_ack_frame) => {
                self.frame_io.handle_data_ack(data_ack_frame);
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

        self.frame_io.step(now);

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
            if now - self.watchdog_time > WATCHDOG_TIMEOUT {
                self.zombie_enter();
            }
        }

        match self.state {
            State::AwaitConnect => (),
            State::AwaitConnectAck => self.await_connect_ack_step(now),
            State::SendConnect => self.send_connect_step(now),
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
            State::AwaitConnect |
            State::AwaitConnectAck |
            State::SendConnect |
            State::Connected => channel.tx.enqueue(data, mode),
            _ => (),
        }
    }

    fn flush_data(&mut self, now: time::Instant, sink: & dyn DataSink) {
        // Take pending messages from tx channels and enqueue them for delivery
        // TODO: Should this be round-robin?
        for (channel_id, channel) in self.channels.iter_mut().enumerate() {
            while let Some((datagram, is_reliable)) = channel.tx.try_send() {
                let data_entry = frame::DataEntry::new(channel_id as ChannelId, frame::Message::Datagram(datagram));
                self.frame_io.enqueue_datagram(data_entry, is_reliable, self.priority_channels.contains(&(channel_id as u32)));
            }
            if let Some(window_ack) = channel.rx.take_window_ack() {
                let data_entry = frame::DataEntry::new(channel_id as ChannelId, frame::Message::WindowAck(window_ack));
                self.frame_io.enqueue_datagram(data_entry, true, self.priority_channels.contains(&(channel_id as u32)));
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

