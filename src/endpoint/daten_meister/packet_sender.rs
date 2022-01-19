
use crate::MAX_CHANNELS;
use crate::MAX_FRAGMENT_SIZE;
use crate::MAX_PACKET_SIZE;
use crate::MAX_PACKET_TRANSFER_WINDOW_SIZE;
use crate::SendMode;
use crate::frame::Datagram;
use crate::frame::FragmentId;

use std::collections::VecDeque;

// Size of the buffer that the receiver will allocate for a packet in bytes
fn alloc_size(packet_size: usize) -> usize {
    if packet_size > MAX_FRAGMENT_SIZE {
        ((packet_size + MAX_FRAGMENT_SIZE - 1) / MAX_FRAGMENT_SIZE) * MAX_FRAGMENT_SIZE
    } else {
        packet_size
    }
}

struct WindowEntry {
    // How many allocation points this packet is worth
    alloc_size: usize,
    // Channel this packet was sent on
    channel_id: u8,
}

struct Channel {
    parent_id: Option<u32>,
}

impl Channel {
    fn new() -> Self {
        Self {
            parent_id: None,
        }
    }
}

#[derive(Debug)]
struct PacketSendEntry {
    data: Box<[u8]>,
    channel_id: u8,
    mode: SendMode,
    flush_id: u32,
}

impl PacketSendEntry {
    fn new(data: Box<[u8]>, channel_id: u8, mode: SendMode, flush_id: u32) -> Self {
        Self {
            data,
            channel_id,
            mode,
            flush_id,
        }
    }
}

pub trait DatagramSink {
    fn send(&mut self, datagram: Datagram, resend: bool);
}

fn emit_fragments(packet: PacketSendEntry, sequence_id: u32, window_parent_lead: u16, channel_parent_lead: u16, sink: &mut impl DatagramSink) {
    let num_fragments = (packet.data.len() + MAX_FRAGMENT_SIZE - 1) / MAX_FRAGMENT_SIZE + (packet.data.len() == 0) as usize;

    let resend = match packet.mode {
        SendMode::TimeSensitive => false,
        SendMode::Unreliable => false,
        SendMode::Resend => true,
        SendMode::Reliable => true,
    };

    debug_assert!(num_fragments != 0);

    if num_fragments == 1 {
        let datagram = Datagram {
            sequence_id,
            channel_id: packet.channel_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet.data,
        };

        sink.send(datagram, resend);
    } else {
        debug_assert!(num_fragments - 1 <= u16::MAX as usize);
        let last_fragment_id = num_fragments - 1;

        for i in 0 .. last_fragment_id {
            let datagram = Datagram {
                sequence_id,
                channel_id: packet.channel_id,
                window_parent_lead,
                channel_parent_lead,
                fragment_id: FragmentId { id: i as u16, last: last_fragment_id as u16 },
                data: packet.data[i * MAX_FRAGMENT_SIZE .. (i + 1)*MAX_FRAGMENT_SIZE].into(),
            };

            sink.send(datagram, resend);
        }

        let datagram = Datagram {
            sequence_id,
            channel_id: packet.channel_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id: FragmentId { id: last_fragment_id as u16, last: last_fragment_id as u16 },
            data: packet.data[last_fragment_id * MAX_FRAGMENT_SIZE .. ].into(),
        };

        sink.send(datagram, resend);
    }
}

pub struct PacketSender {
    packet_send_queue: VecDeque<PacketSendEntry>,

    base_id: u32,
    next_id: u32,
    window: Box<[Option<WindowEntry>]>,

    max_alloc: usize,
    alloc: usize,

    parent_id: Option<u32>,
    channels: Box<[Channel]>,
}

