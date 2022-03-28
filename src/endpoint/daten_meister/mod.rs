
use super::FrameSink;

use crate::SendMode;
use crate::frame;

use std::time;

mod emit;
mod frame_ack_queue;
mod frame_queue;
mod loss_rate;
mod packet_receiver;
mod packet_sender;
mod pending_packet;
mod pending_queue;
mod recv_rate_set;
mod reorder_buffer;
mod resend_queue;
mod send_rate;

#[cfg(test)]
mod packet_tests;

const INITIAL_RTT_ESTIMATE_MS: u64 = 150;
const INITIAL_RTO_ESTIMATE_MS: u64 = 4*INITIAL_RTT_ESTIMATE_MS;
const MIN_SYNC_TIMEOUT_MS: u64 = 2000;
const MIN_SYNC_KEEPALIVE_TIMEOUT_MS: u64 = 5000;

pub trait PacketSink {
    fn send(&mut self, packet_data: Box<[u8]>);
}

pub struct Config {
    pub tx_frame_base_id: u32,
    pub rx_frame_base_id: u32,

    pub tx_frame_window_size: u32,
    pub rx_frame_window_size: u32,

    pub tx_packet_base_id: u32,
    pub rx_packet_base_id: u32,

    pub tx_packet_window_size: u32,
    pub rx_packet_window_size: u32,

    pub tx_bandwidth_limit: u32,

    pub tx_alloc_limit: usize,
    pub rx_alloc_limit: usize,

    pub keepalive: bool,
}

pub struct DatenMeister {
    packet_sender: packet_sender::PacketSender,
    pending_queue: pending_queue::PendingQueue,
    resend_queue: resend_queue::ResendQueue,
    frame_queue: frame_queue::FrameQueue,

    packet_receiver: packet_receiver::PacketReceiver,
    frame_ack_queue: frame_ack_queue::FrameAckQueue,

    send_rate_comp: send_rate::SendRateComp,

    now_ms: u64,
    rtt_ms: u64,
    rto_ms: u64,

    time_base: time::Instant,
    time_last_flushed: Option<time::Instant>,
    sync_timeout_base_ms: u64,

    flush_alloc: isize,
    flush_id: u32,

    sync_reply: bool,
    sync_keepalive: bool,
}

impl DatenMeister {
    pub fn new(config: Config) -> Self {
        Self {
            packet_sender: packet_sender::PacketSender::new(config.tx_packet_window_size, config.tx_packet_base_id, config.tx_alloc_limit),
            pending_queue: pending_queue::PendingQueue::new(),
            resend_queue: resend_queue::ResendQueue::new(),
            frame_queue: frame_queue::FrameQueue::new(config.tx_frame_window_size, config.tx_frame_window_size, config.tx_frame_base_id),

            packet_receiver: packet_receiver::PacketReceiver::new(config.rx_packet_window_size, config.rx_packet_base_id, config.rx_alloc_limit),
            frame_ack_queue: frame_ack_queue::FrameAckQueue::new(config.rx_frame_window_size, config.rx_frame_base_id),

            send_rate_comp: send_rate::SendRateComp::new(config.tx_bandwidth_limit),

            now_ms: 0,
            rtt_ms: 0,
            rto_ms: 0,

            time_base: time::Instant::now(),
            time_last_flushed: None,
            sync_timeout_base_ms: 0,

            flush_alloc: 0,
            flush_id: 0,

            sync_reply: false,
            sync_keepalive: config.keepalive,
        }
    }

    pub fn rtt_s(&self) -> Option<f64> {
        self.send_rate_comp.rtt_s()
    }

    pub fn send_buffer_size(&self) -> usize {
        self.packet_sender.total_size()
    }

    pub fn is_send_pending(&self) -> bool {
        self.packet_sender.pending_count() != 0 || self.pending_queue.len() != 0 || self.resend_queue.len() != 0
    }

    pub fn send(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode) {
        self.packet_sender.enqueue_packet(data, channel_id, mode, self.flush_id);
    }

    pub fn receive(&mut self, sink: &mut impl PacketSink) {
        self.packet_receiver.receive(sink);
    }

    pub fn handle_data_frame(&mut self, frame: frame::DataFrame) {
        if self.frame_ack_queue.window_contains(frame.sequence_id) {
            self.frame_ack_queue.mark_seen(frame.sequence_id, frame.nonce);

            for datagram in frame.datagrams.into_iter() {
                self.packet_receiver.handle_datagram(datagram);
            }
        }
    }

