
use crate::CHANNEL_COUNT;
use crate::Event;
use crate::MAX_FRAME_WINDOW_SIZE;
use crate::MAX_PACKET_SIZE;
use crate::MAX_PACKET_WINDOW_SIZE;
use crate::PROTOCOL_VERSION;
use crate::SendMode;
use crate::frame;

use frame::serial::Serialize;

use rand;

use std::time;
use std::collections::VecDeque;

mod daten_meister;

pub trait FrameSink {
    fn send(&mut self, frame_data: &[u8]);
}

/// Parameters used to configure either endpoint of a `uflow` connection.
#[derive(Clone,Debug)]
pub struct Config {
    /// The maximum send rate, in bytes per second. The endpoint will ensure that its outgoing
    /// bandwidth does not exceed this value.
    ///
    /// Must be greater than 0. Values larger than 2^32 will be truncated.
    pub max_send_rate: usize,

    /// The maximum acceptable receive rate, in bytes per second. The opposing endpoint will ensure
    /// that its outgoing bandwidth does not exceed this value.
    ///
    /// Must be greater than 0. Values larger than 2^32 will be truncated.
    pub max_receive_rate: usize,

    /// The maximum size of a sent packet, in bytes. The endpoint will ensure that it does not send
    /// packets with a size exceeding this value.
    ///
    /// Must be greater than 0, and less than or equal to [`MAX_PACKET_SIZE`].
    pub max_packet_size: usize,

    /// The maximum allocation size of the endpoint's receive buffer, in bytes. The endpoint will
    /// ensure that the total amount of memory allocated to receive packet data doesn't exceed this
    /// value, rounded up to the nearest multiple of
    /// [`MAX_FRAGMENT_SIZE`](crate::MAX_FRAGMENT_SIZE).
    ///
    /// Must be greater than 0.
    ///
    /// *Note*: The maximum allocation size necessarily constrains the maximum receivable packet
    /// size. A connection attempt will fail if the `max_packet_size` of the opposing endpoint
    /// exceeds this value.
    pub max_receive_alloc: usize,

    /// Whether the endpoint should automatically send keepalive frames if no data has been sent
    /// for one keepalive interval (currently 5 seconds). If set to false, the connection will time
    /// out if either endpoint does not send data for one timeout interval (currently 20 seconds).
    pub keepalive: bool,
}

impl Default for Config {
    /// Creates an endpoint configuration with the following parameters:
    ///   * Maximum outgoing bandwidth: 2MB/s
    ///   * Maximum incoming bandwidth: 2MB/s
    ///   * Maximum packet size: 1MB
    ///   * Maximum packet receive allocation: 1MB
    ///   * Keepalive: true
    fn default() -> Self {
        Self {
            max_send_rate: 2_000_000,
            max_receive_rate: 2_000_000,

            max_packet_size: 1_000_000,
            max_receive_alloc: 1_000_000,

            keepalive: true,
        }
    }
}

impl Config {
    /// Returns `true` if each parameter has a valid value.
    pub fn is_valid(&self) -> bool {
        self.max_send_rate > 0 &&
        self.max_receive_rate > 0 &&
        self.max_packet_size > 0 &&
        self.max_packet_size <= MAX_PACKET_SIZE &&
        self.max_receive_alloc > 0
    }
}

#[derive(Debug)]
struct SendEntry {
    data: Box<[u8]>,
    channel_id: u8,
    mode: SendMode,
}

struct ConnectingState {
    last_send_time: Option<time::Instant>,

    connect_frame: frame::ConnectFrame,
    connect_ack_received: bool,
    connect_frame_remote: Option<frame::ConnectFrame>,

    initial_sends: VecDeque<SendEntry>,
    max_send_rate: u32,
}

struct ConnectedState {
    daten_meister: daten_meister::DatenMeister,

    disconnect_flush: bool,
}

struct DisconnectingState {
    last_send_time: Option<time::Instant>,
}

enum State {
    Connecting(ConnectingState),
    Connected(ConnectedState),
    Disconnecting(DisconnectingState),
    Disconnected,
    Zombie,
}

