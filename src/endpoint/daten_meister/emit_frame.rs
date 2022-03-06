
use crate::MAX_FRAME_SIZE;
use crate::frame;
use crate::frame::serial::DataFrameBuilder;
use crate::frame::serial::AckFrameBuilder;

use super::packet_sender;
use super::pending_packet;
use super::datagram_queue;
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
                self.flush_internal(false);
                return self.push_initial(packet_rc, fragment_id, persistent);
            } else if potential_frame_size > self.bytes_remaining {
                self.flush_internal(true);
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

    fn flush_internal(&mut self, rate_limited: bool) {
        if let Some(next_frame) = self.in_progress_frame.take() {
            let frame_data = next_frame.fbuilder.build();
            let fragment_refs = next_frame.fragment_refs.into_boxed_slice();

            debug_assert!(self.frame_queue.can_push());
            self.frame_queue.push(frame_data.len(), self.now_ms, fragment_refs, next_frame.nonce, rate_limited);

            self.bytes_remaining -= frame_data.len();
            (self.callback)(frame_data);
        }
    }

    pub fn flush(&mut self) {
        self.flush_internal(false);
    }

    pub fn total_size(&self) -> usize {
        self.max_send_size - self.bytes_remaining
    }
}

pub struct FrameEmitter<'a> {
    packet_sender: &'a mut packet_sender::PacketSender,
    datagram_queue: &'a mut datagram_queue::DatagramQueue,
    resend_queue: &'a mut resend_queue::ResendQueue,
    frame_queue: &'a mut frame_queue::FrameQueue,
    frame_ack_queue: &'a mut frame_ack_queue::FrameAckQueue,
    flush_id: u32,
}

