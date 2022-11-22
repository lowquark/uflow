use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;
use std::time;

use crate::EndpointConfig;
use crate::frame::serial::Serialize;
use crate::frame;
use crate::half_connection;
use crate::MAX_FRAME_SIZE;
use crate::MAX_FRAME_WINDOW_SIZE;
use crate::MAX_PACKET_WINDOW_SIZE;
use crate::PROTOCOL_VERSION;
use crate::udp_frame_sink::UdpFrameSink;

mod event_queue;
mod remote_client;

static HANDSHAKE_RESEND_INTERVAL_MS: u64 = 2000;
static HANDSHAKE_RESEND_COUNT: u8 = 10;

static ACTIVE_TIMEOUT_MS: u64 = 20000;

static DISCONNECT_RESEND_INTERVAL_MS: u64 = 2000;
static DISCONNECT_RESEND_COUNT: u8 = 10;

static CLOSED_TIMEOUT_MS: u64 = 20000;

pub use remote_client::RemoteClient;

/// Stores configuration parameters for a [`Server`](Server) object.
pub struct Config {
    /// The maximum number of connections, including active connections and connections which are in
    /// the process of connecting / disconnecting.
    pub max_total_connections: usize,
    /// The maximum number of active connections.
    pub max_active_connections: usize,
    /// Whether to emit events in response to client handshake errors.
    pub enable_handshake_errors: bool,
    /// Endpoint configuration to use for inbound client connections.
    pub endpoint_config: EndpointConfig,
}

impl Config {
    /// Returns true if the given configuration is valid.
    pub fn is_valid(&self) -> bool {
        return self.max_total_connections > 0
            && self.max_active_connections > 0
            && self.endpoint_config.is_valid();
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_total_connections: 4096,
            max_active_connections: 32,
            enable_handshake_errors: false,
            endpoint_config: Default::default(),
        }
    }
}

/// Represents a connection error.
#[derive(Debug,PartialEq)]
pub enum ErrorType {
    /// Indicates that an active connection or connection handshake has timed out.
    Timeout,
    /// Indicates that an inbound connection could not be established due to a protocol version
    /// mismatch.
    Version,
    /// Indicates that an inbound connection could not be established due to an endpoint
    /// configuration mismatch.
    Config,
    /// Indicates that a connection could not be established because the maximum number of clients
    /// are already connected to the server.
    ServerFull,
}

/// Used to signal connection events and deliver received packets.
#[derive(Debug)]
pub enum Event {
    /// Indicates a successful connection from a client.
    Connect(net::SocketAddr),
    /// Indicates that a client has disconnected. A disconnection event is only produced if either
    /// party explicitly terminates an active connection.
    Disconnect(net::SocketAddr),
    /// Signals a packet received from a client.
    Receive(net::SocketAddr, Box<[u8]>),
    /// Indicates that the connection has been terminated due to an unrecoverable error.
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

impl<'a> half_connection::PacketSink for EventPacketSink<'a> {
    fn send(&mut self, packet_data: Box<[u8]>) {
        self.event_queue.push(Event::Receive(self.address, packet_data));
    }
}

/// Acts as a host for inbound `uflow` connections.
pub struct Server {
    socket: net::UdpSocket,
    config: Config,

    clients: HashMap<net::SocketAddr, Rc<RefCell<remote_client::RemoteClient>>>,
    active_clients: Vec<Rc<RefCell<remote_client::RemoteClient>>>,

    client_events: event_queue::EventQueue,

    time_base: time::Instant,

