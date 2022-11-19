#![warn(missing_docs)]

//! `uflow` is a non-blocking, connection-based layer over UDP that provides an ordered and
//! drop-tolerant packet streaming interface for real-time applications (e.g. games). It manages
//! connection state, packet sequencing, packet fragmentation, reliable delivery, and congestion
//! control to create a simple and robust solution for low-latency internet communication.
//!
//! # Hosting a Server
//!
//! A `uflow` server is created by calling [`Server::bind[...]()`](server::Server::bind), which
//! opens a UDP socket bound to the specified address, and returns a corresponding `Server` object.
//! The number of active connections will be restricted to the configured limit, and each incoming
//! connection will be initialized using the given endpoint configuration (see:
//! [`EndpointConfig`]).
//!
//! ```
//! let server_address = "127.0.0.1:8888";
//! let config = uflow::server::Config {
//!     max_total_connections: 4096,
//!     max_active_connections: 8,
//!     endpoint_config: Default::default(),
//! };
//!
//! // Create a server object
//! let mut server = uflow::server::Server::bind(server_address, config)
//!     .expect("Failed to bind/configure socket");
//! ```
//!
//! As a non-blocking interface, a server object depends on periodic calls to
//! [`Server::step()`](server::Server::step) to process inbound traffic, to update connection
//! states, and for timekeeping. To signal pending events to the application, `step()` returns an
//! iterator to a list of [`server::Event`] objects which contain information specific to each
//! event type.
//!
//! Once a client handshake has been completed, a [`RemoteClient`](server::RemoteClient) object
//! will be created to represent the new connection. Connected clients are indexed by address, and
//! may be accessed by calling [`Server::client()`](server::Server::client).
//!
//! A `RemoteClient` functions as a handle for a given connection, and allows the server to send
//! packets and query connection status. However, no packets will be placed on the network, and no
//! received packets will be processed until the next call to
//! [`Server::step()`](server::Server::step). The application may call
//! [`Server::flush()`](server::Server::flush) to send all pending outbound data immediately.
//!
//! A basic server loop that extends the above example is shown below:
//!
//! ```
//! # let server_address = "127.0.0.1:8888";
//! # let config = Default::default();
//! # let mut server = uflow::server::Server::bind(server_address, config).unwrap();
//! loop {
//!     // Process inbound UDP frames and handle events
//!     for event in server.step() {
//!         match event {
//!             uflow::server::Event::Connect(client_address) => {
//!                 // TODO: Handle client connection
//!             }
//!             uflow::server::Event::Disconnect(client_address) => {
//!                 // TODO: Handle client disconnection
//!             }
//!             uflow::server::Event::Error(client_address, error) => {
//!                 // TODO: Handle connection error
//!             }
//!             uflow::server::Event::Receive(client_address, packet_data) => {
//!                 // Echo the packet on channel 0
//!                 let mut client = server.client(&client_address).unwrap().borrow_mut();
//!                 client.send(packet_data, 0, uflow::SendMode::Unreliable);
//!             }
//!         }
//!     }
//!
//!     // Send data, update server application state
//!     // ...
//!
//!     // Flush outbound data
//!     server.flush();
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
//! A `uflow` client is created by calling [`Client::connect()`](client::Client::connect), which
//! opens a non-blocking UDP socket, and initiates a connection using the provided destination
//! address and endpoint configuration. If the destination address could be resolved, a new
//! [`Client`](client::Client) object representing the connection will be returned.
//!
//! ```
//! let server_address = "127.0.0.1:8888";
//! let config = Default::default();
//!
//! // Create a client object
//! let mut client = uflow::client::Client::connect(server_address, config)
//!     .expect("Invalid address");
//! ```
//!
//! Like a server, a client depends on periodic calls to [`Client::step()`](client::Client::step)
//! in order to process inbound traffic, to update connection state, and for timekeeping. A basic
//! client loop extending the above example is shown below:
//!
//! ```
//! # let server_address = "127.0.0.1:8888";
//! # let config = Default::default();
//! # let mut client = uflow::client::Client::connect(server_address, config).unwrap();
//! loop {
//!     // Process inbound UDP frames
//!     for event in client.step() {
//!         match event {
//!             uflow::client::Event::Connect => {
//!                 // TODO: Handle connection
//!             }
//!             uflow::client::Event::Disconnect => {
//!                 // TODO: Handle disconnection
//!             }
//!             uflow::client::Event::Error(error) => {
//!                 // TODO: Handle connection error
//!             }
//!             uflow::client::Event::Receive(packet_data) => {
//!                 // TODO: Handle received packets
//!             }
//!         }
//!     }
//!
//!     // Send data, update client application state
//!     // ...
//!
//!     // Flush outbound data
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
//! Packets are sent to a remote host by calling [`client::Client::send()`] or
//! [`server::RemoteClient::send()`], which additionally requires a channel ID and a packet send
//! mode. Any packets that are sent prior to establishing a connection will be sent once the
//! connection succeeds.
//!
//! ```
//! let server_address = "127.0.0.1:8888";
//! let config = Default::default();
//! let mut client = uflow::client::Client::connect(server_address, config).unwrap();
//!
//! let packet_data = "Hello world!".as_bytes();
//! let channel_id = 0;
//! let send_mode = uflow::SendMode::Reliable;
//!
//! client.send(packet_data.into(), channel_id, send_mode);
//! ```
//!
//! Additional details relating to how packets are sent and received by `uflow` are described in
//! the following subsections.
//!
//! ##### Packet Fragmentation and Aggregation
//!
//! All packets are aggregated into larger UDP frames, and are automatically divided into fragments
//! so as to ensure no frame exceeds the internet MTU (1500 bytes). Fragments are transferred with
//! the same send mode as their containing packet—that is, fragments will be resent if and only if
//! the packet is marked with [`SendMode::Persistent`] or [`SendMode::Reliable`]. A packet is
//! considered received once all of its constituent fragments have been received.
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
//! received. This means that if a reliable packet is dropped, that channel will effectively stall
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
//! The total size of all packets awaiting delivery can be obtained by calling
//! [`Client::send_buffer_size()`](client::Client::send_buffer_size) or
//! [`RemoteClient::send_buffer_size()`](server::RemoteClient::send_buffer_size), and if desired, an
//! application can use this value to terminate excessively delayed connections. In addition, the
//! application may send packets using [`SendMode::TimeSensitive`] to drop packets at the sender if
//! they could not be sent immediately (i.e. prior to the next call to `step()`). In the event that
//! the total available bandwidth is limited, this prevents outdated packets from using any
//! unnecessary bandwidth, and prioritizes sending newer packets in the send queue.
//!
//! # Receiving Packets (and Other Events)
//!
//! Connection status updates and received packets are delivered to the application by calling
//! [`Client::step()`](client::Client::step) and [`Server::step()`](server::Server::step), as shown
//! previously. A `Connect` event will be generated when a connection is first established, and if
//! either end of the connection explicitly disconnects, a `Disconnect` event will be generated.
//! Once a packet has been received (and that packet is not waiting for any previous packets), a
//! `Receive` event will be generated. If an error is encountered, or a connection times out at any
//! point, an `Error` event will be generated. Once a `Disconnect` or an `Error` event has been
//! generated, no further events will be generated.
//!
//! ##### Maximum Receive Allocation
//!
//! If a sender is sending a continuous stream of packets, but `step()` is not called on the
//! receiver for whatever reason, the number of packets in the receiver's receive buffer will
//! increase until its [maximum receive allocation](EndpointConfig#structfield.max_receive_alloc)
//! has been reached. At that point, any new packets will be silently ignored.
//!
//! *Note*: This feature is intended to guard against memory allocation attacks. A well-behaved
//! sender will ensure that it does not send new packets exceeding the receiver's memory limit, and
//! the stall will back-propagate accordingly.
//!
//! ##### Optimal Acknowledgements
//!
//! If desired, the sender may call `flush()` on the associated client or server object immediately
//! after all events from `step()` have been handled. By doing so, information relating to which
//! packets have been delivered (and how much buffer space is available) will be relayed to the
//! sender as soon as possible. Although this will prevent acknowledgement data from being
//! aggregated with any subsequent packet data, the resulting increase in bandwidth is probably
//! inconsequential.
//!
//! # Disconnecting
//!
//! A connection is explicitly closed by calling
//! [`Client::disconnect()`](client::Client::disconnect) or
//! [`Server::disconnect()`](server::Server::disconnect), which makes an effort to
//! notify the remote host of the disconnection, and to send all pending outbound packets before
//! doing so. The sender can expect that any pending reliable packets will be delivered prior to
//! disconnecting, provided that the remote host doesn't also disconnect in the meantime. The
//! application must continue to call `step()` to ensure that the disconnection takes place.
//!
//! ```
//! # let server_address = "127.0.0.1:8888";
//! # let config = Default::default();
//! # let mut client = uflow::client::Client::connect(server_address, config).unwrap();
//! client.disconnect();
//!
//! // ... calls to step() continue
//! ```
//!
//! Alternatively, one may call `disconnect_now()`, which sends no further packets and forgets the
//! connection immediately. Because no notification is sent, this will cause a timeout on the
//! remote host.
//!
//! ```
//! # let server_address = "127.0.0.1:8888";
//! # let config = Default::default();
//! # let mut client = uflow::client::Client::connect(server_address, config).unwrap();
//! client.disconnect_now();
//!
//! assert!(client.is_active() == false);
//! ```

