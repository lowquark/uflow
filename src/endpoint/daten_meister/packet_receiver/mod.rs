
use super::PacketSink;

use crate::frame;
use crate::CHANNEL_COUNT;
use crate::MAX_FRAGMENT_SIZE;
use crate::MAX_PACKET_WINDOW_SIZE;
use crate::packet_id;

mod assembly_window;

pub fn datagram_is_valid(dg: &frame::Datagram) -> bool {
    if dg.channel_id as usize >= CHANNEL_COUNT {
        return false;
    }
    if dg.channel_parent_lead != 0 {
        if dg.window_parent_lead == 0 || dg.channel_parent_lead < dg.window_parent_lead {
            return false;
        }
    }
    if dg.fragment_id > dg.fragment_id_last {
        return false;
    }
    if dg.fragment_id < dg.fragment_id_last && dg.data.len() != MAX_FRAGMENT_SIZE {
        return false;
    }
    if dg.data.len() > MAX_FRAGMENT_SIZE {
        return false;
    }
    return true;
}

struct ChannelAdvEntry {
    channel_id: u8,
    channel_parent_lead: u16,
}

struct WindowAdvEntry {
    window_parent_lead: u16,
}

struct DataEntry {
    data: Option<Box<[u8]>>,
}

struct Channel {
    // To ensure that only newer packets are received, if any completed packets have been taken
    // from the channel, the channel's base ID will be set ahead of the receive window's base
    // ID. Once the receive window catches up, the channel's base ID is unset, meaning the
    // channel's base ID is the same as that of the receive window.
    base_id: Option<u32>,
    packet_count: u32,
}

impl Channel {
    fn new() -> Self {
        Self {
            base_id: None,
            packet_count: 0,
        }
    }
}

macro_rules! window_index {
    ($self:ident, $sequence_id:expr) => {
        ($sequence_id & $self.receive_window_mask) as usize
    };
}

pub struct PacketReceiver {
    base_id: u32,
    end_id: u32,

    assembly_window: assembly_window::AssemblyWindow,

    receive_window_size: u32,
    receive_window_mask: u32,

    channel_entries: Box<[ChannelAdvEntry]>,
    window_entries: Box<[WindowAdvEntry]>,
    data_entries: Box<[DataEntry]>,

    entry_flags: Box<[u64]>,
    data_flags: Box<[u64]>,

    channels: Box<[Channel]>,
    channel_base_markers: Box<[Option<u8>]>,

    channel_ready_flags: u64,
    window_ready_flag: bool,
}

impl PacketReceiver {
    pub fn new(window_size: u32, base_id: u32, max_alloc: usize) -> Self {
        debug_assert!(window_size > 0);
        debug_assert!(window_size <= MAX_PACKET_WINDOW_SIZE);
        debug_assert!(window_size & (window_size - 1) == 0);

        debug_assert!(packet_id::is_valid(base_id));

        let channel_entries: Vec<ChannelAdvEntry> =
            (0 .. window_size).map(|_| ChannelAdvEntry { channel_id: 0, channel_parent_lead: 0 }).collect();
        let window_entries: Vec<WindowAdvEntry> =
            (0 .. window_size).map(|_| WindowAdvEntry { window_parent_lead: 0 }).collect();
        let data_entries: Vec<DataEntry> =
            (0 .. window_size).map(|_| DataEntry { data: None }).collect();

        let entry_flags = vec![0u64; (window_size as usize + 63)/64].into_boxed_slice();
        let data_flags = vec![0u64; (window_size as usize + 63)/64].into_boxed_slice();

        let channels: Vec<Channel> = (0 .. CHANNEL_COUNT).map(|_| Channel::new()).collect();
        let channel_base_markers: Vec<Option<u8>> = (0 .. window_size).map(|_| None).collect();

        Self {
            base_id: base_id,
            end_id: base_id,

            assembly_window: assembly_window::AssemblyWindow::new(max_alloc),

            receive_window_size: window_size,
            receive_window_mask: window_size - 1,

            channel_entries: channel_entries.into_boxed_slice(),
            window_entries: window_entries.into_boxed_slice(),
            data_entries: data_entries.into_boxed_slice(),

            entry_flags,
            data_flags,

            channels: channels.into_boxed_slice(),
            channel_base_markers: channel_base_markers.into_boxed_slice(),

            channel_ready_flags: 0,
            window_ready_flag: false,
        }
    }

    pub fn base_id(&self) -> u32 {
        self.base_id
    }

