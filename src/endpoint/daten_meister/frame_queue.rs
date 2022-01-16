
use crate::MAX_FRAME_TRANSFER_WINDOW_SIZE;

use super::packet_sender;
use super::frame_log;
use super::resend_queue;
use super::message_queue;

use super::PersistentMessage;

use crate::frame::FrameAck;
use crate::frame::Message;
use crate::frame::Datagram;
use crate::frame::serial::MessageFrameBuilder;

use std::rc::Rc;
use std::cell::RefCell;

const MAX_SEND_COUNT: u8 = 4;


#[derive(Debug,PartialEq)]
pub enum Error {
    WindowLimited,
    DataLimited,
    SizeLimited,
}


impl packet_sender::DatagramSink for message_queue::MessageQueue {
    fn send(&mut self, datagram: Datagram, resend: bool) {
        self.push_back(message_queue::Entry::new(Message::Datagram(datagram), resend));
    }
}


pub struct FrameQueue {
    message_queue: message_queue::MessageQueue,
    resend_queue: resend_queue::ResendQueue,
    frame_log: frame_log::FrameLog,
}

impl FrameQueue {
    pub fn new(base_id: u32) -> Self {
        Self {
            message_queue: message_queue::MessageQueue::new(),
            resend_queue: resend_queue::ResendQueue::new(),
            frame_log: frame_log::FrameLog::new(base_id),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.message_queue.len() + self.resend_queue.len()
    }

    #[deprecated]
    pub fn enqueue_message(&mut self, message: Message, resend: bool) {
        self.message_queue.push_back(message_queue::Entry::new(message, resend));
    }

    pub fn emit_frame(&mut self,
                      packet_sender: &mut packet_sender::PacketSender,
                      now_ms: u64, rto_ms: u64, size_limit: usize) -> Result<(Box<[u8]>, u32, bool), Error> {
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
                packet_sender.emit_packet_datagrams(&mut self.message_queue);
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

    pub fn acknowledge_frames(&mut self, ack: FrameAck) {
        self.frame_log.acknowledge_frames(ack);
    }

    pub fn forget_frames(&mut self, thresh_ms: u64) {
        self.frame_log.forget_frames(thresh_ms);
    }
}

/*
#[cfg(test)]
mod tests {
    use crate::frame;
    use crate::frame::serial::Serialize;

    use crate::MAX_FRAGMENT_SIZE;
    use crate::MAX_TRANSFER_UNIT;

    use super::FrameQueue;
    use super::Error;

    fn new_dummy_message(sequence_id: u32, data_size: usize) -> frame::Message {
        frame::Message::Datagram(frame::Datagram {
            sequence_id,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: frame::FragmentId { id: 0, last: 0 },
            data: (0 .. data_size).map(|i| i as u8).collect::<Vec<_>>().into(),
        })
    }

    fn new_max_message(sequence_id: u32) -> frame::Message {
        frame::Message::Datagram(frame::Datagram {
            sequence_id,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: frame::FragmentId { id: 0, last: 1 },
            data: (0 .. MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<_>>().into(),
        })
    }

    fn verify_frame(result: Result<(Box<[u8]>, u32, bool), Error>, sequence_id: u32, messages: Vec<frame::Message>) -> Box<[u8]> {
        if let Ok((frame_data, result_sequence_id, _)) = result {
            let frame = frame::Frame::read(&frame_data).unwrap();

            if let frame::Frame::MessageFrame(frame) = frame {
                assert_eq!(result_sequence_id, sequence_id);
                assert_eq!(frame.sequence_id, sequence_id);
                assert_eq!(frame.messages, messages);

                return frame_data;
            } else {
                panic!("Expected MessageFrame");
            }
        } else {
            panic!("Unexpected frame generation error");
        }
    }

    const MAX_SIZE_LIMIT: usize = MAX_TRANSFER_UNIT;

    #[test]
    fn basic() {
        let mut fq = FrameQueue::new(0);

        let now_ms = 0;
        let rto_ms = 100;

        let msg0 = new_dummy_message(0, 15);
        let msg1 = new_dummy_message(1, 15);
        let msg2 = new_dummy_message(2, 15);

        fq.enqueue_message(msg0.clone(), false);
        fq.enqueue_message(msg1.clone(), false);
        fq.enqueue_message(msg2.clone(), false);

        assert_eq!(fq.pending_count(), 3);

        verify_frame(fq.emit_frame(now_ms, rto_ms, MAX_SIZE_LIMIT), 0, vec![ msg0, msg1, msg2 ]);

        assert_eq!(fq.pending_count(), 0);
    }

    #[test]
    fn max_message() {
        let mut fq = FrameQueue::new(0);

        let now_ms = 0;
        let rto_ms = 100;

        let msg0 = new_max_message(0);
        fq.enqueue_message(msg0.clone(), false);

        let frame_data = verify_frame(fq.emit_frame(now_ms, rto_ms, MAX_SIZE_LIMIT), 0, vec![ msg0 ]);

        assert!(frame_data.len() == MAX_TRANSFER_UNIT);
    }

    #[test]
    fn size_limit() {
        let mut fs = FrameQueue::new(0);

        let now_ms = 0;
        let rto_ms = 100;

        assert_eq!(fs.emit_frame(now_ms, rto_ms, 0), Err(Error::DataLimited));
        assert_eq!(fs.emit_frame(now_ms, rto_ms, MAX_TRANSFER_UNIT), Err(Error::DataLimited));

        let msg0 = new_max_message(0);
        fs.enqueue_message(msg0.clone(), false);

        assert_eq!(fs.emit_frame(now_ms, rto_ms, MAX_TRANSFER_UNIT-1), Err(Error::SizeLimited));
        verify_frame(fs.emit_frame(now_ms, rto_ms, MAX_TRANSFER_UNIT), 0, vec![ msg0 ]);

        let msg1 = new_dummy_message(1, 400);
        let msg2 = new_dummy_message(2, 400);
        let msg3 = new_dummy_message(3, 400);
        let msg4 = new_dummy_message(4, 400);
        let msg5 = new_dummy_message(4, 400);
        fs.enqueue_message(msg1.clone(), false);
        fs.enqueue_message(msg2.clone(), false);
        fs.enqueue_message(msg3.clone(), false);
        fs.enqueue_message(msg4.clone(), false);
        fs.enqueue_message(msg5.clone(), false);

        let frame_overhead = 8;
        let message_overhead = 11;
        let min_send_size_1 = frame_overhead + message_overhead + 400;
        let min_send_size_2 = frame_overhead + 2*(message_overhead + 400);

        assert_eq!(fs.emit_frame(now_ms, rto_ms, min_send_size_1 - 1), Err(Error::SizeLimited));
        verify_frame(fs.emit_frame(now_ms, rto_ms, min_send_size_1), 1, vec![ msg1 ]);
        verify_frame(fs.emit_frame(now_ms, rto_ms, min_send_size_2 - 1), 2, vec![ msg2 ]);
        verify_frame(fs.emit_frame(now_ms, rto_ms, min_send_size_2), 3, vec![ msg3, msg4 ]);
    }

    #[test]
    fn resends() {
        let mut fs = FrameQueue::new(0);

        let rto_ms = 100;

        let msg = new_dummy_message(4, 400);
        fs.enqueue_message(msg.clone(), true);

        assert_eq!(fs.pending_count(), 1);

        verify_frame(fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT), 0, vec![ msg.clone() ]);

        assert_eq!(fs.emit_frame(1, rto_ms, MAX_SIZE_LIMIT), Err(Error::DataLimited));

        let resend_times = [ rto_ms, 3*rto_ms, 7*rto_ms, 15*rto_ms, 31*rto_ms, 47*rto_ms, 63*rto_ms ];

        let mut frame_id = 1;

        for time_ms in resend_times.iter() {
            assert_eq!(fs.pending_count(), 1);

            assert_eq!(fs.emit_frame(*time_ms - 1, rto_ms, MAX_SIZE_LIMIT), Err(Error::DataLimited));

            verify_frame(fs.emit_frame(*time_ms, rto_ms, MAX_SIZE_LIMIT), frame_id, vec![ msg.clone() ]);

            assert_eq!(fs.emit_frame(*time_ms + 1, rto_ms, MAX_SIZE_LIMIT), Err(Error::DataLimited));

            frame_id += 1;
        }

        assert_eq!(fs.pending_count(), 1);
    }

    #[test]
    fn basic_acknowledgement() {
        let mut fs = FrameQueue::new(0);

        let rto_ms = 100;

        let msg0 = new_max_message(0);
        let msg1 = new_max_message(1);
        let msg2 = new_max_message(2);
        let msg3 = new_max_message(3);
        let msg4 = new_max_message(4);

        fs.enqueue_message(msg0.clone(), true);
        fs.enqueue_message(msg1.clone(), true);
        fs.enqueue_message(msg2.clone(), true);
        fs.enqueue_message(msg3.clone(), true);
        fs.enqueue_message(msg4.clone(), true);
        fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();

        fs.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b11101, nonce: false });

        verify_frame(fs.emit_frame(rto_ms, rto_ms, MAX_SIZE_LIMIT), 5, vec![ msg1.clone() ]);
    }

    #[test]
    fn max_acknowledgement() {
        let mut fs = FrameQueue::new(0);

        let rto_ms = 100;

        for msg_id in 0..32 {
            let msg = new_max_message(msg_id);
            fs.enqueue_message(msg.clone(), true);
            fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();
        }

        fs.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0xFFFFFFFF, nonce: false });

        assert_eq!(fs.emit_frame(rto_ms, rto_ms, MAX_SIZE_LIMIT), Err(Error::DataLimited));
    }

    #[test]
    fn flexible_acknowledgement_base() {
        let mut fs = FrameQueue::new(0);

        let rto_ms = 100;

        for msg_id in 0..32 {
            let msg = new_max_message(msg_id);
            fs.enqueue_message(msg.clone(), true);
            fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();
        }

        fs.acknowledge_frames(frame::FrameAck { base_id: 0u32.wrapping_sub(16), bitfield: 0xFFFF0000, nonce: false });
        fs.acknowledge_frames(frame::FrameAck { base_id: 0u32.wrapping_add(16), bitfield: 0x0000FFFF, nonce: false });

        assert_eq!(fs.emit_frame(rto_ms, rto_ms, MAX_SIZE_LIMIT), Err(Error::DataLimited));
    }

    #[test]
    fn forget_frames() {
        let mut fs = FrameQueue::new(0);

        let rto_ms = 100;

        let msg0 = new_max_message(0);
        let msg1 = new_max_message(1);
        let msg2 = new_max_message(2);
        let msg3 = new_max_message(3);
        let msg4 = new_max_message(4);

        fs.enqueue_message(msg0.clone(), true);
        fs.emit_frame(0, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.enqueue_message(msg1.clone(), true);
        fs.emit_frame(9, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.enqueue_message(msg2.clone(), true);
        fs.emit_frame(10, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.enqueue_message(msg3.clone(), true);
        fs.emit_frame(11, rto_ms, MAX_SIZE_LIMIT).unwrap();
        fs.enqueue_message(msg4.clone(), true);
        fs.emit_frame(12, rto_ms, MAX_SIZE_LIMIT).unwrap();

        fs.forget_frames(10);

        fs.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b11111, nonce: false });

        verify_frame(fs.emit_frame(rto_ms + 9, rto_ms, MAX_SIZE_LIMIT), 5, vec![ msg0.clone() ]);
        verify_frame(fs.emit_frame(rto_ms + 9, rto_ms, MAX_SIZE_LIMIT), 6, vec![ msg1.clone() ]);
    }
}
*/

