
mod assembly_window;

use assembly_window::AssemblyWindow;

use crate::MAX_CHANNELS;
use crate::MAX_FRAGMENT_SIZE;
use crate::MAX_PACKET_TRANSFER_WINDOW_SIZE;

pub use crate::frame::Datagram;

struct ReceiveEntry {
    channel_id: u8,
    window_parent_lead: u16,
    channel_parent_lead: u16,
    data: Option<Box<[u8]>>,
}

struct Channel {
    // To ensure that only newer packets are received, if any completed packets have been taken
    // from the channel, the channel's base ID will be set ahead of the receive window's base
    // ID. Once the receive window catches up, the channel's base ID is unset, meaning the
    // channel's base ID is the same as that of the receive window.
    base_id: Option<u32>,
}

impl Channel {
    fn new() -> Self {
        Self {
            base_id: None,
        }
    }
}

pub trait PacketSink {
    fn send(&mut self, packet_data: Box<[u8]>);
}

pub struct PacketReceiver {
    base_id: u32,
    end_id: u32,

    assembly_window: AssemblyWindow,

    receive_window: Box<[Option<ReceiveEntry>]>,

    channels: Box<[Channel]>,
    channel_base_markers: Box<[Option<u8>]>,
}

impl PacketReceiver {
    pub fn new(channel_num: usize, max_alloc: usize, base_id: u32) -> Self {
        debug_assert!(channel_num <= MAX_CHANNELS);

        let receive_window: Vec<Option<ReceiveEntry>> = (0..MAX_PACKET_TRANSFER_WINDOW_SIZE).map(|_| None).collect();
        let channels: Vec<Channel> = (0..channel_num).map(|_| Channel::new()).collect();
        let channel_base_markers: Vec<Option<u8>> = (0..MAX_PACKET_TRANSFER_WINDOW_SIZE).map(|_| None).collect();

        Self {
            base_id: base_id,
            end_id: base_id,

            assembly_window: AssemblyWindow::new(max_alloc),

            receive_window: receive_window.into_boxed_slice(),

            channels: channels.into_boxed_slice(),
            channel_base_markers: channel_base_markers.into_boxed_slice(),
        }
    }

    pub fn base_id(&self) -> u32 {
        self.base_id
    }

    fn window_index(sequence_id: u32) -> usize {
        (sequence_id % MAX_PACKET_TRANSFER_WINDOW_SIZE) as usize
    }

    pub fn handle_datagram(&mut self, datagram: Datagram) {
        let base_id = self.base_id;
        let channel_idx = datagram.channel_id as usize;
        let sequence_id = datagram.sequence_id;

        if !datagram.is_valid() {
            // Datagram has invalid contents
            return;
        }

        if sequence_id.wrapping_sub(base_id) >= MAX_PACKET_TRANSFER_WINDOW_SIZE {
            // Packet not contained by transfer window
            return;
        }

        if channel_idx >= self.channels.len() {
            // Packet references non-existent channel
            return;
        }

        let ref mut channel = self.channels[channel_idx];

        let channel_base_id = channel.base_id.unwrap_or(base_id);
        debug_assert!(channel_base_id.wrapping_sub(base_id) <= MAX_PACKET_TRANSFER_WINDOW_SIZE);

        if (sequence_id.wrapping_sub(channel_base_id) as i32) < 0 {
            // Packet already surpassed by this channel
            return;
        }

        let window_idx = Self::window_index(sequence_id);

        // Add this datagram to the assembly window
        if let Some(packet) = self.assembly_window.try_add(window_idx, datagram) {
            // Assembly window has produced a packet, add to receive window
            let new_entry = ReceiveEntry {
                channel_id: packet.channel_id,
                window_parent_lead: packet.window_parent_lead,
                channel_parent_lead: packet.channel_parent_lead,
                data: packet.data,
            };

            debug_assert!(self.receive_window[window_idx].is_none());
            self.receive_window[window_idx] = Some(new_entry);

            // Advance end id if this packet is newer
            if sequence_id.wrapping_sub(self.end_id) < MAX_PACKET_TRANSFER_WINDOW_SIZE {
                self.end_id = sequence_id.wrapping_add(1);
            }
        }
    }

    fn set_channel_base_id(&mut self, channel_id: u8, new_id: u32) {
        let ref mut channel = self.channels[channel_id as usize];

        if let Some(base_id) = channel.base_id {
            self.channel_base_markers[Self::window_index(base_id)] = None;
        }

        debug_assert!(self.channel_base_markers[Self::window_index(new_id)].is_none());
        self.channel_base_markers[Self::window_index(new_id)] = Some(channel_id);

        channel.base_id = Some(new_id);
    }

