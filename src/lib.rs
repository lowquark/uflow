
pub mod host;
pub mod frame;
mod endpoint;

pub use host::Host;
pub use host::Peer;
pub use endpoint::Params as EndpointParams;
pub use host::Event;

pub const PROTOCOL_VERSION: u8 = 0;

pub const MAX_CHANNELS: usize = frame::serial::MAX_CHANNELS;

pub const MAX_ETHERNET_FRAME_SIZE: usize = 1500;
pub const UDP_HEADER_SIZE: usize = 28;
pub const MAX_TRANSFER_UNIT: usize = MAX_ETHERNET_FRAME_SIZE - UDP_HEADER_SIZE;
pub const MAX_FRAGMENT_SIZE: usize = MAX_TRANSFER_UNIT - frame::serial::MAX_DATAGRAM_OVERHEAD - frame::serial::DATA_FRAME_OVERHEAD;
pub const MAX_PACKET_SIZE: usize = MAX_FRAGMENT_SIZE * frame::serial::MAX_FRAGMENTS;

const MAX_PACKET_TRANSFER_WINDOW_SIZE: u32 = 4096;
const MAX_FRAME_TRANSFER_WINDOW_SIZE: u32 = 16384;

pub type ChannelId = u8;

#[derive(Clone,Copy,Debug,PartialEq)]
pub enum SendMode {
    TimeSensitive,
    Unreliable,
    Resend,
    Reliable,
}

pub trait FrameSink {
    fn send(&mut self, frame_data: &[u8]);
}

