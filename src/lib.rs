
mod channel;
mod frame;
pub mod host;
mod peer;
mod transfer;

pub type ChannelId = u8;

#[derive(Clone,Copy,Debug,PartialEq)]
pub enum SendMode {
    Unreliable,
    Reliable,
    Passive,
}

type FrameId = u32;
type PingId = u16;
type ProtocolVersionId = u8;

pub const PROTOCOL_VERSION: ProtocolVersionId = 0;
pub const ETHERNET_MTU: usize = 1500;
pub const UDP_HEADER_SIZE: usize = 28;
pub const MTU: usize = ETHERNET_MTU - UDP_HEADER_SIZE;

pub const MAX_CHANNELS: u32 = 256;

pub trait DataSink {
    fn send(&self, data: &[u8]);
}

