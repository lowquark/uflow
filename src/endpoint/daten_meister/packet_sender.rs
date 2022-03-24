
use super::pending_packet::{PendingPacket, PendingPacketRc};

use crate::CHANNEL_COUNT;
use crate::MAX_FRAGMENT_SIZE;
use crate::MAX_PACKET_SIZE;
use crate::MAX_PACKET_WINDOW_SIZE;
use crate::packet_id;
use crate::SendMode;

use std::collections::VecDeque;
use std::cell::RefCell;
use std::rc::Rc;

// Size of the buffer that the receiver will allocate for a packet in bytes
fn alloc_size(packet_size: usize) -> usize {
    if packet_size > MAX_FRAGMENT_SIZE {
        ((packet_size + MAX_FRAGMENT_SIZE - 1) / MAX_FRAGMENT_SIZE) * MAX_FRAGMENT_SIZE
    } else {
        packet_size
    }
}

struct WindowEntry {
    // The packet to be sent in this slot
    packet: PendingPacketRc,
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

macro_rules! window_index {
    ($self:ident, $sequence_id:expr) => {
        ($sequence_id & $self.window_mask) as usize
    };
}

pub struct PacketSender {
    packet_send_queue: VecDeque<PacketSendEntry>,

    base_id: u32,
    next_id: u32,
    window: Box<[Option<WindowEntry>]>,
    window_size: u32,
    window_mask: u32,

    window_parent_id: Option<u32>,
    channels: Box<[Channel]>,

    max_alloc: usize,
    alloc: usize,

    total_size: usize,
}

impl PacketSender {
    pub fn new(window_size: u32, base_id: u32, max_alloc: usize) -> Self {
        debug_assert!(window_size > 0);
        debug_assert!(window_size <= MAX_PACKET_WINDOW_SIZE);
        debug_assert!(window_size & (window_size - 1) == 0);

        debug_assert!(packet_id::is_valid(base_id));

        let window: Vec<Option<WindowEntry>> = (0 .. window_size).map(|_| None).collect();

        let channels: Vec<Channel> = (0 .. CHANNEL_COUNT).map(|_| Channel::new()).collect();

        let max_alloc_ceil = ((max_alloc + MAX_FRAGMENT_SIZE - 1) / MAX_FRAGMENT_SIZE) * MAX_FRAGMENT_SIZE;

        Self {
            packet_send_queue: VecDeque::new(),

            base_id: base_id,
            next_id: base_id,
            window: window.into_boxed_slice(),
            window_size: window_size,
            window_mask: window_size - 1,

            window_parent_id: None,
            channels: channels.into_boxed_slice(),

            max_alloc: max_alloc_ceil,
            alloc: 0,

            total_size: 0,
        }
    }

    pub fn pending_count(&self) -> usize {
        self.packet_send_queue.len()
    }

    pub fn total_size(&self) -> usize {
        self.total_size
    }

    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    pub fn base_id(&self) -> u32 {
        self.base_id
    }

    // Places a user packet on the send queue.
    pub fn enqueue_packet(&mut self, data: Box<[u8]>, channel_id: u8, mode: SendMode, flush_id: u32) {
        debug_assert!(data.len() <= MAX_PACKET_SIZE);
        debug_assert!(data.len() <= self.max_alloc);
        debug_assert!((channel_id as usize) < CHANNEL_COUNT);

        self.total_size += data.len();
        self.packet_send_queue.push_back(PacketSendEntry::new(data, channel_id, mode, flush_id));
    }

