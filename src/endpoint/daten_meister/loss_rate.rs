
// See RFC 5348: "TCP Friendly Rate Control (TFRC): Protocol Specification"

// This is a constant-time overhead implementation of the loss rate computation, where only the
// most recent loss interval is updated each time a new frame is acked or nacked. Currently, no
// attempt is made to fill holes in the loss history as acks are received for previous nacks. In
// the future, an approximation to hole-filling will be performed which does not require iteration
// over the loss history.

use std::collections::VecDeque;

#[derive(Debug)]
struct LossInterval {
    end_time_ms: u64,
    length: u32,
}

#[derive(Debug)]
pub struct LossIntervalQueue {
    entries: VecDeque<LossInterval>,
}

impl LossIntervalQueue {
    // See section 5.4
    const WEIGHTS: [f64; 8] = [ 1.0, 1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.2 ];

    pub fn new() -> Self {
        Self {
            entries: VecDeque::new()
        }
    }

    pub fn reset(&mut self, initial_p: f64) {
        // Because the loss interval queue is truncated here, the particular loss pattern of the
        // first feedback message is ignored. That is, if frames were acked/nacked as follows:
        //
        //                  first nack
        //                       v
        //             111111111100100111000110111
        // a) +------------------
        // b) +------------------+--+----+----+---
        //
        // Only the single loss interval (a) will be generated, with a length initialized according
        // to the throughput equation, and ignoring any subsequent initial loss events. Standard
        // TFRC would not ignore those loss events, as shown in (b).
        //
        // By ignoring subsequent loss intervals, the throughput equation phase will begin at half
        // of the maximum send rate regardless of the initial loss pattern. This effects a slightly
        // improved response to initial packet drops, but any difference in behavior relative to
        // standard TFRC should be marginal.

        self.entries.truncate(1);
        self.entries[0].length = (Self::WEIGHTS[0] / initial_p).clamp(0.0, u32::MAX as f64).round() as u32;
    }

    pub fn push_ack(&mut self) {
        if let Some(last_interval) = self.entries.front_mut() {
            // Acks always contribute to previous loss interval
            last_interval.length = last_interval.length.saturating_add(1);
        }
    }

    pub fn push_nack(&mut self, send_time_ms: u64, rtt_ms: u64) {
        if let Some(last_interval) = self.entries.front_mut() {
            if send_time_ms >= last_interval.end_time_ms {
                // This nack marks a new loss interval
                self.entries.push_front(LossInterval {
                    end_time_ms: send_time_ms + rtt_ms,
                    length: 1,
                });

                self.entries.truncate(9);
            } else {
                // This nack falls under previous loss interval
                last_interval.length = last_interval.length.saturating_add(1);
            }
        } else {
            // This nack marks a new loss interval
            self.entries.push_front(LossInterval {
                end_time_ms: send_time_ms + rtt_ms,
                length: 1,
            });
        }
    }

