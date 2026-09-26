//! Gates do bloqueio de anuncios (adblock). Correm sobre o codigo que
//! embarca: o mesmo `DomainSet`, o mesmo `decide`, o mesmo `ListClient`
//! apontado a um stub em 127.0.0.1 (`ListEndpoint::loopback`, que so existe
//! nos testes). Nenhum teste sai da maquina; nenhum fala com pgl.yoyo.org.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

use super::*;
use crate::search::{ProviderId, login_hosts};

fn url(text: &str) -> Url {
    Url::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

fn set(domains: &[&str]) -> DomainSet {
    let mut set = DomainSet::new();
    for domain in domains {
        assert!(set.insert(domain), "{domain}");
    }
    set
}

fn rules(domains: &[&str], allow: &[&str]) -> AdblockRules {
    AdblockRules::new(
        Arc::new(set(domains)),
        allow.iter().map(|site| site.to_string()).collect(),
    )
}

/// Uma lista valida com `count` dominios `adN.example-ads.net`.
fn list_text(count: usize) -> String {
    (0..count)
        .map(|index| format!("ad{index}.example-ads.net\n"))
        .collect()
}

// ------------------------------------------------------------ o casamento

/// Gate (critico, sabotagem obrigatoria): o casamento e por sufixo de
/// rotulos. Com `example.com` na lista, `notexample.com` passa e
/// `ads.example.com` fica -- tambem pelo `decide` que o despachante chama.
#[test]
fn suffix_match_rejects_notexample_com() {
    let domains = set(&["example.com", "doubleclick.net"]);
    assert_eq!(domains.find("example.com"), Some("example.com"));
    assert_eq!(domains.find("ads.example.com"), Some("example.com"));
    assert_eq!(domains.find("notexample.com"), None);
    assert_eq!(domains.find("example.com.evil.io"), None);
    assert_eq!(domains.find("xdoubleclick.net"), None);
    assert_eq!(domains.find("ad.doubleclick.net"), Some("doubleclick.net"));

    let rules = rules(&["example.com"], &[]);
    let top = url("https://news.site.test/article");
    assert_eq!(
        rules.decide(
            &url("https://notexample.com/ad.js"),
            Some(&top),
            ResourceKind::Script
        ),
        Decision::Allow(Allow::NotListed)
    );
    assert_eq!(
        rules.decide(
            &url("https://ads.example.com/ad.js"),
            Some(&top),
            ResourceKind::Script
        ),
        Decision::Block {
            listed: "example.com".to_string()
        }
    );
}

/// Gate (relacao, nao relogio): a procura faz uma consulta por rotulo do
/// host, com 10 ou com 400 000 dominios na lista.
#[test]
fn lookup_count_equals_label_count_at_10_and_400000_entries() {
    let hosts = [
        "a.b.c.d.e.f.not-listed.org",
        "x.y.not-listed.org",
        "not-listed.org",
        "ads.tracker.example",
    ];
    for size in [10usize, 400_000] {
        let mut domains = DomainSet::new();
        for index in 0..size {
            assert!(domains.insert(&format!("d{index}.listed.net")));
        }
        assert_eq!(domains.len(), size);
        for host in hosts {
            let mut lookups = 0;
            assert_eq!(domains.find_counting(host, &mut lookups), None, "{host}");
            assert_eq!(
                lookups,
                host.split('.').count(),
                "{host} com {size} dominios: uma consulta por rotulo"
            );
        }
        // Um acerto para no rotulo que casa.
        let mut lookups = 0;
        assert_eq!(
            domains.find_counting("x.y.d7.listed.net", &mut lookups),
            Some("d7.listed.net")
        );
        assert_eq!(lookups, 3);
    }
}

// ------------------------------------------------------------ NEVER_BLOCK

/// Gate (critico, sabotagem obrigatoria): uma pagina de IA ou de login nunca
/// tem nada bloqueado -- e a lista vem do registo dos provedores (cada
/// regra de host de cada provedor e cada host de login), nao de uma lista a
/// mao. Os imitadores do registo nao sao isentos.
#[test]
fn ai_and_login_tops_are_never_blocked() {
    let rules = rules(&["doubleclick.net", "tracker.example"], &[]);
    let ad = url("https://ad.doubleclick.net/pixel.gif");

    let mut tops: Vec<Url> = Vec::new();
    for provider in ProviderId::all() {
        for rule in provider.hosts() {
            let query = if rule.require_udm50 {
                "?udm=50&q=x"
            } else {
                ""
            };
            tops.push(url(&format!("https://{}/{query}", rule.host)));
            if rule.subdomains {
                tops.push(url(&format!("https://sub.{}/", rule.host)));
            }
        }
    }
    for host in login_hosts() {
        tops.push(url(&format!("https://{host}/signin")));
    }
    // Os do brief, por nome, para a lista nao encolher sem ninguem ver.
    for named in [
        "https://www.google.com/search?udm=50&q=x",
        "https://gemini.google.com/app",
        "https://chatgpt.com/c/1",
        "https://claude.ai/new",
        "https://www.perplexity.ai/",
        "https://accounts.google.com/v3/signin",
        "https://login.live.com/",
        "https://login.microsoftonline.com/common",
        "https://appleid.apple.com/auth",
    ] {
        tops.push(url(named));
    }
    assert!(tops.len() >= 20, "o registo encolheu: {}", tops.len());
    for top in &tops {
        assert!(page_is_always_exempt(top), "{top}");
        for kind in [ResourceKind::Script, ResourceKind::Image, ResourceKind::Xhr] {
            assert_eq!(
                rules.decide(&ad, Some(top), kind),
                Decision::Allow(Allow::AiOrLogin),
                "{top} {kind:?}"
            );
        }
    }

    // Imitadores e a pesquisa normal do Google: a lista vale.
    for lookalike in [
        "https://chatgpt.com.evil.io/",
        "https://evilclaude.ai/",
        "https://www.google.com/search?q=x",
        "https://www.google.com/search?udm=14&udm=50",
        "https://accounts.google.com.evil.io/",
        "http://notgemini.google.com.test/",
    ] {
        let top = url(lookalike);
        assert!(!page_is_always_exempt(&top), "{lookalike}");
        assert!(
            rules.decide(&ad, Some(&top), ResourceKind::Script).blocks(),
            "{lookalike}"
        );
    }
}

/// Gate (critico, sabotagem obrigatoria): o proprio documento nunca e
/// bloqueado -- uma navegacao para um dominio da lista (um link de anuncio
/// que o utilizador clicou) abre; os recursos desse dominio numa pagina de
/// outro site nao.
#[test]
fn the_document_itself_is_never_blocked() {
    let rules = rules(&["doubleclick.net"], &[]);
    let listed = url("https://ad.doubleclick.net/click?x=1");
    let top = url("https://news.site.test/");
    assert_eq!(
        rules.decide(&listed, Some(&top), ResourceKind::Document),
        Decision::Allow(Allow::Document)
    );
    assert_eq!(
        rules.decide(&listed, None, ResourceKind::Document),
        Decision::Allow(Allow::Document)
    );
    for kind in [
        ResourceKind::Script,
        ResourceKind::Image,
        ResourceKind::Stylesheet,
        ResourceKind::Xhr,
        ResourceKind::Fetch,
        ResourceKind::Media,
        ResourceKind::Font,
        ResourceKind::Other,
    ] {
        assert!(rules.decide(&listed, Some(&top), kind).blocks(), "{kind:?}");
    }
    // Sem pagina web por cima (about:blank): a lista vale igual.
    assert!(rules.decide(&listed, None, ResourceKind::Script).blocks());
}

#[test]
fn local_first_party_and_allowed_sites_are_never_blocked() {
    let rules = rules(
        &[
            "localhost.test",
            "example.com",
            "cdn.shop.test",
            "neuralia-pdf.localhost",
        ],
        &["allowed.test"],
    );
    let top = url("https://news.site.test/");
    for local in [
        "http://localhost:8080/ad.js",
        "http://neuralia-pdf.localhost/viewer.html",
        "http://sub.localhost/x",
        "http://127.0.0.1:9000/x",
        "http://127.5.6.7/x",
        "http://[::1]:8080/x",
        "http://[::ffff:127.0.0.1]/x",
    ] {
        assert_eq!(
            rules.decide(&url(local), Some(&top), ResourceKind::Script),
            Decision::Allow(Allow::Local),
            "{local}"
        );
    }
    for not_web in [
        "data:text/javascript,1",
        "blob:https://news.site.test/abc",
        "neuralia-pdf://viewer.html",
    ] {
        assert_eq!(
            rules.decide(&url(not_web), Some(&top), ResourceKind::Script),
            Decision::Allow(Allow::NotWeb),
            "{not_web}"
        );
    }
    // O proprio site: subdominios nos dois sentidos, com e sem www.
    let shop = url("https://www.shop.test/cart");
    assert_eq!(
        rules.decide(
            &url("https://cdn.shop.test/a.js"),
            Some(&shop),
            ResourceKind::Script
        ),
        Decision::Allow(Allow::FirstParty)
    );
    let example = url("https://example.com/");
    assert_eq!(
        rules.decide(
            &url("https://ads.example.com/a.js"),
            Some(&example),
            ResourceKind::Script
        ),
        Decision::Allow(Allow::FirstParty)
    );
    // Um site com anuncios permitidos (e so ele: nao o vizinho).
    let allowed = url("https://www.allowed.test/page");
    assert!(rules.site_allowed(&allowed));
    assert_eq!(
        rules.decide(
            &url("https://ads.example.com/a.js"),
            Some(&allowed),
            ResourceKind::Script
        ),
        Decision::Allow(Allow::SiteAllowed)
    );
    let neighbour = url("https://notallowed.test/");
    assert!(!rules.site_allowed(&neighbour));
    assert!(
        rules
            .decide(
                &url("https://ads.example.com/a.js"),
                Some(&neighbour),
                ResourceKind::Script
            )
            .blocks()
    );
    // Host com ponto final e maiusculas: o mesmo dominio.
    assert!(
        rules
            .decide(
                &url("https://ADS.Example.COM./a.js"),
                Some(&top),
                ResourceKind::Script
            )
            .blocks()
    );
}

// ------------------------------------------------------------ o parser

#[test]
fn the_list_parser_takes_plain_hosts_and_abp_names_only() {
    let text = "\
# comentario
! comentario ABP
[Adblock Plus 2.0]

ads.example.com
Tracker.Example.NET.
0.0.0.0 hosts.example.org
127.0.0.1 one.example.org two.example.org # fim
||abp.example.io^
||abp-noanchor.example.io
anúncios.exemplo.br

example.com/path
*.wild.example
||opt.example.io^$third-party
||path.example.io^/x
@@||exception.example.io^
example.com##.banner
site.test#@#.ad
singlelabel
0.0.0.0 localhost
1.2.3.4
0.0.0.0
two tokens.example.com
";
    let parsed = parse_domain_list(text);
    let mut got: Vec<String> = parsed
        .domains
        .to_text()
        .lines()
        .map(str::to_owned)
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            "abp-noanchor.example.io",
            "abp.example.io",
            "ads.example.com",
            "hosts.example.org",
            "one.example.org",
            "tracker.example.net",
            "two.example.org",
            "xn--anncios-71a.exemplo.br",
        ]
    );
    assert_eq!(parsed.ignored, 12, "{parsed:?}");
    assert!(!parsed.truncated);
    // CRLF e o mesmo.
    assert_eq!(
        parse_domain_list("a.example.com\r\nb.example.com\r\n")
            .domains
            .len(),
        2
    );
}

