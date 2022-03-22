
pub mod serial;

#[derive(Clone,Debug,PartialEq)]
pub struct Datagram {
    pub sequence_id: u32,
    pub channel_id: u8,
    pub window_parent_lead: u16,
    pub channel_parent_lead: u16,
    pub fragment_id: u16,
    pub fragment_id_last: u16,
    pub data: Box<[u8]>,
}

#[derive(Debug,PartialEq)]
pub struct DatagramRef<'a> {
    pub sequence_id: u32,
    pub channel_id: u8,
    pub window_parent_lead: u16,
    pub channel_parent_lead: u16,
    pub fragment_id: u16,
    pub fragment_id_last: u16,
    pub data: &'a [u8],
}

impl<'a> From<&'a Datagram> for DatagramRef<'a> {
    fn from(obj: &'a Datagram) -> Self {
        Self {
            sequence_id: obj.sequence_id,
            channel_id: obj.channel_id,
            window_parent_lead: obj.window_parent_lead,
            channel_parent_lead: obj.channel_parent_lead,
            fragment_id: obj.fragment_id,
            fragment_id_last: obj.fragment_id_last,
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
            fragment_id_last: obj.fragment_id_last,
            data: obj.data.into(),
        }
    }
}

#[derive(Clone,Debug,PartialEq)]
pub struct AckGroup {
    pub base_id: u32,
    pub bitfield: u32,
    pub nonce: bool,
}

#[derive(Clone,Debug,PartialEq)]
pub struct ConnectFrame {
    pub version: u8,
    pub nonce: u32,

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
    pub next_frame_id: Option<u32>,
    pub next_packet_id: Option<u32>,
}

#[derive(Clone,Debug,PartialEq)]
pub struct AckFrame {
    pub frame_window_base_id: u32,
    pub packet_window_base_id: u32,
    pub frame_acks: Vec<AckGroup>,
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

