
use std::collections::VecDeque;
use std::time;

use super::frame;
use super::DataSink;
use super::MTU;

#[derive(Debug)]
struct SendEntry {
    data: Option<frame::DataEntry>,
    reliable: bool,
}

impl SendEntry {
    fn new(data: frame::DataEntry, reliable: bool) -> Self {
        Self {
            data: Some(data),
            reliable: reliable,
        }
    }
}

struct SendQueue {
    high_priority: VecDeque<SendEntry>,
    low_priority: VecDeque<SendEntry>,
}

impl SendQueue {
    pub fn new() -> Self {
        Self {
            high_priority: VecDeque::new(),
            low_priority: VecDeque::new(),
        }
    }

    pub fn push_high_priority(&mut self, entry: SendEntry) {
        self.high_priority.push_back(entry);
    }

    pub fn push_low_priority(&mut self, entry: SendEntry) {
        self.low_priority.push_back(entry);
    }

    pub fn is_empty(&self) -> bool {
        self.high_priority.is_empty() && self.low_priority.is_empty()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut SendEntry> {
        self.high_priority.iter_mut().chain(self.low_priority.iter_mut())
    }

    pub fn retain<F>(&mut self, f: F) where F: FnMut(&SendEntry) -> bool + Copy {
        self.high_priority.retain(f);
        self.low_priority.retain(f);
    }
}

#[derive(Debug)]
struct ResendEntry {
    frame_data: Box<[u8]>,
    sequence_id: u32,
    last_send_time: time::Instant,
    send_count: u32,
}

impl ResendEntry {
    fn new(frame_data: Box<[u8]>, last_send_time: time::Instant, sequence_id: u32) -> Self {
        Self {
            frame_data: frame_data,
            sequence_id: sequence_id,
            last_send_time: last_send_time,
            send_count: 1,
        }
    }

    fn mark_sent(&mut self, now: time::Instant) {
        self.last_send_time = now;
        self.send_count += 1;
        if self.send_count > 10 {
            self.send_count = 0;
        }
    }

    fn should_resend(&self, now: time::Instant, timeout: time::Duration) -> bool {
        now - self.last_send_time > timeout*self.send_count
    }
}

struct LeakyBucket {
    alloc: usize,
    alloc_max: usize,
    byte_rate: usize,
    last_step_time: Option<time::Instant>,
}

impl LeakyBucket {
    const MIN_ALLOC_MAX: usize = MTU;

    fn new(byte_rate: usize, alloc_max: usize) -> Self {
        Self {
            alloc: 0,
            alloc_max: alloc_max.max(Self::MIN_ALLOC_MAX),
            byte_rate: byte_rate,
            last_step_time: None,
        }
    }

    fn step(&mut self, now: time::Instant) {
        if let Some(last_step_time) = self.last_step_time {
            let delta_time = (now - last_step_time).as_secs_f64();
            let delta_bytes = ((self.byte_rate as f64)*delta_time).round() as usize;
            self.alloc = std::cmp::min(self.alloc + delta_bytes, self.alloc_max);
        }

        self.last_step_time = Some(now);
    }

    fn bytes_remaining(&self) -> usize {
        self.alloc
    }

    fn mark_sent(&mut self, frame_size: usize) {
        assert!(self.alloc >= frame_size);
        self.alloc -= frame_size;
    }
}

struct AimdBucket {
    alloc: usize,
    size: usize,
    max_size: usize,
}

impl AimdBucket {
    // TODO: Consult RFC for TCP Reno?
    // TODO: Slow start?
    const ACK_INCREASE: usize = MTU;
    const NACK_DECREASE: f64 = 0.5;
    const MIN_SIZE: usize = MTU;

    fn new(max_size: usize) -> Self {
        Self {
            alloc: Self::MIN_SIZE,
            size: Self::MIN_SIZE,
            max_size: max_size.max(Self::MIN_SIZE),
        }
    }

