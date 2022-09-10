use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::net;
use std::rc::{Rc, Weak};

pub struct PendingPeer {
    pub local_nonce: u32,
    pub remote_nonce: u32,
    pub remote_max_receive_rate: u32,
    pub remote_max_packet_size: u32,
    pub remote_max_receive_alloc: u32,
    pub reply_bytes: Box<[u8]>,
}

pub struct ResendEntry {
    pub peer: Weak<PendingPeer>,
    pub addr: net::SocketAddr,
    pub resend_time: u64,
    pub resend_count: u8,
}

impl ResendEntry {
    pub fn new(peer: Weak<PendingPeer>, addr: net::SocketAddr, resend_time: u64, resend_count: u8) -> Self {
        Self {
            peer,
            addr,
            resend_time,
            resend_count,
        }
    }
}

impl PartialOrd for ResendEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.resend_time.cmp(&other.resend_time).reverse())
    }
}

impl PartialEq for ResendEntry {
    fn eq(&self, other: &Self) -> bool {
        self.resend_time == other.resend_time
    }
}

impl Eq for ResendEntry {}

impl Ord for ResendEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.resend_time.cmp(&other.resend_time).reverse()
    }
}

pub type PendingSet = HashMap<net::SocketAddr, Rc<PendingPeer>>;
pub type ResendQueue = BinaryHeap<ResendEntry>;
