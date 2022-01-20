
fn main() {
    // The server will send data on two transmission channels
    let cfg = uflow::EndpointConfig::new()
        .tx_channels(2);

    // Create a server object bound locally on port 8888, with a maximum of 8 concurrent connections
    let mut server = uflow::Server::bind("127.0.0.1:8888", 8, cfg).unwrap();

    // List of active connections
    let mut clients = Vec::new();

    loop {
        // Process inbound UDP frames
        server.step();

        // Add each incoming connection to the client list
        for client_peer in server.incoming() {
            println!("[{:?}] appeared", client_peer.address());
            clients.push(client_peer);
        }

        // Handle events for each connected client
        for client_peer in clients.iter_mut() {
            for event in client_peer.poll_events() {
                match event {
                    uflow::Event::Connect => {
                        println!("[{:?}] connected", client_peer.address());
                    }
                    uflow::Event::Disconnect => {
                        println!("[{:?}] disconnected", client_peer.address());
                    }
                    uflow::Event::Timeout => {
                        println!("[{:?}] timed out", client_peer.address());
                    }
                    uflow::Event::Receive(packet_data, channel_id) => {
                        let packet_data_utf8 = std::str::from_utf8(&packet_data).unwrap();
                        let reversed_string: std::string::String = packet_data_utf8.chars().rev().collect();

                        println!("[{:?}]: received \"{}\" on channel {}", client_peer.address(), packet_data_utf8, channel_id);

                        // Echo the packet reliably on channel 0
                        client_peer.send(packet_data, 0, uflow::SendMode::Reliable);
                        // Echo the reverse of the packet unreliably on channel 1
                        client_peer.send(reversed_string.as_bytes().into(), 1, uflow::SendMode::Unreliable);
                    }
                }
            }
        }

        // Flush outbound UDP frames
        server.flush();

        // Forget clients which have disconnected
        clients.retain(|client_peer| !client_peer.is_disconnected());

        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

