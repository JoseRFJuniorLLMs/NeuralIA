//! Gates do transporte de IA (infra-llm-transport). Correm sobre o codigo que
//! embarca: os mesmos construtores, leitores e `ApiClient`, apontados a um
//! servidor stub em 127.0.0.1 (`Endpoint::loopback`, que so existe nos
//! testes). Nenhum teste sai da maquina.
//!
//! Os ids de modelo dos testes vivem em
//! `tests/fixtures/llm/pick-rule-models.json`, fora de `src/llm/`: nem os
//! testes escrevem um id aqui (gate `no_model_id_is_hard_coded`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::errors::ApiError;
use super::gemini::{self, PageToken, TextPrompt};
use super::models::{self, ModelId, PickRule, PickSource, PriceTier, Purpose};
use crate::security::Locality;

use super::transport::{
    ApiClient, ApiCredential, DEFAULT_MAX_BODY_BYTES, Endpoint, GEMINI_HOST, GENERATION_TIMEOUT,
    LIST_TIMEOUT, Provider,
};

// Com a forma de uma chave da Gemini, montada com `concat!` para o texto do
// codigo nao ter a forma de uma chave (o scanner de segredos do PR). Nao e
// real.
const TEST_KEY: &str = concat!("AIza", "SyTESTONLY-llm-transport_0123456789");
const BODY_SENTINEL: &str = "BODY-SENTINEL-4b1d";
const FIXTURE: &str = include_str!("../../tests/fixtures/llm/pick-rule-models.json");
const ANSWER: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Olá"}]},"finishReason":"STOP"}]}"#;

struct TestKey;

impl ApiCredential for TestKey {
    fn secret(&self) -> &str {
        TEST_KEY
    }
}

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("the pick-rule fixture is JSON")
}

fn model() -> ModelId {
    ModelId::parse("test-model").expect("a valid test id")
}

fn prompt() -> TextPrompt<'static> {
    TextPrompt {
        system: Some("Traduza para pt-BR."),
        user: "Hello",
        temperature: Some(0.2),
        max_output_tokens: Some(256),
        json_output: false,
        response_schema: None,
    }
}

// ------------------------------------------------------------ stub

/// Um servidor HTTP/1.1 em 127.0.0.1: responde a cada ligacao com a
/// resposta seguinte da lista e guarda o pedido bruto antes de responder.
struct Stub {
    port: u16,
    requests: mpsc::Receiver<Vec<u8>>,
}

impl Stub {
    fn serve(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub");
        let port = listener.local_addr().expect("stub address").port();
        let (sender, requests) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let request = read_request(&mut stream);
                let _ = sender.send(request);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        Self { port, requests }
    }

    fn client(&self) -> ApiClient {
        ApiClient::new(Endpoint::loopback(self.port))
    }

    /// Os pedidos que chegaram ate agora. O stub guarda o pedido antes de
    /// responder, por isso, quando a chamada volta, os dela ja aqui estao.
    fn received(&self) -> Vec<Vec<u8>> {
        self.requests.try_iter().collect()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = find(&request, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                break;
            }
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => request.extend_from_slice(&chunk[..read]),
        }
    }
    request
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

fn json_response(status: &str, body: &str) -> Vec<u8> {
    response(
        status,
        &[
            ("Content-Type", "application/json"),
            ("Content-Length", &body.len().to_string()),
        ],
        body.as_bytes(),
    )
}

fn request_line(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .split("\r\n")
        .next()
        .unwrap_or("")
        .to_string()
}

// ------------------------------------------------------------ transporte

