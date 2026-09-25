//! Consistência entre o que embarca e o que o `neural-core` e o README dizem.
//!
//! 1. **Hosts de provedores.** O `onProviderPage` do
//!    `COMPARATOR_INJECT_SCRIPT` decide, numa coluna, qual página é "a página
//!    da IA" (o texto enviado nela vai às outras colunas). Está congelado na
//!    2.2.0 (AGENTS.md §7, scripts injetados) e NÃO é gerado a partir do
//!    registro `neural_core::search`. Este gate lê o script como embarca e
//!    exige que cada host e cada sufixo que ele aceita seja aceito por
//!    `neural_core::is_ai_provider_host`, e que o lookalike de cada sufixo seja
//!    recusado: um host novo no script que o registro não conhece, ou um
//!    registro que deixe passar um lookalike, fica vermelho. É um gate sobre
//!    a lista (§4.3): o comportamento do script no navegador é o
//!    `scripts/test-link-routing.mjs`.
//! 2. **README.** Frases sobre os Livros (EPUB) que o código não cumpria.

use neural_core::is_ai_provider_host;
use url::Url;

const PAGE_SCRIPTS: &str = include_str!("../src/windows_app/page_scripts.rs");
const README: &str = include_str!("../../../README.md");

/// Onde o script é mais largo do que o registro, de propósito e à vista.
///
/// `.google.com` com `udm=50`: o script (congelado) trata qualquer
/// `*.google.com?udm=50` como a página do Google IA; o registro aceita só
/// `google.com` e `www.google.com`, porque em `sites.google.com` qualquer
/// pessoa publica páginas e o registro decide o que o adblock e a
/// anti-distração nunca tocam (auditoria 2.2.0,
/// `core-registry-predicate-scheme-subdomains`). Estreitar o script mexe em
/// `crates/neural-app/src` e num script injetado: fica para quem tem essa área
/// e para o dono. O gate exige que esta entrada continue verdadeira (o script
/// ainda a tem e o registro ainda a recusa), para ela não ficar esquecida aqui
/// depois de o script ser corrigido.
const KNOWN_WIDER_IN_SCRIPT: &[(&str, bool)] = &[(".google.com", true)];

const UDM50: &str = "searchParams.get('udm') === '50'";

/// Um host que o script aceita: exato ou sufixo, com ou sem `udm=50`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptHost {
    literal: String,
    suffix: bool,
    udm50: bool,
}