    fn trigger_ack(&mut self, frame_size: usize) {
        self.alloc += frame_size;
        self.size += Self::ACK_INCREASE;
        if self.size > self.max_size {
            self.size = self.max_size;
        }
        self.alloc += Self::ACK_INCREASE;
        if self.alloc > self.size {
            self.alloc = self.size;
        }
    }

    fn trigger_nack(&mut self) {
        self.size = ((self.size as f64) * Self::NACK_DECREASE).round() as usize;
        if self.size < Self::MIN_SIZE {
            self.size = Self::MIN_SIZE;
        }
        if self.alloc > self.size {
            self.alloc = self.size;
        }
    }

    fn bytes_remaining(&self) -> usize {
        self.alloc
    }

    fn mark_sent(&mut self, frame_size: usize) {
        assert!(self.alloc >= frame_size);
        self.alloc -= frame_size;
    }
}

pub struct FrameIO {
    send_queue: SendQueue,
    resend_queue: VecDeque<ResendEntry>,
    ack_queue: VecDeque<u32>,
    next_sequence_id: u32,
    base_sequence_id: u32,

    bandwidth_throttle: LeakyBucket,
    reliable_throttle: AimdBucket,
}

impl FrameIO {
    const TRANSFER_WINDOW_SIZE: u32 = 2048;

    pub fn new() -> Self {
        Self {
            send_queue: SendQueue::new(),
            resend_queue: VecDeque::new(),
            ack_queue: VecDeque::new(),
            next_sequence_id: 0,
            base_sequence_id: 0,

            bandwidth_throttle: LeakyBucket::new(100_000, 3_000),
            reliable_throttle: AimdBucket::new(100_000),
        }
    }

    pub fn enqueue_datagram(&mut self, data: frame::DataEntry, reliable: bool, high_priority: bool) {
        if high_priority {
            self.send_queue.push_high_priority(SendEntry::new(data, reliable));
        } else {
            self.send_queue.push_low_priority(SendEntry::new(data, reliable));
        }
    }

    pub fn acknowledge_data_frame(&mut self, data: &frame::Data) {
        if data.ack {
            self.ack_queue.push_back(data.sequence_id);
        }
    }

    pub fn handle_data_ack(&mut self, data_ack: frame::DataAck) {
        // TODO: At least binary search, man
        for sequence_id in data_ack.sequence_ids.into_iter() {
            'inner: for (idx, entry) in self.resend_queue.iter().enumerate() {
                if entry.sequence_id == sequence_id {
                    self.reliable_throttle.trigger_ack(entry.frame_data.len());
                    self.resend_queue.remove(idx);

                    if let Some(entry) = self.resend_queue.front() {
                        self.base_sequence_id = entry.sequence_id;
                    } else {
                        self.base_sequence_id = sequence_id.wrapping_add(1);
                    }

                    break 'inner;
                }
            }
        }
    }

    fn try_send_acks(&mut self, sink: & dyn DataSink) -> Result<(),()> {
        while !self.ack_queue.is_empty() {
            let max_size = std::cmp::min(MTU, self.bandwidth_throttle.bytes_remaining());

            if max_size >= frame::DataAck::HEADER_SIZE_BYTES + frame::DataAck::SEQUENCE_ID_SIZE_BYTES {
                let max_ids = (max_size - frame::DataAck::HEADER_SIZE_BYTES)/frame::DataAck::SEQUENCE_ID_SIZE_BYTES;

                let sequence_ids = self.ack_queue.drain(..usize::min(max_ids, self.ack_queue.len())).collect();
                let frame = frame::Frame::DataAck(frame::DataAck {
                    sequence_ids: sequence_ids,
                });

                let frame_data = frame.to_bytes();
                sink.send(&frame_data);
                self.bandwidth_throttle.mark_sent(frame_data.len());
            } else {
                return Err(());
            }
        }
        return Ok(());
    }

