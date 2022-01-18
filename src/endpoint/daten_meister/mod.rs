
use crate::MAX_TRANSFER_UNIT;
use crate::SendMode;
use crate::frame;
use crate::FrameSink;

mod packet_sender;
mod packet_receiver;

mod datagram_queue;
mod resend_queue;
mod frame_log;
mod frame_ack_queue;

mod tfrc;

mod emit_frame;

/*
#[cfg(test)]
mod packet_tests;
*/

use packet_receiver::PacketReceiver;
use packet_sender::PacketSender;
use frame_ack_queue::FrameAckQueue;

use crate::frame::Datagram;

use std::time;

use std::rc::Rc;
use std::rc::Weak;
use std::cell::RefCell;


#[derive(Debug)]
pub struct PersistentDatagram {
    datagram: frame::Datagram,
    acknowledged: bool,
}

impl PersistentDatagram {
    fn new(datagram: frame::Datagram) -> Self {
        Self {
            datagram,
            acknowledged: false,
        }
    }
}

pub type PersistentDatagramRc = Rc<RefCell<PersistentDatagram>>;
pub type PersistentDatagramWeak = Weak<RefCell<PersistentDatagram>>;


impl packet_sender::DatagramSink for datagram_queue::DatagramQueue {
    fn send(&mut self, datagram: Datagram, resend: bool) {
        self.push_back(datagram_queue::Entry::new(datagram, resend));
    }
}


pub trait PacketSink {
    fn send(&mut self, packet_data: Box<[u8]>, channel_id: u8);
}

pub struct DatenMeister {
    packet_sender: PacketSender,
    packet_receiver: PacketReceiver,

    datagram_queue: datagram_queue::DatagramQueue,
    resend_queue: resend_queue::ResendQueue,
    frame_log: frame_log::FrameLog,

    frame_ack_queue: FrameAckQueue,

    send_rate_comp: tfrc::SendRateComp,

    time_base: time::Instant,
    time_last_flushed: Option<time::Instant>,
    time_data_sent_ms: Option<u64>,

    flush_alloc: usize,
    flush_id: u32,
}

impl DatenMeister {
    pub fn new(tx_channels: usize, rx_channels: usize,
               tx_alloc_limit: usize, rx_alloc_limit: usize,
               tx_base_id: u32, rx_base_id: u32) -> Self {
        Self {
            packet_sender: PacketSender::new(tx_channels, tx_alloc_limit, tx_base_id),
            packet_receiver: PacketReceiver::new(rx_channels, rx_alloc_limit, rx_base_id),

            datagram_queue: datagram_queue::DatagramQueue::new(),
            resend_queue: resend_queue::ResendQueue::new(),
            frame_log: frame_log::FrameLog::new(tx_base_id),

            frame_ack_queue: FrameAckQueue::new(),

            send_rate_comp: tfrc::SendRateComp::new(tx_base_id),

            time_base: time::Instant::now(),
            time_last_flushed: None,
            time_data_sent_ms: None,

            flush_alloc: MAX_TRANSFER_UNIT,
            flush_id: 0,
        }
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

        let rtt_ms = self.send_rate_comp.rtt_ms().unwrap_or(100);

        // Forget old frame data
        self.frame_log.forget_frames(now_ms.saturating_sub(rtt_ms*4));
        self.send_rate_comp.forget_frames(now_ms.saturating_sub(rtt_ms*4));

        // Update send rate value
        self.send_rate_comp.step(now_ms);

        // Fill flush allocation according to send rate
        self.fill_flush_alloc(now);

        // Send as many frames as possible
        self.emit_frames(now_ms, rtt_ms, sink);
    }

    fn fill_flush_alloc(&mut self, now: time::Instant) {
        if let Some(time_last_flushed) = self.time_last_flushed {
            let send_rate = self.send_rate_comp.send_rate();
            let rtt_s = self.send_rate_comp.rtt_s();

            let delta_time = (now - time_last_flushed).as_secs_f64();
            let new_bytes = (send_rate * delta_time).round() as usize;
            let alloc_max = ((send_rate * rtt_s.unwrap_or(0.0)).round() as usize).max(MAX_TRANSFER_UNIT);

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
            let sync_timeout_ms = rtt_ms*4;

            if now_ms - *time_data_sent_ms >= sync_timeout_ms {
                let (send_size, rate_limited) =
                    fe.emit_sync_frame(sender_next_id, self.flush_alloc, |data, id, nonce| {
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

