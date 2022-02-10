
use super::FragmentRef;

use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Debug)]
pub struct Entry {
    pub fragment_ref: FragmentRef,
    pub resend_time: u64,
    pub send_count: u8,
}

impl Entry {
    pub fn new(fragment_ref: FragmentRef, resend_time: u64, send_count: u8) -> Self {
        Self { fragment_ref, resend_time, send_count }
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.resend_time.cmp(&other.resend_time).reverse())
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.resend_time == other.resend_time
    }
}

impl Eq for Entry {}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.resend_time.cmp(&other.resend_time).reverse()
    }
}

pub type ResendQueue = BinaryHeap<Entry>;

