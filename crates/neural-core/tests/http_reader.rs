use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::{Duration, Instant},
};

use neural_core::{NeuralError, ReaderClient};

fn serve(responses: Vec<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let address = listener.local_addr().expect("test server address");

    thread::spawn(move || {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        }
    });

    format!("http://{address}")
}

fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status}\r\nConnection: close\r\n").into_bytes();
    for (name, value) in headers {
        bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(body);
    bytes
}

#[test]
fn follows_relative_redirect_and_records_final_url() {
    let server = serve(vec![
        response("302 Found", &[("Location", "/article")], b""),
        response(
            "200 OK",
            &[("Content-Type", "text/html; charset=utf-8")],
            b"<html><head><title>Final</title></head><body><article><p>Conteudo final suficientemente longo para leitura.</p></article></body></html>",
        ),
    ]);

    let article = ReaderClient::new(3, 64 * 1024)
        .fetch(&format!("{server}/start"))
        .expect("reader should follow redirect");

    assert_eq!(article.title, "Final");
    assert!(article.source_url.ends_with("/article"));
}

#[test]
fn enforces_redirect_limit() {
    let mut responses = Vec::new();
    for index in 0..6 {
        responses.push(response(
            "302 Found",
            &[("Location", &format!("/r{}", index + 1))],
            b"",
        ));
    }
    let server = serve(responses);

    let error = ReaderClient::new(3, 64 * 1024)
        .fetch(&format!("{server}/r0"))
        .expect_err("six redirects must be rejected");

    assert!(matches!(error, NeuralError::RedirectLimit));
}

#[test]
fn rejects_declared_oversized_body_before_reading_it() {
    let server = serve(vec![response(
        "200 OK",
        &[("Content-Type", "text/html"), ("Content-Length", "1048576")],
        b"",
    )]);

    let error = ReaderClient::new(3, 128)
        .fetch(&server)
        .expect_err("declared oversized response must fail");

    assert!(matches!(
        error,
        NeuralError::ResponseTooLarge {
            declared: 1_048_576,
            limit: 128
        }
    ));
}

#[test]
fn pdf_requires_content_type_and_signature() {
    let missing_type = serve(vec![response("200 OK", &[], b"%PDF-1.7\nmock")]);
    assert!(matches!(
        ReaderClient::new(3, 64 * 1024).fetch_document(
            &missing_type,
            "application/pdf",
            64 * 1024,
            &|| false
        ),
        Err(NeuralError::UnsupportedContentType(_))
    ));

    let fake_pdf = serve(vec![response(
        "200 OK",
        &[("Content-Type", "application/pdf")],
        b"<html>not a pdf</html>",
    )]);
    assert!(matches!(
        ReaderClient::new(3, 64 * 1024).fetch_document(
            &fake_pdf,
            "application/pdf",
            64 * 1024,
            &|| false
        ),
        Err(NeuralError::UnsupportedContentType(_))
    ));
}

#[test]
fn rejects_streamed_body_over_limit() {
    let body = format!(
        "<html><body><article><p>{}</p></article></body></html>",
        "x".repeat(4096)
    );
    let server = serve(vec![response(
        "200 OK",
        &[("Content-Type", "text/html")],
        body.as_bytes(),
    )]);

    assert!(ReaderClient::new(3, 256).fetch(&server).is_err());
}

#[test]
fn decodes_declared_legacy_charset() {
    let mut body = b"<html><head><title>Caf".to_vec();
    body.push(0xE9);
    body.extend_from_slice(
        b"</title></head><body><article><p>Conteudo suficientemente longo para leitura e teste.</p></article></body></html>",
    );
    let server = serve(vec![response(
        "200 OK",
        &[("Content-Type", "text/html; charset=windows-1252")],
        &body,
    )]);

    let article = ReaderClient::new(3, 64 * 1024)
        .fetch(&server)
        .expect("charset should decode");

    assert_eq!(article.title, "Café");
}

#[test]
fn rejects_explicit_non_html_content_type() {
    let server = serve(vec![response(
        "200 OK",
        &[("Content-Type", "text/plain")],
        b"plain text",
    )]);

    assert!(matches!(
        ReaderClient::new(3, 64 * 1024).fetch(&server),
        Err(NeuralError::UnsupportedContentType(_))
    ));
}

#[test]
fn deadline_bounds_entire_navigation() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let address = listener.local_addr().unwrap();

    thread::spawn(move || {
        if let Ok((mut first, _)) = listener.accept() {
            let mut request = [0u8; 1024];
            let _ = first.read(&mut request);
            let _ = first
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: /slow\r\nConnection: close\r\n\r\n");
        }
        if let Ok((mut second, _)) = listener.accept() {
            let mut request = [0u8; 1024];
            let _ = second.read(&mut request);
            thread::sleep(Duration::from_millis(1500));
            let _ = second.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<article><p>late response with enough content for extraction</p></article>",
            );
        }
    });

    let started = Instant::now();
    let result = ReaderClient::new(1, 64 * 1024).fetch(&format!("http://{address}/start"));

    assert!(result.is_err());
    assert!(
        started.elapsed() < Duration::from_millis(1400),
        "deadline should apply across redirects, elapsed={:?}",
        started.elapsed()
    );
}
