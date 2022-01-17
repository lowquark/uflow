
use crate::frame::Datagram;

use std::collections::VecDeque;

#[derive(Debug)]
pub struct Entry {
    pub datagram: Datagram,
    pub resend: bool,
}

impl Entry {
    pub fn new(datagram: Datagram, resend: bool) -> Self {
        Self {
            datagram,
            resend,
        }
    }
}

pub type DatagramQueue = VecDeque<Entry>;

