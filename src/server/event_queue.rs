use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::rc::Rc;

use super::remote_client::RemoteClient;

#[derive(Copy, Clone, PartialEq)]
pub enum EventType {
    ResendHandshakeSynAck,
    ResendDisconnect,
    ClosedTimeout,
}

pub struct Event {
    pub client: Rc<RefCell<RemoteClient>>,
    pub kind: EventType,
    pub time: u64,
    pub count: u8,
}

impl Event {
    pub fn new(client: Rc<RefCell<RemoteClient>>, kind: EventType, time: u64, count: u8) -> Self {
        Self {
            client,
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