mod half_connection;
mod endpoint_config;
mod frame;
mod packet_id;
mod udp_frame_sink;

/// Contains server-side connection objects and parameters.
pub mod server;

/// Contains client-side connection objects and parameters.
pub mod client;

pub use endpoint_config::EndpointConfig as EndpointConfig;

/// The current protocol version ID.
pub const PROTOCOL_VERSION: u8 = 3;

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
    /// prior to the next call to [`Client::step`](client::Client::step) or
    /// [`Server::step`](server::Server::step)), it will be discarded by the sender. If this packet
    /// has not been received, but a subsequent packet has been received on the same channel, the
    /// receiver may skip this packet.
    TimeSensitive,
    /// This packet will be sent exactly once. If this packet has not been received, but a
    /// subsequent packet has been received on the same channel, the receiver may skip this packet.
    Unreliable,
    /// This packet will be sent and resent until acknowledged by the receiver. If this packet has
    /// not been received, but a subsequent packet has been received on the same channel, the
    /// receiver may skip this packet.
    ///
    /// *Note:* The packet will cease to be resent once the sender has detected a skip.
    Persistent,
    /// This packet will be sent until acknowledged by the receiver. The receiver will not deliver
    /// subsequent packets on the same channel until this packet has been delivered.
    Reliable,
}
