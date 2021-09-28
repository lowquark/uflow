
use super::ProtocolVersionId;
use super::FrameId;
use super::PingId;

#[derive(Clone,Debug,PartialEq)]
pub struct Packet {
    pub channel_id: u8,
    pub sequence_id: u32,
    pub dependent_lead: u16,
    pub data: Box<[u8]>,
}

#[derive(Clone,Debug,PartialEq)]
pub struct PacketFragment {
    pub channel_id: u8,
    pub sequence_id: u32,
    pub dependent_lead: u16,
    pub fragment_id: u16,
    pub last_fragment_id: u16,
    pub data: Box<[u8]>,
}

#[derive(Clone,Debug,PartialEq)]
pub struct PacketSentinel {
    pub channel_id: u8,
    pub sequence_id: u32,
    pub dependent_lead: u16,
}

#[derive(Clone,Debug,PartialEq)]
pub struct WindowAck {
    pub channel_id: u8,
    pub sequence_id: u32,
}

#[derive(Clone,Debug,PartialEq)]
pub enum DataEntry {
    Packet(Packet),
    PacketFragment(PacketFragment),
    PacketSentinel(PacketSentinel),
    WindowAck(WindowAck),
}

#[derive(Debug,PartialEq)]
pub struct Connect {
    pub version: ProtocolVersionId,
    pub num_channels: u8,
    pub rx_bandwidth_max: u32,
}

#[derive(Debug,PartialEq)]
pub struct ConnectAck {
}

#[derive(Debug,PartialEq)]
pub struct Disconnect {
}

#[derive(Debug,PartialEq)]
pub struct DisconnectAck {
}

#[derive(Debug,PartialEq)]
pub struct Ping {
    pub sequence_id: PingId,
}

#[derive(Debug,PartialEq)]
pub struct PingAck {
    pub sequence_id: PingId,
}

#[derive(Debug,PartialEq)]
pub struct Data {
    pub ack: bool,
    pub sequence_id: FrameId,
    pub entries: Vec<DataEntry>,
}

#[derive(Debug,PartialEq)]
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

impl Packet {
    const TYPE_ID: u8 = 0;
    const HEADER_SIZE_BYTES: usize = 9;

    pub fn read(bytes: &[u8]) -> Option<(DataEntry, usize)> {
        if bytes.len() < Self::HEADER_SIZE_BYTES {
            return None;
        }

        let header = &bytes[0 .. Self::HEADER_SIZE_BYTES];

        if header[0] != Self::TYPE_ID {
            return None;
        }
        let channel_id = header[1];

        let sequence_id = ((header[2] as u32) << 16) |
                          ((header[3] as u32) <<  8) |
                          ((header[4] as u32)      );

        let dependent_lead = ((header[5] as u16) <<  8) |
                             ((header[6] as u16)      );

        let data_size = ((header[7] as usize) <<  8) |
                        ((header[8] as usize)      );

        if Self::HEADER_SIZE_BYTES + data_size > bytes.len() {
            return None;
        }

        let data = bytes[Self::HEADER_SIZE_BYTES .. Self::HEADER_SIZE_BYTES + data_size].into();

        Some((
            DataEntry::Packet(Self {
                channel_id: channel_id,
                sequence_id: sequence_id,
                dependent_lead: dependent_lead,
                data: data,
            }),
            Self::HEADER_SIZE_BYTES + data_size
        ))
    }

    pub fn write(&self, bytes: &mut Vec<u8>) {
        assert!(self.data.len() <= u16::MAX as usize);
        let data_len = self.data.len() as u16;

        let header = [
            Self::TYPE_ID,
            self.channel_id,
            (self.sequence_id >> 16) as u8,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
            (self.dependent_lead >>  8) as u8,
            (self.dependent_lead      ) as u8,
            (data_len >>  8) as u8,
            (data_len      ) as u8,
        ];

        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&self.data);
    }

    pub fn encoded_size(&self) -> usize {
        Self::HEADER_SIZE_BYTES + self.data.len()
    }
}

impl PacketFragment {
    const TYPE_ID: u8 = 1;
    const HEADER_SIZE_BYTES: usize = 13;

