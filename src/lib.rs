
#![warn(missing_docs)]

//! `uflow` is a non-blocking, connection-based layer over UDP that provides a drop-tolerant packet
//! streaming interface, designed primarily for use in real-time, multiplayer games. It manages
//! connection state, packet sequencing, fragmentation/reassembly, reliable delivery, and
//! congestion control to produce a simple and robust data link for real-time applications.
//!
//! # Hosting a Server
//!
//! A `uflow` server is created by calling [`Server::bind()`], which opens a non-blocking UDP
//! socket and returns a [`Server`] object according to the provided parameters. The number of
//! active connections will be restricted to the specified limit, and each connection will be
//! initialized according to the provided [`EndpointConfig`].
//!
//! ```
//! let server_address = "127.0.0.1:8888";
//! let max_peer_count = 8;
//! let peer_config = uflow::EndpointConfig::default();
//!
//! let mut server = uflow::Server::bind(server_address, max_peer_count, peer_config)
//!     .expect("Failed to bind/configure socket");
//! ```
//!
//! As a non-blocking socket interface, `Server` objects depend on periodic calls to
//! [`step()`](Server::step) to update connection states and exchange connection metadata. Once a
//! connection request has been received, a [`Peer`] object will be created to represent the
//! inbound connection. Newly connected peers may be retrieved by calling
//! [`incoming()`](Server::incoming()), and all pending outbound data may be flushed by calling
//! [`flush()`](Server::flush).
//!
//! An example server update loop that extends the above example is shown below:
//!
//! ```
//! # let server_address = "127.0.0.1:8888";
//! # let max_peer_count = 8;
//! # let peer_config = uflow::EndpointConfig::default();
//! # let mut server = uflow::Server::bind(server_address, max_peer_count, peer_config)
//! #     .expect("Failed to bind/configure socket");
//! let mut peer_list = Vec::new();
//!
//! loop {
//!     // Process received UDP frames
//!     server.step();
//!
//!     // Handle new connections
//!     for new_peer in server.incoming() {
//!         println!("New connection from {}", new_peer.address());
//!         peer_list.push(new_peer);
//!     }
//!
//!     // Send/receive data, update server application state
//!     for peer in peer_list.iter() {
//!         // ...
//!     }
//!
//!     // Send new outbound data
//!     server.flush();
//!
//!     // Forget disconnected peers
//!     peer_list.retain(|peer| !peer.is_disconnected());
//!
//!     // Sleep for 30ms (≈33.3 steps/second)
//!     std::thread::sleep(std::time::Duration::from_millis(30));
//! #   break;
//! }
//! ```
//!
//! See the `echo_server` example for a complete server implementation.
//!
//! # Connecting to a Server
//!
//! A `uflow` client is created by calling [`Client::bind()`](Client::bind), which opens a
//! non-blocking UDP socket and returns a [`Client`] object according to the specified address.
//! Unlike creating a server, no endpoint parameters are specified when the `Client` object is
//! created, and there is no maximum number of simultaneous connections.
//!
//! ```
//! // Create a client object on any IPv4 address/port
//! let mut client = uflow::Client::bind_any_ipv4()
//!     .expect("Failed to bind/configure socket");
//! ```
//!
//! A connection is then initiated by calling [`connect()`](Client::connect), which takes the
//! address of the remote host, and the connection parameters to use. Assuming the provided address
//! could be resolved, a [`Peer`] object representing the connection will be returned.
//!
//! ```
//! # let mut client = uflow::Client::bind_any_ipv4()
//! #     .expect("Failed to bind/configure socket");
//! let server_address = "127.0.0.1:8888";
//! let peer_config = uflow::EndpointConfig::default();
//!
//! let mut server_peer = client.connect(server_address, peer_config)
//!     .expect("Invalid address");
//! ```
//!
//! Like a server, a client depends on periodic calls to [`step()`](Client::step) for connection
//! management, and [`flush()`](Client::flush) may be used to send all pending outbound data. An
//! example client update loop extending the above example is shown below:
//!
//! ```
//! # let mut client = uflow::Client::bind_any_ipv4()
//! #     .expect("Failed to bind/configure socket");
//! # let server_address = "127.0.0.1:8888";
//! # let peer_config = uflow::EndpointConfig::default();
//! # let mut server_peer = client.connect(server_address, peer_config)
//! #     .expect("Invalid address");
//! loop {
//!     // Process received UDP frames
//!     client.step();
//!
//!     // Send/receive data, update client application state
//!     // ...
//!
//!     // Send new outbound data
//!     client.flush();
//!
//!     // Sleep for 30ms (≈33.3 steps/second)
//!     std::thread::sleep(std::time::Duration::from_millis(30));
//! #   break;
//! }
//! ```
//!
//! See the `echo_client` example for a complete client implementation.
//!
//! # Sending Data
//!
//! Packets sent to a remote host by calling [`Peer::send()`], which takes a channel ID and a
//! [`SendMode`]. All packets sent on a given channel are guaranteed to be received in the order
//! they were sent, and each packet will be delivered according to the specified mode.  Packets
//! which are sent prior to establishing a connection will be sent once the connection succeeds.
//!
//! ```
//! # let mut client = uflow::Client::bind_any_ipv4()
//! #     .expect("Failed to bind/configure socket");
//! # let server_address = "127.0.0.1:8888";
//! # let peer_config = uflow::EndpointConfig::default();
//! # let mut peer = client.connect(server_address, peer_config)
//! #     .expect("Invalid address");
//! let packet_data = "Hello world!".as_bytes();
//! let channel_id = 0;
//! let send_mode = uflow::SendMode::Reliable;
//!
//! peer.send(packet_data.into(), channel_id, send_mode);
//! ```
//!
//! *Note 1*: All packets are aggregated into UDP frames, and fragmented if they are larger than
//! [`MAX_FRAGMENT_SIZE`]. Packets will not be placed on the network until `step()` or `flush()`
//! is called on the associated client or server object. A future method call may permit flushing a
//! single connection immediately.
//!
//! *Note 2*: Because packets that are sent using [`SendMode::Reliable`] may not be skipped, the
//! receiver will not deliver received packets for a given channel until all previous reliable
//! packets on that channel have arrived. If a reliable packet is dropped, its channel will stall
//! for its arrival, but packets received on other channels may still be delivered in the meantime.
//! Careful selection of channel and send mode can thus mitigate the effects of short-term network
//! congestion.
//!
//! *Note 3*: All packets are sent subject to adaptive rate control, a memory limit set by the
//! receiving host, and a maximum transfer window of 4096 outstanding packets. If any of these
//! mechanisms prevent a packet's delivery, the packet will remain in a send queue. (Packets sent
//! using [`SendMode::TimeSensitive`] are an exception to this.) Thus, a sender can expect that
//! packets will begin to accumulate in its send queue if the connection bandwidth is low, or if
//! the receiver has stopped retrieving received packets (i.e. making calls to
//! [`poll_events()`](Peer::poll_events)). A future method call will return the combined size of
//! all pending outbound packets.
//!
//! # Connection Events and Receiving Data
//!
//! Connection events are delivered to the application by calling
//! [`Peer::poll_events()`](peer::Peer::poll_events) on a [`Peer`] object. Each
//! pending [`Event`] is returned via iterator.
//!
//! ```
//! # let mut client = uflow::Client::bind_any_ipv4()
//! #     .expect("Failed to bind/configure socket");
//! # let server_address = "127.0.0.1:8888";
//! # let peer_config = uflow::EndpointConfig::default();
//! # let mut peer = client.connect(server_address, peer_config)
//! #     .expect("Invalid address");
//! for event in peer.poll_events() {
//!     match event {
//!         uflow::Event::Connect => {
//!         }
//!         uflow::Event::Disconnect => {
//!         }
//!         uflow::Event::Timeout => {
//!         }
//!         uflow::Event::Receive(packet_data) => {
//!         }
//!     }
//! }
//! ```
//!
//! A [`Connect`](Event::Connect) event is generated by a `Peer` object when a connection has been
//! established. If either end of the connection explicitly disconnects, a
//! [`Disconnect`](Event::Disconnect) event will be generated. If a connection times out at any
//! point, a [`Timeout`](Event::Timeout) event will be generated. A [`Receive`](Event::Receive)
//! event containing packet data is generated for each received (deliverable) packet.
//!
//! *Note 1*: The last event generated by a `Peer` object will always be a `Timeout` event, even
//! after a connection has closed normally.
//!
//! *Note 2*: Calls to `poll_events()` will free allocation space in the receive buffer, thus
//! allowing new packets to be received. A receiver can expect that if `poll_events()` is not
//! called for whatever reason, that the number of packets in the receive buffer will be limited
//! according to the [maximum receive allocation](EndpointConfig#structfield.max_receive_alloc).
//!
//! # Disconnecting
//!
//! A connection is explicitly closed by calling [`Peer::disconnect()`], which notifies the remote
//! host of the disconnection, and sends all pending outbound packets before doing so. The sender
//! can expect that any currently pending reliable packets will be delivered prior to
//! disconnecting, provided that the remote host doesn't also disconnect in the meantime.
//!
//! ```
//! # let mut client = uflow::Client::bind_any_ipv4()
//! #     .expect("Failed to bind/configure socket");
//! # let server_address = "127.0.0.1:8888";
//! # let peer_config = uflow::EndpointConfig::default();
//! # let mut peer = client.connect(server_address, peer_config)
//! #     .expect("Invalid address");
//! peer.disconnect();
//!
//! // ... calls to step() continue
//! ```
//!
//! Alternatively, one may call [`Peer::disconnect_now()`], which sends no further packets and
//! disconnects immediately. Because no notification is sent, this will cause a timeout on the
//! remote host.
//!
//! ```
//! # let mut client = uflow::Client::bind_any_ipv4()
//! #     .expect("Failed to bind/configure socket");
//! # let server_address = "127.0.0.1:8888";
//! # let peer_config = uflow::EndpointConfig::default();
//! # let mut peer = client.connect(server_address, peer_config)
//! #     .expect("Invalid address");
//! peer.disconnect_now();
//!
//! assert!(peer.is_disconnected());
//! ```
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

