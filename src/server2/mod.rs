mod pending_peer;

use std::collections::HashMap;
use std::net;
use std::rc::Rc;

use crate::endpoint;
use crate::frame;
use crate::MAX_FRAME_SIZE;
use crate::PeerId;
use crate::PROTOCOL_VERSION;
use crate::udp_frame_sink::UdpFrameSink;

pub struct Config {
    pub max_pending_connections: usize,
    pub max_active_connections: usize,
    pub peer_config: endpoint::Config,
}

impl Config {
    pub fn is_valid(&self) -> bool {
        return self.max_pending_connections > 0 &&
               self.max_active_connections > 0 &&
               self.max_pending_connections >= self.max_active_connections &&
               self.peer_config.is_valid();
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
}

pub enum Event {
    Connect(PeerId),
    Disconnect(PeerId),
    Error(PeerId, ErrorKind),
    Receive(PeerId, Box<[u8]>),
}

/// A polling-based socket object which manages inbound `uflow` connections.
pub struct Server {
    socket: net::UdpSocket,
    config: Config,

    pending_set: pending_peer::PendingSet,
    pending_resends: pending_peer::ResendQueue,

    endpoints: HashMap<net::SocketAddr, endpoint::Endpoint>,
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

            pending_set: pending_peer::PendingSet::new(),
            pending_resends: pending_peer::ResendQueue::new(),

            endpoints: HashMap::new(),
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

    /// Flushes pending outbound frames, and reads as many UDP frames as possible from the internal
    /// socket. Returns an iterator of `ServerEvents`s to signal connection status and deliver
    /// received packets for each peer.
    ///
    /// *Note 1*: All events are considered delivered, even if the iterator is not consumed until the
    /// end.
    ///
    /// *Note 2*: Internally, `uflow` uses the [leaky bucket
    /// algorithm](https://en.wikipedia.org/wiki/Leaky_bucket) to control the rate at which UDP
    /// frames are sent. To ensure that data is transferred smoothly, this function should be
    /// called relatively frequently (a minimum of once per connection round-trip time).
    pub fn step(&mut self) {
        self.flush();

        let time_now = 0;

        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok((frame_size, address)) = self.socket.recv_from(&mut frame_data_buf) {
            use frame::serial::Serialize;

            if let Some(frame) = frame::Frame::read(&frame_data_buf[ .. frame_size]) {
                self.handle_frame(address, frame, time_now);
            }
        }

        self.step_pending_peers(time_now);

        for (_, endpoint) in self.endpoints.iter_mut() {
            endpoint.step();
        }

        self.endpoints.retain(|_, endpoint| !endpoint.is_zombie());
    }

    /// Sends as many pending outbound frames (packet data, acknowledgements, keep-alives, etc.) as
    /// possible for each peer.
    pub fn flush(&mut self) {
        for (&address, endpoint) in self.endpoints.iter_mut() {
            let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

            endpoint.flush(data_sink);
        }
    }

    /// Returns the local address of the internal UDP socket.
    pub fn address(&self) -> net::SocketAddr {
        self.socket.local_addr().unwrap()
    }

    fn step_pending_peers(&mut self, time_now: u64) {
        loop {
            if let Some(entry) = self.pending_resends.peek() {
                if let Some(peer) = entry.peer.upgrade() {
                    if entry.resend_time <= time_now {
                        let mut entry = self.pending_resends.pop().unwrap();

                        println!("{:?}", peer.reply_bytes);

                        if entry.resend_count > 0 {
                            entry.resend_count -= 1;
                            entry.resend_time = time_now + 5000;

                            self.pending_resends.push(entry);
                        } else {
                            self.pending_set.remove(&entry.addr);
                        }
                    } else {
                        break;
                    }
                } else {
                    self.pending_resends.pop();
                }
            } else {
                break;
            }
        }
    }

    fn handle_frame(&mut self, address: net::SocketAddr, frame: frame::Frame, time_now: u64) {
        /*
        match frame {
            frame::HandshakeSynFrame(frame) => {
                self.handle_syn(address, frame, time_now);
            }
            frame::HandshakeAckFrame(frame) => {
                self.handle_ack(address, frame);
            }
            frame::HandshakeSynAckFrame(frame) => (),
            _ => (),
        }
        */

        /*
        if let Some(endpoint) = self.endpoints.get_mut(&address) {
            let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

            endpoint.handle_frame(frame, data_sink);
        } else {
            if self.endpoints.len() < self.max_peer_count as usize {
                let mut endpoint = endpoint::Endpoint::new(self.peer_config.clone());
                let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

                endpoint.handle_frame(frame, data_sink);

                self.endpoints.insert(address, endpoint);
            }
        }
        */
    }

    fn handle_syn(&mut self, addr: net::SocketAddr, handshake: frame::HandshakeSynFrame, time_now: u64) {
        if let Some(peer) = self.pending_set.get(&addr) {
            // Ignore subsequent SYN (likely a duplicate)
            return;
        }

        if handshake.version != PROTOCOL_VERSION {
            // Bad version
            let _error = frame::HandshakeErrorFrame {
                error: frame::HandshakeErrorType::Version,
            };

            // TODO: Serialize
            let reply_bytes = vec![0x00].into_boxed_slice();

            self.socket.send_to(&reply_bytes, addr);

            return;
        }

        if self.pending_set.len() >= self.config.max_pending_connections ||
           self.endpoints.len() >= self.config.max_active_connections {
            // No room in the inn
            let _error = frame::HandshakeErrorFrame {
                error: frame::HandshakeErrorType::Full,
            };

            // TODO: Serialize
            let reply_bytes = vec![0x00].into_boxed_slice();

            self.socket.send_to(&reply_bytes, addr);

            return;
        }

        let local_nonce = rand::random::<u32>();

        let reply = frame::HandshakeSynAckFrame {
            nonce_ack: handshake.nonce,
            nonce: local_nonce,
            max_receive_rate: self.config.peer_config.max_receive_rate.min(u32::MAX as usize) as u32,
            max_packet_size: self.config.peer_config.max_packet_size.min(u32::MAX as usize) as u32,
            max_receive_alloc: self.config.peer_config.max_receive_alloc.min(u32::MAX as usize) as u32,
        };

        // TODO: Serialize
        let reply_bytes = vec![0x00].into_boxed_slice();

        self.socket.send_to(&reply_bytes, addr);

        let peer = pending_peer::PendingPeer {
            local_nonce,
            remote_nonce: handshake.nonce,
            remote_max_receive_rate: handshake.max_receive_rate,
            remote_max_packet_size: handshake.max_packet_size,
            remote_max_receive_alloc: handshake.max_receive_alloc,
            reply_bytes,
        };

        let peer_rc = Rc::new(peer);

        self.pending_resends
            .push(pending_peer::ResendEntry::new(
                Rc::downgrade(&peer_rc),
                addr,
                time_now + 5000,
                5,
            ));

        self.pending_set.insert(addr, peer_rc);
    }

    fn handle_ack(&mut self, addr: net::SocketAddr, handshake: frame::HandshakeAckFrame) {
        if let Some(pending_peer) = self.pending_set.remove(&addr) {
            if handshake.nonce_ack != pending_peer.local_nonce {
                return;
            }

            // TODO: Create endpoint
            // self.endpoints.insert(addr, Peer::new(&pending_peer));
        }
    }
}