    events_out: Vec<Event>,
}

impl Server {
    /// Opens a non-blocking UDP socket bound to the provided address, and creates a corresponding
    /// [`Server`](Self) object.
    ///
    /// # Error Handling
    ///
    /// Any errors resulting from socket initialization are forwarded to the caller. This function
    /// will panic if the provided server configuration is not valid.
    pub fn bind<A: net::ToSocketAddrs>(addr: A, config: Config) -> Result<Self, std::io::Error> {
        assert!(config.is_valid(), "invalid server config");

        let socket = net::UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;

        Ok(Self {
            socket,
            config,

            clients: HashMap::new(),
            active_clients: Vec::new(),

            client_events: event_queue::EventQueue::new(),

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
    /// socket. Returns an iterator of [`Event`] objects to signal connection events and deliver
    /// received packets for each client.
    ///
    /// *Note 1*: All events are considered delivered, even if the iterator is not consumed until
    /// the end.
    ///
    /// *Note 2*: Internally, `uflow` uses the [leaky bucket
    /// algorithm](https://en.wikipedia.org/wiki/Leaky_bucket) to control the rate at which UDP
    /// frames are sent. To ensure that data is transferred smoothly, this function should be
    /// called regularly and frequently (at least once per connection round-trip time).
    pub fn step(&mut self) -> impl Iterator<Item = Event> {
        let now_ms = self.now_ms();

        self.flush_active_clients();

        self.handle_frames(now_ms);

        self.handle_events(now_ms);

        self.active_clients.retain(|client| client.borrow().is_active());

        self.step_active_clients(now_ms);

        std::mem::take(&mut self.events_out).into_iter()
    }

    /// Sends as many pending outbound frames as possible for each client.
    pub fn flush(&mut self) {
        self.flush_active_clients();
    }

    /// Returns an object representing the client at the given address. Returns `None` if no such
    /// client exists.
    pub fn client(&self, client_addr: &net::SocketAddr) -> Option<&Rc<RefCell<remote_client::RemoteClient>>> {
        self.clients.get(client_addr)
    }

    /// Immediately terminates the client connection with the provided address. No further data
    /// will be sent or received, and a timeout error will be generated on the client.
    pub fn drop(&mut self, client_addr: &net::SocketAddr) {
        if let Some(client_rc) = self.clients.get(client_addr) {
            // Forget client immediately
            client_rc.borrow_mut().state = remote_client::State::Fin;
            self.clients.remove(client_addr);
        }
    }

    fn now_ms(&self) -> u64 {
        let now = time::Instant::now();
        (now - self.time_base).as_millis() as u64
    }

    fn handle_handshake_syn(
        &mut self,
        client_addr: net::SocketAddr,
        handshake: frame::HandshakeSynFrame,
        now_ms: u64,
    ) {
        if let Some(_) = self.clients.get(&client_addr) {
            // Ignore subsequent SYN (either spam or a duplicate - SYN+ACK will be resent if no ACK
            // is received)
            return;
        }

        if handshake.version != PROTOCOL_VERSION {
            // Bad version
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                nonce_ack: handshake.nonce,
                error: frame::HandshakeErrorType::Version,
            });
            let _ = self.socket.send_to(&reply.write(), client_addr);

            if self.config.enable_handshake_errors {
                self.events_out.push(Event::Error(client_addr, ErrorType::Version));
            }

            return;
        }

        if self.clients.len() >= self.config.max_total_connections
            && self.active_clients.len() >= self.config.max_active_connections
        {
            // No room in the inn
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                nonce_ack: handshake.nonce,
                error: frame::HandshakeErrorType::ServerFull,
            });
            let _ = self.socket.send_to(&reply.write(), client_addr);

            if self.config.enable_handshake_errors {
                self.events_out.push(Event::Error(client_addr, ErrorType::ServerFull));
            }

            return;
        }

        if (handshake.max_receive_alloc as usize) < self.config.endpoint_config.max_packet_size {
            // This connection may stall
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                nonce_ack: handshake.nonce,
                error: frame::HandshakeErrorType::Config, // TODO: Better error status
            });
            let _ = self.socket.send_to(&reply.write(), client_addr);

            if self.config.enable_handshake_errors {
                self.events_out.push(Event::Error(client_addr, ErrorType::Config));
            }

