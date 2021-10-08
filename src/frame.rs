
use super::ProtocolVersionId;
use super::FrameId;
use super::PingId;

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
    pub sequence_id: u32,
    pub dependent_lead: u16,
    pub payload: Payload,
}

#[derive(Clone,Debug,PartialEq)]
pub struct WindowAck {
    pub sequence_id: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub enum Message {
    Datagram(Datagram),
    WindowAck(WindowAck),
}

#[derive(Clone,Debug,PartialEq)]
pub struct DataEntry {
    pub channel_id: u8,
    pub message: Message,
}

#[derive(Clone,Debug,PartialEq)]
pub struct Connect {
    pub sequence_id: u32,
    pub version: ProtocolVersionId,
    pub num_channels: u8,
    pub rx_bandwidth_max: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub struct ConnectAck {
    pub sequence_id: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub struct Disconnect {
}

#[derive(Clone,Debug,PartialEq)]
pub struct DisconnectAck {
}

#[derive(Clone,Debug,PartialEq)]
pub struct Ping {
    pub sequence_id: PingId,
}

#[derive(Clone,Debug,PartialEq)]
pub struct PingAck {
    pub sequence_id: PingId,
}

#[derive(Clone,Debug,PartialEq)]
pub struct Data {
    pub ack: bool,
    pub sequence_id: FrameId,
    pub entries: Vec<DataEntry>,
}

#[derive(Clone,Debug,PartialEq)]
pub struct DataAck {
    pub sequence_ids: Vec<FrameId>,
}

#[derive(Debug,PartialEq)]
pub enum Frame {
    Connect(Connect),
    ConnectAck(ConnectAck),
    Disconnect(Disconnect),
    DisconnectAck(DisconnectAck),
    Ping(Ping),
    PingAck(PingAck),
    Data(Data),
    DataAck(DataAck),
}

impl Fragment {
    pub fn new(fragment_id: u16, last_fragment_id: u16, data: Box<[u8]>) -> Self {
        Self {
            fragment_id: fragment_id,
            last_fragment_id: last_fragment_id,
            data: data,
        }
    }
}

impl Datagram {
    pub fn new(sequence_id: u32, dependent_lead: u16, payload: Payload) -> Self {
        Self {
            sequence_id: sequence_id,
            dependent_lead: dependent_lead,
            payload: payload,
        }
    }
}

impl WindowAck {
    pub fn new(sequence_id: u32) -> Self {
        Self {
            sequence_id: sequence_id,
        }
    }
}

impl DataEntry {
    pub fn new(channel_id: u8, message: Message) -> Self {
        Self {
            channel_id: channel_id,
            message: message,
        }
    }

    const FRAGMENT_TYPE_ID: u8 = 0;
    const SENTINEL_TYPE_ID: u8 = 1;
    const WINDOW_ACK_TYPE_ID: u8 = 2;

    pub const FRAGMENT_HEADER_SIZE_BYTES: usize = 13;
    pub const SENTINEL_SIZE_BYTES: usize = 7;
    pub const WINDOW_ACK_SIZE_BYTES: usize = 5;

    fn read_fragment(buf: &[u8]) -> Option<(DataEntry, &[u8])> {
        let header_size = 13;

        if buf.len() < header_size {
            return None;
        }

        let channel_id       =   buf[ 1];
        let sequence_id      = ((buf[ 2] as u32) << 16) | 
                               ((buf[ 3] as u32) <<  8) | 
                               ((buf[ 4] as u32)      );
        let dependent_lead   = ((buf[ 5] as u16) <<  8) | 
                               ((buf[ 6] as u16)      );
        let fragment_id      = ((buf[ 7] as u16) <<  8) | 
                               ((buf[ 8] as u16)      );
        let last_fragment_id = ((buf[ 9] as u16) <<  8) | 
                               ((buf[10] as u16)      );
        let data_len         = ((buf[11] as usize) <<  8) | 
                               ((buf[12] as usize)      );

        let total_size = header_size + data_len;

        if buf.len() < total_size {
            return None;
        }

        let data = buf[header_size .. total_size].into();

        Some((
            DataEntry {
                channel_id: channel_id,
                message: Message::Datagram(Datagram {
                    sequence_id: sequence_id,
                    dependent_lead: dependent_lead,
                    payload: Payload::Fragment(Fragment {
                        fragment_id: fragment_id,
                        last_fragment_id: last_fragment_id,
                        data: data,
                    }),
                }),
            },
            &buf[total_size ..]
        ))
    }

    fn write_fragment(channel_id: u8,
                      sequence_id: u32, dependent_lead: u16,
                      fragment_id: u16, last_fragment_id: u16,
                      data: &Box<[u8]>, buf: &mut Vec<u8>) {
        assert!(data.len() <= u16::MAX as usize);
        let data_len = data.len() as u16;

        let header = [
            Self::FRAGMENT_TYPE_ID,
            channel_id,
            (sequence_id >> 16) as u8,
            (sequence_id >>  8) as u8,
            (sequence_id      ) as u8,
            (dependent_lead >>  8) as u8,
            (dependent_lead      ) as u8,
            (fragment_id >>  8) as u8,
            (fragment_id      ) as u8,
            (last_fragment_id >>  8) as u8,
            (last_fragment_id      ) as u8,
            (data_len >>  8) as u8,
            (data_len      ) as u8,
        ];

        buf.extend_from_slice(&header);
        buf.extend_from_slice(&data);
    }

    fn read_sentinel(buf: &[u8]) -> Option<(DataEntry, &[u8])> {
        let header_size = 7;

        if buf.len() < header_size {
            return None;
        }

        let channel_id     =   buf[1];
        let sequence_id    = ((buf[2] as u32) << 16) | 
                             ((buf[3] as u32) <<  8) | 
                             ((buf[4] as u32)      );
        let dependent_lead = ((buf[5] as u16) <<  8) | 
                             ((buf[6] as u16)      );

        Some((
            DataEntry {
                channel_id: channel_id,
                message: Message::Datagram(Datagram {
                    sequence_id: sequence_id,
                    dependent_lead: dependent_lead,
                    payload: Payload::Sentinel,
                }),
            },
            &buf[header_size ..]
        ))
    }

    fn write_sentinel(channel_id: u8,
                      sequence_id: u32, dependent_lead: u16,
                      buf: &mut Vec<u8>) {
        let bytes = [
            Self::SENTINEL_TYPE_ID,
            channel_id,
            (sequence_id >> 16) as u8,
            (sequence_id >>  8) as u8,
            (sequence_id      ) as u8,
            (dependent_lead >>  8) as u8,
            (dependent_lead      ) as u8,
        ];

        buf.extend_from_slice(&bytes);
    }

    fn read_window_ack(buf: &[u8]) -> Option<(DataEntry, &[u8])> {
        let header_size = 5;

        if buf.len() < header_size {
            return None;
        }

        let channel_id  =   buf[1];
        let sequence_id = ((buf[2] as u32) << 16) | 
                          ((buf[3] as u32) <<  8) | 
                          ((buf[4] as u32)      );

        Some((
            Self::new(channel_id, Message::WindowAck(WindowAck {
                sequence_id: sequence_id
            })),
            &buf[header_size ..]
        ))
    }

    fn write_window_ack(channel_id: u8,
                        sequence_id: u32,
                        buf: &mut Vec<u8>) {
        let bytes = [
            Self::WINDOW_ACK_TYPE_ID,
            channel_id,
            (sequence_id >> 16) as u8,
            (sequence_id >>  8) as u8,
            (sequence_id      ) as u8,
        ];

        buf.extend_from_slice(&bytes);
    }

    pub fn read(buf: &[u8]) -> Option<(DataEntry, &[u8])> {
        if buf.len() < 1 {
            return None;
        }

        let head = buf[0];

        match head {
            Self::FRAGMENT_TYPE_ID => {
                Self::read_fragment(buf)
            }
            Self::SENTINEL_TYPE_ID => {
                Self::read_sentinel(buf)
            }
            Self::WINDOW_ACK_TYPE_ID => {
                Self::read_window_ack(buf)
            }
            _ => None
        }
    }

    pub fn write(&self, buf: &mut Vec<u8>) {
        match &self.message {
            Message::Datagram(dg) => match &dg.payload {
                Payload::Fragment(fr) => {
                    Self::write_fragment(self.channel_id,
                                         dg.sequence_id, dg.dependent_lead,
                                         fr.fragment_id, fr.last_fragment_id,
                                         &fr.data, buf);
                }
                Payload::Sentinel => {
                    Self::write_sentinel(self.channel_id, dg.sequence_id, dg.dependent_lead, buf);
                }
            }
            Message::WindowAck(dg) => {
                Self::write_window_ack(self.channel_id, dg.sequence_id, buf);
            }
        }
    }

    pub fn encoded_size(&self) -> usize {
        match &self.message {
            Message::Datagram(dg) => match &dg.payload {
                Payload::Fragment(fr) => Self::FRAGMENT_HEADER_SIZE_BYTES + fr.data.len(),
                Payload::Sentinel => Self::SENTINEL_SIZE_BYTES,
            }
            Message::WindowAck(_) => Self::WINDOW_ACK_SIZE_BYTES,
        }
    }
}

impl Connect {
    const TYPE_ID: u8 = 0;
    pub const HEADER_SIZE_BYTES: usize = 11;

    pub fn to_bytes(&self) -> Box<[u8]> {
        let mut bytes = Vec::new();

        let header = [
            Self::TYPE_ID,
            self.version,
            self.num_channels,
            (self.rx_bandwidth_max >> 24) as u8,
            (self.rx_bandwidth_max >> 16) as u8,
            (self.rx_bandwidth_max >>  8) as u8,
            (self.rx_bandwidth_max      ) as u8,
            (self.sequence_id >> 24) as u8,
            (self.sequence_id >> 16) as u8,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
        ];

        bytes.extend_from_slice(&header);

        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::HEADER_SIZE_BYTES {
            return None;
        }

        if bytes[0] != Self::TYPE_ID {
            return None;
        }
        let version          =   bytes[ 1];
        let num_channels     =   bytes[ 2];
        let rx_bandwidth_max = ((bytes[ 3] as u32) << 24) |
                               ((bytes[ 4] as u32) << 16) |
                               ((bytes[ 5] as u32) <<  8) |
                               ((bytes[ 6] as u32)      );
        let sequence_id      = ((bytes[ 7] as u32) << 24) |
                               ((bytes[ 8] as u32) << 16) |
                               ((bytes[ 9] as u32) <<  8) |
                               ((bytes[10] as u32)      );

        Some(Self {
            version: version,
            num_channels: num_channels,
            rx_bandwidth_max: rx_bandwidth_max,
            sequence_id: sequence_id,
        })
    }
}

impl ConnectAck {
    const TYPE_ID: u8 = 1;
    pub const HEADER_SIZE_BYTES: usize = 5;

    pub fn to_bytes(&self) -> Box<[u8]> {
        let mut bytes = Vec::new();

        let header = [
            Self::TYPE_ID,
            (self.sequence_id >> 24) as u8,
            (self.sequence_id >> 16) as u8,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
        ];

        bytes.extend_from_slice(&header);

        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::HEADER_SIZE_BYTES {
            return None;
        }

        if bytes[0] != Self::TYPE_ID {
            return None;
        }
        let sequence_id  = ((bytes[1] as u32) << 24) |
                           ((bytes[2] as u32) << 16) |
                           ((bytes[3] as u32) <<  8) |
                           ((bytes[4] as u32)      );

        Some(Self {
            sequence_id: sequence_id,
        })
    }
}

impl Disconnect {
    const TYPE_ID: u8 = 2;
    const HEADER_SIZE_BYTES: usize = 1;

    pub fn to_bytes(&self) -> Box<[u8]> {
        let mut bytes = Vec::new();
        bytes.push(Self::TYPE_ID);
        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::HEADER_SIZE_BYTES {
            return None;
        }

        if bytes[0] != Self::TYPE_ID {
            return None;
        }

        Some(Self {
        })
    }
}

impl DisconnectAck {
    const TYPE_ID: u8 = 3;
    const HEADER_SIZE_BYTES: usize = 1;

    pub fn to_bytes(&self) -> Box<[u8]> {
        let mut bytes = Vec::new();
        bytes.push(Self::TYPE_ID);
        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::HEADER_SIZE_BYTES {
            return None;
        }

        if bytes[0] != Self::TYPE_ID {
            return None;
        }

        Some(Self {
        })
    }
}

impl Ping {
    const TYPE_ID: u8 = 4;
    const HEADER_SIZE_BYTES: usize = 3;

    pub fn to_bytes(&self) -> Box<[u8]> {
        let mut bytes = Vec::new();
        let header = [
            Self::TYPE_ID,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
        ];
        bytes.extend_from_slice(&header);
        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::HEADER_SIZE_BYTES {
            return None;
        }

        if bytes[0] != Self::TYPE_ID {
            return None;
        }
        let sequence_id = ((bytes[1] as u16) <<  8) |
                          ((bytes[2] as u16)      );

        Some(Self {
            sequence_id: sequence_id,
        })
    }
}

impl PingAck {
    const TYPE_ID: u8 = 5;
    const HEADER_SIZE_BYTES: usize = 3;

    pub fn to_bytes(&self) -> Box<[u8]> {
        let mut bytes = Vec::new();
        let header = [
            Self::TYPE_ID,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
        ];
        bytes.extend_from_slice(&header);
        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::HEADER_SIZE_BYTES {
            return None;
        }

        if bytes[0] != Self::TYPE_ID {
            return None;
        }
        let sequence_id = ((bytes[1] as u16) <<  8) |
                          ((bytes[2] as u16)      );

        Some(Self {
            sequence_id: sequence_id,
        })
    }
}

impl Data {
    const TYPE_ID: u8 = 6;
    pub const HEADER_SIZE_BYTES: usize = 8;

    pub fn new(ack: bool, sequence_id: u32, entries: Vec<DataEntry>) -> Self {
        Self {
            ack: ack,
            sequence_id: sequence_id,
            entries: entries,
        }
    }

    pub fn to_bytes(&self) -> Box<[u8]> {
        let mut bytes = Vec::new();

        assert!(self.entries.len() <= u16::MAX as usize);
        let entry_num = self.entries.len() as u16;

        let header = [
            Self::TYPE_ID,
            self.ack as u8,
            (self.sequence_id >> 24) as u8,
            (self.sequence_id >> 16) as u8,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
            (entry_num >>  8) as u8,
            (entry_num      ) as u8,
        ];

        bytes.extend_from_slice(&header);

        for entry in self.entries.iter() {
            entry.write(&mut bytes);
        }

        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::HEADER_SIZE_BYTES {
            return None;
        }

        let header = &bytes[..Self::HEADER_SIZE_BYTES];

        if header[0] != Self::TYPE_ID {
            return None;
        }

        let ack         = match header[1] { 0 => false, 1 => true, _ => { return None } };
        let sequence_id = ((header[2] as u32) << 24) |
                          ((header[3] as u32) << 16) |
                          ((header[4] as u32) <<  8) |
                          ((header[5] as u32)      );
        let entry_num   = ((header[6] as u16) <<  8) |
                          ((header[7] as u16)      );

        let mut entries = Vec::new();

        let mut buf = &bytes[Self::HEADER_SIZE_BYTES ..];

        for _ in 0..entry_num {
            if let Some((entry, new_buf)) = DataEntry::read(buf) {
                entries.push(entry);
                buf = new_buf;
            } else {
                return None
            }
        }

        if buf.len() == 0 {
            Some(Self {
                ack: ack,
                sequence_id: sequence_id,
                entries: entries,
            })
        } else {
            None
        }
    }
}

impl DataAck {
    const TYPE_ID: u8 = 7;
    pub const HEADER_SIZE_BYTES: usize = 3;
    pub const SEQUENCE_ID_SIZE_BYTES: usize = 4;

    pub fn to_bytes(&self) -> Box<[u8]> {
        assert!(self.sequence_ids.len() <= u16::MAX as usize);
        let sequence_id_num = self.sequence_ids.len() as u16;

        let mut bytes = Vec::new();

        let header = [
            Self::TYPE_ID,
            (sequence_id_num >>  8) as u8,
            (sequence_id_num      ) as u8,
        ];

        bytes.extend_from_slice(&header);

        for sequence_id in self.sequence_ids.iter() {
            let value = [
                (sequence_id >> 24) as u8,
                (sequence_id >> 16) as u8,
                (sequence_id >>  8) as u8,
                (sequence_id >>  0) as u8,
            ];

            bytes.extend_from_slice(&value);
        }

        bytes.into_boxed_slice()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::HEADER_SIZE_BYTES {
            return None;
        }

        let header = &bytes[.. Self::HEADER_SIZE_BYTES];

        if header[0] != Self::TYPE_ID {
            return None;
        }
        let sequence_id_num = ((header[1] as usize) <<  8) |
                              ((header[2] as usize)      );

        let mut read_idx = Self::HEADER_SIZE_BYTES;

        if read_idx + sequence_id_num * Self::SEQUENCE_ID_SIZE_BYTES != bytes.len() {
            return None;
        }

        let mut sequence_ids = Vec::new();

        for _ in 0..sequence_id_num {
            let sequence_id = ((bytes[read_idx + 0] as u32) << 24) |
                              ((bytes[read_idx + 1] as u32) << 16) |
                              ((bytes[read_idx + 2] as u32) <<  8) |
                              ((bytes[read_idx + 3] as u32)      );

            read_idx += Self::SEQUENCE_ID_SIZE_BYTES;

            sequence_ids.push(sequence_id);
        }

        Some(Self {
            sequence_ids: sequence_ids,
        })
    }
}

impl Frame {
    pub fn to_bytes(&self) -> Box<[u8]> {
        match self {
            Frame::Connect(data) => data.to_bytes(),
            Frame::ConnectAck(data) => data.to_bytes(),
            Frame::Disconnect(data) => data.to_bytes(),
            Frame::DisconnectAck(data) => data.to_bytes(),
            Frame::Ping(data) => data.to_bytes(),
            Frame::PingAck(data) => data.to_bytes(),
            Frame::Data(data) => data.to_bytes(),
            Frame::DataAck(data) => data.to_bytes(),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() == 0 {
            return None;
        }
        match bytes[0] {
            Connect::TYPE_ID => Connect::from_bytes(bytes).map(|data| Frame::Connect(data)),
            ConnectAck::TYPE_ID => ConnectAck::from_bytes(bytes).map(|data| Frame::ConnectAck(data)),
            Disconnect::TYPE_ID => Disconnect::from_bytes(bytes).map(|data| Frame::Disconnect(data)),
            DisconnectAck::TYPE_ID => DisconnectAck::from_bytes(bytes).map(|data| Frame::DisconnectAck(data)),
            Ping::TYPE_ID => Ping::from_bytes(bytes).map(|data| Frame::Ping(data)),
            PingAck::TYPE_ID => PingAck::from_bytes(bytes).map(|data| Frame::PingAck(data)),
            Data::TYPE_ID => Data::from_bytes(bytes).map(|data| Frame::Data(data)),
            DataAck::TYPE_ID => DataAck::from_bytes(bytes).map(|data| Frame::DataAck(data)),
            _ => None,
        }
    }
}

#[cfg(test)]
fn verify_consistent(f: &Frame) {
    //println!("frame: {:#?}", f);

    let bytes = f.to_bytes();
    //println!("frame bytes: {:?}", bytes);

    let f2 = Frame::from_bytes(&bytes).unwrap();

    assert_eq!(*f, f2);
}

#[cfg(test)]
fn verify_extra_bytes_fail(f: &Frame) {
    //println!("frame: {:#?}", f);

    let bytes = f.to_bytes();
    //println!("frame bytes: {:?}", bytes);

    let mut bad_bytes_vec = bytes.to_vec();
    bad_bytes_vec.push(0x00);
    let bad_bytes = bad_bytes_vec.into_boxed_slice();

    assert_eq!(Frame::from_bytes(&bad_bytes), None);
}

#[cfg(test)]
fn verify_truncation_fails(f: &Frame) {
    let bytes = f.to_bytes();

    for i in 1..bytes.len() {
        let bytes_trunc = &bytes[0..i];
        assert_eq!(Frame::from_bytes(&bytes_trunc), None);
    }
}

#[test]
fn test_connect_basic() {
    let f = Frame::Connect(Connect {
        version: 0x7F,
        num_channels: 3,
        rx_bandwidth_max: 0xBEEFBEEF,
        sequence_id: 0x13371337,
    });
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_connect_ack_basic() {
    let f = Frame::ConnectAck(ConnectAck {
        sequence_id: 0x13371337,
    });
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_disconnect_basic() {
    let f = Frame::Disconnect(Disconnect {});
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_disconnect_ack_basic() {
    let f = Frame::DisconnectAck(DisconnectAck {});
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_ping_basic() {
    let f = Frame::Ping(Ping {
        sequence_id: 0xBEEF,
    });
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_ping_ack_basic() {
    let f = Frame::PingAck(PingAck {
        sequence_id: 0xBEEF,
    });
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_data_basic() {
    let f = Frame::Data(Data {
        ack: true,
        sequence_id: 0x010203,
        entries: vec![
            DataEntry::new(69, Message::Datagram(
                    Datagram::new(0xBEEF0, 2, Payload::Fragment(Fragment::new(10, 10, vec![  4,  5,  6,  7 ].into_boxed_slice()))))),
            DataEntry::new(69, Message::Datagram(
                    Datagram::new(0xBEEF1, 2, Payload::Sentinel))),
            DataEntry::new(69, Message::WindowAck(
                    WindowAck::new(0xBEEF2))),
        ],
    });
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_data_ack_basic() {
    let f = Frame::DataAck(DataAck {
        sequence_ids: vec![
            0x00000000, 0x00112233, 0x54545454, 0x77777777
        ],
    });
    verify_consistent(&f);
    verify_extra_bytes_fail(&f);
    verify_truncation_fails(&f);
}

#[test]
fn test_connect_random() {
    const NUM_ROUNDS: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let f = Frame::Connect(Connect {
            version: rand::random::<u8>(),
            num_channels: rand::random::<u8>(),
            rx_bandwidth_max: rand::random::<u32>(),
            sequence_id: rand::random::<u32>(),
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }
}

#[test]
fn test_connect_ack_random() {
    const NUM_ROUNDS: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let f = Frame::ConnectAck(ConnectAck {
            sequence_id: rand::random::<u32>(),
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }
}

#[test]
fn test_ping_random() {
    const NUM_ROUNDS: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let f = Frame::Ping(Ping {
            sequence_id: rand::random::<u16>(),
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }
}

#[test]
fn test_ping_ack_random() {
    const NUM_ROUNDS: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let f = Frame::PingAck(PingAck {
            sequence_id: rand::random::<u16>(),
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }
}

#[cfg(test)]
fn random_data(size: usize) -> Box<[u8]> {
    (0..size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice()
}

#[test]
fn test_data_random() {
    const NUM_ROUNDS: usize = 100;
    const MAX_DATAGRAMS: usize = 100;
    const MAX_DATA_SIZE: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let mut entries = Vec::new();

        for _ in 0..rand::random::<usize>() % MAX_DATAGRAMS {
            match rand::random::<u32>() % 3 {
                0 => {
                    let entry = DataEntry::new(rand::random::<u8>(), Message::Datagram(
                            Datagram::new(rand::random::<u32>() & 0xFFFFFF, rand::random::<u16>(), Payload::Fragment(
                                    Fragment::new(rand::random::<u16>(), rand::random::<u16>(), random_data(MAX_DATA_SIZE))))));
                    entries.push(entry);
                }
                1 => {
                    let entry = DataEntry::new(rand::random::<u8>(), Message::Datagram(
                            Datagram::new(rand::random::<u32>() & 0xFFFFFF, rand::random::<u16>(), Payload::Sentinel)));
                    entries.push(entry);
                }
                2 => {
                    let entry = DataEntry::new(rand::random::<u8>(), Message::WindowAck(
                            WindowAck::new(rand::random::<u32>() & 0xFFFFFF)));
                    entries.push(entry);
                }
                _ => panic!("NANI?!")
            }
        }

        let f = Frame::Data(Data {
            ack: rand::random::<bool>(),
            sequence_id: rand::random::<u32>(),
            entries: entries,
        });

        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }
}

#[cfg(test)]
extern crate rand;

#[test]
fn test_data_ack_random() {
    const NUM_ROUNDS: usize = 100;
    const MAX_SEQUENCE_IDS: usize = 1000;

    for _ in 0..NUM_ROUNDS {
        let sequence_ids = (0 .. rand::random::<usize>() % MAX_SEQUENCE_IDS).map(|_| rand::random::<u32>()).collect();
        let f = Frame::DataAck(DataAck {
            sequence_ids: sequence_ids,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }
}

