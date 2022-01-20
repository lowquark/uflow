
fn main() {
    // The client will send data on only one transmission channel
    let params = uflow::EndpointParams::new()
        .tx_channels(1);

    // Create a host object, with a maximum of 1 concurrent connection
    let mut host = uflow::Host::bind_any(1, params).unwrap();

    // Initiate the connection to the server
    let mut client = host.connect("127.0.0.1:8888".parse().unwrap());

    let mut send_counter = 0;
    let mut message_counter = 0;

    loop {
        // Process inbound UDP frames
        host.step();

        // Handle events
        for event in client.poll_events() {
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
            client.send(packet_data, 0, uflow::SendMode::Reliable);

            send_counter = 0;
            message_counter += 1;
        }

        // Flush outbound UDP frames
        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