    pub fn handle_sync_frame(&mut self, frame: frame::SyncFrame) {
        if let Some(next_frame_id) = frame.next_frame_id {
            self.frame_ack_queue.resynchronize(next_frame_id);
        }

        if let Some(next_packet_id) = frame.next_packet_id {
            self.packet_receiver.resynchronize(next_packet_id);
        }

        self.sync_reply = true;
    }

    pub fn handle_ack_frame(&mut self, frame: frame::AckFrame) {
        let rtt_ms = self.send_rate_comp.rtt_ms();

        for frame_ack in frame.frame_acks.into_iter() {
            self.frame_queue.acknowledge_group(frame_ack.clone(), rtt_ms);
        }

        self.frame_queue.advance_transfer_window(frame.frame_window_base_id, rtt_ms);
        self.packet_sender.acknowledge(frame.packet_window_base_id);
    }

    pub fn step(&mut self) {
        let now = time::Instant::now();

        let now_ms = (now - self.time_base).as_millis() as u64;
        let rtt_ms = self.send_rate_comp.rtt_ms().unwrap_or(INITIAL_RTT_ESTIMATE_MS);
        let rto_ms = self.send_rate_comp.rto_ms().unwrap_or(INITIAL_RTO_ESTIMATE_MS);

        // Store these values for subsequent flush()
        self.now_ms = now_ms;
        self.rtt_ms = rtt_ms;
        self.rto_ms = rto_ms;

        // Forget old frame data
        self.frame_queue.forget_frames(now_ms.saturating_sub(rtt_ms*4), self.send_rate_comp.rtt_ms());

        // Fill flush allocation
        self.fill_flush_alloc(now);

        // Ignore previous TimeSensitive packets
        self.flush_id = self.flush_id.wrapping_add(1);

        // Update send rate value
        let ref mut frame_queue = self.frame_queue;
        self.send_rate_comp.step(now_ms, frame_queue.get_feedback(now_ms),
            |new_loss_rate: f64| {
                frame_queue.reset_loss_rate(new_loss_rate);
            }
        );
    }

    pub fn flush(&mut self, sink: &mut impl FrameSink) {
        // Send as many frames as possible
        self.emit_frames(self.now_ms, self.rtt_ms, self.rto_ms, self.flush_id, sink);
    }

    fn fill_flush_alloc(&mut self, now: time::Instant) {
        if let Some(time_last_flushed) = self.time_last_flushed {
            let send_rate = self.send_rate_comp.send_rate();
            let rtt_s = self.send_rate_comp.rtt_s();

            let delta_time = (now - time_last_flushed).as_secs_f64();
            let new_bytes = (send_rate * delta_time).round() as isize;
            let alloc_max = (send_rate * rtt_s.unwrap_or(0.0)).round() as isize;

            self.flush_alloc = self.flush_alloc.saturating_add(new_bytes).min(alloc_max);
        }
        self.time_last_flushed = Some(now);
    }

    fn emit_frames(&mut self, now_ms: u64, rtt_ms: u64, rto_ms: u64, flush_id: u32, sink: &mut impl FrameSink) {
        match self.emit_ack_frames(sink) {
            Err(_) => return,
            Ok(_) => (),
        }

        match self.emit_data_frames(now_ms, rtt_ms, flush_id, sink) {
            Err(_) => return,
            Ok(_) => (),
        }

        match self.emit_sync_frame(now_ms, rto_ms, sink) {
            Err(_) => return,
            Ok(_) => (),
        }
    }

