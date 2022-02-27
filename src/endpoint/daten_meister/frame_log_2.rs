
use super::pending_packet::FragmentRef;

use std::collections::VecDeque;

#[derive(Debug)]
pub struct FrameEntry {
    pub size: u32,
    pub send_time_ms: u64,
    pub fragment_refs: Box<[FragmentRef]>,
    pub nonce: bool,
    pub rate_limited: bool,
    pub acked: bool,
}

#[derive(Debug)]
pub struct FrameLog {
    next_id: u32,
    base_id: u32,
    frames: VecDeque<FrameEntry>,
}

impl FrameLog {
    pub fn new(base_id: u32) -> Self {
        Self {
            next_id: base_id,
            base_id: base_id,
            frames: VecDeque::new(),
        }
    }

    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    pub fn base_id(&self) -> u32 {
        self.base_id
    }

    pub fn get_frame(&self, frame_id: u32) -> Option<&FrameEntry> {
        self.frames.get(frame_id.wrapping_sub(self.base_id) as usize)
    }

    pub fn get_frame_mut(&mut self, frame_id: u32) -> Option<&mut FrameEntry> {
        self.frames.get_mut(frame_id.wrapping_sub(self.base_id) as usize)
    }

    pub fn len(&self) -> u32 {
        self.frames.len() as u32
    }

    pub fn push_frame(&mut self, size: usize, now_ms: u64, fragment_refs: Box<[FragmentRef]>, nonce: bool, rate_limited: bool) {
        debug_assert!(size < u32::MAX as usize);
        debug_assert!(now_ms >= self.frames.back().map_or(0, |frame| frame.send_time_ms));
        debug_assert!(self.frames.len() < u32::MAX as usize);

        self.frames.push_back(FrameEntry {
            size: size as u32,
            send_time_ms: now_ms,
            fragment_refs,
            nonce,
            rate_limited,
            acked: false,
        });

        self.next_id = self.next_id.wrapping_add(1);
    }

    pub fn find_expiration_cutoff(&self, thresh_ms: u64) -> u32 {
        // TODO: Consider using VecDeque::partition_point()
        let mut expiry_point = self.base_id;
        for frame in self.frames.iter() {
            if frame.send_time_ms < thresh_ms {
                expiry_point = expiry_point.wrapping_add(1);
            } else {
                break;
            }
        }
        return expiry_point;
    }

    pub fn drain(&mut self, frame_id: u32) {
        let drain_idx = frame_id.wrapping_sub(self.base_id) as usize;
        self.frames.drain(.. drain_idx);
        self.base_id = frame_id;
    }
}

