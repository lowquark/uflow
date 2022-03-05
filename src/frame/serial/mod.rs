
mod build;

pub use build::DataFrameBuilder;
pub use build::AckFrameBuilder;

use super::*;

const CONNECT_FRAME_ID: u8 = 0;
const CONNECT_ACK_FRAME_ID: u8 = 1;
const DISCONNECT_FRAME_ID: u8 = 2;
const DISCONNECT_ACK_FRAME_ID: u8 = 3;
const DATA_FRAME_ID: u8 = 4;
const SYNC_FRAME_ID: u8 = 5;
const ACK_FRAME_ID: u8 = 6;

const DATAGRAM_HEADER_SIZE_FRAGMENT: usize = 15;
const DATAGRAM_HEADER_SIZE_FULL: usize = 11;

const FRAME_ACK_SIZE: usize = 9;

const CONNECT_FRAME_PAYLOAD_SIZE: usize = 18;
const CONNECT_ACK_FRAME_PAYLOAD_SIZE: usize = 4;

const DISCONNECT_FRAME_PAYLOAD_SIZE: usize = 0;
const DISCONNECT_ACK_FRAME_PAYLOAD_SIZE: usize = 0;

const DATA_FRAME_PAYLOAD_HEADER_SIZE: usize = 7;

const SYNC_FRAME_PAYLOAD_SIZE: usize = 9;

const ACK_FRAME_PAYLOAD_HEADER_SIZE: usize = 10;

pub use build::MAX_CHANNELS;
pub const MAX_FRAGMENTS: usize = 1 << 16;
pub const MAX_DATAGRAM_OVERHEAD: usize = DATAGRAM_HEADER_SIZE_FRAGMENT;
pub const DATA_FRAME_OVERHEAD: usize = 1 + DATA_FRAME_PAYLOAD_HEADER_SIZE;
pub const SYNC_FRAME_SIZE: usize = 1 + SYNC_FRAME_PAYLOAD_SIZE;

fn read_connect_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != CONNECT_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let version = data[0];

    let nonce = ((data[1] as u32) << 24) |
                ((data[2] as u32) << 16) |
                ((data[3] as u32) <<  8) |
                ((data[4] as u32)      );

    let channel_count_sup = data[5];

    let max_receive_rate = ((data[6] as u32) << 24) |
                           ((data[7] as u32) << 16) |
                           ((data[8] as u32) <<  8) |
                           ((data[9] as u32)      );

    let max_packet_size = ((data[10] as u32) << 24) |
                          ((data[11] as u32) << 16) |
                          ((data[12] as u32) <<  8) |
                          ((data[13] as u32)      );

    let max_receive_alloc = ((data[14] as u32) << 24) |
                            ((data[15] as u32) << 16) |
                            ((data[16] as u32) <<  8) |
                            ((data[17] as u32)      );

    Some(Frame::ConnectFrame(ConnectFrame {
        version,
        nonce,
        channel_count_sup,
        max_receive_rate,
        max_packet_size,
        max_receive_alloc,
    }))
}

fn read_connect_ack_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != CONNECT_ACK_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let nonce = ((data[0] as u32) << 24) |
                ((data[1] as u32) << 16) |
                ((data[2] as u32) <<  8) |
                ((data[3] as u32)      );

    Some(Frame::ConnectAckFrame(ConnectAckFrame { nonce }))
}

fn read_disconnect_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != DISCONNECT_FRAME_PAYLOAD_SIZE {
        return None;
    }

    Some(Frame::DisconnectFrame(DisconnectFrame { }))
}

fn read_disconnect_ack_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != DISCONNECT_ACK_FRAME_PAYLOAD_SIZE {
        return None;
    }

    Some(Frame::DisconnectAckFrame(DisconnectAckFrame { }))
}