    fn emit_sync_frame(&mut self, now_ms: u64, rto_ms: u64, sink: &mut impl FrameSink) -> Result<(),()> {
        let elapsed_ms = now_ms - self.sync_timeout_base_ms;
        let sync_timeout_ms = rto_ms.max(MIN_SYNC_TIMEOUT_MS);

        if elapsed_ms >= sync_timeout_ms {
            let next_frame_id =
                if self.frame_queue.next_id() != self.frame_queue.base_id() {
                    Some(self.frame_queue.next_id())
                } else {
                    None
                };

            let next_packet_id =
                if self.packet_sender.next_id() != self.packet_sender.base_id() &&
                   self.resend_queue.len() == 0 && self.pending_queue.len() == 0 {
                    Some(self.packet_sender.next_id())
                } else {
                    None
                };

            if next_frame_id.is_none() && next_packet_id.is_none() {
                if self.sync_keepalive {
                    if elapsed_ms < MIN_SYNC_KEEPALIVE_TIMEOUT_MS {
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }

            if self.flush_alloc < 0 {
                return Err(());
            }

            let frame = frame::Frame::SyncFrame(frame::SyncFrame { next_frame_id, next_packet_id });

            use frame::serial::Serialize;
            let frame_bytes = frame.write();

            sink.send(&frame_bytes);
            self.flush_alloc -= frame_bytes.len() as isize;
            self.sync_timeout_base_ms = now_ms;
        }

        return Ok(());
    }

    fn emit_ack_frames(&mut self, sink: &mut impl FrameSink) -> Result<(),()> {
        let flush_alloc_init = self.flush_alloc;
        let sync_reply_init = self.sync_reply;

        let frame_window_base_id = self.frame_ack_queue.base_id();
        let packet_window_base_id = self.packet_receiver.base_id();

        let ref mut flush_alloc = self.flush_alloc;
        let ref mut sync_reply = self.sync_reply;

        let emit_cb = |frame_bytes: Box<[u8]>| {
            sink.send(&frame_bytes);
            *flush_alloc -= frame_bytes.len() as isize;
            *sync_reply = false;
        };

        let mut afe = emit::AckFrameEmitter::new(frame_window_base_id, packet_window_base_id, flush_alloc_init, emit_cb);

        if sync_reply_init {
            match afe.push_dud() {
                Err(_) => return Err(()),
                Ok(_) => (),
            }
        }

        while let Some(ack_group) = self.frame_ack_queue.peek() {
            match afe.push(ack_group) {
                Err(_) => return Err(()),
                Ok(_) => (),
            }

            self.frame_ack_queue.pop();
        }

        afe.finalize();

        return Ok(());
    }

    fn emit_data_frames(&mut self, now_ms: u64, rtt_ms: u64, flush_id: u32, sink: &mut impl FrameSink) -> Result<(),()> {
        let flush_alloc_init = self.flush_alloc;

        let ref mut send_rate_comp = self.send_rate_comp;
        let ref mut flush_alloc = self.flush_alloc;
        let ref mut sync_timeout_base_ms = self.sync_timeout_base_ms;

        let emit_cb = |frame_bytes: Box<[u8]>| {
            sink.send(&frame_bytes);
            send_rate_comp.notify_frame_sent(now_ms);
            *flush_alloc -= frame_bytes.len() as isize;
            *sync_timeout_base_ms = now_ms;
        };

        let mut dfe = emit::DataFrameEmitter::new(now_ms, &mut self.frame_queue, flush_alloc_init, emit_cb);

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
                    // Being window-limited does not preclude further sends
                    Err(emit::DataPushError::WindowLimited) => return Ok(()),
                    Err(emit::DataPushError::SizeLimited) => return Err(()),
                    Ok(_) => (),
                }

                let entry = self.resend_queue.pop().unwrap();

                const MAX_SEND_COUNT: u8 = 2;

                let new_resend_time = now_ms + rtt_ms*(1 << entry.send_count);
                let new_send_count = (entry.send_count + 1).min(MAX_SEND_COUNT);

                self.resend_queue.push(resend_queue::Entry::new(entry.fragment_ref, new_resend_time, new_send_count));
            } else {
                self.resend_queue.pop();
                continue;
            }
        }

