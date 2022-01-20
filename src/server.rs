
use crate::MAX_FRAME_SIZE;
use crate::frame;

use super::endpoint;
use super::peer;
use super::udp_frame_sink::UdpFrameSink;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;

pub struct Server {
    socket: net::UdpSocket,

    max_connections: usize,
    incoming_cfg: endpoint::Cfg,

    endpoints: HashMap<net::SocketAddr, Rc<RefCell<endpoint::Endpoint>>>,
    incoming_peers: Vec<peer::Peer>,
}

impl Server {
    pub fn bind<A: net::ToSocketAddrs>(addr: A,
                                       max_connections: usize,
                                       incoming_cfg: endpoint::Cfg) -> Result<Self, std::io::Error> {
        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        Ok(Self {
            socket,

            endpoints: HashMap::new(),
            incoming_peers: Vec::new(),

            max_connections,
            incoming_cfg,
        })
    }

    pub fn bind_any(max_connections: usize, incoming_cfg: endpoint::Cfg) -> Result<Self, std::io::Error> {
        Self::bind((net::Ipv4Addr::UNSPECIFIED, 0), max_connections, incoming_cfg)
    }

    pub fn handle_frame(&mut self, address: net::SocketAddr, frame: frame::Frame) {
        if let Some(endpoint) = self.endpoints.get_mut(&address) {
            let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

            endpoint.borrow_mut().handle_frame(frame, data_sink);
        } else {
            if self.endpoints.len() < self.max_connections as usize {
                let ref mut data_sink = UdpFrameSink::new(&self.socket, address);

                let mut endpoint = endpoint::Endpoint::new(self.incoming_cfg.clone());
                endpoint.handle_frame(frame, data_sink);

                let endpoint_ref = Rc::new(RefCell::new(endpoint));
                self.endpoints.insert(address, Rc::clone(&endpoint_ref));
                self.incoming_peers.push(peer::Peer::new(address, endpoint_ref));
            }
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
        self.incoming_peers.retain(|client| !client.is_zombie());
    }

    pub fn incoming(&mut self) -> impl Iterator<Item = peer::Peer> {
        std::mem::take(&mut self.incoming_peers).into_iter()
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

