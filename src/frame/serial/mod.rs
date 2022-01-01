
mod build;

pub use build::MessageFrameBuilder;

use super::*;

const CONNECT_FRAME_ID: u8 = 0;
const CONNECT_ACK_FRAME_ID: u8 = 1;
const DISCONNECT_FRAME_ID: u8 = 2;
const DISCONNECT_ACK_FRAME_ID: u8 = 3;
const MESSAGE_FRAME_ID: u8 = 4;

const CONNECT_FRAME_PAYLOAD_SIZE: usize = 14;
const CONNECT_ACK_FRAME_PAYLOAD_SIZE: usize = 4;
const DISCONNECT_FRAME_PAYLOAD_SIZE: usize = 0;
const DISCONNECT_ACK_FRAME_PAYLOAD_SIZE: usize = 0;

const MESSAGE_HEADER_SIZE: usize = 1;
const DATAGRAM_MESSAGE_HEADER_SIZE_FRAGMENT: usize = 15;
const DATAGRAM_MESSAGE_HEADER_SIZE_FULL: usize = 11;
const ACK_MESSAGE_SIZE: usize = 14;
const RESYNC_MESSAGE_SIZE: usize = 5;

const MESSAGE_FRAME_PAYLOAD_HEADER_SIZE: usize = 7;

pub use build::MAX_CHANNELS;
pub const MAX_MESSAGE_OVERHEAD: usize = DATAGRAM_MESSAGE_HEADER_SIZE_FRAGMENT;
pub const MESSAGE_FRAME_OVERHEAD: usize = 1 + MESSAGE_FRAME_PAYLOAD_HEADER_SIZE;
pub const MAX_FRAGMENTS: usize = 1 << 16;

fn read_connect(data: &[u8]) -> Option<Frame> {
    if data.len() != CONNECT_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let version = data[0];

    let nonce = ((data[1] as u32) << 24) |
                ((data[2] as u32) << 16) |
                ((data[3] as u32) <<  8) |
                ((data[4] as u32)      );

    let tx_channels_sup = data[5];

    let max_rx_alloc = ((data[6] as u32) << 24) |
                       ((data[7] as u32) << 16) |
                       ((data[8] as u32) <<  8) |
                       ((data[9] as u32)      );

    let max_rx_bandwidth = ((data[10] as u32) << 24) |
                           ((data[11] as u32) << 16) |
                           ((data[12] as u32) <<  8) |
                           ((data[13] as u32)      );

    Some(Frame::ConnectFrame(ConnectFrame {
        version,
        nonce,
        tx_channels_sup,
        max_rx_alloc,
        max_rx_bandwidth,
    }))
}

fn read_connect_ack(data: &[u8]) -> Option<Frame> {
    if data.len() != CONNECT_ACK_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let nonce = ((data[0] as u32) << 24) |
                ((data[1] as u32) << 16) |
                ((data[2] as u32) <<  8) |
                ((data[3] as u32)      );

    Some(Frame::ConnectAckFrame(ConnectAckFrame { nonce }))
}

fn read_disconnect(data: &[u8]) -> Option<Frame> {
    if data.len() != DISCONNECT_FRAME_PAYLOAD_SIZE {
        return None;
    }

    Some(Frame::DisconnectFrame(DisconnectFrame { }))
}

fn read_disconnect_ack(data: &[u8]) -> Option<Frame> {
    if data.len() != DISCONNECT_ACK_FRAME_PAYLOAD_SIZE {
        return None;
    }

    Some(Frame::DisconnectAckFrame(DisconnectAckFrame { }))
}

