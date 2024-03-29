
use crate::MAX_FRAME_SIZE;
use super::*;

mod build;
mod crc;

pub use build::DataFrameBuilder;
pub use build::AckFrameBuilder;

const FRAME_HEADER_SIZE: usize = 1;
const FRAME_CRC_SIZE: usize = 4;
const FRAME_OVERHEAD: usize = FRAME_HEADER_SIZE + FRAME_CRC_SIZE;

const HANDSHAKE_SYN_FRAME_ID: u8 = 0;
const HANDSHAKE_SYN_ACK_FRAME_ID: u8 = 1;
const HANDSHAKE_ACK_FRAME_ID: u8 = 2;
const HANDSHAKE_ERROR_FRAME_ID: u8 = 3;
const DISCONNECT_FRAME_ID: u8 = 4;
const DISCONNECT_ACK_FRAME_ID: u8 = 5;
const DATA_FRAME_ID: u8 = 10;
const SYNC_FRAME_ID: u8 = 11;
const ACK_FRAME_ID: u8 = 12;

const HANDSHAKE_SYN_FRAME_PAYLOAD_SIZE: usize = MAX_FRAME_SIZE - FRAME_OVERHEAD; // Padded to internet MTU
const HANDSHAKE_SYN_ACK_FRAME_PAYLOAD_SIZE: usize = 20;
const HANDSHAKE_ACK_FRAME_PAYLOAD_SIZE: usize = 4;
const HANDSHAKE_ERROR_FRAME_PAYLOAD_SIZE: usize = 5;
const DISCONNECT_FRAME_PAYLOAD_SIZE: usize = 0;
const DISCONNECT_ACK_FRAME_PAYLOAD_SIZE: usize = 0;

const DATAGRAM_HEADER_SIZE_MICRO: usize =  6;
const DATAGRAM_HEADER_SIZE_SMALL: usize =  9;
const DATAGRAM_HEADER_SIZE_LARGE: usize = 14;
const DATAGRAM_HEADER_SIZE_MIN: usize =  DATAGRAM_HEADER_SIZE_MICRO;
pub const MAX_DATAGRAM_OVERHEAD: usize = DATAGRAM_HEADER_SIZE_LARGE;
#[cfg(test)]
pub const MIN_DATAGRAM_OVERHEAD: usize = DATAGRAM_HEADER_SIZE_MICRO;

const DATA_FRAME_PAYLOAD_HEADER_SIZE: usize = 5;
pub const DATA_FRAME_OVERHEAD: usize = FRAME_OVERHEAD + DATA_FRAME_PAYLOAD_HEADER_SIZE;
pub const DATA_FRAME_MAX_DATAGRAM_COUNT: usize = 127;

const SYNC_FRAME_PAYLOAD_SIZE: usize = 9;

pub const ACK_GROUP_SIZE: usize = 9;
const ACK_FRAME_PAYLOAD_HEADER_SIZE: usize = 10;
#[cfg(test)]
pub const ACK_FRAME_OVERHEAD: usize = FRAME_OVERHEAD + ACK_FRAME_PAYLOAD_HEADER_SIZE;

pub const MAX_CHANNELS: usize = 64;
pub const MAX_FRAGMENTS: usize = 1 << 16;

fn read_handshake_syn_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != HANDSHAKE_SYN_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let version = data[0];

    let nonce = ((data[1] as u32) << 24) |
                ((data[2] as u32) << 16) |
                ((data[3] as u32) <<  8) |
                ((data[4] as u32)      );

    let max_receive_rate = ((data[5] as u32) << 24) |
                           ((data[6] as u32) << 16) |
                           ((data[7] as u32) <<  8) |
                           ((data[8] as u32)      );

    let max_packet_size = ((data[9] as u32) << 24) |
                          ((data[10] as u32) << 16) |
                          ((data[11] as u32) <<  8) |
                          ((data[12] as u32)      );

    let max_receive_alloc = ((data[13] as u32) << 24) |
                            ((data[14] as u32) << 16) |
                            ((data[15] as u32) <<  8) |
                            ((data[16] as u32)      );

    Some(Frame::HandshakeSynFrame(HandshakeSynFrame {
        version,
        nonce,
        max_receive_rate,
        max_packet_size,
        max_receive_alloc,
    }))
}

