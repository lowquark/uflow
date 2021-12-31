
use crate::frame::AckGroup;

struct Entry {
    frame_bits_base_id: u32,
    frame_bits_size: u8,
    frame_bits: u32,
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
            let frame_bits_base_id = last_entry.frame_bits_base_id;
            let bit = frame_id.wrapping_sub(frame_bits_base_id);
            if bit < 32 {
                if last_entry.frame_bits & (0x00000001 << bit) == 0 {
                    last_entry.frame_bits |= 0x00000001 << bit;
                    last_entry.frame_bits_size += 1;
                    last_entry.nonce_bits |= (nonce as u32) << bit;
                }
            } else {
                self.entries.push_back(Entry {
                    frame_bits_base_id: frame_id,
                    frame_bits_size: 1,
                    frame_bits: 0x00000001,
                    nonce_bits: nonce as u32,
                });
            }
        } else {
            self.entries.push_back(Entry {
                frame_bits_base_id: frame_id,
                frame_bits_size: 1,
                frame_bits: 0x00000001,
                nonce_bits: nonce as u32,
            });
        }
    }

    pub fn pop(&mut self) -> Option<AckGroup> {
        if let Some(first_entry) = self.entries.pop_front() {
            let mut nonce = false;
            for bit in 0 .. 32 {
                if first_entry.frame_bits & (0x00000001 << bit) != 0 {
                    nonce ^= (first_entry.nonce_bits & (0x00000001 << bit)) != 0;
                }
            }

            debug_assert!(first_entry.frame_bits & 0x00000001 != 0);

            return Some(AckGroup {
                frame_bits_base_id: first_entry.frame_bits_base_id,
                frame_bits_size: first_entry.frame_bits_size,
                frame_bits: first_entry.frame_bits,
                frame_bits_nonce: nonce,
            });
        }

        return None;
    }
}