#[test]
fn a_302_is_not_followed() {
    // GET (a listagem) com 302 e POST (gerar) com 307, que manda repetir o
    // metodo e o corpo: nenhum dos dois se segue. Seguir levava o cabecalho
    // com a chave ao destino que a resposta escolhesse.
    let listing = Stub::serve(vec![
        response(
            "302 Found",
            &[
                ("Location", "/v1beta/models?pageSize=1000&moved=1"),
                ("Content-Length", "0"),
            ],
            b"",
        ),
        json_response("200 OK", r#"{"models":[]}"#),
    ]);
    let listed = gemini::list_models(&listing.client(), Some(&TestKey), &|| false);
    assert_eq!(
        listed,
        Err(ApiError::ServiceUnavailable { status: Some(302) })
    );
    assert_eq!(
        listing.received().len(),
        1,
        "the 302 target must never be requested"
    );

    let generation = Stub::serve(vec![
        response(
            "307 Temporary Redirect",
            &[("Location", "/elsewhere"), ("Content-Length", "0")],
            b"",
        ),
        json_response("200 OK", ANSWER),
    ]);
    let generated = gemini::generate_content(
        &generation.client(),
        &model(),
        &prompt(),
        Some(&TestKey),
        &|| false,
    );
    assert_eq!(
        generated,
        Err(ApiError::ServiceUnavailable { status: Some(307) })
    );
    assert_eq!(generation.received().len(), 1);
}

#[test]
fn a_2_mib_body_gives_too_large() {
    let two_mib = 2 * 1024 * 1024;
    let big = format!(
        r#"{{"candidates":[{{"content":{{"parts":[{{"text":"{}"}}]}}}}]}}"#,
        "a".repeat(two_mib)
    );
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(big.as_bytes()).expect("gzip");
    let gzipped = encoder.finish().expect("gzip");
    assert!(
        gzipped.len() < 64 * 1024,
        "the bomb must be small on the wire: {} bytes",
        gzipped.len()
    );

    let cases = [
        (
            "declared Content-Length",
            response(
                "200 OK",
                &[
                    ("Content-Type", "application/json"),
                    ("Content-Length", &two_mib.to_string()),
                ],
                b"",
            ),
        ),
        (
            "streamed without Content-Length",
            response(
                "200 OK",
                &[("Content-Type", "application/json")],
                big.as_bytes(),
            ),
        ),
        // So a contagem do laco de leitura trava este: o `.limit()` do ureq
        // conta os bytes comprimidos.
        (
            "gzip bomb (decoded 2 MiB)",
            response(
                "200 OK",
                &[
                    ("Content-Type", "application/json"),
                    ("Content-Encoding", "gzip"),
                    ("Content-Length", &gzipped.len().to_string()),
                ],
                &gzipped,
            ),
        ),
    ];
    for (label, answer) in cases {
        let stub = Stub::serve(vec![answer]);
        // So o tamanho do texto: uma falha nao despeja 2 MiB no log.
        let result =
            gemini::generate_content(&stub.client(), &model(), &prompt(), Some(&TestKey), &|| {
                false
            })
            .map(|generated| generated.text.len());
        assert_eq!(
            result,
            Err(ApiError::TooLarge {
                limit: DEFAULT_MAX_BODY_BYTES as u64
            }),
            "{label}"
        );
    }
    assert_eq!(DEFAULT_MAX_BODY_BYTES, 1024 * 1024);
}

#[test]
fn the_key_is_never_in_the_request_line() {
    let stub = Stub::serve(vec![
        json_response("200 OK", r#"{"models":[]}"#),
        json_response("200 OK", ANSWER),
    ]);
    let client = stub.client();
    gemini::list_models(&client, Some(&TestKey), &|| false).expect("list");
    let generated =
        gemini::generate_content(&client, &model(), &prompt(), Some(&TestKey), &|| false)
            .expect("generate");
    assert_eq!(generated.text, "Olá");

    let requests = stub.received();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        request_line(&requests[0]),
        "GET /v1beta/models?pageSize=1000 HTTP/1.1"
    );
    assert_eq!(
        request_line(&requests[1]),
        "POST /v1beta/models/test-model:generateContent HTTP/1.1"
    );
    let auth_line = format!("x-goog-api-key: {TEST_KEY}");
    let user_agent = format!("user-agent: NeuralIA/{}", env!("CARGO_PKG_VERSION"));
    for raw in &requests {
        let text = String::from_utf8_lossy(raw);
        let (head, body) = text.split_once("\r\n\r\n").expect("a complete request");
        let mut lines = head.split("\r\n");
        let line = lines.next().expect("request line");
        assert!(!line.contains(TEST_KEY), "key in the request line: {line}");
        assert!(
            !line.to_ascii_lowercase().contains("key="),
            "key parameter in the request line: {line}"
        );
        let headers: Vec<&str> = lines.collect();
        let carrying: Vec<&&str> = headers
            .iter()
            .filter(|header| header.contains(TEST_KEY))
            .collect();
        assert_eq!(
            carrying.len(),
            1,
            "the key travels in exactly one header: {carrying:?}"
        );
        assert!(
            carrying[0].eq_ignore_ascii_case(&auth_line),
            "the key travels only in x-goog-api-key"
        );
        assert!(!body.contains(TEST_KEY), "key in the body");
        assert!(
            headers
                .iter()
                .any(|header| header.eq_ignore_ascii_case(&user_agent)),
            "User-Agent NeuralIA/<version>: {headers:?}"
        );
    }
}

/// Uma linha da tabela: a linha de estado, cabecalhos a mais, o corpo e o
/// erro esperado.
type StatusCase = (&'static str, Vec<(&'static str, String)>, String, ApiError);

