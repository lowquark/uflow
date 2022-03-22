
use crate::MAX_FRAME_SIZE;
use crate::frame;

use super::endpoint;
use super::peer;
use super::udp_frame_sink::UdpFrameSink;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;

/// A polling-based socket object which manages outbound `uflow` connections.
pub struct Client {
    socket: net::UdpSocket,
    endpoints: HashMap<net::SocketAddr, Rc<RefCell<endpoint::Endpoint>>>,
}

impl Client {
    /// Opens a non-blocking UDP socket bound to the provided address, and creates a corresponding
    /// [`Client`](Self) object.
    ///
    /// # Error Handling
    ///
    /// Any errors resulting from socket initialization are forwarded to the caller.
    pub fn bind<A: net::ToSocketAddrs>(addr: A) -> Result<Self, std::io::Error> {
        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        Ok(Self {
            socket,
            endpoints: HashMap::new(),
        })
    }

    /// Equivalent to calling [`bind()`](Self::bind) with
    /// `(`[`std::net::Ipv4Addr::UNSPECIFIED`](std::net::Ipv4Addr::UNSPECIFIED)`, 0)`.
    pub fn bind_any_ipv4() -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv4Addr::UNSPECIFIED, 0))
    }

    /// Equivalent to calling [`bind()`](Self::bind) with
    /// `(`[`std::net::Ipv6Addr::UNSPECIFIED`](std::net::Ipv6Addr::UNSPECIFIED)`, 0)`.
    pub fn bind_any_ipv6() -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv6Addr::UNSPECIFIED, 0))
    }

    /// Initiates a connection to a remote host at the specified address, and returns a
    /// [`Peer`](peer::Peer) object representing the connection.
    ///
    /// # Error Handling
    ///
    /// Any errors resulting from address resolution are forwarded to the caller. This function
    /// will panic if the given endpoint configuration is not valid.
    pub fn connect<A: net::ToSocketAddrs>(&mut self, addr: A, cfg: endpoint::Config) -> Result<peer::Peer, std::io::Error> {
        assert!(cfg.is_valid(), "invalid endpoint config");

        let endpoint = endpoint::Endpoint::new(cfg);
        let endpoint_ref = Rc::new(RefCell::new(endpoint));

        let address = addr.to_socket_addrs()?.next().expect("no useful socket addresses");

        self.endpoints.insert(address, endpoint_ref.clone());
        return Ok(peer::Peer::new(address, endpoint_ref));
    }

    /// Processes UDP frames received since the previous call to [`step()`](Self::step).
    ///
    /// Current [`Peer`](peer::Peer) objects will be updated as relevant data is received. Call
    /// [`Peer::poll_events()`](peer::Peer::poll_events) after calling this function to retrieve
    /// incoming packets and connection status updates for an individual peer.
    pub fn step(&mut self) {
        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok((frame_size, address)) = self.socket.recv_from(&mut frame_data_buf) {
            use frame::serial::Serialize;

            if let Some(frame) = frame::Frame::read(&frame_data_buf[ .. frame_size]) {
                self.handle_frame(address, frame);
            }
        }

        self.step_timeouts();

        self.endpoints.retain(|_, endpoint| !endpoint.borrow().is_zombie());
    }

    /// Sends pending outbound frames (acknowledgements, keep-alives, packet data, etc.) for each
    /// peer.
    ///
    /// *Note*: Internally, `uflow` uses the [leaky bucket
    /// algorithm](https://en.wikipedia.org/wiki/Leaky_bucket) to control the rate at which UDP
    /// frames are sent. Thus, this function should be called relatively frequently (at least once
    /// per connection round-trip time) to ensure that data is transferred smoothly. Regular
    /// intervals are best, but there is no penalty to making two calls in short succession.
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
        }
    }

    fn step_timeouts(&mut self) {
        for (_, endpoint) in self.endpoints.iter_mut() {
            endpoint.borrow_mut().step_timeouts();
        }
    }
}

