
use crate::MAX_FRAME_SIZE;
use crate::SendMode;
use crate::frame;

use super::FrameSink;

use std::time;

mod pending_packet;

mod packet_sender;
mod packet_receiver;

mod datagram_queue;
mod resend_queue;
mod frame_log;
mod frame_ack_queue;

mod recv_rate_set;
mod reorder_buffer;
mod send_rate;
mod loss_rate;
mod frame_queue;

mod tfrc;

mod emit_frame;

#[cfg(test)]
mod packet_tests;

const INITIAL_RTT_ESTIMATE_MS: u64 = 150;
const INITIAL_RTO_ESTIMATE_MS: u64 = 4*INITIAL_RTT_ESTIMATE_MS;
const MIN_SYNC_TIMEOUT_MS: u64 = 2000;

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

pub trait PacketSink {
    fn send(&mut self, packet_data: Box<[u8]>);
}

pub struct DatenMeister {
    packet_sender: packet_sender::PacketSender,
    datagram_queue: datagram_queue::DatagramQueue,
    resend_queue: resend_queue::ResendQueue,
    frame_log: frame_log::FrameLog,

    send_rate_comp: tfrc::SendRateComp,

    packet_receiver: packet_receiver::PacketReceiver,
    frame_ack_queue: frame_ack_queue::FrameAckQueue,

    time_base: time::Instant,
    time_last_flushed: Option<time::Instant>,
    time_data_sent_ms: Option<u64>,

    flush_alloc: usize,
    flush_id: u32,
}

impl DatenMeister {
    pub fn new(tx_channels: usize, rx_channels: usize,
               tx_alloc_limit: usize, rx_alloc_limit: usize,
               tx_base_id: u32, rx_base_id: u32,
               tx_bandwidth_limit: u32) -> Self {
        Self {
            packet_sender: packet_sender::PacketSender::new(tx_channels, tx_alloc_limit, tx_base_id),
            datagram_queue: datagram_queue::DatagramQueue::new(),
            resend_queue: resend_queue::ResendQueue::new(),
            frame_log: frame_log::FrameLog::new(tx_base_id),

            send_rate_comp: tfrc::SendRateComp::new(tx_base_id, tx_bandwidth_limit),

            packet_receiver: packet_receiver::PacketReceiver::new(rx_channels, rx_alloc_limit, rx_base_id),
            frame_ack_queue: frame_ack_queue::FrameAckQueue::new(),

            time_base: time::Instant::now(),
            time_last_flushed: None,
            time_data_sent_ms: None,

            flush_alloc: MAX_FRAME_SIZE,
            flush_id: 0,
        }
    }

    pub fn rtt_s(&self) -> Option<f64> {
        self.send_rate_comp.rtt_s()
    }

    pub fn is_send_pending(&self) -> bool {
        self.packet_sender.pending_count() != 0 || self.datagram_queue.len() != 0 || self.resend_queue.len() != 0
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode) {
        self.packet_sender.enqueue_packet(data, channel_id, mode, self.flush_id);
    }

    pub fn receive(&mut self, sink: &mut impl PacketSink) {
        self.packet_receiver.receive(sink);
    }

    pub fn handle_data_frame(&mut self, frame: frame::DataFrame) {
        self.frame_ack_queue.mark_seen(frame.sequence_id, frame.nonce);

        for datagram in frame.datagrams.into_iter() {
            self.packet_receiver.handle_datagram(datagram);
        }
    }

    pub fn handle_sync_frame(&mut self, frame: frame::SyncFrame) {
        self.frame_ack_queue.mark_seen(frame.sequence_id, frame.nonce);
        self.packet_receiver.resynchronize(frame.sender_next_id);
    }

    pub fn handle_ack_frame(&mut self, frame: frame::AckFrame) {
        self.packet_sender.acknowledge(frame.receiver_base_id);

        for frame_ack in frame.frame_acks.into_iter() {
            self.frame_log.acknowledge_frames(frame_ack.clone());
            self.send_rate_comp.acknowledge_frames(frame_ack);
        }
    }

