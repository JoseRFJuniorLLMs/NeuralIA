//! Consistência entre o que embarca e o que o `neural-core` e o README dizem.
//!
//! 1. **Hosts de provedores.** O `onProviderPage` do
//!    `COMPARATOR_INJECT_SCRIPT` decide, numa coluna, qual página é "a página
//!    da IA" (o texto enviado nela vai às outras colunas). Está congelado na
//!    2.2.0 (AGENTS.md §7, scripts injetados) e NÃO é gerado a partir do
//!    registro `neural_core::search`. Este gate lê a função como embarca,
//!    parte-a em símbolos e só a aceita se ela couber, inteira, numa gramática
//!    fechada (as instruções que a função tem hoje, com `===`, `endsWith`,
//!    `searchParams.get`, `||`, `&&` e parênteses): qualquer outro símbolo
//!    (aspas duplas, `!`, `indexOf`, `here.origin`, um comentário...) ou
//!    outra forma fica vermelho até ser ensinado aqui. A função lida é então
//!    AVALIADA com a precedência do JavaScript (`&&` antes de `||`) sobre uma
//!    tabela de endereços gerada do registro e dos literais do próprio script
//!    (hosts aceitos, subdomínios, lookalikes, o Google sem `udm=50`), e cada
//!    página que ela aceita tem de ser aceita por
//!    `neural_core::is_ai_provider_host`. O script declara `onProviderPage`
//!    uma vez só e só a chama (uma segunda declaração ou uma reatribuição
//!    trocariam a função avaliada aqui pela que corre). O comportamento do
//!    script no navegador é o `scripts/test-link-routing.mjs`.
//! 2. **README.** Frases sobre os Livros (EPUB) que o código não cumpria.

use neural_core::{ProviderId, is_ai_provider_host};
use url::Url;

const PAGE_SCRIPTS: &str = include_str!("../src/windows_app/page_scripts.rs");
const README: &str = include_str!("../../../README.md");

const FUNCTION_NAME: &str = "onProviderPage";
const DECLARATION: &str = "function onProviderPage() {";

