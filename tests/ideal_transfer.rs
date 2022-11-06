use std::thread;
use std::time;

use std::convert::TryInto;

extern crate md5;

const STEP_DURATION: time::Duration = time::Duration::from_millis(15);

const NUM_CHANNELS: usize = 4;

fn server_thread() -> Vec<md5::Digest> {
    let cfg = Default::default();

    let mut server = uflow::server::Server::bind("127.0.0.1:8888", cfg).unwrap();

    let mut all_data: Vec<Vec<u8>> = vec![Vec::new(); NUM_CHANNELS as usize];
    let mut packet_ids = [0u32; NUM_CHANNELS];
    let mut connect_seen = false;

    'outer: loop {
        for event in server.step() {
            match event {
                uflow::server::Event::Connect(peer_addr) => {
                    assert_eq!(connect_seen, false);
                    connect_seen = true;

                    println!("[server] client connected from {:?}", peer_addr);
                }
                uflow::server::Event::Receive(_peer_addr, data) => {
                    let channel_id = data[0] as usize;
                    let packet_id = u32::from_be_bytes(data[1..5].try_into().unwrap());

                    //println!("[server] received data on channel id {}\ndata begins with: {:?}", channel_id, &data[0..4]);

                    let ref mut packet_id_expected = packet_ids[channel_id];

                    if packet_id != *packet_id_expected {
                        panic!("[server] data skipped! received ID: {} expected ID: {}", packet_id, packet_id_expected);
                    }

                    all_data[channel_id].extend_from_slice(&data);
                    *packet_id_expected += 1;
                }
                uflow::server::Event::Disconnect(_peer_addr) => {
                    println!("[server] client disconnected");
                    break 'outer;
                }
                other => panic!("[server] unexpected event: {:?}", other),
            }
        }

        server.flush();

        thread::sleep(STEP_DURATION);
    }

    println!("[server] exiting");

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

fn client_thread() -> Vec<md5::Digest> {
    let cfg = Default::default();

    let mut client = uflow::client::Client::connect("127.0.0.1:8888", cfg).unwrap();

    let num_steps = 100;
    let packets_per_step = 6;
    let packet_size = uflow::MAX_FRAGMENT_SIZE;

    let mut all_data: Vec<Vec<u8>> = vec![Vec::new(); NUM_CHANNELS as usize];
    let mut packet_ids = [0u32; NUM_CHANNELS];
    let mut connect_seen = false;

    for _ in 0..num_steps {
        for event in client.step() {
            match event {
                uflow::client::Event::Connect => {
                    assert_eq!(connect_seen, false);
                    connect_seen = true;

                    println!("[client] connected to server");
                }
                other => panic!("[client] unexpected event: {:?}", other),
            }
        }

        for _ in 0..packets_per_step {
            let channel_id = rand::random::<usize>() % NUM_CHANNELS;
            let ref mut packet_id = packet_ids[channel_id];

            let mut data = (0..packet_size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice();
            data[0] = channel_id as u8;
            data[1..5].clone_from_slice(&packet_id.to_be_bytes());

            // Our local loopback connection is assumed to be both ordered and lossless!
            let mode = match rand::random::<u32>() % 3 {
                0 => uflow::SendMode::Unreliable,
                1 => uflow::SendMode::Persistent,
                2 => uflow::SendMode::Reliable,
                _ => panic!("NANI!?"),
            };

            all_data[channel_id].extend_from_slice(&data);

            client.send(data, channel_id, mode);

            println!("[client] sent packet {} on channel {}", packet_id, channel_id);

            *packet_id += 1;
        }

        client.flush();

        thread::sleep(STEP_DURATION);
    }

    println!("[client] disconnecting");
    client.disconnect();

    'outer: loop {
        for event in client.step() {
            match event {
                uflow::client::Event::Disconnect => {
                    println!("[client] server disconnected");
                    break 'outer;
                }
                other => panic!("[client] unexpected event: {:?}", other),
            }
        }

        client.flush();

        thread::sleep(STEP_DURATION);
    }

    println!("[client] Exiting");

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

#[test]
fn ideal_transfer_two() {
    let server = thread::spawn(server_thread);

    thread::sleep(time::Duration::from_millis(200));

    let client = thread::spawn(client_thread);

    let server_md5s = server.join().unwrap();
    let client_md5s = client.join().unwrap();

    assert_eq!(server_md5s, client_md5s);
}
