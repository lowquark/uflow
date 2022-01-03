
use super::Datagram;
use super::Ack;
use super::Resync;
use super::Message;

use super::MESSAGE_FRAME_ID;

use super::DATAGRAM_MESSAGE_HEADER_SIZE_FRAGMENT;
use super::DATAGRAM_MESSAGE_HEADER_SIZE_FULL;
use super::ACK_MESSAGE_SIZE;
use super::RESYNC_MESSAGE_SIZE;

pub const MAX_CHANNELS: usize = 64;

pub struct MessageFrameBuilder {
    buffer: Vec<u8>,
    count: u16,
}

impl MessageFrameBuilder {
    pub fn new(sequence_id: u32, nonce: bool) -> Self {
        let header = vec![
            MESSAGE_FRAME_ID,
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

    fn add_datagram(&mut self, datagram: &Datagram) {
        let data_len = datagram.data.len();
        let data_len_u16 = data_len as u32;

        debug_assert!(data_len <= u32::MAX as usize);
        debug_assert!((datagram.channel_id as usize) < MAX_CHANNELS);

        if datagram.fragment_id.id == 0 && datagram.fragment_id.last == 0 {
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
                (data_len_u16 >> 8) as u8,
                (data_len_u16     ) as u8,
            ];

            self.buffer.extend_from_slice(&header);
        } else {
            let header = [
                datagram.channel_id | 0xC0,
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

    fn add_ack(&mut self, ack: &Ack) {
        let header = [
            ack.frames.nonce as u8,
            (ack.frames.base_id >> 24) as u8,
            (ack.frames.base_id >> 16) as u8,
            (ack.frames.base_id >>  8) as u8,
            (ack.frames.base_id      ) as u8,
            (ack.frames.bitfield >> 24) as u8,
            (ack.frames.bitfield >> 16) as u8,
            (ack.frames.bitfield >>  8) as u8,
            (ack.frames.bitfield      ) as u8,
            (ack.receiver_base_id >> 24) as u8,
            (ack.receiver_base_id >> 16) as u8,
            (ack.receiver_base_id >>  8) as u8,
            (ack.receiver_base_id      ) as u8,
        ];

        self.buffer.extend_from_slice(&header);
        self.count += 1;
    }

    fn add_resync(&mut self, resync: &Resync) {
        let header = [
            0x40,
            (resync.sender_next_id >> 24) as u8,
            (resync.sender_next_id >> 16) as u8,
            (resync.sender_next_id >>  8) as u8,
            (resync.sender_next_id      ) as u8,
        ];

        self.buffer.extend_from_slice(&header);
        self.count += 1;
    }

    pub fn add(&mut self, message: &Message) {
        match message {
            Message::Datagram(datagram) => self.add_datagram(datagram),
            Message::Ack(ack) => self.add_ack(ack),
            Message::Resync(resync) => self.add_resync(resync),
        }
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

    pub fn count(&self) -> u16 {
        self.count
    }

    pub fn message_size(message: &Message) -> usize {
        match message {
            Message::Datagram(datagram) => {
                if datagram.fragment_id.id == 0 && datagram.fragment_id.last == 0 {
                    DATAGRAM_MESSAGE_HEADER_SIZE_FULL + datagram.data.len()
                } else {
                    DATAGRAM_MESSAGE_HEADER_SIZE_FRAGMENT + datagram.data.len()
                }
            }
            Message::Ack(_) => ACK_MESSAGE_SIZE,
            Message::Resync(_) => RESYNC_MESSAGE_SIZE,
        }
    }
}

