//! Gemini Live: o painel onde o modelo ve a tela, a camera e ouve o microfone
//! (pedido do dono, 23/09/2026: um botao na barra liga e desliga).
//!
//! Aqui fica tudo o que nao precisa de uma janela, para se testar sem ecra:
//!
//! - a origem propria `neuralia-live`, que o wry serve no Windows como
//!   `http://neuralia-live.localhost`. O Chromium trata `*.localhost` como
//!   contexto seguro -- sem isso nao ha `getUserMedia` nem `getDisplayMedia`.
//!   Servem-se so quatro caminhos exatos; o resto e 404;
//! - a lista fechada do canal do painel (`parse_live_message`), com tecto de
//!   tamanho, e a validacao da chave;
//! - a chave, cifrada com DPAPI para o utilizador atual em
//!   `<data_dir>/gemini-live.key`, escrita de forma atomica (a DPAPI e o
//!   ficheiro vivem em `secrets.rs`; aqui ficam o nome, o cabecalho e a
//!   entropia do Live, os de sempre);
//! - os scripts que o nativo corre na pagina. A chave so entra na pagina
//!   quando a sessao arranca, como literal JSON, nunca no URL;
//! - o estado do painel e do olho da barra numa peca so (`LivePanel`).
//!
//! A redacao do log de depuracao (`secrets::redact_debug_secrets`) garante
//! que a chave nunca chega ao disco em claro. A ligacao a janela (botao,
//! painel, eventos) esta em `windows_app.rs`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use url::Url;
use wry::http::{Method, Request, Response as HttpResponse, StatusCode};

use crate::secrets::{SecretFile, wipe};

/// Nome do esquema registado no wry.
pub(crate) const LIVE_PROTOCOL: &str = "neuralia-live";
/// Como o WebView2 ve o esquema (`http://<esquema>.localhost`).
pub(crate) const LIVE_ORIGIN: &str = "http://neuralia-live.localhost";
const LIVE_HOST: &str = "neuralia-live.localhost";
const LIVE_PAGE_PATH: &str = "/live.html";

pub(crate) const LIVE_HTML: &str = include_str!("../../../assets/live/live.html");
pub(crate) const LIVE_CSS: &str = include_str!("../../../assets/live/live.css");
/// Protocolo e sessao, sem DOM: e ESTE texto que os gates correm em Node.
pub(crate) const LIVE_CORE_JS: &str = include_str!("../../../assets/live/live-core.js");
/// A interface do painel.
pub(crate) const LIVE_APP_JS: &str = include_str!("../../../assets/live/live.js");

/// Nada de scripts de fora nem de inline: so os dois ficheiros desta origem e
/// o worklet do microfone (Blob URL criado pela propria pagina). A unica
/// ligacao de rede que a pagina pode abrir e o WebSocket do Gemini.
pub(crate) const LIVE_CSP: &str = "default-src 'none'; script-src 'self' blob:; \
     style-src 'self'; media-src 'self' blob:; \
     connect-src wss://generativelanguage.googleapis.com; base-uri 'none'; \
     form-action 'none'; frame-ancestors 'none'; object-src 'none'";
const LIVE_PERMISSIONS_POLICY: &str =
    "camera=(self), microphone=(self), display-capture=(self), geolocation=(), payment=(), usb=()";

/// O endereco que o painel abre.
pub(crate) fn live_page_url() -> String {
    format!("{LIVE_ORIGIN}{LIVE_PAGE_PATH}")
}

/// Os unicos caminhos que a origem serve, com o tipo de cada um.
fn live_asset(path: &str) -> Option<(&'static str, &'static str)> {
    Some(match path {
        LIVE_PAGE_PATH => ("text/html; charset=utf-8", LIVE_HTML),
        "/live.css" => ("text/css; charset=utf-8", LIVE_CSS),
        "/live-core.js" => ("text/javascript; charset=utf-8", LIVE_CORE_JS),
        "/live.js" => ("text/javascript; charset=utf-8", LIVE_APP_JS),
        _ => return None,
    })
}

/// Responde a origem do painel. O wry entrega o pedido ja com o URI desfeito
/// do contorno do WebView2 (`neuralia-live://localhost/...`); o filtro dele
/// apanha `http://neuralia-live.*`, por isso o host tem de ser conferido aqui
/// -- `http://neuralia-live.exemplo.com/live.html` chega como
/// `neuralia-live://exemplo.com/live.html` e leva 404.
pub(crate) fn serve_live_asset(request: &Request<Vec<u8>>) -> HttpResponse<Cow<'static, [u8]>> {
    let uri = request.uri();
    let asset = if request.method() == Method::GET
        && uri.scheme_str() == Some(LIVE_PROTOCOL)
        && uri.host() == Some("localhost")
        && uri.port().is_none()
    {
        live_asset(uri.path())
    } else {
        None
    };
    let (status, content_type, body) = match asset {
        Some((content_type, text)) => (StatusCode::OK, content_type, text.as_bytes()),
        None => (
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            b"not found" as &[u8],
        ),
    };
    let mut response = HttpResponse::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer");
    if content_type.starts_with("text/html") {
        response = response
            .header("Content-Security-Policy", LIVE_CSP)
            .header("Permissions-Policy", LIVE_PERMISSIONS_POLICY);
    }
    response.body(Cow::Borrowed(body)).unwrap_or_else(|_| {
        let mut fallback = HttpResponse::new(Cow::Borrowed(b"" as &[u8]));
        *fallback.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    })
}

/// O painel so carrega a sua pagina: nem outro caminho da mesma origem, nem
/// outra porta, nem https, nem `neuralia-live.localhost.exemplo.com`, nem um
/// link para a AI Studio. A mesma regra decide de onde vem uma mensagem do
/// canal (`request.uri()` e o documento que a publicou).
pub(crate) fn live_panel_allows_navigation(target: &str) -> bool {
    Url::parse(target.trim()).is_ok_and(|url| {
        url.scheme() == "http"
            && url.host_str() == Some(LIVE_HOST)
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == LIVE_PAGE_PATH
    })
}

/// Uma mensagem do canal so conta se o documento que a publicou e a pagina
/// do painel (o wry entrega-o em `request.uri()`).
pub(crate) fn live_ipc_source_ok(source: &str) -> bool {
    live_panel_allows_navigation(source)
}

/// O porteiro inteiro do canal do painel: a origem E a lista fechada. E isto
/// que o handler de IPC do painel chama, e o que os gates exercitam.
pub(crate) fn live_ipc_message(source: &str, body: &str) -> Option<LiveMessage> {
    if !live_ipc_source_ok(source) {
        return None;
    }
    parse_live_message(body)
}

/// A regra de navegacao com a assinatura que o wry pede. O painel passa esta
/// funcao ao `with_navigation_handler`, sem closure pelo meio.
pub(crate) fn live_panel_navigation(target: String) -> bool {
    live_panel_allows_navigation(&target)
}

/// A chave da API. Nunca se imprime: o `Debug` diz so que existe, para um
/// `{:?}` de um evento (o `UserEvent` deriva Debug) nao a deixar num log.
pub(crate) struct LiveKey(String);

impl LiveKey {
    /// O texto da chave, para o cofre e para o script que arranca a sessao.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for LiveKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LiveKey(<omitida>)")
    }
}

impl PartialEq for LiveKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for LiveKey {}

/// Zerada ao sair de cena (`secrets::wipe`). E so higiene, nao uma garantia:
/// a chave passa por copias que ninguem zera -- o corpo da mensagem que o
/// wry entrega ao handler, o `serde_json::Value` de `parse_live_message`, o
/// script de arranque e o HSTRING em que o wry o converte, e a propria
/// pagina no processo do WebView2. Um despejo de memoria do processo pode ter
/// a chave. O que a protege de verdade e a DPAPI no disco e ela nunca ir para
/// o log (`redact_debug_secrets`).
impl Drop for LiveKey {
    fn drop(&mut self) {
        wipe(unsafe { self.0.as_mut_vec() });
    }
}

const LIVE_KEY_MIN_CHARS: usize = 20;
const LIVE_KEY_MAX_CHARS: usize = 256;

/// As chaves da AI Studio sao ASCII (`AIza...`); aceita-se o alfabeto delas
/// com folga no comprimento, e nada mais -- nem espacos, nem aspas, nem `<`.
pub(crate) fn validate_live_key(raw: &str) -> Option<LiveKey> {
    let key = raw.trim();
    let valid = (LIVE_KEY_MIN_CHARS..=LIVE_KEY_MAX_CHARS).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    valid.then(|| LiveKey(key.to_string()))
}

/// O que a pagina do painel pode pedir. Lista fechada.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LiveMessage {
    /// A pagina carregou, ou o utilizador pediu "Conectar de novo": arranca
    /// com a chave guardada ou pede uma.
    Ready,
    SaveKey(LiveKey),
    /// Pediu para guardar algo que nao tem forma de chave: volta ao ecra da
    /// chave com o aviso, em vez de a pagina ficar a espera para sempre.
    InvalidKey,
    ForgetKey,
    /// A sessao acabou sem o utilizador a desligar (a ligacao caiu, o Google
    /// recusou): ja nada e capturado nem enviado, e o olho deixa o vermelho.
    Stopped,
    Close,
}

/// Chega para uma chave de 256 caracteres dentro do envelope.
pub(crate) const LIVE_MESSAGE_MAX_BYTES: usize = 1024;

pub(crate) fn parse_live_message(body: &str) -> Option<LiveMessage> {
    if body.len() > LIVE_MESSAGE_MAX_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let envelope = value.as_object()?;
    if envelope
        .keys()
        .any(|field| field != "action" && field != "args")
    {
        return None;
    }
    match envelope.get("action")?.as_str()? {
        "ready" => Some(LiveMessage::Ready),
        "close" => Some(LiveMessage::Close),
        "forget_key" => Some(LiveMessage::ForgetKey),
        "stopped" => Some(LiveMessage::Stopped),
        "save_key" => {
            let raw = envelope.get("args")?.get("key")?.as_str()?;
            Some(validate_live_key(raw).map_or(LiveMessage::InvalidKey, LiveMessage::SaveKey))
        }
        _ => None,
    }
}

/// O nome do ficheiro, o cabecalho e a entropia da chave do Live. Os tres
/// sao o formato em disco: um `gemini-live.key` gravado por qualquer versao
/// anterior tem de continuar a abrir (gate
/// `a_live_key_file_from_before_the_refactor_still_loads` em `secrets.rs`).
pub(crate) const LIVE_KEY_FILE: &str = "gemini-live.key";
/// Cabecalho do ficheiro: identifica o formato antes do blob da DPAPI.
pub(crate) const LIVE_KEY_MAGIC: &[u8; 4] = b"NLK1";
/// Entropia extra da DPAPI: outro programa do mesmo utilizador que chame
/// `CryptUnprotectData` sobre o ficheiro sem ela nao o abre.
pub(crate) const LIVE_KEY_ENTROPY: &[u8] = b"NeuralIA/gemini-live/v1";

