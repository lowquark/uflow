
use crate::frame;

use super::recv_rate_set;
use super::reorder_buffer;

use std::collections::VecDeque;

use crate::MAX_FRAME_SIZE;



use super::pending_packet::FragmentRef;

#[derive(Debug)]
pub struct FrameEntry {
    pub size: u32,
    pub send_time_ms: u64,
    pub fragment_refs: Box<[FragmentRef]>,
    pub nonce: bool,
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

    pub fn push_frame(&mut self, size: usize, now_ms: u64, fragment_refs: Box<[FragmentRef]>, nonce: bool) {
        debug_assert!(size < u32::MAX as usize);
        debug_assert!(now_ms >= self.frames.back().map_or(0, |frame| frame.send_time_ms));
        debug_assert!(self.frames.len() < u32::MAX as usize);

        self.frames.push_back(FrameEntry {
            size: size as u32,
            send_time_ms: now_ms,
            fragment_refs,
            nonce,
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

pub struct TransferWindow {
    size: u32,
    base_id: u32,
}

impl TransferWindow {
    pub fn new(size: u32, base_id: u32) -> Self {
        Self { size, base_id }
    }

    pub fn contains(&self, next_id: u32) -> bool {
        return next_id.wrapping_sub(self.base_id) < self.size;
    }
}

// -----------------------------------------------------------------------------

// See RFC 5348: "TCP Friendly Rate Control (TFRC): Protocol Specification"

#[derive(Debug)]
struct LossInterval {
    end_time_ms: u64,
    length: u32,
}

#[derive(Debug)]
struct LossIntervalQueue {
    entries: VecDeque<LossInterval>,
}

impl LossIntervalQueue {
    const WEIGHTS: [f64; 8] = [ 1.0, 1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.2 ];

    pub fn new() -> Self {
        Self {
            entries: VecDeque::new()
        }
    }

    pub fn reset(&mut self, initial_p: f64, end_time_ms: u64) {
        // Because the loss interval queue is reset here, the particular loss pattern of the first
        // feedback message is ignored. That is, if frames were acked/nacked as follows:
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

        self.entries.clear();

        self.entries.push_front(LossInterval {
            end_time_ms,
            length: (Self::WEIGHTS[0] / initial_p).clamp(0.0, u32::MAX as f64).round() as u32,
        });
    }

    pub fn put_ack(&mut self) {
        if let Some(last_interval) = self.entries.front_mut() {
            // Acks always contribute to previous loss interval
            last_interval.length = last_interval.length.saturating_add(1);
        }
    }

    pub fn put_nack(&mut self, send_time_ms: u64, rtt_ms: u64) {
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
        }
    }

    pub fn compute_loss_rate(&self) -> f64 {
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

// -----------------------------------------------------------------------------

pub struct AckData {
    last_send_time_ms: u64,
    total_ack_size: usize,
    nack_found: bool,
}

pub struct FeedbackData {
    rtt_ms: u64,
    receive_rate: u32,
    loss_rate: f64,
    nack_found: bool,
}

pub struct FeedbackComp {
    // Last time feedback was handled
    last_feedback_ms: Option<u64>,

    // Aggregated feedback data from latest ack frames
    ack_data: Option<AckData>,

    // Determines when frames are considered dropped (NDUPACK = 3)
    reorder_buffer: reorder_buffer::ReorderBuffer,

    // Used to compute the receiver loss rate
    loss_intervals: LossIntervalQueue,

    // Used to determine whether the sender was rate-limited
    rate_limited_log: VecDeque<u32>,
    last_frame_id: u32,
}

impl FeedbackComp {
    pub fn new(base_id: u32) -> Self {
        Self {
            last_feedback_ms: None,

            ack_data: None,
            reorder_buffer: reorder_buffer::ReorderBuffer::new(base_id),
            loss_intervals: LossIntervalQueue::new(),

            rate_limited_log: VecDeque::new(),
            last_frame_id: base_id,
        }
    }

    pub fn put_ack_data(&mut self, last_send_time_ms: u64, total_ack_size: usize) {
        // Add to pending feedback data
        if let Some(ref mut feedback_data) = self.ack_data {
            feedback_data.last_send_time_ms = feedback_data.last_send_time_ms.max(last_send_time_ms);
            feedback_data.total_ack_size += total_ack_size;
        } else {
            self.ack_data = Some(AckData { last_send_time_ms, total_ack_size, nack_found: false });
        }
    }

    pub fn get_feedback(&mut self, now_ms: u64) -> Option<FeedbackData> {
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

            let nack_found = ack_data.nack_found;

            return Some(FeedbackData { rtt_ms, receive_rate, loss_rate, nack_found });
        }

        return None;
    }
}

// -----------------------------------------------------------------------------

// Segment size in bytes (s), see section 
const MSS: usize = MAX_FRAME_SIZE;

// Initial TCP window size (based on MSS), see section 4.2
const INITIAL_TCP_WINDOW: u32 = 4380; // std::cmp::min(std::cmp::max(2*MSS, 4380), 4*MSS) as u32;
// Absolute minimum send rate (s/t_mbi), see section 4.3
const MINIMUM_RATE: u32 = (MSS / 64) as u32;

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

fn s_to_ms(v_s: f64) -> u64 {
    (v_s * 1000.0).max(0.0).round() as u64
}

fn ms_to_s(v_s: u64) -> f64 {
    v_s as f64 / 1000.0
}

fn compute_initial_send_rate(rtt_s: f64) -> u32 {
    (INITIAL_TCP_WINDOW as f64 / rtt_s) as u32
}

fn compute_initial_loss_send_rate(rtt_s: f64) -> u32 {
    ((MSS/2) as f64 / rtt_s) as u32
}

impl SendRateComp {
    pub fn new(base_id: u32, max_send_rate: u32) -> Self {
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

    pub fn notify_frame_sent(&mut self, rate_limited: bool) {
    }

    pub fn notify_ack(&mut self, frame_id: u32, frame_log: &FrameLog) {
    }

    pub fn notify_advance(&mut self, new_base_id: u32, frame_log: &FrameLog) {
    }

    pub fn step(&mut self, now_ms: u64, fb_comp: &mut FeedbackComp) {
        match self.mode {
            SendRateMode::AwaitSend => {
                return;
            }
            _ => ()
        }

        if let Some(feedback) = fb_comp.get_feedback(now_ms) {
            let ref mut loss_intervals = fb_comp.loss_intervals;

            self.handle_feedback(now_ms, feedback,
                |new_loss_rate: f64, end_time_ms: u64| {
                    loss_intervals.reset(new_loss_rate, end_time_ms);
                }
            );
        } else if let Some(nofeedback_exp_ms) = self.nofeedback_exp_ms {
            if now_ms >= nofeedback_exp_ms {
                self.nofeedback_expired(now_ms);
            }
        }
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

    fn handle_feedback<F>(&mut self, now_ms: u64, feedback: FeedbackData, reset_loss_rate: F) where F: FnOnce(f64, u64) {
        let rtt_sample_s = ms_to_s(feedback.rtt_ms);
        let recv_rate = feedback.receive_rate;
        let loss_rate = feedback.loss_rate;
        let nack_found = feedback.nack_found;

        let (rtt_s, rtt_ms) = self.update_rtt(rtt_sample_s);
        let rto_s = self.update_rto(rtt_s, self.send_rate);

        let rate_limited = false;

        // TODO: When ThroughputEqn is entered, this may produce a false positive depending on
        // rounding rounding error as the loss intervals are reset. An extra flag may be of use.
        let loss_increase = loss_rate > self.prev_loss_rate;

        let send_rate_limit =
            if rate_limited {
                let max_val = self.recv_rate_set.rate_limited_update(now_ms, recv_rate, rtt_ms);
                max_val.saturating_mul(2)
            } else if loss_increase || nack_found {
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
            SendRateMode::SlowStart(ref mut state) => {
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
}

// -----------------------------------------------------------------------------

pub fn handle_advancement(new_base_id: u32, frame_log: &FrameLog, feedback: &mut FeedbackComp) {
    let ref mut loss_intervals = feedback.loss_intervals;
    feedback.reorder_buffer.advance(new_base_id, |frame_id, was_seen| {
        let sent_frame = frame_log.get_frame(frame_id).unwrap();
        if was_seen {
            loss_intervals.put_ack();
        } else {
            loss_intervals.put_nack(sent_frame.send_time_ms, 100);
        }
    })
}

pub fn handle_ack(frame_id: u32, frame_log: &FrameLog, feedback: &mut FeedbackComp) {
    let base_id = frame_log.base_id();

    let thresh_id_delta = feedback.reorder_buffer.next_id().wrapping_sub(base_id);
    let frame_id_delta = frame_id.wrapping_sub(base_id);

    if frame_id_delta >= thresh_id_delta {
        // New frame
        let ref mut loss_intervals = feedback.loss_intervals;
        feedback.reorder_buffer.put(frame_id, |frame_id, was_seen| {
            let sent_frame = frame_log.get_frame(frame_id).unwrap();
            if was_seen {
                loss_intervals.put_ack();
            } else {
                loss_intervals.put_nack(sent_frame.send_time_ms, 100);
            }
        });
    }
}

pub fn handle_ack_group(ack: frame::FrameAck, frame_log: &mut FrameLog, feedback: &mut FeedbackComp) {
    let mut true_nonce = false;

    let mut last_send_time_ms = 0;
    let mut total_ack_size = 0;

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

        if let Some(ref sent_frame) = frame_log.get_frame(frame_id) {
            if ack.bitfield & (1 << i) != 0 {
                // Receiver claims to have received this packet
                true_nonce ^= sent_frame.nonce;
                last_send_time_ms = last_send_time_ms.max(sent_frame.send_time_ms);
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
        if ack.bitfield & (1 << i) != 0 {
            // Receiver has received this packet
            let frame_id = ack.base_id.wrapping_add(i);

            if let Some(ref mut sent_frame) = frame_log.get_frame_mut(frame_id) {
                if sent_frame.acked == false {
                    sent_frame.acked = true;

                    // Ack each fragment and clear the list
                    let fragment_refs = std::mem::take(&mut sent_frame.fragment_refs);

                    for fragment_ref in fragment_refs.into_iter() {
                        // TODO: This could be a method?
                        if let Some(packet_rc) = fragment_ref.packet.upgrade() {
                            let mut packet_ref = packet_rc.borrow_mut();
                            packet_ref.acknowledge_fragment(fragment_ref.fragment_id);
                        }
                    }

                    // Add to total ack size
                    total_ack_size += sent_frame.size as usize;

                    // TODO: Callback here
                    handle_ack(frame_id, frame_log, feedback);
                }
            }
        }
    }

    feedback.put_ack_data(last_send_time_ms, total_ack_size);
}

fn advance_transfer_window(new_base_id: u32, transfer_window: &mut TransferWindow, frame_log: &mut FrameLog, fb_comp: &mut FeedbackComp) {
    let log_next_id = frame_log.next_id();
    let log_base_id = frame_log.base_id();
    let window_base_id = transfer_window.base_id;

    // Ensure transfer window never backtracks and never advances beyond frame log's next_id
    let next_delta = log_next_id.wrapping_sub(window_base_id);
    let delta = new_base_id.wrapping_sub(window_base_id);

    if delta == 0 || delta > next_delta {
        return;
    }

    // Advance transfer window
    transfer_window.base_id = new_base_id;

    // Prune expired frame log entries, purge corresponding IDs
    let min_base_id = new_base_id.wrapping_sub(transfer_window.size);

    let min_base_delta = min_base_id.wrapping_sub(log_base_id);
    let log_len = log_next_id.wrapping_sub(log_base_id);

    if min_base_delta > 0 && min_base_delta <= log_len {
        // [log_base_id, min_base_id) will be removed
        handle_advancement(min_base_id, frame_log, fb_comp);

        frame_log.drain(min_base_id);
        debug_assert!(frame_log.base_id() == min_base_id);
    }
}

pub fn ack_frames(ack: frame::AckFrame, transfer_window: &mut TransferWindow, frame_log: &mut FrameLog, fb_comp: &mut FeedbackComp) {
    for ack_group in ack.frame_acks.into_iter() {
        handle_ack_group(ack_group, frame_log, fb_comp);
    }

    let receiver_base_id = 0;
    advance_transfer_window(receiver_base_id, transfer_window, frame_log, fb_comp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pending_packet::PendingPacket;

    use std::rc::Rc;
    use std::cell::RefCell;

    /*
    #[test]
    fn window_advancement() {
        let mut tq = FrameLog::new(4, 3, 0);

        let packet_rc = Rc::new(RefCell::new(
            PendingPacket::new(vec![ 0x00, 0x01, 0x02].into_boxed_slice(), 0, 0, 0, 0)
        ));

        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());

        assert_eq!(tq.can_put_frame(), false);

        tq.advance_receiver_base_id(4);
        assert_eq!(tq.receiver_base_id, 4);

        assert_eq!(tq.can_put_frame(), true);

        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());

        assert_eq!(tq.can_put_frame(), false);

        assert_eq!(tq.frames.len(), 7);
        assert_eq!(tq.base_id, 1);
        assert_eq!(tq.next_id, 8);

        tq.advance_receiver_base_id(8);
        assert_eq!(tq.receiver_base_id, 8);

        assert_eq!(tq.frames.len(), 3);
        assert_eq!(tq.base_id, 5);
        assert_eq!(tq.next_id, 8);
    }
    */

    /*
    #[test]
    fn ack_data_aggregation() {
        let mut ast = AckState::new();
        let mut tq = FrameLog::new(8, 8, 0);

        let packet_rc = Rc::new(RefCell::new(
            PendingPacket::new(vec![ 0x00, 0x01, 0x02].into_boxed_slice(), 0, 0, 0, 0)
        ));

        let (_, n0) = tq.put_frame(  1, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_,  _) = tq.put_frame(  2, 1, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_, n2) = tq.put_frame(  4, 2, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_, n3) = tq.put_frame(  8, 3, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.log_rate_limited();
        let (_, n4) = tq.put_frame( 16, 4, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_, n5) = tq.put_frame( 32, 5, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.log_rate_limited();

        let callback = |_frame_id: u32| { };

        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 0, bitfield: 0b101, nonce: n0 ^ n2 }, callback);

        assert_eq!(ast.poll(), Some(AckData { size: 5, send_time_ms: 2, rate_limited: false }));

        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 2, bitfield: 0b11, nonce: n2 ^ n3 }, callback);

        assert_eq!(ast.poll(), Some(AckData { size: 8, send_time_ms: 3, rate_limited: false }));

        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 5, bitfield: 0b1, nonce: n5 }, callback);
        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 4, bitfield: 0b1, nonce: n4 }, callback);

        assert_eq!(ast.poll(), Some(AckData { size: 48, send_time_ms: 5, rate_limited: true }));
    }
    */
}

