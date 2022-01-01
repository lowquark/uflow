
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
use std::ops::Range;
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

#[derive(Clone)]
pub struct Params {
    tx_channels: usize,
    priority_channels: Range<usize>,
    max_peer_rx_alloc: usize,
    max_peer_tx_bandwidth: usize,
    max_peer_rx_bandwidth: usize,
    max_connected_peers: usize,

    // TODO: Blacklist, whitelist
}

impl Params {
    pub fn new() -> Self {
        Self {
            tx_channels: 1,
            priority_channels: 0..0,
            max_peer_rx_alloc: 1_000_000,
            max_peer_tx_bandwidth: 10_000_000,
            max_peer_rx_bandwidth: 10_000_000,
            max_connected_peers: 16,
        }
    }

    pub fn tx_channels(mut self, tx_channels: usize) -> Params {
        self.tx_channels = tx_channels;
        self
    }

    pub fn priority_channels(mut self, priority_channels: Range<usize>) -> Params {
        self.priority_channels = priority_channels;
        self
    }

    pub fn max_peer_rx_alloc(mut self, max_peer_rx_alloc: usize) -> Params {
        self.max_peer_rx_alloc = max_peer_rx_alloc;
        self
    }

    pub fn max_peer_tx_bandwidth(mut self, bandwidth: usize) -> Params {
        self.max_peer_tx_bandwidth = bandwidth;
        self
    }

    pub fn max_peer_rx_bandwidth(mut self, bandwidth: usize) -> Params {
        self.max_peer_rx_bandwidth = bandwidth;
        self
    }

    pub fn max_connected_peers(mut self, num_peers: usize) -> Params {
        self.max_connected_peers = num_peers;
        self
    }
}

impl From<Params> for peer::Params {
    fn from(params: Params) -> Self {
        Self {
            tx_channels: params.tx_channels,
            priority_channels: params.priority_channels,
            max_rx_alloc: params.max_peer_rx_alloc,
            max_tx_bandwidth: params.max_peer_tx_bandwidth,
            max_rx_bandwidth: params.max_peer_rx_bandwidth,
        }
    }
}

pub struct Client {
    address: net::SocketAddr,
    peer_ref: Rc<RefCell<peer::Peer>>,
}

pub struct Host {
    socket: net::UdpSocket,
    peer_list: HashMap<net::SocketAddr, Rc<RefCell<peer::Peer>>>,
    peer_params: peer::Params,
    new_clients: Vec<Client>,
    max_connected_peers: usize,
}

pub type Event = peer::Event;

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

impl Host {
    pub fn bind<A: net::ToSocketAddrs>(addr: A, params: Params) -> Result<Host, std::io::Error> {
        let socket = net::UdpSocket::bind(addr)?;

        socket.set_nonblocking(true)?;

        let max_connected_peers = params.max_connected_peers;

        Ok(Host {
            socket,
            peer_list: HashMap::new(),
            peer_params: params.into(),
            new_clients: Vec::new(),
            max_connected_peers,
        })
    }

    pub fn bind_any(params: Params) -> Result<Host, std::io::Error> {
        Host::bind((net::Ipv4Addr::UNSPECIFIED, 0), params)
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
                            self.peer_list.insert(src_addr, peer_ref.clone());
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

