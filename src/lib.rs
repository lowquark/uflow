
#![warn(missing_docs)]

//! `uflow` is a non-blocking, connection-based layer over UDP that provides an ordered and
//! drop-tolerant packet streaming interface for real-time applications. It manages connection
//! state, packet sequencing, packet fragmentation, reliable delivery, and congestion control to
//! create a simple and robust solution for low-latency internet communication.
//!
//! # Hosting a Server
//!
//! A `uflow` server is created by calling [`Server::bind[...]()`](Server::bind), which opens a UDP
//! socket bound to the specified address, and returns a corresponding `Server` object. The number
//! of active connections will be restricted to the provided limit, and each incoming connection
//! will be initialized using the given configuration (see: [`EndpointConfig`]).
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
//! As a non-blocking interface, a server depends on periodic calls to [`Server::step()`] in
//! order to handle inbound traffic and update connection states accordingly. In addition,
//! [`Server::flush()`] must be called in order to send any pending outbound data on the network.
//!
//! Once the server has received a valid connection request, a [`Peer`] object will be created to
//! represent the connection. New `Peer` objects may be obtained by calling
//! [`Server::incoming()`](Server::incoming), and stored wherever is most convenient.
//!
//! A `Peer` object functions as a handle for a given connection, allowing the application to
//! transfer data to/from a particular host using a single object. However, sent packets will not
//! be placed on the network until the next call to `Server::flush()`, and new events cannot be
//! received until the next call to `Server::step()`.
//!
//! A basic server loop that extends the above example is shown below:
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
//!     // Process inbound UDP frames
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
//!         // peer.poll_events()
//!
//!         // ...
//!
//!         // peer.send(...)
//!     }
//!
//!     // Flush outbound UDP frames
//!     server.flush();
//!
//!     // Remove disconnected peers
//!     peer_list.retain(|peer| !peer.is_disconnected());
//!
//!     // Sleep for 30ms (≈33 updates/second)
//!     std::thread::sleep(std::time::Duration::from_millis(30));
//! #   break;
//! }
//! ```
//!
//! See the `echo_server` example for a complete server implementation.
//!
//! # Connecting to a Server
//!
//! A `uflow` client is created by calling [`Client::bind[...]()`](Client::bind), which opens a UDP
//! socket bound to the specified address, and returns a corresponding `Client` object. Unlike
//! server creation, no connection parameters are provided when the `Client` object is created, and
//! there is no maximum number of simultaneous connections.
//!
//! ```
//! // Create a client object on any IPv4 address/port
//! let mut client = uflow::Client::bind_any_ipv4()
//!     .expect("Failed to bind/configure socket");
//! ```
//!
//! A connection is then initiated by calling [`connect()`](Client::connect), which requires the
//! address of the remote host, and the connection parameters to use. Assuming the provided address
//! could be resolved, a new [`Peer`] object representing the connection will be returned.
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
//! Like a server, a client depends on periodic calls to [`Client::step()`], and
//! [`Client::flush()`] in order to exchange data and update connection states. Once a connection
//! has been established, the corresponding `Peer` will generate a [`Connect`](Event::Connect)
//! event. A basic client loop extending the above example is shown below:
//!
//! ```
//! # let mut client = uflow::Client::bind_any_ipv4()
//! #     .expect("Failed to bind/configure socket");
//! # let server_address = "127.0.0.1:8888";
//! # let peer_config = uflow::EndpointConfig::default();
//! # let mut server_peer = client.connect(server_address, peer_config)
//! #     .expect("Invalid address");
//! loop {
//!     // Process inbound UDP frames
//!     client.step();
//!
//!     // Handle events
//!     for event in server_peer.poll_events() {
//!         match event {
//!             uflow::Event::Connect => {
//!                 // Connected to server!
//!                 // ...
//!             }
//!             _ => (),
//!         }
//!     }
//!
//!     // Send data, update client application state
//!     // ...
//!
//!     // Flush outbound UDP frames
//!     client.flush();
//!
//!     // Sleep for 30ms (≈33 updates/second)
//!     std::thread::sleep(std::time::Duration::from_millis(30));
//! #   break;
//! }
//! ```
//!
//! See the `echo_client` example for a complete client implementation.
//!
//! # Sending Packets
//!
//! Packets are sent to a remote host by calling [`Peer::send()`], which additionally requires a
//! channel ID and a packet send mode. Packets which are sent prior to establishing a connection
//! will remain queued until the connection succeeds.
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
//! ##### Packet Fragmentation and Aggregation
//!
//! All packets are aggregated into larger UDP frames, and are first automatically divided into
//! fragments if their size exceeds [`MAX_FRAGMENT_SIZE`]. Each fragment of a large packet is
//! transferred with the same send mode as the packet itself—that is, fragments will be resent if
//! and only if the packet is marked with [`SendMode::Persistent`] or [`SendMode::Reliable`]. Once
//! all fragments have been received, the original packet will have been recreated, and the packet
//! is considered received.
//!
//! ##### Channels
//!
//! Each connection contains 64 virtual channels that are used to ensure relative packet ordering:
//! packets that are received on a given channel will be delivered to the receiving application in
//! the order they were sent. Packets which are not yet received may be skipped, depending on the
//! send mode of the particular packet, and whether or not any subsequent packets have been
//! received.
//!
//! Because packets that are sent using [`SendMode::Reliable`] may not be skipped, and because all
//! packets on a given channel must be delivered in-order, the receiving application will not see a
//! given received packet until all previous reliable packets on the same channel have also been
//! received. This means that if a reliable packet is dropped, its channel will effectively stall
//! for its arrival, but packets received on other channels may still be delivered in the meantime.
//!
//! Thus, by carefully choosing the send mode and channel of outgoing packets, the latency effects
//! of intermittent network losses can be mitigated. Because `uflow` does not store packets by
//! channel, and otherwise never iterates over the space of channel IDs, there is no penalty to
//! using a large number of channels.
//!
//! ##### Packet Buffering
//!
//! All packets are sent subject to adaptive rate control, a maximum transfer window, and a memory
//! limit set by the receiving host. If any of these mechanisms prevent a packet from being sent,
//! the packet will remain in a queue at the sender. Thus, a sender can expect that packets will
//! begin to accumulate in its queue if the connection bandwidth is low, or if the receiver is not
//! processing packets quickly enough.
//!
//! The total size of all pending packet data can be obtained by calling
//! [`Peer::pending_send_size()`](Peer::pending_send_size), and if desired, an application can use
//! this value to terminate excessively delayed connections. In addition, the application may send
//! packets using [`SendMode::TimeSensitive`] to drop packets at the sender if they could not be
//! sent immediately (i.e. during the next call to `flush()`). In the event that the total
//! available bandwidth is limited, this prevents outdated packets from using any unnecessary
//! bandwidth, and prioritizes sending newer packets in the queue.
//!
//! # Receiving Packets (and Other Events)
//!
//! Connection status updates and received packets are delivered to the application by calling
//! [`Peer::poll_events()`](peer::Peer::poll_events), which returns all pending events via
//! iterator:
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
//! A [`Connect`](Event::Connect) event will be generated by a `Peer` object when a connection is
//! first established. If either end of the connection explicitly disconnects, a
//! [`Disconnect`](Event::Disconnect) event will be generated. Once a packet has been received in
//! full (and that packet is not waiting for any previous packets), a [`Receive`](Event::Receive)
//! event will be generated. If a connection times out at any point, a [`Timeout`](Event::Timeout)
//! event will be generated. The last event generated by a `Peer` object will always be a
//! `Timeout`, even after a connection has closed normally.
//!
//! ##### Maximum Receive Allocation
//!
//! If the sender is sends a continuous stream of packets, but `poll_events()` is not called for
//! whatever reason, the number of packets in the `Peer`'s receive buffer will stop increasing once
//! its [maximum receive allocation](EndpointConfig#structfield.max_receive_alloc) has been
//! reached. At that point, any new packets will be silently ignored.
//!
//! *Note*: This feature is intended to guard against memory allocation attacks. A well-behaved
//! sender will ensure that it does not send new packets exceeding the receiver's memory limit, and
//! the stall will back-propagate accordingly.
//!
//! ##### Optimal Acknowledgements
//!
//! If desired, the sender may call `flush()` on the associated client or server object immediately
//! after all events have been handled. By doing so, information relating to which packets have
//! been delivered (and how much buffer space is now available) will be relayed to the sender as
//! soon as possible. (If `flush()` was called prior to handling events, the resulting
//! acknowledgements would contain slightly outdated information—from a synchronization standpoint,
//! this would be sub-optimal but non-fatal.)
//!
//! # Disconnecting
//!
//! A connection is explicitly closed by calling [`Peer::disconnect()`], which makes an effort to
//! notify the remote host of the disconnection, and to send all pending outbound packets before
//! doing so. The sender can expect that any pending reliable packets will be delivered prior to
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
//! // ... calls to step() & flush() continue
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
mod packet_id;
mod peer;
mod server;
mod udp_frame_sink;

