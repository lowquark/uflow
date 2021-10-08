
use super::Fragment;
use super::Payload;
use super::Datagram;
use super::WindowAck;

use super::FRAGMENT_SIZE;
use super::MAX_PACKET_SIZE;
use super::TRANSFER_WINDOW_SIZE;
use super::WINDOW_ACK_SPACING;

use super::seq;

use std::collections::VecDeque;

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
        // Interim fragments must be <= FRAGMENT_SIZE
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

        if fragment.fragment_id == 0 && fragment.last_fragment_id == 0 {
            Self {
                fragments_remaining: 0,
                total_size: fragment.data.len(),
                data: fragment.data.into_vec(),
                data_recv: vec![true; 0],
            }
        } else {
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
        self.fragments_remaining -= 1;
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
    pub dud: bool,
}

impl Entry {
    fn new(sequence_id: seq::Id, dependent_lead: u16) -> Self {
        Self {
            sequence_id: sequence_id,
            dependent_lead: dependent_lead,
            frag_asm: None,
            dud: false,
        }
    }

    fn new_dud(sequence_id: seq::Id, dependent_lead: u16) -> Self {
        Self {
            sequence_id: sequence_id,
            dependent_lead: dependent_lead,
            frag_asm: None,
            dud: true,
        }
    }

    fn handle_datagram(&mut self, datagram: Datagram) {
        if datagram.sequence_id != self.sequence_id || datagram.dependent_lead != self.dependent_lead {
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

    fn is_dud(&self) -> bool {
        self.dud
    }
}

pub struct Rx {
    // Receive window base
    base_sequence_id: seq::Id,
    // Pending receive entries
    receive_queue: VecDeque<Entry>,
    // Pending window acknowledgement
    window_ack: Option<WindowAck>,
    // Maximum packet size we will receive
    max_packet_size: usize,
}

impl Rx {
    pub fn new(max_packet_size: usize) -> Self {
        assert!(max_packet_size <= MAX_PACKET_SIZE);

        Self {
            base_sequence_id: 0,
            receive_queue: VecDeque::new(),
            window_ack: None,
            max_packet_size: max_packet_size,
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

        if let Payload::Fragment(ref fragment) = datagram.payload {
            if (fragment.last_fragment_id as usize)*FRAGMENT_SIZE > self.max_packet_size {
                // A lower bound on the total packet size is greater than the maximum packet size.
                // Enqueue a dud entry instead of allocating the required memory.
                match self.receive_queue.binary_search_by(|entry| seq::lead_unsigned(entry.sequence_id, ref_id).cmp(&datagram_lead)) {
                    Err(idx) => {
                        let entry = Entry::new_dud(datagram.sequence_id, datagram.dependent_lead);
                        self.receive_queue.insert(idx, entry);
                    }
                    _ => ()
                }
                return;
            }
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

    pub fn take_window_ack(&mut self) -> Option<WindowAck> {
        self.window_ack.take()
    }

    fn new_window_ack(prev_base_id: seq::Id, base_id: seq::Id) -> Option<WindowAck> {
        if base_id & !(WINDOW_ACK_SPACING-1) != prev_base_id & !(WINDOW_ACK_SPACING-1) {
            let window_ack_id = (base_id & !(WINDOW_ACK_SPACING-1)).wrapping_sub(1) & 0xFFFFFF;
            Some(WindowAck::new(window_ack_id))
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
                    // Dependency not yet received
                    return None;
                }

                let mut deliver = false;
                let mut deliver_packet = None;

                if entry.is_dud() {
                    deliver = true;
                    deliver_packet = None;
                } else if let Some(packet) = entry.try_assemble() {
                    deliver = true;
                    if packet.len() <= self.max_packet_size {
                        deliver_packet = Some(packet);
                    } else {
                        // Packet sizes can't be predicted up front, deliver None in case the
                        // application relies on the maximum packet size.
                        deliver_packet = None;
                    }
                } else {
                    let last_lead = seq::lead_unsigned(last_id, entry.sequence_id);
                    if last_lead >= TRANSFER_WINDOW_SIZE - WINDOW_ACK_SPACING {
                        deliver = true;
                        deliver_packet = None;
                    }
                }

                if deliver {
                    self.base_sequence_id = seq::add(entry.sequence_id, 1);
                    self.receive_queue.drain(..idx+1);
                    if let Some(ack) = Self::new_window_ack(prev_base_id, self.base_sequence_id) {
                        self.window_ack = Some(ack);
                    }
                    return deliver_packet;
                }
            }
        }

        None
    }
}

#[test]
fn test_basic_receive() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

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
fn test_basic_receive_fragments() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

    let p0 = (0..FRAGMENT_SIZE*2).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();
    let p0_a = (0..FRAGMENT_SIZE).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();
    let p0_b = (FRAGMENT_SIZE..FRAGMENT_SIZE*2).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 1,
            data: p0_a.clone(),
        })
    };

    let dg1 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 1,
            last_fragment_id: 1,
            data: p0_b.clone(),
        })
    };

    rx.handle_datagram(dg0);
    rx.handle_datagram(dg1);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 1);
}

