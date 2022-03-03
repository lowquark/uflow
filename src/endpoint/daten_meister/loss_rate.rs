
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
}