pub use server::Server;
pub use client::Client;
pub use peer::Peer;
pub use endpoint::Config as EndpointConfig;

/// The current protocol version ID.
pub const PROTOCOL_VERSION: u8 = 2;

/// The maximum number of channels which may be used on a given connection.
pub const CHANNEL_COUNT: usize = frame::serial::MAX_CHANNELS;

/// The maximum size of the frame transfer window, in sequence IDs.
pub const MAX_FRAME_WINDOW_SIZE: u32 = 4096;

/// The maximum size of the packet transfer window, in sequence IDs.
pub const MAX_PACKET_WINDOW_SIZE: u32 = 4096;

/// The common maximum transfer unit (MTU) of the internet.
pub const INTERNET_MTU: usize = 1500;

/// The number of bytes in a UDP header (including the IP header).
pub const UDP_HEADER_SIZE: usize = 28;

/// The maximum size of a `uflow` frame in bytes, according to the internet MTU and UDP header
/// size.
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
    /// [`Server::flush`](Server::flush)), it will be discarded instead of being delayed. If this
    /// packet is dropped, or a subsequent packet arrives on the same channel before this one does,
    /// the receiver may skip this packet.
    TimeSensitive,
    /// This packet will be sent exactly once. If this packet is dropped, or a subsequent packet
    /// arrives on the same channel before this one does, the receiver may skip this packet.
    Unreliable,
    /// This packet will be sent and resent until acknowledged by the receiver. If a subsequent
    /// packet arrives on the same channel before this one does, the receiver may skip this packet.
    /// (The packet will cease to be resent once the sender has detected a skip.)
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
    /// Indicates that a packet has been received from the remote host.
    Receive(
        /// The received packet.
        Box<[u8]>,
    ),
}