fn read_handshake_syn_ack_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != HANDSHAKE_SYN_ACK_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let nonce_ack = ((data[0] as u32) << 24) |
                    ((data[1] as u32) << 16) |
                    ((data[2] as u32) <<  8) |
                    ((data[3] as u32)      );

    let nonce = ((data[4] as u32) << 24) |
                ((data[5] as u32) << 16) |
                ((data[6] as u32) <<  8) |
                ((data[7] as u32)      );

    let max_receive_rate = ((data[8] as u32) << 24) |
                           ((data[9] as u32) << 16) |
                           ((data[10] as u32) <<  8) |
                           ((data[11] as u32)      );

    let max_packet_size = ((data[12] as u32) << 24) |
                          ((data[13] as u32) << 16) |
                          ((data[14] as u32) <<  8) |
                          ((data[15] as u32)      );

    let max_receive_alloc = ((data[16] as u32) << 24) |
                            ((data[17] as u32) << 16) |
                            ((data[18] as u32) <<  8) |
                            ((data[19] as u32)      );

    Some(Frame::HandshakeSynAckFrame(HandshakeSynAckFrame {
        nonce_ack,
        nonce,
        max_receive_rate,
        max_packet_size,
        max_receive_alloc,
    }))
}

fn read_handshake_ack_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != HANDSHAKE_ACK_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let nonce_ack = ((data[0] as u32) << 24) |
                    ((data[1] as u32) << 16) |
                    ((data[2] as u32) <<  8) |
                    ((data[3] as u32)      );

    Some(Frame::HandshakeAckFrame(HandshakeAckFrame {
        nonce_ack,
    }))
}

