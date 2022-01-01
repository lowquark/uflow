
use std::collections::VecDeque;

#[derive(Debug)]
pub struct SentFrame {
    pub size: usize,
    pub send_time_ms: u64,
    // TODO: Store these in high bits of timestamp
    pub nonce: bool,
    pub rate_limited: bool,
}

pub struct FrameLog {
    base_id: u32,
    next_id: u32,
    frames: VecDeque<SentFrame>,
}

impl FrameLog {
    pub fn new(base_id: u32) -> Self {
        Self {
            base_id: base_id,
            next_id: base_id,
            frames: VecDeque::new(),
        }
    }

    pub fn push(&mut self, frame_id: u32, sent_frame: SentFrame) {
        assert!(frame_id == self.next_id);
        assert!(self.frames.len() < u32::MAX as usize);
        self.frames.push_back(sent_frame);
        self.next_id = self.next_id.wrapping_add(1);
    }

    pub fn pop(&mut self, thresh_ms: u64) {
        while let Some(frame) = self.frames.front() {
            if frame.send_time_ms < thresh_ms {
                self.frames.pop_front();
                self.base_id = self.base_id.wrapping_add(1);
            } else {
                return;
            }
        }
    }

    pub fn get(&self, frame_id: u32) -> Option<&SentFrame> {
        self.frames.get(frame_id.wrapping_sub(self.base_id) as usize)
    }

    pub fn base_id(&self) -> u32 {
        self.base_id
    }

    pub fn len(&self) -> u32 {
        self.frames.len() as u32
    }
}

