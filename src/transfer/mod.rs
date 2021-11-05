
use std::collections::VecDeque;
use std::time;

use super::frame;
use super::DataSink;
use super::MTU;

mod queuing;
mod transfer_queue;

const TRANSFER_WINDOW_SIZE: u32 = 128;

/*
struct LeakyBucket {
    alloc: usize,
    byte_rate: usize,
    last_step_time: Option<time::Instant>,
    sdt: f64,
    step_occurred: bool,
}

impl LeakyBucket {
    const STEP_TIME_ALPHA: f64 = 0.875;
    const BURSTINESS_FACTOR: f64 = 2.0;

    fn new(byte_rate: usize) -> Self {
        Self {
            alloc: 0,
            byte_rate: byte_rate,
            last_step_time: None,
            sdt: 0.0,
            step_occurred: false,
        }
    }

    fn step(&mut self, now: time::Instant) {
        if let Some(last_step_time) = self.last_step_time {
            let delta_time = (now - last_step_time).as_secs_f64();

            // Estimating the time delta is obnoxious, but having the user specify alloc_max makes
            // for an odd bandwidth negotiation.
            if self.step_occurred {
                self.sdt = self.sdt * Self::STEP_TIME_ALPHA + delta_time * (1.0 - Self::STEP_TIME_ALPHA);
            } else {
                self.sdt = delta_time;
                self.step_occurred = true;
            }

            let alloc_max = ((self.byte_rate as f64)*self.sdt*Self::BURSTINESS_FACTOR).round() as usize;

            let delta_bytes = ((self.byte_rate as f64)*delta_time).round() as usize;

            // If the bucket cannot fill to at least one MTU, the queue will stall!
            self.alloc = std::cmp::min(self.alloc + delta_bytes, alloc_max.max(MTU));
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
*/

pub struct FrameIO {
    send_queue: queuing::SendQueue,
    transfer_queue: queuing::TransferQueue,
    ack_queue: VecDeque<u32>,
    recv_sequence_id: u32,
}

impl FrameIO {
    pub fn new(tx_sequence_id: u32, rx_sequence_id: u32, max_tx_bandwidth: usize) -> Self {
        Self {
            send_queue: queuing::SendQueue::new(),
            transfer_queue: queuing::TransferQueue::new(tx_sequence_id),
            ack_queue: VecDeque::new(),
            recv_sequence_id: rx_sequence_id,
        }
    }

    pub fn enqueue_datagram(&mut self, data: frame::DataEntry, reliable: bool, high_priority: bool) {
        self.send_queue.push(data, reliable, high_priority);
    }

    pub fn handle_data_frame(&mut self, data: frame::Data) -> Option<Vec<frame::DataEntry>> {
        let recv_window_min = self.recv_sequence_id.wrapping_sub(TRANSFER_WINDOW_SIZE);
        let recv_window_size = 2*TRANSFER_WINDOW_SIZE;

        let in_recv_window = data.sequence_id.wrapping_sub(recv_window_min) < recv_window_size;

        if in_recv_window {
            if data.sequence_id.wrapping_sub(self.recv_sequence_id) < TRANSFER_WINDOW_SIZE {
                // This sequence ID is newer, advance recv_sequence_id to represent the next expected sequence ID
                self.recv_sequence_id = data.sequence_id.wrapping_add(1);
            }

            self.ack_queue.push_back(data.sequence_id);

            Some(data.entries)
        } else {
            None
        }
    }

    pub fn handle_data_ack_frame(&mut self, data_ack: frame::DataAck) {
        self.transfer_queue.acknowledge_frames(data_ack.sequence_ids);
    }

    fn send_acks(&mut self, sink: & dyn DataSink) {
        while !self.ack_queue.is_empty() {
            let max_ids = (MTU - frame::DataAck::HEADER_SIZE_BYTES)/frame::DataAck::SEQUENCE_ID_SIZE_BYTES;

            let sequence_ids = self.ack_queue.drain(..usize::min(max_ids, self.ack_queue.len())).collect();
            let frame = frame::Frame::DataAck(frame::DataAck {
                sequence_ids: sequence_ids,
            });

            let frame_data = frame.to_bytes();
            sink.send(&frame_data);
        }
    }

    pub fn flush(&mut self, now: time::Instant, timeout: time::Duration, sink: & dyn DataSink) {
        self.send_acks(sink);

        self.transfer_queue.flush(now, timeout, sink);

        queuing::send_new_data(&mut self.send_queue, &mut self.transfer_queue, now, sink);
    }

    pub fn is_idle(&self) -> bool {
        self.send_queue.is_empty() && self.transfer_queue.is_empty()
    }
}

