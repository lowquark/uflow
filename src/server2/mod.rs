use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;
use std::time;

use crate::EndpointConfig;
use crate::endpoint::daten_meister;
use crate::frame;
use crate::frame::serial::Serialize;
use crate::udp_frame_sink::UdpFrameSink;
use crate::MAX_FRAME_SIZE;
use crate::PROTOCOL_VERSION;
use crate::MAX_FRAME_WINDOW_SIZE;
use crate::MAX_PACKET_WINDOW_SIZE;

mod event_queue;
mod peer;

static HANDSHAKE_RESEND_INTERVAL_MS: u64 = 2000;
static HANDSHAKE_RESEND_COUNT: u8 = 8;

static ACTIVE_TIMEOUT_MS: u64 = 15000;

static DISCONNECT_RESEND_INTERVAL_MS: u64 = 2000;
static DISCONNECT_RESEND_COUNT: u8 = 8;

static CLOSED_TIMEOUT_MS: u64 = 15000;

pub struct Config {
    pub max_pending_connections: usize,
    pub max_active_connections: usize,
    pub peer_config: EndpointConfig,
}

impl Config {
    pub fn is_valid(&self) -> bool {
        return self.max_pending_connections > 0
            && self.max_active_connections > 0
            && self.peer_config.is_valid();
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_pending_connections: 4096,
            max_active_connections: 32,
            peer_config: Default::default(),
        }
    }
}

pub enum ErrorType {
    HandshakeError,
    HandshakeTimeout,
    Timeout,
}

pub enum Event {
    Connect(net::SocketAddr),
    Disconnect(net::SocketAddr),
    Receive(net::SocketAddr, Box<[u8]>),
    Error(net::SocketAddr, ErrorType),
}

struct EventPacketSink<'a> {
    address: net::SocketAddr,
    event_queue: &'a mut Vec<Event>,
}

impl<'a> EventPacketSink<'a> {
    fn new(address: net::SocketAddr, event_queue: &'a mut Vec<Event>) -> Self {
        Self {
            address,
            event_queue,
        }
    }
}

impl<'a> daten_meister::PacketSink for EventPacketSink<'a> {
    fn send(&mut self, packet_data: Box<[u8]>) {
        self.event_queue.push(Event::Receive(self.address, packet_data));
    }
}

/// A polling-based socket object which manages inbound `uflow` connections.
pub struct Server {
    socket: net::UdpSocket,
    config: Config,

    peers: HashMap<net::SocketAddr, Rc<RefCell<peer::Peer>>>,
    active_peers: Vec<Rc<RefCell<peer::Peer>>>,

    peer_events: event_queue::EventQueue,

    time_base: time::Instant,

    events_out: Vec<Event>,
}

impl Server {
    /// Opens a non-blocking UDP socket bound to the provided address, and creates a corresponding
    /// [`Server`](Self) object.
    ///
    /// The server will limit the number of active connections to `max_peer_count`, and will
    /// silently ignore connection requests which would exceed that limit. Otherwise valid incoming
    /// connections will be initialized according to `peer_config`.
    ///
    /// # Error Handling
    ///
    /// Any errors resulting from socket initialization are forwarded to the caller. This function
    /// will panic if the given endpoint configuration is not valid.
    pub fn bind<A: net::ToSocketAddrs>(addr: A, config: Config) -> Result<Self, std::io::Error> {
        assert!(config.is_valid(), "invalid server config");

        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        Ok(Self {
            socket,
            config,

            peers: HashMap::new(),
            active_peers: Vec::new(),

            peer_events: event_queue::EventQueue::new(),

            time_base: time::Instant::now(),

            events_out: Vec::new(),
        })
    }

