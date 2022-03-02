
use crate::frame;

use super::TransferWindow;

use super::reorder_buffer;
use super::loss_rate;

use super::send_rate;

use super::pending_packet::FragmentRef;

use std::collections::VecDeque;

use crate::MAX_FRAME_TRANSFER_WINDOW_SIZE;

#[derive(Debug)]
pub struct Entry {
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
    frames: VecDeque<Entry>,
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

    pub fn get_frame(&self, frame_id: u32) -> Option<&Entry> {
        self.frames.get(frame_id.wrapping_sub(self.base_id) as usize)
    }

    pub fn get_frame_mut(&mut self, frame_id: u32) -> Option<&mut Entry> {
        self.frames.get_mut(frame_id.wrapping_sub(self.base_id) as usize)
    }

    pub fn len(&self) -> u32 {
        self.frames.len() as u32
    }

    pub fn push(&mut self, entry: Entry) {
        debug_assert!(entry.send_time_ms >= self.frames.back().map_or(0, |last| last.send_time_ms));
        debug_assert!(self.frames.len() < u32::MAX as usize);

        self.frames.push_back(entry);
        self.next_id = self.next_id.wrapping_add(1);
    }

    pub fn find_expiration_cutoff(&self, thresh_ms: u64) -> u32 {
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

fn ms_to_s(v_s: u64) -> f64 {
    v_s as f64 / 1000.0
}

struct AckData {
    last_send_time_ms: u64,
    total_ack_size: usize,
    rate_limited: bool,
}

pub struct FeedbackGen {
    // Last time feedback was handled
    last_feedback_ms: Option<u64>,

    // Aggregated feedback data from latest ack frames
    ack_data: Option<AckData>,

    // Determines when frames are considered dropped (NDUPACK = 3)
    reorder_buffer: reorder_buffer::ReorderBuffer,

    // Used to compute the receiver loss rate
    loss_intervals: loss_rate::LossIntervalQueue,
}

impl FeedbackGen {
    fn new(base_id: u32) -> Self {
        Self {
            last_feedback_ms: None,
            ack_data: None,
            reorder_buffer: reorder_buffer::ReorderBuffer::new(base_id),
            loss_intervals: loss_rate::LossIntervalQueue::new(),
        }
    }

    pub fn reset_loss_rate(&mut self, new_loss_rate: f64, end_time_ms: u64) {
        self.loss_intervals.reset(new_loss_rate, end_time_ms);
    }

    pub fn get_feedback(&mut self, now_ms: u64) -> Option<send_rate::FeedbackData> {
        if let Some(ack_data) = self.ack_data.take() {
            let rtt_ms = now_ms - ack_data.last_send_time_ms;

            let receive_rate = if let Some(last_feedback_ms) = self.last_feedback_ms {
                let delta_time_s = ms_to_s(now_ms - last_feedback_ms);
                (ack_data.total_ack_size as f64 / delta_time_s).clamp(0.0, u32::MAX as f64) as u32
            } else {
                0
            };

            self.last_feedback_ms = Some(now_ms);

            let loss_rate = self.loss_intervals.compute_loss_rate();

            let rate_limited = ack_data.rate_limited;

            return Some(send_rate::FeedbackData { rtt_ms, receive_rate, loss_rate, rate_limited });
        }

        return None;
    }

    fn put_ack_data(&mut self, ack_data: AckData) {
        if let Some(ref mut feedback_data) = self.ack_data {
            feedback_data.last_send_time_ms = feedback_data.last_send_time_ms.max(ack_data.last_send_time_ms);
            feedback_data.total_ack_size += ack_data.total_ack_size;
            feedback_data.rate_limited |= ack_data.rate_limited;
        } else {
            self.ack_data = Some(ack_data);
        }
    }

    fn notify_ack(&mut self, frame_id: u32, frame_log: &FrameLog) {
        let ref mut loss_intervals = self.loss_intervals;

        if self.reorder_buffer.can_put(frame_id) {
            // New frame, cycle reorder buffer
            self.reorder_buffer.put(frame_id, |frame_id, was_seen| {
                let sent_frame = frame_log.get_frame(frame_id).unwrap();

                if was_seen {
                    loss_intervals.push_ack();
                } else {
                    loss_intervals.push_nack(sent_frame.send_time_ms, 100);
                }
            });
        } else {
            // Old frame, fill hole in loss intervals
            // TODO: Fill hole in loss intervals
        }
    }

    fn notify_advancement(&mut self, new_base_id: u32, frame_log: &FrameLog) {
        let ref mut loss_intervals = self.loss_intervals;

        if self.reorder_buffer.can_advance(new_base_id) {
            // This new base ID necessitates ack/nack advancement
            self.reorder_buffer.advance(new_base_id, |frame_id, was_seen| {
                let sent_frame = frame_log.get_frame(frame_id).unwrap();

                if was_seen {
                    loss_intervals.push_ack();
                } else {
                    loss_intervals.push_nack(sent_frame.send_time_ms, 100);
                }
            });
        }
    }
}

struct FrameQueue {
    frame_log: FrameLog,
    feedback_gen: FeedbackGen,
    window: TransferWindow,
}

impl FrameQueue {
    pub fn new(base_id: u32) -> Self {
        Self {
            frame_log: FrameLog::new(base_id),
            feedback_gen: FeedbackGen::new(base_id),
            window: TransferWindow::new(MAX_FRAME_TRANSFER_WINDOW_SIZE, base_id),
        }
    }

    pub fn can_push(&self) -> bool {
        return self.window.contains(self.frame_log.next_id());
    }

    pub fn push(&mut self, size: usize, now_ms: u64, fragment_refs: Box<[FragmentRef]>, nonce: bool, rate_limited: bool) {
        debug_assert!(self.can_push());
        debug_assert!(size <= u32::MAX as usize);

        self.frame_log.push(Entry {
            size: size as u32,
            send_time_ms: now_ms,
            fragment_refs,
            nonce,
            rate_limited,
            acked: false,
        })
    }

    pub fn forget_frames(&mut self, thresh_ms: u64) {
        self.remove_expired_entries(thresh_ms);
    }

    pub fn feedback_gen_mut(&mut self) -> &mut FeedbackGen {
        &mut self.feedback_gen
    }

    pub fn acknowledge_group(&mut self, ack: frame::FrameAck) {
        let mut true_nonce = false;

        let mut last_send_time_ms = 0;
        let mut total_ack_size = 0;
        let mut rate_limited = false;

        let mut bitfield_size = 0;
        for i in (0 .. 32).rev() {
            if ack.bitfield & (1 << i) != 0 {
                bitfield_size = i + 1;
                break;
            }
        }

        if bitfield_size == 0 {
            // Dud
            return;
        }

        for i in 0 .. bitfield_size {
            let frame_id = ack.base_id.wrapping_add(i);

            if let Some(ref sent_frame) = self.frame_log.get_frame(frame_id) {
                if ack.bitfield & (1 << i) != 0 {
                    // Receiver claims to have received this packet
                    true_nonce ^= sent_frame.nonce;
                }
            } else {
                // Packet forgotten or ack group exceeds span of transfer queue
                return;
            }
        }

        if ack.nonce != true_nonce {
            // Penalize bad nonce
            return;
        }

        for i in 0 .. bitfield_size {
            let frame_id = ack.base_id.wrapping_add(i);

            let ref mut sent_frame = self.frame_log.get_frame_mut(frame_id).unwrap();

            rate_limited |= sent_frame.rate_limited;

            if ack.bitfield & (1 << i) != 0 {
                // Receiver has received this packet
                if sent_frame.acked == false {
                    sent_frame.acked = true;

                    // Mark each fragment acknowledged and clear the list
                    let fragment_refs = std::mem::take(&mut sent_frame.fragment_refs);

                    for fragment_ref in fragment_refs.into_iter() {
                        // TODO: This could be a method?
                        if let Some(packet_rc) = fragment_ref.packet.upgrade() {
                            let mut packet_ref = packet_rc.borrow_mut();
                            packet_ref.acknowledge_fragment(fragment_ref.fragment_id);
                        }
                    }

                    // Mark send time of latest included packet
                    last_send_time_ms = last_send_time_ms.max(sent_frame.send_time_ms);

                    // Add to total ack size
                    total_ack_size += sent_frame.size as usize;

                    // Detect nacks
                    self.feedback_gen.notify_ack(frame_id, &mut self.frame_log);
                }
            }
        }

        // Add to pending feedback data
        self.feedback_gen.put_ack_data(AckData { last_send_time_ms, total_ack_size, rate_limited });
    }

    pub fn can_advance_transfer_window(&mut self, new_base_id: u32) -> bool {
        let log_next_id = self.frame_log.next_id();
        let window_base_id = self.window.base_id;

        // Ensure transfer window never backtracks and never advances beyond frame log's next_id
        let next_delta = log_next_id.wrapping_sub(window_base_id);
        let delta = new_base_id.wrapping_sub(window_base_id);

        delta != 0 && delta <= next_delta
    }

    pub fn advance_transfer_window(&mut self, new_base_id: u32) {
        if self.can_advance_transfer_window(new_base_id) {
            self.window.base_id = new_base_id;
            self.remove_old_entries_beyond_window();
        }
    }

    fn remove_expired_entries(&mut self, thresh_ms: u64) {
        let max_base_id = self.frame_log.find_expiration_cutoff(thresh_ms);

        let delta = max_base_id.wrapping_sub(self.frame_log.base_id());

        if delta != 0 {
            debug_assert!(delta <= self.frame_log.len());
            self.feedback_gen.notify_advancement(max_base_id, &self.frame_log);

            self.frame_log.drain(max_base_id);
            debug_assert!(self.frame_log.base_id() == max_base_id);
        }
    }

    fn remove_old_entries_beyond_window(&mut self) {
        let max_base_id = self.window.base_id.wrapping_sub(self.window.size);

        let delta = max_base_id.wrapping_sub(self.frame_log.base_id());

        if delta != 0 && delta <= self.frame_log.len() {
            self.feedback_gen.notify_advancement(max_base_id, &self.frame_log);

            self.frame_log.drain(max_base_id);
            debug_assert!(self.frame_log.base_id() == max_base_id);
        }
    }
}

    /*
    pub fn step(&mut self, now_ms: u64) {
        self.send_rate_comp.step(now_ms, self.feedback_gen.get_feedback(now_ms),
            |new_loss_rate: f64, end_time_ms: u64| {
                self.feedback_gen.reset_loss_rate(new_loss_rate, end_time_ms);
            }
        );
    }
    */

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pending_packet::PendingPacket;

    use std::rc::Rc;
    use std::cell::RefCell;

    #[test]
    fn feedback_generation() {
        let mut fq = FrameQueue::new(0);

        let packet_rc = Rc::new(RefCell::new(
            PendingPacket::new(vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(), 0, 0, 0, 0)
        ));

        let n0 = rand::random();
        let n1 = rand::random();
        let n2 = rand::random();
        let n3 = rand::random();
        let n4 = rand::random();
        let n5 = rand::random();

        fq.push(  1, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice(), n0, false);
        fq.push(  2, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice(), n1, false);
        fq.push(  4, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice(), n2, false);
        fq.push(  8, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice(), n3, false);
        fq.push( 16, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice(), n4,  true);
        fq.push( 32, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice(), n5, false);

        // No feedback until an ack frame has been received
        assert_eq!(fq.feedback_gen_mut().get_feedback(1000), None);

        fq.acknowledge_group(frame::FrameAck { base_id: 0, bitfield: 0b101, nonce: n0 ^ n2 });

        assert_eq!(fq.feedback_gen_mut().get_feedback(1000), Some(send_rate::FeedbackData {
            loss_rate: 0.0,
            receive_rate: 0, // First receive_rate is always zero
            rate_limited: false,
            rtt_ms: 1000,
        }));

        fq.acknowledge_group(frame::FrameAck { base_id: 2, bitfield: 0b11, nonce: n2 ^ n3 });

        assert_eq!(fq.feedback_gen_mut().get_feedback(2000), Some(send_rate::FeedbackData {
            loss_rate: 0.0,
            receive_rate: 8,
            rate_limited: false,
            rtt_ms: 2000,
        }));

        fq.acknowledge_group(frame::FrameAck { base_id: 4, bitfield: 0b1, nonce: n4 });
        fq.acknowledge_group(frame::FrameAck { base_id: 5, bitfield: 0b1, nonce: n5 });

        assert_eq!(fq.feedback_gen_mut().get_feedback(3000), Some(send_rate::FeedbackData {
            loss_rate: 0.2, // Frame 2 was dropped, current loss interval is 5 sequence IDs long
            receive_rate: 48,
            rate_limited: true, // Frame 4 was marked rate limited
            rtt_ms: 3000,
        }));

        // No feedback until an ack frame has been received
        assert_eq!(fq.feedback_gen_mut().get_feedback(3000), None);
    }

    #[test]
    fn window_advancement() {
    }
}