    pub fn flush(&mut self, sink: &mut impl FrameSink) {
        let now = time::Instant::now();
        let now_ms = (now - self.time_base).as_millis() as u64;

        let rtt_ms = self.send_rate_comp.rtt_ms().unwrap_or(INITIAL_RTT_ESTIMATE_MS);

        // Forget old frame data
        self.frame_log.forget_frames(now_ms.saturating_sub(rtt_ms*4));
        self.send_rate_comp.forget_frames(now_ms.saturating_sub(rtt_ms*4));

        // Update send rate value
        self.send_rate_comp.step(now_ms);

        // Fill flush allocation according to send rate
        self.fill_flush_alloc(now);

        // Send as many frames as possible
        // TODO: Consider rto_ms/4 idea further
        self.emit_frames(now_ms, rtt_ms, sink);
    }

    fn fill_flush_alloc(&mut self, now: time::Instant) {
        if let Some(time_last_flushed) = self.time_last_flushed {
            let send_rate = self.send_rate_comp.send_rate();
            let rtt_s = self.send_rate_comp.rtt_s();

            let delta_time = (now - time_last_flushed).as_secs_f64();
            let new_bytes = (send_rate * delta_time).round() as usize;
            let alloc_max = ((send_rate * rtt_s.unwrap_or(0.0)).round() as usize).max(MAX_FRAME_SIZE);

            self.flush_alloc = self.flush_alloc.saturating_add(new_bytes).min(alloc_max);
        }
        self.time_last_flushed = Some(now);
    }

    fn emit_frames(&mut self, now_ms: u64, rtt_ms: u64, sink: &mut impl FrameSink) {
        let flush_id = self.flush_id;
        self.flush_id = self.flush_id.wrapping_add(1);

        let sender_next_id = self.packet_sender.next_id();
        let receiver_base_id = self.packet_receiver.base_id();

        let mut fe = emit_frame::FrameEmitter::new(&mut self.packet_sender,
                                                   &mut self.datagram_queue,
                                                   &mut self.resend_queue,
                                                   &mut self.frame_log,
                                                   &mut self.frame_ack_queue,
                                                   flush_id);

        let ref mut send_rate_comp = self.send_rate_comp;
        let ref mut time_data_sent_ms = self.time_data_sent_ms;

        if let Some(time_data_sent_ms) = time_data_sent_ms {
            let sync_timeout_ms = send_rate_comp.rto_ms().unwrap_or(INITIAL_RTO_ESTIMATE_MS).max(MIN_SYNC_TIMEOUT_MS);

            if now_ms - *time_data_sent_ms >= sync_timeout_ms {
                let (send_size, rate_limited) =
                    fe.emit_sync_frame(now_ms, sender_next_id, self.flush_alloc, |data, id, nonce| {
                        send_rate_comp.log_frame(id, nonce, data.len(), now_ms);
                        *time_data_sent_ms = now_ms;
                        sink.send(&data);
                    });

                self.flush_alloc -= send_size;
                if rate_limited {
                    self.send_rate_comp.log_rate_limited();
                    return;
                }
            }
        }

        let (send_size, rate_limited) =
            fe.emit_ack_frames(receiver_base_id, self.flush_alloc, |data| {
                sink.send(&data);
            });

        self.flush_alloc -= send_size;
        if rate_limited {
            self.send_rate_comp.log_rate_limited();
            return;
        }

        let (send_size, rate_limited) =
            fe.emit_data_frames(now_ms, rtt_ms, self.flush_alloc, |data, id, nonce| {
                send_rate_comp.log_frame(id, nonce, data.len(), now_ms);
                *time_data_sent_ms = Some(now_ms);
                sink.send(&data);
            });

        self.flush_alloc -= send_size;
        if rate_limited {
            self.send_rate_comp.log_rate_limited();
            return;
        }
    }
}

