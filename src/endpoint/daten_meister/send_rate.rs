
// See RFC 5348: "TCP Friendly Rate Control (TFRC): Protocol Specification"

use super::recv_rate_set;
use crate::MAX_FRAME_SIZE;

// Segment size in bytes (s), see section 
const MSS: usize = MAX_FRAME_SIZE;

// Initial TCP window size (based on MSS), see section 4.2
const INITIAL_TCP_WINDOW: u32 = 4380; // std::cmp::min(std::cmp::max(2*MSS, 4380), 4*MSS) as u32;
// Absolute minimum send rate (s/t_mbi), see section 4.3
const MINIMUM_RATE: u32 = (MSS / 64) as u32;

fn s_to_ms(v_s: f64) -> u64 {
    (v_s * 1000.0).max(0.0).round() as u64
}

fn ms_to_s(v_s: u64) -> f64 {
    v_s as f64 / 1000.0
}

fn eval_tcp_throughput(rtt: f64, p: f64) -> u32 {
    let s = MSS as f64;
    let f_p = (p*2.0/3.0).sqrt() + 12.0*(p*3.0/8.0).sqrt()*p*(1.0 + 32.0*p*p);
    (s / (rtt * f_p)) as u32
}

fn eval_tcp_throughput_inv(rtt: f64, target_rate_bps: u32) -> f64 {
    let delta = (target_rate_bps as f64 * 0.05) as u32;

    let mut a = 0.0;
    let mut b = 1.0;

    loop {
        let c = (b + a)/2.0;

        let rate = eval_tcp_throughput(rtt, c);

        if rate > target_rate_bps {
            if rate - target_rate_bps <= delta {
                return c;
            } else {
                a = c;
                continue;
            }
        } else if rate < target_rate_bps {
            if target_rate_bps - rate <= delta {
                return c;
            } else {
                b = c;
                continue;
            }
        } else {
            return c;
        }
    }
}

pub struct FeedbackData {
    pub rtt_ms: u64,
    pub receive_rate: u32,
    pub loss_rate: f64,
    pub rate_limited: bool,
}

struct SlowStartState {
    time_last_doubled_ms: Option<u64>,
}

struct ThroughputEqnState {
    send_rate_tcp: u32,
}

enum SendRateMode {
    AwaitSend,
    SlowStart(SlowStartState),
    ThroughputEqn(ThroughputEqnState),
}

pub struct SendRateComp {
    // Previous loss rate
    prev_loss_rate: f64,

    // Expiration of nofeedback timer
    nofeedback_exp_ms: Option<u64>,
    // Flag indicating whether sender has been idle since the nofeedback timer was sent
    nofeedback_idle: bool,

    // State used to compute send rate
    mode: SendRateMode,
    // Allowed transmit rate (X)
    send_rate: u32,
    // Application specified maximum transmit rate
    max_send_rate: u32,

    // Queue of receive rates reported by receiver (X_recv_set)
    recv_rate_set: recv_rate_set::RecvRateSet,

    // Round trip time estimate
    rtt_s: Option<f64>,
    rtt_ms: Option<u64>,

    // Most recent RTO computation
    rto_ms: Option<u64>,
}

fn compute_initial_send_rate(rtt_s: f64) -> u32 {
    (INITIAL_TCP_WINDOW as f64 / rtt_s) as u32
}

fn compute_initial_loss_send_rate(rtt_s: f64) -> u32 {
    ((MSS/2) as f64 / rtt_s) as u32
}

impl SendRateComp {
    pub fn new(max_send_rate: u32) -> Self {
        Self {
            prev_loss_rate: 0.0,

            nofeedback_exp_ms: None,
            nofeedback_idle: false,

            mode: SendRateMode::AwaitSend,
            send_rate: MSS as u32,
            max_send_rate: max_send_rate,

            recv_rate_set: recv_rate_set::RecvRateSet::new(),

            rtt_s: None,
            rtt_ms: None,

            rto_ms: None,
        }
    }

    pub fn send_rate(&self) -> f64 {
        self.send_rate as f64
    }

    pub fn rtt_s(&self) -> Option<f64> {
        self.rtt_s
    }

    pub fn rtt_ms(&self) -> Option<u64> {
        self.rtt_ms
    }

    pub fn rto_ms(&self) -> Option<u64> {
        self.rto_ms
    }