/// The maximum size of the frame transfer window, in sequence IDs.
pub const MAX_FRAME_WINDOW_SIZE: u32 = 4096;

/// The maximum size of the packet transfer window, in sequence IDs.
pub const MAX_PACKET_WINDOW_SIZE: u32 = 4096;

/// The maximum transfer unit (MTU) of the internet.
pub const INTERNET_MTU: usize = 1500;

/// The number of header bytes of a UDP packet (including the IP header).
pub const UDP_HEADER_SIZE: usize = 28;

/// The maximum size of a `uflow` frame in bytes, according to the internet MTU (1500 bytes) and
/// UDP header size (28 bytes).
pub const MAX_FRAME_SIZE: usize = INTERNET_MTU - UDP_HEADER_SIZE;

/// The maximum size of a packet fragment in bytes, according to frame serialization overhead.
pub const MAX_FRAGMENT_SIZE: usize = MAX_FRAME_SIZE - frame::serial::DATA_FRAME_OVERHEAD - frame::serial::MAX_DATAGRAM_OVERHEAD;

/// The absolute maximum size of a packet, in bytes.
pub const MAX_PACKET_SIZE: usize = MAX_FRAGMENT_SIZE * frame::serial::MAX_FRAGMENTS;

/// A mode by which a packet is sent.
#[derive(Clone,Copy,Debug,PartialEq)]
pub enum SendMode {
    /// This packet will be sent at most once. If this packet cannot be sent immediately (i.e.
    /// during the next call to [`Client::flush`](Client::flush) or
    /// [`Server::flush`](Server::flush)), it will be discarded rather than remain in a send queue.
    /// If this packet is dropped, or a subsequent packet arrives on the same channel before this
    /// one does, the receiver may skip this packet.
    TimeSensitive,
    /// This packet will be sent exactly once. If this packet is dropped, or a subsequent packet
    /// arrives on the same channel before this one does, the receiver may skip this packet.
    Unreliable,
    /// This packet will be sent and resent until acknowledged by the receiver. If a subsequent
    /// packet arrives on the same channel before this one does, the receiver may skip this packet.
    /// (In general, the packet will cease to be resent once the sender has detected a skip.)
    Persistent,
    /// This packet will be sent until acknowledged by the receiver. The receiver will not deliver
    /// subsequent packets on the same channel until this packet has been received.
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
    /// Indicates a connection has timed out (i.e. no packets have been received from the remote
    /// host for some amount of time). No further events will be delivered.
    Timeout,
    /// Indicates a packet has been received from the remote host.
    Receive(
        /// The received packet.
        Box<[u8]>,
    ),
}