#[test]
fn the_domain_set_keeps_its_caps() {
    let mut domains = DomainSet::new();
    assert!(domains.insert("a.example.com"));
    assert!(!domains.insert("a.example.com"), "repetido");
    assert_eq!(domains.len(), 1);
    assert_eq!(domains.bytes(), "a.example.com".len());

    // O tecto de entradas.
    let mut text = String::new();
    for index in 0..MAX_DOMAINS + 5 {
        text.push_str(&format!("d{index}.x.io\n"));
    }
    let parsed = parse_domain_list(&text);
    assert_eq!(parsed.domains.len(), MAX_DOMAINS);
    assert!(parsed.truncated);

    // O tecto de bytes: nomes compridos param antes das entradas.
    let label = "a".repeat(60);
    let long: String = (0..200_000)
        .map(|index| format!("{label}.{label}.{index}.io\n"))
        .collect();
    let parsed = parse_domain_list(&long);
    assert!(parsed.truncated);
    assert!(parsed.domains.bytes() <= MAX_DOMAIN_BYTES);
    assert!(parsed.domains.len() < 200_000);
}

// ------------------------------------------------------------ a validacao

/// Gate (critico, sabotagem obrigatoria): o que chega da rede so e lista
/// com pelo menos 1 000 dominios, ate 2 MiB, e nunca HTML.
#[test]
fn list_validation_refuses_html_short_huge_and_binary_bodies() {
    let good = list_text(MIN_LIST_DOMAINS);
    let parsed = validate_list(Some("text/plain; charset=UTF-8"), good.as_bytes())
        .expect("1 000 dominios em texto");
    assert_eq!(parsed.domains.len(), MIN_LIST_DOMAINS);
    assert!(validate_list(None, good.as_bytes()).is_ok());

    assert_eq!(
        validate_list(
            Some("text/plain"),
            list_text(MIN_LIST_DOMAINS - 1).as_bytes()
        )
        .map(|parsed| parsed.domains.len()),
        Err(ListError::TooFew {
            found: MIN_LIST_DOMAINS - 1
        })
    );
    for html_type in [
        "text/html",
        "TEXT/HTML; charset=utf-8",
        " text/html ;x=y",
        "application/xhtml+xml",
    ] {
        assert_eq!(
            validate_list(Some(html_type), good.as_bytes()).map(|_| ()),
            Err(ListError::Html),
            "{html_type}"
        );
    }
    let page = format!("\u{FEFF}  \n<!DOCTYPE html><html><body>{good}</body></html>");
    assert_eq!(
        validate_list(Some("text/plain"), page.as_bytes()).map(|_| ()),
        Err(ListError::Html)
    );
    let mut huge = good.clone().into_bytes();
    huge.resize(LIST_MAX_BYTES + 1, b'\n');
    assert_eq!(
        validate_list(Some("text/plain"), &huge).map(|_| ()),
        Err(ListError::TooLarge)
    );
    let mut binary = good.into_bytes();
    binary.extend_from_slice(&[0xFF, 0xFE, 0x00]);
    assert_eq!(
        validate_list(Some("text/plain"), &binary).map(|_| ()),
        Err(ListError::NotText)
    );
    assert!(
        ListError::TooFew { found: 3 }
            .pt_br_message()
            .contains("mínimo 1000")
    );
}