fn read_message_message(data: &[u8]) -> Option<(Message, usize)> {
    if data.len() < MESSAGE_HEADER_SIZE {
        return None;
    }

    let header_byte = data[0];

    if header_byte & 0x80 == 0x80 {
        // Datagram
        if header_byte & 0x40 == 0x40 {
            // Fragment
            let header_size = DATAGRAM_MESSAGE_HEADER_SIZE_FRAGMENT;

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

            return Some((Message::Datagram(Datagram {
                channel_id,
                sequence_id,
                window_parent_lead,
                channel_parent_lead,
                fragment_id: FragmentId { id: fragment_id, last: fragment_id_last },
                data,
            }), header_size + data_len));
        } else {
            // Full
            let header_size = DATAGRAM_MESSAGE_HEADER_SIZE_FULL;

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

            return Some((Message::Datagram(Datagram {
                channel_id,
                sequence_id,
                window_parent_lead,
                channel_parent_lead,
                fragment_id: FragmentId { id: 0, last: 0 },
                data,
            }), header_size + data_len));
        }
    } else if header_byte & 0xC0 == 0x00 {
        // Ack
        if data.len() < ACK_MESSAGE_SIZE {
            return None;
        }

        let frame_ack_nonce = header_byte & 0x01 == 0x01;

        let frame_ack_base_id = ((data[1] as u32) << 24) |
                                ((data[2] as u32) << 16) |
                                ((data[3] as u32) <<  8) |
                                ((data[4] as u32)      );

        let frame_ack_size = data[5];

        let frame_ack_bitfield = ((data[6] as u32) << 24) |
                                 ((data[7] as u32) << 16) |
                                 ((data[8] as u32) <<  8) |
                                 ((data[9] as u32)      );

        let receiver_base_id = ((data[10] as u32) << 24) |
                               ((data[11] as u32) << 16) |
                               ((data[12] as u32) <<  8) |
                               ((data[13] as u32)      );

        return Some((Message::Ack(Ack {
            frames: FrameAck {
                base_id: frame_ack_base_id,
                size: frame_ack_size,
                bitfield: frame_ack_bitfield,
                nonce: frame_ack_nonce,
            },
            receiver_base_id,
        }), ACK_MESSAGE_SIZE));
    } else if header_byte & 0xC0 == 0x40 {
        // Resync
        if data.len() < RESYNC_MESSAGE_SIZE {
            return None;
        }

        let sender_next_id = ((data[1] as u32) << 24) |
                             ((data[2] as u32) << 16) |
                             ((data[3] as u32) <<  8) |
                             ((data[4] as u32)      );

        return Some((Message::Resync(Resync {
            sender_next_id,
        }), RESYNC_MESSAGE_SIZE));
    }

    None
}

fn read_message(data: &[u8]) -> Option<Frame> {
    // TODO: Rely on reader object

    if data.len() < MESSAGE_FRAME_PAYLOAD_HEADER_SIZE {
        return None;
    }

    let sequence_id = ((data[0] as u32) << 24) |
                      ((data[1] as u32) << 16) |
                      ((data[2] as u32) <<  8) |
                      ((data[3] as u32)      );

    let nonce = data[4] != 0;

    let message_num = ((data[5] as u16) << 8) |
                      ((data[6] as u16)     );

    let mut data_slice = &data[MESSAGE_FRAME_PAYLOAD_HEADER_SIZE..];
    let mut messages = Vec::new();

    for _ in 0 .. message_num {
        if let Some((message, read_size)) = read_message_message(data_slice) {
            messages.push(message);
            data_slice = &data_slice[read_size..];
        } else {
            return None;
        }
    }

    if data_slice.len() != 0 {
        return None;
    }

    Some(Frame::MessageFrame(MessageFrame { sequence_id, nonce, messages }))
}


