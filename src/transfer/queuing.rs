
use std::collections::VecDeque;
use std::time;

use super::frame;
use super::DataSink;
use super::MTU;
use super::TRANSFER_WINDOW_SIZE;

const SENTINEL_FRAME_SPACING: u32 = TRANSFER_WINDOW_SIZE/2;

#[derive(Debug)]
struct SendEntry {
    data: frame::DataEntry,
    reliable: bool,
}

impl SendEntry {
    fn new(data: frame::DataEntry, reliable: bool) -> Self {
        Self {
            data: data,
            reliable: reliable,
        }
    }
}

pub struct SendQueue {
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

    pub fn push(&mut self, data: frame::DataEntry, reliable: bool, high_priority: bool) {
        debug_assert!(data.encoded_size() <= MTU - frame::Data::HEADER_SIZE_BYTES);

        let entry = SendEntry::new(data, reliable);

        if high_priority {
            self.high_priority.push_back(entry);
        } else {
            self.low_priority.push_back(entry);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.high_priority.is_empty() && self.low_priority.is_empty()
    }

    fn front(&self) -> Option<&SendEntry> {
        if self.high_priority.len() > 0 {
            self.high_priority.front()
        } else {
            self.low_priority.front()
        }
    }

    fn pop_front(&mut self) -> Option<SendEntry> {
        if self.high_priority.len() > 0 {
            self.high_priority.pop_front()
        } else {
            self.low_priority.pop_front()
        }
    }

    fn retain<F>(&mut self, f: F) where F: FnMut(&SendEntry) -> bool + Copy {
        self.high_priority.retain(f);
        self.low_priority.retain(f);
    }
}

struct CongestionWindow {
    size: usize,
}

impl CongestionWindow {
    // TODO: Slow start & congestion avoidance modes
    // TODO: Max size a function of max bandwidth and RTT
    const ACK_INCREASE: usize = MTU;
    const NACK_DECREASE: f64 = 0.5;
    const MIN_SIZE: usize = MTU;
    const MAX_SIZE: usize = 1024*1024*1024;

    fn new() -> Self {
        Self {
            size: Self::MIN_SIZE,
        }
    }

    fn signal_ack(&mut self) {
        self.size += Self::ACK_INCREASE;
        if self.size > Self::MAX_SIZE {
            self.size = Self::MAX_SIZE;
        }
    }

    fn signal_nack(&mut self) {
        self.size = ((self.size as f64) * Self::NACK_DECREASE).round() as usize;
        if self.size < Self::MIN_SIZE {
            self.size = Self::MIN_SIZE;
        }
    }

    fn size(&self) -> usize {
        self.size
    }
}

#[derive(Debug)]
struct TransferEntry {
    resend_frame: Option<Box<[u8]>>,
    unreliable_size: usize,
    sequence_id: u32,
    last_send_time: time::Instant,
    send_count: u32,
    remove: bool,
}

impl TransferEntry {
    fn new(resend_frame: Option<Box<[u8]>>, unreliable_size: usize, sequence_id: u32, send_time: time::Instant) -> Self {
        Self {
            resend_frame: resend_frame,
            unreliable_size: unreliable_size,
            sequence_id: sequence_id,
            last_send_time: send_time,
            send_count: 1,
            remove: false,
        }
    }

    fn new_sentinel(sequence_id: u32, send_time: time::Instant) -> Self {
        Self {
            resend_frame: Some(frame::Frame::Data(frame::Data::new(true, sequence_id, Vec::new())).to_bytes()),
            unreliable_size: 0,
            sequence_id: sequence_id,
            last_send_time: send_time,
            send_count: 1,
            remove: false,
        }
    }

