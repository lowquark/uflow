
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

        if fbuilder.size() >= size_limit {
            return Err(Error::SizeLimited);
        }

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
        let ack_size = ack.size.min(32) as u32;

        for i in 0 .. ack_size {
            let frame_id = ack.base_id.wrapping_add(i);

            if ack.bitfield & (1 << i) != 0 {
                if let Some(sent_frame) = self.sent_frames.get_mut(frame_id.wrapping_sub(self.base_id) as usize) {
                    let persistent_messages = std::mem::take(&mut sent_frame.persistent_messages);

                    for pmsg in persistent_messages.into_iter() {
                        if let Some(pmsg) = pmsg.upgrade() {
                            pmsg.borrow_mut().acknowledged = true;
                        }
                    }
                } else {
                    return;
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

/*
#[cfg(test)]
mod tests {
    use crate::frame;
    use crate::frame::FragmentId;
    use crate::frame::Datagram;
    use crate::frame::Message;
    use crate::frame::Frame;
    use crate::frame::Serialize;

    use super::FrameSender;
    use super::super::MAX_FRAGMENT_SIZE;
    use super::super::MAX_TRANSFER_UNIT;

    use std::collections::VecDeque;

    fn new_dummy_message(sequence_id: u32, data_size: usize) -> Message {
        Message::Datagram(Datagram {
            sequence_id,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: (0 .. data_size).map(|i| i as u8).collect::<Vec<_>>().into(),
        })
    }

    fn new_max_message(sequence_id: u32) -> Message {
        Message::Datagram(Datagram {
            sequence_id,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: (0 .. MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<_>>().into(),
        })
    }

    struct TestFrameSink {
        frames: VecDeque<Box<[u8]>>,
    }

    impl TestFrameSink {
        fn new() -> Self {
            Self {
                frames: VecDeque::new(),
            }
        }

        fn pop(&mut self) -> Box<[u8]> {
            self.frames.pop_front().unwrap()
        }

        fn is_empty(&mut self) -> bool {
            self.frames.is_empty()
        }
    }

    impl super::FrameSink for TestFrameSink {
        fn send(&mut self, frame_bytes: Box<[u8]>) {
            self.frames.push_back(frame_bytes);
        }
    }

    fn verify_frame(frame_data: Box<[u8]>, sequence_id: u32, messages: Vec<Message>) {
        let frame = frame::Frame::read(&frame_data).unwrap();

        if let Frame::MessageFrame(frame) = frame {
            assert_eq!(frame.sequence_id, sequence_id);
            assert_eq!(frame.messages, messages);
        } else {
            panic!("Expected Frame::MessageFrame");
        }
    }

    fn compute_nonce(frame_list: Vec<Box<[u8]>>) -> bool {
        let mut nonce = false;

        for frame_data in frame_list.iter() {
            let frame = frame::Frame::read(&frame_data).unwrap();

            if let Frame::MessageFrame(frame) = frame {
                nonce ^= frame.nonce;
            } else {
                panic!("Expected Frame::MessageFrame");
            }
        }

        return nonce;
    }

    #[test]
    fn basic() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        let msg0 = new_dummy_message(0, 15);
        let msg1 = new_dummy_message(1, 15);
        let msg2 = new_dummy_message(2, 15);

        fs.enqueue_message(msg0.clone(), false);
        fs.enqueue_message(msg1.clone(), false);
        fs.enqueue_message(msg2.clone(), false);

        assert_eq!(fs.pending_count(), 3);

        fs.emit_frames(0, rto_ms, 10000, &mut sink);

        assert_eq!(fs.pending_count(), 0);

        verify_frame(sink.pop(), 0, vec![ msg0, msg1, msg2 ]);
        assert!(sink.is_empty());
    }

    #[test]
    fn max_message() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        let message = new_max_message(0);

        fs.enqueue_message(message.clone(), false);
        fs.emit_frames(0, rto_ms, 10000, &mut sink);

        let frame_data = sink.pop();
        assert!(sink.is_empty());

        assert!(frame_data.len() == MAX_TRANSFER_UNIT);

        verify_frame(frame_data, 0, vec![ message ]);
    }

    #[test]
    fn total_size_limit() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        let msg0 = new_max_message(0);
        let msg1 = new_max_message(1);
        let msg2 = new_max_message(2);
        let msg3 = new_max_message(3);
        let msg4 = new_dummy_message(4, 400);
        let msg5 = new_max_message(5);
        let msg6 = new_dummy_message(6, 400);
        let msg7 = new_dummy_message(7, 400);

        fs.enqueue_message(msg0.clone(), false);
        fs.enqueue_message(msg1.clone(), false);
        fs.enqueue_message(msg2.clone(), false);
        fs.enqueue_message(msg3.clone(), false);
        fs.enqueue_message(msg4.clone(), false);
        fs.enqueue_message(msg5.clone(), false);
        fs.enqueue_message(msg6.clone(), false);
        fs.enqueue_message(msg7.clone(), false);

        fs.emit_frames(0, rto_ms, MAX_TRANSFER_UNIT*3, &mut sink);

        verify_frame(sink.pop(), 0, vec![ msg0 ]);
        verify_frame(sink.pop(), 1, vec![ msg1 ]);
        verify_frame(sink.pop(), 2, vec![ msg2 ]);
        assert!(sink.is_empty());

        fs.emit_frames(0, rto_ms, MAX_TRANSFER_UNIT*1, &mut sink);

        verify_frame(sink.pop(), 3, vec![ msg3 ]);
        assert!(sink.is_empty());

        fs.emit_frames(0, rto_ms, MAX_TRANSFER_UNIT*2, &mut sink);

        verify_frame(sink.pop(), 4, vec![ msg4 ]);
        verify_frame(sink.pop(), 5, vec![ msg5 ]);
        verify_frame(sink.pop(), 6, vec![ msg6, msg7 ]);
        assert!(sink.is_empty());
    }

    #[test]
    fn resends() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        let msg = new_dummy_message(4, 400);
        fs.enqueue_message(msg.clone(), true);

        assert_eq!(fs.pending_count(), 1);

        fs.emit_frames(0, rto_ms, 10000, &mut sink);
        verify_frame(sink.pop(), 0, vec![ msg.clone() ]);
        assert!(sink.is_empty());

        fs.emit_frames(1, rto_ms, 10000, &mut sink);
        assert!(sink.is_empty());

        let resend_times = [ rto_ms, 3*rto_ms, 7*rto_ms, 15*rto_ms, 31*rto_ms, 47*rto_ms, 63*rto_ms ];

        let mut frame_id = 1;

        for time_ms in resend_times.iter() {
            assert_eq!(fs.pending_count(), 1);

            fs.emit_frames(*time_ms - 1, rto_ms, 10000, &mut sink);
            assert!(sink.is_empty());

            fs.emit_frames(*time_ms, rto_ms, 10000, &mut sink);
            verify_frame(sink.pop(), frame_id, vec![ msg.clone() ]);
            assert!(sink.is_empty());

            fs.emit_frames(*time_ms + 1, rto_ms, 10000, &mut sink);
            assert!(sink.is_empty());

            frame_id += 1;
        }

        assert_eq!(fs.pending_count(), 1);
    }

    #[test]
    fn basic_acknowledgement() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

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
        fs.emit_frames(0, rto_ms, 10000, &mut sink);

        let f0 = sink.pop();
        let _f1 = sink.pop();
        let f2 = sink.pop();
        let f3 = sink.pop();
        let f4 = sink.pop();
        assert!(sink.is_empty());

        assert_eq!(fs.acknowledge_frames(0, 0b11101, compute_nonce(vec![ f0, f2, f3, f4 ])), true);

        // All but frame 1 acknowledged, resend will contain msg1
        fs.emit_frames(rto_ms, rto_ms, 10000, &mut sink);
        verify_frame(sink.pop(), 5, vec![ msg1.clone() ]);
        assert!(sink.is_empty());
    }

    #[test]
    fn bad_acknowledgement() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        let msg0 = new_max_message(0);
        let msg1 = new_max_message(1);
        let msg2 = new_max_message(2);
        let msg3 = new_max_message(3);
        let msg4 = new_max_message(4);

        fs.enqueue_message(msg0.clone(), true);
        fs.emit_frames(0, rto_ms, 10000, &mut sink);
        fs.enqueue_message(msg1.clone(), true);
        fs.emit_frames(1, rto_ms, 10000, &mut sink);
        fs.enqueue_message(msg2.clone(), true);
        fs.emit_frames(2, rto_ms, 10000, &mut sink);
        fs.enqueue_message(msg3.clone(), true);
        fs.emit_frames(3, rto_ms, 10000, &mut sink);
        fs.enqueue_message(msg4.clone(), true);
        fs.emit_frames(4, rto_ms, 10000, &mut sink);

        let f0 = sink.pop();
        let f1 = sink.pop();
        let f2 = sink.pop();
        let f3 = sink.pop();
        let f4 = sink.pop();
        assert!(sink.is_empty());

        // Compute bad nonce
        assert_eq!(fs.acknowledge_frames(0, 0b11111, !compute_nonce(vec![ f0, f1, f2, f3, f4 ])), false);

        // Nothing acknowledged - everything resent
        fs.emit_frames(rto_ms + 5, rto_ms, 10000, &mut sink);
        verify_frame(sink.pop(), 5, vec![ msg0.clone() ]);
        verify_frame(sink.pop(), 6, vec![ msg1.clone() ]);
        verify_frame(sink.pop(), 7, vec![ msg2.clone() ]);
        verify_frame(sink.pop(), 8, vec![ msg3.clone() ]);
        verify_frame(sink.pop(), 9, vec![ msg4.clone() ]);
        assert!(sink.is_empty());
    }

    #[test]
    fn bad_acknowledgement_base() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        let msg0 = new_max_message(0);
        let msg1 = new_max_message(1);

        fs.enqueue_message(msg0.clone(), true);
        fs.enqueue_message(msg1.clone(), true);
        fs.emit_frames(0, rto_ms, 10000, &mut sink);

        let f0 = sink.pop();
        let f1 = sink.pop();
        assert!(sink.is_empty());

        assert_eq!(fs.acknowledge_frames(2, 0b00011, compute_nonce(vec![ f0, f1 ])), false);
    }

    #[test]
    fn max_acknowledgement() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        for msg_id in 0..32 {
            let msg = new_max_message(msg_id);
            fs.enqueue_message(msg, true);
            fs.emit_frames(0, rto_ms, 10000, &mut sink);
        }

        let mut frames = Vec::new();
        for _ in 0..32 {
            frames.push(sink.pop());
        }
        assert!(sink.is_empty());

        // Compute bad nonce
        assert_eq!(fs.acknowledge_frames(0, 0xFFFFFFFF, compute_nonce(frames)), true);

        // Nothing acknowledged - everything resent
        fs.emit_frames(rto_ms, rto_ms, 10000, &mut sink);
        assert!(sink.is_empty());
    }

    #[test]
    fn forget_frames() {
        let mut fs = FrameSender::new(0);
        let mut sink = TestFrameSink::new();

        let rto_ms = 100;

        let msg0 = new_max_message(0);
        let msg1 = new_max_message(1);
        let msg2 = new_max_message(2);
        let msg3 = new_max_message(3);
        let msg4 = new_max_message(4);

        fs.enqueue_message(msg0.clone(), true);
        fs.emit_frames(0, rto_ms, 10000, &mut sink);
        fs.enqueue_message(msg1.clone(), true);
        fs.emit_frames(1, rto_ms, 10000, &mut sink);
        fs.enqueue_message(msg2.clone(), true);
        fs.enqueue_message(msg3.clone(), true);
        fs.enqueue_message(msg4.clone(), true);
        fs.emit_frames(10, rto_ms, 10000, &mut sink);

        let f0 = sink.pop();
        let f1 = sink.pop();
        let f2 = sink.pop();
        let f3 = sink.pop();
        let f4 = sink.pop();
        assert!(sink.is_empty());

        fs.forget_frames(10);

        assert_eq!(fs.acknowledge_frames(0,  0b11, compute_nonce(vec![ f0, f1 ])), false);
        assert_eq!(fs.acknowledge_frames(2, 0b111, compute_nonce(vec![ f2, f3, f4 ])), true);

        fs.emit_frames(rto_ms + 2, rto_ms, 10000, &mut sink);
        verify_frame(sink.pop(), 5, vec![ msg0.clone() ]);
        verify_frame(sink.pop(), 6, vec![ msg1.clone() ]);
        assert!(sink.is_empty());
    }
}
*/