fn read_handshake_error_payload(data: &[u8]) -> Option<Frame> {
    if data.len() != HANDSHAKE_ERROR_FRAME_PAYLOAD_SIZE {
        return None;
    }

    let nonce_ack = ((data[0] as u32) << 24) |
                    ((data[1] as u32) << 16) |
                    ((data[2] as u32) <<  8) |
                    ((data[3] as u32)      );

    let error = match data[4] {
        0 => HandshakeErrorType::Version,
        1 => HandshakeErrorType::Config,
        2 => HandshakeErrorType::ServerFull,
        _ => return None,
    };

    Some(Frame::HandshakeErrorFrame(HandshakeErrorFrame {
        nonce_ack,
        error,
    }))
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
    if data.len() < DATAGRAM_HEADER_SIZE_MIN {
        return None;
    }

    if data[0] & 0x80 == 0x00 {
        let header_size = DATAGRAM_HEADER_SIZE_MICRO;

        let data_len = (data[0] & 0x3F) as usize;

        let total_size = header_size + data_len;

        if data.len() < total_size {
            return None;
        }

        // Micro
        let channel_id = ((data[4] >> 2) & 0x20) |
                         ((data[0] >> 2) & 0x10) |
                         ((data[1]     ) & 0x0F);

        let sequence_id = (((data[1] & 0xF0) as u32) << 12) |
                          (((data[2]       ) as u32) <<  8) |
                          (((data[3]       ) as u32)      );

        let window_parent_lead = (data[4] & 0x7F) as u16;

        let channel_parent_lead = data[5] as u16;

        let fragment_id = 0;

        let fragment_id_last = 0;

        let data = data[header_size .. header_size + data_len].into();

        return Some((Datagram {
            channel_id,
            sequence_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id,
            fragment_id_last,
            data,
        }, total_size));
    } else if data[0] & 0x40 == 0x00 {
        // Small
        let header_size = DATAGRAM_HEADER_SIZE_SMALL;

        let data_len = data[1] as usize;

        let total_size = header_size + data_len;

        if data.len() < total_size {
            return None;
        }

        let channel_id = data[0] & 0x3F;

        let sequence_id = (((data[2] & 0x0F) as u32) << 16) |
                          (((data[3]       ) as u32) <<  8) |
                          (((data[4]       ) as u32)      );

        let window_parent_lead = ((data[5] as u16) << 8) |
                                 ((data[6] as u16)     );

        let channel_parent_lead = ((data[7] as u16) << 8) |
                                  ((data[8] as u16)     );

        let fragment_id = 0;

        let fragment_id_last = 0;

        let data = data[header_size .. header_size + data_len].into();

        return Some((Datagram {
            channel_id,
            sequence_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id,
            fragment_id_last,
            data,
        }, total_size));
    } else {
        // Large
        let header_size = DATAGRAM_HEADER_SIZE_LARGE;

        let data_len = ((data[1] as usize) << 8) |
                       ((data[2] as usize)     );

        let total_size = header_size + data_len;

        if data.len() < total_size {
            return None;
        }

        let channel_id = data[0] & 0x3F;

        let sequence_id = (((data[3] & 0x0F) as u32) << 16) |
                          (((data[4]       ) as u32) <<  8) |
                          (((data[5]       ) as u32)      );

        let window_parent_lead = ((data[6] as u16) << 8) |
                                 ((data[7] as u16)     );

        let channel_parent_lead = ((data[8] as u16) << 8) |
                                  ((data[9] as u16)     );

        let fragment_id = ((data[10] as u16) << 8) |
                          ((data[11] as u16)     );

        let fragment_id_last = ((data[12] as u16) << 8) |
                               ((data[13] as u16)     );

        let data = data[header_size .. header_size + data_len].into();

        return Some((Datagram {
            channel_id,
            sequence_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id,
            fragment_id_last,
            data,
        }, total_size));
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

    let nonce = data[4] & 0x80 != 0x00;

    let datagram_num = data[4] & 0x7F;

    let mut data_slice = &data[DATA_FRAME_PAYLOAD_HEADER_SIZE ..];
    let mut datagrams = Vec::new();

    for _ in 0 .. datagram_num {
        if let Some((datagram, read_size)) = read_datagram(data_slice) {
            datagrams.push(datagram);
            data_slice = &data_slice[read_size ..];
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

    let mode = data[0];

    let next_frame_id = if mode & 0x01 != 0x00 {
        Some(((data[1] as u32) << 24) |
             ((data[2] as u32) << 16) |
             ((data[3] as u32) <<  8) |
             ((data[4] as u32)      ))
    } else {
        None
    };

    let next_packet_id = if mode & 0x02 != 0x00 {
        Some(((data[5] as u32) << 24) |
             ((data[6] as u32) << 16) |
             ((data[7] as u32) <<  8) |
             ((data[8] as u32)      ))
    } else {
        None
    };

    Some(Frame::SyncFrame(SyncFrame { next_frame_id, next_packet_id }))
}

fn read_frame_ack(data: &[u8]) -> Option<(AckGroup, usize)> {
    // Ack
    if data.len() < ACK_GROUP_SIZE {
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

    return Some((AckGroup {
        base_id,
        bitfield,
        nonce,
    }, ACK_GROUP_SIZE));
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

    let mut data_slice = &data[ACK_FRAME_PAYLOAD_HEADER_SIZE ..];
    let mut frame_acks = Vec::new();

    for _ in 0 .. frame_ack_num {
        if let Some((frame_ack, read_size)) = read_frame_ack(data_slice) {
            frame_acks.push(frame_ack);
            data_slice = &data_slice[read_size ..];
        } else {
            return None;
        }
    }

    if data_slice.len() != 0 {
        return None;
    }

    Some(Frame::AckFrame(AckFrame { frame_window_base_id, packet_window_base_id, frame_acks }))
}


fn write_handshake_syn(frame: &HandshakeSynFrame) -> Box<[u8]> {
    let mut frame_bytes = Box::new([0; MAX_FRAME_SIZE]);

    let non_padding_bytes = [
        HANDSHAKE_SYN_FRAME_ID,
        frame.version,
        (frame.nonce >> 24) as u8,
        (frame.nonce >> 16) as u8,
        (frame.nonce >>  8) as u8,
        (frame.nonce      ) as u8,
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
    ];

    frame_bytes[.. non_padding_bytes.len()].clone_from_slice(&non_padding_bytes);

    let frame_len = frame_bytes.len();
    let data_bytes = &frame_bytes[0 .. frame_len - 4];

    let crc = crc::compute(&data_bytes);
    frame_bytes[frame_len - 4] = (crc >> 24) as u8;
    frame_bytes[frame_len - 3] = (crc >> 16) as u8;
    frame_bytes[frame_len - 2] = (crc >>  8) as u8;
    frame_bytes[frame_len - 1] = (crc      ) as u8;

    return frame_bytes;
}

fn write_handshake_syn_ack(frame: &HandshakeSynAckFrame) -> Box<[u8]> {
    let mut frame_bytes = Box::new([
        HANDSHAKE_SYN_ACK_FRAME_ID,
        (frame.nonce_ack >> 24) as u8,
        (frame.nonce_ack >> 16) as u8,
        (frame.nonce_ack >>  8) as u8,
        (frame.nonce_ack      ) as u8,
        (frame.nonce >> 24) as u8,
        (frame.nonce >> 16) as u8,
        (frame.nonce >>  8) as u8,
        (frame.nonce      ) as u8,
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
        0,
        0,
        0,
        0,
    ]);

    let frame_len = frame_bytes.len();
    let data_bytes = &frame_bytes[0 .. frame_len - 4];

    let crc = crc::compute(&data_bytes);
    frame_bytes[frame_len - 4] = (crc >> 24) as u8;
    frame_bytes[frame_len - 3] = (crc >> 16) as u8;
    frame_bytes[frame_len - 2] = (crc >>  8) as u8;
    frame_bytes[frame_len - 1] = (crc      ) as u8;

    return frame_bytes;
}

fn write_handshake_ack(frame: &HandshakeAckFrame) -> Box<[u8]> {
    let mut frame_bytes = Box::new([
        HANDSHAKE_ACK_FRAME_ID,
        (frame.nonce_ack >> 24) as u8,
        (frame.nonce_ack >> 16) as u8,
        (frame.nonce_ack >>  8) as u8,
        (frame.nonce_ack      ) as u8,
        0,
        0,
        0,
        0,
    ]);

    let frame_len = frame_bytes.len();
    let data_bytes = &frame_bytes[0 .. frame_len - 4];

    let crc = crc::compute(&data_bytes);
    frame_bytes[frame_len - 4] = (crc >> 24) as u8;
    frame_bytes[frame_len - 3] = (crc >> 16) as u8;
    frame_bytes[frame_len - 2] = (crc >>  8) as u8;
    frame_bytes[frame_len - 1] = (crc      ) as u8;

    return frame_bytes;
}

fn write_handshake_error(frame: &HandshakeErrorFrame) -> Box<[u8]> {
    let mut frame_bytes = Box::new([
        HANDSHAKE_ERROR_FRAME_ID,
        (frame.nonce_ack >> 24) as u8,
        (frame.nonce_ack >> 16) as u8,
        (frame.nonce_ack >>  8) as u8,
        (frame.nonce_ack      ) as u8,
        match frame.error {
            HandshakeErrorType::Version => 0,
            HandshakeErrorType::Config => 1,
            HandshakeErrorType::ServerFull => 2,
        },
        0,
        0,
        0,
        0,
    ]);

    let frame_len = frame_bytes.len();
    let data_bytes = &frame_bytes[0 .. frame_len - 4];

    let crc = crc::compute(&data_bytes);
    frame_bytes[frame_len - 4] = (crc >> 24) as u8;
    frame_bytes[frame_len - 3] = (crc >> 16) as u8;
    frame_bytes[frame_len - 2] = (crc >>  8) as u8;
    frame_bytes[frame_len - 1] = (crc      ) as u8;

    return frame_bytes;
}

fn write_disconnect(_frame: &DisconnectFrame) -> Box<[u8]> {
    let mut frame_bytes = Box::new([
        DISCONNECT_FRAME_ID,
        0,
        0,
        0,
        0,
    ]);

    let frame_len = frame_bytes.len();
    let data_bytes = &frame_bytes[0 .. frame_len - 4];

    let crc = crc::compute(&data_bytes);
    frame_bytes[frame_len - 4] = (crc >> 24) as u8;
    frame_bytes[frame_len - 3] = (crc >> 16) as u8;
    frame_bytes[frame_len - 2] = (crc >>  8) as u8;
    frame_bytes[frame_len - 1] = (crc      ) as u8;

    return frame_bytes;
}

fn write_disconnect_ack(_frame: &DisconnectAckFrame) -> Box<[u8]> {
    let mut frame_bytes = Box::new([
        DISCONNECT_ACK_FRAME_ID,
        0,
        0,
        0,
        0,
    ]);

    let frame_len = frame_bytes.len();
    let data_bytes = &frame_bytes[0 .. frame_len - 4];

    let crc = crc::compute(&data_bytes);
    frame_bytes[frame_len - 4] = (crc >> 24) as u8;
    frame_bytes[frame_len - 3] = (crc >> 16) as u8;
    frame_bytes[frame_len - 2] = (crc >>  8) as u8;
    frame_bytes[frame_len - 1] = (crc      ) as u8;

    return frame_bytes;
}

fn write_data(frame: &DataFrame) -> Box<[u8]> {
    let mut builder = build::DataFrameBuilder::new(frame.sequence_id, frame.nonce);

    for datagram in frame.datagrams.iter() {
        builder.add(&datagram.into());
    }

    builder.build()
}

fn write_sync(frame: &SyncFrame) -> Box<[u8]> {
    let mode = ((frame.next_frame_id.is_some() as u8) << 0) |
               ((frame.next_packet_id.is_some() as u8) << 1);

    let next_frame_id = frame.next_frame_id.unwrap_or(0);
    let next_packet_id = frame.next_packet_id.unwrap_or(0);

    let mut frame_bytes = Box::new([
        SYNC_FRAME_ID,
        mode,
        (next_frame_id >> 24) as u8,
        (next_frame_id >> 16) as u8,
        (next_frame_id >>  8) as u8,
        (next_frame_id      ) as u8,
        (next_packet_id >> 24) as u8,
        (next_packet_id >> 16) as u8,
        (next_packet_id >>  8) as u8,
        (next_packet_id      ) as u8,
        0,
        0,
        0,
        0,
    ]);

    let frame_len = frame_bytes.len();
    let data_bytes = &frame_bytes[0 .. frame_len - 4];

    let crc = crc::compute(&data_bytes);
    frame_bytes[frame_len - 4] = (crc >> 24) as u8;
    frame_bytes[frame_len - 3] = (crc >> 16) as u8;
    frame_bytes[frame_len - 2] = (crc >>  8) as u8;
    frame_bytes[frame_len - 1] = (crc      ) as u8;

    return frame_bytes;
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
    fn read(frame_bytes: &[u8]) -> Option<Self> {
        if frame_bytes.len() < 5 {
            return None;
        }

        let frame_len = frame_bytes.len();
        let data_bytes = &frame_bytes[0 .. frame_len - 4];

        let crc = ((frame_bytes[frame_len - 4] as u32) << 24) |
                  ((frame_bytes[frame_len - 3] as u32) << 16) |
                  ((frame_bytes[frame_len - 2] as u32) <<  8) |
                  ((frame_bytes[frame_len - 1] as u32)      );

        if crc::compute(&data_bytes) != crc {
            return None;
        }

        let payload_bytes = &frame_bytes[1 .. frame_len - 4];

        match frame_bytes[0] {
            HANDSHAKE_SYN_FRAME_ID => read_handshake_syn_payload(payload_bytes),
            HANDSHAKE_SYN_ACK_FRAME_ID => read_handshake_syn_ack_payload(payload_bytes),
            HANDSHAKE_ACK_FRAME_ID => read_handshake_ack_payload(payload_bytes),
            HANDSHAKE_ERROR_FRAME_ID => read_handshake_error_payload(payload_bytes),
            DISCONNECT_FRAME_ID => read_disconnect_payload(payload_bytes),
            DISCONNECT_ACK_FRAME_ID => read_disconnect_ack_payload(payload_bytes),
            DATA_FRAME_ID => read_data_payload(payload_bytes),
            SYNC_FRAME_ID => read_sync_payload(payload_bytes),
            ACK_FRAME_ID => read_ack_payload(payload_bytes),
            _ => None,
        }
    }

    fn write(&self) -> Box<[u8]> {
        match self {
            Frame::HandshakeSynFrame(frame) => write_handshake_syn(frame),
            Frame::HandshakeSynAckFrame(frame) => write_handshake_syn_ack(frame),
            Frame::HandshakeAckFrame(frame) => write_handshake_ack(frame),
            Frame::HandshakeErrorFrame(frame) => write_handshake_error(frame),
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
        println!("frame: {:#?}", f);

        let bytes = f.write();
        println!("frame bytes: {:02X?}", bytes);

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

        for i in 1 .. bytes.len() {
            let bytes_trunc = &bytes[0 .. i];
            assert_eq!(Frame::read(&bytes_trunc), None);
        }
    }

    #[test]
    fn handshake_syn_basic() {
        let f = Frame::HandshakeSynFrame(HandshakeSynFrame {
            version: 0x7F,
            nonce: 0x18273645,
            max_receive_rate: 0x98765432,
            max_packet_size: 0x01234567,
            max_receive_alloc: 0xABCDEF01,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn handshake_syn_ack_basic() {
        let f = Frame::HandshakeSynAckFrame(HandshakeSynAckFrame {
            nonce_ack: 0x03246387,
            nonce: 0x18273645,
            max_receive_rate: 0x98765432,
            max_packet_size: 0x01234567,
            max_receive_alloc: 0xABCDEF01,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn handshake_ack_basic() {
        let f = Frame::HandshakeAckFrame(HandshakeAckFrame {
            nonce_ack: 0x03246387,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
    }

    #[test]
    fn handshake_error_basic() {
        let f = Frame::HandshakeErrorFrame(HandshakeErrorFrame {
            nonce_ack: 0x03246387,
            error: HandshakeErrorType::ServerFull,
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
        let small_data = (0 .. 256).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();

        let f = Frame::DataFrame(DataFrame {
            sequence_id: 0x010203,
            nonce: true,
            datagrams: vec![
                Datagram {
                    sequence_id: 0x45678,
                    channel_id: 63,
                    window_parent_lead: 0x34A8,
                    channel_parent_lead: 0x8A43,
                    fragment_id: 0x4789,
                    fragment_id_last: 0x478A,
                    data: vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(),
                },
                Datagram {
                    sequence_id: 0x12345,
                    channel_id: 63,
                    window_parent_lead: 0x34A8,
                    channel_parent_lead: 0x8A43,
                    fragment_id: 0,
                    fragment_id_last: 0,
                    data: small_data,
                },
                Datagram {
                    sequence_id: 0x12345,
                    channel_id: 63,
                    window_parent_lead: 0x34A8,
                    channel_parent_lead: 0x8A43,
                    fragment_id: 0,
                    fragment_id_last: 0,
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
            next_frame_id: Some(0x01020304),
            next_packet_id: None,
        });
        verify_consistent(&f);
        verify_extra_bytes_fail(&f);
        verify_truncation_fails(&f);
        let f = Frame::SyncFrame(SyncFrame {
            next_frame_id: None,
            next_packet_id: Some(0x05060708),
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
                AckGroup {
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

    fn random_data(min: usize, max: usize) -> Box<[u8]> {
        let len = rand::random::<usize>() % (max - min + 1);
        (0 .. len).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice()
    }

    fn random_data_frame() -> Frame {
        use crate::packet_id;

        const MAX_DATAGRAMS: usize = 64;
        const MAX_DATA_SIZE: usize = 100;

        let mut datagrams = Vec::new();

        for _ in 0 .. rand::random::<usize>() % MAX_DATAGRAMS {
            let datagram = match rand::random::<u32>() % 3 {
                0 => Datagram {
                    // Micro
                    sequence_id: rand::random::<u32>() & packet_id::MASK,
                    channel_id: (rand::random::<usize>() % MAX_CHANNELS) as u8,
                    window_parent_lead: rand::random::<u16>() % 128,
                    channel_parent_lead: rand::random::<u16>() % 256,
                    fragment_id: 0,
                    fragment_id_last: 0,
                    data: random_data(0, 64),
                },
                1 => Datagram {
                    // Small
                    sequence_id: rand::random::<u32>() & packet_id::MASK,
                    channel_id: (rand::random::<usize>() % MAX_CHANNELS) as u8,
                    window_parent_lead: rand::random::<u16>(),
                    channel_parent_lead: rand::random::<u16>(),
                    fragment_id: 0,
                    fragment_id_last: 0,
                    data: random_data(64, MAX_DATA_SIZE),
                },
                2 => {
                    let mut fragment_id_last = rand::random::<u16>();
                    let mut fragment_id = rand::random::<u16>();

                    if fragment_id > fragment_id_last {
                        std::mem::swap(&mut fragment_id, &mut fragment_id_last);
                    }

                    Datagram {
                        // Large
                        sequence_id: rand::random::<u32>() & packet_id::MASK,
                        channel_id: (rand::random::<usize>() % MAX_CHANNELS) as u8,
                        window_parent_lead: rand::random::<u16>(),
                        channel_parent_lead: rand::random::<u16>(),
                        fragment_id,
                        fragment_id_last,
                        data: random_data(0, MAX_DATA_SIZE),
                    }
                }
                _ => panic!()
            };

            datagrams.push(datagram);
        }

        return Frame::DataFrame(DataFrame {
            sequence_id: rand::random::<u32>(),
            nonce: rand::random::<bool>(),
            datagrams: datagrams,
        });
    }

    // TODO: Random handshake/disconnect tests

    #[test]
    fn data_random() {
        const NUM_ROUNDS: usize = 100;

        for _ in 0 .. NUM_ROUNDS {
            let f = random_data_frame();

            verify_consistent(&f);
            verify_extra_bytes_fail(&f);
            verify_truncation_fails(&f);
        }
    }

    #[test]
    fn sync_random() {
        const NUM_ROUNDS: usize = 100;

        for _ in 0 .. NUM_ROUNDS {
            let f = Frame::SyncFrame(SyncFrame {
                next_frame_id: if rand::random::<u32>() % 5 != 0 { Some(rand::random::<u32>()) } else { None },
                next_packet_id: if rand::random::<u32>() % 5 != 0 { Some(rand::random::<u32>()) } else { None },
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

        for _ in 0 .. NUM_ROUNDS {
            let mut frame_acks = Vec::new();

            for _ in 0 .. rand::random::<usize>() % MAX_FRAME_ACKS {
                let frame_ack = AckGroup {
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

    // In practice this has yet to fail for any number of flips. May be rarer than 1 in 2^32, lol.
    #[test]
    fn crc_flips() {
        const NUM_ROUNDS: usize = 10000;

        // The alleged strength of the chosen CRC
        const NUM_FLIPS: usize = 5;
        const MAX_LEN: usize = 8192;

        for _ in 0 .. NUM_ROUNDS {
            let f = random_data_frame();
            let mut frame_bytes = f.write();

            assert!(frame_bytes.len() <= MAX_LEN);

            for _ in 0 .. NUM_FLIPS {
                let bit_index = rand::random::<usize>() % (frame_bytes.len() * 8);

                let bit = bit_index % 8;
                let byte_index = bit_index / 8;

                frame_bytes[byte_index] ^= 0x01 << bit;
            }

            assert_eq!(Frame::read(&frame_bytes), None);
        }
    }
}

