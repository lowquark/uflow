use std::net;

use crate::endpoint::daten_meister::DatenMeister;

pub struct PendingState {
    pub local_nonce: u32,
    pub remote_nonce: u32,
    pub remote_max_receive_rate: u32,
    pub remote_max_packet_size: u32,
    pub remote_max_receive_alloc: u32,
    pub reply_bytes: Box<[u8]>,
}

pub struct ActiveState {
    pub endpoint: DatenMeister,
    pub disconnect_flush: bool,
    pub timeout_time_ms: u64,
}

pub enum State {
    Pending(PendingState),
    Active(ActiveState),
    Closing,
    Closed,
}

pub struct Peer {
    pub address: net::SocketAddr,
    pub state: State,
}

impl Peer {
    pub (super) fn new(address: net::SocketAddr, state: PendingState) -> Self {
        Self {
            address,
            state: State::Pending(state),
        }
    }

    pub fn is_active(&self) -> bool {
        match self.state {
            State::Active(_) => true,
            _ => false,
        }
    }
}
