
use super::DatagramRef;
use super::AckGroup;

use super::DATA_FRAME_ID;
use super::ACK_FRAME_ID;

use super::DATAGRAM_HEADER_SIZE_FRAGMENT;
use super::DATAGRAM_HEADER_SIZE_FULL;

use super::FRAME_ACK_SIZE;

pub const MAX_CHANNELS: usize = 64;

pub struct DataFrameBuilder {
    buffer: Vec<u8>,
    count: u16,
}

impl DataFrameBuilder {
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
        let data_len = datagram.data.len();
        let data_len_u16 = data_len as u32;

        debug_assert!(data_len <= u32::MAX as usize);
        debug_assert!((datagram.channel_id as usize) < MAX_CHANNELS);

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
        self.buffer.into_boxed_slice()
    }

    pub fn size(&self) -> usize {
        self.buffer.len()
    }

    pub fn encoded_size_ref(datagram: &DatagramRef) -> usize {
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
        self.buffer.into_boxed_slice()
    }

    pub fn size(&self) -> usize {
        self.buffer.len()
    }

    pub fn count(&self) -> u16 {
        self.count
    }

    pub fn encoded_size(_frame_ack: &AckGroup) -> usize {
        FRAME_ACK_SIZE
    }
}

