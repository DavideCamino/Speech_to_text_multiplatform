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
fn models(listener: &TcpListener, endpoint: &str) {
    let (mut socket, _) = listener.accept().unwrap();
    let (headers, _) = request(&mut socket);
    assert!(headers.starts_with(&format!("GET {endpoint} ")));
    response(&mut socket, "200 OK", r#"{"data":[{"id":"local-model"}]}"#);
}

fn rewrite_with_local_model(connection: Connection, prompt: &str, text: &str) -> llm::Job {
    llm::start_configured(
        connection,
        String::new(),
        Some((prompt.into(), text.into())),
        llm::Options {
            model: Some("local-model".into()),
            ..Default::default()
        },
    )
}

fn first_text(job: &llm::Job) -> String {
    loop {
        match job.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Status(_) => continue,
            Event::Text(text) => return text,
            event => panic!(
                "Expected text before completion, got {}",
                match event {
                    Event::Finished(_) => "Finished",
                    _ => "an unexpected event",
                }
            ),
        }
    }
}

#[test]
fn configured_discovery_sends_authentication() {
    let (connection, server) = server(|listener| {
        let (mut socket, _) = listener.accept().unwrap();
        let (headers, _) = request(&mut socket);
        assert!(headers.starts_with("GET /models "));
        assert!(
            headers
                .to_lowercase()
                .contains("authorization: bearer test-secret\r\n")
        );
        response(&mut socket, "200 OK", r#"{"data":[{"id":"local-model"}]}"#);
    });
    let job = llm::start_configured(connection, "test-secret".into(), None, Default::default());
    let Event::Models(models) = job.events.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("Expected model list");
    };
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "local-model");
    assert!(finish(&job).is_ok());
    server.join().unwrap();
}

#[test]
fn response_is_delivered_before_generation_finishes() {
    let (release, wait) = mpsc::channel();
    let (connection, server) = server(move |listener| {
        models(&listener, "/models");
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
    let job = rewrite_with_local_model(connection, "Rewrite instructions", "Original text");
    let first = first_text(&job);
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
    let (release, wait) = mpsc::channel();
    let (connection, server) = server(move |listener| {
        models(&listener, "/models");
        let (mut socket, _) = listener.accept().unwrap();
        assert!(
            request(&mut socket)
                .0
                .starts_with("POST /v1/chat/completions ")
        );
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n").unwrap();
        socket.flush().unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let job = rewrite_with_local_model(connection, "Instructions", "Text");
    assert_eq!(first_text(&job), "partial");
    job.cancel.store(true, Ordering::Relaxed);
    assert!(finish(&job).unwrap_err().contains("Cancelled"));
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn discovery_rejects_auth_failures_redirects_and_invalid_json() {
    for (status, body, expected) in [
        ("401 Unauthorized", "private request contents", "401"),
        ("302 Found", "", "302"),
        ("200 OK", "invalid json", "Invalid model list"),
    ] {
        let (connection, server) = server(move |listener| {
            let (mut socket, _) = listener.accept().unwrap();
            assert!(request(&mut socket).0.starts_with("GET /models "));
            response(&mut socket, status, body);
        });
        let job = llm::start_configured(connection, "secret".into(), None, Default::default());
        let error = finish(&job).unwrap_err();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("private request contents"));
        server.join().unwrap();
    }
}

#[test]
fn discovery_accepts_an_empty_model_list() {
    let (connection, server) = server(|listener| {
        let (mut socket, _) = listener.accept().unwrap();
        assert!(request(&mut socket).0.starts_with("GET /models "));
        response(&mut socket, "200 OK", r#"{"data":[]}"#);
    });
    let job = llm::start_configured(connection, String::new(), None, Default::default());
    let Event::Models(models) = job.events.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("Expected an empty model list");
    };
    assert!(models.is_empty());
    assert!(finish(&job).is_ok());
    server.join().unwrap();
}

#[test]
fn malformed_and_truncated_streams_fail() {
    for body in [
        "data: invalid\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
    ] {
        let (connection, server) = server(move |listener| {
            models(&listener, "/models");
            let (mut socket, _) = listener.accept().unwrap();
            assert!(
                request(&mut socket)
                    .0
                    .starts_with("POST /v1/chat/completions ")
            );
            response(&mut socket, "200 OK", body);
        });
        assert!(finish(&rewrite_with_local_model(connection, "prompt", "text")).is_err());
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
        models(&listener, "/v1/models");
    });
    let job = llm::start_configured(connection, String::new(), None, Default::default());
    let Event::Models(models) = job.events.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("Expected models from the fallback endpoint");
    };
    assert_eq!(models[0].id, "local-model");
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
    let job = llm::start_configured(connection, String::new(), None, Default::default());
    assert!(finish(&job).is_err());
}

#[test]
#[allow(deprecated)]
fn legacy_start_reports_the_first_model_without_generating() {
    let (connection, server) = server(|listener| models(&listener, "/v1/models"));
    let job = llm::start(connection, String::new(), None);
    assert_eq!(
        finish(&job).unwrap(),
        "Connected. Loaded model: local-model"
    );
    server.join().unwrap();
}

#[test]
#[allow(deprecated)]
fn legacy_start_generates_with_the_first_model() {
    let (connection, server) = server(|listener| {
        let (mut socket, _) = listener.accept().unwrap();
        assert!(request(&mut socket).0.starts_with("GET /v1/models "));
        response(
            &mut socket,
            "200 OK",
            r#"{"data":[{"id":"first"},{"id":"second"}]}"#,
        );
        let (mut socket, _) = listener.accept().unwrap();
        let (headers, body) = request(&mut socket);
        assert!(headers.starts_with("POST /v1/chat/completions "));
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["model"], "first");
        response(
            &mut socket,
            "200 OK",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Result\"}}]}\n\ndata: [DONE]\n\n",
        );
    });
    let job = llm::start(
        connection,
        String::new(),
        Some(("prompt".into(), "text".into())),
    );
    let Event::Text(text) = job.events.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("Expected legacy generation to emit text without a status event");
    };
    assert_eq!(text, "Result");
    assert_eq!(finish(&job).unwrap(), "Rewrite complete.");
    server.join().unwrap();
}