    // Pulls a single packet from the send queue, respecting both the maximum allocation limit, and
    // the maximum transfer window.
    pub fn emit_packet(&mut self, flush_id: u32) -> Option<(PendingPacketRc, bool)> {
        while let Some(packet) = self.packet_send_queue.front() {
            match packet.mode {
                SendMode::TimeSensitive => {
                    if packet.flush_id != flush_id {
                        self.total_size -= packet.data.len();
                        self.packet_send_queue.pop_front();
                    } else {
                        break;
                    }
                }
                _ => break
            }
        }

        if let Some(packet) = self.packet_send_queue.front() {
            if packet_id::sub(self.next_id, self.base_id) >= self.window_size {
                return None;
            }

            let packet_alloc_size = alloc_size(packet.data.len());

            if self.alloc + packet_alloc_size > self.max_alloc {
                return None;
            }

            let packet = self.packet_send_queue.pop_front().unwrap();

            let sequence_id = self.next_id;
            let ref mut channel = self.channels[packet.channel_id as usize];

            let window_parent_lead =
                if let Some(parent_id) = self.window_parent_id {
                    let lead = packet_id::sub(sequence_id, parent_id);
                    debug_assert!(lead <= u16::MAX as u32);
                    lead as u16
                } else {
                    0
                };

            let channel_parent_lead =
                if let Some(parent_id) = channel.parent_id {
                    let lead = packet_id::sub(sequence_id, parent_id);
                    debug_assert!(lead <= u16::MAX as u32);
                    lead as u16
                } else {
                    0
                };

            let pending_packet = Rc::new(RefCell::new(PendingPacket::new(packet.data,
                                                                         packet.channel_id,
                                                                         sequence_id,
                                                                         window_parent_lead,
                                                                         channel_parent_lead)));

            let pending_packet_clone = Rc::clone(&pending_packet);

            let window_idx = window_index!(self, sequence_id);

            debug_assert!(self.window[window_idx].is_none());
            self.window[window_idx] = Some(WindowEntry {
                packet: pending_packet,
                alloc_size: packet_alloc_size,
                channel_id: packet.channel_id
            });

            self.next_id = packet_id::add(self.next_id, 1);

            self.alloc += packet_alloc_size;

            match packet.mode {
                SendMode::Reliable => {
                    self.window_parent_id = Some(sequence_id);
                    channel.parent_id = Some(sequence_id);
                }
                _ => ()
            }

            let resend = match packet.mode {
                SendMode::TimeSensitive => false,
                SendMode::Unreliable => false,
                SendMode::Persistent => true,
                SendMode::Reliable => true,
            };

            return Some((pending_packet_clone, resend));
        }

        return None;
    }

    // Responds to a receive window acknowledgement. All packet data beyond the new receive window
    // is forgotten, thereby freeing transfer window & allocation space for new packets.
    pub fn acknowledge(&mut self, receiver_base_id: u32) {
        let receiver_delta = packet_id::sub(receiver_base_id, self.base_id);
        let span = packet_id::sub(self.next_id, self.base_id);

        if receiver_delta > span {
            return;
        }

        while self.base_id != receiver_base_id {
            let window_idx = window_index!(self, self.base_id);
            let ref mut entry = self.window[window_idx].as_ref().unwrap();

            let ref mut channel = self.channels[entry.channel_id as usize];

            if let Some(parent_id) = self.window_parent_id {
                if parent_id == self.base_id {
                    self.window_parent_id = None;
                }
            }

            if let Some(parent_id) = channel.parent_id {
                if parent_id == self.base_id {
                    channel.parent_id = None;
                }
            }

            self.alloc -= entry.alloc_size;
            self.total_size -= entry.packet.borrow().size();

            self.window[window_idx] = None;

            self.base_id = packet_id::add(self.base_id, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_packet_data(sequence_id: u32) -> Box<[u8]> {
        sequence_id.to_be_bytes().into()
    }

    fn packet_info(emit_result: (PendingPacketRc, bool)) -> (u32, u8, u16, u16, bool) {
        let packet_ref = emit_result.0.borrow();
        (packet_ref.sequence_id(),
         packet_ref.channel_id(),
         packet_ref.window_parent_lead(),
         packet_ref.channel_parent_lead(),
         emit_result.1)
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
        let mut tx = PacketSender::new(MAX_PACKET_WINDOW_SIZE, 0, 10000);

        tx.enqueue_packet(new_packet_data(0), 0, SendMode::TimeSensitive, 0);
        tx.enqueue_packet(new_packet_data(1), 0, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(2), 0, SendMode::Persistent, 0);
        tx.enqueue_packet(new_packet_data(3), 0, SendMode::Reliable, 0);

        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (0, 0, 0, 0, false));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (1, 0, 0, 0, false));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (2, 0, 0, 0, true));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (3, 0, 0, 0, true));

        assert!(tx.emit_packet(0).is_none());
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

        let mut tx = PacketSender::new(MAX_PACKET_WINDOW_SIZE, 0, 10000);