        loop {
            if self.pending_queue.is_empty() {
                if let Some((packet_rc, resend)) = self.packet_sender.emit_packet(flush_id) {
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
                        // Being window-limited does not preclude further sends
                        Err(emit::DataPushError::WindowLimited) => return Ok(()),
                        Err(emit::DataPushError::SizeLimited) => return Err(()),
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

        dfe.finalize();

        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::SendMode;
    use crate::frame::Datagram;

    use crate::MAX_FRAGMENT_SIZE;
    use crate::MAX_FRAME_SIZE;
    use crate::MAX_FRAME_WINDOW_SIZE;
    use crate::MAX_PACKET_WINDOW_SIZE;

    use std::collections::VecDeque;

    struct TestSink {
        emitted: VecDeque<Box<[u8]>>,
    }

    impl TestSink {
        fn new() -> Self {
            Self {
                emitted: VecDeque::new(),
            }
        }
    }

    impl FrameSink for TestSink {
        fn send(&mut self, frame_bytes: &[u8]) {
            self.emitted.push_back(frame_bytes.into());
        }
    }

    struct TestApparatus {
        dm: DatenMeister,
        flush_id: u32,
    }

    impl TestApparatus {
        fn new() -> Self {
            let config = Config {
                tx_frame_window_size: MAX_FRAME_WINDOW_SIZE,
                rx_frame_window_size: MAX_FRAME_WINDOW_SIZE,

                tx_frame_base_id: 0,
                rx_frame_base_id: 0,

                tx_packet_window_size: MAX_PACKET_WINDOW_SIZE,
                rx_packet_window_size: MAX_PACKET_WINDOW_SIZE,

                tx_packet_base_id: 0,
                rx_packet_base_id: 0,

                tx_bandwidth_limit: 100_000,

                tx_alloc_limit: MAX_FRAGMENT_SIZE * MAX_PACKET_WINDOW_SIZE as usize,
                rx_alloc_limit: MAX_FRAGMENT_SIZE * MAX_PACKET_WINDOW_SIZE as usize,

                keepalive: true,
            };

            Self {
                dm: DatenMeister::new(config),
                flush_id: 0,
            }
        }

        fn set_flush_id(&mut self, flush_id: u32) {
            self.flush_id = flush_id;
        }

        fn receive_sync(&mut self, frame: frame::SyncFrame) {
            self.dm.handle_sync_frame(frame);
        }

        fn receive_ack(&mut self, frame: frame::AckFrame) {
            self.dm.handle_ack_frame(frame);
        }

        fn enqueue_packet(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode) {
            self.dm.send(data, channel_id, mode)
        }

        fn acknowledge_packet_base_id(&mut self, base_id: u32) {
            self.dm.packet_sender.acknowledge(base_id)
        }

        fn acknowledge_frame_group(&mut self, group: frame::AckGroup, rtt_ms: Option<u64>) {
            self.dm.frame_queue.acknowledge_group(group, rtt_ms);
        }

        fn emit_frames(&mut self, now_ms: u64, rtt_ms: u64, flush_alloc: isize) -> VecDeque<Box<[u8]>> {
            let mut test_sink = TestSink::new();

            self.dm.flush_alloc = flush_alloc;

            self.dm.emit_frames(now_ms, rtt_ms, 4*rtt_ms, self.flush_id, &mut test_sink);

            return test_sink.emitted;
        }
    }

    fn test_data_frame(frame_bytes: &Box<[u8]>, sequence_id: u32, datagrams: Vec<Datagram>) {
        use crate::frame::serial::Serialize;

        match frame::Frame::read(frame_bytes).unwrap() {
            frame::Frame::DataFrame(data_frame) => {
                assert_eq!(data_frame.sequence_id, sequence_id);
                assert_eq!(data_frame.datagrams, datagrams);
            }
            _ => panic!("Expected DataFrame")
        }
    }

    fn test_sync_frame(frame_bytes: &Box<[u8]>, expected_frame: frame::SyncFrame) {
        use crate::frame::serial::Serialize;

        match frame::Frame::read(frame_bytes).unwrap() {
            frame::Frame::SyncFrame(sync_frame) => {
                assert_eq!(sync_frame, expected_frame);
            }
            _ => panic!("Expected SyncFrame")
        }
    }

    fn test_ack_frame(frame_bytes: &Box<[u8]>, expected_frame: frame::AckFrame) {
        use crate::frame::serial::Serialize;

        match frame::Frame::read(frame_bytes).unwrap() {
            frame::Frame::AckFrame(ack_frame) => {
                assert_eq!(ack_frame, expected_frame);
            }
            _ => panic!("Expected AckFrame")
        }
    }

    fn data_frame_nonce(frame_bytes: &Box<[u8]>) -> bool {
        use crate::frame::serial::Serialize;

        match frame::Frame::read(frame_bytes).unwrap() {
            frame::Frame::DataFrame(data_frame) => {
                return data_frame.nonce;
            }
            _ => panic!("Expected DataFrame")
        }
    }

    #[test]
    fn basic_send() {
        let now_ms = 0;
        let rtt_ms = 100;

        let mut ta = TestApparatus::new();

        ta.enqueue_packet(vec![ 0, 0, 0 ].into_boxed_slice(), 0, SendMode::Unreliable);

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg0 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: 0,
            fragment_id_last: 0,
            data: vec![ 0, 0, 0 ].into_boxed_slice(),
        };

        test_data_frame(&frames[0], 0, vec![ dg0 ]);
    }

    #[test]
    fn max_frame_size() {
        let now_ms = 0;
        let rtt_ms = 100;

        let mut ta = TestApparatus::new();

        let packet_data = (0 .. 2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ta.enqueue_packet(packet_data.clone(), 0, SendMode::Unreliable);

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 2);

        let dg0 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: 0,
            fragment_id_last: 1,
            data: packet_data[ .. MAX_FRAGMENT_SIZE].into(),
        };

        let dg1 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: 1,
            fragment_id_last: 1,
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        };

        test_data_frame(&frames[0], 0, vec![ dg0 ]);
        test_data_frame(&frames[1], 1, vec![ dg1 ]);

        assert_eq!(frames[0].len(), MAX_FRAME_SIZE);
        assert_eq!(frames[1].len(), MAX_FRAME_SIZE);
    }

