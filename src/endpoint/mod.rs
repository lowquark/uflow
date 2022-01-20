
mod daten_meister;

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

#[derive(Clone,Debug)]
pub struct Params {
    pub tx_channels: usize,
    pub max_rx_alloc: usize,
    pub max_tx_bandwidth: usize,
    pub max_rx_bandwidth: usize,
}

impl Params {
    pub fn new() -> Self {
        Self {
            tx_channels: 1,
            max_rx_alloc: 1_000_000,
            max_tx_bandwidth: 10_000_000,
            max_rx_bandwidth: 10_000_000,
        }
    }

    pub fn tx_channels(mut self, tx_channels: usize) -> Params {
        self.tx_channels = tx_channels;
        self
    }

    pub fn max_rx_alloc(mut self, max_rx_alloc: usize) -> Params {
        self.max_rx_alloc = max_rx_alloc;
        self
    }

    pub fn max_tx_bandwidth(mut self, bandwidth: usize) -> Params {
        self.max_tx_bandwidth = bandwidth;
        self
    }

    pub fn max_rx_bandwidth(mut self, bandwidth: usize) -> Params {
        self.max_rx_bandwidth = bandwidth;
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
    max_tx_bandwidth: u32,
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

    tx_channels: usize,
    was_connected: bool,
}

impl Endpoint {
    pub fn new(params: Params) -> Self {
        assert!(params.tx_channels > 0, "Must have at least one channel");
        assert!(params.tx_channels <= MAX_CHANNELS, "Number of channels exceeds maximum");

        let connect_frame = frame::ConnectFrame {
            version: PROTOCOL_VERSION,
            tx_channels_sup: (params.tx_channels - 1) as u8,
            max_rx_alloc: params.max_rx_alloc.min(u32::MAX as usize) as u32,
            max_rx_bandwidth: params.max_rx_bandwidth.min(u32::MAX as usize) as u32,
            nonce: rand::random::<u32>(),
        };

        let initial_state = State::Connecting(ConnectingState {
            last_send_time: None,

            connect_frame,
            connect_ack_received: false,
            connect_frame_remote: None,

            initial_sends: VecDeque::new(),
            max_tx_bandwidth: params.max_tx_bandwidth.min(u32::MAX as usize) as u32,
        });

        Self {
            state: initial_state,

            event_queue: VecDeque::new(),

            watchdog_time: time::Instant::now(),

            tx_channels: params.tx_channels,
            was_connected: false,
        }
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        assert!(channel_id < self.tx_channels, "Channel ID exceeds maximum");

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
                                let max_tx_bandwidth = state.max_tx_bandwidth;
                                self.enter_connected(connect_frame_local, connect_frame_remote, initial_sends, max_tx_bandwidth);
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
                                let max_tx_bandwidth = state.max_tx_bandwidth;
                                self.enter_connected(connect_frame_local, frame, initial_sends, max_tx_bandwidth);
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
            State::Connecting(_) => {
            }
            State::Connected(ref mut state) => {
                state.daten_meister.receive(&mut EventPacketSink::new(&mut self.event_queue));
            }
            State::Disconnecting(_) => {
            }
            State::Disconnected => {
            }
            State::Zombie => {
            }
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

    pub fn rtt_ms(&self) -> f64 {
        todo!()
    }

    fn enter_connected(&mut self,
                       local: frame::ConnectFrame,
                       remote: frame::ConnectFrame,
                       initial_sends: VecDeque<SendEntry>,
                       max_tx_bandwidth: u32) {
        let tx_channels = local.tx_channels_sup as usize + 1;
        let rx_channels = remote.tx_channels_sup as usize + 1;
        let tx_alloc_limit = remote.max_rx_alloc as usize;
        let rx_alloc_limit = local.max_rx_alloc as usize;
        let tx_base_id = local.nonce;
        let rx_base_id = remote.nonce;
        let tx_bandwidth_limit = max_tx_bandwidth.min(remote.max_rx_bandwidth);

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

