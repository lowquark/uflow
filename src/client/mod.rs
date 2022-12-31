use std::net;
use std::time;

use crate::CHANNEL_COUNT;
use crate::EndpointConfig;
use crate::frame::serial::Serialize;
use crate::frame;
use crate::half_connection;
use crate::MAX_FRAME_SIZE;
use crate::MAX_FRAME_WINDOW_SIZE;
use crate::MAX_PACKET_WINDOW_SIZE;
use crate::PROTOCOL_VERSION;
use crate::SendMode;
use crate::udp_frame_sink::UdpFrameSink;

static HANDSHAKE_RESEND_INTERVAL_MS: u64 = 2000;
static HANDSHAKE_RESEND_COUNT: u8 = 10;

static DISCONNECT_RESEND_INTERVAL_MS: u64 = 2000;
static DISCONNECT_RESEND_COUNT: u8 = 10;

static CLOSED_TIMEOUT_MS: u64 = 20000;

/// Stores configuration parameters for a [`Client`](Client) object.
pub struct Config {
    /// Endpoint configuration to use for outbound server connections.
    pub endpoint_config: EndpointConfig,
}

impl Config {
    /// Returns true if the given configuration is valid.
    pub fn is_valid(&self) -> bool {
        self.endpoint_config.is_valid()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoint_config: Default::default(),
        }
    }
}

/// Represents a connection error.
#[derive(Debug,PartialEq)]
pub enum ErrorType {
    /// Indicates that an active connection or connection handshake has timed out.
    Timeout,
    /// Indicates that a connection could not be established due to a protocol version mismatch.
    Version,
    /// Indicates that a connection could not be established due to an endpoint configuration
    /// mismatch.
    Config,
    /// Indicates that a connection could not be established because the maximum number of clients
    /// are already connected to the server.
    ServerFull,
}

/// Used to signal connection events and deliver received packets.
#[derive(Debug)]
pub enum Event {
    /// Indicates a successful connection to a server.
    Connect,
    /// Indicates that the server has disconnected. A disconnection event is only produced if either
    /// party explicitly terminates an active connection.
    Disconnect,
    /// Signals a packet received from the server.
    Receive(Box<[u8]>),
    /// Indicates that a connection has been terminated due to an unrecoverable error.
    Error(ErrorType),
}

struct PacketReceiveSink<'a> {
    event_queue: &'a mut Vec<Event>,
}

impl<'a> PacketReceiveSink<'a> {
    fn new(event_queue: &'a mut Vec<Event>) -> Self {
        Self {
            event_queue,
        }
    }
}

impl<'a> half_connection::PacketSink for PacketReceiveSink<'a> {
    fn send(&mut self, packet_data: Box<[u8]>) {
        self.event_queue.push(Event::Receive(packet_data));
    }
}

struct SendEntry {
    data: Box<[u8]>,
    channel_id: u8,
    mode: SendMode,
}

pub (super) enum DisconnectMode {
    Now,
    Flush,
}

struct PendingState {
    local_nonce: u32,

    request_bytes: Box<[u8]>,
    resend_time_ms: u64,
    resend_count: u8,

    initial_sends: Vec<SendEntry>,
}

struct ActiveState {
    local_nonce: u32,
    half_connection: half_connection::HalfConnection,
    timeout_time_ms: u64,
    disconnect_signal: Option<DisconnectMode>,
}

struct ClosingState {
    request_bytes: Box<[u8]>,
    resend_time_ms: u64,
    resend_count: u8,
}

struct ClosedState {
    timeout_time_ms: u64,
}

enum State {
    Pending(PendingState),
    Active(ActiveState),
    Closing(ClosingState),
    Closed(ClosedState),
    Fin,
}

/// Manages a single outbound `uflow` connection.
pub struct Client {
    socket: net::UdpSocket,
    config: Config,

    local_addr: net::SocketAddr,
    remote_addr: net::SocketAddr,

    time_base: time::Instant,

    state: State,

    events_out: Vec<Event>,
}