    fn try_unset_channel_base_id(&mut self, sequence_id: u32) {
        let window_idx = Self::window_index(sequence_id);

        if let Some(channel_id) = self.channel_base_markers[window_idx] {
            let ref mut channel = self.channels[channel_id as usize];
            channel.base_id = None;
            self.channel_base_markers[window_idx] = None;
        }
    }

    fn advance_window(&mut self, new_base_id: u32) {
        let base_id = self.base_id;
        let new_base_delta = new_base_id.wrapping_sub(base_id);

        if new_base_delta != 0 {
            assert!(new_base_delta <= MAX_PACKET_TRANSFER_WINDOW_SIZE);

            for i in 0 .. new_base_delta {
                let sequence_id = base_id.wrapping_add(i);
                self.assembly_window.clear(Self::window_index(sequence_id));

                let next_sequence_id = base_id.wrapping_add(i + 1);
                self.try_unset_channel_base_id(next_sequence_id);
            }

            let end_delta = self.end_id.wrapping_sub(base_id);
            if end_delta < new_base_delta {
                self.end_id = new_base_id;
            }

            self.base_id = new_base_id;
        }
    }

    // Delivers as many received packets as possible, and advances channel base IDs accordingly.
    // Advances the transfer window if no reliable packets would be skipped.
    pub fn receive(&mut self, sink: &mut impl PacketSink) {
        let base_id = self.base_id;
        let slot_num = self.end_id.wrapping_sub(base_id);

        let mut channel_flags = (1u64 << self.channels.len()).wrapping_sub(1);

        debug_assert!(slot_num <= MAX_PACKET_TRANSFER_WINDOW_SIZE);

        for i in 0 .. slot_num {
            if channel_flags == 0 {
                break;
            }

            let sequence_id = base_id.wrapping_add(i);

            let ref mut receive_entry = self.receive_window[Self::window_index(sequence_id)];

            if let Some(entry) = receive_entry {
                let channel_id = entry.channel_id;
                let channel_bit = 1u64 << channel_id;

                if channel_flags & channel_bit != 0 {
                    let ref mut channel = self.channels[channel_id as usize];

                    let channel_base_id = channel.base_id.unwrap_or(base_id);
                    debug_assert!(channel_base_id.wrapping_sub(base_id) <= MAX_PACKET_TRANSFER_WINDOW_SIZE);

                    let channel_parent_lead = entry.channel_parent_lead as u32;
                    let channel_delta = sequence_id.wrapping_sub(channel_base_id);

                    if channel_parent_lead == 0 || channel_parent_lead > channel_delta {
                        if let Some(data) = entry.data.take() {
                            sink.send(data);
                        }

                        // TODO: This only needs to be performed once per channel with packets
                        // delivered
                        self.set_channel_base_id(channel_id, sequence_id.wrapping_add(1));
                    } else {
                        // Cease to consider delivering packets from this channel

                        // Note: If any packets are deliverable past this one (parent_lead = 0),
                        // that is an error on the sender's part. However, ignoring all future
                        // packets on this channel permits faster iteration and a possible early
                        // exit.

                        channel_flags &= !channel_bit;
                    }
                }
            }
        }

        let mut new_base_id = base_id;

        for i in 0 .. slot_num {
            let sequence_id = base_id.wrapping_add(i);

            let ref mut receive_entry = self.receive_window[Self::window_index(sequence_id)];

            if let Some(entry) = receive_entry {
                let window_parent_lead = entry.window_parent_lead as u32;
                let window_delta = sequence_id.wrapping_sub(new_base_id);

                if window_parent_lead == 0 || window_parent_lead > window_delta {
                    new_base_id = sequence_id.wrapping_add(1);
                    *receive_entry = None;
                } else {
                    // Cease to consider advancing the window
                    break;
                }
            }
        }

        self.advance_window(new_base_id);
    }

