
mod tx;
mod rx;
mod seq;

// Must be a power of two
const TRANSFER_WINDOW_SIZE: u32 = 16384;
// Must be an integer divisor of TRANSFER_WINDOW_SIZE
const WINDOW_ACK_SPACING: u32 = TRANSFER_WINDOW_SIZE / 32;

// TODO: Make channel constructor parameter
const FRAGMENT_SIZE: usize = 1472-15;

type Fragment = super::frame::Fragment;
type Payload = super::frame::Payload;
type Datagram = super::frame::Datagram;
type WindowAck = super::frame::WindowAck;

type SendMode = super::SendMode;

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

