use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::net::SocketAddr;
use std::rc::Rc;

use super::peer::Peer;

#[derive(Copy, Clone, PartialEq)]
pub enum EventKind {
    ResendHandshakeSynAck,
    ResendDisconnect,
    ClosedTimeout,
}

pub struct Event {
    pub peer: Rc<RefCell<Peer>>,
    pub kind: EventKind,
    pub time: u64,
    pub count: u8,
}

impl Event {
    pub fn new(peer: Rc<RefCell<Peer>>, kind: EventKind, time: u64, count: u8) -> Self {
        Self {
            peer,
            kind,
            time,
            count,
        }
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.time.cmp(&other.time).reverse())
    }
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl Eq for Event {}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        self.time.cmp(&other.time).reverse()
    }
}

pub type EventQueue = BinaryHeap<Event>;
