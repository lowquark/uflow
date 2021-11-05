
use std::collections::VecDeque;
use std::time;

use super::frame;
use super::DataSink;
use super::TRANSFER_WINDOW_SIZE;

const SENTINEL_FRAME_SPACING: u32 = TRANSFER_WINDOW_SIZE/2;
const FRAME_SEND_COUNT_MAX: u8 = 3;

#[derive(Debug)]
struct MixedFrame {
    pub data: Box<[u8]>,
    pub resend_rel_msgs: Box<[frame::DataEntry]>,
}

#[derive(Debug)]
struct ReliableFrame {
    pub data: Box<[u8]>,
}

#[derive(Debug)]
struct UnreliableFrame {
    pub data: Box<[u8]>,
}

#[derive(Debug)]
enum Frame {
    Unreliable(UnreliableFrame),
    Reliable(ReliableFrame),
    Mixed(MixedFrame),
}

impl Frame {
    fn data(&self) -> &Box<[u8]> {
        match self {
            Frame::Unreliable(frame) => &frame.data,
            Frame::Reliable(frame) => &frame.data,
            Frame::Mixed(frame) => &frame.data,
        }
    }
}

#[derive(Debug)]
struct TransferEntry {
    sequence_id: u32,
    frame: Frame,
    last_send_time: Option<time::Instant>,
    send_count: u8,
    remove: bool,
}

impl TransferEntry {
    fn encode_null_frame(sequence_id: u32) -> Box<[u8]> {
        return frame::Data::new(false, sequence_id, vec![]).to_bytes();
    }

    fn encode_basic_frame(sequence_id: u32, msgs: Box<[frame::DataEntry]>) -> Box<[u8]> {
        return frame::Data::new(false, sequence_id, msgs.into_vec()).to_bytes();
    }

    fn encode_mixed_frame(sequence_id: u32,
                          unrel_msgs: Box<[frame::DataEntry]>,
                          rel_msgs: Box<[frame::DataEntry]>) -> (Box<[u8]>, Box<[frame::DataEntry]>) {
        // TODO: Special frame encoder
        let mut all_msgs = rel_msgs.clone().into_vec();
        all_msgs.append(&mut unrel_msgs.into_vec());

        let frame = frame::Data::new(false, sequence_id, all_msgs).to_bytes();

        return (frame, rel_msgs);
    }

    fn new_unreliable(sequence_id: u32, msgs: Box<[frame::DataEntry]>) -> Self {
        Self {
            sequence_id: sequence_id,
            frame: Frame::Unreliable(UnreliableFrame {
                data: Self::encode_basic_frame(sequence_id, msgs),
            }),
            last_send_time: None,
            send_count: 0,
            remove: false,
        }
    }

    fn new_reliable(sequence_id: u32, msgs: Box<[frame::DataEntry]>) -> Self {
        Self {
            sequence_id: sequence_id,
            frame: Frame::Reliable(ReliableFrame {
                data: Self::encode_basic_frame(sequence_id, msgs),
            }),
            last_send_time: None,
            send_count: 0,
            remove: false,
        }
    }

    fn new_mixed(sequence_id: u32, unrel_msgs: Box<[frame::DataEntry]>, rel_msgs: Box<[frame::DataEntry]>) -> Self {
        let (data, rel_msgs) = Self::encode_mixed_frame(sequence_id, unrel_msgs, rel_msgs);

        Self {
            sequence_id: sequence_id,
            frame: Frame::Mixed(MixedFrame {
                data: data,
                resend_rel_msgs: rel_msgs,
            }),
            last_send_time: None,
            send_count: 0,
            remove: false,
        }
    }

    fn apply_drop(&mut self) {
        match self.frame {
            Frame::Unreliable(ref frame) => {
                if self.sequence_id % SENTINEL_FRAME_SPACING == SENTINEL_FRAME_SPACING - 1 {
                    // Convert to sentinel before resending
                    self.frame = Frame::Reliable(ReliableFrame {
                        data: Self::encode_null_frame(self.sequence_id),
                    });
                } else {
                    // Remove this frame later
                    self.remove = true;
                }
            }
            Frame::Reliable(ref frame) => {
                // Do nothing, just resend
            }
            Frame::Mixed(ref mut frame) => {
                // Convert to reliable before resending
                self.frame = Frame::Reliable(ReliableFrame {
                    data: Self::encode_basic_frame(self.sequence_id, std::mem::take(&mut frame.resend_rel_msgs)),
                });
            }
        }
    }

    fn frame_data(&self) -> &Box<[u8]> {
        self.frame.data()
    }
}

