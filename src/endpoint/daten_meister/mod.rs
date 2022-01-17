
use crate::MAX_FRAME_TRANSFER_WINDOW_SIZE;
use crate::MAX_TRANSFER_UNIT;
use crate::SendMode;
use crate::frame;
use crate::FrameSink;

mod packet_sender;
mod packet_receiver;

mod frame_log;
mod resend_queue;
mod datagram_queue;

mod tfrc;

mod frame_ack_queue;

/*
#[cfg(test)]
mod packet_tests;
*/

use packet_receiver::PacketReceiver;
use packet_sender::PacketSender;
use frame_ack_queue::FrameAckQueue;

use crate::frame::Datagram;
use crate::frame::serial::DataFrameBuilder;
use crate::frame::serial::AckFrameBuilder;

use std::time;

use std::rc::Rc;
use std::rc::Weak;
use std::cell::RefCell;


const MAX_SEND_COUNT: u8 = 4;


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
        }
    }

    pub fn is_send_pending(&self) -> bool {
        self.packet_sender.pending_count() != 0 || self.datagram_queue.len() != 0 || self.resend_queue.len() != 0
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode) {
        self.packet_sender.enqueue_packet(data, channel_id, mode);
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
        self.emit_sync_frame(now_ms, rtt_ms*4, sink);
        self.emit_ack_frames(sink);
        self.emit_data_frames(now_ms, rtt_ms, sink);
    }

    fn emit_sync_frame(&mut self, now_ms: u64, timeout_ms: u64, sink: &mut impl FrameSink) {
        if let Some(time_data_sent_ms) = self.time_data_sent_ms {
            if now_ms - time_data_sent_ms >= timeout_ms {
                if self.packet_sender.pending_count() == 0 && self.resend_queue.len() == 0 && self.datagram_queue.len() == 0 {
                    if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
                        return;
                    }

                    if self.flush_alloc >= frame::serial::SYNC_FRAME_SIZE {
                        let frame_id = self.frame_log.next_id();
                        let nonce = rand::random();
                        let sender_next_id = self.packet_sender.next_id();

                        let frame = frame::Frame::SyncFrame(frame::SyncFrame { sequence_id: frame_id, nonce, sender_next_id });

                        use frame::serial::Serialize;
                        let frame_data = frame.write();

                        self.flush_alloc -= frame_data.len();
                        sink.send(&frame_data);

                        self.frame_log.push(frame_id, frame_log::Entry {
                            send_time_ms: now_ms,
                            persistent_datagrams: Box::new([]),
                        });

                        self.send_rate_comp.log_frame(frame_id, nonce, frame_data.len(), now_ms);

                        self.time_data_sent_ms = Some(now_ms);
                    }
                }
            }
        }
    }

    fn emit_ack_frames(&mut self, sink: &mut impl FrameSink) {
        let receiver_base_id = self.packet_receiver.base_id();

        let mut fbuilder = AckFrameBuilder::new(receiver_base_id);

        while let Some(frame_ack) = self.frame_ack_queue.peek() {
            let encoded_size = AckFrameBuilder::encoded_size(&frame_ack);
            let potential_frame_size = fbuilder.size() + encoded_size;

            if potential_frame_size > self.flush_alloc {
                if fbuilder.count() > 0 {
                    let frame_data = fbuilder.build();

                    self.flush_alloc -= frame_data.len();
                    sink.send(&frame_data);
                }

                self.send_rate_comp.log_rate_limited();
                return;
            }

            if potential_frame_size > MAX_TRANSFER_UNIT {
                debug_assert!(fbuilder.count() > 0);

                let frame_data = fbuilder.build();

                self.flush_alloc -= frame_data.len();
                sink.send(&frame_data);

                fbuilder = AckFrameBuilder::new(receiver_base_id);

                continue;
            }

            fbuilder.add(&frame_ack);

            self.frame_ack_queue.pop();
        }

        if fbuilder.count() > 0 {
            let frame_data = fbuilder.build();

            self.flush_alloc -= frame_data.len();
            sink.send(&frame_data);
        }
    }

    fn emit_data_frames(&mut self, now_ms: u64, rtt_ms: u64, sink: &mut impl FrameSink) {
        if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
            return;
        }

        let mut frame_id = self.frame_log.next_id();
        let mut nonce = rand::random();

        let mut fbuilder = DataFrameBuilder::new(frame_id, nonce);
        let mut persistent_datagrams = Vec::new();

        while let Some(entry) = self.resend_queue.peek() {
            let pmsg_ref = entry.persistent_datagram.borrow();

            if pmsg_ref.acknowledged {
                std::mem::drop(pmsg_ref);
                self.resend_queue.pop();
                continue;
            }

            if entry.resend_time > now_ms {
                break;
            }

            // TODO: Drop if beyond packet transfer window

            let encoded_size = DataFrameBuilder::encoded_size(&pmsg_ref.datagram);
            let potential_frame_size = fbuilder.size() + encoded_size;

            if potential_frame_size > self.flush_alloc {
                if fbuilder.count() > 0 {
                    let frame_data = fbuilder.build();

                    self.frame_log.push(frame_id, frame_log::Entry {
                        send_time_ms: now_ms,
                        persistent_datagrams: persistent_datagrams.into(),
                    });

                    self.send_rate_comp.log_frame(frame_id, nonce, frame_data.len(), now_ms);

                    self.time_data_sent_ms = Some(now_ms);

                    self.flush_alloc -= frame_data.len();
                    sink.send(&frame_data);
                }

                self.send_rate_comp.log_rate_limited();
                return;
            }

            if potential_frame_size > MAX_TRANSFER_UNIT {
                debug_assert!(fbuilder.count() > 0);

                let frame_data = fbuilder.build();

                self.frame_log.push(frame_id, frame_log::Entry {
                    send_time_ms: now_ms,
                    persistent_datagrams: persistent_datagrams.into(),
                });

                self.send_rate_comp.log_frame(frame_id, nonce, frame_data.len(), now_ms);

                self.time_data_sent_ms = Some(now_ms);

                self.flush_alloc -= frame_data.len();
                sink.send(&frame_data);

                if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
                    return;
                }

                frame_id = self.frame_log.next_id();
                nonce = rand::random();

                fbuilder = DataFrameBuilder::new(frame_id, nonce);
                persistent_datagrams = Vec::new();
                continue;
            }

            fbuilder.add(&pmsg_ref.datagram);

            std::mem::drop(pmsg_ref);
            let entry = self.resend_queue.pop().unwrap();

            persistent_datagrams.push(Rc::downgrade(&entry.persistent_datagram));

            self.resend_queue.push(resend_queue::Entry::new(entry.persistent_datagram,
                                                            now_ms + rtt_ms*(1 << entry.send_count),
                                                            (entry.send_count + 1).min(MAX_SEND_COUNT)));
        }

        'outer: loop {
            if self.datagram_queue.is_empty() {
                self.packet_sender.emit_packet_datagrams(&mut self.datagram_queue);
                if self.datagram_queue.is_empty() {
                    break 'outer;
                }
            }

            while let Some(entry) = self.datagram_queue.front() {
                let encoded_size = DataFrameBuilder::encoded_size(&entry.datagram);
                let potential_frame_size = fbuilder.size() + encoded_size;

                if potential_frame_size > self.flush_alloc {
                    if fbuilder.count() > 0 {
                        let frame_data = fbuilder.build();

                        self.frame_log.push(frame_id, frame_log::Entry {
                            send_time_ms: now_ms,
                            persistent_datagrams: persistent_datagrams.into(),
                        });

                        self.send_rate_comp.log_frame(frame_id, nonce, frame_data.len(), now_ms);

                        self.time_data_sent_ms = Some(now_ms);

                        self.flush_alloc -= frame_data.len();
                        sink.send(&frame_data);
                    }

                    self.send_rate_comp.log_rate_limited();
                    return;
                }

                if potential_frame_size > MAX_TRANSFER_UNIT {
                    debug_assert!(fbuilder.count() > 0);

                    let frame_data = fbuilder.build();

                    self.frame_log.push(frame_id, frame_log::Entry {
                        send_time_ms: now_ms,
                        persistent_datagrams: persistent_datagrams.into(),
                    });

                    self.send_rate_comp.log_frame(frame_id, nonce, frame_data.len(), now_ms);

                    self.time_data_sent_ms = Some(now_ms);

                    self.flush_alloc -= frame_data.len();
                    sink.send(&frame_data);

                    if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
                        return;
                    }

                    frame_id = self.frame_log.next_id();
                    nonce = rand::random();

                    fbuilder = DataFrameBuilder::new(frame_id, nonce);
                    persistent_datagrams = Vec::new();
                    continue;
                }

                fbuilder.add(&entry.datagram);

                let entry = self.datagram_queue.pop_front().unwrap();

                if entry.resend {
                    let persistent_datagram = Rc::new(RefCell::new(PersistentDatagram::new(entry.datagram)));

                    persistent_datagrams.push(Rc::downgrade(&persistent_datagram));

                    self.resend_queue.push(resend_queue::Entry::new(persistent_datagram, now_ms + rtt_ms, 1));
                }
            }
        }

        if fbuilder.count() > 0 {
            let frame_data = fbuilder.build();

            self.frame_log.push(frame_id, frame_log::Entry {
                send_time_ms: now_ms,
                persistent_datagrams: persistent_datagrams.into(),
            });

            self.send_rate_comp.log_frame(frame_id, nonce, frame_data.len(), now_ms);

            self.time_data_sent_ms = Some(now_ms);

            self.flush_alloc -= frame_data.len();
            sink.send(&frame_data);
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

