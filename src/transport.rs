
use std::collections::VecDeque;
use std::time;

use super::frame;
use super::DataSink;

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

#[derive(Debug)]
struct ResendEntry {
    frame_data: Box<[u8]>,
    last_send_time: time::Instant,
    sequence_id: u32,
}

impl ResendEntry {
    fn new(frame_data: Box<[u8]>, last_send_time: time::Instant, sequence_id: u32) -> Self {
        Self {
            frame_data: frame_data, 
            last_send_time: last_send_time,
            sequence_id: sequence_id,
        }
    }

    fn mark_sent(&mut self, now: time::Instant) {
        self.last_send_time = now;
    }

    fn should_resend(&self, now: time::Instant, timeout: time::Duration) -> bool {
        now - self.last_send_time > timeout
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

    pub fn peek(&self) -> Option<&SendEntry> {
        if let Some(entry) = self.high_priority.front() {
            return Some(entry);
        }
        return self.low_priority.front();
    }

    pub fn pop(&mut self) -> Option<SendEntry> {
        if let Some(entry) = self.high_priority.pop_front() {
            return Some(entry);
        }
        return self.low_priority.pop_front();
    }

    pub fn is_empty(&self) -> bool {
        self.high_priority.is_empty() && self.low_priority.is_empty()
    }
}

pub struct FrameIO {
    send_queue: SendQueue,
    resend_queue: VecDeque<ResendEntry>,
    ack_queue: VecDeque<u32>,
    next_sequence_id: u32,
    base_sequence_id: u32,
}

impl FrameIO {
    // TODO: TRANSFER_WINDOW_SIZE, MTU as arguments to new()
    const TRANSFER_WINDOW_SIZE: u32 = 1024;
    const MTU: usize = 1500 - 28;

    pub fn new() -> Self {
        Self {
            send_queue: SendQueue::new(),
            resend_queue: VecDeque::new(),
            ack_queue: VecDeque::new(),
            next_sequence_id: 0,
            base_sequence_id: 0,
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

    fn pull_frame(&mut self, now: time::Instant, timeout: time::Duration) -> Option<Box<[u8]>> {
        // Send any pending acks
        if !self.ack_queue.is_empty() {
            let capacity_ids = (Self::MTU - frame::DataAck::HEADER_SIZE_BYTES)/frame::DataAck::SEQUENCE_ID_SIZE_BYTES;

            let sequence_ids = self.ack_queue.drain(..usize::min(capacity_ids, self.ack_queue.len())).collect();
            let frame = frame::Frame::DataAck(frame::DataAck {
                sequence_ids: sequence_ids,
            });

            return Some(frame.to_bytes());
        }

        // Resend any pending reliable frames
        for entry in self.resend_queue.iter_mut() {
            if entry.should_resend(now, timeout) {
                entry.mark_sent(now);
                return Some(entry.frame_data.clone());
            }
        }

        if !self.send_queue.is_empty() {
            // Assemble a new data frame from as many send entries as possible if the transfer window permits
            if self.next_sequence_id.wrapping_sub(self.base_sequence_id) < Self::TRANSFER_WINDOW_SIZE {
                let mut unreliable_entries = Vec::new();
                let mut reliable_entries = Vec::new();
                let mut num_entries = 0;

                let mut frame_size = frame::Data::HEADER_SIZE_BYTES;

                // Enqueue at least one datagram, then consider MTU
                while let Some(entry) = self.send_queue.peek() {
                    let encoded_size = entry.data.encoded_size();

                    if num_entries == 0 || frame_size + encoded_size <= Self::MTU {
                        let entry = self.send_queue.pop().unwrap();
                        if entry.reliable {
                            reliable_entries.push(entry.data);
                        } else {
                            unreliable_entries.push(entry.data);
                        }
                        frame_size += encoded_size;
                        num_entries += 1;
                    } else {
                        // Size limit reached, assemble frame
                        break;
                    }
                }

                let sequence_id = self.next_sequence_id;

                if reliable_entries.len() > 0 {
                    // This claims the sequence id
                    self.next_sequence_id = self.next_sequence_id.wrapping_add(1);

                    // Assemble frame of reliable datagrams to resend later
                    let resend_frame = frame::Data {
                        ack: true,
                        sequence_id: sequence_id,
                        entries: reliable_entries,
                    };

                    self.resend_queue.push_back(ResendEntry::new(resend_frame.to_bytes(), now, sequence_id));

                    reliable_entries = resend_frame.entries;
                }

                // Assemble frame of all datagrams
                let mut all_entries = reliable_entries;
                all_entries.append(&mut unreliable_entries);

                let frame = frame::Frame::Data(frame::Data {
                    ack: true,
                    sequence_id: sequence_id,
                    entries: all_entries,
                });

                return Some(frame.to_bytes());
            }
        }

        None
    }

    pub fn flush(&mut self, now: time::Instant, timeout: time::Duration, sink: & dyn DataSink) {
        while let Some(frame_data) = self.pull_frame(now, timeout) {
            sink.send(&frame_data);
        }
    }

    pub fn is_tx_idle(&self) -> bool {
        self.send_queue.is_empty() && self.resend_queue.is_empty()
    }
}

