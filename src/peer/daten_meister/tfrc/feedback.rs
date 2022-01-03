
use super::frame_log;
use crate::frame;

use std::collections::VecDeque;

struct ReorderBuffer {
    buf: [u32; 2],
    size: usize,
}

impl ReorderBuffer {
    fn new() -> Self {
        Self {
            buf: [0; 2],
            size: 0,
        }
    }

    fn min(&mut self, base_id: u32) -> Option<u32> {
        match self.size {
            0 => {
                return None;
            }
            1 => {
                return Some(self.buf[0]);
            }
            2 => {
                let delta0 = self.buf[0].wrapping_sub(base_id);
                let delta1 = self.buf[1].wrapping_sub(base_id);

                if delta0 < delta1 {
                    return Some(self.buf[0]);
                } else {
                    return Some(self.buf[1]);
                }
            }
            _ => panic!()
        }
    }

    fn push(&mut self, base_id: u32, frame_id: u32) -> Option<u32> {
        match self.size {
            0 => {
                self.buf[0] = frame_id;
                self.size = 1;
                return None;
            }
            1 => {
                if self.buf[0] == frame_id {
                    return None;
                }

                self.buf[1] = frame_id;
                self.size = 2;
                return None;
            }
            2 => {
                if self.buf[0] == frame_id {
                    return None;
                }
                if self.buf[1] == frame_id {
                    return None;
                }

                let delta0 = self.buf[0].wrapping_sub(base_id);
                let delta1 = self.buf[1].wrapping_sub(base_id);

                if delta0 < delta1 {
                    let min = self.buf[0];
                    self.buf[0] = frame_id;
                    return Some(min);
                } else {
                    let min = self.buf[1];
                    self.buf[1] = frame_id;
                    return Some(min);
                }
            }
            _ => panic!()
        }
    }

    fn pop(&mut self, base_id: u32) -> Option<u32> {
        match self.size {
            0 => {
                return None;
            }
            1 => {
                self.size = 0;
                return Some(self.buf[0]);
            }
            2 => {
                self.size = 1;

                let delta0 = self.buf[0].wrapping_sub(base_id);
                let delta1 = self.buf[1].wrapping_sub(base_id);

                if delta0 < delta1 {
                    let min = self.buf[0];
                    self.buf[0] = self.buf[1];
                    return Some(min);
                } else {
                    let min = self.buf[1];
                    return Some(min);
                }
            }
            _ => panic!()
        }
    }
}

#[derive(Debug)]
struct LossInterval {
    end_time_ms: u64,
    length: u32,
    nack_count: u32,
    is_initial: bool,
}

struct LossIntervalQueue {
    entries: VecDeque<LossInterval>,
}

impl LossIntervalQueue {
    const WEIGHTS: [f64; 8] = [ 1.0, 1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.2 ];

    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    pub fn seed(&mut self, initial_p: f64) {
        if let Some(interval) = self.entries.back_mut() {
            if interval.is_initial {
                interval.length = (Self::WEIGHTS[0] / initial_p).clamp(0.0, u32::MAX as f64) as u32;
            }
        }
    }

    pub fn ack(&mut self) {
        // Acks always contribute to previous loss interval
        if let Some(last_interval) = self.entries.front_mut() {
            last_interval.length = last_interval.length.saturating_add(1);
        }
    }

    pub fn nack(&mut self, send_time_ms: u64, rtt_ms: u64) {
        if let Some(last_interval) = self.entries.front_mut() {
            if send_time_ms >= last_interval.end_time_ms {
                // This nack marks a new loss interval
                self.entries.push_front(LossInterval {
                    end_time_ms: send_time_ms + rtt_ms,
                    length: 1,
                    nack_count: 1,
                    is_initial: false,
                });

                self.entries.truncate(9);
            } else {
                // This nack falls under previous loss interval
                last_interval.length = last_interval.length.saturating_add(1);
                last_interval.nack_count = last_interval.nack_count.saturating_add(1);
            }
        } else {
            // First loss interval (entry which may be seeded)
            self.entries.push_front(LossInterval {
                end_time_ms: send_time_ms + rtt_ms,
                length: 1,
                nack_count: 1,
                is_initial: true,
            });
        }
    }

    pub fn loss_rate(&self) -> f64 {
        if self.entries.len() > 0 {
            let mut i_total_0 = 0.0;
            let mut i_total_1 = 0.0;
            let mut w_total = 0.0;

            if self.entries.len() > 1 {
                for i in 0..self.entries.len()-1 {
                    i_total_0 += self.entries[i].length as f64 * Self::WEIGHTS[i];
                    w_total += Self::WEIGHTS[i];
                }
                for i in 1..self.entries.len() {
                    i_total_1 += self.entries[i].length as f64 * Self::WEIGHTS[i - 1];
                }

                return w_total / i_total_0.max(i_total_1);
            } else {
                return Self::WEIGHTS[0] / (self.entries[0].length as f64 * Self::WEIGHTS[0]);
            }
        } else {
            return 0.0;
        }
    }
}

#[derive(Debug)]
pub struct Feedback {
    pub last_send_time_ms: u64,
    pub total_ack_size: usize,
    pub loss_rate: f64,
    pub rate_limited: bool,
}

pub struct FeedbackComp {
    frame_log: frame_log::FrameLog,
    next_frame_rate_limited: bool,

    next_ack_id: u32,
    reorder_buffer: ReorderBuffer,
    loss_intervals: LossIntervalQueue,

    pending_feedback: Option<Feedback>,
}

