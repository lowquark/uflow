
use crate::endpoint;
use crate::Event;
use crate::SendMode;

use std::net;
use std::rc::Rc;
use std::cell::RefCell;

/// An object representing a connection to a remote host.
///
/// # Connection States
///
pub struct Peer {
    address: net::SocketAddr,
    endpoint_ref: Rc<RefCell<endpoint::Endpoint>>,
}

impl Peer {
    pub(super) fn new(address: net::SocketAddr, endpoint_ref: Rc<RefCell<endpoint::Endpoint>>) -> Self {
        Self {
            address: address,
            endpoint_ref: endpoint_ref,
        }
    }

    /// Enqueues a packet for delivery to the remote host. The packet will be sent on the given
    /// channel according to the specified mode.
    ///
    /// # Error Handling
    ///
    /// This function will panic if `channel_id` does not refer to a valid channel (i.e.
    /// if `channel_id >= CHANNEL_COUNT`), or if `data.len()` exceeds the [maximum packet
    /// size](endpoint::Config#structfield.max_packet_size).
    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        self.endpoint_ref.borrow_mut().send(data, channel_id, mode);
    }

    /// Delivers all available events for this connection.
    ///
    /// *Note*: All events are considered delivered, even if the iterator is not consumed until the
    /// end.
    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        self.endpoint_ref.borrow_mut().poll_events()
    }

    /// Explicitly terminates the connection, notifying the remote host in the process.
    ///
    /// If the `Peer` is currently connected, all pending packets will be sent prior to
    /// disconnecting, and a [`Disconnect`](Event::Disconnect) event will be generated once the
    /// disconnection is complete.
    pub fn disconnect(&self) {
        self.endpoint_ref.borrow_mut().disconnect();
    }

    // TODO: disconnect_noflush()?

    /// Immediately terminates the connection, without notifying the remote host.
    ///
    /// If the `Peer` is currently connected, a [`Disconnect`](Event::Disconnect) event will be
    /// generated.
    pub fn disconnect_now(&self) {
        self.endpoint_ref.borrow_mut().disconnect_now();
    }

    /// Returns the socket address of the remote host.
    pub fn address(&self) -> net::SocketAddr {
        self.address
    }

    /// Returns the current estimate of the round-trip time (RTT), in seconds.
    ///
    /// If the RTT has not yet been computed, `None` is returned instead.
    pub fn rtt_s(&self) -> Option<f64> {
        self.endpoint_ref.borrow().rtt_s()
    }

    /// Returns the combined size of all outstanding packets (i.e. those which have not yet been
    /// acknowledged by the receiver), in bytes.
    ///
    /// This figure represents the amount of memory allocated by outgoing packets. Thus, packets
    /// which are marked [`Time-Sensitive`](SendMode::TimeSensitive) are included in this total,
    /// even if they would not be sent.
    pub fn send_buffer_size(&self) -> usize {
        self.endpoint_ref.borrow().send_buffer_size()
    }

    /// Returns `true` if the connection has been terminated or timed out.
    ///
    /// *Note*: Once disconnected, a `Peer` will never be reconnected.
    pub fn is_disconnected(&self) -> bool {
        let endpoint_ref = self.endpoint_ref.borrow();
        return endpoint_ref.is_zombie() || endpoint_ref.is_disconnected();
    }

    // Distinguishing zombie from disconnected is of no use to the application
    pub(super) fn is_zombie(&self) -> bool {
        let endpoint_ref = self.endpoint_ref.borrow();
        return endpoint_ref.is_zombie();
    }
}