static CONNECT_INTERVAL: time::Duration = time::Duration::from_millis(1000);
static DISCONNECT_INTERVAL: time::Duration = time::Duration::from_millis(1000);

static CONNECTING_WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(10000);
static CONNECTED_WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(20000);
static DISCONNECTING_WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(3000);
static DISCONNECTED_WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(10000);

struct EventPacketSink<'a> {
    event_queue: &'a mut VecDeque::<Event>,
}

impl<'a> EventPacketSink<'a> {
    fn new(event_queue: &'a mut VecDeque::<Event>) -> Self {
        Self {
            event_queue
        }
    }
}

impl<'a> daten_meister::PacketSink for EventPacketSink<'a> {
    fn send(&mut self, packet_data: Box<[u8]>) {
        self.event_queue.push_back(Event::Receive(packet_data));
    }
}

pub struct Endpoint {
    state: State,

    event_queue: VecDeque<Event>,

    watchdog_time: time::Instant,

    max_packet_size: usize,
    keepalive: bool,
    was_connected: bool,
}

impl Endpoint {
    pub fn new(cfg: Config) -> Self {
        debug_assert!(cfg.is_valid());

        let connect_frame = frame::ConnectFrame {
            version: PROTOCOL_VERSION,

            max_receive_rate: cfg.max_receive_rate.min(u32::MAX as usize) as u32,

            max_packet_size: cfg.max_packet_size.min(u32::MAX as usize) as u32,
            max_receive_alloc: cfg.max_receive_alloc.min(u32::MAX as usize) as u32,

            nonce: rand::random::<u32>(),
        };

        let initial_state = State::Connecting(ConnectingState {
            last_send_time: None,

            connect_frame,
            connect_ack_received: false,
            connect_frame_remote: None,

            initial_sends: VecDeque::new(),
            max_send_rate: cfg.max_send_rate.min(u32::MAX as usize) as u32,
        });

        Self {
            state: initial_state,

            event_queue: VecDeque::new(),

            watchdog_time: time::Instant::now(),

            max_packet_size: cfg.max_packet_size,
            keepalive: cfg.keepalive,

            was_connected: false,
        }
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        assert!(data.len() <= self.max_packet_size,
                "send failed: packet of size {} exceeds configured maximum of {}",
                data.len(),
                self.max_packet_size);

        assert!((channel_id as usize) < CHANNEL_COUNT,
                "send failed: channel ID {} is invalid",
                channel_id);

        match self.state {
            State::Connecting(ref mut state) => {
                state.initial_sends.push_back(SendEntry { data, channel_id: channel_id as u8, mode });
            }
            State::Connected(ref mut state) => {
                state.daten_meister.send(data, channel_id as u8, mode);
            }
            _ => (),
        }
    }

    pub fn disconnect(&mut self) {
        match self.state {
            State::Connecting(_) => {
                self.enter_disconnecting();
            }
            State::Connected(ref mut state) => {
                state.disconnect_flush = true;
            }
            State::Disconnecting(_) => {
            }
            State::Disconnected => {
            }
            State::Zombie => {
            }
        }
    }

    pub fn disconnect_now(&mut self) {
        match self.state {
            State::Connecting(_) => {
                self.enter_disconnected();
            }
            State::Connected(_) => {
                self.enter_disconnected();
            }
            State::Disconnecting(_) => {
                self.enter_disconnected();
            }
            State::Disconnected => {
            }
            State::Zombie => {
            }
        }
    }

    fn validate_handshake(connect_frame_remote: &frame::ConnectFrame, max_packet_size: usize) -> bool {
        connect_frame_remote.version == PROTOCOL_VERSION &&
        connect_frame_remote.max_receive_alloc as usize >= max_packet_size
    }

