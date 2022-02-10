
use super::pending_packet::FragmentRef;

use std::collections::VecDeque;

#[derive(Debug)]
pub struct Entry {
    pub fragment_ref: FragmentRef,
    pub resend: bool,
}

impl Entry {
    pub fn new(fragment_ref: FragmentRef, resend: bool) -> Self {
        Self {
            fragment_ref,
            resend,
        }
    }
}

pub type DatagramQueue = VecDeque<Entry>;

