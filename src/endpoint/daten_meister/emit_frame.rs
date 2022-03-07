
use crate::MAX_FRAME_SIZE;
use crate::frame;
use crate::frame::serial::DataFrameBuilder;
use crate::frame::serial::AckFrameBuilder;

use super::packet_sender;
use super::pending_packet;
use super::pending_queue;
use super::resend_queue;
use super::frame_queue;
use super::frame_ack_queue;

const MAX_SEND_COUNT: u8 = 2;

enum EmitError {
    SizeLimited,
    WindowLimited,
}

struct InProgressDataFrame {
    nonce: bool,
    fbuilder: frame::serial::DataFrameBuilder,
    fragment_refs: Vec<pending_packet::FragmentRef>,
}

struct DataFrameEmitter<'a, F> {
    now_ms: u64,
    frame_queue: &'a mut frame_queue::FrameQueue,

    in_progress_frame: Option<InProgressDataFrame>,

    max_send_size: usize,
    bytes_remaining: usize,

    callback: F,
}

impl<'a, F> DataFrameEmitter<'a, F> where F: FnMut(Box<[u8]>) {
    pub fn new(now_ms: u64, frame_queue: &'a mut frame_queue::FrameQueue, max_send_size: usize, callback: F) -> Self {
        Self {
            now_ms,
            frame_queue,

            in_progress_frame: None,

            max_send_size,
            bytes_remaining: max_send_size,

            callback,
        }
    }

