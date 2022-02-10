
use crate::MAX_FRAME_TRANSFER_WINDOW_SIZE;
use crate::MAX_FRAME_SIZE;
use crate::frame;
use crate::frame::serial::DataFrameBuilder;
use crate::frame::serial::AckFrameBuilder;

use super::packet_sender;
use super::datagram_queue;
use super::resend_queue;
use super::frame_log;
use super::frame_ack_queue;
use super::FragmentRef;

const MAX_SEND_COUNT: u8 = 2;

pub struct FrameEmitter<'a> {
    packet_sender: &'a mut packet_sender::PacketSender,
    datagram_queue: &'a mut datagram_queue::DatagramQueue,
    resend_queue: &'a mut resend_queue::ResendQueue,
    frame_log: &'a mut frame_log::FrameLog,
    frame_ack_queue: &'a mut frame_ack_queue::FrameAckQueue,
    flush_id: u32,
}

impl<'a> FrameEmitter<'a> {
    pub fn new(packet_sender: &'a mut packet_sender::PacketSender,
               datagram_queue: &'a mut datagram_queue::DatagramQueue,
               resend_queue: &'a mut resend_queue::ResendQueue,
               frame_log: &'a mut frame_log::FrameLog,
               frame_ack_queue: &'a mut frame_ack_queue::FrameAckQueue,
               flush_id: u32) -> Self {
        Self {
            packet_sender,
            datagram_queue,
            resend_queue,
            frame_log,
            frame_ack_queue,
            flush_id,
        }
    }

    pub fn emit_data_frames<F>(&mut self, now_ms: u64, rtt_ms: u64, max_send_size: usize, mut f: F) -> (usize, bool) where F: FnMut(Box<[u8]>, u32, bool) {
        let mut bytes_remaining = max_send_size;

        if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
            return (max_send_size - bytes_remaining, false);
        }

        let mut frame_id = self.frame_log.next_id();
        let mut nonce = rand::random();

        let mut fbuilder = DataFrameBuilder::new(frame_id, nonce);
        let mut fragment_refs = Vec::new();

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

                let datagram = packet_ref.datagram(entry.fragment_ref.fragment_id);

                let encoded_size = DataFrameBuilder::encoded_size_ref(&datagram);
                let potential_frame_size = fbuilder.size() + encoded_size;

                if potential_frame_size > bytes_remaining {
                    if fbuilder.count() > 0 {
                        let frame_data = fbuilder.build();
                        bytes_remaining -= frame_data.len();

                        self.frame_log.push(frame_id, frame_log::Entry {
                            send_time_ms: now_ms,
                            fragment_refs: fragment_refs.into_boxed_slice(),
                        });

                        f(frame_data, frame_id, nonce);
                    }

                    return (max_send_size - bytes_remaining, true);
                }

                if potential_frame_size > MAX_FRAME_SIZE {
                    debug_assert!(fbuilder.count() > 0);

                    let frame_data = fbuilder.build();
                    bytes_remaining -= frame_data.len();

                    self.frame_log.push(frame_id, frame_log::Entry {
                        send_time_ms: now_ms,
                        fragment_refs: fragment_refs.into_boxed_slice(),
                    });

                    f(frame_data, frame_id, nonce);

                    if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
                        return (max_send_size - bytes_remaining, false);
                    }

                    frame_id = self.frame_log.next_id();
                    nonce = rand::random();