fn normalized(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// O corpo do `COMPARATOR_INJECT_SCRIPT` (o `r#"…"#` que embarca).
fn comparator_script(source: &str) -> String {
    let source = normalized(source);
    let start = source
        .find("COMPARATOR_INJECT_SCRIPT: &str = r#\"")
        .expect("COMPARATOR_INJECT_SCRIPT em page_scripts.rs");
    let body = &source[start..];
    let open = body.find("r#\"").unwrap() + 3;
    let close = body[open..]
        .find("\"#;")
        .expect("fim do COMPARATOR_INJECT_SCRIPT");
    body[open..open + close].to_string()
}

/// O corpo (entre as chavetas) de `function onProviderPage()`.
fn on_provider_page(script: &str) -> Result<&str, String> {
    let start = script
        .find("function onProviderPage() {")
        .ok_or("function onProviderPage() não está no script")?;
    let open = start + "function onProviderPage() ".len();
    let mut depth = 0usize;
    for (offset, c) in script[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&script[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }
    Err("onProviderPage sem chaveta de fecho".into())
}

/// Os hosts do `onProviderPage`. Só dois formatos contam, `host === '<h>'` e
/// `host.endsWith('<s>')`; qualquer outro uso de `host`, `hostname` ou de uma
/// string no corpo, ou um `return true` sem host, é recusado: o gate não
/// adivinha, e um formato novo tem de ser ensinado aqui.
fn script_hosts(body: &str) -> Result<Vec<ScriptHost>, String> {
    let mut hosts = Vec::new();
    for statement in body.split(';') {
        // A chaveta que fecha um bloco (`} catch`, `}`) fica colada à
        // instrução seguinte.
        let statement = statement
            .trim_start_matches(|c: char| c == '}' || c.is_whitespace())
            .trim_end();
        if statement.is_empty() || statement == "const host = here.hostname.toLowerCase()" {
            continue;
        }
        let udm50 = statement.contains(UDM50);
        let mut residue = statement.replace(UDM50, "");
        let mut found = 0;
        for (pattern, suffix) in [("host === '", false), ("host.endsWith('", true)] {
            while let Some(at) = residue.find(pattern) {
                let from = at + pattern.len();
                let len = residue[from..]
                    .find('\'')
                    .ok_or_else(|| format!("literal sem fecho em {statement:?}"))?;
                let literal = residue[from..from + len].to_string();
                let end = from + len + 1 + usize::from(suffix);
                if suffix && residue.as_bytes().get(end - 1) != Some(&b')') {
                    return Err(format!("endsWith fora do formato em {statement:?}"));
                }
                residue.replace_range(at..end, "HOST");
                hosts.push(ScriptHost {
                    literal,
                    suffix,
                    udm50,
                });
                found += 1;
            }
        }
        let residue = residue.replace("HOST", "");
        if residue.contains("host") || residue.contains('\'') || residue.contains("udm") {
            return Err(format!(
                "formato que o gate não conhece em onProviderPage: {statement:?}"
            ));
        }
        let returns_true = statement.contains("return true")
            || (statement.starts_with("return") && !statement.contains("return false"));
        if returns_true && found == 0 {
            return Err(format!("return sem host em onProviderPage: {statement:?}"));
        }
    }
    Ok(hosts)
}

fn provider_url(host: &str, udm50: bool) -> Url {
    let query = if udm50 { "search?q=x&udm=50" } else { "" };
    Url::parse(&format!("https://{host}/{query}")).expect(host)
}

/// As divergências entre o script e o registro (vazio = consistente).
fn violations(source: &str) -> Vec<String> {
    let script = comparator_script(source);
    let hosts = match on_provider_page(&script).and_then(script_hosts) {
        Ok(hosts) => hosts,
        Err(error) => return vec![error],
    };
    let mut problems = Vec::new();
    for host in &hosts {
        let ScriptHost {
            literal,
            suffix,
            udm50,
        } = host;
        if !suffix {
            if !is_ai_provider_host(&provider_url(literal, *udm50)) {
                problems.push(format!("o script aceita {literal} e o registro não"));
            }
            continue;
        }
        let Some(base) = literal.strip_prefix('.') else {
            problems.push(format!(
                "endsWith('{literal}') sem o ponto aceita lookalikes (evil{literal})"
            ));
            continue;
        };
        let representative = provider_url(&format!("sub{literal}"), *udm50);
        let known = KNOWN_WIDER_IN_SCRIPT.contains(&(literal.as_str(), *udm50));
        match (is_ai_provider_host(&representative), known) {
            (true, false) => {}
            (false, false) => problems.push(format!(
                "o script aceita *{literal} e o registro recusa {representative}"
            )),
            (true, true) => problems.push(format!(
                "*{literal} já não é mais largo no script: tire-o de KNOWN_WIDER_IN_SCRIPT"
            )),
            (false, true) => {}
        }
        for lookalike in [format!("evil{base}"), format!("{base}.evil.io")] {
            let lookalike = provider_url(&lookalike, *udm50);
            if is_ai_provider_host(&lookalike) {
                problems.push(format!("o registro aceita o lookalike {lookalike}"));
            }
        }
    }
    for (literal, udm50) in KNOWN_WIDER_IN_SCRIPT {
        let present = hosts
            .iter()
            .any(|host| host.suffix && host.literal == *literal && host.udm50 == *udm50);
        if !present {
            problems.push(format!(
                "*{literal} saiu do script: tire-o de KNOWN_WIDER_IN_SCRIPT"
            ));
        }
    }
    problems
}

#[test]
fn script_hosts_subset_of_registry() {
    let script = comparator_script(PAGE_SCRIPTS);
    let hosts = script_hosts(on_provider_page(&script).unwrap()).unwrap();
    // O extrator leu a lista, não um corpo vazio.
    for expected in [
        "chatgpt.com",
        "chat.openai.com",
        "claude.ai",
        "gemini.google.com",
    ] {
        assert!(
            hosts
                .iter()
                .any(|host| !host.suffix && host.literal == expected),
            "{expected} não foi extraído de onProviderPage: {hosts:?}"
        );
    }
    assert!(
        hosts.iter().any(|host| host.udm50),
        "o ramo do Google com udm=50 não foi extraído: {hosts:?}"
    );
    let problems = violations(PAGE_SCRIPTS);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Controle negativo do extrator (não é a prova de sabotagem, que quebra o
/// registro no neural-core): uma cópia do script com a deriva que a
/// auditoria usou, ou com um sufixo sem ponto, tem de dar divergência.
#[test]
fn a_drifted_copy_of_the_script_is_caught() {
    let source = normalized(PAGE_SCRIPTS);
    let declaration = "const host = here.hostname.toLowerCase();";
    assert_eq!(source.matches(declaration).count(), 1);
    for (drift, expected) in [
        (
            "if (host === 'chatgpt.com.evil.io') return true;",
            "chatgpt.com.evil.io",
        ),
        (
            "if (host.endsWith('chatgpt.com')) return true;",
            "sem o ponto",
        ),
        ("if (host.includes('chatgpt')) return true;", "não conhece"),
        ("if (here.hostname === 'x.io') return true;", "não conhece"),
        ("if (true) return true;", "return sem host"),
    ] {
        let drifted = source.replacen(declaration, &format!("{declaration}\n    {drift}"), 1);
        let problems = violations(&drifted);
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "{drift}: {problems:#?}"
        );
    }
}

#[test]
fn readme_describes_the_epub_surface_as_it_ships() {
    let readme = normalized(README);
    // A omnibox só abre um livro pelo `epub:<caminho>`; um caminho solto é
    // recusado como caminho local (`route_input` e `parse_intent`).
    assert!(!readme.contains("a `.epub` path in the omnibox"));
    assert!(readme.contains("`epub:<path>`"));
    // O mapa de teclas não é injetado na WebView dos Livros: o README tem de
    // dizer que ali é a exceção a "every surface".
    let keyboard = readme
        .split("## Keyboard")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("secção Keyboard do README");
    assert!(keyboard.contains("They work on every surface unless noted."));
    assert!(
        keyboard.contains("**Exception: Livros (EPUB).**"),
        "a secção Keyboard não assinala a exceção dos Livros"
    );
}
