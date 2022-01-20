
use crate::endpoint;
use crate::Event;
use crate::SendMode;

use std::net;
use std::rc::Rc;
use std::cell::RefCell;

pub struct Peer {
    address: net::SocketAddr,
    endpoint_ref: Rc<RefCell<endpoint::Endpoint>>,
}

impl Peer {
    pub(super) fn new(address: net::SocketAddr, endpoint_ref: Rc<RefCell<endpoint::Endpoint>>) -> Self {
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

    pub fn disconnect_now(&self) {
        self.endpoint_ref.borrow_mut().disconnect_now();
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