    pub fn notify_frame_sent(&mut self, now_ms: u64) {
        match self.mode {
            SendRateMode::AwaitSend => {
                self.nofeedback_exp_ms = Some(now_ms + 2000);
                self.mode = SendRateMode::SlowStart(SlowStartState {
                    time_last_doubled_ms: None,
                });
                self.recv_rate_set.reset_initial(now_ms);
            }
            _ => ()
        }

        self.nofeedback_idle = false;
    }

    pub fn step<F>(&mut self, now_ms: u64, feedback: Option<FeedbackData>, reset_loss_rate: F) where F: FnOnce(f64, u64) {
        match self.mode {
            SendRateMode::AwaitSend => {
                return;
            }
            _ => ()
        }

        if let Some(feedback) = feedback {
            self.handle_feedback(now_ms, feedback, reset_loss_rate);
        } else if let Some(nofeedback_exp_ms) = self.nofeedback_exp_ms {
            if now_ms >= nofeedback_exp_ms {
                self.nofeedback_expired(now_ms);
            }
        }
    }

    fn handle_feedback<F>(&mut self, now_ms: u64, feedback: FeedbackData, reset_loss_rate: F) where F: FnOnce(f64, u64) {
        let rtt_sample_s = ms_to_s(feedback.rtt_ms);
        let recv_rate = feedback.receive_rate;
        let loss_rate = feedback.loss_rate;
        let rate_limited = feedback.rate_limited;

        let (rtt_s, rtt_ms) = self.update_rtt(rtt_sample_s);
        let rto_s = self.update_rto(rtt_s, self.send_rate);

        // TODO: When ThroughputEqn is entered, this may produce a false positive depending on
        // rounding rounding error as the loss intervals are reset. An extra flag may be of use.
        // TODO: Does a new loss event always cause an increase in the loss rate? The spec calls
        // for a "new loss event or an increase in the loss event rate p"
        let loss_increase = loss_rate > self.prev_loss_rate;

        let send_rate_limit =
            if rate_limited {
                // If rate limited during the interval, the interval was not entirely data-limited
                let max_val = self.recv_rate_set.rate_limited_update(now_ms, recv_rate, rtt_ms);
                max_val.saturating_mul(2)
            } else if loss_increase {
                let max_val = self.recv_rate_set.loss_increase_update(now_ms, recv_rate);
                max_val
            } else {
                let max_val = self.recv_rate_set.data_limited_update(now_ms, recv_rate);
                max_val.saturating_mul(2)
            }.min(self.max_send_rate);

        self.prev_loss_rate = loss_rate;

        match self.mode {
            SendRateMode::SlowStart(ref mut state) => {
                if loss_increase {
                    // Nonzero loss, initialize loss history according to loss rate and enter
                    // throughput equation phase, see section 6.3.1

                    let send_rate_target = if state.time_last_doubled_ms.is_none() {
                        // First feedback indicates loss
                        compute_initial_loss_send_rate(rtt_s)
                    } else {
                        // Because this is sender-side TFRC, no X_target approximation is necessary
                        self.send_rate/2
                    };

                    let initial_p = eval_tcp_throughput_inv(rtt_s, send_rate_target);

                    reset_loss_rate(initial_p, now_ms + rtt_ms);

                    // Apply target send rate as if computed loss rate had been received
                    self.send_rate = send_rate_target.min(send_rate_limit).max(MINIMUM_RATE);

                    self.mode = SendRateMode::ThroughputEqn(
                        ThroughputEqnState {
                            send_rate_tcp: send_rate_target,
                        }
                    );
                } else {
                    // No loss, continue slow start phase

                    // Recomputing this term on the fly allows for some adaptation as RTT fluctuates
                    let initial_rate = compute_initial_send_rate(rtt_s);

                    if let Some(time_last_doubled_ms) = state.time_last_doubled_ms {
                        // Continue slow start doubling, see section 4.3, step 5
                        if now_ms - time_last_doubled_ms >= rtt_ms {
                            state.time_last_doubled_ms = Some(now_ms);
                            self.send_rate = (2*self.send_rate).min(send_rate_limit).max(initial_rate);
                        }
                    } else {
                        // Re-initialize slow start phase after first feedback, see section 4.2
                        state.time_last_doubled_ms = Some(now_ms);
                        self.send_rate = initial_rate;
                    }
                }
            }
            SendRateMode::ThroughputEqn(ref mut state) => {
                // Continue throughput equation phase, see section 4.3, step 5
                state.send_rate_tcp = eval_tcp_throughput(rtt_s, loss_rate);

                self.send_rate = state.send_rate_tcp.min(send_rate_limit).max(MINIMUM_RATE);
            }
            _ => panic!()
        }

        // Restart nofeedback timer
        self.nofeedback_exp_ms = Some(now_ms + s_to_ms(rto_s));
        self.nofeedback_idle = true;
    }