#[test]
fn test_max_packet_size() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

    for i in 0..65536 {
        let dg = Datagram {
            sequence_id: 0,
            dependent_lead: 0,
            payload: Payload::Fragment(Fragment {
                fragment_id: i as u16,
                last_fragment_id: 65535,
                data: (i*FRAGMENT_SIZE..(i+1)*FRAGMENT_SIZE).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice(),
            })
        };

        rx.handle_datagram(dg);
    }

    let p0 = (0..FRAGMENT_SIZE*65536).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();
    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive(), None);
}

#[test]
fn test_max_packet_size_exceeded() {
    let mut rx = Rx::new(3*FRAGMENT_SIZE + FRAGMENT_SIZE-1);

    for i in 0..4 {
        let dg = Datagram {
            sequence_id: 0,
            dependent_lead: 0,
            payload: Payload::Fragment(Fragment {
                fragment_id: i as u16,
                last_fragment_id: 3,
                data: (i*FRAGMENT_SIZE..(i+1)*FRAGMENT_SIZE).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice(),
            })
        };

        rx.handle_datagram(dg);
    }

    assert_eq!(rx.receive(), None);
    assert_eq!(rx.base_sequence_id, 1);
}

#[test]
fn test_invalid_fragments() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

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

    // Bad fragment identifier
    let dg1_err_a = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 1,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    // Fragment too small
    let dg1_err_b = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 1,
            data: vec![0u8; FRAGMENT_SIZE-1].into_boxed_slice(),
        })
    };

    // Fragment too large
    let dg1_err_c = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 1,
            data: vec![0u8; FRAGMENT_SIZE+1].into_boxed_slice(),
        })
    };

    // Fragment too large
    let dg1_err_d = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 1,
            last_fragment_id: 1,
            data: vec![0u8; FRAGMENT_SIZE+1].into_boxed_slice(),
        })
    };

    // True datagram
    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p1.clone(),
        })
    };

    rx.handle_datagram(dg0);
    rx.handle_datagram(dg1_err_a.clone());
    rx.handle_datagram(dg1_err_b.clone());
    rx.handle_datagram(dg1_err_c.clone());
    rx.handle_datagram(dg1_err_d.clone());
    assert_eq!(rx.receive_queue.len(), 1);
    rx.handle_datagram(dg1);
    rx.handle_datagram(dg1_err_a);
    rx.handle_datagram(dg1_err_b);
    rx.handle_datagram(dg1_err_c);
    rx.handle_datagram(dg1_err_d);
    assert_eq!(rx.receive_queue.len(), 2);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive().unwrap(), p1);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 2);
    assert_eq!(rx.receive_queue.len(), 0);
}

