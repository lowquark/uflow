
use super::Fragment;
use super::Payload;
use super::Datagram;
use super::WindowAck;

use super::FRAGMENT_SIZE;
use super::TRANSFER_WINDOW_SIZE;
use super::WINDOW_ACK_SPACING;

use super::seq;

use std::collections::VecDeque;

const SENTINEL_EXPIRATION_THRESHOLD: usize = (TRANSFER_WINDOW_SIZE/WINDOW_ACK_SPACING - 2) as usize;

struct FragAsm {
    fragments_remaining: usize,
    total_size: usize,
    data: Vec<u8>,
    data_recv: Vec<bool>,
}

fn validate_fragment(fragment: &Fragment) -> bool {
    if fragment.fragment_id > fragment.last_fragment_id {
        // Invalid fragment ids
        return false;
    }

    if fragment.fragment_id == fragment.last_fragment_id {
        // Last fragment may be <= FRAGMENT_SIZE
        if fragment.data.len() > FRAGMENT_SIZE {
            return false;
        }
    } else {
        // Last fragment must be == FRAGMENT_SIZE
        if fragment.data.len() != FRAGMENT_SIZE {
            return false;
        }
    }

    return true;
}

fn validate_datagram(datagram: &Datagram) -> bool {
    match &datagram.payload {
        Payload::Fragment(fragment) => validate_fragment(fragment),
        Payload::Sentinel => true,
    }
}

impl FragAsm {
    fn new(fragment: Fragment) -> Self {
        assert!(validate_fragment(&fragment));

        let num_fragments = fragment.last_fragment_id as usize + 1;
        let fragment_id = fragment.fragment_id as usize;

        let mut data = vec![0u8; num_fragments*FRAGMENT_SIZE];
        data[fragment_id*FRAGMENT_SIZE ..
             fragment_id*FRAGMENT_SIZE + fragment.data.len()].copy_from_slice(&fragment.data);

        let mut data_recv = vec![false; num_fragments];
        data_recv[fragment_id] = true;

        let total_size: usize = fragment.data.len();

        Self {
            fragments_remaining: num_fragments - 1,
            total_size: total_size,
            data: data,
            data_recv: data_recv,
        }
    }

    fn handle_fragment(&mut self, fragment: Fragment) {
        assert!(validate_fragment(&fragment));

        let num_fragments = fragment.last_fragment_id as usize + 1;
        let fragment_id = fragment.fragment_id as usize;

        if num_fragments != self.data_recv.len() {
            // Fragment does not match this entry
            return;
        }

        if self.data_recv[fragment_id] {
            // Fragment already seen
            return;
        }

        // Assign fragment data
        self.data[fragment_id*FRAGMENT_SIZE ..
                           fragment_id*FRAGMENT_SIZE + fragment.data.len()].copy_from_slice(&fragment.data);
        // Mark as received
        self.data_recv[fragment_id] = true;
        // Update total size
        self.total_size += fragment.data.len();
    }

    fn try_assemble(&mut self) -> Option<Box<[u8]>> {
        if self.fragments_remaining == 0 {
            self.data.truncate(self.total_size);
            Some(std::mem::take(&mut self.data).into_boxed_slice())
        } else {
            None
        }
    }
}

struct Entry {
    pub sequence_id: seq::Id,
    pub dependent_lead: u16,
    pub frag_asm: Option<FragAsm>,
}

impl Entry {
    fn new(datagram: Datagram) -> Self {
        Self {
            sequence_id: datagram.sequence_id,
            dependent_lead: datagram.dependent_lead,
            frag_asm: match datagram.payload {
                Payload::Fragment(fragment) => Some(FragAsm::new(fragment)),
                Payload::Sentinel => None,
            }
        }
    }

    fn handle_datagram(&mut self, datagram: Datagram) {
        if datagram.sequence_id != self.sequence_id {
            return;
        }
        if datagram.dependent_lead != self.dependent_lead {
            return;
        }
        if let Some(ref mut frag_asm) = self.frag_asm {
            match datagram.payload {
                Payload::Fragment(fragment) => frag_asm.handle_fragment(fragment),
                _ => (),
            }
        }
    }

    fn try_assemble(&mut self) -> Option<Box<[u8]>> {
        if let Some(ref mut frag_asm) = self.frag_asm {
            frag_asm.try_assemble()
        } else {
            None
        }
    }

    fn is_sentinel(&self) -> bool {
        self.frag_asm.is_none()
    }
}

pub struct Rx {
    // Receive window base
    base_sequence_id: seq::Id,
    // Pending receive entries
    receive_queue: VecDeque<Entry>,
    // Number of sentinel entries in the queue
    num_sentinels: usize,
}

impl Rx {
    pub fn new() -> Self {
        Self {
            base_sequence_id: 0,
            receive_queue: VecDeque::new(),
            num_sentinels: 0,
        }
    }

    pub fn handle_datagram(&mut self, datagram: Datagram) {
        if !validate_datagram(&datagram) {
            return;
        }

        let ref_id = self.base_sequence_id;
        let datagram_lead = seq::lead_unsigned(datagram.sequence_id, ref_id);

        if datagram_lead >= TRANSFER_WINDOW_SIZE {
            return;
        }

        match self.receive_queue.binary_search_by(|entry| seq::lead_unsigned(entry.sequence_id, ref_id).cmp(&datagram_lead)) {
            Ok(idx) => {
                self.receive_queue[idx].handle_datagram(datagram);
            }
            Err(idx) => {
                let entry = Entry::new(datagram);
                if entry.is_sentinel() {
                    self.num_sentinels += 1;
                }
                self.receive_queue.insert(idx, entry);
            }
        }
    }

    pub fn receive(&mut self) -> Option<Box<[u8]>> {
        // Count sentinels seen before calling drain so as to update total
        let mut sentinel_drain_count: usize = 0;

        for (idx, entry) in self.receive_queue.iter_mut().enumerate() {
            let lead = entry.dependent_lead as u32;
            if lead <= seq::lead_unsigned(entry.sequence_id, self.base_sequence_id) {
                return None;
            }

            if let Some(packet) = entry.try_assemble() {
                self.base_sequence_id = seq::add(entry.sequence_id, 1);
                self.receive_queue.drain(..idx+1);
                self.num_sentinels -= sentinel_drain_count;
                return Some(packet);
            }

            if entry.is_sentinel() {
                sentinel_drain_count += 1;

                if self.num_sentinels >= SENTINEL_EXPIRATION_THRESHOLD {
                    self.base_sequence_id = seq::add(entry.sequence_id, 1);
                    self.receive_queue.drain(..idx+1);
                    self.num_sentinels -= sentinel_drain_count;
                    return None;
                }
            }
        }

        None
    }
}

