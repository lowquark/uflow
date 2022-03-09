
use crate::MAX_FRAME_SIZE;
use crate::frame;
use crate::frame::serial::DataFrameBuilder;
use crate::frame::serial::AckFrameBuilder;

use super::pending_packet;
use super::frame_queue;

#[derive(Debug,PartialEq)]
pub enum DataPushError {
    SizeLimited,
    WindowLimited,
}

struct InProgressDataFrame {
    fbuilder: frame::serial::DataFrameBuilder,
    fragment_refs: Vec<pending_packet::FragmentRef>,
    nonce: bool,
}

pub struct DataFrameEmitter<'a, F> {
    now_ms: u64,
    frame_queue: &'a mut frame_queue::FrameQueue,

    in_progress_frame: Option<InProgressDataFrame>,
    bytes_remaining: usize,
    emit_cb: F,
}

impl<'a, F> DataFrameEmitter<'a, F> where F: FnMut(Box<[u8]>) {
    pub fn new(now_ms: u64, frame_queue: &'a mut frame_queue::FrameQueue, max_send_size: usize, emit_cb: F) -> Self {
        Self {
            now_ms,
            frame_queue,

            in_progress_frame: None,
            bytes_remaining: max_send_size,
            emit_cb,
        }
    }

    pub fn push(&mut self, packet_rc: &pending_packet::PendingPacketRc, fragment_id: u16, persistent: bool) -> Result<(), DataPushError> {
        let packet_ref = packet_rc.borrow();
        let datagram = packet_ref.datagram(fragment_id);

        let encoded_size = DataFrameBuilder::encoded_size(&datagram);

        if let Some(ref mut next_frame) = self.in_progress_frame {
            let potential_frame_size = next_frame.fbuilder.size() + encoded_size;

            if potential_frame_size > self.bytes_remaining {
                // Emit in-progress frame
                self.finalize();
                // Will occur below (overhead increases with new frame)
                self.frame_queue.mark_rate_limited();
                return Err(DataPushError::SizeLimited);
            } else if potential_frame_size > MAX_FRAME_SIZE {
                // Emit in-progress frame
                self.finalize();
            } else {
                // Add datagram to in-progress frame
                next_frame.fbuilder.add(&datagram);
                debug_assert!(next_frame.fbuilder.size() == potential_frame_size);

                if persistent {
                    next_frame.fragment_refs.push(pending_packet::FragmentRef::new(packet_rc, fragment_id));
                }

                return Ok(());
            }
        }

        // Try to start a new frame with this datagram
        let potential_frame_size = DataFrameBuilder::INITIAL_SIZE + encoded_size;

        debug_assert!(potential_frame_size <= MAX_FRAME_SIZE);

        if potential_frame_size > self.bytes_remaining {
            self.frame_queue.mark_rate_limited();
            return Err(DataPushError::SizeLimited);
        }

        if !self.frame_queue.can_push() {
            return Err(DataPushError::WindowLimited);
        }

        let frame_id = self.frame_queue.next_id();
        let nonce = rand::random();

        let mut next_frame = InProgressDataFrame {
            fbuilder: DataFrameBuilder::new(frame_id, nonce),
            fragment_refs: Vec::new(),
            nonce,
        };

        // Add datagram to new frame
        next_frame.fbuilder.add(&datagram);
        debug_assert!(next_frame.fbuilder.size() == potential_frame_size);

        if persistent {
            next_frame.fragment_refs.push(pending_packet::FragmentRef::new(packet_rc, fragment_id));
        }

        debug_assert!(self.in_progress_frame.is_none());
        self.in_progress_frame = Some(next_frame);

        return Ok(());
    }

    pub fn finalize(&mut self) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_bytes = next_frame.fbuilder.build();
            let fragment_refs = next_frame.fragment_refs.into_boxed_slice();

            debug_assert!(self.frame_queue.can_push());
            self.frame_queue.push(frame_bytes.len(), self.now_ms, fragment_refs, next_frame.nonce);

            self.bytes_remaining -= frame_bytes.len();
            (self.emit_cb)(frame_bytes);
        }
    }
}

pub struct AckFrameEmitter<F> {
    frame_window_base_id: u32,
    packet_window_base_id: u32,

    in_progress_frame: Option<frame::serial::AckFrameBuilder>,
    bytes_remaining: usize,
    emit_cb: F,
}

impl<F> AckFrameEmitter<F> where F: FnMut(Box<[u8]>) {
    pub fn new(frame_window_base_id: u32, packet_window_base_id: u32, min_one: bool, max_send_size: usize, emit_cb: F) -> Self {
        let potential_frame_size = AckFrameBuilder::INITIAL_SIZE;

        let in_progress_frame = if min_one && potential_frame_size <= max_send_size {
            Some(AckFrameBuilder::new(frame_window_base_id, packet_window_base_id))
        } else {
            None
        };

        Self {
            frame_window_base_id,
            packet_window_base_id,

            in_progress_frame,
            bytes_remaining: max_send_size,
            emit_cb,
        }
    }

