
use crate::frame;
use crate::frame::serial::AckFrameBuilder;
use crate::frame::serial::DataFrameBuilder;
use crate::MAX_FRAME_SIZE;
use crate::MAX_FRAME_WINDOW_SIZE;
use crate::packet_id;

use super::pending_packet;
use super::frame_queue;

#[derive(Debug,PartialEq)]
pub enum DataPushError {
    SizeLimited,
    WindowLimited,
}

struct InProgressDataFrame {
    fbuilder: frame::serial::DataFrameBuilder,
    resend_refs: Vec<pending_packet::FragmentRef>,
    nonce: bool,
}

pub struct DataFrameEmitter<'a, F> {
    now_ms: u64,
    frame_queue: &'a mut frame_queue::FrameQueue,

    in_progress_frame: Option<InProgressDataFrame>,
    flush_alloc: isize,
    emit_cb: F,
}

impl<'a, F> DataFrameEmitter<'a, F> where F: FnMut(Box<[u8]>) {
    pub fn new(now_ms: u64, frame_queue: &'a mut frame_queue::FrameQueue, flush_alloc: isize, emit_cb: F) -> Self {
        Self {
            now_ms,
            frame_queue,

            in_progress_frame: None,
            flush_alloc,
            emit_cb,
        }
    }

    // Returns Ok(()) if the datagram was added successfully
    // Returns Err(DataPushError) if the datagram could not be added
    pub fn push(&mut self, packet_rc: &pending_packet::PendingPacketRc, fragment_id: u16, resend: bool) -> Result<(), DataPushError> {
        let packet_ref = packet_rc.borrow();
        let datagram = packet_ref.datagram(fragment_id);

        if let Some(ref mut next_frame) = self.in_progress_frame {
            // Try to add to existing frame
            let frame_size = next_frame.fbuilder.size();
            let potential_frame_size = frame_size + DataFrameBuilder::encoded_size(&datagram);

            // Restrict the number of datagrams per frame to ensure that packet IDs are unique over
            // the receiver's frame window, which has size MAX_FRAME_WINDOW_SIZE * 2. I.e.:
            //
            //    max_packet_count * MAX_FRAME_WINDOW_SIZE * 2 <= packet_id::SPAN

            let max_packet_count =
                ((packet_id::SPAN / (MAX_FRAME_WINDOW_SIZE * 2)) as usize).min(frame::serial::DataFrameBuilder::MAX_COUNT);

            if (self.flush_alloc - frame_size as isize) < 0 {
                // Out of bandwidth
                self.finalize();
                self.frame_queue.mark_rate_limited();
                return Err(DataPushError::SizeLimited);
            } else if potential_frame_size > MAX_FRAME_SIZE || next_frame.fbuilder.count() >= max_packet_count {
                // Would exceed maximum
                self.finalize();
            } else {
                next_frame.fbuilder.add(&datagram);
                debug_assert!(next_frame.fbuilder.size() == potential_frame_size);
                if resend {
                    next_frame.resend_refs.push(pending_packet::FragmentRef::new(packet_rc, fragment_id));
                }
                return Ok(());
            }
        }

        // Try to add to new frame
        if self.flush_alloc < 0 {
            // Out of bandwidth
            self.frame_queue.mark_rate_limited();
            return Err(DataPushError::SizeLimited);
        }

        if !self.frame_queue.can_push() {
            // Would exceed window
            return Err(DataPushError::WindowLimited);
        }

        let frame_id = self.frame_queue.next_id();
        let nonce = rand::random();

        let mut next_frame = InProgressDataFrame {
            fbuilder: DataFrameBuilder::new(frame_id, nonce),
            resend_refs: Vec::new(),
            nonce,
        };

        next_frame.fbuilder.add(&datagram);
        if resend {
            next_frame.resend_refs.push(pending_packet::FragmentRef::new(packet_rc, fragment_id));
        }

        debug_assert!(self.in_progress_frame.is_none());
        self.in_progress_frame = Some(next_frame);

        return Ok(());
    }

