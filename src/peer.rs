
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
    /// # Panics
    ///
    /// This function will panic if `channel_id` does not refer to a valid channel, or if
    /// `data.len()` exceeds the [maximum packet
    /// size](endpoint::Config#structfield.max_packet_size).
    pub fn send(&mut self, data: Box<[u8]>, channel_id: usize, mode: SendMode) {
        self.endpoint_ref.borrow_mut().send(data, channel_id, mode);
    }

    /// Delivers all available events for this connection. See [`Event`](Event) for a list of
    /// possible event types.
    ///
    /// *Note 1*: All events are considered delivered even if the iterator is not consumed until
    /// the end.
    ///
    /// *Note 2*: Packets that have been received will not be acknowledged until this function is
    /// called. The number of packets that may be queued prior to being delivered is limited by a
    /// maximum transfer window, and by the [maximum receive
    /// allocation](endpoint::Config#structfield.max_receive_alloc).
    pub fn poll_events(&mut self) -> impl Iterator<Item = Event> {
        self.endpoint_ref.borrow_mut().poll_events()
    }

    /// Terminates the connection, notifying the remote host. If currently connected, all pending
    /// packets will be sent prior to disconnecting.
    ///
    /// A [`Disconnect`](Event::Disconnect) event will be generated if a connection was previously
    /// established.
    pub fn disconnect(&self) {
        self.endpoint_ref.borrow_mut().disconnect();
    }

    // TODO: disconnect_noflush()?

    /// Immediately terminates the connection, without notifying the remote host.
    ///
    /// A [`Disconnect`](Event::Disconnect) event will be generated if a connection was previously
    /// established.
    pub fn disconnect_now(&self) {
        self.endpoint_ref.borrow_mut().disconnect_now();
    }

    /// Returns the address of the remote host.
    pub fn address(&self) -> net::SocketAddr {
        self.address
    }

    /// Returns the current round-trip time estimate (RTT), in seconds.
    ///
    /// If an RTT estimate has not yet been computed, `None` is returned instead.
    pub fn rtt_s(&self) -> Option<f64> {
        self.endpoint_ref.borrow().rtt_s()
    }

    /// Returns `true` if the connection has been terminated.
    pub fn is_disconnected(&self) -> bool {
        let endpoint_ref = self.endpoint_ref.borrow();
        return endpoint_ref.is_zombie() || endpoint_ref.is_disconnected();
    }

    pub(super) fn is_zombie(&self) -> bool {
        let endpoint_ref = self.endpoint_ref.borrow();
        return endpoint_ref.is_zombie();
    }
}

