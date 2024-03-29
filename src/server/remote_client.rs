use std::net;

use crate::half_connection::HalfConnection;
use crate::SendMode;
use crate::CHANNEL_COUNT;

pub (super) enum DisconnectMode {
    Now,
    Flush,
}

pub (super) struct PendingState {
    pub local_nonce: u32,
    pub remote_nonce: u32,
    pub remote_max_receive_rate: u32,
    pub remote_max_receive_alloc: u32,
    pub reply_bytes: Box<[u8]>,
}

pub (super) struct ActiveState {
    pub half_connection: HalfConnection,
    pub timeout_time_ms: u64,
    pub disconnect_signal: Option<DisconnectMode>,
}

pub (super) enum State {
    Pending(PendingState),
    Active(ActiveState),
    Closing,
    Closed,
    Fin,
}

/// Used by a [`Server`](super::Server) object to represent a connected client.
pub struct RemoteClient {
    pub (super) address: net::SocketAddr,
    pub (super) state: State,
    pub (super) max_packet_size: usize,
}

impl RemoteClient {
    /// Returns `true` if the connection is active, that is, a connection handshake has been
    /// completed and the remote host has not yet timed out or disconnected. Returns `false`
    /// otherwise.
    pub fn is_active(&self) -> bool {
        match self.state {
            State::Active(_) => true,
            _ => false,
        }
    }

    /// Enqueues a packet for delivery to this client. The packet will be sent on the given channel
    /// according to the specified mode.
    ///
    /// If the connection is not active, the packet will be silently discarded.
    ///
    /// # Error Handling
    ///
    /// This function will panic if `channel_id` does not refer to a valid channel (i.e. if
    /// `channel_id >= CHANNEL_COUNT`), or if `data.len()` exceeds the [maximum packet
    /// size](crate::EndpointConfig#structfield.max_packet_size).
    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        assert!(data.len() <= self.max_packet_size,
                "send failed: packet of size {} exceeds configured maximum of {}",
                data.len(),
                self.max_packet_size);

        assert!(channel_id < CHANNEL_COUNT,
                "send failed: channel ID {} is invalid",
                channel_id);

        match self.state {
            State::Active(ref mut state) => {
                state.half_connection.send(data, channel_id as u8, mode);
            }
            _ => (),
        }
    }

    /// Gracefully terminates this connection once all packets have been sent.
    ///
    /// If any outbound packets are pending, they will be sent prior to disconnecting. Reliable
    /// packets can be assumed to have been delievered, so long as the client does not also
    /// disconnect in the meantime. The connection will remain active until the next call to
    /// [`Server::step()`](super::Server::step) with no pending outbound packets.
    pub fn disconnect(&mut self) {
        match self.state {
            State::Active(ref mut state) => {
                state.disconnect_signal = Some(DisconnectMode::Flush);
            }
            _ => (),
        }
    }

    /// Gracefully terminates this connection as soon as possible.
    ///
    /// If any outbound packets are pending, they may be flushed prior to disconnecting, but no
    /// packets are guaranteed to be received by the client. The connection will remain active
    /// until the next call to [`Server::step()`](super::Server::step).
    pub fn disconnect_now(&mut self) {
        match self.state {
            State::Active(ref mut state) => {
                state.disconnect_signal = Some(DisconnectMode::Now);
            }
            _ => (),
        }
    }

    /// Returns the current estimate of the round-trip time (RTT), in seconds.
    ///
    /// If the RTT has not yet been computed, or if the connection is not active, `None` is
    /// returned instead.
    pub fn rtt_s(&self) -> Option<f64> {
        match self.state {
            State::Active(ref state) => state.half_connection.rtt_s(),
            _ => None,
        }
    }

    /// Returns the total size of the send buffer (i.e. those packets which have not yet been
    /// acknowledged), in bytes.
    ///
    /// This figure represents the amount of memory allocated for outgoing packets. Packets which
    /// are marked [`TimeSensitive`](SendMode::TimeSensitive) are included in this total, even if
    /// they would not be sent.
    pub fn send_buffer_size(&self) -> usize {
        match self.state {
            State::Active(ref state) => state.half_connection.send_buffer_size(),
            _ => 0,
        }
    }
}
