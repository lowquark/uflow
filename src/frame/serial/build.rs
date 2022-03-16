
use super::DatagramRef;
use super::AckGroup;

use super::DATA_FRAME_ID;
use super::DATA_FRAME_MAX_DATAGRAM_COUNT;
use super::DATA_FRAME_OVERHEAD;
use super::DATAGRAM_HEADER_SIZE_MICRO;
use super::DATAGRAM_HEADER_SIZE_SMALL;
use super::DATAGRAM_HEADER_SIZE_LARGE;

use super::ACK_FRAME_ID;
use super::ACK_FRAME_OVERHEAD;
use super::ACK_GROUP_SIZE;

use super::FRAME_CRC_SIZE;
use super::MAX_CHANNELS;

use super::crc;

use crate::packet_id;

// If we allow a maximum of 64 packets per frame, and 8192 frames per transfer window, then there
// are 16384 frames in the receive window, and a 20-bit sequence ID is sufficient to ensure no
// packets are ambiguous within the receive window.

// C: Channel ID          [0, 2^6)
// S: Sequence ID         [0, 2^20)
// D: Payload length      [0, 2^16)
// W: Window parent lead  [0, 2^16)
// H: Channel parent lead [0, 2^16)
// F: Fragment ID         [0, 2^16)
// L: Last fragment ID    [0, 2^16)

// L == 0 => F == 0

// If L == 0 && D < 64 && W < 128 && H < 256:
//   Micro header (6 bytes)
//   0CDDDDDD  SSSSCCCC  SSSSSSSS  SSSSSSSS  CWWWWWWW  HHHHHHHH

// Else if L == 0 && D < 256 && L == 0:
//   Small header (9 bytes)
//   10CCCCCC  DDDDDDDD  0000SSSS  SSSSSSSS  SSSSSSSS  WWWWWWWW  WWWWWWWW  HHHHHHHH  HHHHHHHH

// Else:
//   Large header (14 bytes)
//   11CCCCCC  DDDDDDDD  DDDDDDDD  0000SSSS  SSSSSSSS  SSSSSSSS  WWWWWWWW  WWWWWWWW  HHHHHHHH  HHHHHHHH  FFFFFFFF  FFFFFFFF  LLLLLLLL  LLLLLLLL

pub struct DataFrameBuilder {
    buffer: Vec<u8>,
    count: usize,
}

impl DataFrameBuilder {
    pub const INITIAL_SIZE: usize = DATA_FRAME_OVERHEAD;
    pub const MAX_COUNT: usize = DATA_FRAME_MAX_DATAGRAM_COUNT;

    pub fn new(sequence_id: u32, nonce: bool) -> Self {
        // TODO: Could also place nonce + 6-bit length in header byte

        let header = vec![
            DATA_FRAME_ID,
            (sequence_id >> 24) as u8,
            (sequence_id >> 16) as u8,
            (sequence_id >>  8) as u8,
            (sequence_id      ) as u8,
            (nonce as u8) << 7,
        ];

        Self {
            buffer: header,
            count: 0,
        }
    }

    pub fn add(&mut self, datagram: &DatagramRef) {
        debug_assert!((datagram.channel_id as usize) < MAX_CHANNELS);
        debug_assert!(packet_id::is_valid(datagram.sequence_id));
        debug_assert!(datagram.data.len() <= u16::MAX as usize);
        debug_assert!(self.count < DATA_FRAME_MAX_DATAGRAM_COUNT);

        let data_len_u16 = datagram.data.len() as u16;

        if datagram.fragment_id.last == 0 {
            debug_assert!(datagram.fragment_id.id == 0);

            if data_len_u16 < 64 && datagram.window_parent_lead < 128 && datagram.channel_parent_lead < 256 {
                // Micro
                let header = [
                    data_len_u16 as u8 | (datagram.channel_id & 0x10) << 2,
                    (datagram.sequence_id >> 12) as u8 & 0xF0 | datagram.channel_id & 0x0F,
                    (datagram.sequence_id >>  8) as u8,
                    (datagram.sequence_id      ) as u8,
                    datagram.window_parent_lead as u8 | (datagram.channel_id & 0x20) << 2,
                    datagram.channel_parent_lead as u8,
                ];

                self.buffer.extend_from_slice(&header);
                self.buffer.extend_from_slice(&datagram.data);
                self.count += 1;

                return;
            } else if data_len_u16 < 256 {
                // Small
                let header = [
                    datagram.channel_id | 0x80,
                    data_len_u16 as u8,
                    (datagram.sequence_id >> 16) as u8,
                    (datagram.sequence_id >>  8) as u8,
                    (datagram.sequence_id      ) as u8,
                    (datagram.window_parent_lead >> 8) as u8,
                    (datagram.window_parent_lead     ) as u8,
                    (datagram.channel_parent_lead >> 8) as u8,
                    (datagram.channel_parent_lead     ) as u8,
                ];

                self.buffer.extend_from_slice(&header);
                self.buffer.extend_from_slice(&datagram.data);
                self.count += 1;

                return;
            }
        }

        // Large
        let header = [
            datagram.channel_id | 0xC0,
            (data_len_u16 >> 8) as u8,
            (data_len_u16     ) as u8,
            (datagram.sequence_id >> 16) as u8,
            (datagram.sequence_id >>  8) as u8,
            (datagram.sequence_id      ) as u8,
            (datagram.window_parent_lead >> 8) as u8,
            (datagram.window_parent_lead     ) as u8,
            (datagram.channel_parent_lead >> 8) as u8,
            (datagram.channel_parent_lead     ) as u8,
            (datagram.fragment_id.id >> 8) as u8,
            (datagram.fragment_id.id     ) as u8,
            (datagram.fragment_id.last >> 8) as u8,
            (datagram.fragment_id.last     ) as u8,
        ];

        self.buffer.extend_from_slice(&header);
        self.buffer.extend_from_slice(&datagram.data);
        self.count += 1;
    }

    pub fn build(mut self) -> Box<[u8]> {
        debug_assert!(self.count <= DATA_FRAME_MAX_DATAGRAM_COUNT);

        let nack_count_offset = 5;
        self.buffer[nack_count_offset] |= self.count as u8;

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

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn size(&self) -> usize {
        self.buffer.len() + FRAME_CRC_SIZE
    }

    pub fn encoded_size(datagram: &DatagramRef) -> usize {
        let data_len = datagram.data.len();

        if datagram.fragment_id.last == 0 {
            if data_len < 64 && datagram.window_parent_lead < 128 && datagram.channel_parent_lead < 256 {
                return DATAGRAM_HEADER_SIZE_MICRO + data_len;
            } else if data_len < 256 {
                return DATAGRAM_HEADER_SIZE_SMALL + data_len;
            }
        }
        return DATAGRAM_HEADER_SIZE_LARGE + data_len;
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

