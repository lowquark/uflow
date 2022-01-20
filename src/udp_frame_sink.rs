
use crate::endpoint;

use std::net;

pub struct UdpFrameSink<'a> {
    socket: &'a net::UdpSocket,
    address: net::SocketAddr,
}

impl<'a> UdpFrameSink<'a> {
    pub fn new(socket: &'a net::UdpSocket, address: net::SocketAddr) -> Self {
        Self {
            socket: socket,
            address: address,
        }
    }
}

impl<'a> endpoint::FrameSink for UdpFrameSink<'a> {
    fn send(&mut self, frame_data: &[u8]) {
        let _ = self.socket.send_to(frame_data, self.address);
    }
}

