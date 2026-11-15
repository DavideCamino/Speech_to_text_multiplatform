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
            Event::Models(_) | Event::Status(_) => {}
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
fn configured_discovery_lists_unloaded_models_without_loading() {
    let (connection, server) = server(|listener| {
        let (mut socket, _) = listener.accept().unwrap();
        assert!(request(&mut socket).0.starts_with("GET /models "));
        response(
            &mut socket,
            "200 OK",
            r#"{"data":[{"id":"first","status":{"value":"unloaded"}},{"id":"second","status":{"value":"loaded"}}]}"#,
        );
    });
    let job = llm::start_configured(connection, String::new(), None, Default::default());
    let Event::Models(models) = job.events.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("Expected model list");
    };
    assert_eq!(models.len(), 2);
    assert!(models[0].needs_load);
    assert!(!models[1].needs_load);
    assert!(finish(&job).is_ok());
    server.join().unwrap();
}

#[test]
fn apply_loads_selected_model_before_rewrite_and_sends_reasoning() {
    for reasoning in llm::Reasoning::ALL {
        let (connection, server) = server(move |listener| {
            let (mut socket, _) = listener.accept().unwrap();
            assert!(request(&mut socket).0.starts_with("GET /models "));
            response(
                &mut socket,
                "200 OK",
                r#"{"data":[{"id":"first"},{"id":"chosen","status":{"value":"unloaded"}}]}"#,
            );
            let (mut socket, _) = listener.accept().unwrap();
            let (headers, body) = request(&mut socket);
            assert!(headers.starts_with("POST /models/load "));
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["model"], "chosen");
            response(&mut socket, "200 OK", r#"{"success":true}"#);
            // Apply waits until loaded; Rewrite independently verifies readiness.
            for status in ["loading", "loaded", "loaded"] {
                let (mut socket, _) = listener.accept().unwrap();
                assert!(request(&mut socket).0.starts_with("GET /models "));
                response(
                    &mut socket,
                    "200 OK",
                    &format!(r#"{{"data":[{{"id":"chosen","status":{{"value":"{status}"}}}}]}}"#),
                );
            }
            let (mut socket, _) = listener.accept().unwrap();
            let (headers, body) = request(&mut socket);
            assert!(headers.starts_with("POST /v1/chat/completions "));
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["model"], "chosen");
            assert_eq!(
                body.get("reasoning_effort").and_then(|v| v.as_str()),
                reasoning.parameter()
            );
            response(
                &mut socket,
                "200 OK",
                "data: {\"choices\":[{\"delta\":{\"content\":\"Result\"}}]}\n\ndata: [DONE]\n\n",
            );
        });
        let options = llm::Options {
            model: Some("chosen".into()),
            reasoning,
        };
        assert!(
            finish(&llm::start_loading(
                connection.clone(),
                String::new(),
                options.clone()
            ))
            .is_ok()
        );
        let job = llm::start_configured(
            connection,
            String::new(),
            Some(("prompt".into(), "text".into())),
            options,
        );
        assert!(finish(&job).is_ok());
        server.join().unwrap();
    }
}

#[test]
fn configured_discovery_falls_back_for_single_model_servers() {
    let (connection, server) = server(|listener| {
        let (mut socket, _) = listener.accept().unwrap();
        assert!(request(&mut socket).0.starts_with("GET /models "));
        response(&mut socket, "404 Not Found", "");
        models(&listener);
    });
    let job = llm::start_configured(connection, String::new(), None, Default::default());
    assert!(finish(&job).is_ok());
    server.join().unwrap();
}

#[test]
fn configured_rewrite_rejects_missing_model_and_load_failures() {
    for available in [false, true] {
        let (connection, server) = server(move |listener| {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            response(
                &mut socket,
                "200 OK",
                if available {
                    r#"{"data":[{"id":"chosen","status":{"value":"unloaded"}}]}"#
                } else {
                    r#"{"data":[{"id":"other"}]}"#
                },
            );
            if available {
                let (mut socket, _) = listener.accept().unwrap();
                assert!(request(&mut socket).0.starts_with("POST /models/load "));
                response(&mut socket, "200 OK", r#"{"success":false}"#);
                let (mut socket, _) = listener.accept().unwrap();
                request(&mut socket);
                response(
                    &mut socket,
                    "200 OK",
                    r#"{"data":[{"id":"chosen","status":{"value":"unloaded"}}]}"#,
                );
            }
        });
        let job = llm::start_loading(
            connection,
            String::new(),
            llm::Options {
                model: Some("chosen".into()),
                ..Default::default()
            },
        );
        let error = finish(&job).unwrap_err();
        assert!(
            error.contains(if available {
                "could not load"
            } else {
                "unavailable"
            }),
            "{error}"
        );
        server.join().unwrap();
    }
}

#[test]
fn apply_waits_for_existing_load_without_loading_twice() {
    let (connection, server) = server(|listener| {
        for status in ["loading", "loading", "loaded"] {
            let (mut socket, _) = listener.accept().unwrap();
            assert!(request(&mut socket).0.starts_with("GET /models "));
            response(
                &mut socket,
                "200 OK",
                &format!(r#"{{"data":[{{"id":"chosen","status":{{"value":"{status}"}}}}]}}"#),
            );
        }
    });
    let job = llm::start_loading(
        connection,
        String::new(),
        llm::Options {
            model: Some("chosen".into()),
            ..Default::default()
        },
    );
    assert!(finish(&job).unwrap().contains("Model ready"));
    server.join().unwrap();
}

#[test]
fn rewrite_does_not_load_unready_models() {
    let (connection, server) = server(|listener| {
        let (mut socket, _) = listener.accept().unwrap();
        assert!(request(&mut socket).0.starts_with("GET /models "));
        response(
            &mut socket,
            "200 OK",
            r#"{"data":[{"id":"chosen","status":{"value":"unloaded"}}]}"#,
        );
    });
    let job = llm::start_configured(
        connection,
        String::new(),
        Some(("prompt".into(), "text".into())),
        llm::Options {
            model: Some("chosen".into()),
            ..Default::default()
        },
    );
    assert!(finish(&job).unwrap_err().contains("Apply"));
    server.join().unwrap();
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