    pub fn handle_datagram(&mut self, datagram: frame::Datagram) {
        let base_id = self.base_id;
        let channel_idx = datagram.channel_id as usize;
        let sequence_id = datagram.sequence_id;

        if !datagram_is_valid(&datagram) {
            // Datagram has invalid contents
            return;
        }

        debug_assert!(packet_id::is_valid(datagram.sequence_id));

        let ref mut channel = self.channels[channel_idx];
        let channel_base_id = channel.base_id.unwrap_or(base_id);

        let channel_lead = packet_id::sub(channel_base_id, base_id);
        let packet_lead = packet_id::sub(sequence_id, base_id);

        debug_assert!(channel_lead <= self.receive_window_size);

        if packet_lead >= self.receive_window_size {
            // Packet not contained by transfer window
            return;
        }

        if packet_lead < channel_lead {
            // Packet already surpassed by this channel
            return;
        }

        let window_idx = window_index!(self, sequence_id);

        // Add this datagram to the assembly window
        if let Some(packet) = self.assembly_window.try_add(window_idx, datagram) {
            // Assembly window has produced a packet, add to receive window
            self.channel_entries[window_idx] = ChannelAdvEntry {
                channel_id: packet.channel_id,
                channel_parent_lead: packet.channel_parent_lead,
            };

            self.window_entries[window_idx] = WindowAdvEntry {
                window_parent_lead: packet.window_parent_lead,
            };

            self.data_entries[window_idx] = DataEntry {
                data: packet.data,
            };

            // Set corresponding bits in entry_flags and data_flags to
            // indicate an entry with data is present at the given sequence ID
            self.entry_flags[window_idx / 64] |= 1 << (window_idx % 64);
            self.data_flags[window_idx / 64] |= 1 << (window_idx % 64);

            // Advance end id if this packet is newer
            if packet_id::sub(sequence_id, self.end_id) < self.receive_window_size {
                self.end_id = packet_id::add(sequence_id, 1);
            }

            channel.packet_count += 1;

            // Set corresponding bit in channel_ready_flags if this packet may be received
            let channel_parent_lead = packet.channel_parent_lead as u32;
            let channel_delta = packet_id::sub(sequence_id, channel_base_id);

            if channel_parent_lead == 0 || channel_parent_lead > channel_delta {
                self.channel_ready_flags |= 1 << channel_idx;
            }

            // Set window ready flag if this packet will cause the window to advance
            let window_parent_lead = packet.window_parent_lead as u32;
            let window_delta = packet_id::sub(sequence_id, base_id);

            if window_parent_lead == 0 || window_parent_lead > window_delta {
                self.window_ready_flag = true;
            }
        }
    }

    fn set_channel_base_id(&mut self, channel_id: u8, new_id: u32) {
        let ref mut channel = self.channels[channel_id as usize];

        if let Some(base_id) = channel.base_id {
            self.channel_base_markers[window_index!(self, base_id)] = None;
        }

        let ref mut base_marker = self.channel_base_markers[window_index!(self, new_id)];

        debug_assert!(base_marker.is_none());
        *base_marker = Some(channel_id);

        channel.base_id = Some(new_id);
    }

    fn try_unset_channel_base_id(&mut self, sequence_id: u32) {
        let window_idx = window_index!(self, sequence_id);

        let ref mut base_marker = self.channel_base_markers[window_idx];
        if let Some(channel_id) = base_marker.take() {
            self.channels[channel_id as usize].base_id = None;
        }
    }

    fn advance_window(&mut self, new_base_id: u32) {
        let new_base_delta = packet_id::sub(new_base_id, self.base_id);

        debug_assert!(new_base_delta <= self.receive_window_size);

        if packet_id::sub(self.end_id, self.base_id) < new_base_delta {
            self.end_id = new_base_id;
        }

        let mut id = self.base_id;
        while id != new_base_id {
            let window_idx = window_index!(self, id);

            let flag_bit = 1 << (window_idx % 64);
            let flags_index = window_idx / 64;

            self.entry_flags[flags_index] &= !flag_bit;

            id = packet_id::add(id, 1);
        }

        let mut id = self.base_id;
        while id != new_base_id {
            let window_idx = window_index!(self, id);

            self.assembly_window.clear(window_idx);

            id = packet_id::add(id, 1);
        }

        let mut id = self.base_id;
        while id != new_base_id {
            id = packet_id::add(id, 1);

            self.try_unset_channel_base_id(id);
        }

        self.base_id = new_base_id;
    }