pub struct TransferQueue {
    entries: VecDeque<TransferEntry>,
    size: usize,
    next_sequence_id: u32,
    base_sequence_id: u32,
}

impl TransferQueue {
    pub fn new(tx_sequence_id: u32) -> Self {
        Self {
            entries: VecDeque::new(),
            size: 0,
            next_sequence_id: tx_sequence_id,
            base_sequence_id: tx_sequence_id,
        }
    }

    fn new_sequence_id(&mut self) -> u32 {
        let sequence_id = self.next_sequence_id;
        self.next_sequence_id = self.next_sequence_id.wrapping_add(1);

        // This would mean the entire sequence ID space (2^32 sequence IDs) has been allocated, but
        // we'll test for it anyway
        debug_assert!(self.next_sequence_id != self.base_sequence_id);

        return sequence_id;
    }

    fn push(&mut self, entry: TransferEntry) {
        self.size += entry.frame_data().len();
        self.entries.push_back(entry);
    }

    pub fn push_unreliable(&mut self, msgs: Vec<frame::DataEntry>) {
        let entry = TransferEntry::new_unreliable(self.new_sequence_id(), msgs.into_boxed_slice());
        self.push(entry);
    }

    pub fn push_reliable(&mut self, msgs: Vec<frame::DataEntry>) {
        let entry = TransferEntry::new_reliable(self.new_sequence_id(), msgs.into_boxed_slice());
        self.push(entry);
    }

    pub fn push_mixed(&mut self, unrel_msgs: Vec<frame::DataEntry>, rel_msgs: Vec<frame::DataEntry>) {
        let entry = TransferEntry::new_mixed(self.new_sequence_id(), unrel_msgs.into_boxed_slice(), rel_msgs.into_boxed_slice());
        self.push(entry);
    }

    fn remove_partial(&mut self, sequence_id: u32) -> u32 {
        let lead = sequence_id.wrapping_sub(self.base_sequence_id);

        match self.entries.binary_search_by(|entry| entry.sequence_id.wrapping_sub(self.base_sequence_id).cmp(&lead)) {
            Ok(idx) => {
                let ref mut entry = self.entries[idx];
                assert!(entry.sequence_id == sequence_id);

                self.size -= entry.frame_data().len();
                entry.remove = true;

                return 1;
            }
            _ => {
                return 0;
            }
        }
    }

    pub fn remove_frames(&mut self, sequence_ids: Vec<u32>) -> u32 {
        let mut num_removals = 0;

        for sequence_id in sequence_ids.into_iter() {
            num_removals += self.remove_partial(sequence_id);
        }

        if let Some(newest_entry) = self.entries.back() {
            let newest_sequence_id = newest_entry.sequence_id;

            self.entries.retain(|entry| !entry.remove);

            if let Some(entry) = self.entries.front() {
                self.base_sequence_id = entry.sequence_id;
            } else {
                self.base_sequence_id = newest_sequence_id.wrapping_add(1);
            }
        }

        return num_removals;
    }

    fn send_frame(entry: &mut TransferEntry, now: time::Instant, sink: & dyn DataSink) {
        sink.send(&entry.frame_data());

        entry.last_send_time = Some(now);

        entry.send_count += 1;
        if entry.send_count >= FRAME_SEND_COUNT_MAX {
            entry.send_count = FRAME_SEND_COUNT_MAX;
        }
    }

    pub fn send_pending_frames(&mut self, now: time::Instant, time_rto: time::Duration, sink: & dyn DataSink) -> u32 {
        let mut num_resends = 0;

        for entry in self.entries.iter_mut() {
            if let Some(time_sent) = entry.last_send_time {
                let timeout_scale = 1 << (entry.send_count - 1);
                let was_dropped = now - time_sent >= time_rto*timeout_scale;

                if was_dropped {
                    num_resends += 1;

                    self.size -= entry.frame_data().len();
                    entry.apply_drop();

                    if !entry.remove {
                        self.size += entry.frame_data().len();

                        Self::send_frame(entry, now, sink);
                    }
                }
            } else {
                Self::send_frame(entry, now, sink);
            }
        }

        self.entries.retain(|entry| !entry.remove);

        return num_resends;
    }

