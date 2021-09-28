
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
    fn new(sequence_id: seq::Id, dependent_lead: u16) -> Self {
        Self {
            sequence_id: sequence_id,
            dependent_lead: dependent_lead,
            frag_asm: None,
        }
    }

    fn handle_datagram(&mut self, datagram: Datagram) {
        if datagram.sequence_id != self.sequence_id {
            return;
        }
        if datagram.dependent_lead != self.dependent_lead {
            return;
        }

        match datagram.payload {
            Payload::Fragment(fragment) => {
                if let Some(ref mut frag_asm) = self.frag_asm {
                    frag_asm.handle_fragment(fragment);
                } else {
                    self.frag_asm = Some(FragAsm::new(fragment));
                }
            }
            Payload::Sentinel => ()
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
    // Pending window acknowledgement
    window_ack: Option<WindowAck>,
}

impl Rx {
    pub fn new() -> Self {
        Self {
            base_sequence_id: 0,
            receive_queue: VecDeque::new(),
            window_ack: None,
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
                let mut entry = Entry::new(datagram.sequence_id, datagram.dependent_lead);
                entry.handle_datagram(datagram);
                self.receive_queue.insert(idx, entry);
            }
        }
    }

    pub fn window_ack(&mut self) -> Option<WindowAck> {
        self.window_ack.take()
    }

    fn new_window_ack(prev_base_id: seq::Id, base_id: seq::Id) -> Option<WindowAck> {
        if base_id & !(WINDOW_ACK_SPACING-1) != prev_base_id & !(WINDOW_ACK_SPACING-1) {
            let window_ack_id = (base_id & !(WINDOW_ACK_SPACING-1)).wrapping_sub(1) & 0xFFFFFF;
            Some(WindowAck {
                sequence_id: window_ack_id
            })
        } else {
            None
        }
    }

    pub fn receive(&mut self) -> Option<Box<[u8]>> {
        let prev_base_id = self.base_sequence_id;

        if !self.receive_queue.is_empty() {
            let last_id = self.receive_queue.back().unwrap().sequence_id;

            for (idx, entry) in self.receive_queue.iter_mut().enumerate() {
                let lead = entry.dependent_lead as u32;

                if lead != 0 && lead <= seq::lead_unsigned(entry.sequence_id, self.base_sequence_id) {
                    return None;
                }

                if let Some(packet) = entry.try_assemble() {
                    self.base_sequence_id = seq::add(entry.sequence_id, 1);
                    self.receive_queue.drain(..idx+1);
                    if let Some(ack) = Self::new_window_ack(prev_base_id, self.base_sequence_id) {
                        self.window_ack = Some(ack);
                    }
                    return Some(packet);
                } else {
                    let last_lead = seq::lead_unsigned(last_id, entry.sequence_id);
                    if last_lead >= TRANSFER_WINDOW_SIZE - WINDOW_ACK_SPACING {
                        self.base_sequence_id = seq::add(entry.sequence_id, 1);
                        self.receive_queue.drain(..idx+1);
                        if let Some(ack) = Self::new_window_ack(prev_base_id, self.base_sequence_id) {
                            self.window_ack = Some(ack);
                        }
                        return None;
                    }
                }
            }
        }

        None
    }
}

#[test]
fn test_basic_receive() {
    let mut rx = Rx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();
    let p2 = vec![ 8,  9, 10, 11].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    let dg2 = Datagram {
        sequence_id: 2,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p2.clone(),
        })
    };

    rx.handle_datagram(dg0);
    rx.handle_datagram(dg1);
    rx.handle_datagram(dg2);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive().unwrap(), p1);
    assert_eq!(rx.receive().unwrap(), p2);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 3);
}

#[test]
fn test_invalid() {
    let mut rx = Rx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 1,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    rx.handle_datagram(dg0);
    rx.handle_datagram(dg1);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 1);
}

