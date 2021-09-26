
mod channel;
mod frame;
mod peer;
pub mod host;

pub type ChannelId = u8;
pub use channel::SendMode;

type FrameId = u32;
type PingId = u16;
type ProtocolVersionId = u8;

pub const PROTOCOL_VERSION: ProtocolVersionId = 0;
pub const MTU: usize = 1500;

