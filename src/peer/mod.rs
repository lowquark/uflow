
mod daten_meister;

use std::collections::VecDeque;
use std::time;
use std::ops::Range;

use crate::frame;
use crate::ChannelId;
use crate::DataSink;
use crate::MAX_CHANNELS;
use crate::PROTOCOL_VERSION;
use crate::SendMode;

use frame::Serialize;

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
    Connecting,
    Connected,
    Disconnecting,
    Disconnected,
    Zombie,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Event {
    Connect,
    Disconnect,
    Receive(Box<[u8]>, ChannelId),
    Timeout,
}

const CONNECT_INTERVAL_MS: u64 = 500;
const DISCONNECT_INTERVAL_MS: u64 = 500;
const WATCHDOG_TIMEOUT_MS: u64 = 20000;

static CONNECT_INTERVAL: time::Duration = time::Duration::from_millis(CONNECT_INTERVAL_MS);
static DISCONNECT_INTERVAL: time::Duration = time::Duration::from_millis(DISCONNECT_INTERVAL_MS);
static WATCHDOG_TIMEOUT: time::Duration = time::Duration::from_millis(WATCHDOG_TIMEOUT_MS);

pub struct Peer {
    state: State,
    params: Params,

    watchdog_time: time::Instant,
    meta_send_time: Option<time::Instant>,

    // Connect, Disconnect
    meta_queue: VecDeque<Box<[u8]>>,
    event_queue: VecDeque<Event>,

    // Connecting stuff
    connect_frame: frame::ConnectFrame,
    connect_ack_received: bool,
    connect_frame_remote: Option<frame::ConnectFrame>,

    // Connected stuff
    was_connected: bool,
    disconnect_flush: bool,
}

impl Peer {
    pub fn new(params: Params) -> Self {
        assert!(params.num_channels > 0, "Must have at least one channel");
        assert!(params.num_channels <= MAX_CHANNELS as u32, "Number of channels exceeds maximum");

        let connect_frame = frame::ConnectFrame {
            version: PROTOCOL_VERSION,
            num_channels: (params.num_channels - 1) as u8,
            max_rx_bandwidth: params.max_rx_bandwidth,
            nonce: rand::random::<u32>(),
        };

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

            was_connected: false,
            disconnect_flush: false,
        }
    }

    fn enqueue_meta(&mut self, frame: frame::Frame) {
        self.meta_queue.push_back(frame.write());
    }

    // State::Connecting
    fn verify_connect_info(&self, remote_info: &frame::ConnectFrame, local_info: &frame::ConnectFrame) -> bool {
        remote_info.num_channels == local_info.num_channels && remote_info.version == local_info.version
    }

    fn try_enter_connected(&mut self) {
        if self.connect_ack_received {
            if let Some(connect_frame) = &self.connect_frame_remote {
                let tx_sequence_id = connect_frame.nonce;
                let rx_sequence_id = self.connect_frame.nonce;
                let max_tx_bandwidth = self.params.max_tx_bandwidth.min(connect_frame.max_rx_bandwidth);
                self.connected_enter(tx_sequence_id, rx_sequence_id, max_tx_bandwidth as usize);
            }
        }
    }

    fn connecting_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::ConnectAckFrame(connect_ack_frame) => {
                if connect_ack_frame.nonce == self.connect_frame.nonce {
                    // The remote peer acknowledged our connection request, stop sending more
                    self.connect_ack_received = true;
                    // Try to enter connected state
                    self.try_enter_connected();
                } else {
                    // The acknowledgement doesn't match our connection request id, fail lol
                    self.disconnected_enter();
                }
            }
            frame::Frame::ConnectFrame(connect_frame) => {
                if self.verify_connect_info(&connect_frame, &self.connect_frame) {
                    // Connection is possible, acknowledge request
                    self.enqueue_meta(frame::Frame::ConnectAckFrame(frame::ConnectAckFrame { nonce: connect_frame.nonce }));
                    // Save remote connection request
                    self.connect_frame_remote = Some(connect_frame);
                    // Try to enter connected state
                    self.try_enter_connected();
                } else {
                    // The connection is not possible
                    self.disconnected_enter();
                }
            }
            frame::Frame::DisconnectFrame(_) => {
                // Peer must not have liked our connect info
                self.enqueue_meta(frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame { }));
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
                self.enqueue_meta(frame::Frame::ConnectFrame(self.connect_frame.clone()));
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

        // TODO: Create DatenMeister
    }

    fn connected_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::ConnectFrame(connect_frame) => {
                // In this state, we've already verified any connection parameters, just ack
                self.enqueue_meta(frame::Frame::ConnectAckFrame(frame::ConnectAckFrame { nonce: connect_frame.nonce }));
            }
            frame::Frame::DisconnectFrame(_) => {
                // Welp
                self.enqueue_meta(frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame { }));
                self.disconnected_enter();
            }
            frame::Frame::MessageFrame(message_frame) => {
            }
            _ => ()
        }
    }

    fn connected_step(&mut self, now: time::Instant) {
        if self.disconnect_flush {
            // Disconnect if there's no pending data --the application is expected to stop calling send()!
            /*
            if self.frame_io.is_idle() && !self.channels.iter().any(|channel| !channel.tx.is_empty()) {
                self.disconnecting_enter();
            }
            */
        }
    }

    // State::Disconnecting
    fn disconnecting_enter(&mut self) {
        self.state = State::Disconnecting;
        self.watchdog_time = time::Instant::now();

        self.meta_send_time = None;
    }

    fn disconnecting_handle_frame(&mut self, frame: frame::Frame) {
        match frame {
            frame::Frame::DisconnectFrame(_) => {
                // The remote is also disconnecting
                self.enqueue_meta(frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame { }));
                self.disconnected_enter();
            }
            frame::Frame::DisconnectAckFrame(_) => {
                // Our disconnect has been received
                self.disconnected_enter();
            }
            _ => ()
        }
    }

    fn disconnecting_step(&mut self, now: time::Instant) {
        if self.meta_send_time.map_or(true, |time| now - time > DISCONNECT_INTERVAL) {
            self.meta_send_time = Some(now);
            self.enqueue_meta(frame::Frame::DisconnectFrame(frame::DisconnectFrame { }));
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
            frame::Frame::DisconnectFrame(_) => {
                self.enqueue_meta(frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame { }));
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
            State::Disconnecting => self.disconnecting_handle_frame(frame),
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
            State::Disconnecting => self.disconnecting_step(now),
            State::Disconnected => (),
            State::Zombie => (),
        }
    }

    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        std::mem::take(&mut self.event_queue).into_iter()
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: ChannelId, mode: SendMode) {
    }

    pub fn flush(&mut self, sink: & dyn DataSink) {
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
        todo!()
    }
}