    fn push_initial(&mut self, packet_rc: &pending_packet::PendingPacketRc, fragment_id: u16, persistent: bool) -> Result<(), EmitError> {
        debug_assert!(self.in_progress_frame.is_none());

        if !self.frame_queue.can_push() {
            return Err(EmitError::WindowLimited);
        }

        let packet_ref = packet_rc.borrow();
        let datagram = packet_ref.datagram(fragment_id);

        let encoded_size = DataFrameBuilder::encoded_size_ref(&datagram);
        let potential_frame_size = frame::serial::DATA_FRAME_OVERHEAD + encoded_size;

        debug_assert!(potential_frame_size <= MAX_FRAME_SIZE);
        if potential_frame_size > self.bytes_remaining {
            self.frame_queue.mark_rate_limited();
            return Err(EmitError::SizeLimited);
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

    pub fn push(&mut self, packet_rc: &pending_packet::PendingPacketRc, fragment_id: u16, persistent: bool) -> Result<(), EmitError> {
        let packet_ref = packet_rc.borrow();
        let datagram = packet_ref.datagram(fragment_id);

        if let Some(ref mut next_frame) = self.in_progress_frame {
            // Try to add to in-progress frame
            let encoded_size = DataFrameBuilder::encoded_size_ref(&datagram);
            let potential_frame_size = next_frame.fbuilder.size() + encoded_size;

            if potential_frame_size > MAX_FRAME_SIZE {
                self.flush();
                return self.push_initial(packet_rc, fragment_id, persistent);
            } else if potential_frame_size > self.bytes_remaining {
                self.flush();
                self.frame_queue.mark_rate_limited();
                return Err(EmitError::SizeLimited);
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

    fn flush(&mut self) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_data = next_frame.fbuilder.build();
            let fragment_refs = next_frame.fragment_refs.into_boxed_slice();

            debug_assert!(self.frame_queue.can_push());
            self.frame_queue.push(frame_data.len(), self.now_ms, fragment_refs, next_frame.nonce);

            self.bytes_remaining -= frame_data.len();
            (self.callback)(frame_data);
        }
    }

    pub fn total_size(&self) -> usize {
        self.max_send_size - self.bytes_remaining
    }
}

pub struct FrameEmitter<'a> {
    packet_sender: &'a mut packet_sender::PacketSender,
    pending_queue: &'a mut pending_queue::PendingQueue,
    resend_queue: &'a mut resend_queue::ResendQueue,
    frame_queue: &'a mut frame_queue::FrameQueue,
    frame_ack_queue: &'a mut frame_ack_queue::FrameAckQueue,
    flush_id: u32,
}

impl<'a> FrameEmitter<'a> {
    pub fn new(packet_sender: &'a mut packet_sender::PacketSender,
               pending_queue: &'a mut pending_queue::PendingQueue,
               resend_queue: &'a mut resend_queue::ResendQueue,
               frame_queue: &'a mut frame_queue::FrameQueue,
               frame_ack_queue: &'a mut frame_ack_queue::FrameAckQueue,
               flush_id: u32) -> Self {
        Self {
            packet_sender,
            pending_queue,
            resend_queue,
            frame_queue,
            frame_ack_queue,
            flush_id,
        }
    }

    pub fn emit_data_frames<F>(&mut self, now_ms: u64, rtt_ms: u64, max_send_size: usize, emit_cb: F) -> usize where F: FnMut(Box<[u8]>) {
        let mut dfe = DataFrameEmitter::new(now_ms, self.frame_queue, max_send_size, emit_cb);

        while let Some(entry) = self.resend_queue.peek() {
            if let Some(packet_rc) = entry.fragment_ref.packet.upgrade() {
                let packet_ref = packet_rc.borrow();

                if packet_ref.fragment_acknowledged(entry.fragment_ref.fragment_id) {
                    self.resend_queue.pop();
                    continue;
                }

                if entry.resend_time > now_ms {
                    break;
                }

                match dfe.push(&packet_rc, entry.fragment_ref.fragment_id, true) {
                    Err(_) => return dfe.total_size(),
                    Ok(_) => (),
                }

                let entry = self.resend_queue.pop().unwrap();

                self.resend_queue.push(resend_queue::Entry::new(entry.fragment_ref,
                                                                now_ms + rtt_ms*(1 << entry.send_count),
                                                                (entry.send_count + 1).min(MAX_SEND_COUNT)));
            } else {
                self.resend_queue.pop();
                continue;
            }
        }

        loop {
            if self.pending_queue.is_empty() {
                if let Some((packet_rc, resend)) = self.packet_sender.emit_packet(self.flush_id) {
                    let pending_packet_ref = packet_rc.borrow();

                    let last_fragment_id = pending_packet_ref.last_fragment_id();
                    for i in 0 ..= last_fragment_id {
                        let fragment_ref = pending_packet::FragmentRef::new(&packet_rc, i);
                        let entry = pending_queue::Entry::new(fragment_ref, resend);
                        self.pending_queue.push_back(entry);
                    }
                } else {
                    break;
                }
            }

            while let Some(entry) = self.pending_queue.front() {
                if let Some(packet_rc) = entry.fragment_ref.packet.upgrade() {
                    let packet_ref = packet_rc.borrow();

                    if packet_ref.fragment_acknowledged(entry.fragment_ref.fragment_id) {
                        self.resend_queue.pop();
                        continue;
                    }

                    match dfe.push(&packet_rc, entry.fragment_ref.fragment_id, entry.resend) {
                        Err(_) => return dfe.total_size(),
                        Ok(_) => (),
                    }

                    let entry = self.pending_queue.pop_front().unwrap();

                    if entry.resend {
                        self.resend_queue.push(resend_queue::Entry::new(entry.fragment_ref, now_ms + rtt_ms, 1));
                    }
                } else {
                    self.resend_queue.pop();
                    continue;
                }
            }
        }

        dfe.flush();
        return dfe.total_size();
    }

    pub fn emit_sync_frame<F>(&mut self,
                              next_frame_id: Option<u32>,
                              next_packet_id: Option<u32>,
                              max_send_size: usize,
                              mut emit_cb: F) -> usize where F: FnMut(Box<[u8]>) {
        if frame::serial::SYNC_FRAME_SIZE > max_send_size {
            return 0;
        }

        let frame = frame::Frame::SyncFrame(frame::SyncFrame { next_frame_id, next_packet_id });

        use frame::serial::Serialize;
        let frame_data = frame.write();

        emit_cb(frame_data);

        return frame::serial::SYNC_FRAME_SIZE;
    }

    pub fn emit_ack_frames<F>(&mut self,
                              frame_window_base_id: u32,
                              packet_window_base_id: u32,
                              max_send_size: usize,
                              min_one: bool,
                              mut emit_cb: F) -> usize where F: FnMut(Box<[u8]>) {
        let mut bytes_remaining = max_send_size;
        let mut frame_sent = false;

        let mut fbuilder = AckFrameBuilder::new(frame_window_base_id, packet_window_base_id);

        let potential_frame_size = fbuilder.size();
        if potential_frame_size > bytes_remaining {
            return 0;
        }

        while let Some(frame_ack) = self.frame_ack_queue.peek() {
            let encoded_size = AckFrameBuilder::encoded_size(&frame_ack);
            let potential_frame_size = fbuilder.size() + encoded_size;

            if potential_frame_size > bytes_remaining {
                if fbuilder.count() > 0 || min_one && !frame_sent {
                    let frame_data = fbuilder.build();
                    bytes_remaining -= frame_data.len();
                    emit_cb(frame_data);
                }

                return max_send_size - bytes_remaining;
            }

            if potential_frame_size > MAX_FRAME_SIZE {
                debug_assert!(fbuilder.count() > 0);

                let frame_data = fbuilder.build();
                bytes_remaining -= frame_data.len();
                frame_sent = true;
                emit_cb(frame_data);

                fbuilder = AckFrameBuilder::new(frame_window_base_id, packet_window_base_id);
                continue;
            }

            fbuilder.add(&frame_ack);

            self.frame_ack_queue.pop();
        }

        if fbuilder.count() > 0 || min_one && !frame_sent {
            let frame_data = fbuilder.build();
            bytes_remaining -= frame_data.len();
            emit_cb(frame_data);
        }

        return max_send_size - bytes_remaining;
    }
}

