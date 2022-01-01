
use crate::frame::Datagram;
use crate::MAX_TRANSFER_UNIT;
use crate::SendMode;
use crate::frame;
use crate::FrameSink;

mod packet_sender;
mod packet_receiver;

mod frame_queue;
mod frame_ack_queue;

mod tfrc;

use packet_receiver::PacketReceiver;
use packet_sender::PacketSender;
use frame_queue::FrameQueue;
use frame_ack_queue::FrameAckQueue;

use std::time;

impl packet_sender::DatagramSink for FrameQueue {
    fn send(&mut self, datagram: Datagram, resend: bool) {
        self.enqueue_message(frame::Message::Datagram(datagram), resend);
    }
}

pub trait PacketSink {
    fn send(&mut self, packet_data: Box<[u8]>, channel_id: u8);
}

pub struct DatenMeister {
    packet_sender: PacketSender,
    packet_receiver: PacketReceiver,

    frame_queue: FrameQueue,
    frame_ack_queue: FrameAckQueue,

    send_rate_comp: tfrc::SendRateComp,

    time_base: time::Instant,
    time_last_flushed: Option<time::Instant>,

    flush_alloc: usize,
}

impl DatenMeister {
    pub fn new(tx_channels: usize, rx_channels: usize,
               tx_alloc_limit: usize, rx_alloc_limit: usize,
               tx_base_id: u32, rx_base_id: u32) -> Self {
        Self {
            packet_sender: PacketSender::new(tx_channels, tx_alloc_limit, tx_base_id),
            packet_receiver: PacketReceiver::new(rx_channels, rx_alloc_limit, rx_base_id),

            frame_queue: FrameQueue::new(tx_base_id),
            frame_ack_queue: FrameAckQueue::new(),

            send_rate_comp: tfrc::SendRateComp::new(tx_base_id),

            time_base: time::Instant::now(),
            time_last_flushed: None,

            flush_alloc: MAX_TRANSFER_UNIT,
        }
    }

    pub fn is_send_pending(&self) -> bool {
        self.packet_sender.pending_count() != 0 || self.frame_queue.pending_count() != 0
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode) {
        self.packet_sender.enqueue_packet(data, channel_id, mode);
    }

    pub fn receive(&mut self, sink: &mut impl PacketSink) {
        self.packet_receiver.receive(sink);
    }

    pub fn handle_message_frame(&mut self, frame: frame::MessageFrame) {
        let now_ms = self.time_base.elapsed().as_millis() as u64;

        self.frame_ack_queue.mark_seen(frame.sequence_id, frame.nonce);

        for message in frame.messages.into_iter() {
            match message {
                frame::Message::Datagram(datagram) => {
                    self.packet_receiver.handle_datagram(datagram);
                }
                frame::Message::Ack(ack) => {
                    self.frame_queue.acknowledge_frames(ack.frames.clone());
                    self.send_rate_comp.acknowledge_frames(ack.frames, now_ms);

                    self.packet_sender.acknowledge(ack.receiver_base_id);
                }
                frame::Message::Resync(resync) => {
                    self.packet_receiver.resynchronize(resync.sender_next_id);
                }
            }
        }
    }

    pub fn flush(&mut self, sink: &mut impl FrameSink) {
        let now = time::Instant::now();
        let now_ms = (now - self.time_base).as_millis() as u64;

        let rtt_ms = self.send_rate_comp.rtt_ms().unwrap_or(100);

        // Forget old frame data
        self.frame_queue.forget_frames(now_ms.saturating_sub(rtt_ms*4));
        self.send_rate_comp.forget_frames(now_ms.saturating_sub(rtt_ms*4));

        // Enqueue acknowledgements
        let packet_base_id = self.packet_receiver.base_id();

        while let Some(frame_ack) = self.frame_ack_queue.pop() {
            let ack_message = frame::Message::Ack(frame::Ack {
                frames: frame_ack,
                receiver_base_id: packet_base_id,
            });

            self.frame_queue.enqueue_message(ack_message, false);
        }

        // Enqueue new packets
        self.packet_sender.emit_datagrams(&mut self.frame_queue);

        // Update send rate value
        self.send_rate_comp.step();

        // Fill flush allocation according to send rate
        if let Some(time_last_flushed) = self.time_last_flushed {
            let send_rate = self.send_rate_comp.send_rate();
            let rtt_s = self.send_rate_comp.rtt_s();

            let delta_time = (now - time_last_flushed).as_secs_f64();
            let new_bytes = (send_rate * delta_time).round() as usize;
            let alloc_max = ((send_rate * rtt_s.unwrap_or(0.0)).round() as usize).max(MAX_TRANSFER_UNIT);

            self.flush_alloc = self.flush_alloc.saturating_add(new_bytes).min(alloc_max);
        }
        self.time_last_flushed = Some(now);

        // Send as many frames as possible
        loop {
            match self.frame_queue.emit_frame(now_ms, rtt_ms, self.flush_alloc.min(MAX_TRANSFER_UNIT)) {
                Ok((frame_data, frame_id, nonce)) => {
                    let frame_size = frame_data.len();

                    use frame::serial::Serialize;

                    sink.send(&frame_data);

                    self.flush_alloc -= frame_size;
                    self.send_rate_comp.log_frame(frame_id, nonce, frame_size, now_ms);
                }
                Err(cond) => {
                    match cond {
                        frame_queue::Error::WindowLimited => (),
                        frame_queue::Error::SizeLimited => self.send_rate_comp.log_rate_limited(),
                        frame_queue::Error::DataLimited => (),
                    }
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DatenMeister;
    use super::SendMode;

    struct TestFrameSink {
        label: Box<str>,
    }

    impl super::FrameSink for TestFrameSink {
        fn send(&mut self, frame_bytes: &[u8]) {
            println!("{} -> {:?}", self.label, frame_bytes);
        }
    }

    #[test]
    fn basic_test() {
        let mut dm0 = DatenMeister::new(1, 1, 100000, 100000, 0, 0);
        let mut sink0 = TestFrameSink { label: "DatenMeister 0".into() };

        dm0.send(vec![0xBE, 0xEF].into_boxed_slice(), 0, SendMode::Reliable);
        dm0.flush(&mut sink0);
    }
}

