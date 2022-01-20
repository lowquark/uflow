
use crate::MAX_FRAME_SIZE;
use crate::frame;

use super::endpoint;
use super::peer;
use super::udp_frame_sink::UdpFrameSink;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;

pub struct Client {
    socket: net::UdpSocket,
    endpoints: HashMap<net::SocketAddr, Rc<RefCell<endpoint::Endpoint>>>,
}

impl Client {
    pub fn bind<A: net::ToSocketAddrs>(addr: A) -> Result<Self, std::io::Error> {
        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        Ok(Self {
            socket,
            endpoints: HashMap::new(),
        })
    }

    pub fn bind_any_ipv4() -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv4Addr::UNSPECIFIED, 0))
    }

    pub fn bind_any_ipv6() -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv6Addr::UNSPECIFIED, 0))
    }

    pub fn connect<A: net::ToSocketAddrs>(&mut self, addr: A, cfg: endpoint::Config) -> Result<peer::Peer, std::io::Error> {
        let endpoint = endpoint::Endpoint::new(cfg);
        let endpoint_ref = Rc::new(RefCell::new(endpoint));

        let address = addr.to_socket_addrs()?.next().expect("No useful socket addresses");

        self.endpoints.insert(address, endpoint_ref.clone());
        return Ok(peer::Peer::new(address, endpoint_ref));
    }

    pub fn handle_frame(&mut self, address: net::SocketAddr, frame: frame::Frame) {
        if let Some(endpoint) = self.endpoints.get_mut(&address) {
            let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

            endpoint.borrow_mut().handle_frame(frame, data_sink);
        }
    }

    pub fn step(&mut self) {
        let mut frame_data_buf = [0; MAX_FRAME_SIZE];

        while let Ok((frame_size, address)) = self.socket.recv_from(&mut frame_data_buf) {
            use frame::serial::Serialize;

            if let Some(frame) = frame::Frame::read(&frame_data_buf[ .. frame_size]) {
                self.handle_frame(address, frame);
            }
        }

        self.flush();

        self.endpoints.retain(|_, endpoint| !endpoint.borrow().is_zombie());
    }

    pub fn flush(&mut self) {
        for (&address, endpoint) in self.endpoints.iter_mut() {
            let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

            endpoint.borrow_mut().flush(data_sink);
        }
    }

    pub fn address(&self) -> Option<net::SocketAddr> {
        self.socket.local_addr().ok()
    }
}

