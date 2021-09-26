
fn server_thread() {
    let mut host = udpl::host::Host::bind("127.0.0.1:8888", udpl::host::Params::new()).unwrap();
    let mut clients = Vec::new();

    for _ in 0..400 {
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
    let mut host = udpl::host::Host::bind_any(udpl::host::Params::new()).unwrap();
    let mut client = host.connect("127.0.0.1:8888".parse().unwrap());

    for _ in 0..200 {
        host.step();

        for event in client.poll_events() {
            println!("[{:?}] event {:?}", client.address(), event);
        }

        host.flush();

        std::thread::sleep(std::time::Duration::from_millis(15));
    }

    client.disconnect();

    for _ in 0..200 {
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

