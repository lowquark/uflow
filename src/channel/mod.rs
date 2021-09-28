
mod tx;
mod rx;
mod seq;

// Must be a power of two
const TRANSFER_WINDOW_SIZE: u32 = 16384;
// Must be an integer divisor of TRANSFER_WINDOW_SIZE
const WINDOW_ACK_SPACING: u32 = TRANSFER_WINDOW_SIZE / 32;

// TODO: Make channel constructor parameter
const FRAGMENT_SIZE: usize = 1472-15;

#[derive(Clone,Debug,PartialEq)]
pub struct Fragment {
    pub fragment_id: u16,
    pub last_fragment_id: u16,
    pub data: Box<[u8]>,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Payload {
    Fragment(Fragment),
    Sentinel,
}

#[derive(Clone,Debug,PartialEq)]
pub struct Datagram {
    pub sequence_id: seq::Id,
    pub dependent_lead: u16,
    pub payload: Payload,
}

#[derive(Clone,Debug,PartialEq)]
pub struct WindowAck {
    pub sequence_id: seq::Id,
}

#[derive(Clone,Copy,Debug,PartialEq)]
pub enum SendMode {
    Unreliable,
    Reliable,
    Passive,
}

pub struct Channel {
    pub tx: tx::Tx,
    pub rx: rx::Rx,
}

impl Channel {
    pub fn new() -> Self {
        Self {
            tx: tx::Tx::new(),
            rx: rx::Rx::new(),
        }
    }
}

