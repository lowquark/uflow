
use super::Fragment;
use super::Payload;
use super::Datagram;
use super::WindowAck;

use super::FRAGMENT_SIZE;
use super::TRANSFER_WINDOW_SIZE;
use super::WINDOW_ACK_SPACING;

use super::seq;

use super::SendMode;

use std::collections::VecDeque;

pub struct Tx {
    // Incrementing sequence id for send queue
    next_sequence_id: seq::Id,
    // Number of unreliable packets sent in a row
    unreliable_count: u32,
    // The id of the most recently enqueued reliable packet
    last_reliable_id: Option<seq::Id>,
    // Transfer window base
    base_sequence_id: u32,
    // Queue of outbound datagrams
    send_queue: VecDeque<(Datagram, bool)>,
}

impl Tx {
    pub fn new() -> Self {
        Self {
            next_sequence_id: 0,
            unreliable_count: 0,
            last_reliable_id: None,
            base_sequence_id: 0,
            send_queue: VecDeque::new(),
        }
    }

    pub fn enqueue_packet(&mut self, sequence_id: seq::Id, dependent_id: Option<seq::Id>, data: Box<[u8]>, reliable: bool) {
        let num_full_fragments = data.len() / FRAGMENT_SIZE;
        let bytes_remaining = data.len() % FRAGMENT_SIZE;

        let caboose = data.len() == 0 || bytes_remaining != 0;
        let num_fragments = num_full_fragments as u32 + caboose as u32;

        let dependent_lead = dependent_id.map_or(0, |id| seq::lead_unsigned(sequence_id, id) as u16);

        let last_fragment_id = (num_fragments - 1) as u16;

        for i in 0..num_full_fragments {
            let begin = i*FRAGMENT_SIZE;
            let end = (i + 1)*FRAGMENT_SIZE;
            let slice = &data[begin .. end];
            self.send_queue.push_back((
                Datagram {
                    sequence_id: sequence_id,
                    dependent_lead: dependent_lead,
                    payload: Payload::Fragment(Fragment {
                        fragment_id: i as u16,
                        last_fragment_id: last_fragment_id,
                        data: slice.into(),
                    })
                },
                reliable
            ));
        }

        if caboose {
            let begin = num_full_fragments * FRAGMENT_SIZE;
            let slice = &data[begin .. ];
            self.send_queue.push_back((
                Datagram {
                    sequence_id: sequence_id,
                    dependent_lead: dependent_lead,
                    payload: Payload::Fragment(Fragment {
                        fragment_id: last_fragment_id,
                        last_fragment_id: last_fragment_id,
                        data: slice.into(),
                    })
                },
                reliable
            ));
        }
    }

    pub fn enqueue_sentinel(&mut self, sequence_id: seq::Id, dependent_id: Option<seq::Id>) {
        let dependent_lead = dependent_id.map_or(0, |id| seq::lead_unsigned(sequence_id, id) as u16);

        self.send_queue.push_back((
            Datagram {
                sequence_id: sequence_id,
                dependent_lead: dependent_lead,
                payload: Payload::Sentinel,
            },
            true
        ));
    }

    pub fn enqueue(&mut self, data: Box<[u8]>, mode: SendMode) {
        if let Some(last_id) = self.last_reliable_id {
            if seq::lead_unsigned(self.next_sequence_id, last_id) >= TRANSFER_WINDOW_SIZE {
                self.last_reliable_id = None;
            }
        }

        let mut datagram_reliable = false;

        if mode == SendMode::Unreliable {
            self.unreliable_count += 1;
            if self.unreliable_count == WINDOW_ACK_SPACING {
                self.unreliable_count = 0;
                self.enqueue_sentinel(self.next_sequence_id, self.last_reliable_id);
                self.next_sequence_id = seq::add(self.next_sequence_id, 1);
            }
            datagram_reliable = false;
        } else {
            self.unreliable_count = 0;
            datagram_reliable = true;
        }

        self.enqueue_packet(self.next_sequence_id, self.last_reliable_id, data, datagram_reliable);

        if mode == SendMode::Reliable {
            // Future packets will wait for this one
            self.last_reliable_id = Some(self.next_sequence_id);
        }

        self.next_sequence_id = seq::add(self.next_sequence_id, 1);
    }

    pub fn try_send(&mut self) -> Option<(Datagram, bool)> {
        if let Some((datagram, _)) = self.send_queue.front() {
            let next_lead = seq::lead_unsigned(datagram.sequence_id, self.base_sequence_id);

            if next_lead < TRANSFER_WINDOW_SIZE {
                return self.send_queue.pop_front();
            }
        }

        None
    }

    pub fn handle_window_ack(&mut self, ack: WindowAck) {
        let new_base_sequence_id = seq::add(ack.sequence_id, 1);
        if seq::lead_unsigned(new_base_sequence_id, self.base_sequence_id) <= TRANSFER_WINDOW_SIZE {
            self.base_sequence_id = new_base_sequence_id;
        }
    }
}

/*
#[test]
fn test_basic_send() {
    let mut tx = Tx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();
    let p2 = vec![ 8,  9, 10, 11].into_boxed_slice();

    tx.enqueue(p0.clone(), SendMode::Unreliable);
    tx.enqueue(p1.clone(), SendMode::Reliable);
    tx.enqueue(p2.clone(), SendMode::Passive);

    let dg0 = tx.try_send().unwrap();
    let dg1 = tx.try_send().unwrap();
    let dg2 = tx.try_send().unwrap();

    assert_eq!(dg0, Datagram::Fragment(Fragment{ sequence_id: 0, dependent_lead: 0, fragment_id: 0, last_fragment_id: 0, data: p0, reliable: false }));
    assert_eq!(dg1, Datagram::Fragment(Fragment{ sequence_id: 1, dependent_lead: 0, fragment_id: 0, last_fragment_id: 0, data: p1, reliable: true }));
    assert_eq!(dg2, Datagram::Fragment(Fragment{ sequence_id: 2, dependent_lead: 1, fragment_id: 0, last_fragment_id: 0, data: p2, reliable: true }));
}

#[test]
fn test_dependents() {
    let mut tx = Tx::new();

    let p0 = vec![ 0,  1,  2,  3].into_boxed_slice();
    tx.enqueue(p0, SendMode::Reliable);

    for _ in 0 .. TRANSFER_WINDOW_SIZE {
        let p1 = vec![ 4,  5,  6,  7].into_boxed_slice();
        tx.enqueue(p1, SendMode::Unreliable);
    }

    for i in 0 .. TRANSFER_WINDOW_SIZE {
        let dg = tx.try_send().unwrap();
        assert_eq!(dg.dependent_lead(), i as u16);
    }

    tx.handle_window_ack(WindowAck{ sequence_id: WINDOW_ACK_SPACING - 1 });

    let dg = tx.try_send().unwrap();
    assert_eq!(dg.dependent_lead(), 0);
}
*/