                    fbuilder = DataFrameBuilder::new(frame_id, nonce);
                    fragment_refs = Vec::new();
                    continue;
                }

                fbuilder.add_ref(&datagram);

                let entry = self.resend_queue.pop().unwrap();

                fragment_refs.push(entry.fragment_ref.clone());

                self.resend_queue.push(resend_queue::Entry::new(entry.fragment_ref,
                                                                now_ms + rtt_ms*(1 << entry.send_count),
                                                                (entry.send_count + 1).min(MAX_SEND_COUNT)));
            } else {
                self.resend_queue.pop();
                continue;
            }
        }

        'outer: loop {
            if self.datagram_queue.is_empty() {
                if let Some((pending_packet_rc, resend)) = self.packet_sender.emit_packet(self.flush_id) {
                    let pending_packet_ref = pending_packet_rc.borrow();

                    let last_fragment_id = pending_packet_ref.last_fragment_id();
                    for i in 0 ..= last_fragment_id {
                        let fragment_ref = FragmentRef::new(&pending_packet_rc, i);
                        let entry = datagram_queue::Entry::new(fragment_ref, resend);
                        self.datagram_queue.push_back(entry);
                    }
                } else {
                    break 'outer;
                }
            }

            while let Some(entry) = self.datagram_queue.front() {
                if let Some(packet_rc) = entry.fragment_ref.packet.upgrade() {
                    let packet_ref = packet_rc.borrow();

                    if packet_ref.fragment_acknowledged(entry.fragment_ref.fragment_id) {
                        self.resend_queue.pop();
                        continue;
                    }

                    let datagram = packet_ref.datagram(entry.fragment_ref.fragment_id);

                    let encoded_size = DataFrameBuilder::encoded_size_ref(&datagram);
                    let potential_frame_size = fbuilder.size() + encoded_size;

                    if potential_frame_size > bytes_remaining {
                        if fbuilder.count() > 0 {
                            let frame_data = fbuilder.build();
                            bytes_remaining -= frame_data.len();

                            self.frame_log.push(frame_id, frame_log::Entry {
                                send_time_ms: now_ms,
                                fragment_refs: fragment_refs.into_boxed_slice(),
                            });

                            f(frame_data, frame_id, nonce);
                        }

                        return (max_send_size - bytes_remaining, true);
                    }

                    if potential_frame_size > MAX_FRAME_SIZE {
                        debug_assert!(fbuilder.count() > 0);

                        let frame_data = fbuilder.build();
                        bytes_remaining -= frame_data.len();

                        self.frame_log.push(frame_id, frame_log::Entry {
                            send_time_ms: now_ms,
                            fragment_refs: fragment_refs.into_boxed_slice(),
                        });

                        f(frame_data, frame_id, nonce);

                        if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
                            return (max_send_size - bytes_remaining, false);
                        }

                        frame_id = self.frame_log.next_id();
                        nonce = rand::random();

                        fbuilder = DataFrameBuilder::new(frame_id, nonce);
                        fragment_refs = Vec::new();
                        continue;
                    }

                    fbuilder.add_ref(&datagram);

                    let entry = self.datagram_queue.pop_front().unwrap();

                    if entry.resend {
                        fragment_refs.push(entry.fragment_ref.clone());

                        self.resend_queue.push(resend_queue::Entry::new(entry.fragment_ref, now_ms + rtt_ms, 1));
                    }
                } else {
                    self.resend_queue.pop();
                    continue;
                }
            }
        }

        if fbuilder.count() > 0 {
            let frame_data = fbuilder.build();
            bytes_remaining -= frame_data.len();

            self.frame_log.push(frame_id, frame_log::Entry {
                send_time_ms: now_ms,
                fragment_refs: fragment_refs.into_boxed_slice(),
            });

            f(frame_data, frame_id, nonce);
        }

        return (max_send_size - bytes_remaining, false);
    }

    pub fn emit_sync_frame<F>(&mut self, now_ms: u64, sender_next_id: u32, max_send_size: usize, mut f: F) -> (usize, bool) where F: FnMut(Box<[u8]>, u32, bool) {
        // TODO: It's a shame that this check needs to happen here (this check seems out of place)
        if self.resend_queue.len() != 0 || self.datagram_queue.len() != 0 {
            return (0, false);
        }

        if self.frame_log.len() == MAX_FRAME_TRANSFER_WINDOW_SIZE {
            return (0, false);
        }

        if frame::serial::SYNC_FRAME_SIZE > max_send_size {
            return (0, true);
        }

        let frame_id = self.frame_log.next_id();
        let nonce = rand::random();

        let frame = frame::Frame::SyncFrame(frame::SyncFrame { sequence_id: frame_id, nonce, sender_next_id });

        use frame::serial::Serialize;
        let frame_data = frame.write();

        f(frame_data, frame_id, nonce);

        self.frame_log.push(frame_id, frame_log::Entry {
            send_time_ms: now_ms,
            fragment_refs: Box::new([]),
        });

        return (frame::serial::SYNC_FRAME_SIZE, false);
    }

    pub fn emit_ack_frames<F>(&mut self, receiver_base_id: u32, max_send_size: usize, mut f: F) -> (usize, bool) where F: FnMut(Box<[u8]>) {
        let mut bytes_remaining = max_send_size;

        let mut fbuilder = AckFrameBuilder::new(receiver_base_id);

        while let Some(frame_ack) = self.frame_ack_queue.peek() {
            let encoded_size = AckFrameBuilder::encoded_size(&frame_ack);
            let potential_frame_size = fbuilder.size() + encoded_size;

            if potential_frame_size > bytes_remaining {
                if fbuilder.count() > 0 {
                    let frame_data = fbuilder.build();
                    bytes_remaining -= frame_data.len();
                    f(frame_data);
                }

                return (max_send_size - bytes_remaining, true);
            }

            if potential_frame_size > MAX_FRAME_SIZE {
                debug_assert!(fbuilder.count() > 0);

                let frame_data = fbuilder.build();
                bytes_remaining -= frame_data.len();
                f(frame_data);

                fbuilder = AckFrameBuilder::new(receiver_base_id);
                continue;
            }

            fbuilder.add(&frame_ack);

            self.frame_ack_queue.pop();
        }

        if fbuilder.count() > 0 {
            let frame_data = fbuilder.build();
            bytes_remaining -= frame_data.len();
            f(frame_data);
        }

        return (max_send_size - bytes_remaining, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                             fl: &mut frame_log::FrameLog,
                             faq: &mut frame_ack_queue::FrameAckQueue,
                             fid: u32,
                             now_ms: u64,
                             rtt_ms: u64,
                             max_send_size: usize) -> (VecDeque<DataFrame>, usize, bool) {
        let mut dfe = FrameEmitter::new(ps, dq, rq, fl, faq, fid);
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
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        ps.enqueue_packet(vec![ 0, 0, 0 ].into_boxed_slice(), 0, SendMode::Unreliable, fid);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, 10000);
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
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let packet_data = (0 .. 2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ps.enqueue_packet(packet_data.clone(), 0, SendMode::Unreliable, fid);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, 10000);
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
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        // No data
        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, 0);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, false);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, false);

        let p0 = (0 .. 2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ps.enqueue_packet(p0.clone(), 0, SendMode::Resend, fid);

        // Send path
        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE-1);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, false);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, false);

        // Resend path
        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE-1);
        assert_eq!(frames.len(), 0);
        assert_eq!(size, 0);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, true);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);
        assert_eq!(size, MAX_FRAME_SIZE);
        assert_eq!(size_limit, false);

        let (frames, size, size_limit) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, MAX_FRAME_SIZE);
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
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let p0 = (0 .. 400).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ps.enqueue_packet(p0.clone(), 0, SendMode::Resend, fid);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 0, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 1);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 1, rtt_ms, MAX_FRAME_SIZE);
        assert_eq!(frames.len(), 0);

        let resend_times = [ rtt_ms, 3*rtt_ms, 7*rtt_ms, 11*rtt_ms, 15*rtt_ms, 19*rtt_ms, 23*rtt_ms ];

        for time_ms in resend_times.iter() {
            let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, *time_ms - 1, rtt_ms, MAX_FRAME_SIZE);
            assert_eq!(frames.len(), 0);

            let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, *time_ms    , rtt_ms, MAX_FRAME_SIZE);
            assert_eq!(frames.len(), 1);

            let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, *time_ms + 1, rtt_ms, MAX_FRAME_SIZE);
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
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();

        ps.enqueue_packet(vec![ 0, 0, 0 ].into_boxed_slice(), 0, SendMode::TimeSensitive, 0);
        ps.enqueue_packet(vec![ 1, 1, 1 ].into_boxed_slice(), 0, SendMode::Unreliable, 0);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, 1, now_ms, rtt_ms, 10000);
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
        let ref mut fl = frame_log::FrameLog::new(0);
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

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        fl.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b11101, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, 10000);
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
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        for _ in 0..32 {
            let packet_data = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
            ps.enqueue_packet(packet_data, 0, SendMode::Resend, fid);
        }

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 32);

        fl.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0xFFFFFFFF, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn multi_acknowledgement() {
        let now_ms = 0;
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 48000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        for _ in 0..32 {
            let packet_data = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
            ps.enqueue_packet(packet_data, 0, SendMode::Resend, fid);
        }

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 32);

        fl.acknowledge_frames(frame::FrameAck { base_id: 0u32.wrapping_sub(16), bitfield: 0xFFFF0000, nonce: false });
        fl.acknowledge_frames(frame::FrameAck { base_id: 0u32.wrapping_add(16), bitfield: 0x0000FFFF, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, 48000);
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn forget_frames() {
        let rtt_ms = 100;

        let ref mut ps = packet_sender::PacketSender::new(1, 48000, 0);
        let ref mut dq = datagram_queue::DatagramQueue::new();
        let ref mut rq = resend_queue::ResendQueue::new();
        let ref mut fl = frame_log::FrameLog::new(0);
        let ref mut faq = frame_ack_queue::FrameAckQueue::new();
        let fid = 0;

        let p0 = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p1 = vec![ 1; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p2 = vec![ 2; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p3 = vec![ 3; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p4 = vec![ 4; MAX_FRAGMENT_SIZE ].into_boxed_slice();

        ps.enqueue_packet(p0.clone(), 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 0, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p1.clone(), 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 9, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p2, 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 10, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p3, 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 11, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        ps.enqueue_packet(p4, 0, SendMode::Resend, 0);
        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 12, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        fl.forget_frames(10);
        fl.acknowledge_frames(frame::FrameAck { base_id: 0, bitfield: 0b11111, nonce: false });

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, 2*rtt_ms, rtt_ms, 10000);
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
        let ref mut fl = frame_log::FrameLog::new(0);
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

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        ps.acknowledge(4);

        let (frames, ..) = test_emit_data_frames(ps, dq, rq, fl, faq, fid, now_ms + rtt_ms, rtt_ms, 10000);
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
}

