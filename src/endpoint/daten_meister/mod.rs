
use crate::MAX_FRAME_TRANSFER_WINDOW_SIZE;
use crate::MAX_TRANSFER_UNIT;
use crate::SendMode;
use crate::frame;
use crate::FrameSink;

mod packet_sender;
mod packet_receiver;

mod frame_log;
mod resend_queue;
mod message_queue;

mod tfrc;

mod frame_ack_queue;

/*
#[cfg(test)]
mod packet_tests;
*/

use packet_receiver::PacketReceiver;
use packet_sender::PacketSender;
use frame_ack_queue::FrameAckQueue;

use crate::frame::Message;
use crate::frame::Datagram;
use crate::frame::serial::MessageFrameBuilder;

use std::time;

use std::rc::Rc;
use std::rc::Weak;
use std::cell::RefCell;


const MAX_SEND_COUNT: u8 = 4;

#[derive(Debug,PartialEq)]
pub enum Error {
    WindowLimited,
    DataLimited,
    SizeLimited,
}


#[derive(Debug)]
pub struct PersistentMessage {
    message: frame::Message,
    acknowledged: bool,
}

impl PersistentMessage {
    fn new(message: frame::Message) -> Self {
        Self {
            message,
            acknowledged: false,
        }
    }
}

pub type PersistentMessageRc = Rc<RefCell<PersistentMessage>>;
pub type PersistentMessageWeak = Weak<RefCell<PersistentMessage>>;


impl packet_sender::DatagramSink for message_queue::MessageQueue {
    fn send(&mut self, datagram: Datagram, resend: bool) {
        self.push_back(message_queue::Entry::new(Message::Datagram(datagram), resend));
    }
}


pub trait PacketSink {
    fn send(&mut self, packet_data: Box<[u8]>, channel_id: u8);
}

pub struct DatenMeister {
    packet_sender: PacketSender,
    packet_receiver: PacketReceiver,

    message_queue: message_queue::MessageQueue,
    resend_queue: resend_queue::ResendQueue,
    frame_log: frame_log::FrameLog,

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

            message_queue: message_queue::MessageQueue::new(),
            resend_queue: resend_queue::ResendQueue::new(),
            frame_log: frame_log::FrameLog::new(tx_base_id),

            frame_ack_queue: FrameAckQueue::new(),

            send_rate_comp: tfrc::SendRateComp::new(tx_base_id),

            time_base: time::Instant::now(),
            time_last_flushed: None,

            flush_alloc: MAX_TRANSFER_UNIT,
        }
    }

    pub fn is_send_pending(&self) -> bool {
        self.packet_sender.pending_count() != 0 || self.message_queue.len() != 0 || self.resend_queue.len() != 0
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode) {
        self.packet_sender.enqueue_packet(data, channel_id, mode);
    }

    pub fn receive(&mut self, sink: &mut impl PacketSink) {
        self.packet_receiver.receive(sink);
    }

    pub fn handle_message_frame(&mut self, frame: frame::MessageFrame) {
        self.frame_ack_queue.mark_seen(frame.sequence_id, frame.nonce);

        for message in frame.messages.into_iter() {
            match message {
                frame::Message::Datagram(datagram) => {
                    self.packet_receiver.handle_datagram(datagram);
                }
                frame::Message::Ack(ack) => {
                    self.frame_log.acknowledge_frames(ack.frames.clone());
                    self.send_rate_comp.acknowledge_frames(ack.frames);

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
        self.frame_log.forget_frames(now_ms.saturating_sub(rtt_ms*4));
        self.send_rate_comp.forget_frames(now_ms.saturating_sub(rtt_ms*4));

        // Enqueue acknowledgements
        let packet_base_id = self.packet_receiver.base_id();

        while let Some(frame_ack) = self.frame_ack_queue.pop() {
            self.enqueue_ack(frame::Ack {
                frames: frame_ack,
                receiver_base_id: packet_base_id,
            });
        }

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
        loop {
            match self.emit_frame(now_ms, rtt_ms, self.flush_alloc.min(MAX_TRANSFER_UNIT)) {
                Ok((frame_data, frame_id, nonce)) => {
                    let frame_size = frame_data.len();

                    sink.send(&frame_data);

                    self.flush_alloc -= frame_size;
                    self.send_rate_comp.log_frame(frame_id, nonce, frame_size, now_ms);
                }
                Err(cond) => {
                    match cond {
                        Error::WindowLimited => (),
                        Error::SizeLimited => self.send_rate_comp.log_rate_limited(),
                        Error::DataLimited => (),
                    }
                    break;
                }
            }
        }
    }

    fn enqueue_ack(&mut self, ack: frame::Ack) {
        // TODO: Separate ack queue
        self.message_queue.push_back(message_queue::Entry::new(frame::Message::Ack(ack), false));
    }

    fn emit_frame(&mut self, now_ms: u64, rto_ms: u64, size_limit: usize) -> Result<(Box<[u8]>, u32, bool), Error> {
        if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
            return Err(Error::WindowLimited);
        }

        let frame_id = self.frame_log.next_id();
        let nonce = rand::random();

        let mut fbuilder = MessageFrameBuilder::new(frame_id, nonce);
        let mut persistent_messages = Vec::new();

        while let Some(entry) = self.resend_queue.peek() {
            if entry.resend_time > now_ms {
                break;
            }

            if entry.persistent_message.borrow().acknowledged {
                self.resend_queue.pop();
                continue;
            }

            // TODO: If this message is a datagram, drop if beyond packet transfer window

            {
                let persistent_message = entry.persistent_message.borrow();

                let encoded_size = MessageFrameBuilder::message_size(&persistent_message.message);

                if fbuilder.size() + encoded_size > size_limit {
                    if fbuilder.count() == 0 {
                        return Err(Error::SizeLimited);
                    } else {
                        break;
                    }
                }

                fbuilder.add(&persistent_message.message);
            }

            let entry = self.resend_queue.pop().unwrap();

            persistent_messages.push(Rc::downgrade(&entry.persistent_message));

            self.resend_queue.push(resend_queue::Entry::new(entry.persistent_message,
                                                            now_ms + rto_ms*(1 << entry.send_count),
                                                            (entry.send_count + 1).min(MAX_SEND_COUNT)));
        }

        'outer: loop {
            if self.message_queue.is_empty() {
                self.packet_sender.emit_packet_datagrams(&mut self.message_queue);
                if self.message_queue.is_empty() {
                    break 'outer;
                }
            }

            while let Some(entry) = self.message_queue.front() {
                let encoded_size = MessageFrameBuilder::message_size(&entry.message);

                if fbuilder.size() + encoded_size > size_limit {
                    if fbuilder.count() == 0 {
                        return Err(Error::SizeLimited);
                    } else {
                        break 'outer;
                    }
                }

                fbuilder.add(&entry.message);

                let entry = self.message_queue.pop_front().unwrap();

                if entry.resend {
                    let persistent_message = Rc::new(RefCell::new(PersistentMessage::new(entry.message)));

                    persistent_messages.push(Rc::downgrade(&persistent_message));

                    self.resend_queue.push(resend_queue::Entry::new(persistent_message, now_ms + rto_ms, 1));
                }
            }
        }

        if fbuilder.count() == 0 {
            // Nothing to send!
            return Err(Error::DataLimited);
        }

        self.frame_log.push(frame_id, frame_log::Entry {
            send_time_ms: now_ms,
            persistent_messages: persistent_messages.into(),
        });

        return Ok((fbuilder.build(), frame_id, nonce));
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