        tx.enqueue_packet(new_packet_data(0), 1, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(1), 1, SendMode::Reliable, 0);
        tx.enqueue_packet(new_packet_data(2), 1, SendMode::Unreliable, 0);

        tx.enqueue_packet(new_packet_data(3), 0, SendMode::Reliable, 0);
        tx.enqueue_packet(new_packet_data(4), 0, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(5), 0, SendMode::Unreliable, 0);
        tx.enqueue_packet(new_packet_data(6), 0, SendMode::Reliable, 0);

        tx.enqueue_packet(new_packet_data(7), 1, SendMode::Reliable, 0);

        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (0, 1, 0, 0, false));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (1, 1, 0, 0, true));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (2, 1, 1, 1, false));
                                                                                 
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (3, 0, 2, 0, true));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (4, 0, 1, 1, false));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (5, 0, 2, 2, false));
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (6, 0, 3, 3, true));
                                                                                 
        assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (7, 1, 1, 6, true));

        assert!(tx.emit_packet(0).is_none());
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

        let mut tx = PacketSender::new(MAX_PACKET_WINDOW_SIZE, 0, 10000);

        let mut flush_id = 0;

        for i in 0 .. MAX_PACKET_WINDOW_SIZE {
            let ref_id = i*7;

            tx.acknowledge(ref_id);

            tx.enqueue_packet(new_packet_data(ref_id + 0), 1, SendMode::Unreliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 1), 1, SendMode::Reliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 2), 1, SendMode::Unreliable, flush_id);

            tx.enqueue_packet(new_packet_data(ref_id + 3), 0, SendMode::Reliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 4), 0, SendMode::Unreliable, flush_id);
            tx.enqueue_packet(new_packet_data(ref_id + 5), 0, SendMode::Reliable, flush_id);

            tx.enqueue_packet(new_packet_data(ref_id + 6), 1, SendMode::Reliable, flush_id);

            assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (ref_id + 0, 1, 0, 0, false));
            assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (ref_id + 1, 1, 0, 0, true));
            assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (ref_id + 2, 1, 1, 1, false));
                                                                                    
            assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (ref_id + 3, 0, 2, 0, true));
            assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (ref_id + 4, 0, 1, 1, false));
            assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (ref_id + 5, 0, 2, 2, true));
                                                                                    
            assert_eq!(packet_info(tx.emit_packet(0).unwrap()), (ref_id + 6, 1, 1, 5, true));

            assert!(tx.emit_packet(0).is_none());

            flush_id = packet_id::add(flush_id, 1);
        }
    }

    /*
    #[test]
    fn fragment_emission() {
        use super::emit_fragments;
        use super::PacketSendEntry;

        let mut sink = TestDatagramSink::new();

        emit_fragments(PacketSendEntry::new(Box::new([]), 0, SendMode::Unreliable, 0), 0, 0, 0, &mut sink);

        assert_eq!(sink.pop(), (frame::Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: frame::FragmentId { id: 0, last: 0 },
            data: Box::new([]),
        }, false));

        assert!(sink.is_empty());

        let packet_data = (0..MAX_FRAGMENT_SIZE).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();

        emit_fragments(PacketSendEntry::new(packet_data.clone(), 0, SendMode::Unreliable, 0), 0, 0, 0, &mut sink);

        assert_eq!(sink.pop(), (frame::Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: frame::FragmentId { id: 0, last: 0 },
            data: packet_data,
        }, false));

        assert!(sink.is_empty());

        let packet_data = (0..MAX_FRAGMENT_SIZE+1).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();

        emit_fragments(PacketSendEntry::new(packet_data.clone(), 0, SendMode::Unreliable, 0), 0, 0, 0, &mut sink);

        assert_eq!(sink.pop(), (frame::Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: frame::FragmentId { id: 0, last: 1 },
            data: packet_data[0..MAX_FRAGMENT_SIZE].into(),
        }, false));

        assert_eq!(sink.pop(), (frame::Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: frame::FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE..MAX_FRAGMENT_SIZE+1].into(),
        }, false));

        assert!(sink.is_empty());
    }
    */

    // TODO: Test transfer window limit
    // TODO: Test allocation limit
    // TODO: Test allocation size tracking
}

