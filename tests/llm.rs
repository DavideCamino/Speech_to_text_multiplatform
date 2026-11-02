//! Local mock HTTP server: these tests never need a model or external network.
use speech2text::llm::{self, Connection, Event};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{atomic::Ordering, mpsc},
    thread,
    time::Duration,
};

fn request(socket: &mut TcpStream) -> (String, String) {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    (headers, String::from_utf8(body).unwrap())
}
fn response(socket: &mut TcpStream, status: &str, body: &str) {
    write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}
fn server(
    serve: impl FnOnce(TcpListener) + Send + 'static,
) -> (Connection, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let connection = Connection {
        port: listener.local_addr().unwrap().port(),
        ..Default::default()
    };
    (connection, thread::spawn(move || serve(listener)))
}
fn finish(job: &llm::Job) -> Result<String, String> {
    loop {
        if let Event::Finished(result) = job.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            return result;
        }
    }
}
fn models(listener: &TcpListener) {
    let (mut socket, _) = listener.accept().unwrap();
    let (headers, _) = request(&mut socket);
    assert!(headers.starts_with("GET /v1/models "));
    response(&mut socket, "200 OK", r#"{"data":[{"id":"local-model"}]}"#);
}

#[test]
fn connection_test_authentication_and_discovery() {
    let (connection, server) = server(|listener| {
        let (mut socket, _) = listener.accept().unwrap();
        let (headers, _) = request(&mut socket);
        assert!(
            headers
                .to_lowercase()
                .contains("authorization: bearer test-secret\r\n")
        );
        response(&mut socket, "200 OK", r#"{"data":[{"id":"local-model"}]}"#);
    });
    let job = llm::start(connection, "test-secret".into(), None);
    assert!(finish(&job).unwrap().contains("local-model"));
    server.join().unwrap();
}

#[test]
fn response_is_delivered_before_generation_finishes() {
    let (release, wait) = mpsc::channel();
    let (connection, server) = server(move |listener| {
        models(&listener);
        let (mut socket, _) = listener.accept().unwrap();
        let (headers, body) = request(&mut socket);
        assert!(headers.starts_with("POST /v1/chat/completions "));
        assert!(!headers.to_lowercase().contains("authorization:"));
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "local-model");
        assert_eq!(body["messages"][0]["content"], "Rewrite instructions");
        assert_eq!(body["messages"][1]["content"], "Original text");
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let first = "data: {\"choices\":[{\"delta\":{\"content\":\"# Héllo\"}}]}\n\n";
        for byte in first.as_bytes() {
            socket.write_all(&[*byte]).unwrap();
        }
        socket.flush().unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        socket
            .write_all(
                b"data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\ndata: [DONE]\n\n",
            )
            .unwrap();
    });
    let job = llm::start(
        connection,
        String::new(),
        Some(("Rewrite instructions".into(), "Original text".into())),
    );
    let Event::Text(first) = job.events.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("Expected text before completion");
    };
    assert_eq!(first, "# Héllo");
    release.send(()).unwrap();
    let mut text = first;
    loop {
        match job.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Text(delta) => text.push_str(&delta),
            Event::Finished(result) => {
                result.unwrap();
                break;
            }
        }
    }
    assert_eq!(text, "# Héllo world");
    server.join().unwrap();
}

#[test]
fn cancellation_interrupts_a_stalled_generation() {
    let (ready, received) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (connection, server) = server(move |listener| {
        models(&listener);
        let (mut socket, _) = listener.accept().unwrap();
        request(&mut socket);
        ready.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let job = llm::start(
        connection,
        String::new(),
        Some(("Instructions".into(), "Text".into())),
    );
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    job.cancel.store(true, Ordering::Relaxed);
    assert!(finish(&job).unwrap_err().contains("Cancelled"));
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn auth_failures_redirects_and_empty_model_lists() {
    for (status, body, expected) in [
        ("401 Unauthorized", "private request contents", "401"),
        ("302 Found", "", "302"),
        ("200 OK", "{\"data\":[]}", "no loaded model"),
        ("200 OK", "invalid json", "Invalid model list"),
    ] {
        let (connection, server) = server(move |listener| {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            response(&mut socket, status, body);
        });
        let error = finish(&llm::start(connection, "secret".into(), None)).unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("private request contents"));
        server.join().unwrap();
    }
}

#[test]
fn malformed_and_truncated_streams_fail() {
    for body in [
        "data: invalid\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
    ] {
        let (connection, server) = server(move |listener| {
            models(&listener);
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            response(&mut socket, "200 OK", body);
        });
        assert!(
            finish(&llm::start(
                connection,
                String::new(),
                Some(("prompt".into(), "text".into()))
            ))
            .is_err()
        );
        server.join().unwrap();
    }
}

#[test]
fn unavailable_server_returns_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let connection = Connection {
        port: listener.local_addr().unwrap().port(),
        ..Default::default()
    };
    drop(listener);
    assert!(finish(&llm::start(connection, String::new(), None)).is_err());
}