    fn size(&self) -> usize {
        self.unreliable_size + match &self.resend_frame {
            Some(frame) => frame.len(),
            None => 0,
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

pub struct TransferQueue {
    congestion_window: CongestionWindow,
    entries: VecDeque<TransferEntry>,
    size: usize,
    next_sequence_id: u32,
    base_sequence_id: u32,
}

impl TransferQueue {
    pub fn new(tx_sequence_id: u32) -> Self {
        Self {
            congestion_window: CongestionWindow::new(),
            entries: VecDeque::new(),
            size: 0,
            next_sequence_id: tx_sequence_id,
            base_sequence_id: tx_sequence_id,
        }
    }

    fn push_frame(&mut self, entry: TransferEntry) {
        assert!(entry.sequence_id.wrapping_sub(self.base_sequence_id) < TRANSFER_WINDOW_SIZE);
        assert!(self.size + entry.size() <= self.congestion_window.size());

        self.size += entry.size();
        self.entries.push_back(entry);
    }

    pub fn send_frame(&mut self, rel_dgs: Vec<frame::DataEntry>, unrel_dgs: Vec<frame::DataEntry>, now: time::Instant, sink: & dyn DataSink) {
        if rel_dgs.len() > 0 || unrel_dgs.len() > 0 {
            let sequence_id = self.next_sequence_id;
            self.next_sequence_id = self.next_sequence_id.wrapping_add(1);

            if rel_dgs.len() == 0 && unrel_dgs.len() > 0 {
                // Frame containing only unreliable datagrams
                let frame_data = frame::Data::new(false, sequence_id, unrel_dgs).to_bytes();
                sink.send(&frame_data);

                // Resend nothing later, subtract from flight size on nack
                self.push_frame(TransferEntry::new(None, frame_data.len(), sequence_id, now));
            } else if rel_dgs.len() > 0 && unrel_dgs.len() == 0 {
                // Frame containing only reliable datagrams
                let resend_frame_data = frame::Data::new(true, sequence_id, rel_dgs).to_bytes();
                sink.send(&resend_frame_data);

                // Resend frame later, subtract nothing on nack
                self.push_frame(TransferEntry::new(Some(resend_frame_data), 0, sequence_id, now));
            } else if rel_dgs.len() > 0 && unrel_dgs.len() > 0 {
                // Save a frame containing only reliable datagrams
                let resend_frame = frame::Data::new(true, sequence_id, rel_dgs);
                let resend_frame_data = resend_frame.to_bytes();
                let resend_frame_data_len = resend_frame_data.len();

                // Take back the reliable datagrams and assemble frame containing all datagrams
                let mut all_dgs = resend_frame.entries;
                let mut unrel_dgs = unrel_dgs;
                all_dgs.append(&mut unrel_dgs);

                // Send combined frame now
                let frame_data = frame::Data::new(true, sequence_id, all_dgs).to_bytes();
                sink.send(&frame_data);

                // Resend reliable frame later, subtract difference on nack
                self.push_frame(TransferEntry::new(Some(resend_frame_data), frame_data.len() - resend_frame_data_len, sequence_id, now));
            }
        }
    }

    pub fn acknowledge_frame(&mut self, sequence_id: u32) {
        if sequence_id.wrapping_sub(self.base_sequence_id) >= TRANSFER_WINDOW_SIZE {
            // Not here
            return;
        }

        // TODO: Binary search!
        for (idx, entry) in self.entries.iter_mut().enumerate() {
            if entry.sequence_id == sequence_id {
                self.size -= entry.size();
                self.entries.remove(idx);

                // TODO: This is effectively slow start. In congestion avoidance, multiple acks
                // within 1 RTT should only increase the window once.
                self.congestion_window.signal_ack();

                if let Some(entry) = self.entries.front() {
                    self.base_sequence_id = entry.sequence_id;
                } else {
                    self.base_sequence_id = sequence_id.wrapping_add(1);
                }

                return;
            }
        }
    }

    pub fn flush(&mut self, now: time::Instant, timeout: time::Duration, sink: & dyn DataSink) {
        let mut cumulative_size = 0;

        for entry in self.entries.iter_mut() {
            if cumulative_size + entry.size() <= self.congestion_window.size() {
                if entry.should_resend(now, timeout) {
                    // TODO: Multiple timeouts within 1 RTT should only decrease the window once
                    self.congestion_window.signal_nack();

                    self.size -= entry.unreliable_size;
                    entry.unreliable_size = 0;

                    if entry.resend_frame.is_none() {
                        if entry.sequence_id % SENTINEL_FRAME_SPACING == SENTINEL_FRAME_SPACING - 1 {
                            *entry = TransferEntry::new_sentinel(entry.sequence_id, entry.last_send_time);
                            self.size += entry.size();
                        } else {
                            entry.remove = true;
                        }
                    }

                    if let Some(ref resend_frame) = entry.resend_frame {
                        sink.send(resend_frame);
                        entry.mark_sent(now);
                    }
                }

                if !entry.remove {
                    cumulative_size += entry.size();
                }
            }
        }

        self.entries.retain(|entry| !entry.remove);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn free_space(&self) -> usize {
        if self.congestion_window.size() > self.size {
            self.congestion_window.size() - self.size
        } else {
            0
        }
    }

    pub fn can_send(&self) -> bool {
        self.next_sequence_id.wrapping_sub(self.base_sequence_id) < TRANSFER_WINDOW_SIZE
    }
}

pub fn send_new_data(send_queue: &mut SendQueue, transfer_queue: &mut TransferQueue, now: time::Instant, sink: & dyn DataSink) {
    // Assembles and sends as many frames as possible, with datagrams taken from the send queue
    // in order, subject to the frame size limit, the congestion window, and the sequence id
    // transfer window. All unreliable datagrams are removed from the send queue, whether or
    // not they've been sent.
    //
    // All entries in the send queue must have an encoded size such that they may be stored in
    // a frame satisfying the MTU (i.e. encoded_size <= MTU - HEADER_SIZE).

    let frame_overhead_bytes = frame::Data::HEADER_SIZE_BYTES;
    let frame_limit_bytes = MTU;

    // Total new congestion window bytes we can send now
    let mut bytes_remaining = transfer_queue.free_space();

    let mut build_more_frames = true;

    while build_more_frames && transfer_queue.can_send() && !send_queue.is_empty() {
        let mut frame_size = frame_overhead_bytes;
        let mut rel_dgs = Vec::new();
        let mut unrel_dgs = Vec::new();

        while let Some(entry) = send_queue.front() {
            let encoded_size = entry.data.encoded_size();

            let hyp_frame_size = frame_size + encoded_size;

            if hyp_frame_size > bytes_remaining {
                // Would be too large for congestion window, assemble what we have and stop
                build_more_frames = false;
                break;
            }

            // This datagram alone must not exceed the frame limit, or we will loop forever!
            assert!(frame_overhead_bytes + encoded_size <= frame_limit_bytes);

            if hyp_frame_size > frame_limit_bytes {
                // Would be too large for this frame, assemble and continue
                break;
            }

            // Verification complete, add datagram to this frame
            let entry = send_queue.pop_front().unwrap();

            if entry.reliable {
                rel_dgs.push(entry.data);
            } else {
                unrel_dgs.push(entry.data);
            }

            frame_size += encoded_size;
        }

        assert!(rel_dgs.len() != 0 || unrel_dgs.len() != 0);

        bytes_remaining -= frame_size;

        transfer_queue.send_frame(rel_dgs, unrel_dgs, now, sink);
    }

    // Retain reliable entries
    send_queue.retain(|entry| entry.reliable == true);
}

