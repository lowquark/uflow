use std::thread;
use std::time;

#[test]
fn client_handshake_timeout() {
    let cfg = Default::default();
    let mut client = uflow::client::Client::connect("127.0.0.1:8888", cfg).unwrap();

    // We expect to see exactly one Error(ErrorType::HandshakeTimeout) within 25 seconds
    let end_time = time::Instant::now() + time::Duration::from_secs(25);
    let mut timeout_seen = false;

    while time::Instant::now() < end_time {
        for event in client.step() {
            match event {
                uflow::client::Event::Error(error) => {
                    assert_eq!(timeout_seen, false);
                    assert_eq!(error, uflow::client::ErrorType::HandshakeTimeout);
                    timeout_seen = true;
                }
                other => panic!("unexpected event: {:?}", other),
            }
        }

        thread::sleep(time::Duration::from_secs(1));
    }

    if !timeout_seen {
        panic!("no timeout event received");
    }
}

#[test]
fn client_active_timeout() {
    thread::spawn(|| {
        let cfg = Default::default();
        let mut server = uflow::server::Server::bind("127.0.0.1:9999", cfg).unwrap();

        loop {
            for event in server.step() {
                match event {
                    uflow::server::Event::Connect(client_address) => {
                        server.drop(&client_address);
                    }
                    _ => (),
                }
            }

            thread::sleep(time::Duration::from_millis(100));
        }
    });

    let client = thread::spawn(|| {
        let cfg = Default::default();
        let mut client = uflow::client::Client::connect("127.0.0.1:9999", cfg).unwrap();

        // We expect to see exactly one Connect and one Error(ErrorType::Timeout) within 25 seconds
        let end_time = time::Instant::now() + time::Duration::from_secs(25);
        let mut connect_seen = false;
        let mut timeout_seen = false;

        while time::Instant::now() < end_time {
            for event in client.step() {
                match event {
                    uflow::client::Event::Connect => {
                        assert_eq!(connect_seen, false);
                        connect_seen = true;
                    }
                    uflow::client::Event::Error(error) => {
                        assert_eq!(timeout_seen, false);
                        assert_eq!(error, uflow::client::ErrorType::Timeout);
                        timeout_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(time::Duration::from_secs(1));
        }

        if !connect_seen {
            panic!("no connect event received");
        }

        if !timeout_seen {
            panic!("no timeout event received");
        }
    });

    client.join().unwrap();
}

#[test]
fn server_handshake_timeout() {
    let server = thread::spawn(|| {
        let cfg = Default::default();
        let mut server = uflow::server::Server::bind("127.0.0.1:7777", cfg).unwrap();

        // We expect to see exactly one Error(_, ErrorType::HandshakeTimeout) within 25 seconds
        let end_time = time::Instant::now() + time::Duration::from_secs(25);
        let mut timeout_seen = false;

        while time::Instant::now() < end_time {
            for event in server.step() {
                match event {
                    uflow::server::Event::Error(_, error) => {
                        assert_eq!(timeout_seen, false);
                        assert_eq!(error, uflow::server::ErrorType::HandshakeTimeout);
                        timeout_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(time::Duration::from_millis(100));
        }

        if !timeout_seen {
            panic!("no timeout event received");
        }
    });

    thread::sleep(time::Duration::from_secs(1));

    // Client::connect sends the first SYN, and this is all the server will receive
    uflow::client::Client::connect("127.0.0.1:7777", Default::default()).unwrap();

    server.join().unwrap();
}

#[test]
fn server_active_timeout() {
    thread::spawn(|| {
        let cfg = Default::default();
        let mut client = uflow::client::Client::connect("127.0.0.1:6666", cfg).unwrap();

        loop {
            for event in client.step() {
                match event {
                    uflow::client::Event::Connect => {
                        return;
                    }
                    _ => (),
                }
            }

            thread::sleep(time::Duration::from_millis(100));
        }
    });

    let server = thread::spawn(|| {
        let cfg = Default::default();
        let mut server = uflow::server::Server::bind("127.0.0.1:6666", cfg).unwrap();

        // We expect to see exactly one Connect and one Error(ErrorType::Timeout) within 25 seconds
        let end_time = time::Instant::now() + time::Duration::from_secs(25);
        let mut connect_seen = false;
        let mut timeout_seen = false;

        while time::Instant::now() < end_time {
            for event in server.step() {
                match event {
                    uflow::server::Event::Connect(_) => {
                        assert_eq!(connect_seen, false);
                        connect_seen = true;
                    }
                    uflow::server::Event::Error(_, error) => {
                        assert_eq!(timeout_seen, false);
                        assert_eq!(error, uflow::server::ErrorType::Timeout);
                        timeout_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(time::Duration::from_secs(1));
        }

        if !connect_seen {
            panic!("no connect event received");
        }

        if !timeout_seen {
            panic!("no timeout event received");
        }
    });

    server.join().unwrap();
}