    // Delivers as many received packets as possible, and advances channel base IDs accordingly.
    // Advances the transfer window if no reliable packets would be skipped.
    pub fn receive(&mut self, sink: &mut impl PacketSink) {
        let base_id = self.base_id;
        let end_id = self.end_id;

        debug_assert!(packet_id::sub(end_id, base_id) <= self.receive_window_size);

        //println!(
        //  "-- receive() base_id: {} end_id: {} channel_ready_flags: {:016X} --",
        //  self.base_id,
        //  self.end_id,
        //  self.channel_ready_flags
        //);

        let mut sequence_id = base_id;

        while sequence_id != end_id {
            if self.channel_ready_flags == 0 {
                //println!("channel_ready_flags == 0, breaking");
                break;
            }

            let window_idx = window_index!(self, sequence_id);

            let flag_bit = 1 << (window_idx % 64);
            let flags_index = window_idx / 64;

            if self.data_flags[flags_index] & flag_bit != 0 {
                let ref mut channel_entry = self.channel_entries[window_idx];

                //println!(
                //  "[{}] ch: {} ch_lead: {} win_lead: {} data[0..4]: {:?}",
                //  sequence_id,
                //  entry.channel_id,
                //  entry.channel_parent_lead,
                //  entry.window_parent_lead,
                //  entry.data.as_ref().map(|data| &data[0..4])
                //);

                let channel_id = channel_entry.channel_id;
                let channel_id_bit = 1u64 << channel_id;

                if self.channel_ready_flags & channel_id_bit != 0 {
                    let ref mut channel = self.channels[channel_id as usize];

                    let channel_base_id = channel.base_id.unwrap_or(base_id);
                    debug_assert!(packet_id::sub(channel_base_id, base_id) <= self.receive_window_size);

                    let channel_parent_lead = channel_entry.channel_parent_lead as u32;
                    let channel_delta = packet_id::sub(sequence_id, channel_base_id);

                    if channel_parent_lead == 0 || channel_parent_lead > channel_delta {
                        sink.send(self.data_entries[window_idx].data.take().unwrap());

                        self.data_flags[flags_index] &= !flag_bit;

                        channel.packet_count -= 1;
                        if channel.packet_count == 0 {
                            self.channel_ready_flags &= !channel_id_bit;
                        }

                        // TODO: Base ID markers only need to be updated once per receive()
                        self.set_channel_base_id(channel_id, packet_id::add(sequence_id, 1));
                    } else {
                        // Cease to consider delivering packets from this channel until new,
                        // deliverable packets have arrived

                        // Note: If any packets are deliverable past this one (parent_lead = 0),
                        // that is an error on the sender's part

                        self.channel_ready_flags &= !channel_id_bit;
                    }
                }
            } else {
                //println!("[{}] None", sequence_id);
            }

            sequence_id = packet_id::add(sequence_id, 1);
        }

        if self.window_ready_flag {
            self.window_ready_flag = false;

            let mut new_base_id = base_id;
            let mut sequence_id = base_id;

            while sequence_id != end_id {
                let window_idx = window_index!(self, sequence_id);

                let flag_bit = 1 << (window_idx % 64);
                let flags_index = window_idx / 64;

                let next_id = packet_id::add(sequence_id, 1);

                if self.entry_flags[flags_index] & flag_bit != 0 {
                    let ref mut window_entry = self.window_entries[window_idx];

                    let window_parent_lead = window_entry.window_parent_lead as u32;
                    let window_delta = packet_id::sub(sequence_id, new_base_id);

                    if window_parent_lead == 0 || window_parent_lead > window_delta {
                        // println!("Forget sequence ID {}", sequence_id);
                        new_base_id = next_id;
                        // Window advancement implies that this packet has been delivered
                        debug_assert!(self.data_flags[flags_index] & flag_bit == 0);
                        debug_assert!(self.data_entries[window_idx].data.is_none());
                    } else {
                        // Cease to consider advancing the window
                        break;
                    }
                }

                sequence_id = next_id;
            }

            self.advance_window(new_base_id);
        }
    }