impl FeedbackComp {
    pub fn new(base_id: u32) -> Self {
        Self {
            frame_log: frame_log::FrameLog::new(base_id),
            next_frame_rate_limited: false,

            next_ack_id: base_id,
            reorder_buffer: ReorderBuffer::new(),
            loss_intervals: LossIntervalQueue::new(),

            pending_feedback: None,
        }
    }

    pub fn log_frame(&mut self, frame_id: u32, nonce: bool, size: usize, send_time_ms: u64) {
        self.frame_log.push(frame_id, frame_log::SentFrame { size, send_time_ms, nonce, rate_limited: self.next_frame_rate_limited });
        self.next_frame_rate_limited = false;
    }

    pub fn log_rate_limited(&mut self) {
        self.next_frame_rate_limited = true;
    }

    pub fn acknowledge_frames(&mut self, ack: frame::FrameAck, rtt_ms: u64) {
        let mut true_nonce = false;
        let mut recv_size = 0;
        let mut last_id = None;
        let mut rate_limited = false;

        let mut ack_size = 0;
        for i in (0 .. 32).rev() {
            if ack.bitfield & (1 << i) != 0 {
                ack_size = i + 1;
                break;
            }
        }

        for i in 0 .. ack_size {
            let frame_id = ack.base_id.wrapping_add(i);

            if let Some(ref sent_frame) = self.frame_log.get(frame_id) {
                if ack.bitfield & (1 << i) != 0 {
                    // Receiver claims to have received this packet
                    true_nonce ^= sent_frame.nonce;
                    recv_size += sent_frame.size;
                    last_id = Some(frame_id);
                }

                rate_limited |= sent_frame.rate_limited;
            } else {
                // Packet forgotten or ack group invalid
                return;
            }
        }

        // Penalize bad nonce
        if ack.nonce != true_nonce {
            return;
        }

        for i in 0 .. ack_size {
            if ack.bitfield & (1 << i) != 0 {
                // Receiver has received this packet
                let frame_id = ack.base_id.wrapping_add(i);

                self.acknowledge_frame(frame_id, rtt_ms);
            }
        }

        if let Some(last_id) = last_id {
            let ref last_frame = self.frame_log.get(last_id).unwrap();

            if let Some(ref mut pending_feedback) = self.pending_feedback {
                pending_feedback.last_send_time_ms = pending_feedback.last_send_time_ms.max(last_frame.send_time_ms);
                pending_feedback.total_ack_size += recv_size;
                pending_feedback.rate_limited |= rate_limited;
            } else {
                self.pending_feedback = Some(Feedback {
                    last_send_time_ms: last_frame.send_time_ms,
                    total_ack_size: recv_size,
                    rate_limited,
                    loss_rate: 0.0,
                });
            }
        }
    }

    pub fn forget_frames(&mut self, thresh_ms: u64, rtt_ms: u64) {
        let expired_count = self.frame_log.count_expired(thresh_ms);

        if expired_count > 0 {
            let base_id = self.frame_log.base_id();
            let new_base_id = base_id.wrapping_add(expired_count);

            let new_end_delta = self.frame_log.next_id().wrapping_sub(new_base_id);

            while let Some(min_frame_id) = self.reorder_buffer.min(self.next_ack_id) {
                let min_delta = min_frame_id.wrapping_sub(new_base_id);

                if min_delta >= new_end_delta {
                    self.reorder_buffer.pop(self.next_ack_id);

                    self.put_nack_range(self.next_ack_id, min_frame_id.wrapping_sub(self.next_ack_id), rtt_ms);
                    self.put_ack();
                    self.next_ack_id = min_frame_id.wrapping_add(1);
                } else {
                    break;
                }
            }

            let next_ack_delta = self.next_ack_id.wrapping_sub(new_base_id);

            if next_ack_delta >= new_end_delta {
                self.put_nack_range(self.next_ack_id, new_base_id.wrapping_sub(self.next_ack_id), rtt_ms);
                self.next_ack_id = new_base_id;
            }

            self.frame_log.drain_front(expired_count);
        }
    }

    pub fn seed_loss_rate(&mut self, loss_rate_initial: f64) {
        self.loss_intervals.seed(loss_rate_initial);
    }

    pub fn pending_feedback(&mut self) -> Option<Feedback> {
        if let Some(ref mut pending_feedback) = self.pending_feedback {
            pending_feedback.loss_rate = self.loss_intervals.loss_rate();
        }
        self.pending_feedback.take()
    }

    fn acknowledge_frame(&mut self, frame_id: u32, rtt_ms: u64) {
        let base_id = self.frame_log.base_id();

        if frame_id.wrapping_sub(base_id) < self.next_ack_id.wrapping_sub(base_id) {
            // This id has already been passed

            // TODO: Attempt to erase congestion event
            //self.loss_intervals.ack_previous_nack(...)

            return;
        }

        if let Some(frame_id) = self.reorder_buffer.push(self.next_ack_id, frame_id) {
            self.put_nack_range(self.next_ack_id, frame_id.wrapping_sub(self.next_ack_id), rtt_ms);
            self.put_ack();
            self.next_ack_id = frame_id.wrapping_add(1);
        }
    }

    fn put_nack_range(&mut self, base_id: u32, num: u32, rtt_ms: u64) {
        for i in 0 .. num {
            let nacked_frame_id = base_id.wrapping_add(i);
            let ref sent_frame = self.frame_log.get(nacked_frame_id).unwrap();
            self.loss_intervals.nack(sent_frame.send_time_ms, rtt_ms);
        }
    }

    fn put_ack(&mut self) {
        self.loss_intervals.ack();
    }
}