// ------------------------------------------------------------ a renovacao

/// Gate (amostrado): nunca na Home, nunca no modo privado, no maximo uma
/// vez por semana, uma hora depois de uma falha.
#[test]
fn refresh_is_never_due_on_home_and_at_most_weekly() {
    let now = 100 * REFRESH_EVERY_MS;
    let state = RefreshState {
        enabled: true,
        fetched_ms: None,
        in_flight: false,
        last_failure_ms: None,
        private: false,
    };
    assert!(refresh_due(now, RefreshSurface::Web, &state));
    assert!(!refresh_due(now, RefreshSurface::Home, &state), "Home");
    assert!(
        !refresh_due(now, RefreshSurface::Local, &state),
        "Leitor/PDF"
    );
    for stored in [Some(now), Some(now - REFRESH_EVERY_MS + 1)] {
        let fresh = RefreshState {
            fetched_ms: stored,
            ..state
        };
        assert!(!refresh_due(now, RefreshSurface::Web, &fresh), "{stored:?}");
    }
    let stale = RefreshState {
        fetched_ms: Some(now - REFRESH_EVERY_MS),
        ..state
    };
    assert!(refresh_due(now, RefreshSurface::Web, &stale));
    assert!(!refresh_due(now, RefreshSurface::Home, &stale), "Home");
    let clock_back = RefreshState {
        fetched_ms: Some(now + 1),
        ..state
    };
    assert!(refresh_due(now, RefreshSurface::Web, &clock_back));
    for blocked in [
        RefreshState {
            enabled: false,
            ..stale
        },
        RefreshState {
            private: true,
            ..stale
        },
        RefreshState {
            in_flight: true,
            ..stale
        },
        RefreshState {
            last_failure_ms: Some(now - RETRY_AFTER_FAILURE_MS + 1),
            ..stale
        },
    ] {
        assert!(
            !refresh_due(now, RefreshSurface::Web, &blocked),
            "{blocked:?}"
        );
    }
    assert!(refresh_due(
        now,
        RefreshSurface::Web,
        &RefreshState {
            last_failure_ms: Some(now - RETRY_AFTER_FAILURE_MS),
            ..stale
        }
    ));

    assert!(download_on_activation(now, None, false));
    assert!(download_on_activation(
        now,
        Some(now - REFRESH_EVERY_MS),
        false
    ));
    assert!(!download_on_activation(now, Some(now - 1), false));
    assert!(!download_on_activation(now, None, true), "modo privado");
}

