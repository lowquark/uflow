mod event_queue;
mod peer;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;
use std::time;

use crate::endpoint;
use crate::frame;
use crate::frame::serial::Serialize;
use crate::udp_frame_sink::UdpFrameSink;
use crate::MAX_FRAME_SIZE;
use crate::PROTOCOL_VERSION;

pub struct PendingPeer {}

pub struct Config {
    pub max_pending_connections: usize,
    pub max_active_connections: usize,
    pub peer_config: endpoint::Config,
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

pub enum ErrorKind {
    Timeout,
    HandshakeError,
    HandshakeTimeout,
}

pub enum Event {
    Connect(net::SocketAddr),
    Disconnect(net::SocketAddr),
    Error(net::SocketAddr, ErrorKind),
    Receive(net::SocketAddr, Box<[u8]>),
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
        self.flush_active_peers();

        let now_ms = self.now_ms();

        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok((frame_size, address)) = self.socket.recv_from(&mut frame_data_buf) {
            if let Some(frame) = frame::Frame::read(&frame_data_buf[..frame_size]) {
                self.handle_frame(address, frame, now_ms);
            }
        }

        self.handle_events(now_ms);
        self.active_peers.retain(|peer| peer.borrow().is_active());

        self.step_active_peers(now_ms);

        std::mem::take(&mut self.events_out).into_iter()
    }

    /// Sends as many pending outbound frames (packet data, acknowledgements, keep-alives, etc.) as
    /// possible for each peer.
    pub fn flush(&mut self) {
        self.flush_active_peers();
    }

    pub fn disconnect(&mut self, peer_addr: &net::SocketAddr) {
        let now_ms = self.now_ms();

        if let Some(peer_rc) = self.peers.get(peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            if peer.is_active() {
                // TODO: Flush pending data before closing
                peer.state = peer::State::Closing;

                self.peer_events.push(event_queue::Event::new(
                    Rc::clone(&peer_rc),
                    event_queue::EventKind::ResendDisconnect,
                    now_ms + 5000,
                    5,
                ));
            }
        }
    }

    fn now_ms(&self) -> u64 {
        let now = time::Instant::now();
        (now - self.time_base).as_millis() as u64
    }

    fn handle_frame(&mut self, address: net::SocketAddr, frame: frame::Frame, now_ms: u64) {
        match frame {
            frame::Frame::HandshakeSynFrame(frame) => {
                self.handle_handshake_syn(address, frame, now_ms);
            }
            frame::Frame::HandshakeAckFrame(frame) => {
                self.handle_handshake_ack(address, frame);
            }
            frame::Frame::HandshakeSynAckFrame(_) => (),
            frame::Frame::DisconnectFrame(frame) => {
                self.handle_disconnect(address, frame, now_ms);
            }
            frame::Frame::DisconnectAckFrame(frame) => {
                self.handle_disconnect_ack(address, frame);
            }
            frame::Frame::DataFrame(frame) => {
                self.handle_data(address, frame);
            }
            frame::Frame::SyncFrame(frame) => {
                self.handle_sync(address, frame);
            }
            frame::Frame::AckFrame(frame) => {
                self.handle_ack(address, frame);
            }
            _ => (),
        }
    }

    fn handle_handshake_syn(
        &mut self,
        peer_addr: net::SocketAddr,
        handshake: frame::HandshakeSynFrame,
        now_ms: u64,
    ) {
        if let Some(_) = self.peers.get(&peer_addr) {
            // Ignore subsequent SYN (likely a duplicate)
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
            event_queue::EventKind::ResendHandshakeSynAck,
            now_ms + 5000,
            5,
        ));