    pub fn finalize(&mut self) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_bytes = next_frame.fbuilder.build();
            let resend_refs = next_frame.resend_refs.into_boxed_slice();

            debug_assert!(self.frame_queue.can_push());
            self.frame_queue.push(frame_bytes.len(), self.now_ms, resend_refs, next_frame.nonce);

            self.flush_alloc -= frame_bytes.len() as isize;
            (self.emit_cb)(frame_bytes);
        }
    }
}

pub struct AckFrameEmitter<F> {
    frame_window_base_id: u32,
    packet_window_base_id: u32,

    in_progress_frame: Option<frame::serial::AckFrameBuilder>,
    flush_alloc: isize,
    emit_cb: F,
}

impl<F> AckFrameEmitter<F> where F: FnMut(Box<[u8]>) {
    pub fn new(frame_window_base_id: u32, packet_window_base_id: u32, flush_alloc: isize, emit_cb: F) -> Self {
        Self {
            frame_window_base_id,
            packet_window_base_id,

            in_progress_frame: None,
            flush_alloc,
            emit_cb,
        }
    }

    pub fn push_dud(&mut self) -> Result<(), ()> {
        if self.in_progress_frame.is_some() {
            return Ok(());
        }

        // Try to start a new frame
        if self.flush_alloc < 0 {
            // Out of bandwidth
            return Err(());
        }

        let fbuilder = AckFrameBuilder::new(self.frame_window_base_id, self.packet_window_base_id);

        debug_assert!(self.in_progress_frame.is_none());
        self.in_progress_frame = Some(fbuilder);

        return Ok(());
    }

    // Returns Ok(()) if the ack group was added successfully
    // Returns Err(()) if the ack group could not be added
    pub fn push(&mut self, ack_group: &frame::AckGroup) -> Result<(), ()> {
        if let Some(ref mut next_frame) = self.in_progress_frame {
            // Try to add to existing frame
            let frame_size = next_frame.size();
            let potential_frame_size = frame_size + AckFrameBuilder::encoded_size(ack_group);

            if (self.flush_alloc - frame_size as isize) < 0 {
                // Out of bandwidth
                self.finalize();
                return Err(());
            } else if potential_frame_size > MAX_FRAME_SIZE {
                // Would exceed maximum
                self.finalize();
            } else {
                next_frame.add(ack_group);
                debug_assert!(next_frame.size() == potential_frame_size);
                return Ok(());
            }
        }

        // Try to add to new frame
        if self.flush_alloc < 0 {
            // Out of bandwidth
            return Err(());
        }

        let mut fbuilder = AckFrameBuilder::new(self.frame_window_base_id, self.packet_window_base_id);
        fbuilder.add(ack_group);

        debug_assert!(self.in_progress_frame.is_none());
        self.in_progress_frame = Some(fbuilder);

        return Ok(());
    }

