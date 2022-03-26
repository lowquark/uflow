
fn main() {
    // Create a client object
    let mut client = uflow::Client::bind_any_ipv4().unwrap();

    // Initiate the connection to the server
    let address = "127.0.0.1:8888";
    let config = uflow::EndpointConfig::default();
    let mut server_peer = client.connect(address, config).expect("Invalid address");

    let mut send_counter = 0;
    let mut message_counter = 0;

    loop {
        // Process inbound UDP frames
        client.step();

        // Handle events
        for event in server_peer.poll_events() {
            match event {
                uflow::Event::Connect => {
                    println!("connected to server");
                }
                uflow::Event::Disconnect => {
                    println!("disconnected from server");
                }
                uflow::Event::Timeout => {
                    println!("server connection timed out");
                }
                uflow::Event::Receive(packet_data) => {
                    let packet_data_utf8 = std::str::from_utf8(&packet_data).unwrap();

                    println!("received \"{}\"", packet_data_utf8);
                }
            }
        }

        // Periodically send incrementing hello worlds on channel 0
        send_counter += 1;
        if send_counter == 10 {
            let packet_data: Box<[u8]> = format!("Hello world {}!", message_counter).as_bytes().into();

            server_peer.send(packet_data, 0, uflow::SendMode::Unreliable);

            send_counter = 0;
            message_counter += 1;
        }

        // Flush outbound UDP frames
        client.flush();

        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

