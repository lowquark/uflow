
pub mod build;

use super::FragmentId;
use super::Datagram;
use super::Ack;
use super::Resync;
use super::Message;

use super::ConnectFrame;
use super::ConnectAckFrame;
use super::DisconnectFrame;
use super::DisconnectAckFrame;
use super::MessageFrame;
use super::Frame;


pub trait Serialize {
    fn read(data: &[u8]) -> Option<Self> where Self: Sized;
    fn write(&self) -> Box<[u8]>;
}


const CONNECT_FRAME_ID: u8 = 0;
const CONNECT_ACK_FRAME_ID: u8 = 1;
const DISCONNECT_FRAME_ID: u8 = 2;
const DISCONNECT_ACK_FRAME_ID: u8 = 3;
const MESSAGE_FRAME_ID: u8 = 4;


fn read_connect(data: &[u8]) -> Option<Frame> {
    if data.len() != 4 {
        return None;
    }

    let nonce = ((data[0] as u32) << 24) |
                ((data[1] as u32) << 16) |
                ((data[2] as u32) <<  8) |
                ((data[3] as u32)      );

    Some(Frame::ConnectFrame(ConnectFrame {
        nonce,
        version: 0,
        num_channels: 0,
        max_rx_bandwidth: 0,
    }))
}

fn read_connect_ack(data: &[u8]) -> Option<Frame> {
    if data.len() != 4 {
        return None;
    }

    let nonce = ((data[0] as u32) << 24) |
                ((data[1] as u32) << 16) |
                ((data[2] as u32) <<  8) |
                ((data[3] as u32)      );

    Some(Frame::ConnectAckFrame(ConnectAckFrame { nonce }))
}

fn read_disconnect(data: &[u8]) -> Option<Frame> {
    if data.len() != 0 {
        return None;
    }

    Some(Frame::DisconnectFrame(DisconnectFrame { }))
}

fn read_disconnect_ack(data: &[u8]) -> Option<Frame> {
    if data.len() != 0 {
        return None;
    }

    Some(Frame::DisconnectAckFrame(DisconnectAckFrame { }))
}

fn read_message_message(data: &[u8]) -> Option<(Message, usize)> {
    if data.len() < 1 {
        return None;
    }

    let header_byte = data[0];

    if header_byte & 0x80 == 0x80 {
        // Datagram
        if header_byte & 0x40 == 0x40 {
            const HEADER_SIZE: usize = 15;

            if data.len() < HEADER_SIZE {
                return None;
            }

            // Fragment
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

            if data.len() < HEADER_SIZE + data_len {
                return None;
            }

            let data = data[HEADER_SIZE .. HEADER_SIZE + data_len].into();

            return Some((Message::Datagram(Datagram {
                channel_id,
                sequence_id,
                window_parent_lead,
                channel_parent_lead,
                fragment_id: FragmentId { id: fragment_id, last: fragment_id_last },
                data,
            }), HEADER_SIZE + data_len));
        } else {
            const HEADER_SIZE: usize = 11;

            if data.len() < HEADER_SIZE {
                return None;
            }

            // Full
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

            if data.len() < HEADER_SIZE + data_len {
                return None;
            }

            let data = data[HEADER_SIZE .. HEADER_SIZE + data_len].into();

            return Some((Message::Datagram(Datagram {
                channel_id,
                sequence_id,
                window_parent_lead,
                channel_parent_lead,
                fragment_id: FragmentId { id: 0, last: 0 },
                data,
            }), HEADER_SIZE + data_len));
        }
    } else if header_byte & 0xC == 0x00 {
        // Ack
        let frame_bits_nonce = header_byte & 0x01 == 0x01;

        let frame_bits_base_id = ((data[1] as u32) << 24) |
                                 ((data[2] as u32) << 16) |
                                 ((data[3] as u32) <<  8) |
                                 ((data[4] as u32)      );

        let frame_bits = ((data[5] as u32) << 24) |
                         ((data[6] as u32) << 16) |
                         ((data[7] as u32) <<  8) |
                         ((data[8] as u32)      );

        let receiver_base_id = ((data[ 9] as u32) << 24) |
                               ((data[10] as u32) << 16) |
                               ((data[11] as u32) <<  8) |
                               ((data[12] as u32)      );

        return Some((Message::Ack(Ack { frame_bits_nonce, frame_bits_base_id, frame_bits, receiver_base_id }), 13));
    } else if header_byte & 0xC == 0x40 {
        // Resync
        let sender_next_id = ((data[1] as u32) << 24) |
                             ((data[2] as u32) << 16) |
                             ((data[3] as u32) <<  8) |
                             ((data[4] as u32)      );

        return Some((Message::Resync(Resync { sender_next_id }), 5));
    }

    None
}

fn read_message(data: &[u8]) -> Option<Frame> {
    // TODO: Rely on reader object

    if data.len() < 7 {
        return None;
    }

    let sequence_id = ((data[0] as u32) << 24) |
                      ((data[1] as u32) << 16) |
                      ((data[2] as u32) <<  8) |
                      ((data[3] as u32)      );

    let nonce = data[4] != 0;

    let message_num = ((data[5] as u16) << 8) |
                      ((data[6] as u16)     );

    let mut data_slice = &data[7..];
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
        (frame.nonce >> 24) as u8,
        (frame.nonce >> 16) as u8,
        (frame.nonce >>  8) as u8,
        (frame.nonce      ) as u8,
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