    // Responds to a resynchronization request sent by the sender. Advances the transfer window to
    // start with the given sequence ID, or to the ID of the first undelivered packet, whichever
    // comes first. Any incomplete or dropped packets are skipped, and as a result, the sender must
    // ensure that all reliable packets have been received in full prior to issuing the request.
    pub fn resynchronize(&mut self, sender_next_id: u32) {
        let sender_delta = sender_next_id.wrapping_sub(self.base_id);

        if sender_delta > MAX_PACKET_TRANSFER_WINDOW_SIZE {
            return;
        }

        for i in 0 .. sender_delta {
            let sequence_id = self.base_id.wrapping_add(i);

            let ref mut receive_entry = self.receive_window[Self::window_index(sequence_id)];

            if let Some(_) = receive_entry {
                // This packet awaits delivery. Stop advancement here.
                self.advance_window(sequence_id);
                return;
            }
        }

        // No complete packets exist. Match with sender.
        self.advance_window(sender_next_id);
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::Datagram;
    use crate::frame::FragmentId;
    use super::MAX_PACKET_TRANSFER_WINDOW_SIZE;

    use super::PacketReceiver;

    use std::collections::VecDeque;

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

    struct TestPacketSink {
        packets: VecDeque<Box<[u8]>>,
    }

    impl TestPacketSink {
        fn new() -> Self {
            Self {
                packets: VecDeque::new(),
            }
        }

        fn pop(&mut self) -> Box<[u8]> {
            self.packets.pop_front().unwrap()
        }

        fn is_empty(&mut self) -> bool {
            self.packets.is_empty()
        }
    }

    impl super::PacketSink for TestPacketSink {
        fn send(&mut self, packet: Box<[u8]>) {
            self.packets.push_back(packet);
        }
    }

    #[test]
    fn basic() {
        /*
                Ref           A 
           1           >1       
          >0  O  O      0  O  O 
              w  c0        w  c0
        */

        let mut rx = PacketReceiver::new(1, 100000, 0);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(0, 0, 0, 0));
        rx.receive(&mut sink);

        // A
        assert_eq!(sink.pop(), new_packet_data(0));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 1);
        assert_eq!(rx.end_id, 1);
        assert_eq!(rx.channels[0].base_id, None);
    }

    #[test]
    fn basic_skip() {
        /*
                Ref           A 
           2           >2       
           1  O  O      1  O  O 
          >0  O  O      0       
              w  c0        w  c0
        */

        let mut rx = PacketReceiver::new(1, 100000, 0);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(1, 0, 0, 0));
        rx.receive(&mut sink);

        // A
        assert_eq!(sink.pop(), new_packet_data(1));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 2);
        assert_eq!(rx.end_id, 2);
        assert_eq!(rx.channels[0].base_id, None);
    }

    #[test]
    fn basic_stall() {
        /*
                Ref           A 
           2            2       
           1  O  O      1  O  O 
          >0  #  #     >0       
              w  c0        w  c0
        */

        let mut rx = PacketReceiver::new(1, 100000, 0);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(1, 0, 1, 1));
        rx.receive(&mut sink);

        // A
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, 2);
        assert_eq!(rx.channels[0].base_id, None);
    }

    #[test]
    fn partial_advancement() {
        /*
                Ref              A    
           5               5     _    
           4  O  O         4  O  O    
           3  O  O         3  O  O    
           2  O     O      2  O     O 
           1               1          
          >0  #     #     >0          
              w  c0 c1        w  c0 c1
        */

        let mut rx = PacketReceiver::new(2, 100000, 0);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(2, 1, 2, 2));
        rx.handle_datagram(new_packet_datagram(3, 0, 3, 0));
        rx.handle_datagram(new_packet_datagram(4, 0, 4, 0));
        rx.receive(&mut sink);

        // A
        assert_eq!(sink.pop(), new_packet_data(3));
        assert_eq!(sink.pop(), new_packet_data(4));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, 5);
        assert_eq!(rx.channels[0].base_id, Some(5));
        assert_eq!(rx.channels[1].base_id, None);
    }

    #[test]
    fn full_advancement() {
        /*
                Ref              A    
           5              >5          
           4  O  O         4  O  O    
           3  O  O         3  O  O    
           2  O     O      2  O     O 
          >1               1          
           0  #     #      0 (#)   (#)
              w  c0 c1        w  c0 c1
        */

        let mut rx = PacketReceiver::new(2, 100000, 1);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(2, 1, 2, 2));
        rx.handle_datagram(new_packet_datagram(3, 0, 3, 0));
        rx.handle_datagram(new_packet_datagram(4, 0, 4, 0));

        rx.receive(&mut sink);
        assert_eq!(sink.pop(), new_packet_data(2));
        assert_eq!(sink.pop(), new_packet_data(3));
        assert_eq!(sink.pop(), new_packet_data(4));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 5);
        assert_eq!(rx.end_id, 5);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);
    }

    #[test]
    fn channel_stall() {
        /*
                Ref              A               B               C    
           6               6               6              >6          
           5  #  #         5     _        >5               5  #  #    
           4  #  #         4  #  #         4  #  #         4  #  #    
           3  O     O      3  O     O      3  O     O      3  O     O 
           2  O     O      2  O     O      2  O     O      2  O     O 
           1  O     O      1  O     O      1  O     O      1  O     O 
          >0  #     #     >0               0  #     #      0  #     # 
              w  c0 c1        w  c0 c1        w  c0 c1        w  c0 c1
        */

        let mut rx = PacketReceiver::new(2, 100000, 0);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(1, 1, 1, 1));
        rx.handle_datagram(new_packet_datagram(2, 1, 2, 2));
        rx.handle_datagram(new_packet_datagram(3, 1, 3, 3));
        rx.handle_datagram(new_packet_datagram(4, 0, 4, 0));
        rx.receive(&mut sink);

        // A
        assert_eq!(sink.pop(), new_packet_data(4));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, 5);
        assert_eq!(rx.channels[0].base_id, Some(5));
        assert_eq!(rx.channels[1].base_id, None);

        rx.handle_datagram(new_packet_datagram(0, 1, 0, 0));
        rx.receive(&mut sink);

        // B
        assert_eq!(sink.pop(), new_packet_data(0));
        assert_eq!(sink.pop(), new_packet_data(1));
        assert_eq!(sink.pop(), new_packet_data(2));
        assert_eq!(sink.pop(), new_packet_data(3));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 5);
        assert_eq!(rx.end_id, 5);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);

        rx.handle_datagram(new_packet_datagram(5, 0, 1, 1));
        rx.receive(&mut sink);

        // C
        assert_eq!(sink.pop(), new_packet_data(5));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 6);
        assert_eq!(rx.end_id, 6);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);
    }

    #[test]
    fn channel_ignore_old() {
        /*
                Ref              A               B               C    
           3               3     _         3     _        >3          
           2  #  #         2  #  #         2  #  #         2  #  #    
           1  O  O         1               1  O  O         1  O  O    
          >0  #     #     >0              >0               0  #     # 
              w  c0 c1        w  c0 c1        w  c0 c1        w  c0 c1
        */

        let mut rx = PacketReceiver::new(2, 100000, 0);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(2, 0, 2, 0));
        rx.receive(&mut sink);

        // A
        assert_eq!(sink.pop(), new_packet_data(2));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, 3);
        assert_eq!(rx.channels[0].base_id, Some(3));
        assert_eq!(rx.channels[1].base_id, None);

        rx.handle_datagram(new_packet_datagram(1, 0, 1, 0));
        rx.receive(&mut sink);

        // B
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, 3);
        assert_eq!(rx.channels[0].base_id, Some(3));
        assert_eq!(rx.channels[1].base_id, None);

        rx.handle_datagram(new_packet_datagram(0, 1, 0, 0));
        rx.receive(&mut sink);

        // C
        assert_eq!(sink.pop(), new_packet_data(0));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 3);
        assert_eq!(rx.end_id, 3);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);
    }

    #[test]
    fn max_stall() {
        let mut rx = PacketReceiver::new(2, 100000, 0);
        let mut sink = TestPacketSink::new();

        for sequence_id in 1 .. MAX_PACKET_TRANSFER_WINDOW_SIZE {
            rx.handle_datagram(new_packet_datagram(sequence_id, 0, sequence_id as u16, sequence_id as u16));
        }
        rx.receive(&mut sink);

        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, MAX_PACKET_TRANSFER_WINDOW_SIZE);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);

        rx.handle_datagram(new_packet_datagram(0, 0, 0, 0));
        rx.receive(&mut sink);

        for sequence_id in 0 .. MAX_PACKET_TRANSFER_WINDOW_SIZE {
            assert_eq!(sink.pop(), new_packet_data(sequence_id));
        }
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, MAX_PACKET_TRANSFER_WINDOW_SIZE);
        assert_eq!(rx.end_id, MAX_PACKET_TRANSFER_WINDOW_SIZE);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);
    }

    #[test]
    fn fill_window_n_times() {
        let n = 4;

        let mut rx = PacketReceiver::new(1, 100000, 0);
        let mut sink = TestPacketSink::new();

        let mut tx_id = 0;
        let mut rx_id = 0;

        for _ in 0 .. n {
            for _ in 0 .. MAX_PACKET_TRANSFER_WINDOW_SIZE {
                rx.handle_datagram(new_packet_datagram(tx_id, 0, 0, 0));
                tx_id += 1;
            }

            rx.receive(&mut sink);

            for _ in 0 .. MAX_PACKET_TRANSFER_WINDOW_SIZE {
                assert_eq!(sink.pop(), new_packet_data(rx_id));
                rx_id += 1;
            }
            assert!(sink.is_empty());
        }
    }

    // TODO: Test invalid datagrams
}

