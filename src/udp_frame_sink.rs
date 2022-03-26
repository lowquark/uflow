
use crate::endpoint;

use std::net;

// TODO: A Result<usize, std::io::Error> stored here could be used to forward errors to
// client/server step/flush after the FrameSink has been used.
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
        //use crate::frame;
        //use frame::serial::Serialize;
        //let time_millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        //println!("{} {:?}", time_millis, frame::Frame::read(&frame_data));
        let _ = self.socket.send_to(frame_data, self.address);
    }
}