// ------------------------------------------------------------ o que se guarda

#[test]
fn settings_keep_only_valid_sites_and_the_reserved_distraction_field() {
    let mut settings = AdblockSettings::default();
    assert!(settings.set_site_blocking("www.Example.com", false));
    assert!(
        !settings.set_site_blocking("www.example.com", false),
        "ja estava"
    );
    assert!(settings.allow_sites.contains("www.example.com"));
    assert!(settings.set_site_blocking("www.example.com", true));
    assert!(settings.allow_sites.is_empty());
    assert!(!settings.set_site_blocking("not a host", false));
    assert!(
        !settings.set_site_blocking("localhost", false),
        "um rotulo so"
    );

    let json = r#"{"enabled":true,"allow_sites":["ok.test","Bad Site","ok.test/x"],"distraction":{"ok.test":false}}"#;
    let loaded: AdblockSettings = serde_json::from_str(json).expect("json");
    let clean = loaded.sanitized();
    assert!(clean.enabled);
    assert_eq!(clean.allow_sites.iter().collect::<Vec<_>>(), ["ok.test"]);
    assert_eq!(clean.distraction.get("ok.test"), Some(&false));
    let empty: AdblockSettings = serde_json::from_str("{}").expect("defaults");
    assert_eq!(empty, AdblockSettings::default());

    let parsed = parse_domain_list(&list_text(20));
    let stored = StoredList::from_set(&parsed.domains, 42);
    assert_eq!(stored.fetched_ms, 42);
    assert_eq!(stored.parse().domains.len(), 20);
    assert!(StoredList::default().is_empty());
}