fn read_datagram(data: &[u8]) -> Option<(Datagram, usize)> {
    if data.len() < 1 {
        return None;
    }

    let header_byte = data[0];

    if header_byte & 0x80 == 0x80 {
        // Fragment
        let header_size = DATAGRAM_HEADER_SIZE_FRAGMENT;

        if data.len() < header_size {
            return None;
        }

        let channel_id = header_byte & 0x3F;

        let sequence_id = ((data[1] as u32) << 24) |
                          ((data[2] as u32) << 16) |
                          ((data[3] as u32) <<  8) |
                          ((data[4] as u32)      );

        let window_parent_lead = ((data[5] as u16) << 8) |
                                 ((data[6] as u16)     );

        let channel_parent_lead = ((data[7] as u16) << 8) |
                                  ((data[8] as u16)     );

        let fragment_id_last = ((data[ 9] as u16) << 8) |
                               ((data[10] as u16)     );

        let fragment_id = ((data[11] as u16) << 8) |
                          ((data[12] as u16)     );

        let data_len_u16 = ((data[13] as u16) << 8) |
                           ((data[14] as u16)     );
        let data_len = data_len_u16 as usize;

        if data.len() < header_size + data_len {
            return None;
        }

        let data = data[header_size .. header_size + data_len].into();

        return Some((Datagram {
            channel_id,
            sequence_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id: FragmentId { id: fragment_id, last: fragment_id_last },
            data,
        }, header_size + data_len));
    } else {
        // Full
        let header_size = DATAGRAM_HEADER_SIZE_FULL;

        if data.len() < header_size {
            return None;
        }

        let channel_id = header_byte & 0x3F;

        let sequence_id = ((data[1] as u32) << 24) |
                          ((data[2] as u32) << 16) |
                          ((data[3] as u32) <<  8) |
                          ((data[4] as u32)      );

        let window_parent_lead = ((data[5] as u16) << 8) |
                                 ((data[6] as u16)     );

        let channel_parent_lead = ((data[7] as u16) << 8) |
                                  ((data[8] as u16)     );

        let data_len_u16 = ((data[ 9] as u16) << 8) |
                           ((data[10] as u16)     );
        let data_len = data_len_u16 as usize;

        if data.len() < header_size + data_len {
            return None;
        }

        let data = data[header_size .. header_size + data_len].into();

        return Some((Datagram {
            channel_id,
            sequence_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id: FragmentId { id: 0, last: 0 },
            data,
        }, header_size + data_len));
    }
}

fn read_data_payload(data: &[u8]) -> Option<Frame> {
    // TODO: Rely on reader object

    if data.len() < DATA_FRAME_PAYLOAD_HEADER_SIZE {
        return None;
    }

    let sequence_id = ((data[0] as u32) << 24) |
                      ((data[1] as u32) << 16) |
                      ((data[2] as u32) <<  8) |
                      ((data[3] as u32)      );

    let nonce = data[4] != 0;

    let datagram_num = ((data[5] as u16) << 8) |
                       ((data[6] as u16)     );

    let mut data_slice = &data[DATA_FRAME_PAYLOAD_HEADER_SIZE..];
    let mut datagrams = Vec::new();

    for _ in 0 .. datagram_num {
        if let Some((datagram, read_size)) = read_datagram(data_slice) {
            datagrams.push(datagram);
            data_slice = &data_slice[read_size..];
        } else {
            return None;
        }
    }

    if data_slice.len() != 0 {
        return None;
    }

    Some(Frame::DataFrame(DataFrame { sequence_id, nonce, datagrams }))
}

fn read_sync_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != SYNC_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let sequence_id = ((data[0] as u32) << 24) |
                      ((data[1] as u32) << 16) |
                      ((data[2] as u32) <<  8) |
                      ((data[3] as u32)      );

    let nonce = data[4] != 0;

    let sender_next_id = ((data[5] as u32) << 24) |
                         ((data[6] as u32) << 16) |
                         ((data[7] as u32) <<  8) |
                         ((data[8] as u32)      );

    Some(Frame::SyncFrame(SyncFrame { sequence_id, nonce, sender_next_id }))
}

fn read_frame_ack(data: &[u8]) -> Option<(FrameAck, usize)> {
    // Ack
    if data.len() < FRAME_ACK_SIZE {
        return None;
    }

    let base_id = ((data[0] as u32) << 24) |
                  ((data[1] as u32) << 16) |
                  ((data[2] as u32) <<  8) |
                  ((data[3] as u32)      );

    let bitfield = ((data[4] as u32) << 24) |
                   ((data[5] as u32) << 16) |
                   ((data[6] as u32) <<  8) |
                   ((data[7] as u32)      );

    let nonce = data[8] != 0;

    return Some((FrameAck {
        base_id,
        bitfield,
        nonce,
    }, FRAME_ACK_SIZE));
}

