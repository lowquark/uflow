
# uflow

uflow is Rust library and UDP networking protocol for realtime internet data
transfer, with a focus on simplicity and security. Though it has been designed
from the ground up, uflow's interface and functionality are inspired by the
venerable [ENet](http://enet.bespin.org) library.

The previous version, 0.2, is described in the [whitepaper](whitepaper.pdf).

The current version, 0.3, has the following improvements over 0.2:

  * TCP-friendly congestion control implemented according to RFC 5348
  * Receiver memory limits (for packet reassembly)
  * No sentinel packets or frames
  * An additional packet send mode which causes packets to be dropped if they
    cannot be sent immediately
  * No iteration over the number of channels

These and other features have proven to be functional, and the new design will
be summarized in a subsequent whitepaper. Documentation can be found at
[docs.rs](https://docs.rs/uflow/0.3.0/uflow/).

