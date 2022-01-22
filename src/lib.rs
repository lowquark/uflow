
#![warn(missing_docs)]

//! `uflow` is a connection-based layer over UDP that provides a loss-tolerant packet streaming
//! interface, designed primarily for use in real-time, multiplayer games. It manages connection
//! state, congestion control, sequencing, and packet fragmentation to produce a simple and robust
//! data link for real-time applications.
//!
//! # Creating a Connection
//!
//! # Sending Data
//!
//! # Closing a Connection
//!

mod client;
mod endpoint;
mod frame;
mod peer;
mod server;
mod udp_frame_sink;

pub use server::Server;
pub use client::Client;
pub use peer::Peer;
pub use endpoint::Config as EndpointConfig;

/// The current protocol version ID.
pub const PROTOCOL_VERSION: u8 = 0;

/// The maximum number of channels which may be used on a given connection.
pub const MAX_CHANNELS: usize = frame::serial::MAX_CHANNELS;

/// The maximum transfer unit (MTU) of the internet.
pub const INTERNET_MTU: usize = 1500;

/// The number of header bytes of a UDP packet (including the IP header).
pub const UDP_HEADER_SIZE: usize = 28;

/// The maximum size of a `uflow` frame in bytes, according to the internet MTU (1500 bytes) and
/// UDP header size (28 bytes).
pub const MAX_FRAME_SIZE: usize = INTERNET_MTU - UDP_HEADER_SIZE;

/// The maximum size of a packet fragment in bytes, according to frame serialization overhead.
pub const MAX_FRAGMENT_SIZE: usize = MAX_FRAME_SIZE - frame::serial::MAX_DATAGRAM_OVERHEAD - frame::serial::DATA_FRAME_OVERHEAD;

/// The absolute maximum size of a packet, in bytes.
pub const MAX_PACKET_SIZE: usize = MAX_FRAGMENT_SIZE * frame::serial::MAX_FRAGMENTS;

const MAX_PACKET_TRANSFER_WINDOW_SIZE: u32 = 4096;
const MAX_FRAME_TRANSFER_WINDOW_SIZE: u32 = 16384;

/// A mode with which a user packet is sent.
#[derive(Clone,Copy,Debug,PartialEq)]
pub enum SendMode {
    /// The packet will be sent at most once. If the packet cannot be sent immediately (i.e.
    /// during the next call to [`Client::flush`](Client::flush) or
    /// [`Server::flush`](Server::flush)), it will be discarded rather than remain in a send queue.
    /// If the packet is dropped, or a subsequent packet arrives on the same channel before it
    /// does, the receiver may skip this packet.
    TimeSensitive,
    /// The packet will be sent exactly once. If the packet is dropped, or a subsequent packet
    /// arrives on the same channel before it does, the receiver may skip this packet.
    Unreliable,
    /// The packet will be sent and resent until acknowledged by the receiver. If a subsequent
    /// packet arrives on the same channel before it does, the receiver may skip this packet. (In
    /// general, the packet will cease to be resent once the sender has detected a skip.)
    Resend,
    /// The packet will be sent until acknowledged by the receiver. The receiver will not deliver
    /// subsequent packets on the same channel until the packet has been received.
    Reliable,
}

/// An event produced by a [`Peer`](peer::Peer) object.
#[derive(Clone,Debug,PartialEq)]
pub enum Event {
    /// Indicates a successful connection to/from a remote host.
    Connect,
    /// Indicates a disconnection from the remote host. A disconnection event is only produced if
    /// the peer was previously connected, and either end explicitly terminates the connection.
    Disconnect,
    /// Indicates a packet has been received from the remote host.
    Receive(
        /// The received user packet.
        Box<[u8]>,
        /// The received user packet's channel ID.
        usize,
    ),
    /// Indicates a connection has timed out (i.e. no packets have been received from the remote
    /// host for some amount of time). No further events will be delivered.
    Timeout,
}