    /// Equivalent to calling [`bind()`](Self::bind) with address
    /// `(`[`std::net::Ipv4Addr::UNSPECIFIED`](std::net::Ipv4Addr::UNSPECIFIED)`, 0)`.
    pub fn bind_any_ipv4(config: Config) -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv4Addr::UNSPECIFIED, 0), config)
    }

    /// Equivalent to calling [`bind()`](Self::bind) with address
    /// `(`[`std::net::Ipv6Addr::UNSPECIFIED`](std::net::Ipv6Addr::UNSPECIFIED)`, 0)`.
    pub fn bind_any_ipv6(config: Config) -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv6Addr::UNSPECIFIED, 0), config)
    }

    /// Returns the local address of the internal UDP socket.
    pub fn address(&self) -> net::SocketAddr {
        self.socket.local_addr().unwrap()
    }

    /// Flushes pending outbound frames, and reads as many UDP frames as possible from the internal
    /// socket. Returns an iterator of `ServerEvents`s to signal connection status and deliver
    /// received packets for each peer.
    ///
    /// *Note 1*: All peer events are considered delivered, even if the iterator is not consumed until the
    /// end.
    ///
    /// *Note 2*: Internally, `uflow` uses the [leaky bucket
    /// algorithm](https://en.wikipedia.org/wiki/Leaky_bucket) to control the rate at which UDP
    /// frames are sent. To ensure that data is transferred smoothly, this function should be
    /// called relatively frequently (a minimum of once per connection round-trip time).
    pub fn step(&mut self) -> impl Iterator<Item = Event> {
        let now_ms = self.now_ms();

        self.flush_active_peers(now_ms);

        self.handle_frames(now_ms);

        self.handle_events(now_ms);

        self.active_peers.retain(|peer| peer.borrow().is_active());

        self.step_active_peers();

        std::mem::take(&mut self.events_out).into_iter()
    }

    /// Sends as many pending outbound frames (packet data, acknowledgements, keep-alives, etc.) as
    /// possible for each peer.
    pub fn flush(&mut self) {
        let now_ms = self.now_ms();
        self.flush_active_peers(now_ms);
    }

    pub fn disconnect(&mut self, peer_addr: &net::SocketAddr) {
        if let Some(peer_rc) = self.peers.get(peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match peer.state {
                peer::State::Active(ref mut state) => {
                    state.disconnect_flush = true;
                }
                _ => (),
            }
        }
    }

    pub fn peer(&self, peer_addr: &net::SocketAddr) -> Option<&Rc<RefCell<peer::Peer>>> {
        self.peers.get(peer_addr)
    }

    fn now_ms(&self) -> u64 {
        let now = time::Instant::now();
        (now - self.time_base).as_millis() as u64
    }

    fn handle_handshake_syn(
        &mut self,
        peer_addr: net::SocketAddr,
        handshake: frame::HandshakeSynFrame,
        now_ms: u64,
    ) {
        if let Some(_) = self.peers.get(&peer_addr) {
            // Ignore subsequent SYN (either spam or a duplicate - SYN+ACK will be resent if dropped)
            return;
        }

        if handshake.version != PROTOCOL_VERSION {
            // Bad version
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                error: frame::HandshakeErrorType::Version,
            });
            let _ = self.socket.send_to(&reply.write(), peer_addr);

            return;
        }

        if self.peers.len() >= self.config.max_pending_connections
            && self.active_peers.len() >= self.config.max_active_connections
        {
            // No room in the inn
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                error: frame::HandshakeErrorType::Full,
            });
            let _ = self.socket.send_to(&reply.write(), peer_addr);

            return;
        }

        if (handshake.max_receive_alloc as usize) < self.config.peer_config.max_packet_size {
            // This connection may stall
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                error: frame::HandshakeErrorType::Full, // TODO: Better error status
            });
            let _ = self.socket.send_to(&reply.write(), peer_addr);

            return;
        }

        if (handshake.max_packet_size as usize) > self.config.peer_config.max_receive_alloc {
            // This connection may stall
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                error: frame::HandshakeErrorType::Full, // TODO: Better error status
            });
            let _ = self.socket.send_to(&reply.write(), peer_addr);

            return;
        }

        // Handshake appears valid, send reply

        let local_nonce = rand::random::<u32>();

        let reply = frame::Frame::HandshakeSynAckFrame(frame::HandshakeSynAckFrame {
            nonce_ack: handshake.nonce,
            nonce: local_nonce,
            max_receive_rate: self
                .config
                .peer_config
                .max_receive_rate
                .min(u32::MAX as usize) as u32,
            max_packet_size: self
                .config
                .peer_config
                .max_packet_size
                .min(u32::MAX as usize) as u32,
            max_receive_alloc: self
                .config
                .peer_config
                .max_receive_alloc
                .min(u32::MAX as usize) as u32,
        });

        let reply_bytes = reply.write();
        let _ = self.socket.send_to(&reply_bytes, peer_addr);

        // Create a tentative peer object

        let peer_rc = Rc::new(RefCell::new(peer::Peer::new(
            peer_addr,
            peer::PendingState {
                local_nonce,
                remote_nonce: handshake.nonce,
                remote_max_receive_rate: handshake.max_receive_rate,
                remote_max_packet_size: handshake.max_packet_size,
                remote_max_receive_alloc: handshake.max_receive_alloc,
                reply_bytes,
            },
        )));

        self.peer_events.push(event_queue::Event::new(
            Rc::clone(&peer_rc),
            event_queue::EventType::ResendHandshakeSynAck,
            now_ms + HANDSHAKE_RESEND_INTERVAL_MS,
            HANDSHAKE_RESEND_COUNT,
        ));

        self.peers.insert(peer_addr, peer_rc);
    }

    fn handle_handshake_ack(
        &mut self,
        peer_addr: net::SocketAddr,
        handshake: frame::HandshakeAckFrame,
        now_ms: u64,
    ) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match peer.state {
                peer::State::Pending(ref state) => {
                    if handshake.nonce_ack == state.local_nonce {
                        use crate::packet_id;

                        let config = daten_meister::Config {
                            tx_frame_window_size: MAX_FRAME_WINDOW_SIZE,
                            rx_frame_window_size: MAX_FRAME_WINDOW_SIZE,

                            tx_frame_base_id: state.local_nonce,
                            rx_frame_base_id: state.remote_nonce,

                            tx_packet_window_size: MAX_PACKET_WINDOW_SIZE,
                            rx_packet_window_size: MAX_PACKET_WINDOW_SIZE,

                            tx_packet_base_id: state.local_nonce & packet_id::MASK,
                            rx_packet_base_id: state.remote_nonce & packet_id::MASK,

                            tx_bandwidth_limit: (self.config.peer_config.max_send_rate as u32).min(state.remote_max_receive_rate),

                            tx_alloc_limit: state.remote_max_receive_alloc as usize,
                            rx_alloc_limit: self.config.peer_config.max_receive_alloc as usize,

                            keepalive: self.config.peer_config.keepalive,
                        };

                        let daten_meister = daten_meister::DatenMeister::new(config);

                        peer.state = peer::State::Active(peer::ActiveState {
                            endpoint: daten_meister,
                            disconnect_flush: false,
                            timeout_time_ms: now_ms + ACTIVE_TIMEOUT_MS,
                        });

                        self.active_peers.push(Rc::clone(&peer_rc));

                        // Signal connect
                        self.events_out.push(Event::Connect(peer_addr));
                    } else {
                        // Bad handshake, forget peer and signal handshake error
                        self.events_out.push(Event::Error(peer_addr, ErrorType::HandshakeError));

                        std::mem::drop(peer);
                        self.peers.remove(&peer_addr);
                    }
                }
                _ => (),
            }
        }
    }

    fn handle_disconnect(
        &mut self,
        peer_addr: net::SocketAddr,
        now_ms: u64,
    ) {
        // TODO: Consider effects of spamming this

        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let reply = frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame {});
            let _ = self.socket.send_to(&reply.write(), peer_addr);

            let mut peer = peer_rc.borrow_mut();

            // Signal remaining received packets prior to endpoint destruction
            match peer.state {
                peer::State::Active(ref mut state) => {
                    state.endpoint.receive(&mut EventPacketSink::new(peer_addr, &mut self.events_out));
                }
                _ => (),
            }

            // Signal disconnect
            self.events_out.push(Event::Disconnect(peer_addr));

            // Close now, but forget after a timeout
            peer.state = peer::State::Closed;

            self.peer_events.push(event_queue::Event::new(
                Rc::clone(&peer_rc),
                event_queue::EventType::ClosedTimeout,
                now_ms + CLOSED_TIMEOUT_MS,
                0,
            ));
        }
    }

    fn handle_disconnect_ack(
        &mut self,
        peer_addr: net::SocketAddr
    ) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let peer = peer_rc.borrow_mut();

            match peer.state {
                peer::State::Closing => {
                    // Forget peer and signal disconnect
                    self.events_out.push(Event::Disconnect(peer_addr));

                    std::mem::drop(peer);
                    self.peers.remove(&peer_addr);
                }
                _ => (),
            }
        }
    }

    fn handle_data(&mut self, peer_addr: net::SocketAddr, frame: frame::DataFrame, now_ms: u64) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match peer.state {
                peer::State::Active(ref mut state) => {
                    state
                        .endpoint
                        .handle_data_frame(frame);

                    state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
                }
                _ => (),
            }
        }
    }

    fn handle_ack(&mut self, peer_addr: net::SocketAddr, frame: frame::AckFrame, now_ms: u64) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match peer.state {
                peer::State::Active(ref mut state) => {
                    state
                        .endpoint
                        .handle_ack_frame(frame);

                    state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
                }
                _ => (),
            }
        }
    }

    fn handle_sync(&mut self, peer_addr: net::SocketAddr, frame: frame::SyncFrame, now_ms: u64) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match peer.state {
                peer::State::Active(ref mut state) => {
                    state
                        .endpoint
                        .handle_sync_frame(frame);

                    state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
                }
                _ => (),
            }
        }
    }

    fn handle_frame(&mut self, address: net::SocketAddr, frame: frame::Frame, now_ms: u64) {
        match frame {
            frame::Frame::HandshakeSynFrame(frame) => {
                self.handle_handshake_syn(address, frame, now_ms);
            }
            frame::Frame::HandshakeAckFrame(frame) => {
                self.handle_handshake_ack(address, frame, now_ms);
            }
            frame::Frame::HandshakeSynAckFrame(_) => (),
            frame::Frame::HandshakeErrorFrame(_) => (),
            frame::Frame::DisconnectFrame(_frame) => {
                self.handle_disconnect(address, now_ms);
            }
            frame::Frame::DisconnectAckFrame(_frame) => {
                self.handle_disconnect_ack(address);
            }
            frame::Frame::DataFrame(frame) => {
                self.handle_data(address, frame, now_ms);
            }
            frame::Frame::SyncFrame(frame) => {
                self.handle_sync(address, frame, now_ms);
            }
            frame::Frame::AckFrame(frame) => {
                self.handle_ack(address, frame, now_ms);
            }
            _ => (),
        }
    }

    fn handle_frames(&mut self, now_ms: u64) {
        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok((frame_size, address)) = self.socket.recv_from(&mut frame_data_buf) {
            if let Some(frame) = frame::Frame::read(&frame_data_buf[..frame_size]) {
                self.handle_frame(address, frame, now_ms);
            }
        }
    }

    fn handle_event(&mut self, now_ms: u64, mut event: event_queue::Event) {
        let peer = event.peer.borrow();

        match peer.state {
            peer::State::Pending(ref state) => {
                if event.kind == event_queue::EventType::ResendHandshakeSynAck {
                    if event.count > 0 {
                        let _ = self.socket.send_to(&state.reply_bytes, peer.address);

                        event.count -= 1;
                        event.time = now_ms + HANDSHAKE_RESEND_INTERVAL_MS;

                        std::mem::drop(peer);
                        self.peer_events.push(event);
                    } else {
                        // Forget peer and signal handshake timeout
                        self.events_out.push(Event::Error(peer.address, ErrorType::HandshakeTimeout));
                        self.peers.remove(&peer.address);
                    }
                }
            }
            peer::State::Closing => {
                if event.kind == event_queue::EventType::ResendDisconnect {
                    if event.count > 0 {
                        let request = frame::Frame::DisconnectFrame(frame::DisconnectFrame {});
                        let _ = self.socket.send_to(&request.write(), peer.address);

                        event.count -= 1;
                        event.time = now_ms + DISCONNECT_RESEND_INTERVAL_MS;

                        std::mem::drop(peer);
                        self.peer_events.push(event);
                    } else {
                        // Forget peer and signal timeout
                        self.events_out.push(Event::Error(peer.address, ErrorType::Timeout));
                        self.peers.remove(&peer.address);
                    }
                }
            }
            peer::State::Closed => {
                if event.kind == event_queue::EventType::ClosedTimeout {
                    // Forget peer at last (disconnect has already been signaled)
                    self.peers.remove(&peer.address);
                }
            }
            _ => (),
        }
    }

    fn handle_events(&mut self, now_ms: u64) {
        while let Some(event) = self.peer_events.peek() {
            if event.time > now_ms {
                break;
            }

            let event = self.peer_events.pop().unwrap();

            self.handle_event(now_ms, event);
        }

        for peer_rc in self.active_peers.iter() {
            let mut peer = peer_rc.borrow_mut();
            let peer_addr = peer.address;

            match peer.state {
                peer::State::Active(ref mut state) => {
                    if now_ms >= state.timeout_time_ms {
                        // Signal remaining received packets
                        state.endpoint.receive(&mut EventPacketSink::new(peer_addr, &mut self.events_out));

                        // Forget peer and signal timeout
                        self.events_out.push(Event::Error(peer_addr, ErrorType::Timeout));
                        self.peers.remove(&peer.address);
                    }
                }
                _ => (),
            }
        }
    }

    fn step_active_peers(&mut self) {
        for peer_rc in self.active_peers.iter() {
            let mut peer = peer_rc.borrow_mut();
            let peer_addr = peer.address;

            match peer.state {
                peer::State::Active(ref mut state) => {
                    // Process and signal received packets
                    state.endpoint.step();
                    state.endpoint.receive(&mut EventPacketSink::new(peer_addr, &mut self.events_out));
                }
                _ => (),
            }
        }
    }

    fn flush_active_peers(&mut self, now_ms: u64) {
        for peer_rc in self.active_peers.iter() {
            let mut peer = peer_rc.borrow_mut();
            let peer_addr = peer.address;

            match peer.state {
                peer::State::Active(ref mut state) => {
                    let ref mut data_sink = UdpFrameSink::new(&self.socket, peer_addr);
                    state.endpoint.flush(data_sink);

                    if state.disconnect_flush && !state.endpoint.is_send_pending() {
                        // Signal remaining received packets
                        state.endpoint.receive(&mut EventPacketSink::new(peer_addr, &mut self.events_out));

                        // Attempt to close the connection
                        peer.state = peer::State::Closing;

                        self.peer_events.push(event_queue::Event::new(
                            Rc::clone(&peer_rc),
                            event_queue::EventType::ResendDisconnect,
                            now_ms + DISCONNECT_RESEND_INTERVAL_MS,
                            DISCONNECT_RESEND_COUNT,
                        ));
                    }
                }
                _ => (),
            }
        }
    }
}
