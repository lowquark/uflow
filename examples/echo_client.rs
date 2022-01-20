
fn main() {
    // Create a client object, with a maximum of 1 concurrent connection
    let mut client = uflow::Client::bind_any_ipv4().unwrap();

    // The client will send data on only one transmission channel
    let cfg = uflow::EndpointCfg::new()
        .tx_channels(1);

    // Initiate the connection to the server
    let mut server = client.connect("127.0.0.1:8888", cfg).expect("Invalid address");

    let mut send_counter = 0;
    let mut message_counter = 0;

    loop {
        // Process inbound UDP frames
        client.step();

        // Handle events
        for event in server.poll_events() {
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
                uflow::Event::Receive(packet_data, channel_id) => {
                    let packet_data_utf8 = std::str::from_utf8(&packet_data).unwrap();

                    println!("received \"{}\" on channel {}", packet_data_utf8, channel_id);
                }
            }
        }

        // Periodically send incrementing hello worlds on channel 0
        send_counter += 1;
        if send_counter == 10 {
            let packet_data: Box<[u8]> = format!("Hello world {}!", message_counter).as_bytes().into();
            server.send(packet_data, 0, uflow::SendMode::Reliable);

            send_counter = 0;
            message_counter += 1;
        }

        // Flush outbound UDP frames
        client.flush();

        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