        self.peers.insert(peer_addr, peer_rc);
    }

    fn handle_handshake_ack(
        &mut self,
        peer_addr: net::SocketAddr,
        handshake: frame::HandshakeAckFrame,
    ) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match &peer.state {
                peer::State::Pending(state) => {
                    if handshake.nonce_ack == state.local_nonce {
                        peer.state = peer::State::Active(peer::ActiveState {
                            endpoint: endpoint::Endpoint::new(self.config.peer_config.clone()),
                        });

                        self.active_peers.push(Rc::clone(&peer_rc));

                        // Signal connect
                        self.events_out.push(Event::Connect(peer_addr));
                    } else {
                        // Bad handshake, forget peer
                        std::mem::drop(peer);
                        self.peers.remove(&peer_addr);

                        // Signal handshake error
                        self.events_out.push(Event::Error(peer_addr, ErrorKind::HandshakeError));
                    }
                }
                _ => (),
            }
        }
    }

    fn handle_disconnect(
        &mut self,
        peer_addr: net::SocketAddr,
        frame: frame::DisconnectFrame,
        now_ms: u64,
    ) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let reply = frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame {});
            let _ = self.socket.send_to(&reply.write(), peer_addr);

            let mut peer = peer_rc.borrow_mut();

            // Forget peer after a timeout
            peer.state = peer::State::Closed;

            self.peer_events.push(event_queue::Event::new(
                Rc::clone(&peer_rc),
                event_queue::EventKind::ClosedTimeout,
                now_ms + 15000,
                0,
            ));

            // Signal disconnect
            self.events_out.push(Event::Disconnect(peer_addr));
        }
    }

    fn handle_disconnect_ack(
        &mut self,
        peer_addr: net::SocketAddr,
        frame: frame::DisconnectAckFrame,
    ) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let peer = peer_rc.borrow_mut();

            match &peer.state {
                peer::State::Closing => {
                    // Forget peer now
                    std::mem::drop(peer);
                    self.peers.remove(&peer_addr);

                    // Signal disconnect
                    self.events_out.push(Event::Disconnect(peer_addr));
                }
                _ => (),
            }
        }
    }

    fn handle_data(&mut self, peer_addr: net::SocketAddr, frame: frame::DataFrame) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match &mut peer.state {
                peer::State::Active(state) => {
                    let ref mut data_sink = UdpFrameSink::new(&self.socket, peer_addr);
                    state
                        .endpoint
                        .handle_frame(frame::Frame::DataFrame(frame), data_sink);
                }
                _ => (),
            }
        }
    }

    fn handle_ack(&mut self, peer_addr: net::SocketAddr, frame: frame::AckFrame) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match &mut peer.state {
                peer::State::Active(state) => {
                    let ref mut data_sink = UdpFrameSink::new(&self.socket, peer_addr);
                    state
                        .endpoint
                        .handle_frame(frame::Frame::AckFrame(frame), data_sink);
                }
                _ => (),
            }
        }
    }

    fn handle_sync(&mut self, peer_addr: net::SocketAddr, frame: frame::SyncFrame) {
        if let Some(peer_rc) = self.peers.get(&peer_addr) {
            let mut peer = peer_rc.borrow_mut();

            match &mut peer.state {
                peer::State::Active(state) => {
                    let ref mut data_sink = UdpFrameSink::new(&self.socket, peer_addr);
                    state
                        .endpoint
                        .handle_frame(frame::Frame::SyncFrame(frame), data_sink);
                }
                _ => (),
            }
        }
    }

    fn handle_event(&mut self, now_ms: u64, mut event: event_queue::Event) {
        let peer = event.peer.borrow_mut();

        match &peer.state {
            peer::State::Pending(state) => {
                if event.kind == event_queue::EventKind::ResendHandshakeSynAck {
                    let _ = self.socket.send_to(&state.reply_bytes, peer.address);

                    if event.count > 0 {
                        event.count -= 1;
                        event.time = now_ms + 5000;

                        std::mem::drop(peer);

                        self.peer_events.push(event);
                    } else {
                        // Forget peer
                        self.peers.remove(&peer.address);

                        // Signal handshake timeout
                        self.events_out.push(Event::Error(peer.address, ErrorKind::HandshakeTimeout));
                    }
                }
            }
            peer::State::Closing => {
                if event.kind == event_queue::EventKind::ResendDisconnect {
                    let reply = frame::Frame::DisconnectFrame(frame::DisconnectFrame {});
                    let _ = self.socket.send_to(&reply.write(), peer.address);

                    if event.count > 0 {
                        event.count -= 1;
                        event.time = now_ms + 5000;

                        std::mem::drop(peer);

                        self.peer_events.push(event);
                    } else {
                        // Forget peer
                        self.peers.remove(&peer.address);

                        // Signal timeout
                        self.events_out.push(Event::Error(peer.address, ErrorKind::Timeout));
                    }
                }
            }
            peer::State::Closed => {
                if event.kind == event_queue::EventKind::ClosedTimeout {
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
    }

    fn step_active_peers(&mut self, now_ms: u64) {
        for peer_rc in self.active_peers.iter() {
            let mut peer = peer_rc.borrow_mut();

            match &mut peer.state {
                peer::State::Active(state) => {
                    state.endpoint.step();

                    // TODO: Signal received packets
                    // self.events_out.push(Event::Receive(peer_addr, ErrorKind::Timeout));
                }
                _ => (),
            }
        }
    }

    fn flush_active_peers(&mut self) {
        for peer_rc in self.active_peers.iter() {
            let mut peer = peer_rc.borrow_mut();
            let peer_addr = peer.address;

            match &mut peer.state {
                peer::State::Active(state) => {
                    let ref mut data_sink = UdpFrameSink::new(&self.socket, peer_addr);
                    state.endpoint.flush(data_sink);
                }
                _ => (),
            }
        }
    }
}
