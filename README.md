
# Uflow

Uflow is a Rust library and UDP networking protocol for realtime internet data
transfer, with a focus on simplicity and robustness. Though it has been
designed from the ground up, Uflow's interface and functionality are inspired
by the venerable [ENet](http://enet.bespin.org) library.

## Features

  * Packet-oriented data transfer between two hosts
  * Automatic packet fragmentation and reassembly according to the internet MTU
    (1500 bytes)
  * 3-way connection handshake for proper connection management
  * Up to 64 independently sequenced packet streams
  * 4 intuitive packet transfer modes: *Time-Sensitive*, *Unreliable*,
    *Persistent*, and *Reliable*
  * TCP-friendly, streaming congestion control implemented according to [RFC 5348](https://datatracker.ietf.org/doc/html/rfc5348)
  * Efficient frame encoding and transfer protocol with minimal packet overhead
  * CRC validation for all transmitted frames ([Polynomial: 0x132c00699](http://users.ece.cmu.edu/~koopman/crc/hd6.html))
  * 100% packet throughput and an unaffected delivery order under ideal network
    conditions
  * Water-tight sequence ID management for maximum dup-mitigation
  * Application-configurable receiver memory limits (to prevent memory
    allocation attacks)
  * Nonce-validated data acknowledgements (to prevent loss rate / bandwidth
    spoofing)
  * Resilient to DDoS amplification (request-reply ratio ≈ 28:1)
  * Meticulously designed and unit tested to ensure stall-free behavior
  * Threadless, non-blocking implementation

## Documentation

Documentation can be found at [docs.rs](https://docs.rs/uflow/latest/uflow/).

## Architecture

Although a previous version is described in the [whitepaper](whitepaper.pdf),
much has changed about the library in the meantime (including the name!). The
current version has the following improvements:

  * TCP-friendly congestion control implemented according to RFC 5348
  * Receiver memory limits (for packet reassembly)
  * No sentinel packets or frames
  * An additional packet send mode which causes packets to be dropped if they
    cannot be sent immediately (Time-Sensitive)
  * No iteration over the number of channels

The new design will soon™ be summarized in an updated whitepaper.