#[test]
fn test_reorder() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

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
    let mut rx = Rx::new(MAX_PACKET_SIZE);

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
    let mut rx = Rx::new(MAX_PACKET_SIZE);

    let p0   = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = (0..FRAGMENT_SIZE*2).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();
    let p1_a = (0..FRAGMENT_SIZE).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();
    let p1_b = (FRAGMENT_SIZE..FRAGMENT_SIZE*2).map(|v| v as u8).collect::<Vec<_>>().into_boxed_slice();
    let p2   = vec![ 8,  9, 10, 11].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1_snt = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Sentinel,
    };

    let dg1_a = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 1,
            data: p1_a.clone(),
        })
    };

    let dg1_b = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 1,
            last_fragment_id: 1,
            data: p1_b.clone(),
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
    // Stall when nothing has arrived
    assert_eq!(rx.receive(), None);

    rx.handle_datagram(dg1_snt);

    // Stall when sentinel has arrived
    assert_eq!(rx.receive(), None);

    rx.handle_datagram(dg1_a);

    // Stall when only part has arrived
    assert_eq!(rx.receive(), None);

    rx.handle_datagram(dg1_b);

    assert_eq!(rx.receive().unwrap(), p1);
    assert_eq!(rx.receive().unwrap(), p2);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 3);
}

#[test]
fn test_skip() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

    let p0   = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1_b = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p4   = vec![ 8,  9, 10, 11].into_boxed_slice();

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    // Skip a partially received packet
    let dg1 = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 1,
            data: p1_b.clone(),
        })
    };

    // Skip packets for which only a sentinel has arrived
    let dg2 = Datagram {
        sequence_id: 2,
        dependent_lead: 0,
        payload: Payload::Sentinel,
    };

    // Datagram 3 never arrives

    // Datagram 4 depends on datagram 0, but anything in between may be skipped
    let dg4 = Datagram {
        sequence_id: 4,
        dependent_lead: 4,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p4.clone(),
        })
    };

    rx.handle_datagram(dg0);
    rx.handle_datagram(dg1);
    rx.handle_datagram(dg2);
    rx.handle_datagram(dg4);

    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive().unwrap(), p4);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 5);
}

#[test]
fn test_sentinels() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();

    let dg0_snt = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Sentinel,
    };

    let dg0 = Datagram {
        sequence_id: 0,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: p0.clone(),
        })
    };

    let dg1_snt = Datagram {
        sequence_id: 1,
        dependent_lead: 0,
        payload: Payload::Sentinel,
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

    rx.handle_datagram(dg0_snt);
    rx.handle_datagram(dg0);
    rx.handle_datagram(dg1);
    rx.handle_datagram(dg1_snt);

    // Sentinels should not affect delivery regardless of order
    assert_eq!(rx.receive().unwrap(), p0);
    assert_eq!(rx.receive().unwrap(), p1);
    assert_eq!(rx.receive(), None);

    assert_eq!(rx.base_sequence_id, 2);
}

#[test]
fn test_receive_window() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

    let dg0 = Datagram {
        sequence_id: 0xFFFFFF,
        dependent_lead: 0,
        payload: Payload::Fragment(Fragment {
            fragment_id: 0,
            last_fragment_id: 0,
            data: vec![ 0xFF ].into_boxed_slice(),
        })
    };

    rx.handle_datagram(dg0);

    for i in 0..2*TRANSFER_WINDOW_SIZE {
        let dg = Datagram {
            sequence_id: i,
            dependent_lead: 0,
            payload: Payload::Fragment(Fragment {
                fragment_id: 0,
                last_fragment_id: 0,
                data: vec![ i as u8 ].into_boxed_slice(),
            })
        };

        rx.handle_datagram(dg);
    }

    for i in 0..TRANSFER_WINDOW_SIZE {
        assert_eq!(rx.receive().unwrap(), vec![ i as u8 ].into_boxed_slice());
    }

    assert_eq!(rx.receive(), None);
    assert_eq!(rx.receive_queue.len(), 0);
}

#[test]
fn test_window_acks() {
    let mut rx = Rx::new(MAX_PACKET_SIZE);

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
            assert_eq!(rx.take_window_ack().unwrap(), WindowAck::new(i));
        } else {
            assert_eq!(rx.take_window_ack(), None);
        }
    }

    assert_eq!(Rx::new_window_ack(0, WINDOW_ACK_SPACING), Some(WindowAck::new(WINDOW_ACK_SPACING-1)));
    assert_eq!(Rx::new_window_ack(0x1000000 - WINDOW_ACK_SPACING, 0x000000), Some(WindowAck::new(0xFFFFFF)));
}