impl Client {
    /// Opens a non-blocking UDP socket bound to an ephemeral address, and returns a corresponding
    /// [`Client`](Self) object. A connection to the server at the provided destination address is
    /// initiated immediately.
    ///
    /// An IPv4/IPv6 socket will be opened according to the type of the destination address.
    ///
    /// # Error Handling
    ///
    /// Any errors resulting from socket initialization are forwarded to the caller. This function
    /// will panic if the provided client configuration is not valid.
    pub fn connect<A: net::ToSocketAddrs>(
        dst_addr: A,
        config: Config
    ) -> Result<Self, std::io::Error> {
        assert!(config.is_valid(), "invalid client config");

        // Open an ephemeral, non-blocking socket based on destination address

        let dst_socket_addr = dst_addr.to_socket_addrs()?.next().expect("expected at least one socket addresses");

        let bind_addr = match dst_socket_addr {
            net::SocketAddr::V4(_) => net::SocketAddr::new(net::IpAddr::V4(net::Ipv4Addr::UNSPECIFIED), 0),
            net::SocketAddr::V6(_) => net::SocketAddr::new(net::IpAddr::V6(net::Ipv6Addr::UNSPECIFIED), 0),
        };

        let socket = net::UdpSocket::bind(bind_addr)?;

        socket.set_nonblocking(true)?;
        socket.connect(dst_socket_addr)?;

        let local_addr = socket.local_addr()?;
        let remote_addr = socket.peer_addr()?;

        // Send initial connection request

        let nonce = rand::random::<u32>();

        let request = frame::Frame::HandshakeSynFrame(frame::HandshakeSynFrame {
            version: PROTOCOL_VERSION,
            nonce,
            max_receive_rate: config
                .endpoint_config
                .max_receive_rate
                .min(u32::MAX as usize) as u32,
            max_packet_size: config
                .endpoint_config
                .max_packet_size
                .min(u32::MAX as usize) as u32,
            max_receive_alloc: config
                .endpoint_config
                .max_receive_alloc
                .min(u32::MAX as usize) as u32,
        });

        let request_bytes = request.write();
        let _ = socket.send_to(&request_bytes, dst_addr);

        // Initialize state object

        let state = State::Pending(PendingState {
            local_nonce: nonce,

            request_bytes,
            resend_time_ms: HANDSHAKE_RESEND_INTERVAL_MS,
            resend_count: HANDSHAKE_RESEND_COUNT,

            initial_sends: Vec::new(),
        });

        Ok(Self {
            socket,
            config,

            local_addr,
            remote_addr,

            time_base: time::Instant::now(),

            state,

            events_out: Vec::new(),
        })
    }

    /// Flushes outbound frames, then processes as many inbound frames as possible from the
    /// internal socket. Returns an iterator of [`Event`] objects to signal connection events and
    /// deliver packets received from the server.
    ///
    /// *Note 1*: All events are considered delivered, even if the iterator is not consumed until
    /// the end.
    ///
    /// *Note 2*: Internally, `uflow` uses the [leaky bucket
    /// algorithm](https://en.wikipedia.org/wiki/Leaky_bucket) to control the rate at which UDP
    /// frames are sent. To ensure that data is transferred smoothly, this function should be
    /// called regularly and relatively frequently.
    pub fn step(&mut self) -> impl Iterator<Item = Event> {
        let now_ms = self.now_ms();

        self.flush_if_active();

        self.handle_frames(now_ms);

        self.handle_events(now_ms);

        self.step_if_active(now_ms);

        std::mem::take(&mut self.events_out).into_iter()
    }

    /// Sends as many outbound frames as possible.
    pub fn flush(&mut self) {
        self.flush_if_active();
    }

    /// Returns `true` if the connection is active, that is, a connection handshake has been
    /// completed and the remote host has not yet timed out or disconnected. Returns `false`
    /// otherwise.
    pub fn is_active(&self) -> bool {
        match self.state {
            State::Active(_) => true,
            _ => false,
        }
    }

    /// Enqueues a packet for delivery to the server. The packet will be sent on the given channel
    /// according to the specified mode.
    ///
    /// If a connection has not yet been established, the packet will remain enqueued until the
    /// connection succeeds. Otherwise, if the connection is not active, the packet will be
    /// silently discarded.
    ///
    /// # Error Handling
    ///
    /// This function will panic if `channel_id` does not refer to a valid channel (`channel_id >=
    /// CHANNEL_COUNT`), or if `data.len()` exceeds the [maximum packet
    /// size](crate::EndpointConfig#structfield.max_packet_size).
    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        assert!(data.len() <= self.config.endpoint_config.max_packet_size,
                "send failed: packet of size {} exceeds configured maximum of {}",
                data.len(),
                self.config.endpoint_config.max_packet_size);

        assert!(channel_id < CHANNEL_COUNT,
                "send failed: channel ID {} is invalid",
                channel_id);