    fn try_send_resends(&mut self, now: time::Instant, timeout: time::Duration, sink: & dyn DataSink) -> Result<(),()> {
        // TODO: Efficient iteration
        for entry in self.resend_queue.iter_mut() {
            if entry.should_resend(now, timeout) {
                if self.bandwidth_throttle.bytes_remaining() >= entry.frame_data.len() {
                    sink.send(&entry.frame_data);
                    entry.mark_sent(now);
                    self.bandwidth_throttle.mark_sent(entry.frame_data.len());
                    self.reliable_throttle.trigger_nack();
                } else {
                    // Wait until we have enough bandwidth tokens
                    return Err(());
                }
            }
        }
        return Ok(());
    }

    fn try_send_data(&mut self, now: time::Instant, sink: & dyn DataSink) {
        // Assembles and sends as many frames as possible, with datagrams taken from the send queue
        // in order, subject to the frame size limit, a total bandwidth constraint, a reliable
        // congestion window, and the maximum sequence id transfer window, ensuring that
        // inter-packet-wise, no reliable datagrams are sent prior to any preceding datagrams. All
        // unreliable datagrams are removed from the send queue, whether or not they've been sent.
        //
        // All entries in the send queue must have an encoded size such that they may be stored in
        // a frame satisfying the MTU (i.e. encoded_size <= MTU - HEADER_SIZE).

        let frame_overhead_bytes = frame::Data::HEADER_SIZE_BYTES;
        let frame_limit_bytes = MTU;

        let mut total_bytes_remaining = self.bandwidth_throttle.bytes_remaining();
        let mut reliable_bytes_remaining = self.reliable_throttle.bytes_remaining();

        let end_sequence_id = self.base_sequence_id.wrapping_add(Self::TRANSFER_WINDOW_SIZE);

        {
            let mut entry_iter = self.send_queue.iter_mut();
            let mut entry_kv = entry_iter.next();

            let mut permit_reliable = true;

            // Assemble as many data frames as possible
            while entry_kv.is_some() {
                let mut frame_size = frame_overhead_bytes;
                let mut frame_size_reliable = None;
                let mut frame_sequence_id = None;

                let mut rel_dgs = Vec::new();
                let mut unrel_dgs = Vec::new();

                // Add as many datagrams (send entries) as possible
                while let Some(entry) = entry_kv.as_mut() {
                    if entry.reliable && !permit_reliable {
                        // A previous reliable datagram would have exceeded the reliable congestion window
                        entry_kv = entry_iter.next();
                        continue;
                    }

                    let encoded_size = entry.data.as_ref().unwrap().encoded_size();

                    if entry.reliable {
                        let hyp_frame_size_reliable = frame_size_reliable.unwrap_or(frame_overhead_bytes) + encoded_size;

                        if hyp_frame_size_reliable > reliable_bytes_remaining {
                            // Would be too large for reliable congestion window, stop considering reliable packets
                            permit_reliable = false;
                            entry_kv = entry_iter.next();
                            continue;
                        }

                        let hyp_sequence_id = self.next_sequence_id;
                        if hyp_sequence_id == end_sequence_id {
                            // Would not have a valid sequence id, stop considering reliable packets
                            permit_reliable = false;
                            entry_kv = entry_iter.next();
                            continue;
                        }
                    }

                    let hyp_frame_size = frame_size + encoded_size;

                    if hyp_frame_size > total_bytes_remaining {
                        // Would be too large for bandwidth window, assemble and stop
                        entry_kv = None;
                        break;
                    }

                    // This datagram alone must not exceed the frame limit, or we will loop forever!
                    assert!(frame_overhead_bytes + encoded_size <= frame_limit_bytes);

                    if hyp_frame_size > frame_limit_bytes {
                        // Would be too large for this frame, assemble and continue
                        break;
                    }

                    // Verification complete, add datagram to this frame

                    let data = entry.data.take();
                    if entry.reliable {
                        rel_dgs.push(data.unwrap());
                    } else {
                        unrel_dgs.push(data.unwrap());
                    }

                    frame_size += encoded_size;
                    if entry.reliable {
                        if let Some(size) = frame_size_reliable {
                            // Subsequent reliable datagram added
                            frame_size_reliable = Some(size + encoded_size);
                        } else {
                            // First reliable datagram added, acquire sequence id
                            frame_size_reliable = Some(frame_overhead_bytes + encoded_size);
                            frame_sequence_id = Some(self.next_sequence_id);
                            self.next_sequence_id = self.next_sequence_id.wrapping_add(1);
                        };
                    }

                    entry_kv = entry_iter.next();
                }

                // Frame complete!

                if rel_dgs.len() != 0 || unrel_dgs.len() != 0 {
                    total_bytes_remaining -= frame_size;
                    if let Some(size) = frame_size_reliable {
                        reliable_bytes_remaining -= size;
                    }

                    self.bandwidth_throttle.mark_sent(frame_size);

                    if rel_dgs.len() == 0 && unrel_dgs.len() > 0 {
                        // Send simple frame containing only unreliable datagrams
                        let needs_ack = false;
                        let send_frame = frame::Data::new(needs_ack, 0, unrel_dgs);
                        let frame_data = send_frame.to_bytes();
                        assert!(frame_data.len() == frame_size);
                        sink.send(&frame_data);
                    } else if rel_dgs.len() > 0 && unrel_dgs.len() == 0 {
                        // Send & save simple frame containing only reliable datagrams
                        let needs_ack = true;
                        let seq_id = frame_sequence_id.unwrap();
                        let resend_frame = frame::Data::new(needs_ack, seq_id, rel_dgs);
                        let frame_data = resend_frame.to_bytes();
                        assert!(frame_data.len() == frame_size);
                        assert!(frame_data.len() == frame_size_reliable.unwrap());
                        sink.send(&frame_data);
                        self.reliable_throttle.mark_sent(frame_data.len());
                        self.resend_queue.push_back(ResendEntry::new(frame_data, now, seq_id));
                    } else if rel_dgs.len() > 0 && unrel_dgs.len() > 0 {
                        let needs_ack = rel_dgs.len() > 0;
                        let seq_id = frame_sequence_id.unwrap();
                        if rel_dgs.len() > 0 {
                            // Save frame containing reliable datagrams
                            let resend_frame = frame::Data::new(needs_ack, seq_id, rel_dgs);
                            let frame_data = resend_frame.to_bytes();
                            assert!(frame_data.len() == frame_size_reliable.unwrap());
                            self.reliable_throttle.mark_sent(frame_data.len());
                            self.resend_queue.push_back(ResendEntry::new(frame_data, now, seq_id));
                            // Take back the reliable datagrams
                            rel_dgs = resend_frame.entries;
                        }
                        // Send frame containing all datagrams
                        let mut all_dgs = rel_dgs;
                        all_dgs.append(&mut unrel_dgs);
                        let send_frame = frame::Data::new(needs_ack, seq_id, all_dgs);
                        let frame_data = send_frame.to_bytes();
                        assert!(frame_data.len() == frame_size);
                        sink.send(&frame_data);
                    }
                }
            }
        }

        // Retain reliable entries which still have data
        self.send_queue.retain(|entry| entry.data.is_some() && entry.reliable == true);
    }

    pub fn step(&mut self, now: time::Instant) {
        self.bandwidth_throttle.step(now);
    }

    pub fn flush(&mut self, now: time::Instant, timeout: time::Duration, sink: & dyn DataSink) {
        if self.try_send_acks(sink).is_err() {
            return;
        }
        if self.try_send_resends(now, timeout, sink).is_err() {
            return;
        }
        self.try_send_data(now, sink);
    }

    pub fn is_idle(&self) -> bool {
        self.send_queue.is_empty() && self.resend_queue.is_empty()
    }
}

