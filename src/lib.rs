
#![warn(missing_docs)]

//! `uflow` is a non-blocking, connection-based layer over UDP that provides an ordered and
//! drop-tolerant packet streaming interface for real-time applications. It manages connection
//! state, packet sequencing, fragmentation/reassembly, reliable delivery, and congestion control
//! to create a simple and robust solution for low-latency internet communication.
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
//! [`step()`](Server::step) to process incoming UDP frames, and to update connection states
//! accordingly. Similarly, [`flush()`](Server::flush) must be called in order to send pending
//! outbound data (packets, acknowledgements, keepalives, and etc.). A basic server loop that
//! extends the above example is shown below:
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
//!         // ...
//!     }
//!
//!     // Flush outbound UDP frames
//!     server.flush();
//!
//!     // Remove disconnected peers
//!     peer_list.retain(|peer| !peer.is_disconnected());
//!
//!     // Sleep for 30ms (≈33.3 steps/second)
//!     std::thread::sleep(std::time::Duration::from_millis(30));
//! #   break;
//! }
//! ```
//!
//! #### Inbound Connections
//!
//! Once a remote host has connected to the server, a [`Peer`] object will be created to represent
//! the connection. Newly created `Peer` objects may be retrieved by calling
//! [`incoming()`](Server::incoming()), at which point they may be stored by the application
//! wherever is most convenient.
//!
//! #### Example
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
//! address of the remote host, and the endpoint parameters to use. Assuming the provided address
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
//! Like a server, a client depends on periodic calls to [`step()`](Client::step) and
//! [`flush()`](Client::flush) in order to exchange UDP frames and update connection states. A
//! basic client loop extending the above example is shown below:
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
//!     // Send/receive data, update client application state
//!     // ...
//!
//!     // Flush outbound UDP frames
//!     client.flush();
//!
//!     // Sleep for 30ms (≈33.3 steps/second)
//!     std::thread::sleep(std::time::Duration::from_millis(30));
//! #   break;
//! }
//! ```
//!
//! #### Example
//!
//! See the `echo_client` example for a complete client implementation.
//!
//! # Sending Data
//!
//! Packets are sent to a remote host by calling [`Peer::send()`], which additionally requires a
//! channel ID and a packet send mode. All packets received on a given channel are guaranteed to be
//! delivered in the order they were sent, and each packet will be transferred according to the
//! [specified mode](SendMode). Packets which are sent prior to establishing a connection will be
//! buffered until the connection succeeds.
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
//! ##### Fragmentation
//!
//! All packets are aggregated into UDP frames, and are first fragmented if their size exceeds
//! [`MAX_FRAGMENT_SIZE`]. No frames are sent until `flush()` is called on the associated client or
//! server object.
//!
//! ##### Reliable Packets & Channel Stalls
//!
//! Because packets that are sent using [`SendMode::Reliable`] may not be skipped, and because all
//! packets on a given channel must be delivered in-order, the application will not see received
//! packets until all previous reliable packets on the same channel have been received. This means
//! that if a reliable packet has been dropped, its channel will effectively stall for its arrival.
//! However, packets received on other channels may still be delivered in the meantime. The effects
//! of intermittent drops can otherwise be mitigated through careful selection of channel and send
//! mode.
//!
//! ##### Packet Buffering
//!
//! All packets are sent subject to adaptive rate control, a maximum transfer window, and a memory
//! limit set by the receiving host. If any of these mechanisms prevent a packet's delivery, the
//! packet will remain in a send queue at the sender. (Packets sent using
//! [`SendMode::TimeSensitive`] are an exception to this.) Thus, a sender can expect that packets
//! will begin to accumulate in its send queue if the connection bandwidth is low, or if the
//! receiver has ceased to call [`Peer::poll_events()`](Peer::poll_events). If this is a problem,
//! the application may query the total size of pending packets in the send queue by calling
//! [`Peer::send_queue_size()`](Peer::send_queue_size), and handle the situation accordingly.
//!
//! # Connection Events and Receiving Data
//!
//! Connection status updates and received packets are delivered to the application by periodically
//! calling [`Peer::poll_events()`](peer::Peer::poll_events). Each pending connection [`Event`] is
//! returned via iterator as follows:
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
//! [`Disconnect`](Event::Disconnect) event will be generated. Once a packet has been received in
//! full (and that packet is not waiting for any previous packets), a [`Receive`](Event::Receive)
//! event will be generated. If a connection times out at any point, a [`Timeout`](Event::Timeout)
//! event will be generated. The last event generated by a `Peer` object will always be a
//! `Timeout`, even after a connection has closed normally.
//!
//! ##### Maximum Receive Allocation
//!
//! If `poll_events()` is not called for whatever reason, the number of packets in the receive
//! buffer will be limited according to the endpoint's [maximum receive
//! allocation](EndpointConfig#structfield.max_receive_alloc), and the delivery of new packets will
//! stall.
//!
//! ##### Optimal Acknowledgements
//!
//! If desired, the application may call `flush()` on the associated client or server object
//! immediately after all events have been handled, as this allows for the latest acknowledgement
//! information to be sent as soon as possible. (If `flush()` were called prior to handling events,
//! the acknowledgements sent would not indicate which packets had been delivered—from a
//! synchronization standpoint, this is sub-optimal but non-fatal.)
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

/// The number of header bytes of a UDP packet (including the IP header).
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

