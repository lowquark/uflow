
# Changelog

## 0.7.0

The public API has changed significantly since the previous version:

* A 3-way handshake similar to TCP has been implemented. This removes the
  possibility of connecting two `Client` objects simultaneously, but allows
  servers to be far more robust to spam and DDoS reflection attacks.

* Client objects now manage a single outbound connection, which resolves a
  major bug involving simultaneous connections to the same server. In general,
  a `Client` object is a drop-in replacement for a `Peer` object.

* Packets and connection events are received from the `Client`/`Server` object
  directly after the call to `step()`, rather than through an awkward
  combination of `step()` followed by `Peer::poll_events()`. This means that
  events from all connected clients must be handled in one place, but allows
  for easier management of per-connection userdata.

* For servers, what now corresponds to a `Peer` object (`RemoteClient`) is not
  stored within an `Arc<RwLock<...>>`, and thus cannot be passed between
  threads.

## 0.6.1

* Subtly optimized memory usage of sender fragment acknowledgement flags

* Switched to `Arc<RwLock<...>>` internally so as to allow `Peer` objects to be
  processed by a separate thread from their containing `Client`/`Server`

## 0.6.0

* Subtly altered the behavior of `(Server|Client)::step()` in order to allow
  acknowledgements to indicate which packets have been delivered without
  waiting for the next call to `step()`

* Reduced the space of packet sequence IDs to a comfortable minimum of 20 bits,
  and optimized the data frame encoding so that certain packets containing less
  than 64 bytes of data require only 6 bytes of overhead

* Added 32-bit checksums to all frames

* Added a keepalive flag to `EndpointConfig`, allowing the application to
  specify whether or not to send keepalive frames

* Added a method to `Peer` which returns the total amount of data awaiting
  delivery, allowing the application to detect and terminate prohibitively slow
  connections

* Implemented a channel tracking mechanism for received packets which
  elliminates any need to specify the number of channels during connection
  initialization (and removed the associated `EndpointConfig` parameter)

* Optimized packet reordering/delivery for sparsely populated receive windows,
  unnecessary iteration, and otherwise cache-efficiency

* Removed the mostly redundant setters of `EndpointConfig`

* Improved the windup behavior of traffic shaping through a more opportunistic
  leaky bucket algorithm—the result is compliant with RFC 5348, section 4.6

* Removed time-tracking from calls to `(Server|Client)::flush()`, thereby
  ensuring that redundant calls do not affect the flush allocation, and that
  `step()` may first call `flush()` without producing erroneous resends

## 0.5.1

Fixed bad protocol version ID

## 0.5.0

Major internal refactoring related to frame queues and TFRC feedback
computation:

* Eliminated a copy when generating initial fragments

* Corrected the initial send rate computation, as well as sender behavior when
  no data acknowledgements have been received

* Fixed the loss rate computation to process the next pending frame ID as soon
  as it arrives

* Implemented a frame receive window to ensure the packets of duplicated frames
  are ignored, paving the way for a reduced packet sequence ID space

* Augmented the existing resynchronization scheme to synchronize both the packet
  window and the frame window as appropriate

