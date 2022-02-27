
use super::reorder_buffer;
use super::loss_rate;
use super::send_rate;
use super::frame_log_2::FrameLog;

fn ms_to_s(v_s: u64) -> f64 {
    v_s as f64 / 1000.0
}

pub struct AckData {
    pub last_send_time_ms: u64,
    pub total_ack_size: usize,
    pub rate_limited: bool,
}

pub struct FeedbackBuilder {
    // Last time feedback was handled
    last_feedback_ms: Option<u64>,

    // Aggregated feedback data from latest ack frames
    ack_data: Option<AckData>,

    // Determines when frames are considered dropped (NDUPACK = 3)
    reorder_buffer: reorder_buffer::ReorderBuffer,

    // Used to compute the receiver loss rate
    loss_intervals: loss_rate::LossIntervalQueue,
}

impl FeedbackBuilder {
    pub fn new(base_id: u32) -> Self {
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

    pub fn put_ack_data(&mut self, ack_data: AckData) {
        if let Some(ref mut feedback_data) = self.ack_data {
            feedback_data.last_send_time_ms = feedback_data.last_send_time_ms.max(ack_data.last_send_time_ms);
            feedback_data.total_ack_size += ack_data.total_ack_size;
            feedback_data.rate_limited |= ack_data.rate_limited;
        } else {
            self.ack_data = Some(ack_data);
        }
    }

    pub fn mark_acked(&mut self, frame_id: u32, frame_log: &FrameLog) {
        let ref mut loss_intervals = self.loss_intervals;

        if self.reorder_buffer.can_put(frame_id) {
            // New frame, cycle reorder buffer
            self.reorder_buffer.put(frame_id, |frame_id, was_seen| {
                let sent_frame = frame_log.get_frame(frame_id).unwrap();

                if was_seen {
                    loss_intervals.put_ack();
                } else {
                    loss_intervals.put_nack(sent_frame.send_time_ms, 100);
                }
            });
        } else {
            // Old frame, fill hole in loss intervals
        }
    }

    pub fn advance_base_id(&mut self, new_base_id: u32, frame_log: &FrameLog) {
        debug_assert!(self.reorder_buffer.can_advance(new_base_id));

        let ref mut loss_intervals = self.loss_intervals;
        self.reorder_buffer.advance(new_base_id, |frame_id, was_seen| {
            let sent_frame = frame_log.get_frame(frame_id).unwrap();

            if was_seen {
                loss_intervals.put_ack();
            } else {
                loss_intervals.put_nack(sent_frame.send_time_ms, 100);
            }
        });
    }
}