/// Onde o script é mais largo do que o registro, de propósito e à vista.
///
/// `*.google.com` com o primeiro `udm` igual a `50`: o script (congelado)
/// trata qualquer `*.google.com?udm=50` como a página do Google IA; o registro
/// aceita só `google.com` e `www.google.com`, porque em `sites.google.com`
/// qualquer pessoa publica páginas e o registro decide o que o adblock e a
/// anti-distração nunca tocam (auditoria 2.2.0,
/// `core-registry-predicate-scheme-subdomains`). Estreitar o script mexe em
/// `crates/neural-app/src` e num script injetado: fica para quem tem essa área
/// e para o dono. O gate exige que esta divergência continue verdadeira (o
/// script ainda aceita um `sites.google.com?udm=50` que o registro recusa),
/// para ela não ficar esquecida aqui depois de o script ser corrigido.
fn known_wider_in_script(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    host.ends_with(".google.com") && first_query_value(url, "udm").as_deref() == Some("50")
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

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Cada ocorrência do nome `onProviderPage` no script é a declaração (uma
/// só) ou uma chamada `onProviderPage()`. Em JS a última declaração ganha e
/// uma atribuição troca a função: nenhuma das duas pode passar sem o gate
/// ver.
fn declared_once_and_only_called(script: &str) -> Result<(), String> {
    let mut declarations = 0;
    for (at, _) in script.match_indices(FUNCTION_NAME) {
        let before = script[..at].chars().next_back();
        let after_at = at + FUNCTION_NAME.len();
        let after = script[after_at..].chars().next();
        if before.is_some_and(is_ident_char) || after.is_some_and(is_ident_char) {
            continue;
        }
        if script[..at].ends_with("function ") {
            declarations += 1;
        } else if !script[after_at..].starts_with("()") {
            let line = script[at..].lines().next().unwrap_or_default();
            return Err(format!(
                "uso de {FUNCTION_NAME} que o gate não conhece (nem declaração nem chamada): {line:?}"
            ));
        }
    }
    if declarations != 1 {
        return Err(format!(
            "{FUNCTION_NAME} declarada {declarations} vezes no script (tem de ser 1)"
        ));
    }
    Ok(())
}

/// O corpo (entre as chavetas) de `function onProviderPage()`. As chavetas
/// dentro de aspas não contam; o que o corpo tem é o tokenizador que decide.
fn on_provider_page(script: &str) -> Result<&str, String> {
    let start = script
        .find(DECLARATION)
        .ok_or_else(|| format!("{DECLARATION} não está no script"))?;
    let open = start + DECLARATION.len() - 1;
    let mut depth = 0usize;
    let mut quote = None;
    for (offset, c) in script[open..].char_indices() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(c),
            (None, '{') => depth += 1,
            (None, '}') => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&script[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }
    Err(format!("{FUNCTION_NAME} sem chaveta de fecho"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    Str(String),
    Punct(&'static str),
}

impl Token {
    fn text(&self) -> String {
        match self {
            Token::Ident(ident) => ident.clone(),
            Token::Str(literal) => format!("'{literal}'"),
            Token::Punct(punct) => (*punct).to_string(),
        }
    }
}

/// Os símbolos do corpo. Só identificadores, strings entre aspas simples
/// (letras, dígitos, `.`, `-` e `_`, sem escapes) e a pontuação da
/// gramática; qualquer outro símbolo (aspas duplas, crases, números, `!`,
/// `>`, comentários...) é recusado.
fn tokenize(body: &str) -> Result<Vec<Token>, String> {
    const PUNCTS: [&str; 10] = ["===", "||", "&&", "(", ")", "{", "}", ";", ".", "="];
    let mut tokens = Vec::new();
    let mut rest = body;
    loop {
        rest = rest.trim_start();
        let Some(c) = rest.chars().next() else {
            return Ok(tokens);
        };
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let len = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
            tokens.push(Token::Ident(rest[..len].to_string()));
            rest = &rest[len..];
        } else if c == '\'' {
            let len = rest[1..]
                .find('\'')
                .ok_or_else(|| format!("string sem fecho em {FUNCTION_NAME}"))?;
            let literal = &rest[1..1 + len];
            if !literal
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            {
                return Err(format!(
                    "string que o gate não conhece em {FUNCTION_NAME}: {literal:?}"
                ));
            }
            tokens.push(Token::Str(literal.to_string()));
            rest = &rest[len + 2..];
        } else if let Some(punct) = PUNCTS.iter().find(|punct| rest.starts_with(**punct)) {
            tokens.push(Token::Punct(*punct));
            rest = &rest[punct.len()..];
        } else {
            let near: String = rest.chars().take(24).collect();
            return Err(format!(
                "símbolo que o gate não conhece em {FUNCTION_NAME}: {near:?}"
            ));
        }
    }
}

/// Uma condição da função, com a árvore que a precedência do JS dá.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Or(Vec<Expr>),
    And(Vec<Expr>),
    /// `host === '<literal>'`
    HostIs(String),
    /// `host.endsWith('<literal>')`
    HostEndsWith(String),
    /// `here.searchParams.get('<name>') === '<value>'`
    Param {
        name: String,
        value: String,
    },
    Bool(bool),
}

/// A função: `if (<cond>) return <bool>;` na ordem, e o `return <cond>;`
/// final.
#[derive(Debug)]
struct Program {
    rules: Vec<(Expr, bool)>,
    fallback: Expr,
}

/// O começo que a função tem de ter, símbolo a símbolo: `host` é o
/// `hostname` em minúsculas do endereço da página, e um endereço que não se
/// lê dá `false`.
const PROLOGUE: &str = "let here ; try { here = new URL ( location . href ) ; } catch ( _ ) { return false ; } const host = here . hostname . toLowerCase ( ) ;";

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<String> {
        self.tokens.get(self.at).map(Token::text)
    }

    fn unknown(&self) -> String {
        let near: Vec<String> = self.tokens[self.at.min(self.tokens.len())..]
            .iter()
            .take(8)
            .map(Token::text)
            .collect();
        format!(
            "formato que o gate não conhece em {FUNCTION_NAME}: {:?}",
            near.join(" ")
        )
    }

    fn eat(&mut self, text: &str) -> Result<(), String> {
        if self.peek().as_deref() == Some(text) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.unknown())
        }
    }

    fn eat_all(&mut self, texts: &str) -> Result<(), String> {
        texts.split_whitespace().try_for_each(|text| self.eat(text))
    }

    fn string(&mut self) -> Result<String, String> {
        match self.tokens.get(self.at) {
            Some(Token::Str(literal)) => {
                self.at += 1;
                Ok(literal.clone())
            }
            _ => Err(self.unknown()),
        }
    }

    fn boolean(&mut self) -> Result<bool, String> {
        let value = match self.peek().as_deref() {
            Some("true") => true,
            Some("false") => false,
            _ => return Err(self.unknown()),
        };
        self.at += 1;
        Ok(value)
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut terms = vec![self.and()?];
        while self.peek().as_deref() == Some("||") {
            self.at += 1;
            terms.push(self.and()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Expr::Or(terms)
        })
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut terms = vec![self.primary()?];
        while self.peek().as_deref() == Some("&&") {
            self.at += 1;
            terms.push(self.primary()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Expr::And(terms)
        })
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek().as_deref() {
            Some("(") => {
                self.at += 1;
                let inner = self.or()?;
                self.eat(")")?;
                Ok(inner)
            }
            Some("true" | "false") => Ok(Expr::Bool(self.boolean()?)),
            Some("host") => {
                self.at += 1;
                if self.peek().as_deref() == Some("===") {
                    self.at += 1;
                    return Ok(Expr::HostIs(self.string()?));
                }
                self.eat_all(". endsWith (")?;
                let literal = self.string()?;
                self.eat(")")?;
                Ok(Expr::HostEndsWith(literal))
            }
            Some("here") => {
                self.eat_all("here . searchParams . get (")?;
                let name = self.string()?;
                self.eat_all(") ===")?;
                let value = self.string()?;
                Ok(Expr::Param { name, value })
            }
            _ => Err(self.unknown()),
        }
    }

    fn program(mut self) -> Result<Program, String> {
        self.eat_all(PROLOGUE)?;
        let mut rules = Vec::new();
        loop {
            match self.peek().as_deref() {
                Some("if") => {
                    self.eat_all("if (")?;
                    let condition = self.or()?;
                    self.eat_all(") return")?;
                    let value = self.boolean()?;
                    self.eat(";")?;
                    rules.push((condition, value));
                }
                Some("return") => {
                    self.at += 1;
                    let fallback = self.or()?;
                    self.eat(";")?;
                    if self.at != self.tokens.len() {
                        return Err(self.unknown());
                    }
                    return Ok(Program { rules, fallback });
                }
                _ => return Err(self.unknown()),
            }
        }
    }
}

