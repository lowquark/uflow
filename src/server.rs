
use crate::MAX_FRAME_SIZE;
use crate::frame;

use super::endpoint;
use super::peer;
use super::udp_frame_sink::UdpFrameSink;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;

/// A polling-based socket object which manages inbound `uflow` connections.
pub struct Server {
    socket: net::UdpSocket,

    max_peer_count: usize,
    peer_config: endpoint::Config,

    endpoints: HashMap<net::SocketAddr, Rc<RefCell<endpoint::Endpoint>>>,
    incoming_peers: Vec<peer::Peer>,
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
    pub fn bind<A: net::ToSocketAddrs>(addr: A,
                                       max_peer_count: usize,
                                       peer_config: endpoint::Config) -> Result<Self, std::io::Error> {
        assert!(peer_config.is_valid(), "invalid endpoint config");

        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        Ok(Self {
            socket,

            endpoints: HashMap::new(),
            incoming_peers: Vec::new(),

            max_peer_count,
            peer_config,
        })
    }

    /// Equivalent to calling [`bind()`](Self::bind) with address
    /// `(`[`std::net::Ipv4Addr::UNSPECIFIED`](std::net::Ipv4Addr::UNSPECIFIED)`, 0)`.
    pub fn bind_any_ipv4(max_peer_count: usize, peer_config: endpoint::Config) -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv4Addr::UNSPECIFIED, 0), max_peer_count, peer_config)
    }

    /// Equivalent to calling [`bind()`](Self::bind) with address
    /// `(`[`std::net::Ipv6Addr::UNSPECIFIED`](std::net::Ipv6Addr::UNSPECIFIED)`, 0)`.
    pub fn bind_any_ipv6(max_peer_count: usize, peer_config: endpoint::Config) -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv6Addr::UNSPECIFIED, 0), max_peer_count, peer_config)
    }

    /// Returns a number of [`Peer`](peer::Peer) objects representing new connections since the
    /// last call to [`incoming()`](Self::incoming). Any connections which have since timed out
    /// will not be returned.
    pub fn incoming(&mut self) -> impl Iterator<Item = peer::Peer> {
        std::mem::take(&mut self.incoming_peers).into_iter()
    }

    /// Reads as many UDP frames as possible from the internal socket, and updates the states of
    /// active connections accordingly. Call [`Peer::poll_events()`](peer::Peer::poll_events) after
    /// calling this function to retrieve incoming packets and connection status updates for an
    /// individual peer.
    ///
    /// *Note*: Internally, `uflow` uses the [leaky bucket
    /// algorithm](https://en.wikipedia.org/wiki/Leaky_bucket) to control the rate at which UDP
    /// frames are sent. To ensure that data is transferred smoothly, this function should be
    /// called relatively frequently (a minimum of once per connection round-trip time).
    pub fn step(&mut self) {
        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok((frame_size, address)) = self.socket.recv_from(&mut frame_data_buf) {
            use frame::serial::Serialize;

            if let Some(frame) = frame::Frame::read(&frame_data_buf[ .. frame_size]) {
                self.handle_frame(address, frame);
            }
        }

        for (_, endpoint) in self.endpoints.iter_mut() {
            endpoint.borrow_mut().step();
        }

        self.endpoints.retain(|_, endpoint| !endpoint.borrow().is_zombie());
        self.incoming_peers.retain(|client| !client.is_zombie());
    }

    /// Sends as many pending outbound frames (packet data, acknowledgements, keep-alives, etc.) as
    /// possible for each peer.
    pub fn flush(&mut self) {
        for (&address, endpoint) in self.endpoints.iter_mut() {
            let ref mut data_sink = UdpFrameSink::new(&self.socket, address);
            endpoint.borrow_mut().flush(data_sink);
        }
    }

    /// Returns the local address of the internal UDP socket.
    pub fn address(&self) -> net::SocketAddr {
        self.socket.local_addr().unwrap()
    }

    fn handle_frame(&mut self, address: net::SocketAddr, frame: frame::Frame) {
        if let Some(endpoint) = self.endpoints.get_mut(&address) {
            let ref mut data_sink = UdpFrameSink::new(&self.socket, address);
            endpoint.borrow_mut().handle_frame(frame, data_sink);
        } else {
            if self.endpoints.len() < self.max_peer_count as usize {
                let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

                let mut endpoint = endpoint::Endpoint::new(self.peer_config.clone());
                endpoint.handle_frame(frame, data_sink);

                let endpoint_ref = Rc::new(RefCell::new(endpoint));
                self.endpoints.insert(address, Rc::clone(&endpoint_ref));
                self.incoming_peers.push(peer::Peer::new(address, endpoint_ref));
            }
        }
    }
}

