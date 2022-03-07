
use crate::MAX_FRAME_SIZE;
use crate::frame;
use crate::frame::serial::DataFrameBuilder;
use crate::frame::serial::AckFrameBuilder;

use super::pending_packet;
use super::frame_queue;

pub enum DataPushError {
    SizeLimited,
    WindowLimited,
}

struct InProgressDataFrame {
    nonce: bool,
    fbuilder: frame::serial::DataFrameBuilder,
    fragment_refs: Vec<pending_packet::FragmentRef>,
}

pub struct DataFrameEmitter<'a, F> {
    now_ms: u64,
    frame_queue: &'a mut frame_queue::FrameQueue,

    in_progress_frame: Option<InProgressDataFrame>,

    bytes_remaining: usize,

    callback: F,
}

impl<'a, F> DataFrameEmitter<'a, F> where F: FnMut(Box<[u8]>) {
    pub fn new(now_ms: u64, frame_queue: &'a mut frame_queue::FrameQueue, max_send_size: usize, callback: F) -> Self {
        Self {
            now_ms,
            frame_queue,

            in_progress_frame: None,

            bytes_remaining: max_send_size,

            callback,
        }
    }

    fn push_initial(&mut self, packet_rc: &pending_packet::PendingPacketRc, fragment_id: u16, persistent: bool) -> Result<(), DataPushError> {
        debug_assert!(self.in_progress_frame.is_none());

        if !self.frame_queue.can_push() {
            return Err(DataPushError::WindowLimited);
        }

        let packet_ref = packet_rc.borrow();
        let datagram = packet_ref.datagram(fragment_id);

        let encoded_size = DataFrameBuilder::encoded_size(&datagram);
        let potential_frame_size = DataFrameBuilder::INITIAL_SIZE + encoded_size;

        debug_assert!(potential_frame_size <= MAX_FRAME_SIZE);

        if potential_frame_size > self.bytes_remaining {
            self.frame_queue.mark_rate_limited();
            return Err(DataPushError::SizeLimited);
        }

        let frame_id = self.frame_queue.next_id();
        let nonce = rand::random();

        let mut fbuilder = DataFrameBuilder::new(frame_id, nonce);
        fbuilder.add(&datagram);

        debug_assert!(fbuilder.size() == potential_frame_size);

        let mut fragment_refs = Vec::new();
        if persistent {
            fragment_refs.push(pending_packet::FragmentRef::new(packet_rc, fragment_id));
        }

        self.in_progress_frame = Some(InProgressDataFrame {
            nonce,
            fbuilder,
            fragment_refs,
        });

        return Ok(());
    }

    pub fn push(&mut self, packet_rc: &pending_packet::PendingPacketRc, fragment_id: u16, persistent: bool) -> Result<(), DataPushError> {
        let packet_ref = packet_rc.borrow();
        let datagram = packet_ref.datagram(fragment_id);

        if let Some(ref mut next_frame) = self.in_progress_frame {
            // Try to add to in-progress frame
            let encoded_size = DataFrameBuilder::encoded_size(&datagram);
            let potential_frame_size = next_frame.fbuilder.size() + encoded_size;

            if potential_frame_size > MAX_FRAME_SIZE {
                self.finalize();
                return self.push_initial(packet_rc, fragment_id, persistent);
            } else if potential_frame_size > self.bytes_remaining {
                self.finalize();
                self.frame_queue.mark_rate_limited();
                return Err(DataPushError::SizeLimited);
            } else {
                next_frame.fbuilder.add(&datagram);
                if persistent {
                    next_frame.fragment_refs.push(pending_packet::FragmentRef::new(packet_rc, fragment_id));
                }
                return Ok(());
            }
        } else {
            // No in-progress frame
            return self.push_initial(packet_rc, fragment_id, persistent);
        }
    }

    pub fn finalize(&mut self) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_data = next_frame.fbuilder.build();
            let fragment_refs = next_frame.fragment_refs.into_boxed_slice();

            debug_assert!(self.frame_queue.can_push());
            self.frame_queue.push(frame_data.len(), self.now_ms, fragment_refs, next_frame.nonce);

            self.bytes_remaining -= frame_data.len();
            (self.callback)(frame_data);
        }
    }
}

pub struct AckFrameEmitter<F> {
    frame_window_base_id: u32,
    packet_window_base_id: u32,

    in_progress_frame: Option<frame::serial::AckFrameBuilder>,

    bytes_remaining: usize,

    callback: F,
}

impl<F> AckFrameEmitter<F> where F: FnMut(Box<[u8]>) {
    pub fn new(frame_window_base_id: u32, packet_window_base_id: u32, min_one: bool, max_send_size: usize, callback: F) -> Self {
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

            callback,
        }
    }

    fn push_initial(&mut self, ack_group: &frame::AckGroup) -> Result<(), ()> {
        debug_assert!(self.in_progress_frame.is_none());

        let encoded_size = AckFrameBuilder::encoded_size(ack_group);
        let potential_frame_size = AckFrameBuilder::INITIAL_SIZE + encoded_size;

        debug_assert!(potential_frame_size <= MAX_FRAME_SIZE);

        if potential_frame_size > self.bytes_remaining {
            return Err(());
        }

        let mut fbuilder = AckFrameBuilder::new(self.frame_window_base_id, self.packet_window_base_id);
        fbuilder.add(ack_group);

        debug_assert!(fbuilder.size() == potential_frame_size);

        self.in_progress_frame = Some(fbuilder);

        return Ok(());
    }

    pub fn push(&mut self, ack_group: &frame::AckGroup) -> Result<(), ()> {
        if let Some(ref mut next_frame) = self.in_progress_frame {
            // Try to add to in-progress frame
            let encoded_size = AckFrameBuilder::encoded_size(ack_group);
            let potential_frame_size = next_frame.size() + encoded_size;

            if potential_frame_size > MAX_FRAME_SIZE {
                self.finalize();
                return self.push_initial(ack_group);
            } else if potential_frame_size > self.bytes_remaining {
                self.finalize();
                return Err(());
            } else {
                next_frame.add(ack_group);
                return Ok(());
            }
        } else {
            // No in-progress frame
            return self.push_initial(ack_group);
        }
    }

    pub fn finalize(&mut self) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_bytes = next_frame.build();
            self.bytes_remaining -= frame_bytes.len();
            (self.callback)(frame_bytes);
        }
    }
}