    pub fn finalize(&mut self) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_bytes = next_frame.build();
            self.flush_alloc -= frame_bytes.len() as isize;
            (self.emit_cb)(frame_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pending_packet::PendingPacket;

    use crate::MAX_FRAME_WINDOW_SIZE;
    use crate::MAX_FRAGMENT_SIZE;

    use std::rc::Rc;
    use std::cell::RefCell;

    fn max_datagram_test(flush_alloc: isize, window_size: u32, push_count: usize, final_result: Result<(),DataPushError>) -> Vec<Box<[u8]>> {
        let now_ms = 0;

        let mut fq = frame_queue::FrameQueue::new(window_size, window_size, 0);

        let mut frames = Vec::new();
        let emit_cb = |frame_bytes: Box<[u8]>| {
            frames.push(frame_bytes);
        };

        let mut dfe = DataFrameEmitter::new(now_ms, &mut fq, flush_alloc, emit_cb);

        let packet_bytes = (0 .. 2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        let packet_rc = Rc::new(RefCell::new(PendingPacket::new(packet_bytes, 0, 0, 0, 0)));

        for _ in 0 .. push_count - 1 {
            assert_eq!(dfe.push(&packet_rc, 0, false), Ok(()));
        }
        assert_eq!(dfe.push(&packet_rc, 0, false), final_result);
        dfe.finalize();

        return frames;
    }

    fn datagram_test(flush_alloc: isize, payload_size: usize, push_count: usize, final_result: Result<(),DataPushError>) -> Vec<frame::DataFrame> {
        let now_ms = 0;

        let mut fq = frame_queue::FrameQueue::new(MAX_FRAME_WINDOW_SIZE, MAX_FRAME_WINDOW_SIZE, 0);

        let mut frames = Vec::new();
        let emit_cb = |frame_bytes: Box<[u8]>| {
            frames.push(frame_bytes);
        };

        let mut dfe = DataFrameEmitter::new(now_ms, &mut fq, flush_alloc, emit_cb);

        let packet_bytes = (0 .. payload_size).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        let packet_rc = Rc::new(RefCell::new(PendingPacket::new(packet_bytes, 0, 0, 0, 0)));

        for _ in 0 .. push_count - 1 {
            assert_eq!(dfe.push(&packet_rc, 0, false), Ok(()));
        }
        assert_eq!(dfe.push(&packet_rc, 0, false), final_result);
        dfe.finalize();

        use frame::serial::Serialize;

        return frames.iter().map(|frame_bytes| {
            match frame::Frame::read(&frame_bytes) {
                Some(frame::Frame::DataFrame(data_frame)) => data_frame,
                _ => panic!(),
            }
        }).collect::<Vec<_>>();
    }

    fn ack_test(flush_alloc: isize, push_count: usize, final_result: Result<(),()>) -> Vec<frame::AckFrame> {
        let mut frames = Vec::new();
        let emit_cb = |frame_bytes: Box<[u8]>| {
            frames.push(frame_bytes);
        };

        let mut afe = AckFrameEmitter::new(0, 0, flush_alloc, emit_cb);

        let ack_group = frame::AckGroup { base_id: 0, bitfield: 0, nonce: false };

        for _ in 0 .. push_count - 1 {
            assert_eq!(afe.push(&ack_group), Ok(()));
        }
        assert_eq!(afe.push(&ack_group), final_result);
        afe.finalize();

        use frame::serial::Serialize;

        return frames.iter().map(|frame_bytes| {
            match frame::Frame::read(&frame_bytes) {
                Some(frame::Frame::AckFrame(ack_frame)) => ack_frame,
                _ => panic!(),
            }
        }).collect::<Vec<_>>();
    }

    #[test]
    fn data_max_frame_size() {
        let frames = max_datagram_test(2 * MAX_FRAME_SIZE as isize, MAX_FRAME_WINDOW_SIZE, 2, Ok(()));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].len(), MAX_FRAME_SIZE);
        assert_eq!(frames[1].len(), MAX_FRAME_SIZE);
    }