/// A chave guardada em disco, cifrada com a DPAPI do utilizador atual (o
/// `SecretFile` de `secrets.rs`, com o cabecalho e a entropia do Live).
pub(crate) struct LiveKeyStore {
    file: SecretFile,
}

impl LiveKeyStore {
    pub(crate) fn in_dir(data_dir: &Path) -> Self {
        Self::at(data_dir.join(LIVE_KEY_FILE))
    }

    /// No caminho que o grant do cofre deu (`stores::LIVE_KEY_STORE`).
    pub(crate) fn at(path: PathBuf) -> Self {
        Self {
            file: SecretFile::new(path, *LIVE_KEY_MAGIC, LIVE_KEY_ENTROPY),
        }
    }

    pub(crate) fn secret_file(&self) -> &SecretFile {
        &self.file
    }

    /// A chave, ou `None` se nao houver ficheiro ou ele nao abrir -- um
    /// ficheiro estragado vale como "sem chave" (volta-se a pedir), nunca
    /// derruba o app.
    pub(crate) fn load(&self) -> Option<LiveKey> {
        let plain = self.file.load()?;
        validate_live_key(std::str::from_utf8(plain.bytes()).ok()?)
    }

    /// Escreve num temporario e troca: um corte a meio deixa a chave antiga
    /// ou a nova, nunca meio ficheiro.
    pub(crate) fn save(&self, key: &LiveKey) -> std::io::Result<()> {
        self.file.save(key.expose().as_bytes())
    }

    /// "Trocar chave": apaga o ficheiro. Sem ficheiro ja esta esquecida.
    pub(crate) fn forget(&self) -> std::io::Result<()> {
        self.file.forget()
    }
}

/// O que o nativo corre na pagina do painel. Os dados vao como literal JSON.
fn live_call(method: &str, config: &serde_json::Value) -> String {
    format!("window.__neuraliaLive && window.__neuraliaLive.{method}({config});")
}

/// Arranca a sessao: e o unico sitio onde a chave entra na pagina.
pub(crate) fn live_start_script(
    key: &LiveKey,
    theme: &serde_json::Value,
    notice: Option<&str>,
) -> String {
    live_call(
        "start",
        &serde_json::json!({ "theme": theme, "key": key.expose(), "notice": notice }),
    )
}

pub(crate) fn live_ask_key_script(theme: &serde_json::Value, error: Option<&str>) -> String {
    live_call(
        "askKey",
        &serde_json::json!({ "theme": theme, "error": error }),
    )
}

pub(crate) fn live_theme_script(theme: &serde_json::Value) -> String {
    live_call("theme", theme)
}

/// O que fazer com uma mensagem do painel. Separado da janela para se testar:
/// o `App` so corre o script devolvido ou fecha o painel.
pub(crate) enum LiveStep {
    /// Correr na pagina o script que arranca a sessao: a partir daqui a tela,
    /// a camera e o microfone podem estar a sair. Leva a chave: nao tem
    /// `Debug` de proposito.
    Start(String),
    /// Correr na pagina o script do ecra da chave: nada e capturado.
    AskKey(String),
    /// A pagina diz que a sessao acabou: nada a correr, so o olho muda.
    Stopped,
    Close,
}

pub(crate) fn live_step(
    message: LiveMessage,
    store: &LiveKeyStore,
    theme: &serde_json::Value,
) -> LiveStep {
    match message {
        LiveMessage::Ready => match store.load() {
            Some(key) => LiveStep::Start(live_start_script(&key, theme, None)),
            None => LiveStep::AskKey(live_ask_key_script(theme, None)),
        },
        // Mesmo sem conseguir guardar, a sessao arranca com a chave que o
        // utilizador acabou de dar -- e o aviso diz que vale so desta vez.
        LiveMessage::SaveKey(key) => LiveStep::Start(match store.save(&key) {
            Ok(()) => live_start_script(&key, theme, None),
            Err(error) => live_start_script(
                &key,
                theme,
                Some(&format!(
                    "Não foi possível salvar a chave ({error}); ela vale só para esta sessão."
                )),
            ),
        }),
        LiveMessage::InvalidKey => {
            LiveStep::AskKey(live_ask_key_script(theme, Some(LIVE_INVALID_KEY_NOTICE)))
        }
        LiveMessage::ForgetKey => LiveStep::AskKey(match store.forget() {
            Ok(()) => live_ask_key_script(theme, None),
            Err(error) => live_ask_key_script(
                theme,
                Some(&format!("Não foi possível apagar a chave antiga: {error}")),
            ),
        }),
        LiveMessage::Stopped => LiveStep::Stopped,
        LiveMessage::Close => LiveStep::Close,
    }
}

/// O aviso de uma chave com forma errada. A pagina mostra o mesmo texto
/// quando recusa sozinha uma colagem grande demais para o canal.
pub(crate) const LIVE_INVALID_KEY_NOTICE: &str =
    "Isso não parece uma chave da API do Gemini. Copie de novo da AI Studio.";

/// O que o `App` faz depois de `LivePanel::follow`. `Run` leva o script (e,
/// no arranque, a chave): sem `Debug` de proposito.
pub(crate) enum LiveAction {
    Run(String),
    Close,
    Nothing,
}

/// O que o olho da barra mostra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveIndicator {
    /// Painel fechado: nada e capturado.
    Off,
    /// Painel aberto sem sessao (a pedir a chave, ou a sessao acabou): nada
    /// e capturado nem enviado, mas o painel continua la.
    Standby,
    /// O nativo mandou a pagina arrancar: a tela, a camera e o microfone podem
    /// estar a sair para o Google.
    Live,
}

/// O painel do Gemini Live e o estado do olho numa peca so: o olho e lido
/// daqui, nunca de uma bandeira a parte que alguem se esqueca de repor.
/// Generico na vista para os gates correrem sem janela; no app e `WebView`.
///
/// A pagina so comeca a capturar quando o nativo corre o `start()` dela
/// (`LiveStep::Start`), por isso o vermelho comeca ai -- antes de o script
/// correr -- e so acaba quando a pagina diz que parou ou o painel fecha.
pub(crate) struct LivePanel<W> {
    view: Option<W>,
    started: bool,
}

impl<W> LivePanel<W> {
    pub(crate) const fn off() -> Self {
        Self {
            view: None,
            started: false,
        }
    }

    /// Abre com a vista nova (uma que ja estivesse aberta e largada).
    pub(crate) fn open(&mut self, view: W) {
        self.started = false;
        self.view = Some(view);
    }

    /// Fecha: devolve a vista para o chamador a esconder e largar.
    pub(crate) fn close(&mut self) -> Option<W> {
        self.started = false;
        self.view.take()
    }

    pub(crate) fn view(&self) -> Option<&W> {
        self.view.as_ref()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.view.is_some()
    }

    /// Acompanha o passo e devolve o que o `App` faz a seguir. O script so
    /// sai daqui DEPOIS de o estado do olho mudar: nao ha forma de arrancar a
    /// sessao sem o olho ficar vermelho, nem de o esquecer. Sem painel aberto
    /// nao ha onde correr nada.
    pub(crate) fn follow(&mut self, step: LiveStep) -> LiveAction {
        if self.view.is_none() {
            return match step {
                LiveStep::Close => LiveAction::Close,
                _ => LiveAction::Nothing,
            };
        }
        match step {
            LiveStep::Start(script) => {
                self.started = true;
                LiveAction::Run(script)
            }
            LiveStep::AskKey(script) => {
                self.started = false;
                LiveAction::Run(script)
            }
            LiveStep::Stopped => {
                self.started = false;
                LiveAction::Nothing
            }
            LiveStep::Close => LiveAction::Close,
        }
    }

