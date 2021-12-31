
mod serial;

pub use serial::Serialize;
pub use serial::build::MessageFrameBuilder;

#[derive(Clone,Debug,PartialEq)]
pub struct FragmentId {
    pub id: u16,
    pub last: u16,
}

#[derive(Clone,Debug,PartialEq)]
pub struct Datagram {
    pub sequence_id: u32,
    pub channel_id: u8,
    pub window_parent_lead: u16,
    pub channel_parent_lead: u16,
    pub fragment_id: FragmentId,
    pub data: Box<[u8]>,
}

impl Datagram {
    // TODO: Does validity concern this layer?
    pub fn is_valid(&self) -> bool {
        use super::MAX_FRAGMENT_SIZE;

        if self.channel_parent_lead != 0 {
            if self.window_parent_lead == 0 || self.channel_parent_lead < self.window_parent_lead {
                return false;
            }
        }
        if self.fragment_id.id > self.fragment_id.last {
            return false;
        }
        if self.fragment_id.id < self.fragment_id.last && self.data.len() != MAX_FRAGMENT_SIZE {
            return false;
        }
        if self.data.len() > MAX_FRAGMENT_SIZE {
            return false;
        }

        return true;
    }
}

#[derive(Clone,Debug,PartialEq)]
pub struct AckGroup {
    pub frame_bits_base_id: u32,
    pub frame_bits_size: u8,
    pub frame_bits: u32,
    pub frame_bits_nonce: bool,
}

#[derive(Clone,Debug,PartialEq)]
pub struct Ack {
    pub frame_bits_base_id: u32,
    pub frame_bits: u32,
    pub frame_bits_nonce: bool,
    pub receiver_base_id: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub struct Resync {
    pub sender_next_id: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Message {
    Datagram(Datagram),
    Ack(Ack),
    Resync(Resync),
}


#[derive(Clone,Debug,PartialEq)]
pub struct ConnectFrame {
    pub nonce: u32,
    pub version: u8,
    // TODO: Rename to tx_channels
    pub num_channels: u8,
    pub max_rx_bandwidth: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub struct ConnectAckFrame {
    pub nonce: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub struct DisconnectFrame {
}

#[derive(Clone,Debug,PartialEq)]
pub struct DisconnectAckFrame {
}

#[derive(Clone,Debug,PartialEq)]
pub struct MessageFrame {
    pub sequence_id: u32,
    pub nonce: bool,
    pub messages: Vec<Message>,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Frame {
    ConnectFrame(ConnectFrame),
    ConnectAckFrame(ConnectAckFrame),
    DisconnectFrame(DisconnectFrame),
    DisconnectAckFrame(DisconnectAckFrame),
    MessageFrame(MessageFrame),
}