impl PacketSender {
    pub fn new(channel_num: usize, max_alloc: usize, base_id: u32) -> Self {
        debug_assert!(channel_num <= MAX_CHANNELS);

        let window: Vec<Option<WindowEntry>> = (0..MAX_PACKET_TRANSFER_WINDOW_SIZE).map(|_| None).collect();
        let channels: Vec<Channel> = (0..channel_num).map(|_| Channel::new()).collect();

        Self {
            packet_send_queue: VecDeque::new(),

            base_id: base_id,
            next_id: base_id,
            window: window.into_boxed_slice(),

            max_alloc: max_alloc,
            alloc: 0,

            parent_id: None,
            channels: channels.into_boxed_slice(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.packet_send_queue.len()
    }

    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    // Places a user packet on the send queue. Fails silently if the packet is impossible to send.
    pub fn enqueue_packet(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode, flush_id: u32) {
        if data.len() > MAX_PACKET_SIZE {
            return;
        }
        if alloc_size(data.len()) > self.max_alloc {
            return;
        }
        if channel_id as usize >= self.channels.len() {
            return;
        }

        self.packet_send_queue.push_back(PacketSendEntry::new(data, channel_id, mode, flush_id));
    }

    // Sends as many packets from the send queue as possible, respecting both the maximum
    // allocation limit, and the maximum transfer window.
    pub fn emit_packet_datagrams(&mut self, flush_id: u32, sink: &mut impl DatagramSink) {
        while let Some(packet) = self.packet_send_queue.front() {
            match packet.mode {
                SendMode::TimeSensitive => {
                    if packet.flush_id != flush_id {
                        self.packet_send_queue.pop_front();
                    } else {
                        break;
                    }
                }
                _ => break
            }
        }

        if let Some(packet) = self.packet_send_queue.front() {
            if self.next_id.wrapping_sub(self.base_id) >= MAX_PACKET_TRANSFER_WINDOW_SIZE {
                return;
            }

            let packet_alloc_size = alloc_size(packet.data.len());

            if self.alloc + packet_alloc_size > self.max_alloc {
                return;
            }

            let packet = self.packet_send_queue.pop_front().unwrap();

            let sequence_id = self.next_id;
            let ref mut channel = self.channels[packet.channel_id as usize];

            let window_parent_lead =
                if let Some(parent_id) = self.parent_id {
                    let lead = sequence_id.wrapping_sub(parent_id);
                    debug_assert!(lead <= u16::MAX as u32);
                    lead as u16
                } else {
                    0
                };

            let channel_parent_lead =
                if let Some(parent_id) = channel.parent_id {
                    let lead = sequence_id.wrapping_sub(parent_id);
                    debug_assert!(lead <= u16::MAX as u32);
                    lead as u16
                } else {
                    0
                };

            match packet.mode {
                SendMode::Reliable => {
                    self.parent_id = Some(sequence_id);
                    channel.parent_id = Some(sequence_id);
                }
                _ => ()
            }

            self.next_id = self.next_id.wrapping_add(1);
            self.alloc += packet_alloc_size;

            let window_idx = (sequence_id % MAX_PACKET_TRANSFER_WINDOW_SIZE) as usize;
            debug_assert!(self.window[window_idx].is_none());
            self.window[window_idx] = Some(WindowEntry {
                alloc_size: packet_alloc_size,
                channel_id: packet.channel_id
            });

            emit_fragments(packet, sequence_id, window_parent_lead, channel_parent_lead, sink);
        }
    }

    // Responds to a receive window acknowledgement. All packet data beyond the new receive window
    // is forgotten, thereby freeing transfer window & allocation space for new packets.
    pub fn acknowledge(&mut self, receiver_base_id: u32) {
        let window_size = self.next_id.wrapping_sub(self.base_id);
        let ack_delta = receiver_base_id.wrapping_sub(self.base_id);

        if ack_delta > window_size {
            return;
        }

        while self.base_id != receiver_base_id {
            let window_idx = (self.base_id % MAX_PACKET_TRANSFER_WINDOW_SIZE) as usize;

            let ref mut window_entry = self.window[window_idx];

            if let Some(entry) = window_entry {
                let ref mut channel = self.channels[entry.channel_id as usize];

                if let Some(parent_id) = self.parent_id {
                    if parent_id == self.base_id {
                        self.parent_id = None;
                    }
                }

                if let Some(parent_id) = channel.parent_id {
                    if parent_id == self.base_id {
                        channel.parent_id = None;
                    }
                }

                self.alloc -= entry.alloc_size;

                *window_entry = None;
            } else {
                panic!();
            }

            self.base_id = self.base_id.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_packet_data(sequence_id: u32) -> Box<[u8]> {
        sequence_id.to_be_bytes().into()
    }

    fn new_packet_datagram(sequence_id: u32, channel_id: u8, window_parent_lead: u16, channel_parent_lead: u16) -> Datagram {
        Datagram {
            sequence_id,
            channel_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: new_packet_data(sequence_id),
        }
    }

    struct TestDatagramSink {
        datagrams: VecDeque<(Datagram, bool)>,
    }

    impl TestDatagramSink {
        fn new() -> Self {
            Self {
                datagrams: VecDeque::new(),
            }
        }

        fn pop(&mut self) -> (Datagram, bool) {
            self.datagrams.pop_front().unwrap()
        }

        fn is_empty(&mut self) -> bool {
            self.datagrams.is_empty()
        }
    }

    impl super::DatagramSink for TestDatagramSink {
        fn send(&mut self, datagram: Datagram, reliable: bool) {
            self.datagrams.push_back((datagram, reliable));
        }
    }

    #[test]
    fn alloc_size_correct() {
        assert_eq!(alloc_size(0), 0);
        assert_eq!(alloc_size(1), 1);
        assert_eq!(alloc_size(  MAX_FRAGMENT_SIZE-1),   MAX_FRAGMENT_SIZE-1);
        assert_eq!(alloc_size(  MAX_FRAGMENT_SIZE  ),   MAX_FRAGMENT_SIZE);
        assert_eq!(alloc_size(  MAX_FRAGMENT_SIZE+1), 2*MAX_FRAGMENT_SIZE);
        assert_eq!(alloc_size(2*MAX_FRAGMENT_SIZE-1), 2*MAX_FRAGMENT_SIZE);
        assert_eq!(alloc_size(2*MAX_FRAGMENT_SIZE  ), 2*MAX_FRAGMENT_SIZE);
        assert_eq!(alloc_size(2*MAX_FRAGMENT_SIZE+1), 3*MAX_FRAGMENT_SIZE);
    }

    #[test]
    fn basic() {
        let mut tx = PacketSender::new(1, 10000, 0);
        let mut sink = TestDatagramSink::new();

        tx.enqueue_packet(new_packet_data(0), 0, SendMode::TimeSensitive, 0);
        tx.enqueue_packet(new_packet_data(1), 0, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(2), 0, SendMode::Resend, 0);
        tx.enqueue_packet(new_packet_data(3), 0, SendMode::Reliable, 0);

        for _ in 0..4 {
            tx.emit_packet_datagrams(0, &mut sink);
        }

        assert_eq!(sink.pop(), (new_packet_datagram(0, 0, 0, 0), false));
        assert_eq!(sink.pop(), (new_packet_datagram(1, 0, 0, 0), false));
        assert_eq!(sink.pop(), (new_packet_datagram(2, 0, 0, 0), true));
        assert_eq!(sink.pop(), (new_packet_datagram(3, 0, 0, 0), true));
        assert!(sink.is_empty());
    }

    #[test]
    fn parent_leads() {
        /*
           7  #     #
           6  #  #
           5  O  O
           4  O  O
           3  #  #
           2  O     O
           1  #     #
           0  O     O
              w  c0 c1
        */

        let mut tx = PacketSender::new(2, 10000, 0);
        let mut sink = TestDatagramSink::new();

        tx.enqueue_packet(new_packet_data(0), 1, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(1), 1, SendMode::Reliable, 0);
        tx.enqueue_packet(new_packet_data(2), 1, SendMode::Unreliable, 0);

        tx.enqueue_packet(new_packet_data(3), 0, SendMode::Reliable, 0);
        tx.enqueue_packet(new_packet_data(4), 0, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(5), 0, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(6), 0, SendMode::Reliable, 0);

        tx.enqueue_packet(new_packet_data(7), 1, SendMode::Reliable, 0);

        for _ in 0..8 {
            tx.emit_packet_datagrams(0, &mut sink);
        }

        assert_eq!(sink.pop(), (new_packet_datagram(0, 1, 0, 0), false));
        assert_eq!(sink.pop(), (new_packet_datagram(1, 1, 0, 0), true));
        assert_eq!(sink.pop(), (new_packet_datagram(2, 1, 1, 1), false));

        assert_eq!(sink.pop(), (new_packet_datagram(3, 0, 2, 0), true));
        assert_eq!(sink.pop(), (new_packet_datagram(4, 0, 1, 1), false));
        assert_eq!(sink.pop(), (new_packet_datagram(5, 0, 2, 2), false));
        assert_eq!(sink.pop(), (new_packet_datagram(6, 0, 3, 3), true));

        assert_eq!(sink.pop(), (new_packet_datagram(7, 1, 1, 6), true));

        assert!(sink.is_empty());
    }

    #[test]
    fn parent_leads_acknowledgement() {
        /*
           6  #     #
           5  #  #
           4  O  O
           3  #  #
           2  O     O
           1  #     #
           0  O     O
              w  c0 c1
        */

        let mut tx = PacketSender::new(2, 10000, 0);
        let mut sink = TestDatagramSink::new();

        let mut flush_id = 0;

        for i in 0 .. MAX_PACKET_TRANSFER_WINDOW_SIZE {
            let ref_id = i*7;

            tx.acknowledge(ref_id);

            tx.enqueue_packet(new_packet_data(ref_id + 0), 1, SendMode::Unreliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 1), 1, SendMode::Reliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 2), 1, SendMode::Unreliable, flush_id);

            tx.enqueue_packet(new_packet_data(ref_id + 3), 0, SendMode::Reliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 4), 0, SendMode::Unreliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 5), 0, SendMode::Reliable, flush_id);

            tx.enqueue_packet(new_packet_data(ref_id + 6), 1, SendMode::Reliable, flush_id);

            for _ in 0 .. 7 {
                tx.emit_packet_datagrams(flush_id, &mut sink);
            }

            assert_eq!(sink.pop(), (new_packet_datagram(ref_id + 0, 1, 0, 0), false));
            assert_eq!(sink.pop(), (new_packet_datagram(ref_id + 1, 1, 0, 0), true));
            assert_eq!(sink.pop(), (new_packet_datagram(ref_id + 2, 1, 1, 1), false));

            assert_eq!(sink.pop(), (new_packet_datagram(ref_id + 3, 0, 2, 0), true));
            assert_eq!(sink.pop(), (new_packet_datagram(ref_id + 4, 0, 1, 1), false));
            assert_eq!(sink.pop(), (new_packet_datagram(ref_id + 5, 0, 2, 2), true));

            assert_eq!(sink.pop(), (new_packet_datagram(ref_id + 6, 1, 1, 5), true));

            assert!(sink.is_empty());

            flush_id = flush_id.wrapping_add(1);
        }
    }