/// A tabela de estados: cada corpo leva a chave e o sentinela, para o mesmo
/// quadro servir o `status_mapping` e o `errors_never_carry_key_or_body`.
fn status_cases() -> Vec<StatusCase> {
    let gemini_error = |code: u16, message: &str, status: &str, detail: &str| {
        format!(
            r#"{{"error":{{"code":{code},"message":"{message} {TEST_KEY} {BODY_SENTINEL}","status":"{status}","details":[{detail}]}}}}"#
        )
    };
    // Uma quota da Gemini esgotada, com a forma dos corpos reais (tirada de
    // relatos publicos, nao de uma chamada): o texto fala de faturacao tanto
    // na quota por minuto como na diaria, as duas trazem `RetryInfo`, e so o
    // `quotaId` do `QuotaFailure` as distingue.
    let gemini_quota = |quota_id: &str, retry_delay: &str| {
        gemini_error(
            429,
            "You exceeded your current quota, please check your plan and billing details.",
            "RESOURCE_EXHAUSTED",
            &format!(
                r#"{{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[{{"quotaMetric":"generativelanguage.googleapis.com/generate_content_free_tier_requests","quotaId":"{quota_id}","quotaDimensions":{{"location":"global","model":"test-model"}},"quotaValue":"10"}}]}},{{"@type":"type.googleapis.com/google.rpc.Help","links":[{{"description":"Learn more about Gemini API quotas"}}]}},{{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"{retry_delay}"}}"#
            ),
        )
    };
    let plain = format!(r#"{{"error":{{"message":"{TEST_KEY} {BODY_SENTINEL}"}}}}"#);
    vec![
        (
            "400 Bad Request",
            vec![],
            gemini_error(
                400,
                "API key not valid. Please pass a valid API key.",
                "INVALID_ARGUMENT",
                r#"{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID"}"#,
            ),
            ApiError::KeyRejected { status: 400 },
        ),
        (
            "401 Unauthorized",
            vec![],
            plain.clone(),
            ApiError::KeyRejected { status: 401 },
        ),
        (
            "403 Forbidden",
            vec![],
            gemini_error(403, "Permission denied.", "PERMISSION_DENIED", "{}"),
            ApiError::KeyRejected { status: 403 },
        ),
        (
            "403 Forbidden",
            vec![],
            gemini_error(403, "Billing is not enabled.", "PERMISSION_DENIED", "{}"),
            ApiError::NoCredits { status: 403 },
        ),
        (
            "400 Bad Request",
            vec![],
            format!(
                r#"{{"type":"error","error":{{"type":"invalid_request_error","message":"Your credit balance is too low to access the API. {TEST_KEY} {BODY_SENTINEL}"}}}}"#
            ),
            ApiError::NoCredits { status: 400 },
        ),
        (
            "402 Payment Required",
            vec![],
            plain.clone(),
            ApiError::NoCredits { status: 402 },
        ),
        (
            "429 Too Many Requests",
            vec![],
            format!(
                r#"{{"error":{{"message":"You exceeded your current quota, please check your plan and billing details. {TEST_KEY} {BODY_SENTINEL}","type":"insufficient_quota","code":"insufficient_quota"}}}}"#
            ),
            ApiError::NoCredits { status: 429 },
        ),
        // A quota por minuto da Gemini passa sozinha: "muitos pedidos" com a
        // espera do `retryDelay`, embora o texto fale de faturacao.
        (
            "429 Too Many Requests",
            vec![],
            gemini_quota(
                "GenerateRequestsPerMinutePerProjectPerModel-FreeTier",
                "37s",
            ),
            ApiError::RateLimited {
                retry_after_secs: Some(37),
            },
        ),
        // A diaria so volta no dia seguinte: os 49 s do `retryDelay` nao
        // chegam ao cartao.
        (
            "429 Too Many Requests",
            vec![],
            gemini_quota("GenerateRequestsPerDayPerProjectPerModel-FreeTier", "49s"),
            ApiError::NoCredits { status: 429 },
        ),
        (
            "429 Too Many Requests",
            vec![("Retry-After", "7".to_string())],
            plain.clone(),
            ApiError::RateLimited {
                retry_after_secs: Some(7),
            },
        ),
        (
            "429 Too Many Requests",
            vec![("Retry-After", "999999".to_string())],
            plain.clone(),
            ApiError::RateLimited {
                retry_after_secs: Some(3600),
            },
        ),
        (
            "429 Too Many Requests",
            vec![],
            gemini_error(
                429,
                "Resource has been exhausted.",
                "RESOURCE_EXHAUSTED",
                r#"{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"12.2s"}"#,
            ),
            ApiError::RateLimited {
                retry_after_secs: Some(13),
            },
        ),
        (
            "429 Too Many Requests",
            vec![],
            plain.clone(),
            ApiError::RateLimited {
                retry_after_secs: None,
            },
        ),
        (
            "404 Not Found",
            vec![],
            gemini_error(404, "Model is not found.", "NOT_FOUND", "{}"),
            ApiError::ModelUnavailable { status: 404 },
        ),
        (
            "400 Bad Request",
            vec![],
            gemini_error(400, "Invalid JSON payload.", "INVALID_ARGUMENT", "{}"),
            ApiError::Rejected { status: 400 },
        ),
        (
            "409 Conflict",
            vec![],
            plain.clone(),
            ApiError::Rejected { status: 409 },
        ),
        (
            "408 Request Timeout",
            vec![],
            plain.clone(),
            ApiError::Timeout,
        ),
        (
            "504 Gateway Timeout",
            vec![],
            plain.clone(),
            ApiError::Timeout,
        ),
        (
            "500 Internal Server Error",
            vec![],
            plain.clone(),
            ApiError::ServiceUnavailable { status: Some(500) },
        ),
        (
            "503 Service Unavailable",
            vec![],
            gemini_error(503, "The model is overloaded.", "UNAVAILABLE", "{}"),
            ApiError::ServiceUnavailable { status: Some(503) },
        ),
        (
            "529 Overloaded",
            vec![],
            plain,
            ApiError::ServiceUnavailable { status: Some(529) },
        ),
    ]
}

fn serve_status(status: &str, headers: &[(&'static str, String)], body: &str) -> Stub {
    let length = body.len().to_string();
    let mut all = vec![
        ("Content-Type", "application/json"),
        ("Content-Length", length.as_str()),
    ];
    all.extend(headers.iter().map(|(name, value)| (*name, value.as_str())));
    Stub::serve(vec![response(status, &all, body.as_bytes())])
}

#[test]
fn status_mapping() {
    for (status, headers, body, expected) in status_cases() {
        let stub = serve_status(status, &headers, &body);
        let result =
            gemini::generate_content(&stub.client(), &model(), &prompt(), Some(&TestKey), &|| {
                false
            });
        assert_eq!(result, Err(expected), "{status} {headers:?}");
    }

    let messages = [
        (ApiError::KeyRejected { status: 401 }, "Chave recusada"),
        (
            ApiError::NoCredits { status: 402 },
            "Sem créditos ou limite de gastos",
        ),
        (
            ApiError::RateLimited {
                retry_after_secs: Some(7),
            },
            "Muitos pedidos, tente em 7 s",
        ),
        (
            ApiError::ModelUnavailable { status: 404 },
            "Modelo indisponível, escolha outro",
        ),
        (
            ApiError::ServiceUnavailable { status: None },
            "Serviço indisponível",
        ),
        (ApiError::Timeout, "Demorou demais"),
        (ApiError::Incomplete, "Resposta incompleta"),
    ];
    for (error, message) in messages {
        assert_eq!(error.pt_br_message(), message);
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn errors_never_carry_key_or_body() {
    let mut errors = Vec::new();
    for (status, headers, body, _) in status_cases() {
        assert!(body.contains(TEST_KEY) && body.contains(BODY_SENTINEL));
        let stub = serve_status(status, &headers, &body);
        errors.push(
            gemini::generate_content(&stub.client(), &model(), &prompt(), Some(&TestKey), &|| {
                false
            })
            .expect_err(status),
        );
    }
    // Respostas 2xx estragadas que tambem levam a chave e o sentinela.
    for body in [
        format!(r#"{{"candidates":[{{"content":{{"parts":[{{"text":"{TEST_KEY} {BODY_SENTINEL}"#),
        format!(r#"{{"promptFeedback":{{"blockReason":"{TEST_KEY} {BODY_SENTINEL}"}}}}"#),
        format!(
            r#"{{"candidates":[{{"content":{{"parts":[]}},"finishReason":"{BODY_SENTINEL}"}}]}}"#
        ),
    ] {
        let stub = Stub::serve(vec![json_response("200 OK", &body)]);
        errors.push(
            gemini::generate_content(&stub.client(), &model(), &prompt(), Some(&TestKey), &|| {
                false
            })
            .expect_err("a broken answer"),
        );
    }
    let page = format!(r#"{{"models":[],"nextPageToken":"{TEST_KEY} {BODY_SENTINEL}"}}"#);
    let stub = Stub::serve(vec![json_response("200 OK", &page)]);
    errors.push(
        gemini::list_models(&stub.client(), Some(&TestKey), &|| false)
            .expect_err("a page token with a space"),
    );

    assert!(errors.contains(&ApiError::Incomplete));
    for error in &errors {
        for text in [
            error.to_string(),
            format!("{error:?}"),
            error.pt_br_message(),
        ] {
            assert!(!text.contains(TEST_KEY), "the key leaked: {text}");
            assert!(!text.contains(BODY_SENTINEL), "the body leaked: {text}");
            assert!(
                !text.contains("API key not valid"),
                "the body leaked: {text}"
            );
        }
    }
}

#[test]
fn a_call_stops_at_its_own_deadline() {
    assert_eq!(GENERATION_TIMEOUT, Duration::from_secs(60));
    assert_eq!(LIST_TIMEOUT, Duration::from_secs(15));
    assert_eq!(
        gemini::generate_content_request(&model(), &prompt()).timeout(),
        GENERATION_TIMEOUT
    );
    assert_eq!(gemini::list_models_request(None).timeout(), LIST_TIMEOUT);

    // Um servidor que aceita, le e nao responde; fecha so ao fim de 3 s. Sem
    // o prazo a chamada acabava com a ligacao fechada, nao com `Timeout`.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = read_request(&mut stream);
            thread::sleep(Duration::from_secs(3));
        }
    });
    let mut request = gemini::generate_content_request(&model(), &prompt());
    request.timeout = Duration::from_millis(400);
    let result = ApiClient::new(Endpoint::loopback(port)).send(&request, Some(&TestKey), &|| false);
    assert_eq!(result, Err(ApiError::Timeout));
}

#[test]
fn cancel_is_cooperative() {
    // Antes do pedido: nada chega ao servidor.
    let stub = Stub::serve(vec![json_response("200 OK", ANSWER)]);
    let result =
        gemini::generate_content(&stub.client(), &model(), &prompt(), Some(&TestKey), &|| {
            true
        });
    assert_eq!(result, Err(ApiError::Cancelled));
    assert!(stub.received().is_empty());

    // Entre blocos do corpo: o servidor manda metade, liga a desistencia e
    // so manda o resto 300 ms depois.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let flag = Arc::new(AtomicBool::new(false));
    let server_flag = Arc::clone(&flag);
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = read_request(&mut stream);
        let half = ANSWER.len() / 2;
        let head = format!(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            ANSWER.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&ANSWER.as_bytes()[..half]);
        let _ = stream.flush();
        server_flag.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(300));
        let _ = stream.write_all(&ANSWER.as_bytes()[half..]);
    });
    let cancelled = || flag.load(Ordering::SeqCst);
    let result = gemini::generate_content(
        &ApiClient::new(Endpoint::loopback(port)),
        &model(),
        &prompt(),
        Some(&TestKey),
        &cancelled,
    );
    assert_eq!(result, Err(ApiError::Cancelled));
}

// ------------------------------------------------------------ localidade

/// infra-llm-untrusted: o host fixado e `Public` e o agente resolve com o
/// `PublicResolver`. A mesma origem local resolvida como `Public` (ou `Lan`)
/// nunca chega a ligar: o endereco 127.0.0.1 sai no resolvedor, antes de
/// qualquer byte (e da chave). Controlo: como `Loopback`, chega ao stub.
#[test]
fn the_pinned_transport_resolves_only_public_addresses() {
    assert_eq!(
        Endpoint::pinned(Provider::Gemini).locality(),
        Locality::Public
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the trap");
    listener.set_nonblocking(true).expect("non-blocking trap");
    let port = listener.local_addr().expect("trap address").port();
    for locality in [Locality::Public, Locality::Lan] {
        let client = ApiClient::new(Endpoint::loopback(port).resolved_as(locality));
        let sent = client.send(&gemini::list_models_request(None), Some(&TestKey), &|| {
            false
        });
        assert_eq!(
            sent,
            Err(ApiError::ServiceUnavailable { status: None }),
            "{locality:?} must refuse the loopback address"
        );
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "{locality:?} opened a connection to 127.0.0.1"
        );
    }

    let stub = Stub::serve(vec![json_response("200 OK", r#"{"models":[]}"#)]);
    let reached = gemini::list_models(&stub.client(), Some(&TestKey), &|| false);
    assert_eq!(reached.map(|models| models.len()), Ok(0));
    assert_eq!(
        stub.received().len(),
        1,
        "the Loopback control reaches the stub"
    );
}

// ------------------------------------------------------------ construtores

#[test]
fn pinned_urls_stay_on_the_pinned_host() {
    let endpoint = Endpoint::pinned(Provider::Gemini);
    let hostile = PageToken::parse("x/../@evil.example:1/?a=b&c#frag").expect("ascii token");
    let requests = [
        gemini::list_models_request(None),
        gemini::list_models_request(Some(&hostile)),
        gemini::generate_content_request(&model(), &prompt()),
    ];
    for request in &requests {
        let url = url::Url::parse(&endpoint.url(request.path())).expect("a valid URL");
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some(GEMINI_HOST));
        assert_eq!(url.port(), None);
        assert_eq!(url.username(), "");
        assert_eq!(url.fragment(), None);
        assert!(url.path().starts_with("/v1beta/models"), "{url}");
    }
    let paged = url::Url::parse(&endpoint.url(requests[1].path())).expect("URL");
    let query: BTreeMap<String, String> = paged.query_pairs().into_owned().collect();
    assert_eq!(
        query.get("pageToken").map(String::as_str),
        Some("x/../@evil.example:1/?a=b&c#frag")
    );
    assert_eq!(query.get("pageSize").map(String::as_str), Some("1000"));
    assert_eq!(query.len(), 2);

    // Ids que partiriam o caminho nunca viram `ModelId`.
    let long = "a".repeat(129);
    for id in [
        "",
        "../x",
        "a/b",
        "a?b",
        "a#b",
        "a:b",
        "a b",
        "a%2fb",
        "a..b",
        "Upper",
        "-a",
        long.as_str(),
    ] {
        assert!(ModelId::parse(id).is_none(), "{id:?}");
    }
    for token in ["", "a b", "a\nb", "á"] {
        assert!(PageToken::parse(token).is_none(), "{token:?}");
    }
}

#[test]
fn generate_content_request_and_answer() {
    let request = gemini::generate_content_request(&model(), &prompt());
    assert_eq!(request.path(), "/v1beta/models/test-model:generateContent");
    let body: Value = serde_json::from_slice(request.body()).expect("JSON body");
    assert_eq!(body["contents"][0]["role"], "user");
    assert_eq!(body["contents"][0]["parts"][0]["text"], "Hello");
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        "Traduza para pt-BR."
    );
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 256);
    assert!(body["generationConfig"].get("responseMimeType").is_none());
    let json = gemini::generate_content_request(
        &model(),
        &TextPrompt {
            json_output: true,
            ..prompt()
        },
    );
    let body: Value = serde_json::from_slice(json.body()).expect("JSON body");
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert!(body["generationConfig"].get("responseSchema").is_none());
    // A forma da Traducao: um array de strings, com o JSON implicito.
    let schema = gemini::generate_content_request(
        &model(),
        &TextPrompt {
            response_schema: Some(gemini::ResponseSchema::StringArray),
            ..prompt()
        },
    );
    let body: Value = serde_json::from_slice(schema.body()).expect("JSON body");
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        body["generationConfig"]["responseSchema"],
        serde_json::json!({ "type": "ARRAY", "items": { "type": "STRING" } })
    );

    let thought = r#"{"candidates":[{"content":{"parts":[{"text":"pensando","thought":true},{"text":"Ol"},{"text":"á"}]},"finishReason":"MAX_TOKENS"}]}"#;
    let generated = gemini::parse_generate_content(thought.as_bytes()).expect("text");
    assert_eq!(generated.text, "Olá");
    assert_eq!(generated.finish, gemini::FinishReason::MaxTokens);
    let blocked =
        br#"{"candidates":[{"content":{"parts":[{"text":"x"}]},"finishReason":"SAFETY"}]}"#;
    assert_eq!(
        gemini::parse_generate_content(blocked)
            .expect("text")
            .finish,
        gemini::FinishReason::Blocked
    );
    for broken in [
        &b""[..],
        b"{",
        b"[]",
        br#"{"candidates":[]}"#,
        br#"{"candidates":[{"content":{"parts":[{"text":"x","thought":true}]}}]}"#,
        br#"{"candidates":[{"finishReason":"STOP"}]}"#,
    ] {
        assert_eq!(
            gemini::parse_generate_content(broken),
            Err(ApiError::Incomplete)
        );
    }
}

// ------------------------------------------------------------ modelos

const BRIEF_DENY_FAMILIES: [&str; 14] = [
    "realtime",
    "audio",
    "image",
    "tts",
    "transcribe",
    "search",
    "codex",
    "pro",
    "nano",
    "oss",
    "chat-latest",
    "live",
    "native-audio",
    "embedding",
];

fn id(value: &Value) -> ModelId {
    let raw = value.as_str().expect("a fixture id");
    ModelId::parse(raw).unwrap_or_else(|| panic!("fixture id {raw:?} must be a ModelId"))
}

fn tier(name: &str) -> PriceTier {
    match name {
        "Low" => PriceTier::Low,
        "Medium" => PriceTier::Medium,
        "High" => PriceTier::High,
        "Unknown" => PriceTier::Unknown,
        other => panic!("unknown tier {other}"),
    }
}

#[test]
fn pick_rule_table_over_every_denied_family() {
    let fixture = fixture();
    let rule = PickRule::for_purpose(Purpose::Translation, Provider::Gemini)
        .expect("the translation rule");
    let expect = id(&fixture["translation"]["expect"]);
    let pages = fixture["pages"].as_array().expect("pages");
    let served: BTreeSet<String> = pages
        .iter()
        .flat_map(|page| page["models"].as_array().expect("models"))
        .filter_map(|model| {
            model["name"]
                .as_str()?
                .strip_prefix("models/")
                .map(str::to_string)
        })
        .collect();

    // Cada termo negado do brief tem ids reais da familia e um chamariz: um
    // `*-flash-lite` mais novo que a escolha, que a forma e a estabilidade
    // aceitam e so o termo negado tira. Tirar um termo de DENY_TOKENS poe o
    // chamariz dele a ganhar.
    let denied = fixture["denied"].as_object().expect("denied");
    assert_eq!(
        denied.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BRIEF_DENY_FAMILIES.into_iter().collect::<BTreeSet<_>>(),
        "the fixture covers every denied family of the brief"
    );
    for (family, ids) in denied {
        let ids: Vec<ModelId> = ids.as_array().expect("ids").iter().map(id).collect();
        for model in &ids {
            assert!(
                models::deny_hits(model).contains(&family.as_str()),
                "{model} must be denied by '{family}'"
            );
            assert!(!rule.accepts(model), "{model}");
        }
        let decoy = ids.last().expect("a decoy");
        assert!(
            rule.accepts_ignoring_deny(decoy) && rule.newer(decoy, &expect),
            "the decoy {decoy} must beat {expect} but for '{family}'"
        );
        assert!(
            served.contains(decoy.as_str()),
            "the stub serves the decoy of '{family}'"
        );
    }

    // Listar (duas paginas no stub), filtrar e escolher: o codigo que
    // embarca.
    let stub = Stub::serve(
        pages
            .iter()
            .map(|page| json_response("200 OK", &page.to_string()))
            .collect(),
    );
    let listed = gemini::list_models(&stub.client(), Some(&TestKey), &|| false).expect("list");
    let requests = stub.received();
    assert_eq!(requests.len(), 2);
    assert!(!request_line(&requests[0]).contains("pageToken"));
    assert!(
        request_line(&requests[1]).contains(fixture["page_token_query"].as_str().expect("query")),
        "{}",
        request_line(&requests[1])
    );
    let names = pages
        .iter()
        .map(|page| page["models"].as_array().expect("models").len())
        .sum::<usize>();
    assert_eq!(
        listed.len(),
        names - 3,
        "the three hostile names are dropped"
    );
    let usable = models::filter_generation_models(listed.clone());
    assert!(usable.iter().all(|model| model.generates_text));
    assert!(usable.len() < listed.len());

    let chosen = models::pick(Purpose::Translation, Provider::Gemini, &usable, None)
        .expect("a translation model");
    assert_eq!(chosen.model, expect);
    assert_eq!(chosen.source, PickSource::Rule);
    assert_eq!(
        chosen.price_tier,
        tier(fixture["translation"]["tier"].as_str().expect("tier"))
    );
    assert_eq!(
        chosen.consent_line(),
        fixture["translation"]["consent_line"]
            .as_str()
            .expect("line")
    );

    // A ordem inteira: escolher, tirar, voltar a escolher. So os ids do
    // `ranking` alguma vez ganham; depois deles nao ha escolha.
    let mut remaining = usable;
    let mut order = Vec::new();
    while let Some(next) = models::pick(Purpose::Translation, Provider::Gemini, &remaining, None) {
        remaining.retain(|model| model.id != next.model);
        order.push(next.model.as_str().to_string());
    }
    let ranking: Vec<String> = fixture["translation"]["ranking"]
        .as_array()
        .expect("ranking")
        .iter()
        .map(|value| value.as_str().expect("id").to_string())
        .collect();
    assert_eq!(order, ranking);
}

#[test]
fn the_owner_pin_and_the_price_tier() {
    let fixture = fixture();
    let listed: Vec<_> = fixture["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .flat_map(|page| {
            gemini::parse_models_page(page.to_string().as_bytes())
                .expect("page")
                .models
        })
        .collect();
    for case in fixture["pins"].as_array().expect("pins") {
        let pin = id(&case["pin"]);
        let chosen = models::pick(Purpose::Translation, Provider::Gemini, &listed, Some(&pin))
            .expect("a pick");
        assert_eq!(chosen.model, id(&case["model"]), "pin {pin}");
        let source = match case["source"].as_str().expect("source") {
            "OwnerPin" => PickSource::OwnerPin,
            "RuleAfterMissingPin" => PickSource::RuleAfterMissingPin,
            other => panic!("unknown source {other}"),
        };
        assert_eq!(chosen.source, source, "pin {pin}");
        assert_eq!(
            chosen.price_tier,
            tier(case["tier"].as_str().expect("tier")),
            "pin {pin}"
        );
    }
    for (model, expected) in fixture["tiers"].as_object().expect("tiers") {
        let model = ModelId::parse(model).expect("id");
        assert_eq!(
            PriceTier::of(&model),
            tier(expected.as_str().expect("tier")),
            "{model}"
        );
    }
    assert_eq!(
        models::pick(Purpose::Translation, Provider::Gemini, &[], None),
        None
    );
}

