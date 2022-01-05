
use crate::MAX_TRANSFER_UNIT;
use crate::frame;
use crate::peer;
use crate::ChannelId;
use crate::FrameSink;
use crate::SendMode;

use frame::serial::Serialize;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::rc::Rc;

struct PeerFrameSink<'a> {
    socket: &'a net::UdpSocket,
    address: net::SocketAddr,
}

impl<'a> PeerFrameSink<'a> {
    fn new(socket: &'a net::UdpSocket, address: net::SocketAddr) -> Self {
        Self {
            socket: socket,
            address: address,
        }
    }
}

impl<'a> FrameSink for PeerFrameSink<'a> {
    fn send(&mut self, frame_data: &[u8]) {
        let _ = self.socket.send_to(frame_data, self.address);
    }
}

pub type Event = peer::Event;

pub struct Client {
    address: net::SocketAddr,
    peer_ref: Rc<RefCell<peer::Peer>>,
}

impl Client {
    fn new(address: net::SocketAddr, peer_ref: Rc<RefCell<peer::Peer>>) -> Self {
        Self {
            address: address,
            peer_ref: peer_ref,
        }
    }

    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        self.peer_ref.borrow_mut().poll_events()
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: ChannelId, mode: SendMode) {
        self.peer_ref.borrow_mut().send(data, channel_id, mode);
    }

    pub fn disconnect(&self) {
        self.peer_ref.borrow_mut().disconnect();
    }

    pub fn address(&self) -> net::SocketAddr {
        self.address
    }

    pub fn is_zombie(&self) -> bool {
        self.peer_ref.borrow().is_zombie()
    }

    pub fn rtt_ms(&self) -> f64 {
        self.peer_ref.borrow().rtt_ms()
    }
}

pub struct Host {
    socket: net::UdpSocket,

    max_connected_peers: usize,
    peer_params: peer::Params,

    peer_list: HashMap<net::SocketAddr, Rc<RefCell<peer::Peer>>>,
    new_clients: Vec<Client>,
}

impl Host {
    pub fn bind<A: net::ToSocketAddrs>(addr: A, max_connected_peers: usize, peer_params: peer::Params) -> Result<Host, std::io::Error> {
        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        Ok(Host {
            socket,

            peer_list: HashMap::new(),
            new_clients: Vec::new(),

            max_connected_peers,
            peer_params,
        })
    }

    pub fn bind_any(max_connected_peers: usize, peer_params: peer::Params) -> Result<Host, std::io::Error> {
        Host::bind((net::Ipv4Addr::UNSPECIFIED, 0), max_connected_peers, peer_params)
    }

    // TODO: It would be possible to use special peer parameters here
    // As a bonus, if self.peer_params were set to None, incoming connections would not be accepted
    pub fn connect(&mut self, addr: net::SocketAddr) -> Client {
        let peer = peer::Peer::new(self.peer_params.clone());

        let peer_ref = Rc::new(RefCell::new(peer));
        self.peer_list.insert(addr, peer_ref.clone());

        return Client::new(addr, peer_ref);
    }

    pub fn step(&mut self) {
        let mut recv_buf = [0; MAX_TRANSFER_UNIT];

        while let Ok((recv_size, src_addr)) = self.socket.recv_from(&mut recv_buf) {
            if let Some(frame) = frame::Frame::read(&recv_buf[..recv_size]) {
                let mut data_sink = PeerFrameSink::new(&self.socket, src_addr);

                match self.peer_list.get_mut(&src_addr) {
                    Some(peer) => {
                        peer.borrow_mut().handle_frame(frame, &mut data_sink);
                    }
                    None => {
                        if self.peer_list.len() < self.max_connected_peers as usize {
                            let mut peer = peer::Peer::new(self.peer_params.clone());
                            peer.handle_frame(frame, &mut data_sink);

                            let peer_ref = Rc::new(RefCell::new(peer));
                            self.peer_list.insert(src_addr, Rc::clone(&peer_ref));
                            self.new_clients.push(Client::new(src_addr, peer_ref));
                        }
                    }
                }
            }
        }

        self.flush();

        self.peer_list.retain(|_, peer| !peer.borrow().is_zombie());
        self.new_clients.retain(|client| !client.is_zombie());
    }

    pub fn incoming(&mut self) -> impl Iterator<Item = Client> {
        std::mem::take(&mut self.new_clients).into_iter()
    }

    pub fn flush(&mut self) {
        for (address, peer) in self.peer_list.iter_mut() {
            let mut data_sink = PeerFrameSink::new(&self.socket, *address);
            peer.borrow_mut().flush(&mut data_sink);
        }
    }

    pub fn address(&self) -> Option<net::SocketAddr> {
        self.socket.local_addr().ok()
    }
}

