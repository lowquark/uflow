use std::thread;
use std::time;

static DURATION: time::Duration = time::Duration::from_secs(1);
static STEP_INTERVAL: time::Duration = time::Duration::from_millis(100);

#[test]
fn client_disconnect_now() {
    let server_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut server = uflow::server::Server::bind("127.0.0.1:9999", cfg).unwrap();

        // We expect to see exactly one Connect and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in server.step() {
                match event {
                    uflow::server::Event::Connect(_) => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;
                    }
                    uflow::server::Event::Disconnect(_) => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[server] no connect event received");
        }

        if !disconnect_seen {
            panic!("[server] no disconnect event received");
        }
    });

    thread::sleep(STEP_INTERVAL/2);

    let client_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut client = uflow::client::Client::connect("127.0.0.1:9999", cfg).unwrap();

        // We expect to see exactly one Connect and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in client.step() {
                match event {
                    uflow::client::Event::Connect => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;

                        client.disconnect_now();
                    }
                    uflow::client::Event::Disconnect => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[client] no connect event received");
        }

        if !disconnect_seen {
            panic!("[client] no disconnect event received");
        }
    });

    client_thread.join().unwrap();
    server_thread.join().unwrap();
}

#[test]
fn server_disconnect_now() {
    let server_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut server = uflow::server::Server::bind("127.0.0.1:8888", cfg).unwrap();

        // We expect to see exactly one Connect and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in server.step() {
                match event {
                    uflow::server::Event::Connect(peer_address) => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;

                        server.client(&peer_address).unwrap().borrow_mut().disconnect_now();
                    }
                    uflow::server::Event::Disconnect(_) => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[server] no connect event received");
        }

        if !disconnect_seen {
            panic!("[server] no disconnect event received");
        }
    });

    thread::sleep(STEP_INTERVAL/2);

    let client_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut client = uflow::client::Client::connect("127.0.0.1:8888", cfg).unwrap();

        // We expect to see exactly one Connect and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in client.step() {
                match event {
                    uflow::client::Event::Connect => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;
                    }
                    uflow::client::Event::Disconnect => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[client] no connect event received");
        }

        if !disconnect_seen {
            panic!("[client] no disconnect event received");
        }
    });

    client_thread.join().unwrap();
    server_thread.join().unwrap();
}

#[test]
fn client_disconnect_flush() {
    let server_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut server = uflow::server::Server::bind("127.0.0.1:7777", cfg).unwrap();

        // We expect to see exactly one Connect, one Receive, and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut receive_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in server.step() {
                match event {
                    uflow::server::Event::Connect(_) => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(receive_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;
                    }
                    uflow::server::Event::Receive(_, data) => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(receive_seen, false);
                        assert_eq!(disconnect_seen, false);
                        receive_seen = true;

                        assert_eq!(data, [0, 1, 2, 3].into());
                    }
                    uflow::server::Event::Disconnect(_) => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(receive_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[server] no connect event received");
        }

        if !receive_seen {
            panic!("[server] no receive event received");
        }

        if !disconnect_seen {
            panic!("[server] no disconnect event received");
        }
    });

    thread::sleep(STEP_INTERVAL/2);

    let client_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut client = uflow::client::Client::connect("127.0.0.1:7777", cfg).unwrap();

        // We expect to see exactly one Connect, and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in client.step() {
                match event {
                    uflow::client::Event::Connect => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;

                        client.disconnect();
                        client.send([0, 1, 2, 3].into(), 0, uflow::SendMode::Reliable);
                    }
                    uflow::client::Event::Disconnect => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[client] no connect event received");
        }

        if !disconnect_seen {
            panic!("[client] no disconnect event received");
        }
    });

    client_thread.join().unwrap();
    server_thread.join().unwrap();
}

#[test]
fn server_disconnect_flush() {
    let server_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut server = uflow::server::Server::bind("127.0.0.1:6666", cfg).unwrap();

        // We expect to see exactly one Connect, one Receive, and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in server.step() {
                match event {
                    uflow::server::Event::Connect(peer_address) => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;

                        let mut client = server.client(&peer_address).unwrap().borrow_mut();
                        client.disconnect();
                        client.send([0, 1, 2, 3].into(), 0, uflow::SendMode::Reliable);
                    }
                    uflow::server::Event::Disconnect(_) => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[server] no connect event received");
        }

        if !disconnect_seen {
            panic!("[server] no disconnect event received");
        }
    });

    thread::sleep(STEP_INTERVAL/2);

    let client_thread = thread::spawn(|| {
        let cfg = Default::default();
        let mut client = uflow::client::Client::connect("127.0.0.1:6666", cfg).unwrap();

        // We expect to see exactly one Connect, one Receive, and one Disconnect within DURATION
        let mut connect_seen = false;
        let mut receive_seen = false;
        let mut disconnect_seen = false;
        let end_time = time::Instant::now() + DURATION;

        while time::Instant::now() < end_time {
            for event in client.step() {
                match event {
                    uflow::client::Event::Connect => {
                        assert_eq!(connect_seen, false);
                        assert_eq!(receive_seen, false);
                        assert_eq!(disconnect_seen, false);
                        connect_seen = true;
                    }
                    uflow::client::Event::Receive(data) => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(receive_seen, false);
                        assert_eq!(disconnect_seen, false);
                        receive_seen = true;

                        assert_eq!(data, [0, 1, 2, 3].into());
                    }
                    uflow::client::Event::Disconnect => {
                        assert_eq!(connect_seen, true);
                        assert_eq!(receive_seen, true);
                        assert_eq!(disconnect_seen, false);
                        disconnect_seen = true;
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }

            thread::sleep(STEP_INTERVAL);
        }

        if !connect_seen {
            panic!("[client] no connect event received");
        }

        if !disconnect_seen {
            panic!("[client] no disconnect event received");
        }
    });

    client_thread.join().unwrap();
    server_thread.join().unwrap();
}