    pub(crate) fn indicator(&self) -> LiveIndicator {
        match (&self.view, self.started) {
            (None, _) => LiveIndicator::Off,
            (Some(_), false) => LiveIndicator::Standby,
            (Some(_), true) => LiveIndicator::Live,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{
        blob, dpapi_protect, dpapi_unprotect, redact_debug_secrets, take_dpapi_output,
    };
    use serde_json::{Value, json};
    use std::process::{Command, Stdio};

    /// Uma chave com a forma das da AI Studio. Nao e uma chave real. Montada
    /// com `concat!` para o texto do codigo nao ter a forma de uma chave
    /// Google: o scanner de segredos do PR (GitGuardian) acusava-a.
    const TEST_KEY: &str = concat!("AIza", "SyTESTONLY-not-a-real-key_0123456789");
    const OTHER_KEY: &str = concat!("AIza", "SyOTHERTEST-still-not-a-real-key-42");

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neuralia-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pasta temporaria");
        dir
    }

    fn key(text: &str) -> LiveKey {
        validate_live_key(text).expect("chave de teste valida")
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    // ------------------------------------------------------------ Node

    /// Corre `body` (corpo de uma funcao async com `core` e `input`) em Node,
    /// com o `live-core.js` QUE EMBARCA carregado no contexto. Devolve o JSON
    /// que o corpo devolver.
    fn node_core(body: &str, input: Value) -> Value {
        const HARNESS: &str = r#"
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
vm.runInThisContext(input.core, { filename: 'live-core.js' });
const core = globalThis.NeuraliaLiveCore;
const AsyncFunction = (async () => {}).constructor;
new AsyncFunction('core', 'input', input.body)(core, input.data)
  .then((out) => process.stdout.write(JSON.stringify(out === undefined ? null : out)))
  .catch((error) => { process.stderr.write(String((error && error.stack) || error)); process.exit(1); });
"#;
        run_node(
            HARNESS,
            json!({ "core": LIVE_CORE_JS, "body": body, "data": input }),
        )
    }

    fn run_node(harness: &str, input: Value) -> Value {
        use std::io::Write as _;
        let mut child = Command::new("node")
            .arg("-e")
            .arg(harness)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("os gates do Gemini Live precisam do `node` no PATH (o CI ja o usa)");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.to_string().as_bytes())
            .expect("escrever o cenario");
        let output = child.wait_with_output().expect("node terminou");
        assert!(
            output.status.success(),
            "harness falhou: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("JSON do harness")
    }

    /// O que um script do nativo chama na pagina: metodo e argumento.
    fn page_call(script: &str) -> (String, Value) {
        let out = node_core(
            r#"
const calls = [];
globalThis.window = { __neuraliaLive: new Proxy({}, { get: (_, method) => (arg) => calls.push({ method, arg }) }) };
require('node:vm').runInThisContext(input.script);
return calls;
"#,
            json!({ "script": script }),
        );
        let calls = out.as_array().expect("chamadas");
        assert_eq!(calls.len(), 1, "o script faz uma chamada: {out}");
        (
            calls[0]["method"].as_str().expect("metodo").to_string(),
            calls[0]["arg"].clone(),
        )
    }

    // ------------------------------------------------------------ Rust

    #[test]
    fn the_key_is_stored_with_dpapi_and_a_bad_file_means_no_key() {
        let dir = temp_dir("dpapi");
        let store = LiveKeyStore::in_dir(&dir);
        let path = dir.join(LIVE_KEY_FILE);
        assert!(store.load().is_none(), "sem ficheiro nao ha chave");

        store.save(&key(TEST_KEY)).expect("guardar a chave");
        let bytes = std::fs::read(&path).expect("o ficheiro existe");
        assert!(bytes.starts_with(LIVE_KEY_MAGIC));
        // Nem inteira, nem um pedaco, nem em UTF-16: nada em claro no disco.
        assert!(!contains(&bytes, TEST_KEY.as_bytes()), "chave em claro");
        assert!(
            !contains(&bytes, &TEST_KEY.as_bytes()[6..22]),
            "pedaco em claro"
        );
        let utf16: Vec<u8> = TEST_KEY.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(!contains(&bytes, &utf16), "chave em UTF-16");
        assert_eq!(store.load().as_ref().map(LiveKey::expose), Some(TEST_KEY));
        assert!(
            !dir.join("gemini-live.key.tmp").exists(),
            "o temporario da escrita atomica nao fica para tras"
        );

        // Guardar outra substitui a primeira.
        store.save(&key(OTHER_KEY)).expect("trocar a chave");
        assert_eq!(store.load().as_ref().map(LiveKey::expose), Some(OTHER_KEY));

        // Estragado de qualquer maneira: "sem chave", nunca um panic.
        let good = std::fs::read(&path).expect("ficheiro");
        let mut flipped = good.clone();
        let middle = LIVE_KEY_MAGIC.len() + (good.len() - LIVE_KEY_MAGIC.len()) / 2;
        flipped[middle] ^= 0x5a;
        let corrupt: [Vec<u8>; 6] = [
            flipped,
            good[..good.len() / 2].to_vec(),
            LIVE_KEY_MAGIC.to_vec(),
            good[LIVE_KEY_MAGIC.len()..].to_vec(),
            b"lixo que nao e um blob da DPAPI".to_vec(),
            Vec::new(),
        ];
        for (index, bad) in corrupt.iter().enumerate() {
            std::fs::write(&path, bad).expect("escrever o estragado");
            assert!(
                store.load().is_none(),
                "ficheiro estragado {index} deu chave"
            );
        }

        // Um blob que abre mas nao tem forma de chave tambem vale "sem chave".
        let mut not_a_key = LIVE_KEY_MAGIC.to_vec();
        not_a_key.extend(dpapi_protect(b"tem espacos e <html>", LIVE_KEY_ENTROPY).expect("cifrar"));
        std::fs::write(&path, &not_a_key).expect("escrever");
        assert!(store.load().is_none());

        // Trocar chave: esquece, e esquecer duas vezes nao e erro.
        store.save(&key(TEST_KEY)).expect("guardar");
        store.forget().expect("esquecer");
        assert!(!path.exists(), "o ficheiro foi apagado");
        assert!(store.load().is_none());
        store.forget().expect("esquecer sem ficheiro");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_dpapi_entropy_is_part_of_the_lock() {
        // O mesmo utilizador, sem a entropia do NeuralIA: nao abre.
        use windows_sys::Win32::Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        };
        let protected = dpapi_protect(TEST_KEY.as_bytes(), LIVE_KEY_ENTROPY).expect("cifrar");
        let input = blob(&protected).expect("blob");
        let mut output = CRYPT_INTEGER_BLOB::default();
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok != 0 {
            let _ = unsafe { take_dpapi_output(&output) };
        }
        assert_eq!(ok, 0, "sem a entropia a DPAPI nao pode devolver a chave");
        assert_eq!(
            dpapi_unprotect(&protected, LIVE_KEY_ENTROPY).map(|plain| plain.bytes().to_vec()),
            Some(TEST_KEY.as_bytes().to_vec())
        );
    }

    #[test]
    fn live_panel_messages_are_a_closed_list_with_limits() {
        assert_eq!(
            parse_live_message(r#"{"action":"ready","args":{}}"#),
            Some(LiveMessage::Ready)
        );
        assert_eq!(
            parse_live_message(r#"{"action":"close"}"#),
            Some(LiveMessage::Close)
        );
        assert_eq!(
            parse_live_message(r#"{"action":"forget_key","args":{}}"#),
            Some(LiveMessage::ForgetKey)
        );
        assert_eq!(
            parse_live_message(r#"{"action":"stopped","args":{}}"#),
            Some(LiveMessage::Stopped)
        );
        let save = |key: &str| json!({ "action": "save_key", "args": { "key": key } }).to_string();
        assert_eq!(
            parse_live_message(&save(&format!("  {TEST_KEY}\n"))),
            Some(LiveMessage::SaveKey(key(TEST_KEY))),
            "os espacos a volta de uma chave colada saem"
        );
        let longest = "k".repeat(LIVE_KEY_MAX_CHARS);
        assert_eq!(
            parse_live_message(&save(&longest)),
            Some(LiveMessage::SaveKey(key(&longest))),
            "a maior chave aceite cabe no envelope"
        );

        // Com forma errada: InvalidKey, para a pagina nao ficar pendurada.
        let too_long = "a".repeat(LIVE_KEY_MAX_CHARS + 1);
        for bad in [
            "curta",
            "AIza com espacos no meio 0123456789",
            "<script>alert(1)</script>0123456789",
            "\"aspas\"-0123456789012345678",
            "chave-com-acentos-çãõ-0123456789",
            too_long.as_str(),
        ] {
            assert_eq!(
                parse_live_message(&save(bad)),
                Some(LiveMessage::InvalidKey),
                "{bad}"
            );
        }

        // Fora da lista, malformado ou com campos a mais: nada.
        for body in [
            r#"{"action":"open","args":{"url":"https://exemplo.com"}}"#,
            r#"{"action":"eval","args":{"code":"x"}}"#,
            r#"{"action":"READY"}"#,
            r#"{"action":"save_key"}"#,
            r#"{"action":"save_key","args":{"key":5}}"#,
            r#"{"action":"ready","cap":"x"}"#,
            r#"{"args":{}}"#,
            r#"["ready"]"#,
            r#""ready""#,
            "ready",
            "{",
            "",
        ] {
            assert_eq!(parse_live_message(body), None, "{body}");
        }
        let padded =
            json!({ "action": "ready", "args": { "pad": "x".repeat(LIVE_MESSAGE_MAX_BYTES) } })
                .to_string();
        assert_eq!(parse_live_message(&padded), None, "acima do tecto");

        // O Debug de uma mensagem com a chave nao a mostra.
        let debug = format!("{:?}", parse_live_message(&save(TEST_KEY)));
        assert!(!debug.contains(TEST_KEY), "{debug}");
        assert!(debug.contains("omitida"), "{debug}");
    }

    #[test]
    fn the_live_panel_only_navigates_to_its_own_page() {
        assert_eq!(live_page_url(), "http://neuralia-live.localhost/live.html");
        for target in [
            live_page_url().as_str(),
            "http://neuralia-live.localhost/live.html#fim",
            "http://NEURALIA-LIVE.localhost/live.html",
        ] {
            assert!(live_panel_allows_navigation(target), "{target}");
            assert!(live_panel_navigation(target.to_string()), "{target}");
            assert!(live_ipc_source_ok(target), "{target}");
            assert_eq!(
                live_ipc_message(target, r#"{"action":"ready","args":{}}"#),
                Some(LiveMessage::Ready),
                "{target}"
            );
            assert_eq!(
                live_ipc_message(target, r#"{"action":"eval","args":{}}"#),
                None,
                "a pagina certa continua presa a lista fechada"
            );
        }
        for target in [
            "http://neuralia-live.localhost/",
            "http://neuralia-live.localhost/live.js",
            "http://neuralia-live.localhost:8080/live.html",
            "https://neuralia-live.localhost/live.html",
            "http://neuralia-live.localhost.exemplo.com/live.html",
            "http://neuralia-live.exemplo.com/live.html",
            "http://user:senha@neuralia-live.localhost/live.html",
            "neuralia-live://localhost/live.html",
            "http://neuralia-pdf.localhost/viewer.html",
            "https://aistudio.google.com/apikey",
            "https://generativelanguage.googleapis.com/",
            "about:blank",
            "data:text/html,<p>x</p>",
            "javascript:alert(1)",
            "file:///C:/Windows/win.ini",
            "",
        ] {
            assert!(!live_panel_allows_navigation(target), "{target}");
            assert!(!live_panel_navigation(target.to_string()), "{target}");
            assert!(!live_ipc_source_ok(target), "{target}");
            // Uma mensagem valida vinda de outro documento nao passa.
            for body in [
                r#"{"action":"ready","args":{}}"#,
                r#"{"action":"forget_key","args":{}}"#,
                r#"{"action":"close"}"#,
            ] {
                assert_eq!(live_ipc_message(target, body), None, "{target} {body}");
            }
        }
    }

    fn get(uri: &str) -> HttpResponse<Cow<'static, [u8]>> {
        serve_live_asset(
            &Request::builder()
                .uri(uri)
                .body(Vec::new())
                .expect("pedido"),
        )
    }

    fn header<'a>(response: &'a HttpResponse<Cow<'static, [u8]>>, name: &str) -> Option<&'a str> {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    fn directive<'a>(csp: &'a str, name: &str) -> Vec<&'a str> {
        csp.split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix(name).map(str::split_whitespace))
            .map(Iterator::collect)
            .unwrap_or_default()
    }

    #[test]
    fn the_live_origin_serves_only_its_four_files() {
        // O URI tal como o wry o entrega depois de desfazer o contorno do
        // WebView2 (webview2/mod.rs, `prepare_request`).
        for (path, content_type, body) in [
            ("/live.html", "text/html", LIVE_HTML),
            ("/live.css", "text/css", LIVE_CSS),
            ("/live-core.js", "text/javascript", LIVE_CORE_JS),
            ("/live.js", "text/javascript", LIVE_APP_JS),
        ] {
            let response = get(&format!("neuralia-live://localhost{path}"));
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert!(
                header(&response, "Content-Type")
                    .is_some_and(|value| value.starts_with(content_type)),
                "{path}"
            );
            assert_eq!(response.body().as_ref(), body.as_bytes(), "{path}");
            assert_eq!(header(&response, "X-Content-Type-Options"), Some("nosniff"));
            assert_eq!(header(&response, "Cache-Control"), Some("no-store"));
            assert_eq!(
                header(&response, "Content-Security-Policy").is_some(),
                path == "/live.html",
                "{path}"
            );
        }

        let page = get("neuralia-live://localhost/live.html?x=1");
        assert_eq!(page.status(), StatusCode::OK);
        let csp = header(&page, "Content-Security-Policy").expect("CSP");
        assert_eq!(directive(csp, "default-src"), ["'none'"]);
        assert_eq!(
            directive(csp, "connect-src"),
            ["wss://generativelanguage.googleapis.com"],
            "a unica rede e o WebSocket do Gemini"
        );
        assert_eq!(directive(csp, "script-src"), ["'self'", "blob:"]);
        assert!(
            header(&page, "Permissions-Policy")
                .is_some_and(|value| value.contains("camera=(self)"))
        );
        // Presenca proibida (AGENTS.md §4.3): nenhum script inline nem de
        // fora -- cada <script> do HTML aponta para esta origem.
        for tag in LIVE_HTML.split("<script").skip(1) {
            assert!(tag.trim_start().starts_with("src=\"/"), "<script{tag}");
        }

        for uri in [
            "neuralia-live://localhost/",
            "neuralia-live://localhost/live.html/",
            "neuralia-live://localhost/LIVE.HTML",
            "neuralia-live://localhost/../live.html",
            "neuralia-live://localhost/live-core.js.map",
            "neuralia-live://localhost/viewer.html",
            "neuralia-live://localhost/document.pdf",
            "neuralia-live://localhost/gemini-live.key",
            "neuralia-live://exemplo.com/live.html",
            "neuralia-live://localhost:81/live.html",
            "neuralia-pdf://localhost/live.html",
            "http://neuralia-live.localhost/live.html",
        ] {
            let response = get(uri);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(response.body().as_ref(), b"not found", "{uri}");
        }
        let post = serve_live_asset(
            &Request::builder()
                .method(Method::POST)
                .uri("neuralia-live://localhost/live.html")
                .body(b"x".to_vec())
                .expect("pedido"),
        );
        assert_eq!(post.status(), StatusCode::NOT_FOUND, "so GET");
    }

    #[test]
    fn the_panel_asks_for_the_key_once_then_starts_with_it() {
        let dir = temp_dir("step");
        let store = LiveKeyStore::in_dir(&dir);
        let theme = json!({ "--bg": "#101112" });
        // O passo diz o que o script faz: `Start` arranca (o olho fica
        // vermelho), `AskKey` pede a chave (nada e capturado).
        let run = |message: LiveMessage| match live_step(message, &store, &theme) {
            LiveStep::Start(script) => {
                let (method, arg) = page_call(&script);
                assert_eq!(method, "start", "Start corre o start() da pagina");
                (method, arg)
            }
            LiveStep::AskKey(script) => {
                let (method, arg) = page_call(&script);
                assert_eq!(method, "askKey", "AskKey corre o askKey() da pagina");
                (method, arg)
            }
            LiveStep::Stopped => ("<parou>".to_string(), Value::Null),
            LiveStep::Close => ("<fechar>".to_string(), Value::Null),
        };

        // Primeira vez: pede a chave.
        let (method, arg) = run(LiveMessage::Ready);
        assert_eq!(method, "askKey");
        assert_eq!(arg["theme"], theme);
        assert!(arg["error"].is_null());
        assert!(arg.get("key").is_none(), "o ecra da chave nunca leva uma");

        // Guardar arranca a sessao com a chave, e fica guardada.
        let (method, arg) = run(LiveMessage::SaveKey(key(TEST_KEY)));
        assert_eq!(method, "start");
        assert_eq!(arg["key"], TEST_KEY);
        assert_eq!(arg["theme"], theme);
        assert!(arg["notice"].is_null());
        assert!(dir.join(LIVE_KEY_FILE).exists());

        // Da proxima vez arranca logo, sem pedir.
        let (method, arg) = run(LiveMessage::Ready);
        assert_eq!(method, "start");
        assert_eq!(arg["key"], TEST_KEY);

        // Chave com forma errada: volta ao ecra da chave, com o aviso.
        let (method, arg) = run(LiveMessage::InvalidKey);
        assert_eq!(method, "askKey");
        assert!(arg["error"].as_str().is_some_and(|text| !text.is_empty()));

        // Trocar chave: esquece e volta a pedir.
        let (method, _) = run(LiveMessage::ForgetKey);
        assert_eq!(method, "askKey");
        assert!(!dir.join(LIVE_KEY_FILE).exists());
        assert_eq!(run(LiveMessage::Ready).0, "askKey");

        assert!(matches!(
            live_step(LiveMessage::Close, &store, &theme),
            LiveStep::Close
        ));
        // A pagina diz que a sessao caiu: nada a correr, e a chave fica.
        assert!(matches!(
            live_step(LiveMessage::Stopped, &store, &theme),
            LiveStep::Stopped
        ));

        // Sem onde guardar (a "pasta" e um ficheiro): a sessao arranca na
        // mesma, e o aviso diz que a chave vale so desta vez.
        let blocked = dir.join("bloqueado");
        std::fs::write(&blocked, b"ficheiro").expect("ficheiro");
        let nowhere = LiveKeyStore::in_dir(&blocked);
        let LiveStep::Start(script) =
            live_step(LiveMessage::SaveKey(key(OTHER_KEY)), &nowhere, &theme)
        else {
            panic!("esperava arrancar");
        };
        let (method, arg) = page_call(&script);
        assert_eq!(method, "start");
        assert_eq!(arg["key"], OTHER_KEY);
        assert!(
            arg["notice"]
                .as_str()
                .is_some_and(|text| text.contains("só para esta sessão"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redaction_hides_the_key_in_every_shape_it_could_take() {
        let script = live_start_script(&key(TEST_KEY), &json!({}), None);
        let url = format!("wss://generativelanguage.googleapis.com/ws/x?key={TEST_KEY}&alt=json");
        let loose = format!("chave solta {TEST_KEY} no meio");
        let upper = format!("KEY={TEST_KEY}");
        let json_field = format!("{{\"key\":\"{TEST_KEY}\"}}");
        for line in [&script, &url, &loose, &upper, &json_field] {
            let clean = redact_debug_secrets(line);
            assert!(!clean.contains(TEST_KEY), "{clean}");
            assert!(
                !clean.contains(&TEST_KEY[4..20]),
                "pedaco da chave: {clean}"
            );
            assert!(clean.contains("[chave omitida]"), "{clean}");
        }
        assert_eq!(
            redact_debug_secrets(&url),
            "wss://generativelanguage.googleapis.com/ws/x?key=[chave omitida]&alt=json"
        );
        // O resto do log fica igual, e com acentos no meio.
        for plain in [
            "live panel: ligado",
            "resized 1440x900 surface=Comparator",
            "monkey=5 ção AIza curto",
        ] {
            assert!(matches!(redact_debug_secrets(plain), Cow::Borrowed(text) if text == plain));
        }
    }

    // ------------------------------------------------------ JS que embarca

    #[test]
    fn live_js_pcm_and_setup_are_what_the_api_expects() {
        let out = node_core(
            r#"
const d = core.createDownsampler(48000, 16000);
const tone = new Float32Array(4800);
for (let i = 0; i < tone.length; i++) tone[i] = 0.5 * Math.sin(2 * Math.PI * 440 * i / 48000);
let total = 0, at = 0;
for (const n of [128, 2048, 1000, 1624]) { total += d(tone.subarray(at, at + n)).length; at += n; }
const odd = core.createDownsampler(44100, 16000);
let oddTotal = 0;
for (let k = 0; k < 30; k++) oddTotal += odd(new Float32Array(1470)).length;
return {
  total, oddTotal,
  clip: Array.from(core.createDownsampler(16000, 16000)(Float32Array.from([2, -2, 1, -1, 0, 0.5, NaN]))),
  average: Array.from(core.createDownsampler(48000, 16000)(Float32Array.from([0.3, 0.3, 0.3, -0.6, -0.6, -0.6]))),
  base64: core.pcm16ToBase64(Int16Array.from([1, -2, 32767, -32768])),
  roundTrip: Array.from(core.base64ToPcm16(core.pcm16ToBase64(Int16Array.from([1, -2, 32767, -32768, 0])))),
  big: core.base64ToPcm16(core.pcm16ToBase64(new Int16Array(100000).fill(-3))).every((v) => v === -3),
  floats: Array.from(core.pcm16ToFloat32(Int16Array.from([-32768, 0, 16384]))),
  audio: core.audioMessage(Int16Array.from([1, -2])),
  video: core.videoMessage('SlBFRw=='),
  setup: core.setupMessage(),
  resumed: core.setupMessage('h-1'),
  keyMax: core.KEY_MAX_CHARS,
  invalidKey: core.INVALID_KEY_NOTICE,
  streamEnd: core.audioStreamEndMessage(),
  url: core.socketUrl('AIza a&b'),
  endpoint: core.ENDPOINT,
  frames: [core.frameSize(1920, 1080), core.frameSize(800, 600), core.frameSize(1080, 1920), core.frameSize(0, 10)],
  pip: core.pictureInPicture({ width: 1024, height: 576 }, { width: 640, height: 480 })
};
"#,
            Value::Null,
        );
        // 4800 amostras a 48 kHz, em blocos irregulares: 1600 a 16 kHz, nem
        // mais nem menos. A 44,1 kHz a razao nao e inteira e o resto passa
        // de bloco para bloco.
        assert_eq!(out["total"], 1600);
        assert_eq!(out["oddTotal"], 16000);
        assert_eq!(
            out["clip"],
            json!([32767, -32768, 32767, -32768, 0, 16384, 0])
        );
        assert_eq!(out["average"], json!([9830, -19661]));
        // 01 00 | FE FF | FF 7F | 00 80: little-endian.
        assert_eq!(out["base64"], "AQD+//9/AIA=");
        assert_eq!(out["roundTrip"], json!([1, -2, 32767, -32768, 0]));
        assert_eq!(out["big"], true);
        assert_eq!(out["floats"], json!([-1, 0, 0.5]));
        assert_eq!(
            out["audio"],
            json!({ "realtimeInput": { "audio": { "data": "AQD+/w==", "mimeType": "audio/pcm;rate=16000" } } })
        );
        assert_eq!(
            out["video"],
            json!({ "realtimeInput": { "video": { "data": "SlBFRw==", "mimeType": "image/jpeg" } } })
        );

        // O modelo e exatamente este (regra do dono), audio nativo, com as
        // duas transcricoes.
        let setup = &out["setup"]["setup"];
        assert_eq!(
            setup["model"],
            "models/gemini-2.5-flash-native-audio-preview-12-2025"
        );
        assert_eq!(
            setup["generationConfig"]["responseModalities"],
            json!(["AUDIO"])
        );
        assert_eq!(setup["inputAudioTranscription"], json!({}));
        assert_eq!(setup["outputAudioTranscription"], json!({}));
        let instruction = setup["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .expect("instrucao");
        assert!(instruction.contains("português do Brasil") && instruction.contains("tela"));
        // Sem compressao do contexto o Google corta uma sessao com video aos
        // ~2 min; sem sessionResumption a ligacao (~10 min) acaba sem volta.
        assert_eq!(
            setup["contextWindowCompression"],
            json!({ "slidingWindow": {} })
        );
        assert_eq!(setup["sessionResumption"], json!({}));
        assert_eq!(
            out["resumed"]["setup"]["sessionResumption"],
            json!({ "handle": "h-1" }),
            "retomar leva o handle"
        );
        assert_eq!(
            out["resumed"]["setup"]["model"], setup["model"],
            "o modelo nao muda ao retomar"
        );
        assert_eq!(
            out["streamEnd"],
            json!({ "realtimeInput": { "audioStreamEnd": true } })
        );
        // A pagina recusa sozinha o que o nativo recusaria, com o mesmo texto.
        assert_eq!(out["keyMax"], LIVE_KEY_MAX_CHARS);
        assert_eq!(out["invalidKey"], LIVE_INVALID_KEY_NOTICE);

        assert_eq!(
            out["endpoint"],
            "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"
        );
        assert_eq!(
            out["url"],
            format!("{}?key=AIza%20a%26b", out["endpoint"].as_str().unwrap())
        );
        assert_eq!(
            out["frames"],
            json!([
                { "width": 1024, "height": 576 },
                { "width": 800, "height": 600 },
                { "width": 576, "height": 1024 },
                null
            ])
        );
        assert_eq!(
            out["pip"],
            json!({ "x": 752, "y": 368, "width": 256, "height": 192 })
        );
    }

    #[test]
    fn live_js_reads_blob_frames_transcripts_and_interruptions() {
        let out = node_core(
            r#"
const audio = core.pcm16ToBase64(Int16Array.from([100, -100]));
const turn = { serverContent: {
  modelTurn: { parts: [
    { text: 'pensando' },
    { inlineData: { mimeType: 'audio/pcm;rate=24000', data: audio } },
    { inlineData: { mimeType: 'image/png', data: 'x' } }
  ] },
  inputTranscription: { text: 'Oi' },
  outputTranscription: { text: 'Olá' },
  turnComplete: true
} };
const fromBlob = core.parseServerMessage(await core.frameText(new Blob([JSON.stringify(turn)])));
const fromBuffer = core.parseServerMessage(await core.frameText(new TextEncoder().encode('{"setupComplete":{}}').buffer));
const fromView = core.parseServerMessage(await core.frameText(new TextEncoder().encode('{"goAway":{"timeLeft":"5s"}}')));
const blob = new Blob(['{"setupComplete":{}}']);
return {
  handles: [
    core.parseServerMessage('{"sessionResumptionUpdate":{"newHandle":"h1","resumable":true}}').resumeHandle,
    core.parseServerMessage('{"sessionResumptionUpdate":{"newHandle":"h2","resumable":false}}').resumeHandle,
    core.parseServerMessage('{"sessionResumptionUpdate":{"resumable":true}}').resumeHandle,
    core.parseServerMessage('{"sessionResumptionUpdate":{"newHandle":7,"resumable":true}}').resumeHandle
  ],
  fromBlob,
  setupComplete: fromBuffer.setupComplete,
  goAway: fromView.goAway,
  interrupted: core.parseServerMessage('{"serverContent":{"interrupted":true}}').interrupted,
  notInterrupted: core.parseServerMessage('{"serverContent":{"turnComplete":true}}').interrupted,
  error: core.parseServerMessage('{"error":{"message":"quota"}}').error,
  defaultRate: core.parseServerMessage(JSON.stringify({ serverContent: { modelTurn: { parts: [{ inlineData: { mimeType: 'audio/pcm', data: audio } }] } } })).audio[0].rate,
  invalid: [core.parseServerMessage('nao e json'), core.parseServerMessage('42'), core.parseServerMessage('null'), core.parseServerMessage(String(blob))],
  closes: [
    core.describeClose(1007, 'API key not valid. Please pass a valid API key.'),
    core.describeClose(1008, "Method doesn't allow unregistered callers"),
    core.describeClose(1006, ''),
    core.describeClose(1011, 'Internal error'),
    core.describeClose(1000, '')
  ],
  quota: core.describeClose(1011, 'You exceeded your current quota, please check your plan and billing details.'),
  model: core.describeClose(1008, 'models/x is not found for API version v1beta, or is not supported for bidiGenerateContent'),
  exhausted: core.describeClose(1011, 'RESOURCE_EXHAUSTED'),
  internal: core.describeClose(1011, 'Internal error.')
};
"#,
            Value::Null,
        );
        assert_eq!(
            out["handles"],
            json!(["h1", "", "", ""]),
            "so um handle retomavel conta"
        );
        assert_eq!(
            out["fromBlob"],
            json!({
                "setupComplete": false, "goAway": false, "resumeHandle": "", "error": "",
                "audio": [{ "data": "ZACc/w==", "rate": 24000 }],
                "inputText": "Oi", "outputText": "Olá",
                "interrupted": false, "turnComplete": true
            })
        );
        assert_eq!(out["setupComplete"], true);
        assert_eq!(out["goAway"], true);
        assert_eq!(out["interrupted"], true);
        assert_eq!(out["notInterrupted"], false);
        assert_eq!(out["error"], "quota");
        assert_eq!(out["defaultRate"], 24000);
        assert_eq!(out["invalid"], json!([null, null, null, null]));
        let closes = out["closes"].as_array().expect("fechos");
        let key_problem: Vec<bool> = closes
            .iter()
            .map(|close| close["keyProblem"].as_bool().expect("bool"))
            .collect();
        assert_eq!(key_problem, [true, true, false, false, false]);
        for close in closes {
            assert!(
                close["message"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
            );
        }
        assert!(closes[3]["message"].as_str().unwrap().contains("1011"));
        // Em portugues do Brasil, e o motivo cru do servidor (ingles) so
        // como detalhe, sem ponto dobrado.
        assert_eq!(
            closes[2]["message"],
            "A conexão caiu (rede ou servidor indisponível)."
        );
        assert_eq!(closes[3]["detail"], "Internal error");
        assert_eq!(out["internal"]["detail"], "Internal error");
        for close in [&out["quota"], &out["exhausted"]] {
            assert_eq!(close["keyProblem"], false);
            let message = close["message"].as_str().expect("mensagem");
            assert!(message.contains("cota"), "{message}");
            assert!(!message.contains("quota"), "{message}");
        }
        assert_eq!(
            out["quota"]["detail"],
            "You exceeded your current quota, please check your plan and billing details"
        );
        assert_eq!(
            out["model"]["message"],
            "O modelo do Gemini Live não está disponível para esta chave."
        );
        for close in closes
            .iter()
            .chain([&out["quota"], &out["model"], &out["internal"]])
        {
            let message = close["message"].as_str().expect("mensagem");
            assert!(!message.contains(".."), "{message}");
            for european in ["ligação", "partilh", "ecrã", "A ligar"] {
                assert!(!message.contains(european), "{message}");
            }
        }
    }

    /// Dublês do navegador para a sessao: socket, contexto de audio, faixas
    /// de media, worklet, frames e temporizador, todos a registar o que lhes
    /// fazem.
    const SESSION_FAKES: &str = r#"
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
function world(options) {
  const w = { sockets: [], contexts: [], tracks: [], requests: [], ui: { errors: [], notices: [], transcripts: [], camera: [], screen: [], sources: [], statuses: [], ended: 0 }, micChunk: null, micDisconnects: 0, tick: null, cleared: [], pending: [] };
  class Socket {
    constructor(url) { this.url = url; this.readyState = 0; this.sent = []; w.sockets.push(this); }
    send(text) { this.sent.push(JSON.parse(text)); }
    close(code) { this.readyState = 3; this.closedWith = code; }
    open() { this.readyState = 1; this.onopen && this.onopen({}); }
    receive(data) { this.onmessage && this.onmessage({ data }); }
    drop(code, reason) { this.readyState = 3; this.onclose && this.onclose({ code, reason }); }
  }
  class Context {
    constructor() { this.sampleRate = 48000; this.currentTime = 10; this.state = 'running'; this.destination = {}; this.sources = []; this.closed = 0; w.contexts.push(this); }
    createBuffer(channels, length, rate) { const data = new Float32Array(length); return { length, sampleRate: rate, getChannelData: () => data }; }
    createBufferSource() { const s = { starts: [], stops: 0, connect() {}, start(at) { this.starts.push(at); }, stop() { this.stops++; } }; this.sources.push(s); return s; }
    close() { this.closed++; this.state = 'closed'; return Promise.resolve(); }
  }
  function stream(kind) {
    const track = { kind, stopped: 0, stop() { this.stopped++; } };
    w.tracks.push(track);
    return { getTracks: () => [track], getVideoTracks: () => (kind === 'audio' ? [] : [track]) };
  }
  const media = (kind) => {
    w.requests.push(kind);
    if (options && options.deferred) return new Promise((resolve) => w.pending.push(() => resolve(stream(kind))));
    return Promise.resolve(stream(kind));
  };
  w.env = {
    WebSocket: Socket,
    AudioContext: Context,
    getUserMedia: (c) => media(c.audio ? 'audio' : 'video'),
    getDisplayMedia: () => media('screen'),
    createMic: async (ctx, s, onChunk) => { w.micChunk = onChunk; return { rate: ctx.sampleRate, disconnect() { w.micDisconnects++; } }; },
    grabFrame: async () => 'SlBFRw==',
    setInterval: (fn, ms) => { w.tick = fn; w.intervalMs = ms; return 7; },
    clearInterval: (id) => w.cleared.push(id)
  };
  w.uiHandlers = {
    error: (message, keyProblem, detail) => w.ui.errors.push({ message, keyProblem, detail }),
    notice: (kind, text) => w.ui.notices.push([kind, text]),
    status: (text, tone) => w.ui.statuses.push(tone),
    ended: () => { w.ui.ended++; },
    transcript: (role, text) => w.ui.transcripts.push([role, text]),
    camera: (s) => w.ui.camera.push(!!s),
    screen: (s) => w.ui.screen.push(!!s),
    sources: (s) => w.ui.sources.push(s)
  };
  return w;
}
const realtime = (socket, kind) => socket.sent.filter((m) => m.realtimeInput && m.realtimeInput[kind]);
"#;

    #[test]
    fn live_js_session_streams_plays_and_stops_everything() {
        let body = format!(
            "{SESSION_FAKES}{}",
            r#"
const w = world();
const session = core.createSession({ env: w.env, ui: w.uiHandlers, key: input.key });
await session.start();
const socket = w.sockets[0];
const out = { url: socket.url, requests: w.requests.slice().sort(), intervalMs: w.intervalMs };
socket.open();
out.first = socket.sent[0];
// Antes do setupComplete nada de voz nem de frames sai.
w.micChunk(new Float32Array(4800).fill(0.25));
w.tick(); await flush();
out.beforeReady = socket.sent.length;
// O servidor responde em Blob, como no navegador.
socket.receive(new Blob([JSON.stringify({ setupComplete: {} })]));
await flush();
out.live = session.live;
w.micChunk(new Float32Array(2400).fill(0.25));
out.afterHalf = realtime(socket, 'audio').length;
w.micChunk(new Float32Array(2400).fill(0.25));
const audio = realtime(socket, 'audio');
out.audio = audio.map((m) => ({ mime: m.realtimeInput.audio.mimeType, samples: core.base64ToPcm16(m.realtimeInput.audio.data).length, first: core.base64ToPcm16(m.realtimeInput.audio.data)[0] }));
w.tick(); await flush();
out.video = realtime(socket, 'video').map((m) => m.realtimeInput.video);
// Duas falas de 100 ms a 24 kHz: a segunda comeca onde a primeira acaba.
const pcm = core.pcm16ToBase64(new Int16Array(2400).fill(1000));
const part = { inlineData: { mimeType: 'audio/pcm;rate=24000', data: pcm } };
socket.receive(new Blob([JSON.stringify({ serverContent: { modelTurn: { parts: [part, part] }, inputTranscription: { text: 'Oi' }, outputTranscription: { text: 'Olá' } } })]));
await flush();
const context = w.contexts[0];
out.starts = context.sources.map((s) => s.starts[0]);
// Interrompido: o que estava na fila cala-se.
socket.receive(JSON.stringify({ serverContent: { interrupted: true } }));
await flush();
out.stopsAfterInterrupt = context.sources.map((s) => s.stops);
out.transcripts = w.ui.transcripts;
// "Parar partilha" do Windows termina a faixa da tela por fora.
const screenTrack = w.tracks.find((t) => t.kind === 'screen');
screenTrack.onended();
await flush();
out.afterOsStop = { screen: session.sources().screen, stopped: screenTrack.stopped };
session.stop();
out.after = {
  stopped: w.tracks.map((t) => [t.kind, t.stopped]),
  socketClosed: socket.closedWith,
  contextClosed: context.closed,
  cleared: w.cleared,
  micDisconnects: w.micDisconnects,
  live: session.live,
  sources: session.sources(),
  cameraOff: w.ui.camera[w.ui.camera.length - 1]
};
// Depois de parar, o microfone ja nao manda nada.
const sent = socket.sent.length;
w.micChunk(new Float32Array(4800));
out.sentAfterStop = socket.sent.length - sent;
return out;
"#
        );
        let out = node_core(&body, json!({ "key": TEST_KEY }));
        assert_eq!(
            out["url"],
            format!(
                "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key={TEST_KEY}"
            )
        );
        assert_eq!(out["requests"], json!(["audio", "screen", "video"]));
        assert_eq!(out["intervalMs"], 1000, "um frame por segundo");
        assert_eq!(
            out["first"]["setup"]["model"], "models/gemini-2.5-flash-native-audio-preview-12-2025",
            "a primeira mensagem e o setup"
        );
        assert_eq!(out["beforeReady"], 1, "so o setup antes do setupComplete");
        assert_eq!(out["live"], true, "o setupComplete chegou num Blob");
        assert_eq!(out["afterHalf"], 0, "50 ms ainda nao fazem uma mensagem");
        assert_eq!(
            out["audio"],
            json!([{ "mime": "audio/pcm;rate=16000", "samples": 1600, "first": 8192 }])
        );
        assert_eq!(
            out["video"],
            json!([{ "data": "SlBFRw==", "mimeType": "image/jpeg" }])
        );
        let starts: Vec<f64> = out["starts"]
            .as_array()
            .expect("inicios")
            .iter()
            .map(|at| at.as_f64().expect("f64"))
            .collect();
        assert_eq!(starts.len(), 2);
        assert!(
            (starts[0] - 10.0).abs() < 1e-9 && (starts[1] - 10.1).abs() < 1e-9,
            "{starts:?}"
        );
        assert_eq!(out["stopsAfterInterrupt"], json!([1, 1]));
        assert_eq!(
            out["transcripts"],
            json!([["user", "Oi"], ["model", "Olá"]])
        );
        assert_eq!(out["afterOsStop"], json!({ "screen": false, "stopped": 1 }));
        assert_eq!(
            out["after"],
            json!({
                "stopped": [["audio", 1], ["video", 1], ["screen", 1]],
                "socketClosed": 1000,
                "contextClosed": 1,
                "cleared": [7],
                "micDisconnects": 1,
                "live": false,
                "sources": { "screen": false, "camera": false, "mic": false },
                "cameraOff": false
            })
        );
        assert_eq!(out["sentAfterStop"], 0);
    }

    #[test]
    fn live_js_turns_off_devices_granted_after_off_and_reports_a_bad_key() {
        let body = format!(
            "{SESSION_FAKES}{}",
            r#"
// 1. O utilizador desliga antes de responder ao aviso de permissao: as
//    faixas que chegam depois morrem logo.
const late = world({ deferred: true });
const s1 = core.createSession({ env: late.env, ui: late.uiHandlers, key: 'K' });
const starting = s1.start();
await flush();
s1.stop();
for (const grant of late.pending) grant();
await starting; await flush();
const out = { late: late.tracks.map((t) => [t.kind, t.stopped]), lateMic: late.micChunk === null, lateNotices: late.ui.notices.filter(([, text]) => text).length };

// 2. Camera desligada e ligada de novo antes da primeira resposta: fica so
//    uma faixa viva.
const twice = world({ deferred: true });
const s2 = core.createSession({ env: twice.env, ui: twice.uiHandlers, key: 'K' });
const s2start = s2.start();
await flush();
s2.setCamera(false);
const again = s2.setCamera(true);
await flush();
for (const grant of twice.pending) grant();
await s2start; await again; await flush();
out.twice = { alive: twice.tracks.filter((t) => t.kind === 'video' && !t.stopped).length, videos: twice.tracks.filter((t) => t.kind === 'video').length, camera: s2.sources().camera };
s2.stop();

// 3. O Google recusa a chave: erro visivel, com "Trocar chave", e tudo parado.
const bad = world();
const s3 = core.createSession({ env: bad.env, ui: bad.uiHandlers, key: 'K' });
await s3.start();
bad.sockets[0].open();
bad.sockets[0].receive('{"setupComplete":{}}');
await flush();
bad.sockets[0].drop(1007, 'API key not valid. Please pass a valid API key.');
out.bad = { errors: bad.ui.errors, stopped: bad.tracks.map((t) => t.stopped), contextClosed: bad.contexts[0].closed, live: s3.live, sockets: bad.sockets.length, ended: bad.ui.ended };
return out;
"#
        );
        let out = node_core(&body, Value::Null);
        assert_eq!(
            out["late"],
            json!([["audio", 1], ["video", 1], ["screen", 1]]),
            "faixa concedida depois de desligar ficou acesa"
        );
        assert_eq!(out["lateMic"], true, "o microfone nem chegou a ligar-se");
        assert_eq!(out["lateNotices"], 0);
        assert_eq!(
            out["twice"],
            json!({ "alive": 1, "videos": 2, "camera": true })
        );
        assert_eq!(out["bad"]["errors"].as_array().map(Vec::len), Some(1));
        assert_eq!(out["bad"]["errors"][0]["keyProblem"], true);
        assert_eq!(out["bad"]["stopped"], json!([1, 1, 1]));
        assert_eq!(out["bad"]["contextClosed"], 1);
        assert_eq!(out["bad"]["live"], false);
        assert_eq!(out["bad"]["sockets"], 1, "chave recusada nao se retoma");
        assert_eq!(out["bad"]["ended"], 1, "a pagina avisa o nativo que parou");
    }

    #[test]
    fn a_server_that_keeps_dropping_after_setup_cannot_reconnect_forever() {
        // O setupComplete zera o tecto de religacoes SEGUIDAS; sem um tecto
        // por sessao, um servidor que aceita e volta a fechar religava sem
        // fim, a gastar a cota do dono.
        let body = format!(
            "{SESSION_FAKES}{}",
            r#"
const w = world();
const session = core.createSession({ env: w.env, ui: w.uiHandlers, key: 'K' });
await session.start();
for (let round = 0; round < 12; round++) {
  const socket = w.sockets[w.sockets.length - 1];
  if (!socket || socket.closedWith) break;
  socket.open();
  socket.receive('{"setupComplete":{}}');
  socket.receive(JSON.stringify({ sessionResumptionUpdate: { newHandle: 'h' + round, resumable: true } }));
  await flush();
  socket.drop(1011, 'Internal error');
  await flush();
}
return {
  sockets: w.sockets.length,
  live: session.live,
  errors: w.ui.errors.map((e) => e.message),
  ended: w.ui.ended,
  stopped: w.tracks.map((t) => t.stopped),
  caps: [core.MAX_DROP_RESUMES, core.MAX_GOAWAY_RESUMES]
};
"#
        );
        let out = node_core(&body, Value::Null);
        assert_eq!(out["caps"], json!([6, 36]));
        assert_eq!(
            out["sockets"], 7,
            "a primeira ligacao + 6 religacoes, e para: {out}"
        );
        assert_eq!(out["live"], false);
        assert_eq!(out["ended"], 1);
        assert_eq!(out["stopped"], json!([1, 1, 1]), "tela, camera e mic param");
        assert_eq!(
            out["errors"],
            json!([
                "A conexão caiu várias vezes e a sessão foi encerrada para não gastar a sua cota. Ligue de novo quando quiser."
            ])
        );
    }

    #[test]
    fn live_js_keeps_the_session_across_go_away_and_drops() {
        let body = format!(
            "{SESSION_FAKES}{}",
            r#"
const ready = async (socket) => { socket.open(); socket.receive('{"setupComplete":{}}'); await flush(); };
const w = world();
const session = core.createSession({ env: w.env, ui: w.uiHandlers, key: 'K' });
await session.start();
const first = w.sockets[0];
await ready(first);
first.receive(JSON.stringify({ sessionResumptionUpdate: { newHandle: 'h1', resumable: true } }));
await flush();
// O servidor avisa que vai fechar: a sessao passa ja para uma ligacao nova,
// com o handle, e a tela, a camera e o microfone continuam ligados.
first.receive(JSON.stringify({ goAway: { timeLeft: '10s' } }));
await flush();
const out = { afterGoAway: { sockets: w.sockets.length, oldClosed: first.closedWith, live: session.live } };
const second = w.sockets[1];
second.open();
out.secondSetup = second.sent[0].setup.sessionResumption;
out.secondModel = second.sent[0].setup.model;
second.receive('{"setupComplete":{}}');
await flush();
out.resumed = { live: session.live, stopped: w.tracks.map((t) => t.stopped), sources: session.sources(), errors: w.ui.errors.length };
// A voz vai para a ligacao nova; o fecho tardio da velha nao faz nada.
w.micChunk(new Float32Array(4800).fill(0.25));
out.audioOnSecond = realtime(second, 'audio').length;
first.drop(1000, '');
await flush();
out.afterOldClose = { live: session.live, sockets: w.sockets.length, errors: w.ui.errors.length };
// Uma queda (1011) com handle: retoma de novo.
second.receive(JSON.stringify({ sessionResumptionUpdate: { newHandle: 'h2', resumable: true } }));
await flush();
second.drop(1011, 'Internal error');
await flush();
const third = w.sockets[2];
third.open();
out.thirdSetup = third.sent[0].setup.sessionResumption;
// Religacoes que nunca chegam ao setupComplete acabam por desistir.
third.drop(1006, '');
await flush();
w.sockets[3].drop(1006, '');
await flush();
out.stillTrying = { sockets: w.sockets.length, errors: w.ui.errors.length };
w.sockets[4].drop(1006, '');
await flush();
out.giveUp = { sockets: w.sockets.length, live: session.live, errors: w.ui.errors.map((e) => e.message), ended: w.ui.ended, stopped: w.tracks.map((t) => t.stopped) };

// Sem handle nao ha como retomar: para tudo e diz porque.
const n = world();
const plain = core.createSession({ env: n.env, ui: n.uiHandlers, key: 'K' });
await plain.start();
await ready(n.sockets[0]);
n.sockets[0].drop(1006, '');
await flush();
out.noHandle = { sockets: n.sockets.length, errors: n.ui.errors.map((e) => e.message), ended: n.ui.ended, stopped: n.tracks.map((t) => t.stopped) };
return out;
"#
        );
        let out = node_core(&body, Value::Null);
        assert_eq!(
            out["afterGoAway"],
            json!({ "sockets": 2, "oldClosed": 1000, "live": false })
        );
        assert_eq!(out["secondSetup"], json!({ "handle": "h1" }));
        assert_eq!(
            out["secondModel"],
            "models/gemini-2.5-flash-native-audio-preview-12-2025"
        );
        assert_eq!(
            out["resumed"],
            json!({
                "live": true,
                "stopped": [0, 0, 0],
                "sources": { "screen": true, "camera": true, "mic": true },
                "errors": 0
            }),
            "retomar nao larga nenhuma faixa"
        );
        assert_eq!(out["audioOnSecond"], 1);
        assert_eq!(
            out["afterOldClose"],
            json!({ "live": true, "sockets": 2, "errors": 0 })
        );
        assert_eq!(out["thirdSetup"], json!({ "handle": "h2" }));
        assert_eq!(
            out["stillTrying"],
            json!({ "sockets": 5, "errors": 0 }),
            "tres religacoes seguidas antes de desistir"
        );
        assert_eq!(
            out["giveUp"],
            json!({
                "sockets": 5,
                "live": false,
                "errors": ["A conexão caiu (rede ou servidor indisponível)."],
                "ended": 1,
                "stopped": [1, 1, 1]
            })
        );
        assert_eq!(
            out["noHandle"],
            json!({
                "sockets": 1,
                "errors": ["A conexão caiu (rede ou servidor indisponível)."],
                "ended": 1,
                "stopped": [1, 1, 1]
            })
        );
    }

    #[test]
    fn live_js_mic_off_flushes_and_ends_the_audio_stream() {
        let body = format!(
            "{SESSION_FAKES}{}",
            r#"
const w = world();
const session = core.createSession({ env: w.env, ui: w.uiHandlers, key: 'K' });
await session.start();
const socket = w.sockets[0];
socket.open();
socket.receive('{"setupComplete":{}}');
await flush();
// 2048 amostras a 48 kHz: 682 a 16 kHz, menos do que uma mensagem.
w.micChunk(new Float32Array(2048).fill(0.25));
const before = socket.sent.length;
await session.setMic(false);
const after = socket.sent.slice(before);
// Desligar outra vez (ou a camera) nao manda mais nada.
await session.setMic(false);
await session.setCamera(false);
return {
  after: after.map((m) => m.realtimeInput.audioStreamEnd === true ? 'end' : core.base64ToPcm16(m.realtimeInput.audio.data).length),
  extra: socket.sent.length - before - after.length,
  micTrack: w.tracks.find((t) => t.kind === 'audio').stopped
};
"#
        );
        let out = node_core(&body, Value::Null);
        assert_eq!(
            out["after"],
            json!([682, "end"]),
            "o resto da fila e depois o fim do audio"
        );
        assert_eq!(out["extra"], 0);
        assert_eq!(out["micTrack"], 1);
    }

    /// A pagina inteira (HTML, live-core.js e live.js que embarcam) num DOM
    /// de brinquedo. Os ids vem do proprio `LIVE_HTML`: se o live.js pedir
    /// um elemento que o HTML nao tem, isto rebenta. `input.options` afina o
    /// navegador (`screenFails`: quantas vezes a tela e recusada antes de
    /// abrir; `suspended`: o som comeca parado, como sem gesto). O cenario e
    /// `input.body`, corpo de uma funcao async que recebe `t` e devolve JSON.
    const PAGE_HARNESS: &str = r#"
const vm = require('node:vm');
const input = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
const options = input.options || {};
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const posts = [], sockets = [], contexts = [], tracks = [], requests = [], themeVars = {}, docListeners = {};
let screenFails = options.screenFails || 0;
function el(id, tag, hidden) {
  const e = {
    id, tagName: String(tag || 'div').toUpperCase(), hidden: !!hidden, value: '', disabled: false,
    textContent: '', className: '', attrs: {}, children: [], listeners: {}, dataset: {},
    srcObject: null, videoWidth: 0, videoHeight: 0, scrollTop: 0, scrollHeight: 0,
    style: { setProperty() {} },
    addEventListener(type, fn) { (e.listeners[type] = e.listeners[type] || []).push(fn); },
    dispatch(type, extra) { for (const fn of e.listeners[type] || []) fn(Object.assign({ preventDefault() {}, key: '' }, extra || {})); },
    click() { e.dispatch('click'); },
    setAttribute(k, v) { e.attrs[k] = String(v); },
    getAttribute(k) { return k in e.attrs ? e.attrs[k] : null; },
    focus() {}, play() { return Promise.resolve(); },
    appendChild(c) { e.children.push(c); return c; },
    removeChild(c) { e.children.splice(e.children.indexOf(c), 1); },
    get childElementCount() { return e.children.length; },
    get firstElementChild() { return e.children[0]; },
    get lastChild() { return e.children[e.children.length - 1]; },
    getContext() { return { drawImage() {} }; },
    toDataURL() { return 'data:image/jpeg;base64,SlBFRw=='; }
  };
  return e;
}
const elements = {};
for (const match of input.html.matchAll(/<(\w+)([^>]*?)\sid="([^"]+)"([^>]*)>/g)) {
  const attrs = match[2] + ' ' + match[4] + ' ';
  const node = el(match[3], match[1], /\shidden\s/.test(attrs));
  const pressed = /aria-pressed="(\w+)"/.exec(attrs);
  if (pressed) node.attrs['aria-pressed'] = pressed[1];
  elements[match[3]] = node;
}
class Socket {
  constructor(url) { this.url = url; this.readyState = 0; this.sent = []; sockets.push(this); }
  send(t) { this.sent.push(t); }
  close(code) { this.readyState = 3; this.closedWith = code; }
  open() { this.readyState = 1; this.onopen && this.onopen({}); }
  receive(data) { this.onmessage && this.onmessage({ data }); }
  drop(code, reason) { this.readyState = 3; this.onclose && this.onclose({ code, reason }); }
}
class Context {
  constructor() { this.sampleRate = 48000; this.currentTime = 0; this.state = options.suspended ? 'suspended' : 'running'; this.destination = {}; this.closed = 0; this.audioWorklet = { addModule: async () => {} }; contexts.push(this); }
  createMediaStreamSource() { return { connect() {}, disconnect() {} }; }
  createGain() { return { gain: { value: 1 }, connect() {}, disconnect() {} }; }
  resume() { this.state = 'running'; return Promise.resolve(); }
  close() { this.closed++; this.state = 'closed'; return Promise.resolve(); }
}
class WorkletNode { constructor() { this.port = { onmessage: null }; } connect() {} disconnect() {} }
function stream(kind) { const t = { kind, stopped: 0, stop() { this.stopped++; } }; tracks.push(t); return { getTracks: () => [t], getVideoTracks: () => (kind === 'audio' ? [] : [t]) }; }
const sandbox = {
  document: {
    getElementById: (id) => elements[id] || null,
    createElement: (tag) => el('', tag, false),
    createTextNode: (text) => ({ textContent: text }),
    documentElement: { style: { setProperty(k, v) { themeVars[k] = v; } } },
    body: el('body', 'body', false),
    addEventListener(type, fn) { (docListeners[type] = docListeners[type] || []).push(fn); }
  },
  navigator: { mediaDevices: {
    getUserMedia: async (c) => { requests.push(c.audio ? 'mic' : 'camera'); return stream(c.audio ? 'audio' : 'video'); },
    getDisplayMedia: async () => {
      requests.push('screen');
      if (screenFails > 0) { screenFails--; const error = new Error('sem gesto'); error.name = 'InvalidStateError'; throw error; }
      return stream('screen');
    }
  } },
  WebSocket: Socket, AudioContext: Context, AudioWorkletNode: WorkletNode,
  Blob, TextDecoder, TextEncoder, btoa, atob, setTimeout, clearTimeout,
  URL: { createObjectURL: () => 'blob:worklet', revokeObjectURL() {} },
  setInterval: () => 1, clearInterval() {},
  ipc: { postMessage: (message) => posts.push(String(message)) },
  addEventListener() {}
};
sandbox.window = sandbox;
sandbox.top = sandbox;
vm.createContext(sandbox);
vm.runInContext(input.core, sandbox, { filename: 'live-core.js' });
vm.runInContext(input.app, sandbox, { filename: 'live.js' });
const t = { vm, sandbox, input, flush, elements, posts, sockets, contexts, tracks, requests, themeVars, docListeners };
t.shown = () => (elements.notices.hidden ? [] : elements.notices.children.map((c) => [c.dataset.kind, c.textContent]));
const AsyncFunction = (async () => {}).constructor;
new AsyncFunction('t', input.body)(t)
  .then((out) => process.stdout.write(JSON.stringify(out === undefined ? null : out)))
  .catch((error) => { process.stderr.write(String((error && error.stack) || error)); process.exit(1); });
"#;

    fn run_page(body: &str, options: Value, scripts: Value) -> Value {
        let mut input = json!({
            "html": LIVE_HTML,
            "core": LIVE_CORE_JS,
            "app": LIVE_APP_JS,
            "key": TEST_KEY,
            "body": body,
            "options": options,
        });
        for (name, script) in scripts.as_object().expect("scripts") {
            input[name] = script.clone();
        }
        run_node(PAGE_HARNESS, input)
    }

    /// Tudo o que a pagina publicou passa pelo parser do nativo.
    fn parsed_posts(posts: &Value) -> Vec<LiveMessage> {
        posts
            .as_array()
            .expect("mensagens")
            .iter()
            .map(|post| {
                let text = post.as_str().expect("texto");
                parse_live_message(text).unwrap_or_else(|| panic!("o nativo recusou {text}"))
            })
            .collect()
    }

    #[test]
    fn the_shipped_page_speaks_only_the_closed_channel_and_off_stops_everything() {
        let theme = json!({ "--bg": "#202124" });
        let out = run_page(
            r#"
const { vm, sandbox, input, flush, elements, posts, sockets, contexts, tracks, requests, themeVars } = t;
const out = { afterLoad: posts.slice() };
vm.runInContext(input.askScript, sandbox);
out.askKey = { keyHidden: elements['key-screen'].hidden, liveHidden: elements['live-screen'].hidden, errorHidden: elements['key-error'].hidden, bg: themeVars['--bg'], sockets: sockets.length, requests: requests.length };
elements.key.value = '  ' + input.key + '  ';
elements.save.click();
out.afterSave = { field: elements.key.value, disabled: elements.save.disabled };
vm.runInContext(input.startScript, sandbox);
for (let i = 0; i < 5; i++) await flush();
out.started = {
  keyHidden: elements['key-screen'].hidden, liveHidden: elements['live-screen'].hidden,
  socketUrl: sockets[0] && sockets[0].url, requests: requests.slice().sort(),
  pressed: ['t-screen', 't-camera', 't-mic'].map((id) => elements[id].getAttribute('aria-pressed')),
  camHidden: elements.cam.hidden
};
elements['t-camera'].click();
for (let i = 0; i < 3; i++) await flush();
out.cameraOff = { pressed: elements['t-camera'].getAttribute('aria-pressed'), stopped: tracks.filter((t) => t.kind === 'video').map((t) => t.stopped), camHidden: elements.cam.hidden };
elements.off.click();
await flush();
out.off = { stopped: tracks.map((t) => [t.kind, t.stopped]), socketClosed: sockets[0].closedWith, contextsClosed: contexts.map((c) => c.closed) };
elements['change-key'].click();
out.posts = posts;
return out;
"#,
            json!({}),
            json!({
                "askScript": live_ask_key_script(&theme, None),
                "startScript": live_start_script(&key(TEST_KEY), &theme, None),
            }),
        );
        assert_eq!(out["afterLoad"], json!([r#"{"action":"ready","args":{}}"#]));
        assert_eq!(
            out["askKey"],
            json!({ "keyHidden": false, "liveHidden": true, "errorHidden": true, "bg": "#202124", "sockets": 0, "requests": 0 }),
            "o ecra da chave nao abre rede nem pede tela, camera ou microfone"
        );
        assert_eq!(
            out["afterSave"],
            json!({ "field": "", "disabled": true }),
            "a chave sai do campo logo que e enviada"
        );
        assert_eq!(
            out["started"],
            json!({
                "keyHidden": true,
                "liveHidden": false,
                "socketUrl": format!("wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key={TEST_KEY}"),
                "requests": ["camera", "mic", "screen"],
                "pressed": ["true", "true", "true"],
                "camHidden": false
            })
        );
        assert_eq!(
            out["cameraOff"],
            json!({ "pressed": "false", "stopped": [1], "camHidden": true })
        );
        assert_eq!(
            out["off"],
            json!({
                "stopped": [["audio", 1], ["video", 1], ["screen", 1]],
                "socketClosed": 1000,
                "contextsClosed": [1]
            }),
            "Desligar parou tudo"
        );
        // A sequencia que o utilizador fez, aceite pelo nativo.
        assert_eq!(
            parsed_posts(&out["posts"]),
            [
                LiveMessage::Ready,
                LiveMessage::SaveKey(key(TEST_KEY)),
                LiveMessage::Close,
                LiveMessage::ForgetKey,
            ]
        );
    }

    /// O que o dono ve quando as coisas nao correm bem: uma colagem grande
    /// demais tem resposta, cada aviso fica ate o seu motivo acabar (o da
    /// tela nao some debaixo do do som, nem o da chave nao salva), e depois
    /// de a ligacao cair ha "Conectar de novo" -- que nao apaga a chave -- e
    /// o nativo fica a saber que ja nada sai.
    #[test]
    fn the_shipped_page_keeps_every_notice_and_recovers_after_a_drop() {
        let theme = json!({ "--bg": "#202124" });
        // O aviso real do nativo quando a chave nao pode ser salva.
        let dir = temp_dir("page-notices");
        let blocked = dir.join("bloqueado");
        std::fs::write(&blocked, b"ficheiro").expect("ficheiro");
        let LiveStep::Start(start_script) = live_step(
            LiveMessage::SaveKey(key(TEST_KEY)),
            &LiveKeyStore::in_dir(&blocked),
            &theme,
        ) else {
            panic!("esperava arrancar");
        };
        let out = run_page(
            r#"
const { vm, sandbox, input, flush, elements, posts, sockets, requests, docListeners } = t;
const out = {};
vm.runInContext(input.askScript, sandbox);
// 1100 caracteres: nenhuma chave e assim, e nem cabe no canal do painel.
elements.key.value = 'A'.repeat(1100);
elements.save.click();
out.oversize = { posts: posts.length, disabled: elements.save.disabled, errorHidden: elements['key-error'].hidden, error: elements['key-error'].textContent, field: elements.key.value };
// Arranca sem gesto: a tela e recusada e o som fica parado.
vm.runInContext(input.startScript, sandbox);
for (let i = 0; i < 6; i++) await flush();
out.started = { notices: t.shown(), screen: elements['t-screen'].getAttribute('aria-pressed') };
// O primeiro clique ativa o som: so o aviso do som sai.
for (const fn of docListeners.pointerdown || []) fn({});
for (let i = 0; i < 3; i++) await flush();
out.afterClick = t.shown();
// Clicar em Tela liga-a: o aviso da tela sai.
elements['t-screen'].click();
for (let i = 0; i < 3; i++) await flush();
out.afterScreen = { notices: t.shown(), screen: elements['t-screen'].getAttribute('aria-pressed') };
// A ligacao cai (sem handle para retomar).
const socket = sockets[0];
socket.open();
socket.receive('{"setupComplete":{}}');
await flush();
socket.drop(1006, '');
for (let i = 0; i < 3; i++) await flush();
out.dropped = {
  errorHidden: elements['error-box'].hidden, restartHidden: elements.restart.hidden,
  changeKeyHidden: elements['error-key'].hidden, error: elements.error.textContent,
  notices: t.shown(), last: posts[posts.length - 1]
};
// Os botoes das fontes de uma sessao morta nao abrem nada.
const before = { sockets: sockets.length, requests: requests.length };
elements['t-mic'].click();
elements['t-camera'].click();
await flush();
out.deadToggles = { sockets: sockets.length - before.sockets, requests: requests.length - before.requests };
elements.restart.click();
out.restart = { errorHidden: elements['error-box'].hidden, last: posts[posts.length - 1] };
out.posts = posts;
return out;
"#,
            json!({ "screenFails": 1, "suspended": true }),
            json!({
                "askScript": live_ask_key_script(&theme, None),
                "startScript": start_script,
            }),
        );
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            out["oversize"],
            json!({
                "posts": 1,
                "disabled": false,
                "errorHidden": false,
                "error": LIVE_INVALID_KEY_NOTICE,
                "field": ""
            }),
            "colagem grande demais: aviso na hora, botao ativo, nada no canal"
        );

        let screen_hint =
            "Tela não compartilhada. Clique em «Tela» para escolher uma janela ou a tela inteira.";
        let audio_hint = "Clique no painel para ativar o som e o microfone.";
        let started = out["started"]["notices"].as_array().expect("avisos");
        let kinds: Vec<&str> = started
            .iter()
            .map(|notice| notice[0].as_str().expect("tipo"))
            .collect();
        assert_eq!(kinds, ["save", "screen", "audio"], "{started:?}");
        assert!(
            started[0][1]
                .as_str()
                .is_some_and(|text| text.contains("só para esta sessão")),
            "{started:?}"
        );
        assert_eq!(started[1][1], screen_hint);
        assert_eq!(started[2][1], audio_hint);
        assert_eq!(out["started"]["screen"], "false");

        let save_notice = started[0].clone();
        assert_eq!(
            out["afterClick"],
            json!([save_notice, ["screen", screen_hint]]),
            "o clique so tira o aviso do som"
        );
        assert_eq!(
            out["afterScreen"],
            json!({ "notices": [save_notice], "screen": "true" }),
            "a tela ligou: o aviso dela sai, o da chave fica"
        );
        assert_eq!(
            out["dropped"],
            json!({
                "errorHidden": false,
                "restartHidden": false,
                "changeKeyHidden": true,
                "error": "A conexão caiu (rede ou servidor indisponível).",
                "notices": [save_notice],
                "last": r#"{"action":"stopped","args":{}}"#
            })
        );
        assert_eq!(out["deadToggles"], json!({ "sockets": 0, "requests": 0 }));
        assert_eq!(
            out["restart"],
            json!({ "errorHidden": true, "last": r#"{"action":"ready","args":{}}"# })
        );
        assert_eq!(
            parsed_posts(&out["posts"]),
            [LiveMessage::Ready, LiveMessage::Stopped, LiveMessage::Ready],
            "nem a colagem grande nem Conectar de novo apagam a chave"
        );
    }
}
