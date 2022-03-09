
# Uflow

Uflow is a Rust library and UDP networking protocol for realtime internet data
transfer, with a focus on simplicity and security. Though it has been designed
from the ground up, Uflow's interface and functionality are inspired by the
venerable [ENet](http://enet.bespin.org) library.

## Features

  * Packet-oriented data transfer between two hosts
  * 4-way connection handshake supporting both client-server and peer-to-peer
    connections
  * Automatic packet fragmentation and reassembly according to the internet MTU
    (1500 bytes)
  * Up to 64 virtual, independently sequenced packet streams
  * 4 intuitive packet transfer modes: *Time-Sensitive*, *Unreliable*,
    *Persistent*, and *Reliable*
  * TCP-friendly, streaming congestion control implemented according to RFC
    5348
  * Efficient and minimal frame encoding and transfer protocol
  * 100% packet throughput under ideal network conditions
  * Water-tight sequence ID management for maximum dup-mitigation
  * Application-configurable receiver memory limits (to prevent memory
    allocation attacks)
  * Sender-validated acknowledgements (to prevent loss rate / bandwidth
    spoofing)
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

These and other features have proven to be functional, and the new design will
soon™ be summarized in a subsequent whitepaper.