// ------------------------------------------------------------ fonte

/// Os ficheiros de `src/llm.rs` e `src/llm/` (lidos do disco: um ficheiro
/// novo entra sozinho), com LF.
fn llm_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("llm.rs")];
    let mut dirs = vec![root.join("llm")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src/llm") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .expect("read an llm source")
                .replace("\r\n", "\n");
            let name = path
                .strip_prefix(&root)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/");
            (name, text)
        })
        .collect()
}

/// Prefixos de familias de modelos. Um prefixo seguido de letra ou digito e
/// um id de modelo (`<familia>-2.5-...`, `<familia>-pro`, `<familia>3`).
const MODEL_FAMILIES: &[&str] = &[
    "gemini-",
    "gemma-",
    "gpt-",
    "chatgpt-",
    "claude-",
    "text-embedding-",
    "imagen-",
    "veo-",
    "learnlm-",
    "codex-",
    "mistral-",
    "llama",
    "o1-",
    "o3-",
    "o4-",
];

fn model_ids_in(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut found = Vec::new();
    for family in MODEL_FAMILIES {
        for (at, _) in lower.match_indices(family) {
            let boundary = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
            let next = bytes.get(at + family.len());
            if boundary && next.is_some_and(u8::is_ascii_alphanumeric) {
                let end = lower[at..]
                    .find(|character: char| {
                        !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '/'))
                    })
                    .map_or(lower.len(), |offset| at + offset);
                found.push(lower[at..end].to_string());
            }
        }
    }
    found
}