            return;
        }

        if (handshake.max_packet_size as usize) > self.config.endpoint_config.max_receive_alloc {
            // This connection may stall
            let reply = frame::Frame::HandshakeErrorFrame(frame::HandshakeErrorFrame {
                nonce_ack: handshake.nonce,
                error: frame::HandshakeErrorType::Config, // TODO: Better error status
            });
            let _ = self.socket.send_to(&reply.write(), client_addr);

            if self.config.enable_handshake_errors {
                self.events_out.push(Event::Error(client_addr, ErrorType::Config));
            }

            return;
        }

        // Handshake appears valid, send reply

        let local_nonce = rand::random::<u32>();

        let reply = frame::Frame::HandshakeSynAckFrame(frame::HandshakeSynAckFrame {
            nonce_ack: handshake.nonce,
            nonce: local_nonce,
            max_receive_rate: self
                .config
                .endpoint_config
                .max_receive_rate
                .min(u32::MAX as usize) as u32,
            max_packet_size: self
                .config
                .endpoint_config
                .max_packet_size
                .min(u32::MAX as usize) as u32,
            max_receive_alloc: self
                .config
                .endpoint_config
                .max_receive_alloc
                .min(u32::MAX as usize) as u32,
        });

        let reply_bytes = reply.write();
        let _ = self.socket.send_to(&reply_bytes, client_addr);

        // Create a tentative client object

        let client_rc = Rc::new(RefCell::new(remote_client::RemoteClient {
            address: client_addr,
            state: remote_client::State::Pending(remote_client::PendingState {
                local_nonce,
                remote_nonce: handshake.nonce,
                remote_max_receive_rate: handshake.max_receive_rate,
                remote_max_receive_alloc: handshake.max_receive_alloc,
                reply_bytes,
            }),
            max_packet_size: self.config.endpoint_config.max_packet_size,
        }));

        self.client_events.push(event_queue::Event::new(
            Rc::clone(&client_rc),
            event_queue::EventType::ResendHandshakeSynAck,
            now_ms + HANDSHAKE_RESEND_INTERVAL_MS,
            HANDSHAKE_RESEND_COUNT,
        ));

        self.clients.insert(client_addr, client_rc);
    }

    fn handle_handshake_ack(
        &mut self,
        client_addr: net::SocketAddr,
        handshake: frame::HandshakeAckFrame,
        now_ms: u64,
    ) {
        if let Some(client_rc) = self.clients.get(&client_addr) {
            let mut client = client_rc.borrow_mut();

            match client.state {
                remote_client::State::Pending(ref state) => {
                    if handshake.nonce_ack == state.local_nonce {
                        use crate::packet_id;

                        let config = half_connection::Config {
                            tx_frame_window_size: MAX_FRAME_WINDOW_SIZE,
                            rx_frame_window_size: MAX_FRAME_WINDOW_SIZE,

                            tx_frame_base_id: state.local_nonce,
                            rx_frame_base_id: state.remote_nonce,

                            tx_packet_window_size: MAX_PACKET_WINDOW_SIZE,
                            rx_packet_window_size: MAX_PACKET_WINDOW_SIZE,

                            tx_packet_base_id: state.local_nonce & packet_id::MASK,
                            rx_packet_base_id: state.remote_nonce & packet_id::MASK,

                            tx_bandwidth_limit: (self.config.endpoint_config.max_send_rate as u32).min(state.remote_max_receive_rate),

                            tx_alloc_limit: state.remote_max_receive_alloc as usize,
                            rx_alloc_limit: self.config.endpoint_config.max_receive_alloc as usize,

                            keepalive: self.config.endpoint_config.keepalive,
                        };

                        let half_connection = half_connection::HalfConnection::new(config);

                        client.state = remote_client::State::Active(remote_client::ActiveState {
                            half_connection,
                            timeout_time_ms: now_ms + ACTIVE_TIMEOUT_MS,
                            disconnect_signal: None,
                        });

                        self.active_clients.push(Rc::clone(&client_rc));

                        // Signal connect
                        self.events_out.push(Event::Connect(client_addr));
                    }
                }
                _ => (),
            }
        }
    }

    fn handle_disconnect(
        &mut self,
        client_addr: net::SocketAddr,
        now_ms: u64,
    ) {
        if let Some(client_rc) = self.clients.get(&client_addr) {
            // TODO: Should this response be throttled?
            let reply = frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame {});
            let _ = self.socket.send_to(&reply.write(), client_addr);

            let mut client = client_rc.borrow_mut();

            // Signal remaining received packets prior to connection destruction
            match client.state {
                remote_client::State::Active(ref mut state) => {
                    state.half_connection.receive(&mut EventPacketSink::new(client_addr, &mut self.events_out));
                }
                _ => (),
            }

            // Signal disconnect
            self.events_out.push(Event::Disconnect(client_addr));

            // Close now, but forget after a timeout
            client.state = remote_client::State::Closed;

            self.client_events.push(event_queue::Event::new(
                Rc::clone(&client_rc),
                event_queue::EventType::ClosedTimeout,
                now_ms + CLOSED_TIMEOUT_MS,
                0,
            ));
        }
    }

    fn handle_disconnect_ack(
        &mut self,
        client_addr: net::SocketAddr
    ) {
        if let Some(client_rc) = self.clients.get(&client_addr) {
            let mut client = client_rc.borrow_mut();

            match client.state {
                remote_client::State::Closing => {
                    // Forget client and signal disconnect
                    self.events_out.push(Event::Disconnect(client_addr));

                    client.state = remote_client::State::Fin;
                    std::mem::drop(client);
                    self.clients.remove(&client_addr);
                }
                _ => (),
            }
        }
    }

    fn handle_data(
        &mut self,
        client_addr: net::SocketAddr,
        frame: frame::DataFrame,
        now_ms: u64
    ) {
        if let Some(client_rc) = self.clients.get(&client_addr) {
            let mut client = client_rc.borrow_mut();

            match client.state {
                remote_client::State::Active(ref mut state) => {
                    state
                        .half_connection
                        .handle_data_frame(frame);

                    state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
                }
                _ => (),
            }
        }
    }

    fn handle_ack(
        &mut self,
        client_addr: net::SocketAddr,
        frame: frame::AckFrame,
        now_ms: u64
    ) {
        if let Some(client_rc) = self.clients.get(&client_addr) {
            let mut client = client_rc.borrow_mut();

            match client.state {
                remote_client::State::Active(ref mut state) => {
                    state
                        .half_connection
                        .handle_ack_frame(frame);

                    state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
                }
                _ => (),
            }
        }
    }

    fn handle_sync(
        &mut self,
        client_addr: net::SocketAddr,
        frame: frame::SyncFrame,
        now_ms: u64
    ) {
        if let Some(client_rc) = self.clients.get(&client_addr) {
            let mut client = client_rc.borrow_mut();

            match client.state {
                remote_client::State::Active(ref mut state) => {
                    state
                        .half_connection
                        .handle_sync_frame(frame);

                    state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
                }
                _ => (),
            }
        }
    }

    fn handle_frame(
        &mut self,
        address: net::SocketAddr,
        frame: frame::Frame,
        now_ms: u64
    ) {
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
        }
    }

    fn handle_frames(
        &mut self,
        now_ms: u64
    ) {
        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok((frame_size, address)) = self.socket.recv_from(&mut frame_data_buf) {
            if let Some(frame) = frame::Frame::read(&frame_data_buf[..frame_size]) {
                self.handle_frame(address, frame, now_ms);
            }
        }
    }

    fn handle_event(
        &mut self,
        mut event: event_queue::Event,
        now_ms: u64
    ) {
        let mut client = event.client.borrow_mut();

        match client.state {
            remote_client::State::Pending(ref state) => {
                if event.kind == event_queue::EventType::ResendHandshakeSynAck {
                    if event.count > 0 {
                        let _ = self.socket.send_to(&state.reply_bytes, client.address);

                        event.count -= 1;
                        event.time = now_ms + HANDSHAKE_RESEND_INTERVAL_MS;

                        std::mem::drop(client);
                        self.client_events.push(event);
                    } else {
                        if self.config.enable_handshake_errors {
                            // Forget client and signal handshake timeout
                            self.events_out.push(Event::Error(client.address, ErrorType::Timeout));
                        }

                        client.state = remote_client::State::Fin;
                        self.clients.remove(&client.address);
                    }
                }
            }
            remote_client::State::Closing => {
                if event.kind == event_queue::EventType::ResendDisconnect {
                    if event.count > 0 {
                        let request = frame::Frame::DisconnectFrame(frame::DisconnectFrame {});
                        let _ = self.socket.send_to(&request.write(), client.address);

                        event.count -= 1;
                        event.time = now_ms + DISCONNECT_RESEND_INTERVAL_MS;

                        std::mem::drop(client);
                        self.client_events.push(event);
                    } else {
                        // Forget client and signal timeout
                        self.events_out.push(Event::Error(client.address, ErrorType::Timeout));

                        client.state = remote_client::State::Fin;
                        self.clients.remove(&client.address);
                    }
                }
            }
            remote_client::State::Closed => {
                if event.kind == event_queue::EventType::ClosedTimeout {
                    // Forget client at last (disconnect has already been signaled)
                    client.state = remote_client::State::Fin;
                    self.clients.remove(&client.address);
                }
            }
            _ => (),
        }
    }

    fn handle_events(
        &mut self,
        now_ms: u64
    ) {
        while let Some(event) = self.client_events.peek() {
            if event.time > now_ms {
                break;
            }

            let event = self.client_events.pop().unwrap();

            self.handle_event(event, now_ms);
        }

        for client_rc in self.active_clients.iter() {
            let mut client = client_rc.borrow_mut();
            let client_addr = client.address;

            match client.state {
                remote_client::State::Active(ref mut state) => {
                    if now_ms >= state.timeout_time_ms {
                        // Signal remaining received packets
                        state.half_connection.receive(&mut EventPacketSink::new(client_addr, &mut self.events_out));

                        // Forget client and signal timeout
                        self.events_out.push(Event::Error(client_addr, ErrorType::Timeout));

                        client.state = remote_client::State::Fin;
                        self.clients.remove(&client.address);
                    }
                }
                _ => (),
            }
        }
    }

    fn step_active_clients(&mut self, now_ms: u64) {
        for client_rc in self.active_clients.iter() {
            let mut client = client_rc.borrow_mut();
            let client_addr = client.address;

            match client.state {
                remote_client::State::Active(ref mut state) => {
                    let disconnect_now = match state.disconnect_signal {
                        Some(remote_client::DisconnectMode::Now) => true,
                        Some(remote_client::DisconnectMode::Flush) => !state.half_connection.is_send_pending(),
                        None => false,
                    };

                    if disconnect_now {
                        // Signal remaining received packets
                        state.half_connection.receive(&mut EventPacketSink::new(client_addr, &mut self.events_out));

                        // Attempt to close the connection
                        let request = frame::Frame::DisconnectFrame(frame::DisconnectFrame {});
                        let _ = self.socket.send_to(&request.write(), client.address);

                        client.state = remote_client::State::Closing;

                        self.client_events.push(event_queue::Event::new(
                            Rc::clone(&client_rc),
                            event_queue::EventType::ResendDisconnect,
                            now_ms + DISCONNECT_RESEND_INTERVAL_MS,
                            DISCONNECT_RESEND_COUNT,
                        ));
                    } else {
                        // Process and signal received packets
                        state.half_connection.step();
                        state.half_connection.receive(&mut EventPacketSink::new(client_addr, &mut self.events_out));
                    }
                }
                _ => (),
            }
        }
    }

    fn flush_active_clients(&mut self) {
        for client_rc in self.active_clients.iter() {
            let mut client = client_rc.borrow_mut();
            let client_addr = client.address;

            match client.state {
                remote_client::State::Active(ref mut state) => {
                    let ref mut data_sink = UdpFrameSink::new(&self.socket, client_addr);
                    state.half_connection.flush(data_sink);
                }
                _ => (),
            }
        }
    }
}