/// O primeiro valor de `name` na query, como o `URLSearchParams.get`.
fn first_query_value(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

impl Expr {
    fn eval(&self, host: &str, url: &Url) -> bool {
        match self {
            Expr::Or(terms) => terms.iter().any(|term| term.eval(host, url)),
            Expr::And(terms) => terms.iter().all(|term| term.eval(host, url)),
            Expr::HostIs(literal) => host == literal,
            Expr::HostEndsWith(literal) => host.ends_with(literal.as_str()),
            Expr::Param { name, value } => {
                first_query_value(url, name).as_deref() == Some(value.as_str())
            }
            Expr::Bool(value) => *value,
        }
    }

    fn literals<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        match self {
            Expr::Or(terms) | Expr::And(terms) => {
                terms.iter().for_each(|term| term.literals(out));
            }
            Expr::HostIs(_) | Expr::HostEndsWith(_) => out.push(self),
            Expr::Param { .. } | Expr::Bool(_) => {}
        }
    }
}

impl Program {
    /// O que a função devolve com `location.href` = `url`.
    fn accepts(&self, url: &Url) -> bool {
        let host = url.host_str().unwrap_or_default().to_lowercase();
        for (condition, value) in &self.rules {
            if condition.eval(&host, url) {
                return *value;
            }
        }
        self.fallback.eval(&host, url)
    }

    fn host_literals(&self) -> Vec<&Expr> {
        let mut out = Vec::new();
        for (condition, _) in &self.rules {
            condition.literals(&mut out);
        }
        self.fallback.literals(&mut out);
        out
    }
}

