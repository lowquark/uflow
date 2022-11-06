use std::net;
use std::thread;
use std::time;

use std::convert::TryInto;

extern crate md5;

const STEP_DURATION: time::Duration = time::Duration::from_millis(15);

const NUM_CHANNELS: usize = 64;

struct BandwidthLimiter {
    bandwidth: f64,
    limit: usize,
    last_send_time: time::Instant,
    count: usize,
}

impl BandwidthLimiter {
    fn new(bandwidth: f64, limit: usize) -> Self {
        Self {
            bandwidth: bandwidth,
            limit: limit,
            last_send_time: time::Instant::now(),
            count: 0,
        }
    }

    fn try_send(&mut self, size: usize) -> bool {
        let now = time::Instant::now();

        let tokens = ((now - self.last_send_time).as_secs_f64() * self.bandwidth) as usize;

        self.last_send_time = now;

        if tokens <= self.count {
            self.count -= tokens;
        } else {
            self.count = 0;
        }

        if self.count + size <= self.limit {
            self.count += size;
            return true;
        } else {
            return false;
        }
    }
}

fn router_thread() {
    let socket = net::UdpSocket::bind("127.0.0.1:9001").unwrap();

    // Throttle data to 300kB/s, with a maximum queue size of 20kB
    let server_addr: net::SocketAddr = "127.0.0.1:8888".parse().unwrap();
    let mut server_limiter = BandwidthLimiter::new(300_000.0, 20_000);

    let mut server_bytes_sent = 0;
    let mut server_bytes_dropped = 0;

    let mut client_addr: Option<net::SocketAddr> = None;
    let mut client_limiter = BandwidthLimiter::new(300_000.0, 20_000);

    loop {
        let mut recv_buf = [0; 1500];

        while let Ok((recv_size, src_addr)) = socket.recv_from(&mut recv_buf) {
            let udp_frame_size = recv_size + uflow::UDP_HEADER_SIZE;

            if src_addr.port() == 8888 {
                if let Some(client_addr) = client_addr {
                    if client_limiter.try_send(udp_frame_size) {
                        //println!("client-bound packet sent! size: {}", recv_size);
                        socket.send_to(&recv_buf[..recv_size], client_addr).unwrap();
                    } else {
                        println!("[router] client-bound packet dropped!");
                    }
                }
            } else {
                if client_addr.is_none() {
                    client_addr = Some(src_addr);
                }
                if server_limiter.try_send(udp_frame_size) {
                    socket.send_to(&recv_buf[..recv_size], server_addr).unwrap();
                    server_bytes_sent += udp_frame_size;

                    /*
                    println!("sent! dropped/total: {}/{} ({:.3})",
                        server_bytes_dropped,
                        server_bytes_sent + server_bytes_dropped,
                        server_bytes_dropped as f64 / (server_bytes_sent + server_bytes_dropped) as f64);
                    */
                } else {
                    //println!("server-bound packet dropped!");
                    server_bytes_dropped += udp_frame_size;

                    println!("[router] drop! dropped/total: {}/{} ({:.3})",
                        server_bytes_dropped,
                        server_bytes_sent + server_bytes_dropped,
                        server_bytes_dropped as f64 / (server_bytes_sent + server_bytes_dropped) as f64);
                }
            }
        }
    }
}

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

    // Send data at ~= 6 * 1500 B / 0.015 s = 600kB/s
    let num_steps = 200;
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

            all_data[channel_id].extend_from_slice(&data);
            client.send(data, channel_id, uflow::SendMode::Reliable);

            //println!("[client] sent packet {} on channel {}", packet_id, channel_id);

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

    println!("[client] exiting");

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

#[test]
fn reliable() {
    thread::spawn(router_thread);

    thread::sleep(time::Duration::from_millis(200));

    let server = thread::spawn(server_thread);

    thread::sleep(time::Duration::from_millis(200));

    let client = thread::spawn(client_thread);

    let server_md5s = server.join().unwrap();
    let client_md5s = client.join().unwrap();

    assert_eq!(server_md5s, client_md5s);
}
