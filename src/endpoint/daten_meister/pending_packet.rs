
use crate::frame;
use crate::MAX_FRAGMENT_SIZE;

use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;

#[derive(Debug)]
pub struct PendingPacket {
    data: Box<[u8]>,
    channel_id: u8,

    sequence_id: u32,
    window_parent_lead: u16,
    channel_parent_lead: u16,
    last_fragment_id: u16,

    // TODO: Use a bitfield
    ack_flags: Vec<bool>,
}

impl PendingPacket {
    pub fn new(data: Box<[u8]>, channel_id: u8,
               sequence_id: u32, window_parent_lead: u16, channel_parent_lead: u16) -> Self {
        let num_fragments = (data.len() + MAX_FRAGMENT_SIZE - 1) / MAX_FRAGMENT_SIZE + (data.len() == 0) as usize;
        debug_assert!(num_fragments != 0);

        debug_assert!(num_fragments - 1 <= u16::MAX as usize);
        let last_fragment_id = (num_fragments - 1) as u16;

        Self {
            data,
            channel_id,

            sequence_id,
            window_parent_lead,
            channel_parent_lead,
            last_fragment_id,

            ack_flags: vec![false; num_fragments],
        }
    }

    #[cfg(test)]
    pub fn sequence_id(&self) -> u32 {
        self.sequence_id
    }

    #[cfg(test)]
    pub fn channel_id(&self) -> u8 {
        self.channel_id
    }

    #[cfg(test)]
    pub fn window_parent_lead(&self) -> u16 {
        self.window_parent_lead
    }

    #[cfg(test)]
    pub fn channel_parent_lead(&self) -> u16 {
        self.channel_parent_lead
    }

    pub fn last_fragment_id(&self) -> u16 {
        self.last_fragment_id
    }

    pub fn fragment_acknowledged(&self, fragment_id: u16) -> bool {
        self.ack_flags[fragment_id as usize]
    }

    pub fn acknowledge_fragment(&mut self, fragment_id: u16) {
        self.ack_flags[fragment_id as usize] = true;
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn datagram<'a>(&'a self, fragment_id: u16) -> frame::DatagramRef<'a> {
        debug_assert!(fragment_id <= self.last_fragment_id);

        let i = fragment_id as usize;
        let data = if fragment_id == self.last_fragment_id {
            &self.data[i * MAX_FRAGMENT_SIZE .. ]
        } else {
            &self.data[i * MAX_FRAGMENT_SIZE .. (i + 1)*MAX_FRAGMENT_SIZE]
        };

        frame::DatagramRef {
            sequence_id: self.sequence_id,
            channel_id: self.channel_id,
            window_parent_lead: self.window_parent_lead,
            channel_parent_lead: self.channel_parent_lead,
            fragment_id,
            fragment_id_last: self.last_fragment_id,
            data,
        }
    }
}

pub type PendingPacketRc = Rc<RefCell<PendingPacket>>;
pub type PendingPacketWeak = Weak<RefCell<PendingPacket>>;

#[derive(Debug)]
pub struct FragmentRef {
    pub packet: PendingPacketWeak,
    pub fragment_id: u16,
}

impl Clone for FragmentRef {
    fn clone(&self) -> Self {
        Self {
            packet: Weak::clone(&self.packet),
            fragment_id: self.fragment_id,
        }
    }
}

impl FragmentRef {
    pub fn new(packet_rc: &PendingPacketRc, fragment_id: u16) -> Self {
        Self {
            packet: Rc::downgrade(packet_rc),
            fragment_id,
        }
    }
}