fn write_connect(frame: &ConnectFrame) -> Box<[u8]> {
    Box::new([
        CONNECT_FRAME_ID,
        frame.version,
        (frame.nonce >> 24) as u8,
        (frame.nonce >> 16) as u8,
        (frame.nonce >>  8) as u8,
        (frame.nonce      ) as u8,
        frame.tx_channels_sup,
        (frame.max_rx_alloc >> 24) as u8,
        (frame.max_rx_alloc >> 16) as u8,
        (frame.max_rx_alloc >>  8) as u8,
        (frame.max_rx_alloc      ) as u8,
        (frame.max_rx_bandwidth >> 24) as u8,
        (frame.max_rx_bandwidth >> 16) as u8,
        (frame.max_rx_bandwidth >>  8) as u8,
        (frame.max_rx_bandwidth      ) as u8,
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

fn write_message(frame: &MessageFrame) -> Box<[u8]> {
    let mut builder = build::MessageFrameBuilder::new(frame.sequence_id, frame.nonce);

    for message in frame.messages.iter() {
        builder.add(message);
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
            CONNECT_FRAME_ID => read_connect(&data[1..]),
            CONNECT_ACK_FRAME_ID => read_connect_ack(&data[1..]),
            DISCONNECT_FRAME_ID => read_disconnect(&data[1..]),
            DISCONNECT_ACK_FRAME_ID => read_disconnect_ack(&data[1..]),
            MESSAGE_FRAME_ID => read_message(&data[1..]),
            _ => None,
        }
    }

    fn write(&self) -> Box<[u8]> {
        match self {
            Frame::ConnectFrame(frame) => write_connect(frame),
            Frame::ConnectAckFrame(frame) => write_connect_ack(frame),
            Frame::DisconnectFrame(frame) => write_disconnect(frame),
            Frame::DisconnectAckFrame(frame) => write_disconnect_ack(frame),
            Frame::MessageFrame(frame) => write_message(frame),
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
            nonce: 0x13371337,
            tx_channels_sup: 3,
            max_rx_alloc: 0xBEEFBEEF,
            max_rx_bandwidth: 0xBEEFBEEF,
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
    fn message_basic() {
        let f = Frame::MessageFrame(MessageFrame {
            sequence_id: 0x010203,
            nonce: true,
            messages: vec![
                Message::Datagram(Datagram {
                    sequence_id: 0x12345678,
                    channel_id: 63,
                    window_parent_lead: 0x34A8,
                    channel_parent_lead: 0x8A43,
                    fragment_id: FragmentId {
                        id: 0x4789,
                        last: 0x478A,
                    },
                    data: vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(),
                }),
                Message::Datagram(Datagram {
                    sequence_id: 0x12345678,
                    channel_id: 63,
                    window_parent_lead: 0x34A8,
                    channel_parent_lead: 0x8A43,
                    fragment_id: FragmentId {
                        id: 0,
                        last: 0,
                    },
                    data: vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(),
                }),
                Message::Ack(Ack {
                    frames: FrameAck {
                        base_id: 0x28475809,
                        size: 32,
                        bitfield: 0b01000100111101110110100110101u32,
                        nonce: true,
                    },
                    receiver_base_id: 0x12345678,
                }),
                Message::Resync(Resync {
                    sender_next_id: 0x12345678,
                }),
            ],
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn message_empty() {
        let f = Frame::MessageFrame(MessageFrame {
            sequence_id: 0x010203,
            nonce: true,
            messages: Vec::new(),
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
                nonce: 0x13371337,
                tx_channels_sup: rand::random::<u8>(),
                max_rx_alloc: rand::random::<u32>(),
                max_rx_bandwidth: rand::random::<u32>(),
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
    fn message_random() {
        const NUM_ROUNDS: usize = 100;
        const MAX_MESSAGES: usize = 100;
        const MAX_DATA_SIZE: usize = 100;

        for _ in 0..NUM_ROUNDS {
            let mut messages = Vec::new();

            for _ in 0 .. rand::random::<usize>() % MAX_MESSAGES {
                let message = match rand::random::<u32>() % 4 {
                    0 => Message::Datagram(Datagram {
                        sequence_id: rand::random::<u32>(),
                        channel_id: (rand::random::<usize>() % MAX_CHANNELS) as u8,
                        window_parent_lead: rand::random::<u16>(),
                        channel_parent_lead: rand::random::<u16>(),
                        fragment_id: FragmentId {
                            id: rand::random::<u16>(),
                            last: rand::random::<u16>(),
                        },
                        data: random_data(MAX_DATA_SIZE),
                    }),
                    1 => Message::Datagram(Datagram {
                        sequence_id: rand::random::<u32>(),
                        channel_id: (rand::random::<usize>() % MAX_CHANNELS) as u8,
                        window_parent_lead: rand::random::<u16>(),
                        channel_parent_lead: rand::random::<u16>(),
                        fragment_id: FragmentId {
                            id: 0,
                            last: 0,
                        },
                        data: random_data(MAX_DATA_SIZE),
                    }),
                    2 => Message::Ack(Ack {
                        frames: FrameAck {
                            base_id: rand::random::<u32>(),
                            size: rand::random::<u8>(),
                            bitfield: rand::random::<u32>(),
                            nonce: rand::random::<bool>(),
                        },
                        receiver_base_id: rand::random::<u32>(),
                    }),
                    3 => Message::Resync(Resync {
                        sender_next_id: rand::random::<u32>(),
                    }),
                    _ => panic!()
                };

                messages.push(message);
            }

            let f = Frame::MessageFrame(MessageFrame {
                sequence_id: rand::random::<u32>(),
                nonce: rand::random::<bool>(),
                messages: messages,
            });

            verify_consistent(&f);
            verify_extra_bytes_fail(&f);
            verify_truncation_fails(&f);
        }
    }
}