    fn size(&self) -> usize {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::TransferQueue;
    use super::DataSink;
    use super::frame;
    use super::SENTINEL_FRAME_SPACING;
    use std::time;
    use std::collections::VecDeque;
    use std::cell::RefCell;

    struct TestSink {
        sent_frames: RefCell<VecDeque<Box<[u8]>>>,
    }

    impl DataSink for TestSink {
        fn send(&self, data: &[u8]) {
            self.sent_frames.borrow_mut().push_back(data.into());
        }
    }

    impl TestSink {
        fn new() -> Self {
            Self {
                sent_frames: RefCell::new(VecDeque::new()),
            }
        }

        fn assert_sent(&self, frames: Vec<Box<[u8]>>) {
            let frames_deque: VecDeque<Box<[u8]>> = frames.into();
            assert_eq!(*self.sent_frames.borrow(), frames_deque);
        }

        fn clear_sent(&mut self) {
            self.sent_frames.borrow_mut().clear();
        }
    }

    fn random_data(size: usize) -> Box<[u8]> {
        (0..size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice()
    }

    fn random_frame_data_entry() -> frame::DataEntry {
        frame::DataEntry::new(rand::random::<u8>(), frame::Message::Datagram(
            frame::Datagram::new(rand::random::<u32>() & 0xFFFFFF, 0x0000, frame::Payload::Fragment(
                frame::Fragment::new(0x0000, 0x0000, random_data(32))))))
    }

    fn random_frame(sequence_id: u32) -> (Vec<frame::DataEntry>, Box<[u8]>) {
        let msgs = (1..2).map(|_| random_frame_data_entry()).collect::<Vec<_>>();

        let frame = frame::Data::new(false, sequence_id, msgs.clone()).to_bytes();

        (msgs, frame)
    }

    fn random_mixed_frame(sequence_id: u32) -> (Vec<frame::DataEntry>, Vec<frame::DataEntry>, Box<[u8]>, Box<[u8]>) {
        let unrel_msgs = (1..2).map(|_| random_frame_data_entry()).collect::<Vec<_>>();
        let rel_msgs = (1..2).map(|_| random_frame_data_entry()).collect::<Vec<_>>();

        let mut all_msgs = rel_msgs.clone();
        all_msgs.append(&mut unrel_msgs.clone());

        let first_frame = frame::Data::new(false, sequence_id, all_msgs).to_bytes();
        let resend_frame = frame::Data::new(false, sequence_id, rel_msgs.clone()).to_bytes();

        (unrel_msgs, rel_msgs, first_frame, resend_frame)
    }

    struct TestScheduleEntry {
        time: time::Instant,
        frames: Vec<Box<[u8]>>,
        size: usize,
    }

    fn test_schedule(transfer_queue: &mut TransferQueue, rto: time::Duration, schedule: Vec<TestScheduleEntry>) {
        for entry in schedule.into_iter() {
            let test_sink = TestSink::new();
            transfer_queue.send_pending_frames(entry.time, rto, &test_sink);
            test_sink.assert_sent(entry.frames);

            assert_eq!(transfer_queue.size(), entry.size);
        }
    }

    #[test]
    fn test_unreliable_timeout() {
        let mut transfer_queue = TransferQueue::new(0);
        let (msgs, frame) = random_frame(0);
        transfer_queue.push_unreliable(msgs);

        let t0 = time::Instant::now();
        let rto = time::Duration::from_millis(100);
        let eps = time::Duration::from_millis(1);

        test_schedule(&mut transfer_queue, rto, vec![
            TestScheduleEntry{ time: t0              , frames: vec![ frame.clone() ], size: frame.len() },
            TestScheduleEntry{ time: t0         + eps, frames: vec![               ], size: frame.len() },

            TestScheduleEntry{ time: t0 +   rto - eps, frames: vec![               ], size: frame.len() },
            TestScheduleEntry{ time: t0 +   rto      , frames: vec![               ], size: 0           },
        ]);
    }

    #[test]
    fn test_unreliable_timeout_sentinel() {
        let mut transfer_queue = TransferQueue::new(SENTINEL_FRAME_SPACING-1);
        let (msgs, frame) = random_frame(SENTINEL_FRAME_SPACING-1);
        transfer_queue.push_unreliable(msgs);

        let sentinel_frame = frame::Data::new(false, SENTINEL_FRAME_SPACING-1, vec![]).to_bytes();

        let t0 = time::Instant::now();
        let rto = time::Duration::from_millis(100);
        let eps = time::Duration::from_millis(1);

        test_schedule(&mut transfer_queue, rto, vec![
            TestScheduleEntry{ time: t0              , frames: vec![ frame.clone()          ], size: frame.len() },
            TestScheduleEntry{ time: t0         + eps, frames: vec![                        ], size: frame.len() },

            TestScheduleEntry{ time: t0 +   rto - eps, frames: vec![                        ], size: frame.len() },
            TestScheduleEntry{ time: t0 +   rto      , frames: vec![ sentinel_frame.clone() ], size: sentinel_frame.len() },
            TestScheduleEntry{ time: t0 +   rto + eps, frames: vec![                        ], size: sentinel_frame.len() },

            TestScheduleEntry{ time: t0 + 3*rto - eps, frames: vec![                        ], size: sentinel_frame.len() },
            TestScheduleEntry{ time: t0 + 3*rto      , frames: vec![ sentinel_frame.clone() ], size: sentinel_frame.len() },
            TestScheduleEntry{ time: t0 + 3*rto + eps, frames: vec![                        ], size: sentinel_frame.len() },

            TestScheduleEntry{ time: t0 + 7*rto - eps, frames: vec![                        ], size: sentinel_frame.len() },
            TestScheduleEntry{ time: t0 + 7*rto      , frames: vec![ sentinel_frame.clone() ], size: sentinel_frame.len() },
            TestScheduleEntry{ time: t0 + 7*rto + eps, frames: vec![                        ], size: sentinel_frame.len() },
        ]);
    }

    #[test]
    fn test_reliable_timeout() {
        let mut transfer_queue = TransferQueue::new(0);
        let (msgs, frame) = random_frame(0);
        transfer_queue.push_reliable(msgs);

        let t0 = time::Instant::now();
        let rto = time::Duration::from_millis(100);
        let eps = time::Duration::from_millis(1);

        test_schedule(&mut transfer_queue, rto, vec![
            TestScheduleEntry{ time: t0              , frames: vec![ frame.clone() ], size: frame.len() },
            TestScheduleEntry{ time: t0         + eps, frames: vec![               ], size: frame.len() },

            TestScheduleEntry{ time: t0 +   rto - eps, frames: vec![               ], size: frame.len() },
            TestScheduleEntry{ time: t0 +   rto      , frames: vec![ frame.clone() ], size: frame.len() },
            TestScheduleEntry{ time: t0 +   rto + eps, frames: vec![               ], size: frame.len() },

            TestScheduleEntry{ time: t0 + 3*rto - eps, frames: vec![               ], size: frame.len() },
            TestScheduleEntry{ time: t0 + 3*rto      , frames: vec![ frame.clone() ], size: frame.len() },
            TestScheduleEntry{ time: t0 + 3*rto + eps, frames: vec![               ], size: frame.len() },

            TestScheduleEntry{ time: t0 + 7*rto - eps, frames: vec![               ], size: frame.len() },
            TestScheduleEntry{ time: t0 + 7*rto      , frames: vec![ frame.clone() ], size: frame.len() },
            TestScheduleEntry{ time: t0 + 7*rto + eps, frames: vec![               ], size: frame.len() },
        ]);
    }

    #[test]
    fn test_mixed_timeout() {
        let mut transfer_queue = TransferQueue::new(0);
        let (unrel_msgs, rel_msgs, first_frame, resend_frame) = random_mixed_frame(0);
        transfer_queue.push_mixed(unrel_msgs, rel_msgs);

        let t0 = time::Instant::now();
        let rto = time::Duration::from_millis(100);
        let eps = time::Duration::from_millis(1);

        test_schedule(&mut transfer_queue, rto, vec![
            TestScheduleEntry{ time: t0              , frames: vec![ first_frame.clone()  ], size: first_frame.len() },
            TestScheduleEntry{ time: t0         + eps, frames: vec![                      ], size: first_frame.len() },

            TestScheduleEntry{ time: t0 +   rto - eps, frames: vec![                      ], size: first_frame.len() },
            TestScheduleEntry{ time: t0 +   rto      , frames: vec![ resend_frame.clone() ], size: resend_frame.len() },
            TestScheduleEntry{ time: t0 +   rto + eps, frames: vec![                      ], size: resend_frame.len() },

            TestScheduleEntry{ time: t0 + 3*rto - eps, frames: vec![                      ], size: resend_frame.len() },
            TestScheduleEntry{ time: t0 + 3*rto      , frames: vec![ resend_frame.clone() ], size: resend_frame.len() },
            TestScheduleEntry{ time: t0 + 3*rto + eps, frames: vec![                      ], size: resend_frame.len() },

            TestScheduleEntry{ time: t0 + 7*rto - eps, frames: vec![                      ], size: resend_frame.len() },
            TestScheduleEntry{ time: t0 + 7*rto      , frames: vec![ resend_frame.clone() ], size: resend_frame.len() },
            TestScheduleEntry{ time: t0 + 7*rto + eps, frames: vec![                      ], size: resend_frame.len() },
        ]);
    }

    // May also test resend count, removal count
}