fn read_ack_payload(data: &[u8]) -> Option<Frame> {
    if data.len() < ACK_FRAME_PAYLOAD_HEADER_SIZE {
        return None;
    }

    let frame_window_base_id = ((data[0] as u32) << 24) |
                               ((data[1] as u32) << 16) |
                               ((data[2] as u32) <<  8) |
                               ((data[3] as u32)      );

    let packet_window_base_id = ((data[4] as u32) << 24) |
                                ((data[5] as u32) << 16) |
                                ((data[6] as u32) <<  8) |
                                ((data[7] as u32)      );

    let frame_ack_num = ((data[8] as u16) << 8) |
                        ((data[9] as u16)     );

    let mut data_slice = &data[ACK_FRAME_PAYLOAD_HEADER_SIZE..];
    let mut frame_acks = Vec::new();

    for _ in 0 .. frame_ack_num {
        if let Some((frame_ack, read_size)) = read_frame_ack(data_slice) {
            frame_acks.push(frame_ack);
            data_slice = &data_slice[read_size..];
        } else {
            return None;
        }
    }

    if data_slice.len() != 0 {
        return None;
    }

    Some(Frame::AckFrame(AckFrame { frame_window_base_id, packet_window_base_id, frame_acks }))
}


fn write_connect(frame: &ConnectFrame) -> Box<[u8]> {
    Box::new([
        CONNECT_FRAME_ID,
        frame.version,
        (frame.nonce >> 24) as u8,
        (frame.nonce >> 16) as u8,
        (frame.nonce >>  8) as u8,
        (frame.nonce      ) as u8,
        frame.channel_count_sup,
        (frame.max_receive_rate >> 24) as u8,
        (frame.max_receive_rate >> 16) as u8,
        (frame.max_receive_rate >>  8) as u8,
        (frame.max_receive_rate      ) as u8,
        (frame.max_packet_size >> 24) as u8,
        (frame.max_packet_size >> 16) as u8,
        (frame.max_packet_size >>  8) as u8,
        (frame.max_packet_size      ) as u8,
        (frame.max_receive_alloc >> 24) as u8,
        (frame.max_receive_alloc >> 16) as u8,
        (frame.max_receive_alloc >>  8) as u8,
        (frame.max_receive_alloc      ) as u8,
    ])
}

fn write_connect_ack(frame: &ConnectAckFrame) -> Box<[u8]> {
    Box::new([
        CONNECT_ACK_FRAME_ID,
        (frame.nonce >> 24) as u8,
        (frame.nonce >> 16) as u8,
        (frame.nonce >>  8) as u8,
        (frame.nonce      ) as u8,
    ])
}

fn write_disconnect(_frame: &DisconnectFrame) -> Box<[u8]> {
    Box::new([
        DISCONNECT_FRAME_ID
    ])
}

fn write_disconnect_ack(_frame: &DisconnectAckFrame) -> Box<[u8]> {
    Box::new([
        DISCONNECT_ACK_FRAME_ID
    ])
}

fn write_data(frame: &DataFrame) -> Box<[u8]> {
    let mut builder = build::DataFrameBuilder::new(frame.sequence_id, frame.nonce);

    for datagram in frame.datagrams.iter() {
        builder.add(&datagram.into());
    }

    builder.build()
}

fn write_sync(frame: &SyncFrame) -> Box<[u8]> {
    Box::new([
        SYNC_FRAME_ID,
        (frame.sequence_id >> 24) as u8,
        (frame.sequence_id >> 16) as u8,
        (frame.sequence_id >>  8) as u8,
        (frame.sequence_id      ) as u8,
        frame.nonce as u8,
        (frame.sender_next_id >> 24) as u8,
        (frame.sender_next_id >> 16) as u8,
        (frame.sender_next_id >>  8) as u8,
        (frame.sender_next_id      ) as u8,
    ])
}

fn write_ack(frame: &AckFrame) -> Box<[u8]> {
    let mut builder = build::AckFrameBuilder::new(frame.frame_window_base_id, frame.packet_window_base_id);

    for frame_ack in frame.frame_acks.iter() {
        builder.add(frame_ack);
    }

    builder.build()
}

pub trait Serialize {
    fn read(data: &[u8]) -> Option<Self> where Self: Sized;
    fn write(&self) -> Box<[u8]>;
}