#[test]
fn no_model_id_is_hard_coded() {
    // O detector ve um id quando ele existe (montado aqui, nao escrito).
    for sample in [
        format!("let id = \"{}{}\";", "gemini-", "2.5-flash-lite"),
        format!("// {}{}", "llama", "3.1-8b"),
        format!("\"{}{}\"", "claude-", "haiku-4-5"),
        format!("\"{}{}\"", "gpt-", "5-mini"),
    ] {
        assert!(!model_ids_in(&sample).is_empty(), "{sample}");
    }
    let sources = llm_sources();
    assert!(
        sources.len() >= 6,
        "src/llm.rs and src/llm/ must be read: {:?}",
        sources.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    for (name, text) in &sources {
        let found = model_ids_in(text);
        assert!(
            found.is_empty(),
            "{name} writes model ids {found:?}: models come from the runtime list"
        );
    }
}

#[test]
fn release_has_no_endpoint_override() {
    let sources = llm_sources();
    let shipped: Vec<&(String, String)> = sources
        .iter()
        .filter(|(name, _)| name != "llm/tests.rs")
        .collect();
    assert!(shipped.len() >= 5, "the shipped llm sources must be read");

    // Nenhuma variavel de ambiente muda um host, uma origem ou um fixture.
    for (name, text) in &shipped {
        for needle in [
            "env::var",
            "var_os(",
            "env::vars",
            "option_env!",
            "NEURALIA_",
        ] {
            assert!(
                !text.contains(needle),
                "{name} reads the environment: {needle}"
            );
        }
        for (at, _) in text.match_indices("env!(") {
            assert!(
                text[at..].starts_with("env!(\"CARGO_PKG_VERSION\")"),
                "{name}: only the package version is read at build time"
            );
        }
    }

    // O loopback so compila nos testes e e o unico sitio com uma origem que
    // nao e o host fixado por HTTPS.
    let declared: usize = shipped
        .iter()
        .map(|(_, text)| text.matches("fn loopback(").count())
        .sum();
    assert_eq!(declared, 1, "Endpoint::loopback is declared once");
    let transport = &shipped
        .iter()
        .find(|(name, _)| name == "llm/transport.rs")
        .expect("transport.rs")
        .1;
    let signature = "    #[cfg(test)]\n    pub(crate) fn loopback(port: u16) -> Self {\n";
    let start = transport
        .find(signature)
        .expect("Endpoint::loopback must be #[cfg(test)]");
    let close = "\n    }\n";
    let end = start
        + signature.len()
        + transport[start + signature.len()..]
            .find(close)
            .expect("the end of loopback")
        + close.len();
    assert!(transport[start..end].contains("127.0.0.1"));
    let without_loopback = format!("{}{}", &transport[..start], &transport[end..]);
    assert_eq!(
        without_loopback.matches("origin: format!(").count(),
        1,
        "Endpoint::pinned is the only other constructor"
    );

    let mut schemes = 0;
    for (name, text) in &shipped {
        let text = if name == "llm/transport.rs" {
            &without_loopback
        } else {
            text
        };
        for needle in [
            "127.0.0.1",
            "localhost",
            "[::1]",
            "0.0.0.0",
            "provider: None",
        ] {
            assert!(!text.contains(needle), "{name} outside loopback: {needle}");
        }
        for line in text.lines().filter(|line| line.contains("://")) {
            assert_eq!(
                line.trim(),
                "origin: format!(\"https://{}\", provider.host()),",
                "{name}: the only origin is the pinned host over HTTPS"
            );
            schemes += 1;
        }
    }
    assert_eq!(schemes, 1);
}
