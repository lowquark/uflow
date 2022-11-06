use crate::MAX_PACKET_SIZE;

/// Parameters used to configure either endpoint of a `uflow` connection.
#[derive(Clone,Debug)]
pub struct EndpointConfig {
    /// The maximum send rate, in bytes per second. The endpoint will ensure that its outgoing
    /// bandwidth does not exceed this value.
    ///
    /// Must be greater than 0. Values larger than 2^32 will be truncated.
    pub max_send_rate: usize,

    /// The maximum acceptable receive rate, in bytes per second. The opposing endpoint will ensure
    /// that its outgoing bandwidth does not exceed this value.
    ///
    /// Must be greater than 0. Values larger than 2^32 will be truncated.
    pub max_receive_rate: usize,

    /// The maximum size of a sent packet, in bytes. The endpoint will ensure that it does not send
    /// packets with a size exceeding this value.
    ///
    /// Must be greater than 0, and less than or equal to [`MAX_PACKET_SIZE`].
    pub max_packet_size: usize,

    /// The maximum allocation size of the endpoint's receive buffer, in bytes. The endpoint will
    /// ensure that the total amount of memory allocated to receive packet data doesn't exceed this
    /// value, rounded up to the nearest multiple of
    /// [`MAX_FRAGMENT_SIZE`](crate::MAX_FRAGMENT_SIZE).
    ///
    /// Must be greater than 0.
    ///
    /// *Note*: The maximum allocation size necessarily constrains the maximum receivable packet
    /// size. A connection attempt will fail if the `max_packet_size` of the opposing endpoint
    /// exceeds this value.
    pub max_receive_alloc: usize,

    /// Whether the endpoint should automatically send keepalive frames if no data has been sent
    /// for one keepalive interval (currently 5 seconds). If set to false, the connection will time
    /// out if either endpoint does not send data for one timeout interval (currently 20 seconds).
    pub keepalive: bool,
}

impl Default for EndpointConfig {
    /// Creates an endpoint configuration with the following parameters:
    ///   * Maximum outgoing bandwidth: 2MB/s
    ///   * Maximum incoming bandwidth: 2MB/s
    ///   * Maximum packet size: 1MB
    ///   * Maximum packet receive allocation: 1MB
    ///   * Keepalive: true
    fn default() -> Self {
        Self {
            max_send_rate: 2_000_000,
            max_receive_rate: 2_000_000,

            max_packet_size: 1_000_000,
            max_receive_alloc: 1_000_000,

            keepalive: true,
        }
    }
}

impl EndpointConfig {
    /// Returns `true` if each parameter has a valid value.
    pub fn is_valid(&self) -> bool {
        self.max_send_rate > 0 &&
        self.max_receive_rate > 0 &&
        self.max_packet_size > 0 &&
        self.max_packet_size <= MAX_PACKET_SIZE &&
        self.max_receive_alloc > 0
    }
}