    pub fn read(bytes: &[u8]) -> Option<(DataEntry, usize)> {
        if bytes.len() < Self::HEADER_SIZE_BYTES {
            return None;
        }

        let header = &bytes[0 .. Self::HEADER_SIZE_BYTES];

        if header[0] != Self::TYPE_ID {
            return None;
        }
        let channel_id = header[1];

        let sequence_id = ((header[2] as u32) << 16) |
                          ((header[3] as u32) <<  8) |
                          ((header[4] as u32)      );

        let dependent_lead = ((header[5] as u16) <<  8) |
                             ((header[6] as u16)      );

        let fragment_id = ((header[7] as u16) <<  8) |
                          ((header[8] as u16)      );

        let last_fragment_id = ((header[ 9] as u16) <<  8) |
                               ((header[10] as u16)      );

        let data_size = ((header[11] as usize) <<  8) |
                        ((header[12] as usize)      );

        if Self::HEADER_SIZE_BYTES + data_size > bytes.len() {
            return None;
        }

        let data = bytes[Self::HEADER_SIZE_BYTES .. Self::HEADER_SIZE_BYTES + data_size].into();

        Some((
            DataEntry::PacketFragment(Self {
                channel_id: channel_id,
                sequence_id: sequence_id,
                dependent_lead: dependent_lead,
                fragment_id: fragment_id,
                last_fragment_id: last_fragment_id,
                data: data,
            }),
            Self::HEADER_SIZE_BYTES + data_size
        ))
    }

    pub fn write(&self, bytes: &mut Vec<u8>) {
        assert!(self.data.len() <= u16::MAX as usize);
        let data_len = self.data.len() as u16;

        let header = [
            Self::TYPE_ID,
            self.channel_id,
            (self.sequence_id >> 16) as u8,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
            (self.dependent_lead >>  8) as u8,
            (self.dependent_lead      ) as u8,
            (self.fragment_id >>  8) as u8,
            (self.fragment_id      ) as u8,
            (self.last_fragment_id >>  8) as u8,
            (self.last_fragment_id      ) as u8,
            (data_len >>  8) as u8,
            (data_len      ) as u8,
        ];

        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&self.data);
    }

    pub fn encoded_size(&self) -> usize {
        Self::HEADER_SIZE_BYTES + self.data.len()
    }
}

impl PacketSentinel {
    const TYPE_ID: u8 = 2;
    const HEADER_SIZE_BYTES: usize = 7;

    pub fn read(bytes: &[u8]) -> Option<(DataEntry, usize)> {
        if bytes.len() < Self::HEADER_SIZE_BYTES {
            return None;
        }
        if bytes[0] != Self::TYPE_ID {
            return None;
        }

        let channel_id = bytes[1];

        let sequence_id = ((bytes[2] as u32) << 16) |
                          ((bytes[3] as u32) <<  8) |
                          ((bytes[4] as u32)      );

        let dependent_lead = ((bytes[5] as u16) <<  8) |
                             ((bytes[6] as u16)      );

        Some((
            DataEntry::PacketSentinel(Self {
                channel_id: channel_id,
                sequence_id: sequence_id,
                dependent_lead: dependent_lead,
            }),
            Self::HEADER_SIZE_BYTES
        ))
    }

    pub fn write(&self, bytes: &mut Vec<u8>) {
        let header = [
            Self::TYPE_ID,
            self.channel_id,
            (self.sequence_id >> 16) as u8,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
            (self.dependent_lead >>  8) as u8,
            (self.dependent_lead      ) as u8,
        ];

        bytes.extend_from_slice(&header);
    }

    pub fn encoded_size(&self) -> usize {
        Self::HEADER_SIZE_BYTES
    }
}

impl WindowAck {
    const TYPE_ID: u8 = 3;
    const HEADER_SIZE_BYTES: usize = 5;

    pub fn read(bytes: &[u8]) -> Option<(DataEntry, usize)> {
        if bytes.len() < Self::HEADER_SIZE_BYTES {
            return None;
        }
        if bytes[0] != Self::TYPE_ID {
            return None;
        }

        let channel_id = bytes[1];

        let sequence_id = ((bytes[2] as u32) << 16) |
                          ((bytes[3] as u32) <<  8) |
                          ((bytes[4] as u32)      );

        Some((
            DataEntry::WindowAck(Self {
                channel_id: channel_id,
                sequence_id: sequence_id,
            }),
            Self::HEADER_SIZE_BYTES
        ))
    }

    pub fn write(&self, bytes: &mut Vec<u8>) {
        let header = [
            Self::TYPE_ID,
            self.channel_id,
            (self.sequence_id >> 16) as u8,
            (self.sequence_id >>  8) as u8,
            (self.sequence_id      ) as u8,
        ];

        bytes.extend_from_slice(&header);
    }