    #[test]
    fn fragment_emission() {
        use super::emit_fragments;
        use super::PacketSendEntry;

        let mut sink = TestDatagramSink::new();

        emit_fragments(PacketSendEntry::new(Box::new([]), 0, SendMode::Unreliable, 0), 0, 0, 0, &mut sink);

        assert_eq!(sink.pop(), (Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: Box::new([]),
        }, false));

        assert!(sink.is_empty());

        let packet_data = (0..MAX_FRAGMENT_SIZE).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();

        emit_fragments(PacketSendEntry::new(packet_data.clone(), 0, SendMode::Unreliable, 0), 0, 0, 0, &mut sink);

        assert_eq!(sink.pop(), (Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet_data,
        }, false));

        assert!(sink.is_empty());

        let packet_data = (0..MAX_FRAGMENT_SIZE+1).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();

        emit_fragments(PacketSendEntry::new(packet_data.clone(), 0, SendMode::Unreliable, 0), 0, 0, 0, &mut sink);

        assert_eq!(sink.pop(), (Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: packet_data[0..MAX_FRAGMENT_SIZE].into(),
        }, false));

        assert_eq!(sink.pop(), (Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE..MAX_FRAGMENT_SIZE+1].into(),
        }, false));

        assert!(sink.is_empty());
    }

    // TODO: Test transfer window limit
    // TODO: Test allocation limit
    // TODO: Test allocation size tracking
}

