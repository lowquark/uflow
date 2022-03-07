
mod fragment_buffer;

use fragment_buffer::FragmentBuffer;

use super::MAX_FRAGMENT_SIZE;
use super::MAX_PACKET_WINDOW_SIZE;
use super::Datagram;

struct ActiveEntry {
    // How many allocation points this entry is worth
    alloc_size: usize,

    // Used to validate future packets
    channel_id: u8,
    window_parent_lead: u16,
    channel_parent_lead: u16,
    last_fragment_id: u16,

    // Assembles a multi-fragment packet
    asm_buffer: FragmentBuffer,
}

impl ActiveEntry {
    fn new(alloc_size: usize, channel_id: u8, window_parent_lead: u16, channel_parent_lead: u16, last_fragment_id: u16, num_fragments: usize) -> Self {
        Self {
            alloc_size,

            channel_id,
            window_parent_lead,
            channel_parent_lead,
            last_fragment_id,

            asm_buffer: FragmentBuffer::new(num_fragments),
        }
    }
}

enum WindowEntry {
    Open,
    Closed(usize),
    Active(ActiveEntry),
}

#[derive(Debug,PartialEq)]
pub struct Packet {
    pub channel_id: u8,
    pub sequence_id: u32,
    pub window_parent_lead: u16,
    pub channel_parent_lead: u16,
    pub data: Option<Box<[u8]>>,
}

fn packet_alloc_size(datagram: &Datagram) -> usize {
    let num_fragments = datagram.fragment_id.last as usize + 1;
    if num_fragments > 1 {
        num_fragments*MAX_FRAGMENT_SIZE
    } else {
        datagram.data.len()
    }
}

pub struct AssemblyWindow {
    window: Box<[WindowEntry]>,

    alloc: usize,
    max_alloc: usize,
}

impl AssemblyWindow {
    pub fn new(max_alloc: usize) -> Self {
        let max_alloc_ceil = (max_alloc + MAX_FRAGMENT_SIZE - 1)/MAX_FRAGMENT_SIZE*MAX_FRAGMENT_SIZE;

        let window: Vec<WindowEntry> = (0..MAX_PACKET_WINDOW_SIZE).map(|_| WindowEntry::Open).collect();

        Self {
            window: window.into_boxed_slice(),

            alloc: 0,
            max_alloc: max_alloc_ceil,
        }
    }

    pub fn try_add(&mut self, idx: usize, datagram: Datagram) -> Option<Packet> {
        match &mut self.window[idx] {
            WindowEntry::Open => {
                // New packet

                // If this packet would exceed the memory limit, enqueue a closed entry with no
                // allocation value and pass on a dud packet (as if the packet was received in
                // full). Otherwise, pass the datagram on directly or allocate an assembly
                // buffer as appropriate.

                let alloc_size = packet_alloc_size(&datagram);

                if self.alloc + alloc_size > self.max_alloc {
                    // Never should have come here!
                    self.window[idx] = WindowEntry::Closed(0);

                    return Some(Packet {
                        channel_id: datagram.channel_id,
                        sequence_id: datagram.sequence_id,
                        window_parent_lead: datagram.window_parent_lead,
                        channel_parent_lead: datagram.channel_parent_lead,
                        data: None
                    });
                } else {
                    self.alloc += alloc_size;

                    if datagram.fragment_id.last == 0 {
                        let new_entry = WindowEntry::Closed(alloc_size);

                        self.window[idx] = new_entry;

                        return Some(Packet {
                            channel_id: datagram.channel_id,
                            sequence_id: datagram.sequence_id,
                            window_parent_lead: datagram.window_parent_lead,
                            channel_parent_lead: datagram.channel_parent_lead,
                            data: Some(datagram.data),
                        });
                    } else {
                        let num_fragments = datagram.fragment_id.last as usize + 1;

                        let mut new_entry = ActiveEntry::new(alloc_size,
                                                             datagram.channel_id,
                                                             datagram.window_parent_lead,
                                                             datagram.channel_parent_lead,
                                                             datagram.fragment_id.last, 
                                                             num_fragments);

                        new_entry.asm_buffer.write(datagram.fragment_id.id as usize, datagram.data);

                        self.window[idx] = WindowEntry::Active(new_entry);

                        return None;
                    }
                }
            }
            WindowEntry::Closed(_) => {
                // Packet has been rejected or has already been received
                return None;
            }
            WindowEntry::Active(ref mut entry) => {
                // In-progress packet

                // Validate datagram against existing datagrams
                if datagram.channel_id != entry.channel_id {
                    return None;
                }
                if datagram.window_parent_lead != entry.window_parent_lead {
                    return None;
                }
                if datagram.channel_parent_lead != entry.channel_parent_lead {
                    return None;
                }
                if datagram.fragment_id.last != entry.last_fragment_id {
                    return None;
                }

                entry.asm_buffer.write(datagram.fragment_id.id as usize, datagram.data);

                if entry.asm_buffer.is_finished() {
                    let new_entry = WindowEntry::Closed(entry.alloc_size);
                    let prev_entry = std::mem::replace(&mut self.window[idx], new_entry);

                    match prev_entry {
                        WindowEntry::Active(entry) => {
                            return Some(Packet {
                                channel_id: datagram.channel_id,
                                sequence_id: datagram.sequence_id,
                                window_parent_lead: datagram.window_parent_lead,
                                channel_parent_lead: datagram.channel_parent_lead,
                                data: Some(entry.asm_buffer.finalize()),
                            });
                        }
                        _ => panic!()
                    }
                } else {
                    return None;
                }
            }
        }
    }