    // Responds to a resynchronization request sent by the sender. Advances the transfer window to
    // start with the given sequence ID, or to the ID of the first undelivered packet, whichever
    // comes first. Any incomplete or dropped packets are skipped, and as a result, the sender must
    // ensure that all reliable packets have been received in full prior to issuing the request.
    pub fn resynchronize(&mut self, sender_next_id: u32) {
        debug_assert!(packet_id::is_valid(sender_next_id));

        let base_id = self.base_id;
        let sender_delta = packet_id::sub(sender_next_id, base_id);

        if sender_delta > self.receive_window_size {
            return;
        }

        let mut sequence_id = base_id;

        while sequence_id != sender_next_id {
            let window_idx = window_index!(self, sequence_id);

            let flag_bit = 1 << (window_idx % 64);
            let flags_index = window_idx / 64;

            if self.entry_flags[flags_index] & flag_bit != 0 {
                // This packet awaits delivery. Stop advancement here.
                break;
            }

            sequence_id = packet_id::add(sequence_id, 1);
        }

        self.advance_window(sequence_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;

    fn new_packet_data(sequence_id: u32) -> Box<[u8]> {
        sequence_id.to_be_bytes().into()
    }

    fn new_packet_datagram(sequence_id: u32, channel_id: u8, window_parent_lead: u16, channel_parent_lead: u16) -> frame::Datagram {
        frame::Datagram {
            sequence_id,
            channel_id,
            window_parent_lead,
            channel_parent_lead,
            fragment_id: 0,
            fragment_id_last: 0,
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

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
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

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
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

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
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

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
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

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 1, 100000);
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

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
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

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
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
    fn channel_skip_received() {
        /*
                Ref              A               B    
           3               3              >3          
           2  #  #         2     _         2  #  #    
           1  O  O         1  O  O         1  O  O    
          >0  #     #     >0               0  #     # 
          -1  #  #        -1              -1  #  #    
              w  c0 c1        w  c0 c1        w  c0 c1
        */

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
        let mut sink = TestPacketSink::new();

        rx.handle_datagram(new_packet_datagram(1, 0, 1, 2));
        rx.receive(&mut sink);

        // A
        assert_eq!(sink.pop(), new_packet_data(1));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, 2);
        assert_eq!(rx.channels[0].base_id, Some(2));
        assert_eq!(rx.channels[1].base_id, None);

        rx.handle_datagram(new_packet_datagram(0, 1, 1, 0));
        rx.handle_datagram(new_packet_datagram(2, 0, 2, 3));
        rx.receive(&mut sink);

        // B
        assert_eq!(sink.pop(), new_packet_data(0));
        assert_eq!(sink.pop(), new_packet_data(2));
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 3);
        assert_eq!(rx.end_id, 3);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);
    }

    #[test]
    fn max_stall() {
        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
        let mut sink = TestPacketSink::new();

        for sequence_id in 1 .. MAX_PACKET_WINDOW_SIZE {
            rx.handle_datagram(new_packet_datagram(sequence_id, 0, sequence_id as u16, sequence_id as u16));
        }
        rx.receive(&mut sink);

        assert!(sink.is_empty());

        assert_eq!(rx.base_id, 0);
        assert_eq!(rx.end_id, MAX_PACKET_WINDOW_SIZE);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);

        rx.handle_datagram(new_packet_datagram(0, 0, 0, 0));
        rx.receive(&mut sink);

        for sequence_id in 0 .. MAX_PACKET_WINDOW_SIZE {
            assert_eq!(sink.pop(), new_packet_data(sequence_id));
        }
        assert!(sink.is_empty());

        assert_eq!(rx.base_id, MAX_PACKET_WINDOW_SIZE);
        assert_eq!(rx.end_id, MAX_PACKET_WINDOW_SIZE);
        assert_eq!(rx.channels[0].base_id, None);
        assert_eq!(rx.channels[1].base_id, None);
    }

    #[test]
    fn fill_window_n_times() {
        let n = 4;

        let mut rx = PacketReceiver::new(MAX_PACKET_WINDOW_SIZE, 0, 100000);
        let mut sink = TestPacketSink::new();

        let mut tx_id = 0;
        let mut rx_id = 0;

        for _ in 0 .. n {
            for _ in 0 .. MAX_PACKET_WINDOW_SIZE {
                rx.handle_datagram(new_packet_datagram(tx_id, 0, 0, 0));
                tx_id += 1;
            }

            rx.receive(&mut sink);

            for _ in 0 .. MAX_PACKET_WINDOW_SIZE {
                assert_eq!(sink.pop(), new_packet_data(rx_id));
                rx_id += 1;
            }
            assert!(sink.is_empty());
        }
    }

    // TODO: Test invalid datagrams
}