    #[test]
    fn data_size_limited() {
        let payload_len = 5;
        let datagram_overhead = frame::serial::MIN_DATAGRAM_OVERHEAD;
        let frame_overhead = frame::serial::DATA_FRAME_OVERHEAD;

        let len_a = (frame_overhead + 1 * (datagram_overhead + payload_len)) as isize;
        let len_b = (frame_overhead + 2 * (datagram_overhead + payload_len)) as isize;

        let test_cases: Vec<(isize, usize, usize, Result<(),DataPushError>)> = vec![
            ( 0        , 1, 1, Ok(()) ),
            ( len_a - 1, 1, 1, Ok(()) ),
            ( len_a    , 1, 1, Ok(()) ),
            ( len_a + 1, 1, 1, Ok(()) ),

            ( len_a    , 2, 2, Ok(()) ),
            ( len_b - 1, 2, 2, Ok(()) ),
            ( len_b    , 2, 2, Ok(()) ),
            ( len_b + 1, 2, 2, Ok(()) ),

            ( len_b    , 3, 3, Ok(()) ),

            ( 0        , 2, 1, Err(DataPushError::SizeLimited) ),
            ( len_a - 1, 2, 1, Err(DataPushError::SizeLimited) ),
            ( len_a    , 2, 2, Ok(()) ),
            ( len_a + 1, 2, 2, Ok(()) ),

            ( len_a    , 3, 2, Err(DataPushError::SizeLimited) ),
            ( len_b - 1, 3, 2, Err(DataPushError::SizeLimited) ),
            ( len_b    , 3, 3, Ok(()) ),
            ( len_b + 1, 3, 3, Ok(()) ),

            ( len_b    , 4, 3, Err(DataPushError::SizeLimited) ),
        ];

        for (idx, test) in test_cases.into_iter().enumerate() {
            println!("{}", idx);
            let frames = datagram_test(test.0, payload_len, test.1, test.3);
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].datagrams.len(), test.2);
        }
    }

    #[test]
    fn data_window_limited() {
        let frames = max_datagram_test(6 * MAX_FRAME_SIZE as isize, 5, 6, Err(DataPushError::WindowLimited));
        assert_eq!(frames.len(), 5);
    }

    #[test]
    fn ack_max_frame_size() {
        let max_datagrams = (MAX_FRAME_SIZE - frame::serial::ACK_FRAME_OVERHEAD) / frame::serial::ACK_GROUP_SIZE;

        let frames = ack_test(2 * MAX_FRAME_SIZE as isize, max_datagrams, Ok(()));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].frame_acks.len(), max_datagrams);

        let frames = ack_test(2 * MAX_FRAME_SIZE as isize, max_datagrams + 1, Ok(()));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].frame_acks.len(), max_datagrams);
        assert_eq!(frames[1].frame_acks.len(), 1);
    }

    #[test]
    fn ack_size_limited() {
        let len_a = (frame::serial::ACK_FRAME_OVERHEAD + 1 * frame::serial::ACK_GROUP_SIZE) as isize;
        let len_b = (frame::serial::ACK_FRAME_OVERHEAD + 2 * frame::serial::ACK_GROUP_SIZE) as isize;

        let test_cases: Vec<(isize, usize, usize, Result<(),()>)> = vec![
            ( 0        , 1, 1, Ok(()) ),
            ( len_a - 1, 1, 1, Ok(()) ),
            ( len_a    , 1, 1, Ok(()) ),
            ( len_a + 1, 1, 1, Ok(()) ),

            ( len_a    , 2, 2, Ok(()) ),
            ( len_b - 1, 2, 2, Ok(()) ),
            ( len_b    , 2, 2, Ok(()) ),
            ( len_b + 1, 2, 2, Ok(()) ),

            ( len_b    , 3, 3, Ok(()) ),

            ( 0        , 2, 1, Err(()) ),
            ( len_a - 1, 2, 1, Err(()) ),
            ( len_a    , 2, 2, Ok(()) ),
            ( len_a + 1, 2, 2, Ok(()) ),

            ( len_a    , 3, 2, Err(()) ),
            ( len_b - 1, 3, 2, Err(()) ),
            ( len_b    , 3, 3, Ok(()) ),
            ( len_b + 1, 3, 3, Ok(()) ),

            ( len_b    , 4, 3, Err(()) ),
        ];

        for (idx, test) in test_cases.into_iter().enumerate() {
            println!("{}", idx);
            let frames = ack_test(test.0, test.1, test.3);
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].frame_acks.len(), test.2);
        }
    }

    #[test]
    fn ack_min_one() {
        let mut frames = Vec::new();
        let emit_cb = |frame_bytes: Box<[u8]>| {
            frames.push(frame_bytes);
        };

        let mut afe = AckFrameEmitter::new(0, 0, MAX_FRAME_SIZE as isize, emit_cb);
        assert_eq!(afe.push_dud(), Ok(()));
        afe.finalize();

        assert_eq!(frames.len(), 1);
    }
}