    pub fn encoded_size(&self) -> usize {
        Self::HEADER_SIZE_BYTES
    }
}

impl DataEntry {
    pub fn read(bytes: &[u8]) -> Option<(DataEntry, usize)> {
        if bytes.len() >= 1 {
            match bytes[0] {
                Packet::TYPE_ID => Packet::read(bytes),
                PacketFragment::TYPE_ID => PacketFragment::read(bytes),
                PacketSentinel::TYPE_ID => PacketSentinel::read(bytes),
                WindowAck::TYPE_ID => WindowAck::read(bytes),
                _ => None
            }
        } else {
            None
        }
    }

    pub fn write(&self, bytes: &mut Vec<u8>) {
        match self {
            DataEntry::Packet(packet) => packet.write(bytes),
            DataEntry::PacketFragment(fragment) => fragment.write(bytes),
            DataEntry::PacketSentinel(sentinel) => sentinel.write(bytes),
            DataEntry::WindowAck(window_ack) => window_ack.write(bytes),
        }
    }

    pub fn new_packet(channel_id: u8, sequence_id: u32, dependent_lead: u16, data: Box<[u8]>) -> DataEntry {
        DataEntry::Packet(Packet {
            channel_id: channel_id,
            sequence_id: sequence_id,
            dependent_lead: dependent_lead,
            data: data,
        })
    }

    pub fn new_fragment(channel_id: u8,
                        sequence_id: u32, dependent_lead: u16,
                        fragment_id: u16, last_fragment_id: u16,
                        data: Box<[u8]>) -> DataEntry {
        DataEntry::PacketFragment(PacketFragment {
            channel_id: channel_id,
            sequence_id: sequence_id,
            dependent_lead: dependent_lead,
            fragment_id: fragment_id,
            last_fragment_id: last_fragment_id,
            data: data,
        })
    }

    pub fn new_sentinel(channel_id: u8, sequence_id: u32, dependent_lead: u16) -> DataEntry {
        DataEntry::PacketSentinel(PacketSentinel {
            channel_id: channel_id,
            sequence_id: sequence_id,
            dependent_lead: dependent_lead,
        })
    }

    pub fn encoded_size(&self) -> usize {
        match self {
            DataEntry::Packet(entry) => entry.encoded_size(),
            DataEntry::PacketFragment(entry) => entry.encoded_size(),
            DataEntry::PacketSentinel(entry) => entry.encoded_size(),
            DataEntry::WindowAck(entry) => entry.encoded_size(),
        }
    }
}

impl Connect {
    const TYPE_ID: u8 = 0;
    const HEADER_SIZE_BYTES: usize = 7;

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
        let version = bytes[1];
        let num_channels = bytes[2];
        let rx_bandwidth_max = ((bytes[3] as u32) << 24) |
                               ((bytes[4] as u32) << 16) |
                               ((bytes[5] as u32) <<  8) |
                               ((bytes[6] as u32)      );

        Some(Self {
            version: version,
            num_channels: num_channels,
            rx_bandwidth_max: rx_bandwidth_max,
        })
    }
}

impl ConnectAck {
    const TYPE_ID: u8 = 1;
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

