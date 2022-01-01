
extern crate md5;

static NUM_CHANNELS: usize = 4;

fn server_thread() -> Vec<md5::Digest> {
    let params = udpl::host::Params::new()
        .tx_channels(NUM_CHANNELS);

    let mut host = udpl::host::Host::bind("127.0.0.1:8888", params).unwrap();
    let mut clients = Vec::new();

    let mut all_data: Vec<Vec<u8>> = vec![Vec::new(); NUM_CHANNELS as usize];

    'outer: loop {
        host.step();

        for client in host.incoming() {
            clients.push(client);
        }

        for client in clients.iter_mut() {
            for event in client.poll_events() {
                match event {
                    udpl::host::Event::Connect => {
                        println!("Server connect");
                    }
                    udpl::host::Event::Receive(data, channel_id) => {
                        all_data[channel_id as usize].extend_from_slice(&data);
                    }
                    udpl::host::Event::Disconnect => {
                        println!("Server disconnect");
                        break 'outer;
                    }
                    other => println!("Unexpected server event: {:?}", other),
                }
            }
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

fn client_thread() -> Vec<md5::Digest> {
    let params = udpl::host::Params::new()
        .tx_channels(NUM_CHANNELS);

    let mut host = udpl::host::Host::bind_any(params).unwrap();
    let mut client = host.connect("127.0.0.1:8888".parse().unwrap());

    let num_steps = 100;
    let packets_per_step = 20;
    let packet_size = udpl::MAX_TRANSFER_UNIT/3;

    let mut all_data: Vec<Vec<u8>> = vec![Vec::new(); NUM_CHANNELS as usize];

    for _ in 0..num_steps {
        host.step();

        for event in client.poll_events() {
            match event {
                udpl::host::Event::Connect => {
                    println!("Client connect");
                }
                other => println!("Unexpected client event: {:?}", other),
            }
        }

        for _ in 0..packets_per_step {
            let data = (0..packet_size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice();

            let channel_id = rand::random::<u8>() % NUM_CHANNELS as u8;

            // Our local loopback connection is assumed to be both ordered and lossless!
            let mode = match rand::random::<u32>() % 3 {
                0 => udpl::SendMode::Unreliable,
                1 => udpl::SendMode::Resend,
                2 => udpl::SendMode::Reliable,
                _ => panic!("NANI!?"),
            };

            all_data[channel_id as usize].extend_from_slice(&data);

            client.send(data, channel_id, mode);
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    println!("Client disconnecting...");
    client.disconnect();

    'outer: loop {
        host.step();

        for event in client.poll_events() {
            match event {
                udpl::host::Event::Disconnect => {
                    println!("Client disconnect");
                    break 'outer;
                }
                other => println!("Unexpected client event: {:?}", other),
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

#[test]
fn test_ideal_transfer() {
    let server = std::thread::spawn(server_thread);

    std::thread::sleep(std::time::Duration::from_millis(200));

    let client = std::thread::spawn(client_thread);

    let server_md5s = server.join().unwrap();
    let client_md5s = client.join().unwrap();

    assert_eq!(server_md5s, client_md5s);
}

