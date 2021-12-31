
use crate::frame::FrameAck;

struct Entry {
    base_id: u32,
    size: u8,
    bitfield: u32,
    // TODO: Just store this as a bool and update as frames are received
    nonce_bits: u32,
}

pub struct FrameAckQueue {
    entries: std::collections::VecDeque<Entry>,
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
                    last_entry.size += 1;
                    last_entry.nonce_bits |= (nonce as u32) << bit;
                }
            } else {
                self.entries.push_back(Entry {
                    base_id: frame_id,
                    size: 1,
                    bitfield: 0x00000001,
                    nonce_bits: nonce as u32,
                });
            }
        } else {
            self.entries.push_back(Entry {
                base_id: frame_id,
                size: 1,
                bitfield: 0x00000001,
                nonce_bits: nonce as u32,
            });
        }
    }

    pub fn pop(&mut self) -> Option<FrameAck> {
        if let Some(first_entry) = self.entries.pop_front() {
            let mut nonce = false;
            for bit in 0 .. 32 {
                if first_entry.bitfield & (0x00000001 << bit) != 0 {
                    nonce ^= (first_entry.nonce_bits & (0x00000001 << bit)) != 0;
                }
            }

            debug_assert!(first_entry.bitfield & 0x00000001 != 0);

            return Some(FrameAck {
                base_id: first_entry.base_id,
                size: first_entry.size,
                bitfield: first_entry.bitfield,
                nonce: nonce,
            });
        }

        return None;
    }
}

