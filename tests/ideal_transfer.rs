
extern crate md5;

use std::convert::TryInto;

const NUM_CHANNELS: usize = 4;

fn server_thread() -> Vec<md5::Digest> {
    let params = uflow::EndpointParams::new()
        .tx_channels(NUM_CHANNELS);

    let mut host = uflow::Host::bind("127.0.0.1:8888", 1, params).unwrap();
    let mut clients = Vec::new();

    let mut all_data: Vec<Vec<u8>> = vec![Vec::new(); NUM_CHANNELS as usize];

    let mut packet_ids = [0u32; NUM_CHANNELS];

    'outer: loop {
        host.step();

        for client in host.incoming() {
            clients.push(client);
        }

        for client in clients.iter_mut() {
            for event in client.poll_events() {
                match event {
                    uflow::Event::Connect => {
                        println!("[server] client connected");
                    }
                    uflow::Event::Receive(data, channel_id) => {
                        println!("[server] received data on channel id {}\ndata begins with: {:?}", channel_id, &data[0..4]);

                        let ref mut packet_id_expected = packet_ids[channel_id];

                        let packet_id = u32::from_be_bytes(data[0..4].try_into().unwrap());

                        if packet_id != *packet_id_expected {
                            panic!("[server] data skipped! received ID: {} expected ID: {}", packet_id, packet_id_expected);
                        }

                        all_data[channel_id].extend_from_slice(&data);
                        *packet_id_expected += 1;
                    }
                    uflow::Event::Disconnect => {
                        println!("[server] client disconnected");
                        break 'outer;
                    }
                    other => println!("[server] unexpected event: {:?}", other),
                }
            }
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    println!("[server] exiting");

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

fn client_thread() -> Vec<md5::Digest> {
    let params = uflow::EndpointParams::new()
        .tx_channels(NUM_CHANNELS);

    let mut host = uflow::Host::bind_any(1, params).unwrap();
    let mut client = host.connect("127.0.0.1:8888".parse().unwrap());

    let num_steps = 100;
    let packets_per_step = 20;
    let packet_size = uflow::MAX_FRAGMENT_SIZE/3;

    let mut all_data: Vec<Vec<u8>> = vec![Vec::new(); NUM_CHANNELS as usize];

    let mut packet_ids = [0u32; NUM_CHANNELS];

    for _ in 0..num_steps {
        host.step();

        for event in client.poll_events() {
            match event {
                uflow::Event::Connect => {
                    println!("[client] connected to server");
                }
                other => println!("[client] unexpected event: {:?}", other),
            }
        }

        for _ in 0..packets_per_step {
            let channel_id = rand::random::<usize>() % NUM_CHANNELS;
            let ref mut packet_id = packet_ids[channel_id];

            let mut data = (0..packet_size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice();
            data[0..4].clone_from_slice(&packet_id.to_be_bytes());

            // Our local loopback connection is assumed to be both ordered and lossless!
            let mode = match rand::random::<u32>() % 3 {
                0 => uflow::SendMode::Unreliable,
                1 => uflow::SendMode::Resend,
                2 => uflow::SendMode::Reliable,
                _ => panic!("NANI!?"),
            };

            all_data[channel_id].extend_from_slice(&data);

            client.send(data, channel_id, mode);

            println!("[client] sent packet {} on channel {}", packet_id, channel_id);

            *packet_id += 1;
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    println!("[client] disconnecting");
    client.disconnect();

    'outer: loop {
        host.step();

        for event in client.poll_events() {
            match event {
                uflow::Event::Disconnect => {
                    println!("[client] server disconnected");
                    break 'outer;
                }
                other => println!("[client] unexpected event: {:?}", other),
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    println!("[client] Exiting");

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

#[test]
fn ideal_transfer() {
    let server = std::thread::spawn(server_thread);

    std::thread::sleep(std::time::Duration::from_millis(200));

    let client = std::thread::spawn(client_thread);

    let server_md5s = server.join().unwrap();
    let client_md5s = client.join().unwrap();

    assert_eq!(server_md5s, client_md5s);
}