    pub fn push(&mut self, ack_group: &frame::AckGroup) -> Result<(), ()> {
        let encoded_size = AckFrameBuilder::encoded_size(ack_group);

        if let Some(ref mut next_frame) = self.in_progress_frame {
            // Try to add to in-progress frame
            let potential_frame_size = next_frame.size() + encoded_size;

            if potential_frame_size > self.bytes_remaining {
                // Emit in-progress frame
                self.finalize();
                // Will occur below (overhead increases with new frame)
                return Err(());
            } else if potential_frame_size > MAX_FRAME_SIZE {
                // Emit in-progress frame
                self.finalize();
            } else {
                // Add ack group to in-progress frame
                next_frame.add(ack_group);
                return Ok(());
            }
        }

        // Try to start a new frame with this ack group
        let potential_frame_size = AckFrameBuilder::INITIAL_SIZE + encoded_size;

        debug_assert!(potential_frame_size <= MAX_FRAME_SIZE);

        if potential_frame_size > self.bytes_remaining {
            return Err(());
        }

        // Add ack group to new frame
        let mut fbuilder = AckFrameBuilder::new(self.frame_window_base_id, self.packet_window_base_id);
        fbuilder.add(ack_group);
        debug_assert!(fbuilder.size() == potential_frame_size);

        debug_assert!(self.in_progress_frame.is_none());
        self.in_progress_frame = Some(fbuilder);

        return Ok(());
    }

    pub fn finalize(&mut self) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_bytes = next_frame.build();
            self.bytes_remaining -= frame_bytes.len();
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

    fn max_datagram_test(max_send_size: usize, window_size: u32, push_count: usize, final_result: Result<(),DataPushError>) -> Vec<Box<[u8]>> {
        let now_ms = 0;

        let mut fq = frame_queue::FrameQueue::new(0, window_size, window_size);

        let mut frames = Vec::new();
        let emit_cb = |frame_bytes: Box<[u8]>| {
            frames.push(frame_bytes);
        };

        let mut dfe = DataFrameEmitter::new(now_ms, &mut fq, max_send_size, emit_cb);

        let packet_bytes = (0 .. 2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        let packet_rc = Rc::new(RefCell::new(PendingPacket::new(packet_bytes, 0, 0, 0, 0)));

        for _ in 0 .. push_count - 1 {
            assert_eq!(dfe.push(&packet_rc, 0, false), Ok(()));
        }
        assert_eq!(dfe.push(&packet_rc, 0, false), final_result);
        dfe.finalize();

        return frames;
    }

    #[test]
    fn data_max_frame_size() {
        let frames = max_datagram_test(2*MAX_FRAME_SIZE, MAX_FRAME_WINDOW_SIZE, 2, Ok(()));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].len(), MAX_FRAME_SIZE);
        assert_eq!(frames[1].len(), MAX_FRAME_SIZE);
    }

    #[test]
    fn data_size_limited() {
        let frames = max_datagram_test(0, MAX_FRAME_WINDOW_SIZE, 1, Err(DataPushError::SizeLimited));
        assert_eq!(frames.len(), 0);
        let frames = max_datagram_test(1, MAX_FRAME_WINDOW_SIZE, 1, Err(DataPushError::SizeLimited));
        assert_eq!(frames.len(), 0);

        let frames = max_datagram_test(MAX_FRAME_SIZE - 1, MAX_FRAME_WINDOW_SIZE, 1, Err(DataPushError::SizeLimited));
        assert_eq!(frames.len(), 0);
        let frames = max_datagram_test(MAX_FRAME_SIZE    , MAX_FRAME_WINDOW_SIZE, 2, Err(DataPushError::SizeLimited));
        assert_eq!(frames.len(), 1);
        let frames = max_datagram_test(MAX_FRAME_SIZE + 1, MAX_FRAME_WINDOW_SIZE, 2, Err(DataPushError::SizeLimited));
        assert_eq!(frames.len(), 1);

        let frames = max_datagram_test(2*MAX_FRAME_SIZE - 1, MAX_FRAME_WINDOW_SIZE, 2, Err(DataPushError::SizeLimited));
        assert_eq!(frames.len(), 1);
        let frames = max_datagram_test(2*MAX_FRAME_SIZE    , MAX_FRAME_WINDOW_SIZE, 3, Err(DataPushError::SizeLimited));
        assert_eq!(frames.len(), 2);
    }

    #[test]
    fn data_window_limited() {
        let frames = max_datagram_test(6*MAX_FRAME_SIZE, 5, 6, Err(DataPushError::WindowLimited));
        assert_eq!(frames.len(), 5);
    }
}

