
pub mod serial;

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

#[derive(Debug,PartialEq)]
pub struct DatagramRef<'a> {
    pub sequence_id: u32,
    pub channel_id: u8,
    pub window_parent_lead: u16,
    pub channel_parent_lead: u16,
    pub fragment_id: FragmentId,
    pub data: &'a [u8],
}

impl<'a> From<&'a Datagram> for DatagramRef<'a> {
    fn from(obj: &'a Datagram) -> Self {
        Self {
            sequence_id: obj.sequence_id,
            channel_id: obj.channel_id,
            window_parent_lead: obj.window_parent_lead,
            channel_parent_lead: obj.channel_parent_lead,
            fragment_id: obj.fragment_id.clone(),
            data: &obj.data,
        }
    }
}

impl<'a> From<DatagramRef<'a>> for Datagram {
    fn from(obj: DatagramRef<'a>) -> Self {
        Self {
            sequence_id: obj.sequence_id,
            channel_id: obj.channel_id,
            window_parent_lead: obj.window_parent_lead,
            channel_parent_lead: obj.channel_parent_lead,
            fragment_id: obj.fragment_id,
            data: obj.data.into(),
        }
    }
}

impl Datagram {
    // TODO: Does validity concern this layer? (No.)
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
pub struct FrameAck {
    pub base_id: u32,
    pub bitfield: u32,
    pub nonce: bool,
}

#[derive(Clone,Debug,PartialEq)]
pub struct ConnectFrame {
    pub version: u8,
    pub nonce: u32,
    pub channel_count_sup: u8,
    pub max_receive_rate: u32,
    pub max_packet_size: u32,
    pub max_receive_alloc: u32,
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
pub struct DataFrame {
    pub sequence_id: u32,
    pub nonce: bool,
    pub datagrams: Vec<Datagram>,
}

#[derive(Clone,Debug,PartialEq)]
pub struct SyncFrame {
    pub sequence_id: u32,
    pub nonce: bool,
    pub sender_next_id: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub struct AckFrame {
    pub frame_window_base_id: u32,
    pub packet_window_base_id: u32,
    pub frame_acks: Vec<FrameAck>,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Frame {
    ConnectFrame(ConnectFrame),
    ConnectAckFrame(ConnectAckFrame),
    DisconnectFrame(DisconnectFrame),
    DisconnectAckFrame(DisconnectAckFrame),
    DataFrame(DataFrame),
    SyncFrame(SyncFrame),
    AckFrame(AckFrame),
}

