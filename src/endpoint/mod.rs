
mod daten_meister;

use crate::MAX_PACKET_SIZE;
use crate::MAX_CHANNELS;
use crate::PROTOCOL_VERSION;
use crate::frame;
use crate::SendMode;
use crate::Event;

use frame::serial::Serialize;
use daten_meister::DatenMeister;

use rand;

use std::time;
use std::collections::VecDeque;

pub trait FrameSink {
    fn send(&mut self, frame_data: &[u8]);
}

/// Parameters used to configure either endpoint of a `uflow` connection.
#[derive(Clone,Debug)]
pub struct Config {
    /// The number of channels used to send packets.
    ///
    /// Must be greater than 0, and less than or equal to [`MAX_CHANNELS`](MAX_CHANNELS).
    ///
    /// *Note*: The number of channels used by an endpoint may differ from the opposing endpoint.
    pub channel_count: usize,

    /// The maximum send rate, in bytes per second. The endpoint will ensure that its outgoing
    /// bandwidth does not exceed this value.
    ///
    /// Must be greater than 0. Currently, rates larger than `2^32` are silently truncated.
    pub max_send_rate: usize,

    /// The maximum acceptable receive rate, in bytes per second. The opposing endpoint will ensure
    /// that its outgoing bandwidth does not exceed this value.
    ///
    /// Must be greater than 0. Currently, rates larger than `2^32` are silently truncated.
    pub max_receive_rate: usize,

    /// The maximum allocation size of the endpoint's receive queue, in bytes.
    ///
    /// Must be greater than 0. This value is silently rounded up to the nearest multiple of
    /// [`MAX_FRAGMENT_SIZE`](crate::MAX_FRAGMENT_SIZE).
    // TODO: Actually do this ^
    ///
    /// Received packets contribute to the receive queue's allocation size according to their
    /// allocation size. If the allocation size of a received packet would cause the receive queue
    /// to exceed its maximum allocation size, that packet will be silently ignored. (This
    /// mechanism prevents a memory allocation attack; a well-behaved sender will ensure that the
    /// receiver's allocation limit is not exceeded.)
    ///
    /// A packet's allocation size is given by the packet's size if it is less than or equal to
    /// [`MAX_FRAGMENT_SIZE`](crate::MAX_FRAGMENT_SIZE). Otherwise, the packet's allocation size is
    /// the smallest multiple of [`MAX_FRAGMENT_SIZE`](crate::MAX_FRAGMENT_SIZE) which contains the
    /// packet.
    ///
    /// *Note*: The maximum allocation size necessarily constrains the maximum deliverable packet
    /// size. Currently, any packets which would exceed the receiver's maximum allocation size are
    /// silently discarded!
    pub max_receive_alloc: usize

    // The dilemma above can be solved by forbidding connections for which:
    //
    //      receiver.max_receive_alloc < sender.min_receiver_alloc ,
    //
    // where min_receiver_alloc is some minimum expected receive allocation. If the sender adheres
    // to its own size restriction, i.e.:
    //
    //      packet_size <= sender.min_receiver_alloc ,
    //
    // then it can be seen that:
    //
    //      packet_size <= receiver.max_receive_alloc ,
    //
    // and that all sent packets may be delivered. In addition, an immediate error can be generated
    // when an application attempts to send a packet with a size beyond its own, self-imposed
    // limitation. (If an error was generated based on the receiver's size limitation, crashing the
    // remote endpoint would be trivial.)

    // In the event that an initial packet is dropped, the transfer window will stall, preventing
    // the sender's allocation counter from decreasing until that packet has been received or
    // skipped. The larger the receiver's allocation limit is, the more packets may be sent and
    // delivered in spite of such a stall. Thus, having a larger receiver allocation limit than is
    // stricly necessary, i.e.
    //
    //      receiver.max_receive_alloc > sender.min_receiver_alloc ,
    //
    // still benefits the connection in a way that is adjustable to the receiver.

    // So, TODO:
    //pub min_receiver_alloc: usize,
    // or:
    //pub min_send_alloc: usize,
    // better:
    //pub max_packet_size: usize,
}

impl Default for Config {
    /// Creates an endpoint configuration with the following parameters:
    ///   * Number of channels: 1
    ///   * Maximum outgoing bandwidth: 10MB/s
    ///   * Maximum incoming bandwidth: 10MB/s
    ///   * Maximum receive allocation: 1MB
    fn default() -> Self {
        Self {
            channel_count: 1,
            max_send_rate: 10_000_000,
            max_receive_rate: 10_000_000,
            max_receive_alloc: 1_000_000,
        }
    }
}

impl Config {
    /// Sets `channel_count` to the provided value.
    pub fn channel_count(mut self, channel_count: usize) -> Config {
        self.channel_count = channel_count;
        self
    }

