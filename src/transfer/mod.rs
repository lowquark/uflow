
use std::collections::VecDeque;
use std::time;

use super::frame;
use super::DataSink;
use super::MTU;

mod congestion_window;
mod send_queue;
mod transfer_queue;

use congestion_window::CongestionWindow;
use send_queue::SendQueue;
use transfer_queue::TransferQueue;

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

fn assemble_frame(send_queue: &mut send_queue::SendQueue, max_size: usize) -> (Vec<frame::DataEntry>, Vec<frame::DataEntry>) {
    let mut frame_size = frame::Data::HEADER_SIZE_BYTES;
    let mut unrel_msgs = Vec::new();
    let mut rel_msgs = Vec::new();

    while let Some(entry) = send_queue.front() {
        let encoded_size = entry.data.encoded_size();
        let hyp_frame_size = frame_size + encoded_size;

        if hyp_frame_size > max_size {
            // Would be too large for congestion window, return what we have
            break;
        }

        if hyp_frame_size > MTU {
            // Would be too large for this frame, return what we have
            break;
        }

        // Verification complete, add datagram to this frame
        frame_size += encoded_size;

        let entry = send_queue.pop_front().unwrap();
        if entry.reliable {
            rel_msgs.push(entry.data);
        } else {
            unrel_msgs.push(entry.data);
        }
    }

    return (unrel_msgs, rel_msgs);
}

// Assembles and sends as many frames as possible, with datagrams taken from the send queue
// in order, subject to the frame size limit, the congestion window, and the sequence id
// transfer window.
//
// All entries in the send queue must have an encoded size such that they may be stored in
// a frame satisfying the MTU (i.e. encoded_size <= MTU - HEADER_SIZE).
fn enqueue_new_data(send_queue: &mut SendQueue, transfer_queue: &mut TransferQueue, cwnd: usize) {
    while cwnd > transfer_queue.size() && transfer_queue.sequence_id_span() <= TRANSFER_WINDOW_SIZE {
        let free_space = cwnd - transfer_queue.size();

        let (unrel_msgs, rel_msgs) = assemble_frame(send_queue, free_space);

        if unrel_msgs.len() > 0 && rel_msgs.len() > 0 {
            transfer_queue.push_mixed(unrel_msgs, rel_msgs);
        } else if unrel_msgs.len() > 0 {
            transfer_queue.push_unreliable(unrel_msgs);
        } else if rel_msgs.len() > 0 {
            transfer_queue.push_reliable(rel_msgs);
        } else {
            break;
        }
    }
}

pub struct FrameIO {
    send_queue: SendQueue,
    transfer_queue: TransferQueue,
    congestion_window: CongestionWindow,
    ack_queue: VecDeque<u32>,
    recv_sequence_id: u32,
}

impl FrameIO {
    pub fn new(tx_sequence_id: u32, rx_sequence_id: u32, max_tx_bandwidth: usize) -> Self {
        Self {
            send_queue: SendQueue::new(),
            transfer_queue: TransferQueue::new(tx_sequence_id),
            congestion_window: CongestionWindow::new(),
            ack_queue: VecDeque::new(),
            recv_sequence_id: rx_sequence_id,
        }
    }

    pub fn enqueue_datagram(&mut self, data: frame::DataEntry, reliable: bool, high_priority: bool) {
        debug_assert!(data.encoded_size() <= MTU - frame::Data::HEADER_SIZE_BYTES);

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
        let bytes_acked = self.transfer_queue.remove_frames(data_ack.sequence_ids);

        if bytes_acked > 0 {
            self.congestion_window.signal_ack(bytes_acked);
        }
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

        enqueue_new_data(&mut self.send_queue, &mut self.transfer_queue, self.congestion_window.size());

        let bytes_nacked = self.transfer_queue.send_pending_frames(now, timeout, sink);

        if bytes_nacked > 0 {
            self.congestion_window.signal_nack(now, timeout);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.send_queue.is_empty() && self.transfer_queue.is_empty()
    }
}