impl Serialize for Frame {
    fn read(data: &[u8]) -> Option<Self> {
        if data.len() == 0 {
            return None;
        }

        match data[0] {
            CONNECT_FRAME_ID => read_connect_payload(&data[1..]),
            CONNECT_ACK_FRAME_ID => read_connect_ack_payload(&data[1..]),
            DISCONNECT_FRAME_ID => read_disconnect_payload(&data[1..]),
            DISCONNECT_ACK_FRAME_ID => read_disconnect_ack_payload(&data[1..]),
            DATA_FRAME_ID => read_data_payload(&data[1..]),
            SYNC_FRAME_ID => read_sync_payload(&data[1..]),
            ACK_FRAME_ID => read_ack_payload(&data[1..]),
            _ => None,
        }
    }

    fn write(&self) -> Box<[u8]> {
        match self {
            Frame::ConnectFrame(frame) => write_connect(frame),
            Frame::ConnectAckFrame(frame) => write_connect_ack(frame),
            Frame::DisconnectFrame(frame) => write_disconnect(frame),
            Frame::DisconnectAckFrame(frame) => write_disconnect_ack(frame),
            Frame::DataFrame(frame) => write_data(frame),
            Frame::SyncFrame(frame) => write_sync(frame),
            Frame::AckFrame(frame) => write_ack(frame),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify_consistent(f: &Frame) {
        //println!("frame: {:#?}", f);

        let bytes = f.write();
        //println!("frame bytes: {:?}", bytes);

        let f2 = Frame::read(&bytes).unwrap();

        assert_eq!(*f, f2);
    }

    fn verify_extra_bytes_fail(f: &Frame) {
        //println!("frame: {:#?}", f);

        let bytes = f.write();
        //println!("frame bytes: {:?}", bytes);

        let mut bad_bytes_vec = bytes.to_vec();
        bad_bytes_vec.push(0x00);
        let bad_bytes = bad_bytes_vec.into_boxed_slice();

        assert_eq!(Frame::read(&bad_bytes), None);
    }

    fn verify_truncation_fails(f: &Frame) {
        let bytes = f.write();

        for i in 1..bytes.len() {
            let bytes_trunc = &bytes[0..i];
            assert_eq!(Frame::read(&bytes_trunc), None);
        }
    }

    #[test]
    fn connect_basic() {
        let f = Frame::ConnectFrame(ConnectFrame {
            version: 0x7F,
            nonce: 0x18273645,
            channel_count_sup: 3,
            max_receive_rate: 0x98765432,
            max_packet_size: 0x01234567,
            max_receive_alloc: 0xABCDEF01,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn connect_ack_basic() {
        let f = Frame::ConnectAckFrame(ConnectAckFrame {
            nonce: 0x13371337,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn disconnect_basic() {
        let f = Frame::DisconnectFrame(DisconnectFrame {});
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn disconnect_ack_basic() {
        let f = Frame::DisconnectAckFrame(DisconnectAckFrame {});
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn data_basic() {
        let f = Frame::DataFrame(DataFrame {
            sequence_id: 0x010203,
            nonce: true,
            datagrams: vec![
                Datagram {
                    sequence_id: 0x12345678,
                    channel_id: 63,
                    window_parent_lead: 0x34A8,
                    channel_parent_lead: 0x8A43,
                    fragment_id: FragmentId {
                        id: 0x4789,
                        last: 0x478A,
                    },
                    data: vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(),
                },
                Datagram {
                    sequence_id: 0x12345678,
                    channel_id: 63,
                    window_parent_lead: 0x34A8,
                    channel_parent_lead: 0x8A43,
                    fragment_id: FragmentId {
                        id: 0,
                        last: 0,
                    },
                    data: vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(),
                },
            ],
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn sync_basic() {
        let f = Frame::SyncFrame(SyncFrame {
            sequence_id: 0x010203,
            nonce: true,
            sender_next_id: 0x12345678,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn ack_basic() {
        let f = Frame::AckFrame(AckFrame {
            frame_window_base_id: 0x010203,
            packet_window_base_id: 0x040506,
            frame_acks: vec![
                FrameAck {
                    base_id: 0x28475809,
                    bitfield: 0b01000100111101110110100110101u32,
                    nonce: true,
                },
            ],
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn data_empty() {
        let f = Frame::DataFrame(DataFrame {
            sequence_id: 0x010203,
            nonce: true,
            datagrams: Vec::new(),
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn ack_empty() {
        let f = Frame::AckFrame(AckFrame {
            frame_window_base_id: 0x010203,
            packet_window_base_id: 0x040506,
            frame_acks: Vec::new(),
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn connect_random() {
        const NUM_ROUNDS: usize = 100;

        for _ in 0..NUM_ROUNDS {
            let f = Frame::ConnectFrame(ConnectFrame {
                version: rand::random::<u8>(),
                nonce: rand::random::<u32>(),
                channel_count_sup: rand::random::<u8>(),
                max_receive_rate: rand::random::<u32>(),
                max_packet_size: rand::random::<u32>(),
                max_receive_alloc: rand::random::<u32>(),
            });
            verify_consistent(&f);
            verify_extra_bytes_fail(&f);
            verify_truncation_fails(&f);
        }
    }

    #[test]
    fn connect_ack_random() {
        const NUM_ROUNDS: usize = 100;

        for _ in 0..NUM_ROUNDS {
            let f = Frame::ConnectAckFrame(ConnectAckFrame {
                nonce: rand::random::<u32>(),
            });
            verify_consistent(&f);
            verify_extra_bytes_fail(&f);
            verify_truncation_fails(&f);
        }
    }

    fn random_data(size: usize) -> Box<[u8]> {
        (0..size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice()
    }

    #[test]
    fn data_random() {
        const NUM_ROUNDS: usize = 100;
        const MAX_DATAGRAMS: usize = 100;
        const MAX_DATA_SIZE: usize = 100;

        for _ in 0..NUM_ROUNDS {
            let mut datagrams = Vec::new();

            for _ in 0 .. rand::random::<usize>() % MAX_DATAGRAMS {
                let datagram = match rand::random::<u32>() % 2 {
                    0 => Datagram {
                        sequence_id: rand::random::<u32>(),
                        channel_id: (rand::random::<usize>() % MAX_CHANNELS) as u8,
                        window_parent_lead: rand::random::<u16>(),
                        channel_parent_lead: rand::random::<u16>(),
                        fragment_id: FragmentId {
                            id: rand::random::<u16>(),
                            last: rand::random::<u16>(),
                        },
                        data: random_data(MAX_DATA_SIZE),
                    },
                    1 => Datagram {
                        sequence_id: rand::random::<u32>(),
                        channel_id: (rand::random::<usize>() % MAX_CHANNELS) as u8,
                        window_parent_lead: rand::random::<u16>(),
                        channel_parent_lead: rand::random::<u16>(),
                        fragment_id: FragmentId {
                            id: 0,
                            last: 0,
                        },
                        data: random_data(MAX_DATA_SIZE),
                    },
                    _ => panic!()
                };

                datagrams.push(datagram);
            }

            let f = Frame::DataFrame(DataFrame {
                sequence_id: rand::random::<u32>(),
                nonce: rand::random::<bool>(),
                datagrams: datagrams,
            });

            verify_consistent(&f);
            verify_extra_bytes_fail(&f);
            verify_truncation_fails(&f);
        }
    }

    #[test]
    fn sync_random() {
        const NUM_ROUNDS: usize = 100;

        for _ in 0..NUM_ROUNDS {
            let f = Frame::SyncFrame(SyncFrame {
                sequence_id: rand::random::<u32>(),
                nonce: rand::random::<bool>(),
                sender_next_id: rand::random::<u32>(),
            });
            verify_consistent(&f);
            verify_extra_bytes_fail(&f);
            verify_truncation_fails(&f);
        }
    }

    #[test]
    fn ack_random() {
        const NUM_ROUNDS: usize = 100;
        const MAX_FRAME_ACKS: usize = 100;

        for _ in 0..NUM_ROUNDS {
            let mut frame_acks = Vec::new();

            for _ in 0 .. rand::random::<usize>() % MAX_FRAME_ACKS {
                let frame_ack = FrameAck {
                    base_id: rand::random::<u32>(),
                    bitfield: rand::random::<u32>(),
                    nonce: rand::random(),
                };

                frame_acks.push(frame_ack);
            }

            let f = Frame::AckFrame(AckFrame {
                frame_window_base_id: rand::random::<u32>(),
                packet_window_base_id: rand::random::<u32>(),
                frame_acks: frame_acks,
            });

            verify_consistent(&f);
            verify_extra_bytes_fail(&f);
            verify_truncation_fails(&f);
        }
    }
}