// ------------------------------------------------------------ o download

/// Um servidor HTTP/1.1 em 127.0.0.1 que responde a cada ligacao com a
/// resposta seguinte da lista e guarda o pedido bruto.
struct Stub {
    port: u16,
    requests: mpsc::Receiver<String>,
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
                let request = read_head(&mut stream);
                let _ = sender.send(request);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        Self { port, requests }
    }

    fn client(&self) -> ListClient {
        ListClient::new(ListEndpoint::loopback(self.port))
    }

    fn received(&self) -> Vec<String> {
        self.requests.try_iter().collect()
    }
}

fn read_head(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
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

fn never() -> bool {
    false
}

/// Gate (critico): o download da lista pela politica do transporte -- o
/// pedido pede texto no caminho fixado, com o User-Agent do NeuralIA; um
/// 302 nao se segue (o stub ve UM pedido); um corpo acima de 2 MiB,
/// declarado ou nao, e recusado; HTML e uma lista curta tambem; o prazo
/// corta um servidor parado.
#[test]
fn the_list_download_goes_through_the_transport_policy() {
    let body = list_text(1_200);
    let stub = Stub::serve(vec![response(
        "200 OK",
        &[
            ("Content-Type", "text/plain; charset=UTF-8"),
            ("Content-Length", &body.len().to_string()),
        ],
        body.as_bytes(),
    )]);
    let parsed = stub.client().fetch(&never).expect("the list");
    assert_eq!(parsed.domains.len(), 1_200);
    let requests = stub.received();
    assert_eq!(requests.len(), 1);
    let head = requests[0].to_ascii_lowercase();
    assert!(
        requests[0].starts_with(&format!("GET {LIST_PATH} HTTP/1.1\r\n")),
        "{}",
        requests[0]
    );
    assert!(head.contains("\r\naccept: text/plain\r\n"), "{head}");
    assert!(
        head.contains(&format!(
            "\r\nuser-agent: neuralia/{}\r\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{head}"
    );

    // 302 para outro host: nao se segue.
    let stub = Stub::serve(vec![
        response(
            "302 Found",
            &[
                ("Location", "http://127.0.0.1:9/elsewhere"),
                ("Content-Length", "0"),
            ],
            b"",
        ),
        response("200 OK", &[("Content-Type", "text/plain")], body.as_bytes()),
    ]);
    assert_eq!(
        stub.client().fetch(&never).map(|_| ()),
        Err(FetchError::Transport(ApiError::ServiceUnavailable {
            status: Some(302)
        }))
    );
    assert_eq!(stub.received().len(), 1, "o redirect foi seguido");

    // Acima do tecto: declarado, e sem Content-Length.
    let huge = vec![b'a'; LIST_MAX_BYTES + 1];
    let stub = Stub::serve(vec![
        response(
            "200 OK",
            &[("Content-Length", &huge.len().to_string())],
            &huge,
        ),
        response("200 OK", &[], &huge),
    ]);
    let too_large = Err(FetchError::Transport(ApiError::TooLarge {
        limit: LIST_MAX_BYTES as u64,
    }));
    assert_eq!(stub.client().fetch(&never).map(|_| ()), too_large);
    assert_eq!(stub.client().fetch(&never).map(|_| ()), too_large);

    // HTML e lista curta.
    let stub = Stub::serve(vec![
        response("200 OK", &[("Content-Type", "text/html")], body.as_bytes()),
        response(
            "200 OK",
            &[("Content-Type", "text/plain")],
            list_text(10).as_bytes(),
        ),
        response("404 Not Found", &[("Content-Length", "0")], b""),
    ]);
    assert_eq!(
        stub.client().fetch(&never).map(|_| ()),
        Err(FetchError::List(ListError::Html))
    );
    assert_eq!(
        stub.client().fetch(&never).map(|_| ()),
        Err(FetchError::List(ListError::TooFew { found: 10 }))
    );
    assert_eq!(
        stub.client().fetch(&never).map(|_| ()),
        Err(FetchError::Transport(ApiError::ServiceUnavailable {
            status: Some(404)
        }))
    );

    // Desistir antes de ligar.
    assert_eq!(
        stub.client().fetch(&|| true).map(|_| ()),
        Err(FetchError::Transport(ApiError::Cancelled))
    );

    // O prazo: um servidor que aceita e nao responde.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let hold = thread::spawn(move || {
        let accepted = listener.accept();
        thread::sleep(Duration::from_millis(1_500));
        drop(accepted);
    });
    let client = ListClient::new(ListEndpoint::loopback(port));
    let started = Instant::now();
    assert_eq!(
        client
            .fetch_within(Duration::from_millis(300), &never)
            .map(|_| ()),
        Err(FetchError::Transport(ApiError::Timeout))
    );
    assert!(started.elapsed() < Duration::from_millis(1_400));
    let _ = hold.join();
    assert_eq!(LIST_TIMEOUT, Duration::from_secs(20));
    assert_eq!(LIST_MAX_BYTES, 2 * 1024 * 1024);
}

/// Gate (presenca proibida, como o do transporte de IA): o cliente da lista
/// so tem a origem fixada por HTTPS; o loopback so compila nos testes.
#[test]
fn the_list_client_only_reaches_the_pinned_host() {
    let source = include_str!("../adblock.rs").replace("\r\n", "\n");
    let shipped = source
        .split("\n#[cfg(test)]\nmod tests;")
        .next()
        .expect("the shipped part");
    assert_eq!(shipped.matches("fn loopback(").count(), 1);
    let signature = "    #[cfg(test)]\n    pub(crate) fn loopback(port: u16) -> Self {\n";
    let start = shipped
        .find(signature)
        .expect("ListEndpoint::loopback must be #[cfg(test)]");
    let end = start
        + signature.len()
        + shipped[start + signature.len()..]
            .find("\n    }\n")
            .expect("the end of loopback")
        + "\n    }\n".len();
    assert!(shipped[start..end].contains("127.0.0.1"));
    let without_loopback = format!("{}{}", &shipped[..start], &shipped[end..]);
    // Nenhuma origem http:// literal fora do loopback (o "127.0.0.1" dos
    // sumidouros do formato hosts e texto da lista, nao uma origem), e a
    // unica https:// e a fixada.
    for needle in ["\"http://", "pinned: false", "Locality::Loopback"] {
        assert!(!without_loopback.contains(needle), "{needle}");
    }
    assert_eq!(without_loopback.matches("\"https://").count(), 1);
    let origins: Vec<&str> = without_loopback
        .lines()
        .filter(|line| line.contains("origin: format!("))
        .map(str::trim)
        .collect();
    assert_eq!(origins, ["origin: format!(\"https://{LIST_HOST}\"),"]);
    assert_eq!(LIST_HOST, "pgl.yoyo.org");
    assert!(!without_loopback.contains("std::env"), "nada do ambiente");

    let pinned = ListClient::new(ListEndpoint::pinned());
    assert_eq!(pinned.endpoint().locality(), Locality::Public);
    let config = pinned.agent_config();
    assert!(config.https_only());
    assert_eq!(config.max_redirects(), 0);
    assert!(!config.http_status_as_error());
}
