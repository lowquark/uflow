use std::net;
use std::time;

use crate::CHANNEL_COUNT;
use crate::EndpointConfig;
use crate::endpoint::daten_meister;
use crate::frame;
use crate::frame::serial::Serialize;
use crate::udp_frame_sink::UdpFrameSink;
use crate::MAX_FRAME_SIZE;
use crate::PROTOCOL_VERSION;
use crate::MAX_FRAME_WINDOW_SIZE;
use crate::MAX_PACKET_WINDOW_SIZE;
use crate::SendMode;

static HANDSHAKE_RESEND_INTERVAL_MS: u64 = 2000;
static HANDSHAKE_RESEND_COUNT: u8 = 8;

static ACTIVE_TIMEOUT_MS: u64 = 15000;

static DISCONNECT_RESEND_INTERVAL_MS: u64 = 2000;
static DISCONNECT_RESEND_COUNT: u8 = 8;

static CLOSED_TIMEOUT_MS: u64 = 15000;

pub struct Config {
    pub peer_config: EndpointConfig,
}

impl Config {
    pub fn is_valid(&self) -> bool {
        return self.peer_config.is_valid();
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
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
    Connect,
    Disconnect,
    Receive(Box<[u8]>),
    Error(ErrorType),
}

struct EventPacketSink<'a> {
    event_queue: &'a mut Vec<Event>,
}

impl<'a> EventPacketSink<'a> {
    fn new(event_queue: &'a mut Vec<Event>) -> Self {
        Self {
            event_queue,
        }
    }
}

impl<'a> daten_meister::PacketSink for EventPacketSink<'a> {
    fn send(&mut self, packet_data: Box<[u8]>) {
        self.event_queue.push(Event::Receive(packet_data));
    }
}

struct SendEntry {
    data: Box<[u8]>,
    channel_id: u8,
    mode: SendMode,
}

pub struct PendingState {
    local_nonce: u32,

    request_bytes: Box<[u8]>,
    resend_time_ms: u64,
    resend_count: u8,

    initial_sends: Vec<SendEntry>,
}

pub struct ActiveState {
    local_nonce: u32,
    endpoint: daten_meister::DatenMeister,
    disconnect_flush: bool,
    timeout_time_ms: u64,
}

pub struct ClosingState {
    request_bytes: Box<[u8]>,
    resend_time_ms: u64,
    resend_count: u8,
}

pub struct ClosedState {
    timeout_time_ms: u64,
}

