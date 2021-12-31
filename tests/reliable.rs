
use std::net;
use std::time;

extern crate md5;

static NUM_CHANNELS: u32 = 4;

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

        //println!("count/limit: {}/{}", self.count, self.limit);

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
    let mut server_limiter = BandwidthLimiter::new(600_000.0, 100_000);

    let mut server_bytes_sent = 0;
    let mut server_bytes_dropped = 0;

    let mut client_addr: Option<net::SocketAddr> = None;
    let mut client_limiter = BandwidthLimiter::new(600_000.0, 100_000);

    loop {
        let mut recv_buf = [0; 1500];

        while let Ok((recv_size, src_addr)) = socket.recv_from(&mut recv_buf) {
            let udp_frame_size = recv_size + udpl::UDP_HEADER_SIZE;

            if src_addr.port() == 8888 {
                if let Some(client_addr) = client_addr {
                    if client_limiter.try_send(udp_frame_size) {
                        //println!("client-bound packet sent! size: {}", recv_size);
                        socket.send_to(&recv_buf[..recv_size], client_addr).unwrap();
                    } else {
                        println!("client-bound packet dropped!");
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

                    println!("drop! dropped/total: {}/{} ({:.3})",
                        server_bytes_dropped,
                        server_bytes_sent + server_bytes_dropped,
                        server_bytes_dropped as f64 / (server_bytes_sent + server_bytes_dropped) as f64);
                }
            }
        }
    }
}

fn server_thread() -> Vec<md5::Digest> {
    let params = udpl::host::Params::new()
        .num_channels(NUM_CHANNELS);

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
                    }
                    udpl::host::Event::Receive(data, channel_id) => {
                        all_data[channel_id as usize].extend_from_slice(&data);
                    }
                    udpl::host::Event::Disconnect => {
                        break 'outer;
                    }
                    udpl::host::Event::Timeout => {
                    }
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
        .max_peer_tx_bandwidth(10_000_000)
        .num_channels(NUM_CHANNELS);

    let mut host = udpl::host::Host::bind_any(params).unwrap();
    let mut client = host.connect("127.0.0.1:9001".parse().unwrap());

    // Send data at 654.2kB/s
    let num_steps = 500;
    let packets_per_step = 20;
    let packet_size = udpl::MTU/3;

    let mut all_data: Vec<Vec<u8>> = vec![Vec::new(); NUM_CHANNELS as usize];

    for _ in 0..num_steps {
        host.step();

        for _ in client.poll_events() {
        }

        for _ in 0..packets_per_step {
            let data = (0..packet_size).map(|_| rand::random::<u8>()).collect::<Vec<_>>().into_boxed_slice();

            let channel_id = rand::random::<u8>() % NUM_CHANNELS as u8;

            let mode = udpl::SendMode::Reliable;

            all_data[channel_id as usize].extend_from_slice(&data);
            client.send(data, channel_id, mode);
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    client.disconnect();

    'outer: loop {
        host.step();

        for event in client.poll_events() {
            match event {
                udpl::host::Event::Connect => {
                }
                udpl::host::Event::Receive(_, _) => {
                }
                udpl::host::Event::Disconnect => {
                    break 'outer;
                }
                udpl::host::Event::Timeout => {
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    return all_data.into_iter().map(|data| md5::compute(data)).collect();
}

#[test]
fn test_reliable_transfer() {
    std::thread::spawn(router_thread);

    std::thread::sleep(std::time::Duration::from_millis(200));

    let server = std::thread::spawn(server_thread);

    std::thread::sleep(std::time::Duration::from_millis(200));

    let client = std::thread::spawn(client_thread);

    let server_md5s = server.join().unwrap();
    let client_md5s = client.join().unwrap();

    assert_eq!(server_md5s, client_md5s);
}

