
use crate::frame::FrameAck;

use super::FragmentRef;

use std::collections::VecDeque;

pub struct Entry {
    pub send_time_ms: u64,
    pub fragment_refs: Box<[FragmentRef]>,
}

pub struct FrameLog {
    base_id: u32,
    next_id: u32,
    frames: VecDeque<Entry>,
}

impl FrameLog {
    pub fn new(base_id: u32) -> Self {
        Self {
            base_id: base_id,
            next_id: base_id,
            frames: VecDeque::new(),
        }
    }

    pub fn push(&mut self, frame_id: u32, sent_frame: Entry) {
        assert!(frame_id == self.next_id);
        assert!(self.frames.len() < u32::MAX as usize);
        self.frames.push_back(sent_frame);
        self.next_id = self.next_id.wrapping_add(1);
    }

    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    pub fn len(&self) -> u32 {
        self.frames.len() as u32
    }

    pub fn acknowledge_frames(&mut self, ack: FrameAck) {
        let mut ack_size = 0;
        for i in (0 .. 32).rev() {
            if ack.bitfield & (1 << i) != 0 {
                ack_size = i + 1;
                break;
            }
        }

        for i in 0 .. ack_size {
            if ack.bitfield & (1 << i) != 0 {
                let frame_id = ack.base_id.wrapping_add(i);

                if let Some(sent_frame) = self.frames.get_mut(frame_id.wrapping_sub(self.base_id) as usize) {
                    let mut fragment_refs = std::mem::take(&mut sent_frame.fragment_refs);

                    for fragment_ref in fragment_refs.as_mut().into_iter() {
                        fragment_ref.acknowledge();
                    }
                }
            }
        }
    }

    pub fn forget_frames(&mut self, thresh_ms: u64) {
        while let Some(frame) = self.frames.front() {
            if frame.send_time_ms < thresh_ms {
                self.frames.pop_front();
                self.base_id = self.base_id.wrapping_add(1);
            } else {
                return;
            }
        }
    }
}