enum State {
    Pending(PendingState),
    Active(ActiveState),
    Closing(ClosingState),
    Closed(ClosedState),
    Fin,
}

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
    /// Opens a non-blocking UDP socket bound to the provided address, and creates a corresponding
    /// [`Client`](Self) object.
    ///
    /// # Error Handling
    ///
    /// Any errors resulting from socket initialization are forwarded to the caller.
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
                .peer_config
                .max_receive_rate
                .min(u32::MAX as usize) as u32,
            max_packet_size: config
                .peer_config
                .max_packet_size
                .min(u32::MAX as usize) as u32,
            max_receive_alloc: config
                .peer_config
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

    /// *Note*: Internally, `uflow` uses the [leaky bucket
    /// algorithm](https://en.wikipedia.org/wiki/Leaky_bucket) to control the rate at which UDP
    /// frames are sent. To ensure that data is transferred smoothly, this function should be
    /// called relatively frequently (a minimum of once per connection round-trip time).
    pub fn step(&mut self) -> impl Iterator<Item = Event> {
        let now_ms = self.now_ms();

        self.flush_if_active(now_ms);

        self.handle_frames(now_ms);

        self.handle_events(now_ms);

        self.step_if_active();

        std::mem::take(&mut self.events_out).into_iter()
    }

    /// Sends as many pending outbound frames (packet data, acknowledgements, keep-alives, etc.) as
    /// possible.
    pub fn flush(&mut self) {
        let now_ms = self.now_ms();
        self.flush_if_active(now_ms);
    }

    /// Enqueues a packet for delivery to the remote host. The packet will be sent on the given
    /// channel according to the specified mode.
    ///
    /// # Error Handling
    ///
    /// This function will panic if `channel_id` does not refer to a valid channel (i.e.
    /// if `channel_id >= CHANNEL_COUNT`), or if `data.len()` exceeds the [maximum packet
    /// size](endpoint::Config#structfield.max_packet_size).
    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        assert!(data.len() <= self.config.peer_config.max_packet_size,
                "send failed: packet of size {} exceeds configured maximum of {}",
                data.len(),
                self.config.peer_config.max_packet_size);

        assert!(channel_id < CHANNEL_COUNT,
                "send failed: channel ID {} is invalid",
                channel_id);

        match self.state {
            State::Pending(ref mut state) => {
                state.initial_sends.push(SendEntry { data, channel_id: channel_id as u8, mode });
            }
            State::Active(ref mut state) => {
                state.endpoint.send(data, channel_id as u8, mode);
            }
            _ => (),
        }
    }

    /// Explicitly terminates the connection, notifying the remote host in the process.
    ///
    /// If the `Peer` is currently connected, all pending packets will be sent prior to
    /// disconnecting, and a [`Disconnect`](Event::Disconnect) event will be generated once the
    /// disconnection is complete.
    pub fn disconnect(&mut self) {
        match self.state {
            State::Pending(ref mut state) => {
                // No point in assuming the server will reply, so enter fin immediately
                self.state = State::Fin;
            }
            State::Active(ref mut state) => {
                state.disconnect_flush = true;
            }
            _ => (),
        }
    }

    /// Returns the local address of the internal UDP socket.
    pub fn local_addr(&self) -> net::SocketAddr {
        self.local_addr
    }

    /// Returns the address of the remote host.
    pub fn remote_addr(&self) -> net::SocketAddr {
        self.remote_addr
    }

    /// Returns the current estimate of the round-trip time (RTT), in seconds.
    ///
    /// If the RTT has not yet been computed, `None` is returned instead.
    pub fn rtt_s(&self) -> Option<f64> {
        match self.state {
            State::Active(ref state) => state.endpoint.rtt_s(),
            _ => None,
        }
    }

    /// Returns the combined size of all outstanding packets (i.e. those which have not yet been
    /// acknowledged by the server), in bytes.
    ///
    /// This figure represents the amount of memory allocated by outgoing packets. Thus, packets
    /// which are marked [`Time-Sensitive`](SendMode::TimeSensitive) are included in this total,
    /// even if they would not be sent.
    pub fn send_buffer_size(&self) -> usize {
        match self.state {
            State::Active(ref state) => state.endpoint.send_buffer_size(),
            _ => 0,
        }
    }

    fn now_ms(&self) -> u64 {
        let now = time::Instant::now();
        (now - self.time_base).as_millis() as u64
    }

    fn handle_handshake_syn_ack(&mut self, frame: frame::HandshakeSynAckFrame) {
        match self.state {
            State::Pending(ref state) => {
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

                    let config = daten_meister::Config {
                        tx_frame_window_size: MAX_FRAME_WINDOW_SIZE,
                        rx_frame_window_size: MAX_FRAME_WINDOW_SIZE,

                        tx_frame_base_id: state.local_nonce,
                        rx_frame_base_id: frame.nonce,

                        tx_packet_window_size: MAX_PACKET_WINDOW_SIZE,
                        rx_packet_window_size: MAX_PACKET_WINDOW_SIZE,

                        tx_packet_base_id: state.local_nonce & packet_id::MASK,
                        rx_packet_base_id: frame.nonce & packet_id::MASK,

                        tx_bandwidth_limit: (self.config.peer_config.max_send_rate as u32).min(frame.max_receive_rate),

                        tx_alloc_limit: frame.max_receive_alloc as usize,
                        rx_alloc_limit: self.config.peer_config.max_receive_alloc as usize,

                        keepalive: self.config.peer_config.keepalive,
                    };

                    let daten_meister = daten_meister::DatenMeister::new(config);

                    // Initialize connection and signal connect
                    self.events_out.push(Event::Connect);

                    self.state = State::Active(ActiveState {
                        local_nonce: state.local_nonce,
                        endpoint: daten_meister,
                        disconnect_flush: false,
                        timeout_time_ms: ACTIVE_TIMEOUT_MS,
                    });
                } else {
                    // Forget connection and signal handshake error
                    self.events_out.push(Event::Error(ErrorType::HandshakeError));
                    self.state = State::Fin;
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

        // TODO: This frame type needs a nonce_ack field!

        match self.state {
            State::Pending(_) => {
                // Forget connection and signal handshake error
                self.events_out.push(Event::Error(ErrorType::HandshakeError));
                self.state = State::Fin;
            }
            _ => (),
        }
    }

    fn handle_disconnect(&mut self, now_ms: u64) {
        // Receiving a disconnect in any state (other than fin) terminates the connection
        // immediately. The original and subsequent requests are acknowledged for a finite amount
        // of time.

        // TODO: Should a disconnect frame contain a nonce field which references the current
        // connection?

        match self.state {
            State::Fin => return,
            _ => (),
        }

        let reply = frame::Frame::DisconnectAckFrame(frame::DisconnectAckFrame {});
        let _ = self.socket.send(&reply.write());

        // Signal remaining received packets prior to endpoint destruction
        match self.state {
            State::Active(ref mut state) => {
                state.endpoint.receive(&mut EventPacketSink::new(&mut self.events_out));
            }
            _ => (),
        }

        // Signal disconnect
        self.events_out.push(Event::Disconnect);

        // Close now, but forget after a timeout
        self.state = State::Closed(ClosedState {
            timeout_time_ms: now_ms + CLOSED_TIMEOUT_MS,
        });
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
                state.endpoint.handle_data_frame(frame);
                state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
            }
            _ => (),
        }
    }

    fn handle_sync(&mut self, now_ms: u64, frame: frame::SyncFrame) {
        match self.state {
            State::Active(ref mut state) => {
                state.endpoint.handle_sync_frame(frame);
                state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
            }
            _ => (),
        }
    }

    fn handle_ack(&mut self, now_ms: u64, frame: frame::AckFrame) {
        match self.state {
            State::Active(ref mut state) => {
                state.endpoint.handle_ack_frame(frame);
                state.timeout_time_ms = now_ms + ACTIVE_TIMEOUT_MS;
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
            _ => (),
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
                        self.events_out.push(Event::Error(ErrorType::HandshakeTimeout));
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
            _ => (),
        }
    }

    fn step_if_active(&mut self) {
        match self.state {
            State::Active(ref mut state) => {
                // Process and signal received packets
                state.endpoint.step();
                state.endpoint.receive(&mut EventPacketSink::new(&mut self.events_out));
            }
            _ => (),
        }
    }

    fn flush_if_active(&mut self, now_ms: u64) {
        match self.state {
            State::Active(ref mut state) => {
                let ref mut data_sink = UdpFrameSink::new(&self.socket, self.remote_addr);
                state.endpoint.flush(data_sink);

                if state.disconnect_flush && !state.endpoint.is_send_pending() {
                    // Signal remaining received packets
                    state.endpoint.receive(&mut EventPacketSink::new(&mut self.events_out));

                    // Attempt to close the connection
                    let request_bytes = frame::Frame::DisconnectFrame(frame::DisconnectFrame {}).write();
                    let _ = self.socket.send(&request_bytes);

                    self.state = State::Closing(ClosingState {
                        request_bytes,
                        resend_time_ms: now_ms + DISCONNECT_RESEND_INTERVAL_MS,
                        resend_count: DISCONNECT_RESEND_COUNT,
                    });
                }
            }
            _ => (),
        }
    }
}