    fn nofeedback_expired(&mut self, now_ms: u64) {
        /*
        Section 4.4, step 1, can be de-mangled into the following:

        If (slow start) and (no feedback received and sender has not been idle) {
            // We do not have X_Bps or recover_rate yet.
            // Halve the allowed sending rate.
            X = max(X/2, s/t_mbi);
        } Else if (slow start) and (X < 2 * recover_rate, and sender has been idle) {
            // Don't halve the allowed sending rate.
            Do nothing;
        } Else if (slow start) {
            // We do not have X_Bps yet.
            // Halve the allowed sending rate.
            X = max(X/2, s/t_mbi);
        } Else if (throughput eqn) and (X_recv < recover_rate and sender has been idle) {
            // Don't halve the allowed sending rate.
            Do nothing;
        } Else if (throughput eqn) {
            If (X_Bps > 2*X_recv)) {
                // 2*X_recv was already limiting the sending rate.
                // Halve the allowed sending rate.
                Update_Limits(X_recv;)
            } Else {
                // The sending rate was limited by X_Bps, not by X_recv.
                // Halve the allowed sending rate.
                Update_Limits(X_Bps/2);
            }
        }

        where `slow start <=> p == 0`, and `throughput eqn <=> p > 0`.
        */

        match self.mode {
            SendRateMode::SlowStart(_) => {
                if let Some(rtt_s) = self.rtt_s {
                    // Recomputing this term on the fly allows for some adaptation as RTT fluctuates
                    let recover_rate = compute_initial_send_rate(rtt_s);

                    if self.nofeedback_idle && self.send_rate < 2*recover_rate {
                        // Do nothing, this is acceptable
                    } else {
                        // Halve send rate every RTO, subject to minimum
                        self.send_rate = (self.send_rate/2).max(MINIMUM_RATE);
                    }
                } else {
                    // In slow start, but no feedback has been received.
                    debug_assert!(self.nofeedback_idle == false);

                    // Halve send rate every RTO, subject to minimum
                    self.send_rate = (self.send_rate/2).max(MINIMUM_RATE);
                }
            }
            SendRateMode::ThroughputEqn(ref mut state) => {
                let rtt_s = self.rtt_s.unwrap();
                let recover_rate = compute_initial_send_rate(rtt_s);
                let recv_rate = self.recv_rate_set.max();

                if self.nofeedback_idle && recv_rate < recover_rate {
                    // Do nothing, this is acceptable
                } else {
                    // Alter recv_rate_set so as to halve current send rate moving forward
                    let current_limit = state.send_rate_tcp.min(recv_rate.saturating_mul(2));
                    let new_limit = (current_limit/2).max(MINIMUM_RATE);
                    self.recv_rate_set.reset(now_ms, new_limit/2);
                    self.send_rate = state.send_rate_tcp.min(new_limit);
                }
            }
            _ => panic!()
        }

        // Compute RTO for the new send rate, see section 4.4 step 2
        // No default RTT to use is given, but when no feedback has yet been received, using RTT = 0
        // will cause the RTO to begin at 2s, and double each time send_rate is halved above. This may
        // or may not be the intended behavior.
        let rto_s = self.update_rto(self.rtt_s.unwrap_or(0.0), self.send_rate);

        self.nofeedback_exp_ms = Some(now_ms + s_to_ms(rto_s));
        self.nofeedback_idle = true;
    }

    fn update_rtt(&mut self, rtt_sample_s: f64) -> (f64, u64) {
        // See section 4.3 step 2
        const RTT_ALPHA: f64 = 0.1;
        let new_rtt_s = if let Some(rtt_s) = self.rtt_s {
            (1.0 - RTT_ALPHA)*rtt_s + RTT_ALPHA*rtt_sample_s
        } else {
            rtt_sample_s
        };
        let new_rtt_ms = s_to_ms(new_rtt_s);
        self.rtt_s = Some(new_rtt_s);
        self.rtt_ms = Some(new_rtt_ms);
        return (new_rtt_s, new_rtt_ms);
    }

    fn update_rto(&mut self, rtt_s: f64, send_rate: u32) -> f64 {
        // See section 4.3 step 3 and section 4.4 step 2
        let rto_s = (4.0*rtt_s).max((2*MSS) as f64 / send_rate as f64);
        self.rto_ms = Some(s_to_ms(rto_s));
        return rto_s;
    }
}

