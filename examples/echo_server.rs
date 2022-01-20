
fn main() {
    // The server will send data on two transmission channels
    let cfg = uflow::EndpointCfg::new()
        .tx_channels(2);

    // Create a server object bound to localhost on port 8888, with a maximum of 8 concurrent connections
    let mut server = uflow::Server::bind("127.0.0.1:8888", 8, cfg).unwrap();

    // List of active connections
    let mut clients = Vec::new();

    loop {
        // Process inbound UDP frames
        server.step();

        for client in server.incoming() {
            // Add each incoming connection to the client list
            println!("[{:?}] appeared", client.address());
            clients.push(client);
        }

        for client in clients.iter_mut() {
            // Handle events for each connected client
            for event in client.poll_events() {
                match event {
                    uflow::Event::Connect => {
                        println!("[{:?}] connected", client.address());
                    }
                    uflow::Event::Disconnect => {
                        println!("[{:?}] disconnected", client.address());
                    }
                    uflow::Event::Timeout => {
                        println!("[{:?}] timed out", client.address());
                    }
                    uflow::Event::Receive(packet_data, channel_id) => {
                        let packet_data_utf8 = std::str::from_utf8(&packet_data).unwrap();
                        let reversed_string: std::string::String = packet_data_utf8.chars().rev().collect();

                        println!("[{:?}]: received \"{}\" on channel {}", client.address(), packet_data_utf8, channel_id);

                        // Echo the packet reliably on channel 0
                        client.send(packet_data, 0, uflow::SendMode::Reliable);
                        // Echo the reverse of the packet unreliably on channel 1
                        client.send(reversed_string.as_bytes().into(), 1, uflow::SendMode::Unreliable);
                    }
                }
            }
        }

        // Flush outbound UDP frames
        server.flush();

        // Forget clients which have disconnected
        clients.retain(|client| !client.is_disconnected());

        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