        match self.state {
            State::Pending(ref mut state) => {
                state.initial_sends.push(SendEntry { data, channel_id: channel_id as u8, mode });
            }
            State::Active(ref mut state) => {
                state.half_connection.send(data, channel_id as u8, mode);
            }
            State::Closing(_) => {
                // Disconnecting, nothing to do
            }
            State::Closed(_) => {
                // Remote host has closed this connection, nothing to do
            }
            State::Fin => {
                // Connection is dead, nothing to do
            }
        }
    }

    /// Gracefully terminates this connection once all packets have been sent.
    ///
    /// If any outbound packets are pending, they will be sent prior to disconnecting. Reliable
    /// packets can be assumed to have been delievered, so long as the server does not also
    /// disconnect in the meantime. The connection will remain active until the next call to
    /// [`Client::step()`] with no pending outbound packets.
    pub fn disconnect(&mut self) {
        match self.state {
            State::Pending(_) => {
                // No point in assuming the server will reply, so enter fin immediately
                self.state = State::Fin;
            }
            State::Active(ref mut state) => {
                state.disconnect_signal = Some(DisconnectMode::Flush);
            }
            _ => (),
        }
    }

    /// Gracefully terminates this connection as soon as possible.
    ///
    /// If any outbound packets are pending, they may be flushed prior to disconnecting, but no
    /// packets are guaranteed to be received by the server. The connection will remain active
    /// until the next call to [`Client::step()`].
    pub fn disconnect_now(&mut self) {
        match self.state {
            State::Pending(_) => {
                // No point in assuming the server will reply, so enter fin immediately
                self.state = State::Fin;
            }
            State::Active(ref mut state) => {
                state.disconnect_signal = Some(DisconnectMode::Now);
            }
            _ => (),
        }
    }

    /// Returns the local address of the internal UDP socket.
    pub fn local_address(&self) -> net::SocketAddr {
        self.local_addr
    }

    /// Returns the address of the server.
    pub fn remote_address(&self) -> net::SocketAddr {
        self.remote_addr
    }

    /// Returns the current estimate of the round-trip time (RTT), in seconds.
    ///
    /// If the RTT has not yet been computed, `None` is returned instead.
    pub fn rtt_s(&self) -> Option<f64> {
        match self.state {
            State::Active(ref state) => state.half_connection.rtt_s(),
            _ => None,
        }
    }

    /// Returns the combined size of all outstanding packets (i.e. those which have not yet been
    /// acknowledged), in bytes.
    ///
    /// This figure represents the amount of memory allocated for outgoing packets. Packets which
    /// are marked [`TimeSensitive`](SendMode::TimeSensitive) are included in this total, even if
    /// they would not be sent.
    pub fn send_buffer_size(&self) -> usize {
        match self.state {
            State::Active(ref state) => state.half_connection.send_buffer_size(),
            _ => 0,
        }
    }

    fn now_ms(&self) -> u64 {
        let now = time::Instant::now();
        (now - self.time_base).as_millis() as u64
    }

    fn handle_handshake_syn_ack(&mut self, frame: frame::HandshakeSynAckFrame) {
        match self.state {
            State::Pending(ref mut state) => {
                // If the server responds to our SYN with a matching SYN+ACK, it has already
                // validated the connection parameters, so initialize an active connection. Since
                // it is trivial for either party to intentionally stall a connection, there is no
                // need for the client to double check the protocol version or connection
                // parameters.
                //
                // Otherwise, if the nonce does not match, a duplicated SYN+ACK from a previous
                // handshake must have been received. Ignore that.

                if frame.nonce_ack == state.local_nonce {
                    let reply = frame::Frame::HandshakeAckFrame(frame::HandshakeAckFrame {
                        nonce_ack: frame.nonce,
                    });
                    let _ = self.socket.send(&reply.write());

                    use crate::packet_id;

                    let config = half_connection::Config {
                        tx_frame_window_size: MAX_FRAME_WINDOW_SIZE,
                        rx_frame_window_size: MAX_FRAME_WINDOW_SIZE,

                        tx_frame_base_id: state.local_nonce,
                        rx_frame_base_id: frame.nonce,

                        tx_packet_window_size: MAX_PACKET_WINDOW_SIZE,
                        rx_packet_window_size: MAX_PACKET_WINDOW_SIZE,

                        tx_packet_base_id: state.local_nonce & packet_id::MASK,
                        rx_packet_base_id: frame.nonce & packet_id::MASK,

                        tx_bandwidth_limit: (self.config.endpoint_config.max_send_rate as u32).min(frame.max_receive_rate),

                        tx_alloc_limit: frame.max_receive_alloc as usize,
                        rx_alloc_limit: self.config.endpoint_config.max_receive_alloc as usize,

                        keepalive_interval_ms: if self.config.endpoint_config.keepalive {
                            Some(self.config.endpoint_config.keepalive_interval_ms)
                        } else {
                            None
                        },
                    };

                    let mut half_connection = half_connection::HalfConnection::new(config);

                    let initial_sends = std::mem::take(&mut state.initial_sends);
                    for initial_send in initial_sends.into_iter() {
                        half_connection.send(initial_send.data, initial_send.channel_id, initial_send.mode);
                    }

                    // Initialize connection and signal connect
                    self.events_out.push(Event::Connect);

                    self.state = State::Active(ActiveState {
                        local_nonce: state.local_nonce,
                        half_connection,
                        timeout_time_ms: self.config.endpoint_config.active_timeout_ms,
                        disconnect_signal: None,
                    });
                }
            }
            State::Active(ref state) => {
                // A matching SYN+ACK has already been received, so acknowledge this one assuming
                // the nonce ack matches ours (and ignore it otherwise). This case is only
                // encountered when our initial ACK was dropped - all that matters is that the
                // server receives an ACK.

                if frame.nonce_ack == state.local_nonce {
                    let reply = frame::Frame::HandshakeAckFrame(frame::HandshakeAckFrame {
                        nonce_ack: frame.nonce,
                    });
                    let _ = self.socket.send(&reply.write());
                }
            }
            _ => (),
        }
    }

    fn handle_handshake_error(&mut self, frame: frame::HandshakeErrorFrame) {
        // Receiving a handshake error frame signals that the connection request was somehow
        // impossible. If the response matches our SYN, forward this to the user as a handshake
        // error and forget the connection.

        match self.state {
            State::Pending(ref state) => {
                if frame.nonce_ack == state.local_nonce {
                    let error_type = match frame.error {
                        frame::HandshakeErrorType::Version => ErrorType::Version,
                        frame::HandshakeErrorType::Config => ErrorType::Config,
                        frame::HandshakeErrorType::ServerFull => ErrorType::ServerFull,
                    };

                    // Forget connection and signal appropriate handshake error
                    self.events_out.push(Event::Error(error_type));
                    self.state = State::Fin;
                }
            }
            _ => (),
        }
    }

    fn handle_disconnect(&mut self, now_ms: u64) {
        // Receiving a disconnect while active or closing terminates the connection immediately.
        // The original and subsequent requests are acknowledged for a finite amount of time.

        // TODO: Should a disconnect frame contain a nonce field which references the current
        // connection?

        match self.state {
            State::Pending(_) => {
                // A client has no reason to respond to a disconnect frame while the connection is
                // pending
            },
            State::Active(ref mut state) => {
                let reply = frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame {});
                let _ = self.socket.send(&reply.write());

                // Signal any remaining received packets prior to connection destruction
                state.half_connection.receive(&mut PacketReceiveSink::new(&mut self.events_out));

                // Signal disconnect
                self.events_out.push(Event::Disconnect);

                // Close now, but forget after a timeout
                self.state = State::Closed(ClosedState {
                    timeout_time_ms: now_ms + CLOSED_TIMEOUT_MS,
                });
            },
            State::Closing(_) => {
                // This may as well be an acknowledgement
                let reply = frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame {});
                let _ = self.socket.send(&reply.write());

                // Signal disconnect
                self.events_out.push(Event::Disconnect);

                // Close now, but forget after a timeout
                self.state = State::Closed(ClosedState {
                    timeout_time_ms: now_ms + CLOSED_TIMEOUT_MS,
                });
            },
            State::Closed(_) => {
                // Acknowledge subsequent disconnection requests
                let reply = frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame {});
                let _ = self.socket.send(&reply.write());
            },
            State::Fin => (),
        }
    }

    fn handle_disconnect_ack(&mut self) {
        match self.state {
            State::Closing(_) => {
                // Signal disconnect and forget connection
                self.events_out.push(Event::Disconnect);
                self.state = State::Fin;
            }
            _ => (),
        }
    }

    fn handle_data(&mut self, now_ms: u64, frame: frame::DataFrame) {
        match self.state {
            State::Active(ref mut state) => {
                state.half_connection.handle_data_frame(frame);
                state.timeout_time_ms = now_ms + self.config.endpoint_config.active_timeout_ms;
            }
            _ => (),
        }
    }

    fn handle_sync(&mut self, now_ms: u64, frame: frame::SyncFrame) {
        match self.state {
            State::Active(ref mut state) => {
                state.half_connection.handle_sync_frame(frame);
                state.timeout_time_ms = now_ms + self.config.endpoint_config.active_timeout_ms;
            }
            _ => (),
        }
    }

    fn handle_ack(&mut self, now_ms: u64, frame: frame::AckFrame) {
        match self.state {
            State::Active(ref mut state) => {
                state.half_connection.handle_ack_frame(frame);
                state.timeout_time_ms = now_ms + self.config.endpoint_config.active_timeout_ms;
            }
            _ => (),
        }
    }

    fn handle_frame(&mut self, frame: frame::Frame, now_ms: u64) {
        match frame {
            frame::Frame::HandshakeSynFrame(_) => (),
            frame::Frame::HandshakeAckFrame(_) => (),
            frame::Frame::HandshakeSynAckFrame(frame) => {
                self.handle_handshake_syn_ack(frame);
            },
            frame::Frame::HandshakeErrorFrame(frame) => {
                self.handle_handshake_error(frame);
            }
            frame::Frame::DisconnectFrame(_frame) => {
                self.handle_disconnect(now_ms);
            }
            frame::Frame::DisconnectAckFrame(_frame) => {
                self.handle_disconnect_ack();
            }
            frame::Frame::DataFrame(frame) => {
                self.handle_data(now_ms, frame);
            }
            frame::Frame::SyncFrame(frame) => {
                self.handle_sync(now_ms, frame);
            }
            frame::Frame::AckFrame(frame) => {
                self.handle_ack(now_ms, frame);
            }
        }
    }

    fn handle_frames(&mut self, now_ms: u64) {
        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok(frame_size) = self.socket.recv(&mut frame_data_buf) {
            if let Some(frame) = frame::Frame::read(&frame_data_buf[..frame_size]) {
                self.handle_frame(frame, now_ms);
            }
        }
    }

    fn handle_events(&mut self, now_ms: u64) {
        match self.state {
            State::Pending(ref mut state) => {
                if now_ms >= state.resend_time_ms {
                    if state.resend_count > 0 {
                        // Resend SYN
                        let _ = self.socket.send(&state.request_bytes);
                        state.resend_time_ms = now_ms + HANDSHAKE_RESEND_INTERVAL_MS;
                        state.resend_count -= 1;
                    } else {
                        // Signal timeout and forget connection
                        self.events_out.push(Event::Error(ErrorType::Timeout));
                        self.state = State::Fin;
                    }
                }
            }
            State::Active(ref state) => {
                if now_ms >= state.timeout_time_ms {
                    // Signal timeout and forget connection
                    self.events_out.push(Event::Error(ErrorType::Timeout));
                    self.state = State::Fin;
                }
            }
            State::Closing(ref mut state) => {
                if now_ms >= state.resend_time_ms {
                    if state.resend_count > 0 {
                        // Resend disconnect request
                        let _ = self.socket.send(&state.request_bytes);
                        state.resend_time_ms = now_ms + DISCONNECT_RESEND_INTERVAL_MS;
                        state.resend_count -= 1;
                    } else {
                        // Signal timeout and forget connection
                        self.events_out.push(Event::Error(ErrorType::Timeout));
                        self.state = State::Fin;
                    }
                }
            }
            State::Closed(ref state) => {
                if now_ms >= state.timeout_time_ms {
                    // Forget connection at last (disconnect has already been signaled)
                    self.state = State::Fin;
                }
            }
            State::Fin => (),
        }
    }

    fn step_if_active(&mut self, now_ms: u64) {
        match self.state {
            State::Active(ref mut state) => {
                let disconnect_now = match state.disconnect_signal {
                    Some(DisconnectMode::Now) => true,
                    Some(DisconnectMode::Flush) => !state.half_connection.is_send_pending(),
                    None => false,
                };

                if disconnect_now {
                    // Signal remaining received packets
                    state.half_connection.receive(&mut PacketReceiveSink::new(&mut self.events_out));

                    // Attempt to close the connection
                    let request_bytes = frame::Frame::DisconnectFrame(frame::DisconnectFrame {}).write();
                    let _ = self.socket.send(&request_bytes);

                    self.state = State::Closing(ClosingState {
                        request_bytes,
                        resend_time_ms: now_ms + DISCONNECT_RESEND_INTERVAL_MS,
                        resend_count: DISCONNECT_RESEND_COUNT,
                    });
                } else {
                    // Process and signal received packets
                    state.half_connection.step();
                    state.half_connection.receive(&mut PacketReceiveSink::new(&mut self.events_out));
                }
            }
            _ => (),
        }
    }

    fn flush_if_active(&mut self) {
        match self.state {
            State::Active(ref mut state) => {
                let ref mut data_sink = UdpFrameSink::new(&self.socket, self.remote_addr);
                state.half_connection.flush(data_sink);
            }
            _ => (),
        }
    }
}
