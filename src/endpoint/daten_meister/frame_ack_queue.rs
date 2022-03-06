
use crate::frame;

struct ReceiveWindow {
    base_id: u32,
    size: u32,
}

impl ReceiveWindow {
    pub fn new(base_id: u32, size: u32) -> Self {
        Self { base_id, size }
    }

    pub fn contains(&self, frame_id: u32) -> bool {
        frame_id.wrapping_sub(self.base_id) < self.size
    }

    pub fn advance(&mut self, new_base_id: u32) -> bool {
        let delta = new_base_id.wrapping_sub(self.base_id);
        if delta > 0 && delta <= self.size {
            self.base_id = new_base_id;
            true
        } else {
            false
        }
    }

    pub fn base_id(&self) -> u32 {
        self.base_id
    }
}

pub struct FrameAckQueue {
    entries: std::collections::VecDeque<frame::AckGroup>,
    receive_window: ReceiveWindow,
}

impl FrameAckQueue {
    pub fn new(base_id: u32, size: u32) -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            receive_window: ReceiveWindow::new(base_id, size),
        }
    }

    pub fn base_id(&self) -> u32 {
        self.receive_window.base_id()
    }

    pub fn resynchronize(&mut self, sender_next_id: u32) {
        self.receive_window.advance(sender_next_id);
    }

    pub fn window_contains(&self, frame_id: u32) -> bool {
        self.receive_window.contains(frame_id)
    }

    pub fn mark_seen(&mut self, frame_id: u32, nonce: bool) {
        if self.receive_window.contains(frame_id) {
            self.receive_window.advance(frame_id.wrapping_add(1));

            if let Some(last_entry) = self.entries.back_mut() {
                let bit = frame_id.wrapping_sub(last_entry.base_id);
                if bit < 32 {
                    if last_entry.bitfield & (0x00000001 << bit) == 0 {
                        last_entry.bitfield |= 0x00000001 << bit;
                        last_entry.nonce ^= nonce;
                    }
                } else {
                    self.entries.push_back(frame::AckGroup {
                        base_id: frame_id,
                        bitfield: 0x00000001,
                        nonce: nonce,
                    });
                }
            } else {
                self.entries.push_back(frame::AckGroup {
                    base_id: frame_id,
                    bitfield: 0x00000001,
                    nonce: nonce,
                });
            }
        }
    }

    pub fn pop(&mut self) -> Option<frame::AckGroup> {
        if let Some(first_entry) = self.entries.pop_front() {
            debug_assert!(first_entry.bitfield & 0x00000001 != 0);

            return Some(first_entry);
        }

        return None;
    }

    pub fn peek(&self) -> Option<&frame::AckGroup> {
        self.entries.front()
    }
}

