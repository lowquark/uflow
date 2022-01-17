
use crate::frame;

pub struct FrameAckQueue {
    entries: std::collections::VecDeque<frame::FrameAck>,
}

impl FrameAckQueue {
    pub fn new() -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
        }
    }

    pub fn mark_seen(&mut self, frame_id: u32, nonce: bool) {
        if let Some(last_entry) = self.entries.back_mut() {
            let bit = frame_id.wrapping_sub(last_entry.base_id);
            if bit < 32 {
                if last_entry.bitfield & (0x00000001 << bit) == 0 {
                    last_entry.bitfield |= 0x00000001 << bit;
                    last_entry.nonce ^= nonce;
                }
            } else {
                self.entries.push_back(frame::FrameAck {
                    base_id: frame_id,
                    bitfield: 0x00000001,
                    nonce: nonce,
                });
            }
        } else {
            self.entries.push_back(frame::FrameAck {
                base_id: frame_id,
                bitfield: 0x00000001,
                nonce: nonce,
            });
        }
    }

    pub fn pop(&mut self) -> Option<frame::FrameAck> {
        if let Some(first_entry) = self.entries.pop_front() {
            debug_assert!(first_entry.bitfield & 0x00000001 != 0);

            return Some(first_entry);
        }

        return None;
    }

    pub fn peek(&self) -> Option<&frame::FrameAck> {
        self.entries.front()
    }
}