    pub fn handle_frame(&mut self, frame: frame::Frame, sink: &mut impl FrameSink) {
        match self.state {
            State::Connecting(ref mut state) => {
                self.watchdog_time = time::Instant::now();

                match frame {
                    frame::Frame::ConnectAckFrame(frame) => {
                        if frame.nonce == state.connect_frame.nonce {
                            // The remote endpoint acknowledged our connection request, stop sending more
                            state.connect_ack_received = true;
                            // Try to enter connected state
                            if let Some(connect_frame_remote) = state.connect_frame_remote.take() {
                                let initial_sends = std::mem::take(&mut state.initial_sends);
                                let connect_frame_local = state.connect_frame.clone();
                                let max_send_rate = state.max_send_rate;
                                self.enter_connected(connect_frame_local, connect_frame_remote, initial_sends, max_send_rate);
                            }
                        } else {
                            // The acknowledgement doesn't match our connection request id, fail lol
                            self.enter_disconnected();
                        }
                    }
                    frame::Frame::ConnectFrame(frame) => {
                        if Self::validate_handshake(&frame, self.max_packet_size) {
                            // Connection is possible, acknowledge request
                            sink.send(&frame::Frame::ConnectAckFrame(frame::ConnectAckFrame { nonce: frame.nonce }).write());
                            // Try to enter connected state
                            if state.connect_ack_received {
                                let initial_sends = std::mem::take(&mut state.initial_sends);
                                let connect_frame_local = state.connect_frame.clone();
                                let max_send_rate = state.max_send_rate;
                                self.enter_connected(connect_frame_local, frame, initial_sends, max_send_rate);
                            } else {
                                // Wait for ack
                                state.connect_frame_remote = Some(frame);
                            }
                        } else {
                            // The connection is not possible
                            self.enter_disconnected();
                        }
                    }
                    frame::Frame::DisconnectFrame(_) => {
                        // Remote endpoint must not have liked our connect info
                        sink.send(&frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame { }).write());
                        self.enter_disconnected();
                    }
                    _ => ()
                }
            }
            State::Connected(ref mut state) => {
                self.watchdog_time = time::Instant::now();

                match frame {
                    frame::Frame::ConnectFrame(connect_frame) => {
                        // In this state, we've already verified any connection parameters, just ack
                        sink.send(&frame::Frame::ConnectAckFrame(frame::ConnectAckFrame { nonce: connect_frame.nonce }).write());
                    }
                    frame::Frame::DisconnectFrame(_) => {
                        // Welp
                        sink.send(&frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame { }).write());

                        // Receive as many pending packets as possible
                        state.daten_meister.receive(&mut EventPacketSink::new(&mut self.event_queue));

                        self.enter_disconnected();
                    }
                    frame::Frame::DataFrame(data_frame) => {
                        state.daten_meister.handle_data_frame(data_frame);
                    }
                    frame::Frame::SyncFrame(sync_frame) => {
                        state.daten_meister.handle_sync_frame(sync_frame);
                    }
                    frame::Frame::AckFrame(ack_frame) => {
                        state.daten_meister.handle_ack_frame(ack_frame);
                    }
                    _ => ()
                }
            }
            State::Disconnecting(_) => {
                match frame {
                    frame::Frame::DisconnectFrame(_) => {
                        // The remote is also disconnecting
                        sink.send(&frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame { }).write());
                        self.enter_disconnected();
                    }
                    frame::Frame::DisconnectAckFrame(_) => {
                        // Our disconnect has been received
                        self.enter_disconnected();
                    }
                    _ => ()
                }
            }
            State::Disconnected => {
            }
            State::Zombie => {
            }
        }
    }

    pub fn step(&mut self) {
        let now = time::Instant::now();

        match self.state {
            State::Connecting(_) => {
                if now - self.watchdog_time > CONNECTING_WATCHDOG_TIMEOUT {
                    self.enter_zombie();
                    return;
                }
            }
            State::Connected(ref mut state) => {
                state.daten_meister.step();

                if now - self.watchdog_time > CONNECTED_WATCHDOG_TIMEOUT {
                    self.enter_zombie();
                    return;
                }
            }
            State::Disconnecting(_) => {
                if now - self.watchdog_time > DISCONNECTING_WATCHDOG_TIMEOUT {
                    self.enter_zombie();
                    return;
                }
            }
            State::Disconnected => {
                if now - self.watchdog_time > DISCONNECTED_WATCHDOG_TIMEOUT {
                    self.enter_zombie();
                    return;
                }
            }
            State::Zombie => {
            }
        }
    }

    pub fn flush(&mut self, sink: &mut impl FrameSink) {
        let now = time::Instant::now();

        match self.state {
            State::Connecting(ref mut state) => {
                if !state.connect_ack_received {
                    if state.last_send_time.map_or(true, |time| now - time > CONNECT_INTERVAL) {
                        state.last_send_time = Some(now);
                        sink.send(&frame::Frame::ConnectFrame(state.connect_frame.clone()).write());
                    }
                }
            }
            State::Connected(ref mut state) => {
                state.daten_meister.flush(sink);

                if state.disconnect_flush {
                    if !state.daten_meister.is_send_pending() {
                        self.enter_disconnecting();
                        return;
                    }
                }
            }
            State::Disconnecting(ref mut state) => {
                if state.last_send_time.map_or(true, |time| now - time > DISCONNECT_INTERVAL) {
                    state.last_send_time = Some(now);
                    sink.send(&frame::Frame::DisconnectFrame(frame::DisconnectFrame { }).write());
                }
            }
            _ => (),
        }
    }

    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        match self.state {
            State::Connected(ref mut state) => {
                state.daten_meister.receive(&mut EventPacketSink::new(&mut self.event_queue));
            }
            _ => ()
        }

        std::mem::take(&mut self.event_queue).into_iter()
    }

    pub fn is_disconnected(&self) -> bool {
        match self.state {
            State::Disconnected => true,
            _ => false,
        }
    }

    pub fn is_zombie(&self) -> bool {
        match self.state {
            State::Zombie => true,
            _ => false,
        }
    }

    pub fn rtt_s(&self) -> Option<f64> {
        match self.state {
            State::Connected(ref state) => state.daten_meister.rtt_s(),
            _ => None
        }
    }

    pub fn pending_send_size(&self) -> usize {
        match self.state {
            State::Connected(ref state) => state.daten_meister.pending_send_size(),
            _ => 0
        }
    }

    fn enter_connected(&mut self,
                       local: frame::ConnectFrame,
                       remote: frame::ConnectFrame,
                       initial_sends: VecDeque<SendEntry>,
                       max_send_rate: u32) {
        use crate::packet_id;

        let config = daten_meister::Config {
            tx_frame_window_size: MAX_FRAME_WINDOW_SIZE,
            rx_frame_window_size: MAX_FRAME_WINDOW_SIZE,

            tx_frame_base_id: local.nonce,
            rx_frame_base_id: remote.nonce,

            tx_packet_window_size: MAX_PACKET_WINDOW_SIZE,
            rx_packet_window_size: MAX_PACKET_WINDOW_SIZE,

            tx_packet_base_id: local.nonce & packet_id::MASK,
            rx_packet_base_id: remote.nonce & packet_id::MASK,

            tx_bandwidth_limit: max_send_rate.min(remote.max_receive_rate),

            tx_alloc_limit: remote.max_receive_alloc as usize,
            rx_alloc_limit: local.max_receive_alloc as usize,

            keepalive: self.keepalive,
        };

        let mut daten_meister = daten_meister::DatenMeister::new(config);

        for send in initial_sends.into_iter() {
            daten_meister.send(send.data, send.channel_id, send.mode);
        }

        self.state = State::Connected(ConnectedState {
            daten_meister,
            disconnect_flush: false,
        });

        self.event_queue.push_back(Event::Connect);

        self.was_connected = true;
    }

    fn enter_disconnecting(&mut self) {
        self.state = State::Disconnecting(DisconnectingState {
            last_send_time: None,
        });
    }

    fn enter_disconnected(&mut self) {
        self.state = State::Disconnected;

        if self.was_connected {
            self.event_queue.push_back(Event::Disconnect);
        }
    }

    fn enter_zombie(&mut self) {
        self.state = State::Zombie;

        self.event_queue.push_back(Event::Timeout);
    }
}

