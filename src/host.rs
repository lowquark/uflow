
use super::peer;
use super::frame;
use super::MTU;
use super::ChannelId;
use super::SendMode;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net;
use std::ops::Range;
use std::rc::Rc;

use super::DataSink;

struct PeerDataSink<'a> {
    socket: &'a net::UdpSocket,
    address: net::SocketAddr,
}

impl<'a> PeerDataSink<'a> {
    fn new(socket: &'a net::UdpSocket, address: net::SocketAddr) -> Self {
        Self {
            socket: socket,
            address: address,
        }
    }
}

impl<'a> DataSink for PeerDataSink<'a> {
    fn send(&self, data: &[u8]) {
        let _ = self.socket.send_to(data, self.address);
    }
}


#[derive(Clone)]
pub struct Params {
    num_channels: u32,
    priority_channels: Range<u32>,
    min_peer_tx_bandwidth: u32,
    max_peer_tx_bandwidth: u32,
    max_peer_rx_bandwidth: u32,
    max_connected_peers: u32,
    // TODO: Blacklist, whitelist
}

impl Params {
    pub fn new() -> Self {
        Self {
            num_channels: 1,
            priority_channels: 0..0,
            min_peer_tx_bandwidth: 10_000,
            max_peer_tx_bandwidth: 1_000_000,
            max_peer_rx_bandwidth: 1_000_000,
            max_connected_peers: 10,
        }
    }

    pub fn num_channels(mut self, num_channels: u32) -> Params {
        self.num_channels = num_channels;
        self
    }

    pub fn priority_channels(mut self, priority_channels: Range<u32>) -> Params {
        self.priority_channels = priority_channels;
        self
    }

    pub fn min_peer_tx_bandwidth(mut self, bandwidth: u32) -> Params {
        self.min_peer_tx_bandwidth = bandwidth;
        self
    }

    pub fn max_peer_tx_bandwidth(mut self, bandwidth: u32) -> Params {
        self.max_peer_tx_bandwidth = bandwidth;
        self
    }

    pub fn max_peer_rx_bandwidth(mut self, bandwidth: u32) -> Params {
        self.max_peer_rx_bandwidth = bandwidth;
        self
    }

    pub fn max_connected_peers(mut self, num_peers: u32) -> Params {
        self.max_connected_peers = num_peers;
        self
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
    max_connected_peers: u32,
}

type Event = peer::Event;

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

        Ok(Host {
            socket: socket,
            peer_list: HashMap::new(),
            peer_params: peer::Params {
                num_channels: params.num_channels,
                min_tx_bandwidth: params.min_peer_tx_bandwidth,
                max_tx_bandwidth: params.max_peer_tx_bandwidth,
                max_rx_bandwidth: params.max_peer_rx_bandwidth,
                priority_channels: params.priority_channels,
            },
            new_clients: Vec::new(),
            max_connected_peers: params.max_connected_peers,
        })
    }

    // TODO: Will this work with IPv6?
    pub fn bind_any(params: Params) -> Result<Host, std::io::Error> {
        Host::bind((net::Ipv4Addr::UNSPECIFIED, 0), params)
    }

    pub fn connect(&mut self, addr: net::SocketAddr) -> Client {
        let peer = peer::Peer::new_active(self.peer_params.clone());

        let peer_ref = Rc::new(RefCell::new(peer));
        self.peer_list.insert(addr, peer_ref.clone());
        Client::new(addr, peer_ref)
    }

    pub fn step(&mut self) {
        let mut recv_buf = [0; MTU];

        while let Ok((recv_size, src_addr)) = self.socket.recv_from(&mut recv_buf) {
            if let Some(frame) = frame::Frame::from_bytes(&recv_buf[..recv_size]) {
                match self.peer_list.get_mut(&src_addr) {
                    Some(peer) => {
                        peer.borrow_mut().handle_frame(frame);
                    }
                    None => {
                        if self.peer_list.len() < self.max_connected_peers as usize {
                            let mut peer = peer::Peer::new_passive(self.peer_params.clone());
                            peer.handle_frame(frame);

                            let peer_ref = Rc::new(RefCell::new(peer));
                            self.peer_list.insert(src_addr, peer_ref.clone());
                            self.new_clients.push(Client::new(src_addr, peer_ref));
                        }
                    }
                }
            }
        }

        for (_, peer) in self.peer_list.iter_mut() {
            peer.borrow_mut().step();
        }

        self.peer_list.retain(|_, peer| !peer.borrow().is_zombie());
        self.new_clients.retain(|client| !client.is_zombie());

        // TODO: Should this only flush meta?
        // Is calling flush here less efficient than a single call?
        self.flush();
    }

    pub fn incoming(&mut self) -> impl Iterator<Item = Client> {
        std::mem::take(&mut self.new_clients).into_iter()
    }

    pub fn flush(&mut self) {
        for (address, peer) in self.peer_list.iter_mut() {
            peer.borrow_mut().flush(&PeerDataSink::new(&self.socket, *address));
        }
    }

    pub fn address(&self) -> Option<net::SocketAddr> {
        self.socket.local_addr().ok()
    }
}