    // Time sensitive packet IDs should not be resent if the flush ID does not match.
    #[test]
    fn time_sensitive_drop() {
        let now_ms = 0;
        let rtt_ms = 100;

        let mut ta = TestApparatus::new();

        ta.enqueue_packet(vec![ 0, 0, 0 ].into_boxed_slice(), 0, SendMode::TimeSensitive);
        ta.enqueue_packet(vec![ 1, 1, 1 ].into_boxed_slice(), 0, SendMode::Unreliable);

        ta.set_flush_id(1);

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg0 = Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: 0,
            fragment_id_last: 0,
            data: vec![ 1, 1, 1 ].into_boxed_slice(),
        };

        test_data_frame(&frames[0], 0, vec![ dg0 ]);
    }

    // Once the packet transfer window advances, persistent packets in the resend queue should not
    // be resent.
    #[test]
    fn no_resend_after_packet_skip() {
        let now_ms = 0;
        let rtt_ms = 100;

        let mut ta = TestApparatus::new();

        let p0 = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p1 = vec![ 1; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p2 = vec![ 2; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p3 = vec![ 3; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p4 = vec![ 4; MAX_FRAGMENT_SIZE ].into_boxed_slice();

        ta.enqueue_packet(p0        , 0, SendMode::Persistent);
        ta.enqueue_packet(p1        , 0, SendMode::Persistent);
        ta.enqueue_packet(p2        , 0, SendMode::Persistent);
        ta.enqueue_packet(p3        , 0, SendMode::Persistent);
        ta.enqueue_packet(p4.clone(), 0, SendMode::Persistent);

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        ta.acknowledge_packet_base_id(4);

        let frames = ta.emit_frames(now_ms + rtt_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg4 = Datagram {
            sequence_id: 4,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: 0,
            fragment_id_last: 0,
            data: p4,
        };

        test_data_frame(&frames[0], 5, vec![ dg4 ]);
    }

    // Frames which have been acknowledged should not be resent.
    #[test]
    fn no_resend_after_ack() {
        let now_ms = 0;
        let rtt_ms = 100;

        let mut ta = TestApparatus::new();

        let p0 = vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p1 = vec![ 1; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p2 = vec![ 2; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p3 = vec![ 3; MAX_FRAGMENT_SIZE ].into_boxed_slice();
        let p4 = vec![ 4; MAX_FRAGMENT_SIZE ].into_boxed_slice();

        ta.enqueue_packet(p0        , 0, SendMode::Persistent);
        ta.enqueue_packet(p1.clone(), 0, SendMode::Persistent);
        ta.enqueue_packet(p2        , 0, SendMode::Persistent);
        ta.enqueue_packet(p3        , 0, SendMode::Persistent);
        ta.enqueue_packet(p4        , 0, SendMode::Persistent);

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        let n0 = data_frame_nonce(&frames[0]);
        let n2 = data_frame_nonce(&frames[2]);
        let n3 = data_frame_nonce(&frames[3]);
        let n4 = data_frame_nonce(&frames[4]);

        ta.acknowledge_frame_group(frame::AckGroup { base_id: 0, bitfield: 0b11101, nonce: n0 ^ n2 ^ n3 ^ n4 }, Some(rtt_ms));

        let frames = ta.emit_frames(now_ms + rtt_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        let dg1 = Datagram {
            sequence_id: 1,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: 0,
            fragment_id_last: 0,
            data: p1,
        };

        test_data_frame(&frames[0], 5, vec![ dg1 ]);
    }

    // Simple sync case for which both the frame and packet windows are resynchronized.
    #[test]
    fn sync_frame_and_packet_window() {
        let rtt_ms = INITIAL_RTT_ESTIMATE_MS;

        let mut ta = TestApparatus::new();
        let mut now_ms = 0;

        for _ in 0 .. 5 {
            ta.enqueue_packet(vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice(), 0, SendMode::Unreliable);
        }

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        now_ms += MIN_SYNC_TIMEOUT_MS;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        test_sync_frame(&frames[0], frame::SyncFrame { next_frame_id: Some(5), next_packet_id: Some(5) });
    }

    // Sync case for which packets exist in the resend/pending queues, and the frame transfer
    // window is full, so only the frame window is resynchronized.
    #[test]
    fn sync_frame_window_only() {
        let rtt_ms = INITIAL_RTT_ESTIMATE_MS;

        let mut ta = TestApparatus::new();
        let mut now_ms = 0;

        for _ in 0 .. MAX_FRAME_WINDOW_SIZE {
            ta.enqueue_packet(vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice(), 0, SendMode::Persistent);
        }

        let frames = ta.emit_frames(now_ms, rtt_ms, MAX_FRAME_SIZE as isize * MAX_FRAME_WINDOW_SIZE as isize);
        assert_eq!(frames.len(), MAX_FRAME_WINDOW_SIZE as usize);

        now_ms += MIN_SYNC_TIMEOUT_MS;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        test_sync_frame(&frames[0], frame::SyncFrame { next_frame_id: Some(MAX_FRAME_WINDOW_SIZE), next_packet_id: None });
    }

    // Sync case for which no the receiver has not yet called receive(), and only the packet window
    // is resynchronized.
    #[test]
    fn sync_packet_window_only() {
        let rtt_ms = INITIAL_RTT_ESTIMATE_MS;

        let mut ta = TestApparatus::new();
        let mut now_ms = 0;

        for _ in 0 .. 5 {
            ta.enqueue_packet(vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice(), 0, SendMode::Unreliable);
        }

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 5);

        ta.receive_ack(frame::AckFrame { frame_acks: Vec::new(), frame_window_base_id: 5, packet_window_base_id: 0 });

        now_ms += MIN_SYNC_TIMEOUT_MS;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        test_sync_frame(&frames[0], frame::SyncFrame { next_frame_id: None, next_packet_id: Some(5) });
    }

    // An ack frame should always be sent in response to a sync frame
    #[test]
    fn sync_response() {
        let now_ms = 0;
        let rtt_ms = INITIAL_RTT_ESTIMATE_MS;

        let mut ta = TestApparatus::new();

        ta.receive_sync(frame::SyncFrame { next_frame_id: Some(5), next_packet_id: Some(5) });

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        test_ack_frame(&frames[0], frame::AckFrame { frame_acks: Vec::new(), frame_window_base_id: 5, packet_window_base_id: 5 });
    }

    // Packets should be resent [1, 2, 4, 4, ... 4] RTTs after the previous send.
    #[test]
    fn resend_timing() {
        let rtt_ms = INITIAL_RTT_ESTIMATE_MS;

        let mut ta = TestApparatus::new();

        let p0 = (0 .. 400).map(|i| i as u8).collect::<Vec<u8>>().into_boxed_slice();
        ta.enqueue_packet(p0.clone(), 0, SendMode::Persistent);

        let frames = ta.emit_frames(0, rtt_ms, MAX_FRAME_SIZE as isize);
        assert_eq!(frames.len(), 1);

        let frames = ta.emit_frames(1, rtt_ms, MAX_FRAME_SIZE as isize);
        assert_eq!(frames.len(), 0);

        let resend_times = [ rtt_ms, 3*rtt_ms, 7*rtt_ms, 11*rtt_ms, 15*rtt_ms, 19*rtt_ms, 23*rtt_ms ];

        for &now_ms in resend_times.iter() {
            let frames = ta.emit_frames(now_ms - 1, rtt_ms, MAX_FRAME_SIZE as isize);
            assert_eq!(frames.len(), 0);

            let frames = ta.emit_frames(now_ms    , rtt_ms, MAX_FRAME_SIZE as isize);
            assert_eq!(frames.len(), 1);

            let frames = ta.emit_frames(now_ms + 1, rtt_ms, MAX_FRAME_SIZE as isize);
            assert_eq!(frames.len(), 0);
        }
    }

    // Keepalive syncs should be sent periodically after no data has been sent
    #[test]
    fn keepalive_timing() {
        let rtt_ms = INITIAL_RTT_ESTIMATE_MS;

        let mut ta = TestApparatus::new();
        let mut now_ms = 0;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 0);

        now_ms += MIN_SYNC_KEEPALIVE_TIMEOUT_MS - 1;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 0);

        now_ms += 1;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);
        test_sync_frame(&frames[0], frame::SyncFrame { next_frame_id: None, next_packet_id: None });

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 0);

        now_ms += MIN_SYNC_KEEPALIVE_TIMEOUT_MS;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);
        test_sync_frame(&frames[0], frame::SyncFrame { next_frame_id: None, next_packet_id: None });

        // Disrupt the normal timing
        now_ms += MIN_SYNC_KEEPALIVE_TIMEOUT_MS/2;

        ta.enqueue_packet(vec![ 0; MAX_FRAGMENT_SIZE ].into_boxed_slice(), 0, SendMode::Unreliable);
        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);

        now_ms += MIN_SYNC_TIMEOUT_MS;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);
        test_sync_frame(&frames[0], frame::SyncFrame { next_frame_id: Some(1), next_packet_id: Some(1) });

        ta.receive_ack(frame::AckFrame { frame_acks: Vec::new(), frame_window_base_id: 1, packet_window_base_id: 1 });

        now_ms += MIN_SYNC_KEEPALIVE_TIMEOUT_MS;

        let frames = ta.emit_frames(now_ms, rtt_ms, 10000);
        assert_eq!(frames.len(), 1);
        test_sync_frame(&frames[0], frame::SyncFrame { next_frame_id: None, next_packet_id: None });
    }

    // No two packets in a frame receive window may have the same ID
    #[test]
    fn packet_unambiguity() {
        let now_ms = 0;
        let rtt_ms = INITIAL_RTT_ESTIMATE_MS;

        let mut ta = TestApparatus::new();

        use frame::serial::{DATA_FRAME_OVERHEAD, DATA_FRAME_MAX_DATAGRAM_COUNT, MIN_DATAGRAM_OVERHEAD};
        use crate::packet_id;

        // Here we assume that sending hundreds of zero-length packets reaches the datagram limit,
        // not the frame size limit.
        assert!(DATA_FRAME_OVERHEAD + DATA_FRAME_MAX_DATAGRAM_COUNT * MIN_DATAGRAM_OVERHEAD <= MAX_FRAME_SIZE);

        let mut max_id: usize = 0;

        let receive_window_size = 2*MAX_FRAME_WINDOW_SIZE as usize;

        for _ in 0 .. receive_window_size {
            for _ in 0 .. DATA_FRAME_MAX_DATAGRAM_COUNT as usize {
                ta.enqueue_packet(Vec::new().into_boxed_slice(), 0, SendMode::Unreliable);
            }

            let frames = ta.emit_frames(now_ms, rtt_ms, MAX_FRAME_SIZE as isize);
            assert_eq!(frames.len(), 1);

            use frame::serial::Serialize;
            let data_frame = match frame::Frame::read(&frames[0]) {
                Some(frame::Frame::DataFrame(data_frame)) => data_frame,
                _ => panic!(),
            };

            for datagram in data_frame.datagrams.iter() {
                // If the sequence IDs of packets wrap back around to zero, this will fail. If this
                // does not fail, we do not wrap, and therefore have unique packets throughout the
                // frame receive window.
                assert_eq!(datagram.sequence_id as usize, max_id);
                max_id += 1;
            }

            // Keep the windows happy
            ta.receive_ack(frame::AckFrame {
                frame_acks: Vec::new(),
                frame_window_base_id: data_frame.sequence_id,
                packet_window_base_id: max_id as u32,
            });
        }

        println!("max_id: {}", max_id);
        println!("packet_id::SPAN: {}", packet_id::SPAN);
    }
}

