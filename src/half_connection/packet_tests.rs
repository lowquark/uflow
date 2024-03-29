
use super::packet_sender;
use super::packet_receiver;

use crate::MAX_PACKET_WINDOW_SIZE;
use crate::SendMode;
use crate::frame;
use crate::packet_id;

use std::collections::VecDeque;

struct TestDatagramSink {
    pub datagrams: VecDeque<frame::Datagram>,
}

impl TestDatagramSink {
    pub fn new() -> Self {
        Self {
            datagrams: VecDeque::new(),
        }
    }

    pub fn pull(&mut self, sender: &mut packet_sender::PacketSender, flush_id: u32) {
        while let Some((pending_packet_rc, _)) = sender.emit_packet(flush_id) {
            let pending_packet_ref = std::cell::RefCell::borrow(&pending_packet_rc);
            let last_fragment_id = pending_packet_ref.last_fragment_id();

            for i in 0 ..= last_fragment_id {
                self.datagrams.push_back(pending_packet_ref.datagram(i).into());
            }
        }
    }
}

struct TestPacketSink {
    pub packets: VecDeque<Box<[u8]>>,
}

impl TestPacketSink {
    pub fn new() -> Self {
        Self {
            packets: VecDeque::new(),
        }
    }
}

impl super::PacketSink for TestPacketSink {
    fn send(&mut self, packet: Box<[u8]>) {
        self.packets.push_back(packet);
    }
}

fn random_packet_data(size: usize) -> Box<[u8]> {
    (0 .. size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice()
}

fn random_packet_data_with_id(id: u32, size: usize) -> Box<[u8]> {
    let mut data = (0 .. size.max(4)).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice();
    data[0..4].clone_from_slice(&id.to_be_bytes());
    return data;
}

/// Ensures that packets of various sizes can be fragmented and reassembled correctly.
#[test]
fn random_transfer() {
    use crate::CHANNEL_COUNT;

    const NUM_PACKETS: usize = 1024;
    const MAX_PACKET_SIZE: usize = 5000;
    const MAX_ALLOC_SIZE: usize = MAX_PACKET_SIZE*NUM_PACKETS;
    const WINDOW_SIZE: u32 = 1024;

    let base_id = packet_id::sub(0u32, NUM_PACKETS as u32/2);

    let mut sender = packet_sender::PacketSender::new(WINDOW_SIZE, base_id, MAX_ALLOC_SIZE);
    let mut receiver = packet_receiver::PacketReceiver::new(WINDOW_SIZE, base_id, MAX_ALLOC_SIZE);

    let mut sent_packet_ids = [0u32; CHANNEL_COUNT];
    let mut sent_packets = VecDeque::new();

    for _ in 0 .. NUM_PACKETS {
        let channel_id = rand::random::<u8>() % CHANNEL_COUNT as u8;
        let ref mut packet_id = sent_packet_ids[channel_id as usize];

        let size = rand::random::<usize>() % MAX_PACKET_SIZE;

        sent_packets.push_back((random_packet_data_with_id(*packet_id, size), channel_id));
        *packet_id += 1;
    }

    for (packet, channel_id) in sent_packets.clone().into_iter() {
        let send_mode = match rand::random::<u32>() % 4 {
            0 => SendMode::TimeSensitive,
            1 => SendMode::Unreliable,
            2 => SendMode::Persistent,
            3 => SendMode::Reliable,
            _ => panic!()
        };

        sender.enqueue_packet(packet, channel_id, send_mode, 0);
    }

    let mut datagram_sink = TestDatagramSink::new();
    loop {
        datagram_sink.pull(&mut sender, 0);

        let datagrams = std::mem::take(&mut datagram_sink.datagrams);

        if datagrams.is_empty() {
            break;
        }

        for datagram in datagrams.into_iter() {
            receiver.handle_datagram(datagram);
        }
    }

    let mut packet_sink = TestPacketSink::new();
    receiver.receive(&mut packet_sink);

    assert_eq!(packet_sink.packets, sent_packets.into_iter().map(|pair| pair.0).collect::<Vec<_>>());
}

fn test_single_transfer(packet_size: usize, max_alloc: usize) {
    let mut sender = packet_sender::PacketSender::new(MAX_PACKET_WINDOW_SIZE, 0, max_alloc);
    let mut receiver = packet_receiver::PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, max_alloc);

    let packet_data = random_packet_data(packet_size);
    sender.enqueue_packet(packet_data.clone(), 0, SendMode::Unreliable, 0);

    let mut datagram_sink = TestDatagramSink::new();
    datagram_sink.pull(&mut sender, 0);

    for datagram in datagram_sink.datagrams.into_iter() {
        receiver.handle_datagram(datagram);
    }

    let mut packet_sink = TestPacketSink::new();
    receiver.receive(&mut packet_sink);

    assert_eq!(packet_sink.packets.len(), 1);
    assert_eq!(packet_sink.packets[0], packet_data);
}

/// Ensures a packet of size zero may be transferred.
#[test]
fn null_packet_transfer() {
    test_single_transfer(0, 10_000);
}

/// Ensures a packet of maximum size may be transferred.
/// Uses the minimum possible allocation limit.
#[test]
#[ignore]
fn max_packet_transfer() {
    use crate::MAX_PACKET_SIZE;

    test_single_transfer(MAX_PACKET_SIZE, MAX_PACKET_SIZE);
}

/// Ensures PacketSender/PacketReceiver can transfer a packet with a size equal to the maximum
/// allocation limit. (They must internally round the allocation limit up to the nearest multiple
/// of MAX_FRAGMENT_SIZE for this to work.)
#[test]
fn single_packet_max_alloc() {
    use crate::MAX_FRAGMENT_SIZE;

    let sizes = MAX_FRAGMENT_SIZE/2 .. MAX_FRAGMENT_SIZE*2 + MAX_FRAGMENT_SIZE/2;

    for size in sizes {
        test_single_transfer(size, size);
    }
}