/// A função `onProviderPage` do script, lida pela gramática fechada.
fn provider_page_program(source: &str) -> Result<Program, String> {
    let script = comparator_script(source);
    declared_once_and_only_called(&script)?;
    let tokens = tokenize(on_provider_page(&script)?)?;
    Parser { tokens, at: 0 }.program()
}

const QUERIES: [&str; 6] = [
    "",
    "search?q=x",
    "search?q=x&udm=50",
    "search?q=x&udm=14",
    "search?q=x&udm=14&udm=50",
    "search?udm=50&udm=14",
];

fn urls_for(host: &str) -> Vec<Url> {
    QUERIES
        .iter()
        .filter_map(|query| Url::parse(&format!("https://{host}/{query}")).ok())
        .collect()
}

/// A tabela: cada host do registro com os seus subdomínios e lookalikes,
/// cada literal do script com os dele, e hosts fixos (um qualquer, o Google
/// e o `sites.google.com`), cada um com e sem `udm=50`. Devolve os endereços
/// e, à parte, os lookalikes (que ninguém pode aceitar).
fn probe_table(program: &Program) -> (Vec<Url>, Vec<Url>) {
    let mut hosts: Vec<String> = [
        "evil.io",
        "google.com",
        "www.google.com",
        "sites.google.com",
        "accounts.google.com",
    ]
    .map(String::from)
    .to_vec();
    let mut lookalikes: Vec<String> = Vec::new();
    let mut base_of = |host: &str, hosts: &mut Vec<String>| {
        hosts.push(host.to_string());
        hosts.push(format!("sub.{host}"));
        lookalikes.push(format!("evil{host}"));
        lookalikes.push(format!("{host}.evil.io"));
    };
    for provider in ProviderId::all() {
        for rule in provider.hosts() {
            base_of(rule.host, &mut hosts);
        }
    }
    for literal in program.host_literals() {
        match literal {
            Expr::HostIs(host) => base_of(host, &mut hosts),
            Expr::HostEndsWith(suffix) => match suffix.strip_prefix('.') {
                Some(base) => base_of(base, &mut hosts),
                None => base_of(suffix, &mut hosts),
            },
            _ => {}
        }
    }
    let urls = hosts.iter().flat_map(|host| urls_for(host)).collect();
    let lookalikes = lookalikes.iter().flat_map(|host| urls_for(host)).collect();
    (urls, lookalikes)
}

/// As divergências entre o script e o registro (vazio = consistente).
fn violations(source: &str) -> Vec<String> {
    let program = match provider_page_program(source) {
        Ok(program) => program,
        Err(error) => return vec![error],
    };
    let mut problems = Vec::new();
    for literal in program.host_literals() {
        if let Expr::HostEndsWith(suffix) = literal
            && !suffix.starts_with('.')
        {
            problems.push(format!(
                "endsWith('{suffix}') sem o ponto aceita lookalikes (evil{suffix})"
            ));
        }
    }
    let (urls, lookalikes) = probe_table(&program);
    let mut wider_seen = false;
    for url in urls.iter().chain(&lookalikes) {
        if !program.accepts(url) || is_ai_provider_host(url) {
            continue;
        }
        if known_wider_in_script(url) {
            wider_seen = true;
        } else {
            problems.push(format!("o script aceita {url} e o registro recusa"));
        }
    }
    // Um lookalike que o script aceite e o registro recuse já saiu acima.
    for url in &lookalikes {
        if is_ai_provider_host(url) {
            problems.push(format!("o registro aceita o lookalike {url}"));
        }
    }
    if !wider_seen {
        problems.push(
            "*.google.com?udm=50 já não é mais largo no script: tire-o de known_wider_in_script"
                .to_string(),
        );
    }
    // Sem isto uma função que recusa tudo passaria: o script tem de
    // reconhecer as páginas que os 3 slots padrão abrem.
    for provider in ProviderId::default_slots() {
        let url = provider.search_url("x", "pt-BR").expect("URL do slot");
        if !program.accepts(&url) {
            problems.push(format!(
                "o script não reconhece a página do slot padrão {url}"
            ));
        }
    }
    problems
}

