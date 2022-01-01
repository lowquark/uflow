
mod frame_log;
mod feedback;
mod recv_rate_set;

use recv_rate_set::RecvRateSet;

use crate::frame::FrameAck;

use std::time;

const MSS: usize = super::MAX_TRANSFER_UNIT;

fn eval_rto_s(rtt_s: f64, send_rate: u32) -> f64 {
    (4.0*rtt_s).max((2*MSS) as f64/send_rate as f64)
}

fn eval_tcp_throughput(rtt: f64, p: f64) -> u32 {
    let s = MSS as f64;
    let f_p = (p*2.0/3.0).sqrt() + 12.0*(p*3.0/8.0).sqrt()*p*(1.0 + 32.0*p*p);
    (s / (rtt * f_p)) as u32
}

// TODO: Test this one in particular
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
                println!("a: {}, b: {}", a, b);
                continue;
            }
        } else if rate < target_rate_bps {
            if target_rate_bps - rate <= delta {
                return c;
            } else {
                b = c;
                println!("a: {}, b: {}", a, b);
                continue;
            }
        } else {
            return c;
        }
    }
}

struct SlowStartState {
    time_last_doubled: time::Instant,
}

struct ThroughputEqnState {
    send_rate_tcp: u32,
}

enum SendRateMode {
    AwaitSend,
    AwaitFeedback,
    SlowStart(SlowStartState),
    ThroughputEqn(ThroughputEqnState),
}

pub struct SendRateComp {
    feedback_comp: feedback::FeedbackComp,

    // Previous loss rate computation
    prev_loss_rate: f64,
    // Last time feedback was handled
    last_feedback_time: Option<time::Instant>,

    // Expiration of nofeedback timer
    nofeedback_exp: Option<time::Instant>,
    // Flag indicating whether sender has been idle since the nofeedback timer was sent
    nofeedback_idle: bool,

    // State used to compute send rate
    send_rate_mode: SendRateMode,
    // Allowed transmit rate (X)
    send_rate: u32,

    // Queue of receive rates reported by receiver (X_recv_set)
    recv_rate_set: RecvRateSet,

    // Round trip time estimate
    rtt_s: Option<f64>,
    rtt_ms: Option<u64>,
}

impl SendRateComp {
    const INITIAL_RATE: u32 = 4380; // std::cmp::min(std::cmp::max(2*MSS, 4380), 4*MSS) as u32;
    const RECOVER_RATE: u32 = Self::INITIAL_RATE;
    const MINIMUM_RATE: u32 = (MSS / 64) as u32;

    const LOSS_INITIAL_RTT_MS: u64 = 100;

    pub fn new(base_id: u32) -> Self {
        Self {
            feedback_comp: feedback::FeedbackComp::new(base_id),

            prev_loss_rate: 0.0,
            last_feedback_time: None,

            nofeedback_exp: None,
            nofeedback_idle: false,

            send_rate_mode: SendRateMode::AwaitSend,
            send_rate: MSS as u32,

            recv_rate_set: RecvRateSet::new(),

            rtt_s: None,
            rtt_ms: None,
        }
    }

    pub fn log_frame(&mut self, frame_id: u32, nonce: bool, size: usize, send_time_ms: u64) {
        self.feedback_comp.log_frame(frame_id, nonce, size, send_time_ms);

        let now = time::Instant::now();

        match self.send_rate_mode {
            SendRateMode::AwaitSend => {
                self.send_rate_mode = SendRateMode::AwaitFeedback;
                self.nofeedback_exp = Some(now + time::Duration::from_millis(2000));
                self.recv_rate_set.reset_initial(now);
            }
            _ => ()
        }

        self.nofeedback_idle = false;
    }

    pub fn log_rate_limited(&mut self) {
        self.feedback_comp.log_rate_limited();
    }

    pub fn acknowledge_frames(&mut self, ack: FrameAck, now_ms: u64) {
        self.feedback_comp.acknowledge_frames(ack, now_ms, self.rtt_ms.unwrap_or(Self::LOSS_INITIAL_RTT_MS));
    }

    pub fn forget_frames(&mut self, thresh_ms: u64) {
        self.feedback_comp.forget_frames(thresh_ms, self.rtt_ms.unwrap_or(Self::LOSS_INITIAL_RTT_MS));
    }