#[test]
fn test_reorder() {
    let mut rx = Rx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();
    let p2 = vec![ 8,  9, 10, 11].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    let dg2 = Datagram {
        sequence_id: 2,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p2.clone(),
        })
    };

    rx.handle_datagram(dg2);
    rx.handle_datagram(dg1);
    rx.handle_datagram(dg0);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive().unwrap(), p1);
    assert_eq!(rx.receive().unwrap(), p2);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 3);
}

#[test]
fn test_duplicates() {
    let mut rx = Rx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();
    let p2 = vec![ 8,  9, 10, 11].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    let dg2 = Datagram {
        sequence_id: 2,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p2.clone(),
        })
    };

    rx.handle_datagram(dg0.clone());
    rx.handle_datagram(dg1.clone());
    rx.handle_datagram(dg1);
    rx.handle_datagram(dg2.clone());

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive().unwrap(), p1);
    assert_eq!(rx.receive().unwrap(), p2);

    rx.handle_datagram(dg0);
    rx.handle_datagram(dg2);

    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 3);
}

#[test]
fn test_stall() {
    let mut rx = Rx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();
    let p2 = vec![ 8,  9, 10, 11].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    let dg2 = Datagram {
        sequence_id: 2,
        dependent_lead: 1,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p2.clone(),
        })
    };

    rx.handle_datagram(dg2);
    rx.handle_datagram(dg0);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive(), None);

    rx.handle_datagram(dg1);

    assert_eq!(rx.receive().unwrap(), p1);
    assert_eq!(rx.receive().unwrap(), p2);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 3);
}

#[test]
fn test_skip() {
    let mut rx = Rx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();
    let p2 = vec![ 8,  9, 10, 11].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    let dg2 = Datagram {
        sequence_id: 2,
        dependent_lead: 2,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p2.clone(),
        })
    };

    rx.handle_datagram(dg2);
    rx.handle_datagram(dg0);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive().unwrap(), p2);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 3);
}

#[test]
fn test_receive_window() {
    let mut rx = Rx::new();

    for i in 0..2*TRANSFER_WINDOW_SIZE {
        let dg0 = Datagram {
            sequence_id: i,
            dependent_lead: 0,
            payload: Payload::Fragment(Fragment {
                fragment_id: 0,
                last_fragment_id: 0,
                data: vec![ i as u8 ].into_boxed_slice(),
            })
        };

        rx.handle_datagram(dg0);
    }

    for i in 0..TRANSFER_WINDOW_SIZE {
        assert_eq!(rx.receive().unwrap(), vec![ i as u8 ].into_boxed_slice());
    }

    assert_eq!(rx.receive(), None);
    assert_eq!(rx.receive_queue.len(), 0);
}

#[test]
fn test_window_acks() {
    let mut rx = Rx::new();

    for i in 0..TRANSFER_WINDOW_SIZE {
        let dg0 = Datagram {
            sequence_id: i,
            dependent_lead: 0,
            payload: Payload::Fragment(Fragment {
                fragment_id: 0,
                last_fragment_id: 0,
                data: vec![ i as u8 ].into_boxed_slice(),
            })
        };

        rx.handle_datagram(dg0);
    }

    for i in 0..TRANSFER_WINDOW_SIZE {
        assert_eq!(rx.receive().unwrap(), vec![ i as u8 ].into_boxed_slice());

        if i % WINDOW_ACK_SPACING == WINDOW_ACK_SPACING - 1 {
            assert_eq!(rx.window_ack().unwrap(), WindowAck { sequence_id: i });
        } else {
            assert_eq!(rx.window_ack(), None);
        }
    }

    assert_eq!(Rx::new_window_ack(0, WINDOW_ACK_SPACING), Some(WindowAck { sequence_id: WINDOW_ACK_SPACING-1 }));
    assert_eq!(Rx::new_window_ack(0x1000000 - WINDOW_ACK_SPACING, 0x000000), Some(WindowAck { sequence_id: 0xFFFFFF }));
}

// TODO: Test fragmentation

