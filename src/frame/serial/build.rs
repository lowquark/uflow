
use super::DatagramRef;
use super::AckGroup;

use super::DATA_FRAME_ID;
use super::DATA_FRAME_OVERHEAD;
use super::DATAGRAM_HEADER_SIZE_FRAGMENT;
use super::DATAGRAM_HEADER_SIZE_FULL;

use super::ACK_FRAME_ID;
use super::ACK_FRAME_OVERHEAD;
use super::ACK_GROUP_SIZE;

use super::FRAME_CRC_SIZE;
use super::MAX_CHANNELS;

use super::crc;

// Consider:
//
// * Max 64 packets per frame
//
// * Max 8192 frames per transfer window (16384 valid frames in a receive window)
//
// * 20-bit sequence ID
//      At 64 packets per frame, and with a receive window of size 16384, this identifies
//      packets unambiguously
//
// * Fragment bit
//      If set, fragment ID is 0/0
//      If unset, fragment ID is present
//
// * 15-bit shrinking values
//      If top bit of first byte is unset, one byte (7 bits)
//      If top bit of first byte is set, two bytes (15 bits)
//
// Min overhead:  7 bytes
// Max overhead: 14 bytes

pub struct DataFrameBuilder {
    buffer: Vec<u8>,
    count: u16,
}

impl DataFrameBuilder {
    pub const INITIAL_SIZE: usize = DATA_FRAME_OVERHEAD;

    pub fn new(sequence_id: u32, nonce: bool) -> Self {
        let header = vec![
            DATA_FRAME_ID,
            (sequence_id >> 24) as u8,
            (sequence_id >> 16) as u8,
            (sequence_id >>  8) as u8,
            (sequence_id      ) as u8,
            nonce as u8,
            0,
            0
        ];

        Self {
            buffer: header,
            count: 0,
        }
    }

    pub fn add(&mut self, datagram: &DatagramRef) {
        debug_assert!((datagram.channel_id as usize) < MAX_CHANNELS);
        debug_assert!(datagram.data.len() <= u16::MAX as usize);

        let data_len_u16 = datagram.data.len() as u16;

        if datagram.fragment_id.id == 0 && datagram.fragment_id.last == 0 {
            let header = [
                datagram.channel_id,
                (datagram.sequence_id >> 24) as u8,
                (datagram.sequence_id >> 16) as u8,
                (datagram.sequence_id >>  8) as u8,
                (datagram.sequence_id      ) as u8,
                (datagram.window_parent_lead >> 8) as u8,
                (datagram.window_parent_lead     ) as u8,
                (datagram.channel_parent_lead >> 8) as u8,
                (datagram.channel_parent_lead     ) as u8,
                (data_len_u16 >> 8) as u8,
                (data_len_u16     ) as u8,
            ];

            self.buffer.extend_from_slice(&header);
        } else {
            let header = [
                datagram.channel_id | 0x80,
                (datagram.sequence_id >> 24) as u8,
                (datagram.sequence_id >> 16) as u8,
                (datagram.sequence_id >>  8) as u8,
                (datagram.sequence_id      ) as u8,
                (datagram.window_parent_lead >> 8) as u8,
                (datagram.window_parent_lead     ) as u8,
                (datagram.channel_parent_lead >> 8) as u8,
                (datagram.channel_parent_lead     ) as u8,
                (datagram.fragment_id.last >> 8) as u8,
                (datagram.fragment_id.last     ) as u8,
                (datagram.fragment_id.id >> 8) as u8,
                (datagram.fragment_id.id     ) as u8,
                (data_len_u16 >> 8) as u8,
                (data_len_u16     ) as u8,
            ];

            self.buffer.extend_from_slice(&header);
        }

        self.buffer.extend_from_slice(&datagram.data);
        self.count += 1;
    }

    pub fn build(mut self) -> Box<[u8]> {
        let count_offset_0 = 6;
        let count_offset_1 = 7;
        self.buffer[count_offset_0] = (self.count >> 8) as u8;
        self.buffer[count_offset_1] = (self.count     ) as u8;

        let data_bytes = self.buffer.as_slice();
        let crc = crc::compute(&data_bytes);

        self.buffer.extend_from_slice(&[
            (crc >> 24) as u8,
            (crc >> 16) as u8,
            (crc >>  8) as u8,
            (crc      ) as u8,
        ]);

        self.buffer.into_boxed_slice()
    }

    pub fn size(&self) -> usize {
        self.buffer.len() + FRAME_CRC_SIZE
    }

    pub fn encoded_size(datagram: &DatagramRef) -> usize {
        if datagram.fragment_id.id == 0 && datagram.fragment_id.last == 0 {
            DATAGRAM_HEADER_SIZE_FULL + datagram.data.len()
        } else {
            DATAGRAM_HEADER_SIZE_FRAGMENT + datagram.data.len()
        }
    }
}

pub struct AckFrameBuilder {
    buffer: Vec<u8>,
    count: u16,
}

impl AckFrameBuilder {
    pub const INITIAL_SIZE: usize = ACK_FRAME_OVERHEAD;

    pub fn new(frame_window_base_id: u32, packet_window_base_id: u32) -> Self {
        let header = vec![
            ACK_FRAME_ID,
            (frame_window_base_id >> 24) as u8,
            (frame_window_base_id >> 16) as u8,
            (frame_window_base_id >>  8) as u8,
            (frame_window_base_id      ) as u8,
            (packet_window_base_id >> 24) as u8,
            (packet_window_base_id >> 16) as u8,
            (packet_window_base_id >>  8) as u8,
            (packet_window_base_id      ) as u8,
            0,
            0
        ];

        Self {
            buffer: header,
            count: 0,
        }
    }

    pub fn add(&mut self, frame_ack: &AckGroup) {
        let header = [
            (frame_ack.base_id >> 24) as u8,
            (frame_ack.base_id >> 16) as u8,
            (frame_ack.base_id >>  8) as u8,
            (frame_ack.base_id      ) as u8,
            (frame_ack.bitfield >> 24) as u8,
            (frame_ack.bitfield >> 16) as u8,
            (frame_ack.bitfield >>  8) as u8,
            (frame_ack.bitfield      ) as u8,
            frame_ack.nonce as u8,
        ];

        self.buffer.extend_from_slice(&header);
        self.count += 1;
    }

    pub fn build(mut self) -> Box<[u8]> {
        let count_offset_0 = 9;
        let count_offset_1 = 10;
        self.buffer[count_offset_0] = (self.count >> 8) as u8;
        self.buffer[count_offset_1] = (self.count     ) as u8;

        let data_bytes = self.buffer.as_slice();
        let crc = crc::compute(&data_bytes);

        self.buffer.extend_from_slice(&[
            (crc >> 24) as u8,
            (crc >> 16) as u8,
            (crc >>  8) as u8,
            (crc      ) as u8,
        ]);

        self.buffer.into_boxed_slice()
    }

    pub fn size(&self) -> usize {
        self.buffer.len() + FRAME_CRC_SIZE
    }

    pub fn encoded_size(_frame_ack: &AckGroup) -> usize {
        ACK_GROUP_SIZE
    }
}

