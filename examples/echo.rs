
// TODO: Actual echo example

fn server_thread() {
    let params = udpl::PeerParams::new()
        .max_tx_bandwidth(1_000_000)
        .max_rx_bandwidth(500_000)
        .tx_channels(2);

    let mut host = udpl::Host::bind("127.0.0.1:8888", 1, params).unwrap();
    let mut clients = Vec::new();

    for _ in 0..220 {
        host.step();

        for client in host.incoming() {
            println!("[{:?}] appeared", client.address());
            clients.push(client);
        }

        for client in clients.iter_mut() {
            for event in client.poll_events() {
                println!("[{:?}] event {:?}", client.address(), event);
            }
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

fn client_thread() {
    let params = udpl::PeerParams::new()
        .max_tx_bandwidth(1_000_000)
        .max_rx_bandwidth(750_000)
        .tx_channels(2)
        .priority_channels(0..1);

    let mut host = udpl::Host::bind_any(1, params).unwrap();
    let mut client = host.connect("127.0.0.1:8888".parse().unwrap());

    for i in 0..200 {
        host.step();

        for event in client.poll_events() {
            println!("[{:?}] event {:?}", client.address(), event);
        }

        let a = i*3 as u16;
        let b = i*3 + 1 as u16;
        let c = i*3 + 2 as u16;

        client.send(Box::new(a.to_be_bytes()), 1, udpl::SendMode::Reliable);
        client.send(Box::new(b.to_be_bytes()), 0, udpl::SendMode::Reliable);
        client.send(Box::new(c.to_be_bytes()), 0, udpl::SendMode::Reliable);

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    client.disconnect();

    for _ in 0..20 {
        host.step();

        for event in client.poll_events() {
            println!("[{:?}] event {:?}", client.address(), event);
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

fn main() {
    let server = std::thread::spawn(server_thread);

    std::thread::sleep(std::time::Duration::from_millis(200));

    let client = std::thread::spawn(client_thread);

    client.join().unwrap();
    server.join().unwrap();
}