    pub fn step(&mut self) {
        match self.send_rate_mode {
            SendRateMode::AwaitSend => {
                return;
            }
            _ => ()
        }

        let now = time::Instant::now();

        if let Some(pending_feedback) = self.feedback_comp.pending_feedback() {
            println!("pending_feedback: {:?}", pending_feedback);

            let rtt_ms = pending_feedback.rtt_ms as f64 / 1000.0;

            let receive_rate = if let Some(last_feedback_time) = self.last_feedback_time {
                (pending_feedback.total_ack_size as f64 / (now - last_feedback_time).as_secs_f64()).clamp(0.0, u32::MAX as f64) as u32
            } else {
                0
            };

            let loss_rate = pending_feedback.loss_rate;

            let rate_limited = pending_feedback.rate_limited;

            self.handle_feedback(now, rtt_ms, receive_rate, loss_rate, rate_limited);

            self.last_feedback_time = Some(now);

            println!("New send rate: {}", self.send_rate);
        } else if let Some(nofeedback_exp) = self.nofeedback_exp {
            if now >= nofeedback_exp {
                self.nofeedback_expired(now);
            }
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

    fn update_rtt(&mut self, rtt_sample_s: f64) -> f64 {
        const RTT_ALPHA: f64 = 0.1;

        let new_rtt = if let Some(rtt_s) = self.rtt_s {
            (1.0 - RTT_ALPHA)*rtt_s + RTT_ALPHA*rtt_sample_s
        } else {
            rtt_sample_s
        };

        self.rtt_s = Some(new_rtt);
        self.rtt_ms = Some((new_rtt * 1000.0).round().max(0.0) as u64);

        return new_rtt;
    }

    fn handle_feedback(&mut self, now: time::Instant, rtt_sample_s: f64, recv_rate: u32, loss_rate: f64, rate_limited: bool) {
        let rtt_s = self.update_rtt(rtt_sample_s);
        let rto_s = eval_rto_s(rtt_s, self.send_rate);

        let send_rate_limit =
            if rate_limited {
                let max_val = self.recv_rate_set.rate_limited_update(now, recv_rate, rtt_s);
                max_val.saturating_mul(2)
            } else if loss_rate > self.prev_loss_rate {
                let max_val = self.recv_rate_set.loss_increase_update(now, recv_rate);
                max_val
            } else {
                let max_val = self.recv_rate_set.data_limited_update(now, recv_rate);
                max_val.saturating_mul(2)
            };

        self.prev_loss_rate = loss_rate;

        match &mut self.send_rate_mode {
            SendRateMode::AwaitFeedback => {
                if loss_rate == 0.0 {
                    // Enter slow start phase
                    self.send_rate = (Self::INITIAL_RATE as f64 / rtt_s) as u32;

                    self.send_rate_mode = SendRateMode::SlowStart(
                        SlowStartState {
                            time_last_doubled: now,
                        }
                    );
                } else {
                    // Enter throughput equation phase
                    let send_rate_target = ((MSS/2) as f64 / rtt_s) as u32;
                    let initial_p = eval_tcp_throughput_inv(rtt_s, send_rate_target);
                    self.feedback_comp.seed_loss_rate(initial_p);

                    self.send_rate = send_rate_target.min(send_rate_limit).max(Self::MINIMUM_RATE);

                    self.send_rate_mode = SendRateMode::ThroughputEqn(
                        ThroughputEqnState {
                            send_rate_tcp: send_rate_target,
                        }
                    );
                }
            }
            SendRateMode::SlowStart(state) => {
                if loss_rate == 0.0 {
                    // Continue slow start phase
                    let rtt_dur = time::Duration::from_secs_f64(rtt_s);

                    if now - state.time_last_doubled >= rtt_dur {
                        self.send_rate = (2*self.send_rate).min(send_rate_limit).max(Self::INITIAL_RATE);
                        state.time_last_doubled = now;
                    }
                } else {
                    // Enter throughput equation phase
                    let send_rate_target = self.send_rate/2;
                    let initial_p = eval_tcp_throughput_inv(rtt_s, send_rate_target);
                    self.feedback_comp.seed_loss_rate(initial_p);

                    self.send_rate = send_rate_target.min(send_rate_limit).max(Self::MINIMUM_RATE);

                    self.send_rate_mode = SendRateMode::ThroughputEqn(
                        ThroughputEqnState {
                            send_rate_tcp: send_rate_target,
                        }
                    );
                }
            }
            SendRateMode::ThroughputEqn(state) => {
                // Continue throughput equation phase
                state.send_rate_tcp = eval_tcp_throughput(rtt_s, loss_rate);
                self.send_rate = state.send_rate_tcp.min(send_rate_limit).max(Self::MINIMUM_RATE);
            }
            _ => ()
        }

        self.nofeedback_exp = Some(now + time::Duration::from_secs_f64(rto_s));
        self.nofeedback_idle = true;
    }

    fn nofeedback_expired(&mut self, now: time::Instant) {
        match &self.send_rate_mode {
            SendRateMode::AwaitFeedback => {
                // Halve send rate every RTO, subject to minimum
                self.send_rate = (self.send_rate/2).max(Self::MINIMUM_RATE);
            }
            SendRateMode::SlowStart(_) => {
                if self.nofeedback_idle && self.send_rate < 2*Self::RECOVER_RATE {
                    // Do nothing, this is acceptable
                } else {
                    // Halve send rate every RTO, subject to minimum
                    self.send_rate = (self.send_rate/2).max(Self::MINIMUM_RATE);
                }
            }
            SendRateMode::ThroughputEqn(state) => {
                let recv_rate = self.recv_rate_set.max();
                if self.nofeedback_idle && recv_rate < Self::RECOVER_RATE {
                    // Do nothing, this is acceptable
                } else {
                    // Alter recv_rate_set so as to halve current send rate moving forward
                    let current_limit = state.send_rate_tcp.min(recv_rate.saturating_mul(2));
                    let new_limit = (current_limit/2).max(Self::MINIMUM_RATE);
                    self.recv_rate_set.reset(now, new_limit/2);
                    self.send_rate = state.send_rate_tcp.min(new_limit);
                }
            }
            _ => ()
        }

        // Compute RTO for the new send rate
        let rto_s = eval_rto_s(self.rtt_s.unwrap_or(0.0), self.send_rate);

        self.nofeedback_exp = Some(now + time::Duration::from_secs_f64(rto_s));
        self.nofeedback_idle = true;
    }
}