    /// Sets `max_send_rate` to the provided value.
    pub fn max_send_rate(mut self, rate: usize) -> Config {
        self.max_send_rate = rate;
        self
    }

    /// Sets `max_receive_rate` to the provided value.
    pub fn max_receive_rate(mut self, rate: usize) -> Config {
        self.max_receive_rate = rate;
        self
    }

    /// Sets `max_receive_alloc` to the provided value.
    pub fn max_receive_alloc(mut self, max_receive_alloc: usize) -> Config {
        self.max_receive_alloc = max_receive_alloc;
        self
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
    daten_meister: DatenMeister,

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

static CONNECT_INTERVAL: time::Duration = time::Duration::from_millis(500);
static DISCONNECT_INTERVAL: time::Duration = time::Duration::from_millis(500);

static CONNECTING_WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(10000);
static CONNECTED_WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(10000);
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
    fn send(&mut self, packet_data: Box<[u8]>, channel_id: u8) {
        self.event_queue.push_back(Event::Receive(packet_data, channel_id as usize));
    }
}

pub struct Endpoint {
    state: State,

    event_queue: VecDeque<Event>,

    watchdog_time: time::Instant,

    channel_count: usize,
    was_connected: bool,
}

impl Endpoint {
    pub fn new(cfg: Config) -> Self {
        assert!(cfg.channel_count > 0, "Must have at least one channel");
        assert!(cfg.channel_count <= MAX_CHANNELS, "Number of channels exceeds maximum");

        let connect_frame = frame::ConnectFrame {
            version: PROTOCOL_VERSION,
            tx_channels_sup: (cfg.channel_count - 1) as u8,
            max_rx_alloc: cfg.max_receive_alloc.min(u32::MAX as usize) as u32,
            max_rx_bandwidth: cfg.max_receive_rate.min(u32::MAX as usize) as u32,
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

            channel_count: cfg.channel_count,
            was_connected: false,
        }
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        assert!(data.len() < MAX_PACKET_SIZE, "Packet size exceeds maximum");
        assert!(channel_id < self.channel_count, "Channel ID exceeds maximum");

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

    fn validate_handshake(remote_info: &frame::ConnectFrame) -> bool {
        remote_info.tx_channels_sup as usize <= MAX_CHANNELS - 1 &&
        remote_info.version == PROTOCOL_VERSION
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
                        if Self::validate_handshake(&frame) {
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

    pub fn flush(&mut self, sink: &mut impl FrameSink) {
        let now = time::Instant::now();

        match self.state {
            State::Connecting(ref mut state) => {
                if now - self.watchdog_time > CONNECTING_WATCHDOG_TIMEOUT {
                    self.enter_zombie();
                    return;
                }

                if !state.connect_ack_received {
                    if state.last_send_time.map_or(true, |time| now - time > CONNECT_INTERVAL) {
                        state.last_send_time = Some(now);
                        sink.send(&frame::Frame::ConnectFrame(state.connect_frame.clone()).write());
                    }
                }
            }
            State::Connected(ref mut state) => {
                if now - self.watchdog_time > CONNECTED_WATCHDOG_TIMEOUT {
                    self.enter_zombie();
                    return;
                }

                state.daten_meister.flush(sink);

                if state.disconnect_flush {
                    if !state.daten_meister.is_send_pending() {
                        self.enter_disconnecting();
                    }
                }
            }
            State::Disconnecting(ref mut state) => {
                if now - self.watchdog_time > DISCONNECTING_WATCHDOG_TIMEOUT {
                    self.enter_zombie();
                    return;
                }

                if state.last_send_time.map_or(true, |time| now - time > DISCONNECT_INTERVAL) {
                    state.last_send_time = Some(now);
                    sink.send(&frame::Frame::DisconnectFrame(frame::DisconnectFrame { }).write());
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

    fn enter_connected(&mut self,
                       local: frame::ConnectFrame,
                       remote: frame::ConnectFrame,
                       initial_sends: VecDeque<SendEntry>,
                       max_send_rate: u32) {
        let tx_channels = local.tx_channels_sup as usize + 1;
        let rx_channels = remote.tx_channels_sup as usize + 1;
        let tx_alloc_limit = remote.max_rx_alloc as usize;
        let rx_alloc_limit = local.max_rx_alloc as usize;
        let tx_base_id = local.nonce;
        let rx_base_id = remote.nonce;
        let tx_bandwidth_limit = max_send_rate.min(remote.max_rx_bandwidth);

        let mut daten_meister = DatenMeister::new(tx_channels, rx_channels,
                                                  tx_alloc_limit, rx_alloc_limit,
                                                  tx_base_id, rx_base_id,
                                                  tx_bandwidth_limit);

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