impl<'a> FrameEmitter<'a> {
    pub fn new(packet_sender: &'a mut packet_sender::PacketSender,
               datagram_queue: &'a mut datagram_queue::DatagramQueue,
               resend_queue: &'a mut resend_queue::ResendQueue,
               frame_queue: &'a mut frame_queue::FrameQueue,
               frame_ack_queue: &'a mut frame_ack_queue::FrameAckQueue,
               flush_id: u32) -> Self {
        Self {
            packet_sender,
            datagram_queue,
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
            if self.datagram_queue.is_empty() {
                if let Some((packet_rc, resend)) = self.packet_sender.emit_packet(self.flush_id) {
                    let pending_packet_ref = packet_rc.borrow();

                    let last_fragment_id = pending_packet_ref.last_fragment_id();
                    for i in 0 ..= last_fragment_id {
                        let fragment_ref = pending_packet::FragmentRef::new(&packet_rc, i);
                        let entry = datagram_queue::Entry::new(fragment_ref, resend);
                        self.datagram_queue.push_back(entry);
                    }
                } else {
                    break;
                }
            }

            while let Some(entry) = self.datagram_queue.front() {
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

                    let entry = self.datagram_queue.pop_front().unwrap();

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
                              next_frame_id: u32,
                              next_packet_id: u32,
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
                              mut emit_cb: F) -> usize where F: FnMut(Box<[u8]>) {
        let mut bytes_remaining = max_send_size;

        let mut fbuilder = AckFrameBuilder::new(frame_window_base_id, packet_window_base_id);

        while let Some(frame_ack) = self.frame_ack_queue.peek() {
            let encoded_size = AckFrameBuilder::encoded_size(&frame_ack);
            let potential_frame_size = fbuilder.size() + encoded_size;

            if potential_frame_size > bytes_remaining {
                if fbuilder.count() > 0 {
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
                emit_cb(frame_data);

                fbuilder = AckFrameBuilder::new(frame_window_base_id, packet_window_base_id);
                continue;
            }

            fbuilder.add(&frame_ack);

            self.frame_ack_queue.pop();
        }

        if fbuilder.count() > 0 {
            let frame_data = fbuilder.build();
            bytes_remaining -= frame_data.len();
            emit_cb(frame_data);
        }

        return max_send_size - bytes_remaining;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /*
    use crate::SendMode;
    use crate::frame::Datagram;
    use crate::frame::FragmentId;

    use crate::MAX_FRAGMENT_SIZE;

    use std::collections::VecDeque;

    #[derive(Debug)]
    struct DataFrame {
        data: Box<[u8]>,
        id: u32,
    }

    fn test_emit_data_frames(ps: &mut packet_sender::PacketSender,
                             dq: &mut datagram_queue::DatagramQueue,
                             rq: &mut resend_queue::ResendQueue,
                             fq: &mut frame_queue::FrameQueue,
                             faq: &mut frame_ack_queue::FrameAckQueue,
                             fid: u32,
                             now_ms: u64,
                             rtt_ms: u64,
                             max_send_size: usize) -> (VecDeque<DataFrame>, usize, bool) {
        let mut dfe = FrameEmitter::new(ps, dq, rq, fq, faq, fid);
        let mut emitted = VecDeque::new();
        let (total_size, send_size_limited) =
            dfe.emit_data_frames(now_ms, rtt_ms, max_send_size, |data, id, _nonce| {
                emitted.push_back(DataFrame { data, id });
            });
        return (emitted, total_size, send_size_limited);
    }

    fn test_data_frame(frame: &DataFrame, sequence_id: u32, datagrams: Vec<Datagram>) {
        use crate::frame::serial::Serialize;

        assert_eq!(frame.id, sequence_id);

        match frame::Frame::read(&frame.data).unwrap() {
            frame::Frame::DataFrame(read_data_frame) => {
                assert_eq!(read_data_frame.sequence_id, sequence_id);
                assert_eq!(read_data_frame.datagrams, datagrams);
            }
            _ => panic!("Expected DataFrame")
        }
    }

    #[test]
    fn basic() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 10000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        ps.enqueue_packet(vec![ 0, 0, 0 ].into_boxed_slice(), 0, SendMode::Unreliable, fid);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg0 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: vec![ 0, 0, 0 ].into_boxed_slice(),
        };

        test_data_frame(&frames[0], 0, vec![ dg0 ]);
    }

    #[test]
    fn max_frame_size() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 10000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let packet_data = (0 .. 2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ps.enqueue_packet(packet_data.clone(), 0, SendMode::Unreliable, fid);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 2);

        let dg0 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: packet_data[ .. MAX_FRAGMENT_SIZE].into(),
        };

        let dg1 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        };

        test_data_frame(&frames[0], 0, vec![ dg0 ]);
        test_data_frame(&frames[1], 1, vec![ dg1 ]);

        assert_eq!(frames[0].data.len(), MAX_FRAME_SIZE);
        assert_eq!(frames[1].data.len(), MAX_FRAME_SIZE);
    }

    #[test]
    fn size_limited_flag() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 10000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        // No data
        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, 0);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, false);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, false);

        let p0 = (0 .. 2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ps.enqueue_packet(p0.clone(), 0, SendMode::Resend, fid);

        // Send path
        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE-1);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, false);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, false);

        // Resend path
        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE-1);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, false);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, false);
    }

    #[test]
    fn resend_timing() {
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 10000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let p0 = (0 .. 400).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ps.enqueue_packet(p0.clone(), 0, SendMode::Resend, fid);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 0, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 1, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 0);

        let resend_times = [ rtt_ms, 3*rtt_ms, 7*rtt_ms, 11*rtt_ms, 15*rtt_ms, 19*rtt_ms, 23*rtt_ms ];

        for time_ms in resend_times.iter() {
            let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, *time_ms - 1, rtt_ms, MAX_FRAME_SIZE);
            assert_eq!(frames.len(), 0);

            let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, *time_ms    , rtt_ms, MAX_FRAME_SIZE);
            assert_eq!(frames.len(), 1);

            let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, *time_ms + 1, rtt_ms, MAX_FRAME_SIZE);
            assert_eq!(frames.len(), 0);
        }
    }

    #[test]
    fn time_sensitive_drop() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 10000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();

        ps.enqueue_packet(vec![ 0, 0, 0 ].into_boxed_slice(), 0, SendMode::TimeSensitive, 0);
        ps.enqueue_packet(vec![ 1, 1, 1 ].into_boxed_slice(), 0, SendMode::Unreliable, 0);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, 1, now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg0 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: vec![ 1, 1, 1 ].into_boxed_slice(),
        };

        test_data_frame(&frames[0], 0, vec![ dg0 ]);
    }

    #[test]
    fn basic_acknowledgement() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 10000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let p0 = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p1 = vec![ 1; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p2 = vec![ 2; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p3 = vec![ 3; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p4 = vec![ 4; MAX_FRAGMENT_SIZE ].into_boxed_slice();

        ps.enqueue_packet(p0        , 0, SendMode::Resend, 0);
        ps.enqueue_packet(p1.clone(), 0, SendMode::Resend, 0);
        ps.enqueue_packet(p2        , 0, SendMode::Resend, 0);
        ps.enqueue_packet(p3        , 0, SendMode::Resend, 0);
        ps.enqueue_packet(p4        , 0, SendMode::Resend, 0);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        fq.acknowledge_group(frame::AckGroup { base_id: 0, bitfield: 0b11101, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg1 = Datagram {
            sequence_id: 1,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: p1,
        };

        test_data_frame(&frames[0], 5, vec![ dg1 ]);
    }

    #[test]
    fn max_acknowledgement() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 48000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        for _ in 0..32 {
            let packet_data = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
            ps.enqueue_packet(packet_data, 0, SendMode::Resend, fid);
        }

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 32);

        fq.acknowledge_group(frame::AckGroup { base_id: 0, bitfield: 0xFFFFFFFF, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn multi_acknowledgement() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 48000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        for _ in 0..32 {
            let packet_data = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
            ps.enqueue_packet(packet_data, 0, SendMode::Resend, fid);
        }

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 32);

        fq.acknowledge_group(frame::AckGroup { base_id: 0u32.wrapping_sub(16), bitfield: 0xFFFF0000, nonce: false });
        fq.acknowledge_group(frame::AckGroup { base_id: 0u32.wrapping_add(16), bitfield: 0x0000FFFF, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn forget_frames() {
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 48000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let p0 = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p1 = vec![ 1; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p2 = vec![ 2; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p3 = vec![ 3; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p4 = vec![ 4; MAX_FRAGMENT_SIZE ].into_boxed_slice();

        ps.enqueue_packet(p0.clone(), 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 0, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p1.clone(), 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 9, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p2, 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 10, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p3, 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 11, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p4, 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 12, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        fq.forget_frames(10);
        fq.acknowledge_group(frame::AckGroup { base_id: 0, bitfield: 0b11111, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, 2*rtt_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 2);

        let dg0 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: p0,
        };

        let dg1 = Datagram {
            sequence_id: 1,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: p1,
        };

        test_data_frame(&frames[0], 5, vec![ dg0 ]);
        test_data_frame(&frames[1], 6, vec![ dg1 ]);
    }

    #[test]
    fn no_resend_after_skip() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 10000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fq = frame_queue::FrameQueue::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let p0 = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p1 = vec![ 1; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p2 = vec![ 2; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p3 = vec![ 3; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p4 = vec![ 4; MAX_FRAGMENT_SIZE ].into_boxed_slice();

        ps.enqueue_packet(p0        , 0, SendMode::Resend, 0);
        ps.enqueue_packet(p1        , 0, SendMode::Resend, 0);
        ps.enqueue_packet(p2        , 0, SendMode::Resend, 0);
        ps.enqueue_packet(p3        , 0, SendMode::Resend, 0);
        ps.enqueue_packet(p4.clone(), 0, SendMode::Resend, 0);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        ps.acknowledge(4);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fq, faq, fid, now_ms + rtt_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg4 = Datagram {
            sequence_id: 4,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: p4,
        };

        test_data_frame(&frames[0], 5, vec![ dg4 ]);
    }
    */
}