    pub fn to_bytes(&self) -> Box<[u8]> {
        assert!(self.entries.len() <= u16::MAX as usize);
        let entry_num = self.entries.len() as u16;

        let mut bytes = Vec::new();

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

        let ack = match header[1] { 0 => false, 1 => true, _ => { return None } };

        let sequence_id = ((header[2] as u32) << 24) |
                          ((header[3] as u32) << 16) |
                          ((header[4] as u32) <<  8) |
                          ((header[5] as u32)      );

        let entry_num = ((header[6] as u16) <<  8) |
                        ((header[7] as u16)      );

        let mut read_idx = Self::HEADER_SIZE_BYTES;
        let mut entries = Vec::new();

        for _ in 0..entry_num {
            if let Some((entry, read_size)) = DataEntry::read(&bytes[read_idx..]) {
                read_idx += read_size;
                entries.push(entry);
            } else {
                return None
            }
        }

        if read_idx == bytes.len() {
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

pub struct DataBuilder {
    entries: Vec<DataEntry>,
    size: usize,
    max_size: usize,
}

impl DataBuilder {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: Vec::new(),
            size: Data::HEADER_SIZE_BYTES,
            max_size: max_size,
        }
    }

    pub fn can_add(&mut self, entry: &DataEntry) -> bool {
        self.entries.len() == 0 || self.size + entry.encoded_size() <= self.max_size
    }

    pub fn add(&mut self, entry: DataEntry) {
        self.entries.push(entry);
    }

    pub fn build(self) -> (Data, usize) {
        (
            Data {
                ack: false,
                sequence_id: 0,
                entries: self.entries,
            },
            self.size
        )
    }
}

#[cfg(test)]
fn verify_bytes_consistent(f: &Frame) {
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

#[test]
fn test_connect_basic() {
    let f = Frame::Connect(Connect {
        version: 0x7F,
        num_channels: 3,
        rx_bandwidth_max: 0xBEEFBEEF,
    });
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_connect_ack_basic() {
    let f = Frame::ConnectAck(ConnectAck {});
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_disconnect_basic() {
    let f = Frame::Disconnect(Disconnect {});
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_disconnect_ack_basic() {
    let f = Frame::DisconnectAck(DisconnectAck {});
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_ping_basic() {
    let f = Frame::Ping(Ping {
        sequence_id: 0xBEEF,
    });
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_ping_ack_basic() {
    let f = Frame::PingAck(PingAck {
        sequence_id: 0xBEEF,
    });
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_data_basic() {
    let f = Frame::Data(Data {
        ack: true,
        sequence_id: 0x010203,
        entries: vec![
            DataEntry::new_packet(69, 0xBEEF, 1, vec![  0,  1,  2,  3 ].into_boxed_slice()),
            DataEntry::new_fragment(69, 0xBEF0, 2, 10, 10, vec![  4,  5,  6,  7 ].into_boxed_slice()),
            DataEntry::new_sentinel(69, 0xBEF1, 3),
        ],
    });
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_data_ack_basic() {
    let f = Frame::DataAck(DataAck {
        sequence_ids: vec![
            0x00000000, 0x00112233, 0x54545454, 0x77777777
        ],
    });
    verify_bytes_consistent(&f);
    verify_extra_bytes_fail(&f);
}

#[test]
fn test_connect_random() {
    const NUM_ROUNDS: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let f = Frame::Connect(Connect {
            version: rand::random::<u8>(),
            num_channels: rand::random::<u8>(),
            rx_bandwidth_max: rand::random::<u32>(),
        });
        verify_bytes_consistent(&f);
        verify_extra_bytes_fail(&f);
    }
}

#[test]
fn test_ping_random() {
    const NUM_ROUNDS: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let f = Frame::Ping(Ping {
            sequence_id: rand::random::<u16>(),
        });
        verify_bytes_consistent(&f);
        verify_extra_bytes_fail(&f);
    }
}

#[test]
fn test_ping_ack_random() {
    const NUM_ROUNDS: usize = 100;

    for _ in 0..NUM_ROUNDS {
        let f = Frame::PingAck(PingAck {
            sequence_id: rand::random::<u16>(),
        });
        verify_bytes_consistent(&f);
        verify_extra_bytes_fail(&f);
    }
}

#[test]
fn test_data_random() {
    const NUM_ROUNDS: usize = 100;
    const MAX_DATAGRAMS: usize = 1000;
    const MAX_DATA_SIZE: usize = 1000;

    for _ in 0..NUM_ROUNDS {
        let mut entries = Vec::new();

        for _ in 0..rand::random::<usize>() % MAX_DATAGRAMS {
            match rand::random::<u32>() % 3 {
                0 => {
                    let entry = DataEntry::new_packet(rand::random::<u8>(),
                                                      rand::random::<u32>() & 0xFFFFFF,
                                                      rand::random::<u16>(),
                                                      (0..MAX_DATA_SIZE).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice());
                    entries.push(entry);
                }
                1 => {
                    let entry = DataEntry::new_fragment(rand::random::<u8>(),
                                                        rand::random::<u32>() & 0xFFFFFF,
                                                        rand::random::<u16>(),
                                                        rand::random::<u16>(),
                                                        rand::random::<u16>(),
                                                        (0..MAX_DATA_SIZE).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice());
                    entries.push(entry);
                }
                2 => {
                    let entry = DataEntry::new_sentinel(rand::random::<u8>(),
                                                        rand::random::<u32>() & 0xFFFFFF,
                                                        rand::random::<u16>());
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

        verify_bytes_consistent(&f);
        verify_extra_bytes_fail(&f);
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
        verify_bytes_consistent(&f);
        verify_extra_bytes_fail(&f);
    }
}