#[test]
fn script_hosts_subset_of_registry() {
    let program = provider_page_program(PAGE_SCRIPTS).unwrap();
    // A função foi lida, e é a que embarca: os 3 `if` e o `return` final,
    // e o que ela aceita e recusa de facto.
    assert_eq!(program.rules.len(), 3, "{program:#?}");
    for (url, expected) in [
        ("https://chatgpt.com/", true),
        ("https://sub.chatgpt.com/", true),
        ("https://chat.openai.com/", true),
        ("https://claude.ai/new", true),
        ("https://gemini.google.com/app", true),
        ("https://www.google.com/search?q=x&udm=50", true),
        ("https://www.google.com/search?q=x", false),
        ("https://www.google.com/search?q=x&udm=14&udm=50", false),
        ("https://evilchatgpt.com/", false),
        ("https://chatgpt.com.evil.io/", false),
        ("https://perplexity.ai/", false),
    ] {
        let url = Url::parse(url).unwrap();
        assert_eq!(program.accepts(&url), expected, "{url}");
    }
    let problems = violations(PAGE_SCRIPTS);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Controle negativo do leitor (não é a prova de sabotagem, que quebra o
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
        ("if (true) return true;", "https://evil.io/"),
        ("// if (host === 'evil.io') return true;", "não conhece"),
    ] {
        let drifted = source.replacen(declaration, &format!("{declaration}\n    {drift}"), 1);
        let problems = violations(&drifted);
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "{drift}: {problems:#?}"
        );
    }
}

/// As derivas da revisão (`rev-consistency-gate-misses-drifts`): cada uma faz
/// o `onProviderPage` aceitar uma página que o registro recusa, e cada uma
/// tem de dar divergência.
#[test]
fn drifts_that_accept_pages_the_registry_refuses_are_caught() {
    let source = normalized(PAGE_SCRIPTS);
    let declaration = "const host = here.hostname.toLowerCase();";
    let google_return = "return (host === 'google.com' || host.endsWith('.google.com'))\n      && here.searchParams.get('udm') === '50';";
    let function_end = format!("{google_return}\n  }}\n");
    for anchor in [declaration, google_return, function_end.as_str()] {
        assert_eq!(source.matches(anchor).count(), 1, "{anchor}");
    }
    let after_declaration = |line: &str| (declaration, format!("{declaration}\n    {line}"));
    for (label, (anchor, replacement), expected) in [
        (
            "aspas duplas e return 1",
            after_declaration(r#"if (here.origin === "https://evil.io") return 1;"#),
            "não conhece",
        ),
        (
            "indexOf e return !0",
            after_declaration(r#"if (location.href.indexOf("evil.io") >= 0) return !0;"#),
            "não conhece",
        ),
        (
            "precedência do || e do &&",
            (
                google_return,
                "return host === 'www.google.com' || (host === 'google.com' || host.endsWith('.google.com'))\n      && here.searchParams.get('udm') === '50';".to_string(),
            ),
            "https://www.google.com/search?q=x e o registro recusa",
        ),
        (
            "segunda declaração",
            (
                function_end.as_str(),
                format!("{function_end}  function onProviderPage() {{ return true; }}\n"),
            ),
            "declarada 2 vezes",
        ),
        (
            "reatribuição",
            (
                function_end.as_str(),
                format!("{function_end}  onProviderPage = function () {{ return true; }};\n"),
            ),
            "nem declaração nem chamada",
        ),
        (
            "um if que devolve true a qualquer Google",
            after_declaration("if (host.endsWith('.google.com')) return true;"),
            "https://sites.google.com/ e o registro recusa",
        ),
    ] {
        let drifted = source.replacen(anchor, &replacement, 1);
        assert_ne!(drifted, source, "{label}: a deriva não entrou");
        let problems = violations(&drifted);
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "{label}: {problems:#?}"
        );
    }
}

/// A leitura segue a precedência do JavaScript: `a || b && c` é
/// `a || (b && c)`, não `(a || b) && c`.
#[test]
fn the_reader_gives_and_precedence_over_or() {
    let tokens = tokenize("host === 'a' || host === 'b' && host === 'c'").unwrap();
    let expr = Parser { tokens, at: 0 }.or().unwrap();
    assert_eq!(
        expr,
        Expr::Or(vec![
            Expr::HostIs("a".into()),
            Expr::And(vec![Expr::HostIs("b".into()), Expr::HostIs("c".into())]),
        ])
    );
    let url = Url::parse("https://a/").unwrap();
    assert!(expr.eval("a", &url));
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