    pub fn compute_loss_rate(&self) -> f64 {
        // See section 5.4
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

#[cfg(test)]
mod tests {
    use super::*;

    /* Tests from previous incarnation. Because frames are now consumed greedily from the reorder
     * buffer, some modification will be necessary.
     *
    #[test]
    fn basic() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame(0, false, 53, 0);
        fbc.log_frame(1, false, 71, 0);
        fbc.log_frame(2, false, 89, 0);
        fbc.log_frame(3, false, 107, 10);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b1111, nonce: false }, 100);

        let feedback = fbc.pending_feedback().unwrap();

        assert_eq!(feedback.last_send_time_ms, 10);
        assert_eq!(feedback.total_ack_size, 320);
        assert_eq!(feedback.loss_rate, 0.0);
        assert_eq!(feedback.rate_limited, false);

        assert_eq!(fbc.pending_feedback(), None);
    }

    #[test]
    fn bad_nonce() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame(0, false, 53, 0);
        fbc.log_frame(1, true, 71, 0);
        fbc.log_frame(2, false, 89, 0);
        fbc.log_frame(3, true, 107, 10);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b1111, nonce: true }, 100);

        assert_eq!(fbc.pending_feedback(), None);
    }

    #[test]
    fn rate_limited() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame(0, false, 53, 0);
        fbc.log_rate_limited();
        fbc.log_frame(1, false, 71, 0);
        fbc.log_frame(2, false, 89, 0);
        fbc.log_frame(3, false, 107, 10);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b1001, nonce: false }, 100);

        let feedback = fbc.pending_feedback().unwrap();

        assert_eq!(feedback.last_send_time_ms, 10);
        assert_eq!(feedback.total_ack_size, 160);
        assert_eq!(feedback.rate_limited, true);
    }

    #[test]
    fn loss_intervals_reorder() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame( 0, false, 64, 0);
        fbc.log_frame( 1, false, 64, 0);
        fbc.log_frame( 2, false, 64, 0);
        fbc.log_frame( 3, false, 64, 0);
        fbc.log_frame( 4, false, 64, 0);
        fbc.log_frame( 5, false, 64, 0);
        fbc.log_frame( 6, false, 64, 0);
        fbc.log_frame( 7, false, 64, 0);
        fbc.log_frame( 8, false, 64, 0);
        fbc.log_frame( 9, false, 64, 0);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b0000001001, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ ]);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b0001001001, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ ]);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b1001001001, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ 3 ]);
    }

    #[test]
    fn loss_intervals_rtt() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame( 0, false, 64,   0); // ack
        fbc.log_frame( 1, false, 64,   0);
        fbc.log_frame( 2, false, 64,   1);
        fbc.log_frame( 3, false, 64,  99);
        fbc.log_frame( 4, false, 64, 100);
        fbc.log_frame( 5, false, 64, 200);
        fbc.log_frame( 6, false, 64, 200);
        fbc.log_frame( 7, false, 64, 200); // ack
        fbc.log_frame( 8, false, 64, 200); // ack
        fbc.log_frame( 9, false, 64, 200); // ack

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b1110000001, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ 3, 1, 3 ]);
    }

    #[test]
    fn loss_intervals_reorder_forget_partial() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame( 0, false, 64,   0);
        fbc.log_frame( 1, false, 64,   0);
        fbc.log_frame( 2, false, 64, 100); // ack
        fbc.log_frame( 3, false, 64, 100);
        fbc.log_frame( 4, false, 64, 150);
        fbc.log_frame( 5, false, 64, 150); // ack
        fbc.log_frame( 6, false, 64, 199);
        fbc.log_frame( 7, false, 64, 200);
        fbc.log_frame( 8, false, 64, 300);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b000000100, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ ]);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b000100000, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ ]);

        assert_eq!(fbc.next_ack_id, 0);
        assert_eq!(fbc.frame_log.base_id(), 0);
        assert_eq!(fbc.frame_log.next_id(), 9);

        fbc.forget_frames(150, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ 1, 3 ]);

        assert_eq!(fbc.next_ack_id, 4);
        assert_eq!(fbc.frame_log.base_id(), 4);
        assert_eq!(fbc.frame_log.next_id(), 9);
    }

    #[test]
    fn loss_intervals_reorder_forget_full() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame( 0, false, 64,   0);
        fbc.log_frame( 1, false, 64,   0);
        fbc.log_frame( 2, false, 64, 100); // ack
        fbc.log_frame( 3, false, 64, 100);
        fbc.log_frame( 4, false, 64, 100);
        fbc.log_frame( 5, false, 64, 100); // ack
        fbc.log_frame( 6, false, 64, 199);
        fbc.log_frame( 7, false, 64, 200);
        fbc.log_frame( 8, false, 64, 300);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b000000100, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ ]);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b000100000, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ ]);

        assert_eq!(fbc.next_ack_id, 0);
        assert_eq!(fbc.frame_log.base_id(), 0);
        assert_eq!(fbc.frame_log.next_id(), 9);

        fbc.forget_frames(300, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ 1, 4, 3 ]);

        assert_eq!(fbc.next_ack_id, 8);
        assert_eq!(fbc.frame_log.base_id(), 8);
        assert_eq!(fbc.frame_log.next_id(), 9);
    }

    #[test]
    fn max_loss_intervals() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame( 0, false, 64,    0); // ack
        fbc.log_frame( 1, false, 64,    0);
        fbc.log_frame( 2, false, 64,  100);
        fbc.log_frame( 3, false, 64,  200);
        fbc.log_frame( 4, false, 64,  200);
        fbc.log_frame( 5, false, 64,  300);
        fbc.log_frame( 6, false, 64,  400);
        fbc.log_frame( 7, false, 64,  500);
        fbc.log_frame( 8, false, 64,  500);
        fbc.log_frame( 9, false, 64,  500);
        fbc.log_frame(10, false, 64,  600);
        fbc.log_frame(11, false, 64,  700);
        fbc.log_frame(12, false, 64,  800);
        fbc.log_frame(13, false, 64,  800);
        fbc.log_frame(14, false, 64,  900);
        fbc.log_frame(15, false, 64,  900);
        fbc.log_frame(16, false, 64,  900); // ack
        fbc.log_frame(17, false, 64, 1000); // ack
        fbc.log_frame(18, false, 64, 1000); // ack

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0x70001, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ 3, 2, 1, 1, 3, 1, 1, 2, 1 ]);
    }

    #[test]
    fn loss_intervals_changing_rtt() {
        let mut fbc = FeedbackComp::new(0);

        fbc.log_frame( 0, false, 64,   0); // ack @ 100ms RTT
        fbc.log_frame( 1, false, 64,   0);
        fbc.log_frame( 2, false, 64,   0); // ack @ 100ms RTT
        fbc.log_frame( 3, false, 64,   0); // ack @ 100ms RTT
        fbc.log_frame( 4, false, 64,   0); // ack @ 100ms RTT
        fbc.log_frame( 5, false, 64,  99);
        fbc.log_frame( 6, false, 64, 100);
        fbc.log_frame( 7, false, 64, 150);
        fbc.log_frame( 8, false, 64, 200); // ack @ 50ms RTT
        fbc.log_frame( 9, false, 64, 200); // ack @ 50ms RTT
        fbc.log_frame(10, false, 64, 200); // ack @ 50ms RTT

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b00000011101, nonce: false }, 100);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ 2 ]);

        fbc.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b11100000000, nonce: false }, 50);

        assert_eq!(fbc.loss_intervals.entries.iter().map(|entry| entry.length).collect::<Vec<_>>(), vec![ 2, 1, 5 ]);
    }
    */
}

