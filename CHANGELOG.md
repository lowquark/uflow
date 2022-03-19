
# Changelog

## 0.6.0

* Subtly altered the behavior of `(Server|Client)::step()` (and made calls to
  `flush()` mandatory) in order to allow acknowledgements to indicate which
  packets have been delivered without waiting for the next call to `step()`

* Reduced the space of packet sequence IDs to a comfortable minimum of 20 bits,
  and optimized the data frame encoding so that certain packets containing less
  than 64 bytes of data require only 6 bytes of overhead

* Added 32-bit checksums to all frames

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

