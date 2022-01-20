
use crate::MAX_FRAME_SIZE;
use crate::frame;
use crate::endpoint;
use crate::SendMode;
use crate::Event;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;

struct UdpFrameSink<'a> {
    socket: &'a net::UdpSocket,
    address: net::SocketAddr,
}

impl<'a> UdpFrameSink<'a> {
    fn new(socket: &'a net::UdpSocket, address: net::SocketAddr) -> Self {
        Self {
            socket: socket,
            address: address,
        }
    }
}

impl<'a> endpoint::FrameSink for UdpFrameSink<'a> {
    fn send(&mut self, frame_data: &[u8]) {
        let _ = self.socket.send_to(frame_data, self.address);
    }
}

pub struct Peer {
    address: net::SocketAddr,
    endpoint_ref: Rc<RefCell<endpoint::Endpoint>>,
}

impl Peer {
    fn new(address: net::SocketAddr, endpoint_ref: Rc<RefCell<endpoint::Endpoint>>) -> Self {
        Self {
            address: address,
            endpoint_ref: endpoint_ref,
        }
    }

    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        self.endpoint_ref.borrow_mut().poll_events()
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        self.endpoint_ref.borrow_mut().send(data, channel_id, mode);
    }

    pub fn disconnect(&self) {
        self.endpoint_ref.borrow_mut().disconnect();
    }

    pub fn address(&self) -> net::SocketAddr {
        self.address
    }

    pub fn is_disconnected(&self) -> bool {
        let endpoint_ref = self.endpoint_ref.borrow();
        return endpoint_ref.is_zombie() || endpoint_ref.is_disconnected();
    }

    pub fn is_zombie(&self) -> bool {
        self.endpoint_ref.borrow().is_zombie()
    }

    pub fn rtt_ms(&self) -> f64 {
        self.endpoint_ref.borrow().rtt_ms()
    }
}

pub struct Host {
    socket: net::UdpSocket,

    max_connections: usize,
    incoming_params: endpoint::Params,

    endpoint_list: HashMap<net::SocketAddr, Rc<RefCell<endpoint::Endpoint>>>,
    new_clients: Vec<Peer>,
}

impl Host {
    pub fn bind<A: net::ToSocketAddrs>(addr: A, max_connections: usize, incoming_params: endpoint::Params) -> Result<Host, std::io::Error> {
        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        Ok(Host {
            socket,

            endpoint_list: HashMap::new(),
            new_clients: Vec::new(),

            max_connections,
            incoming_params,
        })
    }

    pub fn bind_any(max_connections: usize, incoming_params: endpoint::Params) -> Result<Host, std::io::Error> {
        Host::bind((net::Ipv4Addr::UNSPECIFIED, 0), max_connections, incoming_params)
    }

    // TODO: It would be possible to use special endpoint parameters here
    // As a bonus, if self.endpoint_params were set to None, incoming connections would not be accepted
    // TODO: Also need to consider how max_connections is interpreted w.r.t. outgoing connections
    pub fn connect(&mut self, addr: net::SocketAddr) -> Peer {
        let endpoint = endpoint::Endpoint::new(self.incoming_params.clone());

        let endpoint_ref = Rc::new(RefCell::new(endpoint));
        self.endpoint_list.insert(addr, endpoint_ref.clone());

        return Peer::new(addr, endpoint_ref);
    }

    pub fn handle_frame(&mut self, src_addr: net::SocketAddr, frame: frame::Frame) {
        let mut data_sink = UdpFrameSink::new(&self.socket, src_addr);

        match self.endpoint_list.get_mut(&src_addr) {
            Some(endpoint) => {
                endpoint.borrow_mut().handle_frame(frame, &mut data_sink);
            }
            None => {
                if self.endpoint_list.len() < self.max_connections as usize {
                    let mut endpoint = endpoint::Endpoint::new(self.incoming_params.clone());
                    endpoint.handle_frame(frame, &mut data_sink);

                    let endpoint_ref = Rc::new(RefCell::new(endpoint));
                    self.endpoint_list.insert(src_addr, Rc::clone(&endpoint_ref));
                    self.new_clients.push(Peer::new(src_addr, endpoint_ref));
                }
            }
        }
    }

    pub fn step(&mut self) {
        let mut recv_buf = [0; MAX_FRAME_SIZE];

        while let Ok((recv_size, src_addr)) = self.socket.recv_from(&mut recv_buf) {
            use frame::serial::Serialize;

            if let Some(frame) = frame::Frame::read(&recv_buf[..recv_size]) {
                self.handle_frame(src_addr, frame);
            }
        }

        self.flush();

        self.endpoint_list.retain(|_, endpoint| !endpoint.borrow().is_zombie());
        self.new_clients.retain(|client| !client.is_zombie());
    }

    pub fn incoming(&mut self) -> impl Iterator<Item = Peer> {
        std::mem::take(&mut self.new_clients).into_iter()
    }

    pub fn flush(&mut self) {
        for (address, endpoint) in self.endpoint_list.iter_mut() {
            let mut data_sink = UdpFrameSink::new(&self.socket, *address);
            endpoint.borrow_mut().flush(&mut data_sink);
        }
    }

    pub fn address(&self) -> Option<net::SocketAddr> {
        self.socket.local_addr().ok()
    }
}

