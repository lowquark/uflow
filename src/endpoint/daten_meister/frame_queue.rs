
use crate::MAX_FRAME_TRANSFER_WINDOW_SIZE;

use crate::frame::FrameAck;
use crate::frame::Message;
use crate::frame::serial::MessageFrameBuilder;

use std::collections::BinaryHeap;
use std::collections::VecDeque;
use std::cmp::Ordering;

use std::rc::Rc;
use std::rc::Weak;
use std::cell::RefCell;

const MAX_SEND_COUNT: u8 = 4;

#[derive(Debug)]
struct PersistentMessage {
    message: Message,
    acknowledged: bool,
}

impl PersistentMessage {
    fn new(message: Message) -> Self {
        Self {
            message,
            acknowledged: false,
        }
    }
}

type PersistentMessageRc = Rc<RefCell<PersistentMessage>>;
type PersistentMessageWeak = Weak<RefCell<PersistentMessage>>;


#[derive(Debug)]
struct ResendEntry {
    pub persistent_message: PersistentMessageRc,
    pub resend_time: u64,
    pub send_count: u8,
}

impl ResendEntry {
    pub fn new(persistent_message: PersistentMessageRc, resend_time: u64, send_count: u8) -> Self {
        Self { persistent_message, resend_time, send_count }
    }
}

impl PartialOrd for ResendEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.resend_time.cmp(&other.resend_time).reverse())
    }
}

impl PartialEq for ResendEntry {
    fn eq(&self, other: &Self) -> bool {
        self.resend_time == other.resend_time
    }
}

impl Eq for ResendEntry {}

impl Ord for ResendEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.resend_time.cmp(&other.resend_time).reverse()
    }
}

type ResendQueue = BinaryHeap<ResendEntry>;


#[derive(Debug)]
struct SendEntry {
    message: Message,
    resend: bool,
}

impl SendEntry {
    fn new(message: Message, resend: bool) -> Self {
        Self {
            message,
            resend,
        }
    }
}


#[derive(Debug)]
struct SentFrame {
    send_time_ms: u64,
    persistent_messages: Box<[PersistentMessageWeak]>,
}


#[derive(Debug,PartialEq)]
pub enum Error {
    WindowLimited,
    DataLimited,
    SizeLimited,
}


pub struct FrameQueue {
    send_queue: VecDeque<SendEntry>,
    resend_queue: ResendQueue,

    next_id: u32,
    base_id: u32,
    sent_frames: VecDeque<SentFrame>,
}

impl FrameQueue {
    pub fn new(base_id: u32) -> Self {
        Self {
            send_queue: VecDeque::new(),
            resend_queue: ResendQueue::new(),

            next_id: base_id,
            base_id: base_id,
            sent_frames: VecDeque::new(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.send_queue.len() + self.resend_queue.len()
    }

    pub fn enqueue_message(&mut self, message: Message, resend: bool) {
        self.send_queue.push_back(SendEntry::new(message, resend));
    }

    pub fn emit_frame(&mut self, now_ms: u64, rto_ms: u64, size_limit: usize) -> Result<(Box<[u8]>, u32, bool), Error> {
        if self.sent_frames.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE as usize {
            return Err(Error::WindowLimited);
        }

        let frame_id = self.next_id;
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

            self.resend_queue.push(ResendEntry::new(entry.persistent_message,
                                                    now_ms + rto_ms*(1 << entry.send_count),
                                                    (entry.send_count + 1).min(MAX_SEND_COUNT)));
        }

        while let Some(entry) = self.send_queue.front() {
            let encoded_size = MessageFrameBuilder::message_size(&entry.message);

            if fbuilder.size() + encoded_size > size_limit {
                if fbuilder.count() == 0 {
                    return Err(Error::SizeLimited);
                } else {
                    break;
                }
            }

            fbuilder.add(&entry.message);

            let entry = self.send_queue.pop_front().unwrap();

            if entry.resend {
                let persistent_message = Rc::new(RefCell::new(PersistentMessage::new(entry.message)));

                persistent_messages.push(Rc::downgrade(&persistent_message));

                self.resend_queue.push(ResendEntry::new(persistent_message, now_ms + rto_ms, 1));
            }
        }

        if fbuilder.count() == 0 {
            // Nothing to send!
            return Err(Error::DataLimited);
        }

        self.next_id = self.next_id.wrapping_add(1);
        self.sent_frames.push_back(SentFrame {
            send_time_ms: now_ms,
            persistent_messages: persistent_messages.into(),
        });

        return Ok((fbuilder.build(), frame_id, nonce));
    }

    pub fn acknowledge_frames(&mut self, ack: FrameAck) {
        let mut ack_size = 0;
        for i in (0 .. 32).rev() {
            if ack.bitfield & (1 << i) != 0 {
                ack_size = i + 1;
                break;
            }
        }

        for i in 0 .. ack_size {
            if ack.bitfield & (1 << i) != 0 {
                let frame_id = ack.base_id.wrapping_add(i);

                if let Some(sent_frame) = self.sent_frames.get_mut(frame_id.wrapping_sub(self.base_id) as usize) {
                    let persistent_messages = std::mem::take(&mut sent_frame.persistent_messages);

                    for pmsg in persistent_messages.into_iter() {
                        if let Some(pmsg) = pmsg.upgrade() {
                            pmsg.borrow_mut().acknowledged = true;
                        }
                    }
                }
            }
        }
    }

    pub fn forget_frames(&mut self, thresh_ms: u64) {
        while let Some(frame) = self.sent_frames.front() {
            if frame.send_time_ms < thresh_ms {
                self.sent_frames.pop_front();
                self.base_id = self.base_id.wrapping_add(1);
            } else {
                return;
            }
        }
    }
}

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