    pub fn clear(&mut self, idx: usize) {
        let ref mut window_entry = self.window[idx];

        match window_entry {
            WindowEntry::Open => {
            }
            WindowEntry::Closed(ref alloc_size) => {
                self.alloc -= alloc_size;
            }
            WindowEntry::Active(entry) => {
                self.alloc -= entry.alloc_size;
            }
        }

        self.window[idx] = WindowEntry::Open;
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::Datagram;
    use crate::frame::FragmentId;

    use super::Packet;
    use super::AssemblyWindow;
    use super::MAX_FRAGMENT_SIZE;

    use rand;

    #[test]
    fn single_packet_single_fragment() {
        let packet_size = 100;

        let mut window = AssemblyWindow::new(10000);

        let packet_data = (0..packet_size).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet_data.clone(),
        });

        assert_eq!(result.unwrap(), Packet {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            data: Some(packet_data),
        });
    }

    #[test]
    fn single_packet_multi_fragment() {
        let mut window = AssemblyWindow::new(10000);

        let packet_data = (0..MAX_FRAGMENT_SIZE*5).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        for i in 0 .. 5 {
            let fragment_data = packet_data[i * MAX_FRAGMENT_SIZE .. (i + 1) * MAX_FRAGMENT_SIZE].into();

            let result = window.try_add(0, Datagram {
                sequence_id: 0,
                channel_id: 0,
                window_parent_lead: 0,
                channel_parent_lead: 0,
                fragment_id: FragmentId { id: i as u16, last: 4 },
                data: fragment_data,
            });

            if i == 4 {
                assert_eq!(result.unwrap(), Packet {
                    sequence_id: 0,
                    channel_id: 0,
                    window_parent_lead: 0,
                    channel_parent_lead: 0,
                    data: Some(packet_data),
                });

                break;
            } else {
                assert_eq!(result, None);
            }
        }
    }

    #[test]
    fn alloc_exceeded() {
        let packet_0_size = 100;

        let mut window = AssemblyWindow::new(packet_0_size);

        let packet_0_data = (0..packet_0_size).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();
        let packet_2_data = (0..MAX_FRAGMENT_SIZE*2).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        // Receive a packet which maxes out the allocation counter
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet_0_data.clone(),
        });

        assert_eq!(result.unwrap(), Packet {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            data: Some(packet_0_data.clone()),
        });

        // A zero length packet should not trip the memory allocation counter
        let result = window.try_add(1, Datagram {
            sequence_id: 1,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: vec![].into_boxed_slice(),
        });

        assert_eq!(result.unwrap(), Packet {
            sequence_id: 1,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            data: Some(vec![].into_boxed_slice()),
        });

        // A nonzero length packet should produce a dud on first fragment, and no response to
        // remaining fragments.
        let result = window.try_add(2, Datagram {
            sequence_id: 2,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: packet_2_data[ .. MAX_FRAGMENT_SIZE].into(),
        });

        assert_eq!(result.unwrap(), Packet {
            sequence_id: 2,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            data: None,
        });

        let result = window.try_add(2, Datagram {
            sequence_id: 2,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_2_data[MAX_FRAGMENT_SIZE .. ].into(),
        });

        assert_eq!(result, None);

        // Clearing entries should allow future packets to be received correctly
        window.clear(0);
        window.clear(1);
        window.clear(2);

        let result = window.try_add(3, Datagram {
            sequence_id: 3,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet_0_data.clone(),
        });

        assert_eq!(result.unwrap(), Packet {
            sequence_id: 3,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            data: Some(packet_0_data.clone()),
        });
    }

    #[test]
    fn alloc_counter() {
        use super::packet_alloc_size;
        use std::collections::VecDeque;

        let packet_num = 100;

        let mut window = AssemblyWindow::new(packet_num*4*MAX_FRAGMENT_SIZE);

        let mut alloc_sum = 0;

        let mut alloc_sizes = VecDeque::new();

        for i in 0 .. 100 {
            let packet_size = if rand::random() {
                rand::random::<usize>() % MAX_FRAGMENT_SIZE
            } else {
                (rand::random::<usize>() % (3*MAX_FRAGMENT_SIZE)) + MAX_FRAGMENT_SIZE
            };

            let fragment_0_data = (0..packet_size.min(MAX_FRAGMENT_SIZE)).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();
            let num_fragments = (packet_size + MAX_FRAGMENT_SIZE - 1) / MAX_FRAGMENT_SIZE + (packet_size == 0) as usize;

            let datagram_0 = Datagram {
                sequence_id: i,
                channel_id: 0,
                window_parent_lead: 0,
                channel_parent_lead: 0,
                fragment_id: FragmentId { id: 0, last: (num_fragments - 1) as u16 },
                data: fragment_0_data,
            };

            let alloc_size = packet_alloc_size(&datagram_0);
            alloc_sum += alloc_size;
            alloc_sizes.push_back(alloc_size);

            window.try_add(i as usize, datagram_0);

            assert_eq!(window.alloc, alloc_sum);
        }

        for i in 0 .. 100 {
            alloc_sizes.pop_front();

            window.clear(i);

            assert_eq!(window.alloc, alloc_sizes.iter().sum());
        }
    }

    /*
    #[test]
    fn invalid_datagrams() {
        use super::MAX_PACKET_WINDOW_SIZE;

        let mut window = AssemblyWindow::new(10000);

        let packet_size = 100;
        let packet_data = (0..packet_size).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        // Beyond receive window
        let result = window.try_add(0, Datagram {
            sequence_id: MAX_PACKET_WINDOW_SIZE,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet_data.clone(),
        });

        assert_eq!(result, None);

        // Window parent behind channel parent
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 2,
            channel_parent_lead: 1,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet_data.clone(),
        });

        assert_eq!(result, None);

        // Window parent unset, channel parent set
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 1,
            fragment_id: FragmentId { id: 0, last: 0 },
            data: packet_data.clone(),
        });

        assert_eq!(result, None);

        // Bad fragment id
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 0 },
            data: packet_data.clone(),
        });

        assert_eq!(result, None);

        let packet_data = (0..MAX_FRAGMENT_SIZE + MAX_FRAGMENT_SIZE/2).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        // Last fragment too large
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data.clone(),
        });

        assert_eq!(result, None);

        // OK last fragment
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        });

        assert_eq!(result, None);

        // Bad fragment id
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 2, last: 1 },
            data: packet_data[ .. MAX_FRAGMENT_SIZE].into(),
        });

        assert_eq!(result, None);

        // Bad fragment size
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: packet_data[ .. MAX_FRAGMENT_SIZE-1].into(),
        });

        assert_eq!(result, None);

        // Successful receipt
        let result = window.try_add(Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: packet_data[ .. MAX_FRAGMENT_SIZE].into(),
        });

        assert_eq!(result.unwrap(), Packet {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            data: Some(packet_data),
        });
    }
    */

    #[test]
    fn inconsistent_datagrams() {
        let mut window = AssemblyWindow::new(10000);

        let packet_data = (0..2*MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        // Initial fragment
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: packet_data[ .. MAX_FRAGMENT_SIZE].into(),
        });

        assert_eq!(result, None);

        // Duplicate fragment
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 0, last: 1 },
            data: packet_data[ .. MAX_FRAGMENT_SIZE].into(),
        });

        assert_eq!(result, None);

        // Different channel ID
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 1,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        });

        assert_eq!(result, None);

        // Different window parent
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 1,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        });

        assert_eq!(result, None);

        // Different channel parent
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 1,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        });

        assert_eq!(result, None);

        // Different fragment count
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 2 },
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        });

        assert_eq!(result, None);

        // Successful receipt
        let result = window.try_add(0, Datagram {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            fragment_id: FragmentId { id: 1, last: 1 },
            data: packet_data[MAX_FRAGMENT_SIZE .. ].into(),
        });

        assert_eq!(result.unwrap(), Packet {
            sequence_id: 0,
            channel_id: 0,
            window_parent_lead: 0,
            channel_parent_lead: 0,
            data: Some(packet_data),
        });
    }
}

