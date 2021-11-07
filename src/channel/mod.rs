
mod tx;
mod rx;
mod seq;

// Maximum transfer window size, in units of sequence ids
// Must be a power of two
const TRANSFER_WINDOW_SIZE: u32 = 65536;

// Spacing for both window acknowledgements and sentinel datagrams
// Must be an integer divisor of TRANSFER_WINDOW_SIZE
const WINDOW_ACK_SPACING: u32 = TRANSFER_WINDOW_SIZE / 32;

// The largest permitted size for a given fragment
const FRAGMENT_SIZE: usize = super::MTU - super::frame::Data::HEADER_SIZE_BYTES - super::frame::DataEntry::FRAGMENT_HEADER_SIZE_BYTES;

// The largest possible size of a fragmented packet
pub const MAX_PACKET_SIZE: usize = 65536*FRAGMENT_SIZE;

type SendMode = super::SendMode;

type Fragment = super::frame::Fragment;
type Payload = super::frame::Payload;
type Datagram = super::frame::Datagram;
type WindowAck = super::frame::WindowAck;

pub struct Channel {
    pub tx: tx::Tx,
    pub rx: rx::Rx,
}

impl Channel {
    pub fn new(max_packet_size: usize) -> Self {
        Self {
            tx: tx::Tx::new(max_packet_size),
            rx: rx::Rx::new(max_packet_size),
        }
    }
}

#[test]
fn test_transmission() {
    let mut tx = tx::Tx::new(MAX_PACKET_SIZE);
    let mut rx = rx::Rx::new(MAX_PACKET_SIZE);

    let min_packet_size: usize = 0;
    let max_packet_size: usize = 5000;
    let packet_num = 2*TRANSFER_WINDOW_SIZE;

    for i in 0..packet_num {
        let packet_size = if i % 128 == 0 {
            0
        } else {
            min_packet_size + rand::random::<usize>() % (max_packet_size - min_packet_size)
        };

        let packet = (0..packet_size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice();

        tx.enqueue(packet.clone(), SendMode::Unreliable);

        while let Some((dg, _)) = tx.try_send() {
            rx.handle_datagram(dg);
        }

        assert_eq!(rx.receive().unwrap(), packet);
        assert_eq!(rx.receive(), None);

        if let Some(ack) = rx.take_window_ack() {
            tx.handle_window_ack(ack);
        }
    }
}

