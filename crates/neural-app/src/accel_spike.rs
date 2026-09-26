//! Spike do `AcceleratorKeyPressed` (item infra-accel-spike do plano 2.3).
//!
//! Pergunta a que responde, no exe de release: com `Handled = TRUE` num
//! `AcceleratorKeyPressed`, cada atalho nativo por omissao (a tabela
//! `SPIKE_CHORDS`) deixa de chegar ao `keydown` da pagina e ao `act()` do
//! `NEURALIA_KEYMAP_SCRIPT`, e o lado nativo dispara uma so vez -- tambem com
//! a tecla presa (auto-repeat)? A resposta e a tabela por hospedeiro que o
//! job `accel-spike` imprime (`scripts/test-accel-spike.ps1`), no workflow
//! proprio `.github/workflows/accel-spike.yml`, fora do CI de que o release
//! depende.
//!
//! Este modulo so existe nos testes e no build de CI com
//! `--features accel-spike`. O exe publicado e compilado sem a feature: o
//! `SPIKE_BUILD_MARKER` nao esta nos bytes dele (gate
//! `scripts/test-accel-spike-marker.ps1` sobre o `ci-tested/NeuralIA.exe`,
//! amarrado ao ci.yml por `scripts/test-release-contract.mjs`). Nenhum
//! comportamento do produto sai daqui.
//!
//! Aqui vive so o que nao toca em janelas -- a tabela de atalhos, a decisao
//! Handled/dispara, os comandos do condutor, as linhas do registo, o PDF
//! minimo e os scripts da sonda --, testado tambem no runner Linux. A cola
//! com o `App` e o COM do WebView2 esta em `accel_spike_app.rs`.
#![cfg_attr(
    any(not(feature = "accel-spike"), not(target_os = "windows")),
    allow(dead_code)
)]

use url::{Host, Url};

/// Vai para o registo do spike na primeira linha e fica nos bytes do exe
/// compilado com a feature. O gate de release procura-o no exe publicado:
/// tem de estar ausente.
pub(crate) const SPIKE_BUILD_MARKER: &str = "NEURALIA-ACCEL-SPIKE-BUILD-v1";

/// Variavel de ambiente com a pasta do condutor (comandos e registo). Sem
/// ela o exe do spike marca os atalhos como tratados mas nao regista nada,
/// nao le comandos e nao engole eventos.
pub(crate) const SPIKE_DIR_ENV: &str = "NEURALIA_ACCEL_SPIKE_DIR";

/// O ficheiro que o condutor escreve (por rename atomico) e o exe consome.
pub(crate) const SPIKE_COMMAND_FILE: &str = "cmd.txt";
/// O registo JSON Lines que o condutor le.
pub(crate) const SPIKE_LOG_FILE: &str = "spike.log";

/// Os hospedeiros de WebView da tabela do spike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpikeHost {
    Column,
    Split,
    PrivateSplit,
    External,
    Reader,
    Pdf,
    Epub,
    SidePanel,
    Service,
}

impl SpikeHost {
    pub(crate) const ALL: [SpikeHost; 9] = [
        SpikeHost::Column,
        SpikeHost::Split,
        SpikeHost::PrivateSplit,
        SpikeHost::External,
        SpikeHost::Reader,
        SpikeHost::Pdf,
        SpikeHost::Epub,
        SpikeHost::SidePanel,
        SpikeHost::Service,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            SpikeHost::Column => "Column",
            SpikeHost::Split => "Split",
            SpikeHost::PrivateSplit => "PrivateSplit",
            SpikeHost::External => "External",
            SpikeHost::Reader => "Reader",
            SpikeHost::Pdf => "Pdf",
            SpikeHost::Epub => "Epub",
            SpikeHost::SidePanel => "SidePanel",
            SpikeHost::Service => "Service",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<SpikeHost> {
        SpikeHost::ALL.into_iter().find(|host| host.name() == name)
    }

    /// Os hospedeiros que carregam a pagina de fixture de 127.0.0.1: o
    /// `open` deles exige o endereco. A coluna aceita-o sem o exigir: sem
    /// ele fica na pagina ao vivo, ou volta a ela se a fixture tinha sido
    /// pedida (`ColumnFixture::release`; ver `column_fixture_navigation`).
    pub(crate) fn loads_fixture(self) -> bool {
        matches!(
            self,
            SpikeHost::Split | SpikeHost::PrivateSplit | SpikeHost::External | SpikeHost::Service
        )
    }
}

/// Um atalho nativo por omissao do plano (ipc_plan: "Every new chord is a
/// native accelerator").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpikeChord {
    pub(crate) name: &'static str,
    /// Tecla virtual do Windows, como o `AcceleratorKeyPressed` a entrega.
    pub(crate) vk: u32,
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    /// O `KeyboardEvent.code` com que a pagina veria a tecla.
    pub(crate) code: &'static str,
}

const fn chord(
    name: &'static str,
    vk: u32,
    ctrl: bool,
    shift: bool,
    code: &'static str,
) -> SpikeChord {
    SpikeChord {
        name,
        vk,
        ctrl,
        shift,
        code,
    }
}

/// Os dez atalhos do brief, pela ordem dele. Nenhum usa Alt.
pub(crate) const SPIKE_CHORDS: [SpikeChord; 10] = [
    chord("Ctrl+D", 0x44, true, false, "KeyD"),
    chord("Ctrl+J", 0x4A, true, false, "KeyJ"),
    chord("Ctrl+Shift+E", 0x45, true, true, "KeyE"),
    chord("Ctrl+Shift+A", 0x41, true, true, "KeyA"),
    chord("Ctrl+Shift+N", 0x4E, true, true, "KeyN"),
    chord("Ctrl+Shift+P", 0x50, true, true, "KeyP"),
    chord("F1", 0x70, false, false, "F1"),
    chord("Ctrl+Shift+S", 0x53, true, true, "KeyS"),
    chord("Ctrl+Shift+F", 0x46, true, true, "KeyF"),
    chord("Ctrl+O", 0x4F, true, false, "KeyO"),
];

/// `COREWEBVIEW2_KEY_EVENT_KIND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyEventKind {
    KeyDown,
    KeyUp,
    SystemKeyDown,
    SystemKeyUp,
}

impl KeyEventKind {
    /// Os valores do enum do WebView2 (0..=3); outro valor nao e uma tecla
    /// que o spike conheca.
    pub(crate) fn from_raw(raw: i32) -> Option<KeyEventKind> {
        match raw {
            0 => Some(KeyEventKind::KeyDown),
            1 => Some(KeyEventKind::KeyUp),
            2 => Some(KeyEventKind::SystemKeyDown),
            3 => Some(KeyEventKind::SystemKeyUp),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            KeyEventKind::KeyDown => "down",
            KeyEventKind::KeyUp => "up",
            KeyEventKind::SystemKeyDown => "sysdown",
            KeyEventKind::SystemKeyUp => "sysup",
        }
    }

    fn is_down(self) -> bool {
        matches!(self, KeyEventKind::KeyDown | KeyEventKind::SystemKeyDown)
    }
}

/// O que o handler le de um `AcceleratorKeyPressed`: a tecla, o tipo, os
/// modificadores (`GetKeyState`) e o `WasKeyDown` do `PhysicalKeyStatus`,
/// que marca a repeticao da tecla presa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpikeKey {
    pub(crate) vk: u32,
    pub(crate) kind: KeyEventKind,
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
    pub(crate) was_down: bool,
}

/// A decisao do handler para uma tecla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpikeVerdict {
    /// Indice em `SPIKE_CHORDS`, quando a tecla e um dos atalhos.
    pub(crate) chord: Option<usize>,
    /// `Handled = TRUE`: so os atalhos da tabela, em baixo, repetidos e em
    /// cima. Tudo o resto segue para a pagina como sempre.
    pub(crate) handled: bool,
    /// O lado nativo dispara: so a primeira descida, nunca a repeticao.
    pub(crate) fire: bool,
    /// Descida repetida (tecla presa): tratada, mas nao dispara.
    pub(crate) repeat: bool,
}

pub(crate) fn chord_index(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<usize> {
    if alt {
        return None;
    }
    SPIKE_CHORDS
        .iter()
        .position(|chord| chord.vk == vk && chord.ctrl == ctrl && chord.shift == shift)
}

pub(crate) fn spike_verdict(key: SpikeKey) -> SpikeVerdict {
    let chord = chord_index(key.vk, key.ctrl, key.shift, key.alt);
    let bound = chord.is_some();
    let down = key.kind.is_down();
    SpikeVerdict {
        chord,
        handled: bound,
        fire: bound && down && !key.was_down,
        repeat: bound && down && key.was_down,
    }
}

/// Um pedido do condutor (`scripts/test-accel-spike.ps1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpikeVerb {
    /// Abre o hospedeiro (com a URL da fixture quando ele a carrega).
    Open(Option<String>),
    /// Instala a sonda de teclas no documento de topo e diz onde esta.
    Arm,
    /// Janela em primeiro plano e teclado na WebView do hospedeiro.
    Focus,
    /// Comeca a tentativa N: zera a sonda e o contador de `act()`.
    Begin(u32),
    /// Le o que a pagina viu na tentativa N.
    Pull(u32),
    /// O primeiro comando da corrida: o ack diz que o event loop ja le
    /// comandos (o `hello` sai no `resumed`, antes de o comparador do
    /// `NEURALIA_STARTUP_INPUT` se construir) e se o hospedeiro esta aberto.
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpikeCommand {
    pub(crate) seq: u64,
    pub(crate) host: SpikeHost,
    pub(crate) verb: SpikeVerb,
}

/// So a fixture do condutor: `http://127.0.0.1:<porta>/...`. O exe do
/// spike nunca abre outro endereco por um comando.
pub(crate) fn is_loopback_fixture(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "http"
        && matches!(parsed.host(), Some(Host::Ipv4(ip)) if ip.is_loopback() && ip.octets() == [127, 0, 0, 1])
        && parsed.port().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

/// A origem (`http://127.0.0.1:<porta>`) de uma URL da fixture; `None` para
/// qualquer outra URL.
pub(crate) fn fixture_origin(url: &str) -> Option<String> {
    if !is_loopback_fixture(url) {
        return None;
    }
    Url::parse(url)
        .ok()
        .map(|parsed| parsed.origin().ascii_serialization())
}

/// A excecao de navegacao da coluna, so no exe do spike: a coluna pode ir
/// para `target` quando o condutor ja pediu a fixture nela (`open Column
/// <url>`, que guarda a origem em `fixture`) e `target` e exatamente dessa
/// origem -- http, 127.0.0.1 e a porta da fixture. Tudo o resto fica com o
/// gate que embarca (`comparator_webview_builder`), que este codigo nao
/// toca; sem o pedido do condutor nem a fixture passa.
pub(crate) fn column_fixture_navigation(target: &str, fixture: Option<&str>) -> bool {
    match (fixture, fixture_origin(target)) {
        (Some(allowed), Some(origin)) => origin == allowed,
        _ => false,
    }
}

/// O pedido do condutor para a coluna, que o exe do spike guarda: a origem
/// da fixture pedida (`open Column <url>`) e a navegacao do WebView2 que a
/// leva (o `NavigationId` do `NavigationStarting`), para o
/// `NavigationCompleted` dessa navegacao ir para o registo e o condutor
/// saber logo se a coluna carregou a fixture ou se o runtime a recusou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnFixture {
    origin: Option<String>,
    navigation: Option<u64>,
}

impl ColumnFixture {
    /// Nenhum pedido: nem a fixture passa pela excecao.
    pub(crate) const NONE: ColumnFixture = ColumnFixture {
        origin: None,
        navigation: None,
    };

    /// `open Column <url>`: a partir daqui a excecao deixa passar a origem
    /// exata de `url` (e so ela). Devolve essa origem; `None` se `url` nao
    /// e a fixture de 127.0.0.1 (nada passa).
    pub(crate) fn request(&mut self, url: &str) -> Option<&str> {
        self.origin = fixture_origin(url);
        self.navigation = None;
        self.origin.as_deref()
    }

    /// Um `NavigationStarting` da coluna: `true` quando e a fixture pedida
    /// (`column_fixture_navigation`) -- a excecao repoe `Cancel = false` e a
    /// navegacao fica a ser a que se segue ate ao `NavigationCompleted`.
    pub(crate) fn starting(&mut self, uri: &str, navigation: u64) -> bool {
        let fixture = column_fixture_navigation(uri, self.origin.as_deref());
        if fixture {
            self.navigation = Some(navigation);
        }
        fixture
    }

    /// Um `NavigationCompleted` da coluna: `true` so para a navegacao da
    /// fixture (as das paginas ao vivo das tres colunas nao contam).
    pub(crate) fn completed(&self, navigation: u64) -> bool {
        self.navigation == Some(navigation)
    }

    /// `open Column` sem URL: a excecao deixa de valer. Devolve se havia um
    /// pedido da fixture -- entao a coluna pode ter ficado a meio dela e o
    /// exe repoe a pagina ao vivo do fornecedor.
    pub(crate) fn release(&mut self) -> bool {
        let requested = self.origin.is_some();
        *self = ColumnFixture::NONE;
        requested
    }
}

/// O detalhe do ack de `ping`: o hospedeiro, se esta aberto e a superficie.
/// O condutor le o `aberto=` (Test-PingOpen).
pub(crate) fn ping_detail(host: SpikeHost, open: bool, surface: &str) -> String {
    format!("{} aberto={open} surface={surface}", host.name())
}

/// `<seq> <verbo> <Hospedeiro> [argumento]`, uma linha.
pub(crate) fn parse_spike_command(line: &str) -> Result<SpikeCommand, String> {
    let mut parts = line.split_whitespace();
    let seq = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| format!("comando sem numero: {line:?}"))?;
    let verb = parts
        .next()
        .ok_or_else(|| format!("comando sem verbo: {line:?}"))?;
    let host = parts
        .next()
        .and_then(SpikeHost::parse)
        .ok_or_else(|| format!("hospedeiro desconhecido: {line:?}"))?;
    let argument = parts.next();
    if parts.next().is_some() {
        return Err(format!("argumentos a mais: {line:?}"));
    }
    let trial = |argument: Option<&str>| {
        argument
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|trial| *trial > 0)
            .ok_or_else(|| format!("tentativa invalida: {line:?}"))
    };
    let verb = match verb {
        "open" => match argument {
            Some(url) if is_loopback_fixture(url) => SpikeVerb::Open(Some(url.to_string())),
            Some(_) => return Err(format!("fixture fora de 127.0.0.1: {line:?}")),
            None if host.loads_fixture() => {
                return Err(format!("{} precisa da URL da fixture", host.name()));
            }
            None => SpikeVerb::Open(None),
        },
        "arm" | "focus" | "ping" if argument.is_some() => {
            return Err(format!("argumentos a mais: {line:?}"));
        }
        "arm" => SpikeVerb::Arm,
        "focus" => SpikeVerb::Focus,
        "ping" => SpikeVerb::Ping,
        "begin" => SpikeVerb::Begin(trial(argument)?),
        "pull" => SpikeVerb::Pull(trial(argument)?),
        _ => return Err(format!("verbo desconhecido: {line:?}")),
    };
    Ok(SpikeCommand { seq, host, verb })
}

/// Uma string JSON (com as aspas), escapada.
pub(crate) fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 || ch == '\u{2028}' || ch == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// A primeira linha do registo: o marcador do build, a tabela de atalhos e
/// os hospedeiros. O condutor compara a tabela com a dele antes de comecar.
pub(crate) fn hello_line(pid: u32) -> String {
    let mut out = String::from("{\"t\":\"hello\",\"build\":");
    out.push_str(&json_string(SPIKE_BUILD_MARKER));
    out.push_str(&format!(",\"pid\":{pid},\"chords\":["));
    for (index, chord) in SPIKE_CHORDS.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"name\":{},\"vk\":{},\"ctrl\":{},\"shift\":{},\"code\":{}}}",
            json_string(chord.name),
            chord.vk,
            chord.ctrl,
            chord.shift,
            json_string(chord.code)
        ));
    }
    out.push_str("],\"hosts\":[");
    for (index, host) in SpikeHost::ALL.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(host.name()));
    }
    out.push_str("]}");
    out
}

pub(crate) fn ack_line(seq: u64, ok: bool, detail: &str) -> String {
    format!(
        "{{\"t\":\"ack\",\"seq\":{seq},\"ok\":{ok},\"detail\":{}}}",
        json_string(detail)
    )
}

/// O handler nativo instalado (ou nao) numa WebView acabada de construir.
pub(crate) fn hooked_line(host: SpikeHost, error: Option<&str>) -> String {
    match error {
        None => format!(
            "{{\"t\":\"hooked\",\"host\":{},\"ok\":true}}",
            json_string(host.name())
        ),
        Some(error) => format!(
            "{{\"t\":\"hooked\",\"host\":{},\"ok\":false,\"error\":{}}}",
            json_string(host.name()),
            json_string(error)
        ),
    }
}

/// Uma tecla da tabela que o `AcceleratorKeyPressed` de `host` viu.
pub(crate) fn native_line(
    trial: u32,
    host: SpikeHost,
    kind: KeyEventKind,
    verdict: SpikeVerdict,
) -> Option<String> {
    let chord = SPIKE_CHORDS.get(verdict.chord?)?;
    Some(format!(
        "{{\"t\":\"native\",\"trial\":{trial},\"host\":{},\"chord\":{},\"kind\":{},\"handled\":{},\"fired\":{},\"repeat\":{}}}",
        json_string(host.name()),
        json_string(chord.name),
        json_string(kind.name()),
        verdict.handled,
        verdict.fire,
        verdict.repeat
    ))
}

/// Um evento que um `act()` da pagina produziu (e que o spike engoliu).
pub(crate) fn act_line(trial: u32, act: &str) -> String {
    format!(
        "{{\"t\":\"act\",\"trial\":{trial},\"act\":{}}}",
        json_string(act)
    )
}

/// O `NavigationStarting` da fixture na coluna: o `Cancel` que o gate que
/// embarca deixou (`gate`) e o que ficou depois da excecao do spike
/// (`cancel`). So a fixture (127.0.0.1) vai para aqui.
pub(crate) fn colnav_start_line(navigation: u64, uri: &str, gate: bool, cancel: bool) -> String {
    format!(
        "{{\"t\":\"colnav\",\"phase\":\"start\",\"nav\":{navigation},\"uri\":{},\"gate\":{gate},\"cancel\":{cancel}}}",
        json_string(uri)
    )
}

/// O `NavigationCompleted` dessa navegacao: `IsSuccess` e o
/// `COREWEBVIEW2_WEB_ERROR_STATUS` (14 = OPERATION_CANCELED).
pub(crate) fn colnav_done_line(navigation: u64, ok: bool, status: i32) -> String {
    format!(
        "{{\"t\":\"colnav\",\"phase\":\"done\",\"nav\":{navigation},\"ok\":{ok},\"status\":{status}}}"
    )
}

/// O que a sonda devolveu (`raw` e o JSON do `ExecuteScript`, dado da
/// pagina: vai como string, e o condutor volta a le-lo).
pub(crate) fn page_line(seq: u64, trial: u32, host: SpikeHost, phase: &str, raw: &str) -> String {
    format!(
        "{{\"t\":\"page\",\"seq\":{seq},\"trial\":{trial},\"host\":{},\"phase\":{},\"result\":{}}}",
        json_string(host.name()),
        json_string(phase),
        json_string(raw)
    )
}

/// Um PDF de uma pagina, com a tabela xref certa, para o hospedeiro PDF sem
/// rede: o visualizador so precisa de bytes.
pub(crate) fn tiny_pdf() -> Vec<u8> {
    let content = "BT /F1 24 Tf 72 700 Td (NeuralIA accel spike) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }
    let xref = out.len();
    out.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
    out.push_str("0000000000 65535 f \n");
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

/// A sonda, no inicio de cada tentativa (e no `arm`, que e o mesmo com a
/// tentativa 0). Instala -- uma vez por documento -- ouvintes de `keydown` e
/// `keypress` na fase de captura da `window` do documento de topo, que
/// guardam as teclas CONFIAVEIS (as do sistema) que nao sao modificadores;
/// zera o que ela e a fixture ja tinham visto; tira a barra de procura que o
/// Ctrl+F do mapa de teclas cria; e tira o foco de um iframe, para a tecla
/// cair no documento de topo. Devolve o token da sonda (outro documento =
/// outro token: uma navegacao a meio da tentativa invalida-a), o endereco, o
/// estado de carga e se o documento tem o foco.
pub(crate) const PROBE_ARM_SCRIPT: &str = r#"(function () {
  var w = window;
  var probe = w.__neuraliaAccelProbe;
  if (!probe) {
    probe = { down: [], press: [], token: String(Date.now()) + '-' + String(Math.random()).slice(2, 10) };
    var skip = Object.create(null);
    skip.Control = 1; skip.Shift = 1; skip.Alt = 1; skip.Meta = 1;
    var record = function (list) {
      return function (e) {
        if (!e.isTrusted || skip[e.key]) { return; }
        if (list.length >= 200) { return; }
        list.push({ key: String(e.key), code: String(e.code), ctrl: !!e.ctrlKey, shift: !!e.shiftKey, repeat: !!e.repeat });
      };
    };
    w.addEventListener('keydown', record(probe.down), true);
    w.addEventListener('keypress', record(probe.press), true);
    Object.defineProperty(w, '__neuraliaAccelProbe', { value: probe });
  }
  probe.down.length = 0;
  probe.press.length = 0;
  var fixture = w.__fixtureSeen;
  if (fixture && fixture.down && fixture.press) { fixture.down.length = 0; fixture.press.length = 0; }
  var find = document.getElementById('neuralia-find');
  if (find && find.remove) { find.remove(); }
  try {
    var active = document.activeElement;
    if (active && (active.tagName === 'IFRAME' || active.tagName === 'FRAME') && active.blur) { active.blur(); }
  } catch (err) {}
  return { token: probe.token, href: String(location.href), ready: String(document.readyState), focus: !!document.hasFocus() };
})()"#;

/// Fim de uma tentativa: o que a sonda e a fixture viram, se a barra de
/// procura do mapa de teclas apareceu, e onde estava o foco.
pub(crate) const PROBE_PULL_SCRIPT: &str = r#"(function () {
  var probe = window.__neuraliaAccelProbe;
  var fixture = window.__fixtureSeen;
  var active = document.activeElement;
  var fixtureList = function (name) {
    return fixture && fixture[name] && fixture[name].slice ? fixture[name].slice(0, 50) : null;
  };
  return {
    token: probe ? probe.token : null,
    down: probe ? probe.down.slice(0, 50) : [],
    press: probe ? probe.press.slice(0, 50) : [],
    fixtureDown: fixtureList('down'),
    fixturePress: fixtureList('press'),
    find: !!document.getElementById('neuralia-find'),
    focus: !!document.hasFocus(),
    active: active && active.tagName ? String(active.tagName) : '',
    href: String(location.href)
  };
})()"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn key(vk: u32, ctrl: bool, shift: bool) -> SpikeKey {
        SpikeKey {
            vk,
            kind: KeyEventKind::KeyDown,
            ctrl,
            shift,
            alt: false,
            was_down: false,
        }
    }

    #[test]
    fn the_chord_table_is_the_briefs_ten_default_native_chords() {
        let names: Vec<&str> = SPIKE_CHORDS.iter().map(|chord| chord.name).collect();
        assert_eq!(
            names,
            [
                "Ctrl+D",
                "Ctrl+J",
                "Ctrl+Shift+E",
                "Ctrl+Shift+A",
                "Ctrl+Shift+N",
                "Ctrl+Shift+P",
                "F1",
                "Ctrl+Shift+S",
                "Ctrl+Shift+F",
                "Ctrl+O",
            ]
        );
        // O nome diz os modificadores e a tecla; o vk e o code tem de dizer o
        // mesmo, senao o condutor carrega numa tecla e o handler procura
        // outra.
        for chord in SPIKE_CHORDS {
            let key = chord.name.rsplit('+').next().expect("tecla");
            assert_eq!(
                chord.ctrl,
                chord.name.starts_with("Ctrl+"),
                "{}",
                chord.name
            );
            assert_eq!(chord.shift, chord.name.contains("Shift+"), "{}", chord.name);
            if key == "F1" {
                assert_eq!((chord.vk, chord.code), (0x70, "F1"));
            } else {
                assert_eq!(key.len(), 1, "{}", chord.name);
                let letter = key.as_bytes()[0];
                assert_eq!(chord.vk, u32::from(letter), "{}", chord.name);
                assert_eq!(chord.code, format!("Key{key}"), "{}", chord.name);
            }
        }
        // Uma combinacao, uma entrada: dois atalhos com o mesmo vk e os
        // mesmos modificadores fariam o segundo nunca ser medido.
        for (index, chord) in SPIKE_CHORDS.iter().enumerate() {
            assert_eq!(
                chord_index(chord.vk, chord.ctrl, chord.shift, false),
                Some(index)
            );
        }
    }

    #[test]
    fn only_the_table_chords_are_handled_and_only_the_first_press_fires() {
        for (index, chord) in SPIKE_CHORDS.iter().enumerate() {
            let first = spike_verdict(key(chord.vk, chord.ctrl, chord.shift));
            assert_eq!(
                first,
                SpikeVerdict {
                    chord: Some(index),
                    handled: true,
                    fire: true,
                    repeat: false
                },
                "{}",
                chord.name
            );
            // Tecla presa: as descidas seguintes chegam com WasKeyDown e sao
            // tratadas (a pagina nao as ve) mas nao disparam de novo.
            let held = spike_verdict(SpikeKey {
                was_down: true,
                ..key(chord.vk, chord.ctrl, chord.shift)
            });
            assert!(held.handled && !held.fire && held.repeat, "{}", chord.name);
            // A subida tambem fica fora da pagina, sem disparar.
            let up = spike_verdict(SpikeKey {
                kind: KeyEventKind::KeyUp,
                ..key(chord.vk, chord.ctrl, chord.shift)
            });
            assert!(up.handled && !up.fire && !up.repeat, "{}", chord.name);
        }

        // Os mesmos vk com outros modificadores seguem para a pagina: o
        // Ctrl+F da procura, o Ctrl+N da aba nova, o Ctrl+P de imprimir e o
        // Ctrl+Shift+J das ferramentas continuam a ser do mapa de teclas.
        for (vk, ctrl, shift) in [
            (0x46, true, false),
            (0x4E, true, false),
            (0x50, true, false),
            (0x4A, true, true),
            (0x44, false, false),
            (0x70, true, false),
            (0x4F, true, true),
        ] {
            let verdict = spike_verdict(key(vk, ctrl, shift));
            assert_eq!(
                verdict,
                SpikeVerdict {
                    chord: None,
                    handled: false,
                    fire: false,
                    repeat: false
                },
                "vk {vk:#x} ctrl {ctrl} shift {shift}"
            );
        }
        // Com Alt nada e da tabela, e os modificadores sozinhos nunca sao.
        let alt = spike_verdict(SpikeKey {
            alt: true,
            ..key(0x44, true, false)
        });
        assert!(!alt.handled && alt.chord.is_none());
        for modifier in [0x10, 0x11, 0x12] {
            assert!(!spike_verdict(key(modifier, true, true)).handled);
        }
    }

    #[test]
    fn key_event_kinds_follow_the_webview2_enum() {
        assert_eq!(KeyEventKind::from_raw(0), Some(KeyEventKind::KeyDown));
        assert_eq!(KeyEventKind::from_raw(1), Some(KeyEventKind::KeyUp));
        assert_eq!(KeyEventKind::from_raw(2), Some(KeyEventKind::SystemKeyDown));
        assert_eq!(KeyEventKind::from_raw(3), Some(KeyEventKind::SystemKeyUp));
        assert_eq!(KeyEventKind::from_raw(4), None);
        assert_eq!(KeyEventKind::from_raw(-1), None);
    }

    #[test]
    fn driver_commands_parse_only_the_closed_grammar() {
        assert_eq!(
            parse_spike_command("7 open Split http://127.0.0.1:5123/fixture.html"),
            Ok(SpikeCommand {
                seq: 7,
                host: SpikeHost::Split,
                verb: SpikeVerb::Open(Some("http://127.0.0.1:5123/fixture.html".to_string())),
            })
        );
        assert_eq!(
            parse_spike_command("8 open Epub"),
            Ok(SpikeCommand {
                seq: 8,
                host: SpikeHost::Epub,
                verb: SpikeVerb::Open(None),
            })
        );
        assert_eq!(
            parse_spike_command("9 begin Column 12").map(|command| command.verb),
            Ok(SpikeVerb::Begin(12))
        );
        assert_eq!(
            parse_spike_command("10 pull Pdf 12").map(|command| command.verb),
            Ok(SpikeVerb::Pull(12))
        );
        assert_eq!(
            parse_spike_command("11 arm SidePanel").map(|command| command.verb),
            Ok(SpikeVerb::Arm)
        );
        assert_eq!(
            parse_spike_command("12 focus Service").map(|command| command.verb),
            Ok(SpikeVerb::Focus)
        );
        // A coluna aceita a fixture (a pagina do brief, sem rede) ou nada (a
        // pagina ao vivo que o NEURALIA_STARTUP_INPUT abriu).
        assert_eq!(
            parse_spike_command("13 open Column http://127.0.0.1:5123/fixture.html")
                .map(|command| command.verb),
            Ok(SpikeVerb::Open(Some(
                "http://127.0.0.1:5123/fixture.html".to_string()
            )))
        );
        assert_eq!(
            parse_spike_command("14 open Column").map(|command| command.verb),
            Ok(SpikeVerb::Open(None))
        );
        // O primeiro comando da corrida, a qualquer hospedeiro, sem argumento.
        assert_eq!(
            parse_spike_command("1 ping Column"),
            Ok(SpikeCommand {
                seq: 1,
                host: SpikeHost::Column,
                verb: SpikeVerb::Ping,
            })
        );
        assert_eq!(
            parse_spike_command("2 ping External").map(|command| command.verb),
            Ok(SpikeVerb::Ping)
        );

        for refused in [
            "",
            "open Split http://127.0.0.1:5123/",
            "1 open Split",
            "1 open External https://example.com/",
            "1 open External http://127.0.0.2:5123/",
            "1 open External http://localhost:5123/",
            "1 open External http://127.0.0.1/",
            "1 open External http://user@127.0.0.1:5123/",
            "1 open Tab http://127.0.0.1:5123/",
            "1 begin Column",
            "1 begin Column 0",
            "1 pull Column x",
            "1 arm Column extra",
            "1 focus Column extra",
            "1 open Column http://127.0.0.1:5123/ extra",
            "1 close Column",
            "1 ping Column extra",
            "1 ping",
            "1 ping Tab",
        ] {
            assert!(parse_spike_command(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn the_column_fixture_exception_is_the_exact_fixture_origin_only() {
        let origin = fixture_origin("http://127.0.0.1:5123/fixture.html");
        assert_eq!(origin.as_deref(), Some("http://127.0.0.1:5123"));
        let allowed = origin.as_deref();
        assert!(column_fixture_navigation(
            "http://127.0.0.1:5123/fixture.html",
            allowed
        ));
        assert!(column_fixture_navigation(
            "http://127.0.0.1:5123/fixture.html?x=1#y",
            allowed
        ));
        for refused in [
            "http://127.0.0.1:5124/fixture.html",
            "https://127.0.0.1:5123/fixture.html",
            "http://localhost:5123/fixture.html",
            "http://127.0.0.2:5123/fixture.html",
            "http://[::1]:5123/fixture.html",
            "http://user@127.0.0.1:5123/fixture.html",
            "http://127.0.0.1/fixture.html",
            "http://192.168.0.1:5123/fixture.html",
            "https://www.google.com/search?q=x",
            "about:blank",
            "neuralia:home",
            "",
        ] {
            assert!(!column_fixture_navigation(refused, allowed), "{refused:?}");
        }
        // Sem o `open Column <url>` do condutor, nem a propria fixture passa.
        assert!(!column_fixture_navigation(
            "http://127.0.0.1:5123/fixture.html",
            None
        ));
        for refused in [
            "https://www.google.com/",
            "http://localhost:5123/",
            "http://127.0.0.1/",
        ] {
            assert_eq!(fixture_origin(refused), None, "{refused:?}");
        }
    }

    /// A coluna a caminho da fixture: so a navegacao da fixture pedida fica
    /// seguida (o `NavigationCompleted` dela vai para o registo, as das
    /// paginas ao vivo nao), e o `open Column` sem URL desfaz o pedido e diz
    /// se havia um (entao o exe repoe a pagina ao vivo).
    #[test]
    fn the_column_fixture_request_follows_only_its_own_navigation() {
        let mut state = ColumnFixture::NONE;
        // Sem pedido nada passa nem fica seguido.
        assert!(!state.starting("http://127.0.0.1:5123/fixture.html", 7));
        assert!(!state.completed(7));
        assert!(!state.release());

        assert_eq!(
            state.request("http://127.0.0.1:5123/fixture.html"),
            Some("http://127.0.0.1:5123")
        );
        // As paginas ao vivo das tres colunas continuam a navegar e nao
        // contam; a fixture sim, com o id dela.
        assert!(!state.starting("https://www.google.com/search?q=x", 3));
        assert!(!state.starting("http://127.0.0.1:5124/fixture.html", 4));
        assert!(state.starting("http://127.0.0.1:5123/fixture.html", 9));
        assert!(state.completed(9));
        assert!(!state.completed(3));
        assert!(!state.completed(4));

        // Um pedido novo esquece a navegacao anterior.
        assert_eq!(
            state.request("http://127.0.0.1:6000/fixture.html"),
            Some("http://127.0.0.1:6000")
        );
        assert!(!state.completed(9));
        assert!(!state.starting("http://127.0.0.1:5123/fixture.html", 10));

        // O recuo para a pagina ao vivo: o pedido acaba, e acaba uma vez.
        assert!(state.release());
        assert_eq!(state, ColumnFixture::NONE);
        assert!(!state.release());
        assert!(!state.starting("http://127.0.0.1:6000/fixture.html", 11));

        // Um endereco que nao e a fixture nao abre excecao nenhuma.
        assert_eq!(state.request("http://localhost:5123/fixture.html"), None);
        assert!(!state.starting("http://localhost:5123/fixture.html", 12));
        assert!(!state.release());
    }

    #[test]
    fn host_names_round_trip_and_only_web_hosts_need_the_fixture() {
        for host in SpikeHost::ALL {
            assert_eq!(SpikeHost::parse(host.name()), Some(host));
        }
        let fixture: Vec<&str> = SpikeHost::ALL
            .into_iter()
            .filter(|host| host.loads_fixture())
            .map(SpikeHost::name)
            .collect();
        assert_eq!(fixture, ["Split", "PrivateSplit", "External", "Service"]);
    }

    #[test]
    fn log_lines_are_single_line_json_with_escaped_page_data() {
        assert_eq!(json_string("a\"b\\c\nd\u{1}"), r#""a\"b\\c\nd\u0001""#);
        assert_eq!(json_string("\u{2028}"), "\"\\u2028\"");

        let hello = hello_line(42);
        assert!(hello.contains(SPIKE_BUILD_MARKER));
        assert!(hello.contains(r#""pid":42"#));
        for chord in SPIKE_CHORDS {
            assert!(hello.contains(&json_string(chord.name)), "{}", chord.name);
        }
        for host in SpikeHost::ALL {
            assert!(hello.contains(&json_string(host.name())));
        }

        let verdict = spike_verdict(key(0x4E, true, true));
        assert_eq!(
            native_line(3, SpikeHost::Reader, KeyEventKind::KeyDown, verdict).as_deref(),
            Some(
                r#"{"t":"native","trial":3,"host":"Reader","chord":"Ctrl+Shift+N","kind":"down","handled":true,"fired":true,"repeat":false}"#
            )
        );
        // Uma tecla fora da tabela nao vai para o registo.
        let other = spike_verdict(key(0x4E, true, false));
        assert_eq!(
            native_line(3, SpikeHost::Reader, KeyEventKind::KeyDown, other),
            None
        );

        let page = page_line(5, 3, SpikeHost::Pdf, "pull", "{\"a\":\"x\ny\"}");
        assert!(!page.contains('\n'));
        assert!(page.ends_with(r#""result":"{\"a\":\"x\ny\"}"}"#));
        assert_eq!(
            ack_line(4, false, "sem \"comparador\""),
            r#"{"t":"ack","seq":4,"ok":false,"detail":"sem \"comparador\""}"#
        );
        assert_eq!(
            act_line(9, "newtab"),
            r#"{"t":"act","trial":9,"act":"newtab"}"#
        );
        assert_eq!(
            hooked_line(SpikeHost::Epub, None),
            r#"{"t":"hooked","host":"Epub","ok":true}"#
        );
        assert_eq!(
            hooked_line(SpikeHost::Epub, Some("E_NOINTERFACE")),
            r#"{"t":"hooked","host":"Epub","ok":false,"error":"E_NOINTERFACE"}"#
        );
        // As linhas da coluna a caminho da fixture: o -SelfTest do condutor
        // le exatamente estas (New-ColumnNavRecords).
        assert_eq!(
            colnav_start_line(9, "http://127.0.0.1:5123/fixture.html", true, false),
            r#"{"t":"colnav","phase":"start","nav":9,"uri":"http://127.0.0.1:5123/fixture.html","gate":true,"cancel":false}"#
        );
        assert_eq!(
            colnav_done_line(9, false, 14),
            r#"{"t":"colnav","phase":"done","nav":9,"ok":false,"status":14}"#
        );
        assert_eq!(
            ping_detail(SpikeHost::Column, true, "Comparator"),
            "Column aberto=true surface=Comparator"
        );
        assert_eq!(
            ping_detail(SpikeHost::External, false, "Home"),
            "External aberto=false surface=Home"
        );
    }

    #[test]
    fn the_tiny_pdf_has_a_valid_cross_reference_table() {
        let pdf = tiny_pdf();
        let text = String::from_utf8(pdf.clone()).expect("ascii");
        assert!(text.starts_with("%PDF-1.4\n"));
        assert!(text.ends_with("%%EOF\n"));

        let startxref: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|tail| tail.lines().next())
            .and_then(|value| value.parse().ok())
            .expect("startxref");
        assert!(text[startxref..].starts_with("xref\n0 6\n"));

        let table: Vec<&str> = text[startxref..].lines().skip(2).take(6).collect();
        assert_eq!(table[0], "0000000000 65535 f ");
        for (number, entry) in table.iter().enumerate().skip(1) {
            // Cada entrada tem 20 bytes com o fim de linha, como a norma pede.
            assert_eq!(entry.len() + 1, 20, "{entry:?}");
            let offset: usize = entry[..10].parse().expect("offset");
            assert!(
                text[offset..].starts_with(&format!("{number} 0 obj\n")),
                "objeto {number} fora do sitio"
            );
        }

        let stream = text
            .split_once(">>\nstream\n")
            .and_then(|(_, tail)| tail.split_once("\nendstream"))
            .map(|(stream, _)| stream)
            .expect("stream");
        assert!(text.contains(&format!("/Length {} >>", stream.len())));
    }

    /// A sonda corre de verdade (Node) sobre um DOM minimo: guarda as teclas
    /// confiaveis que nao sao modificadores, instala-se uma so vez por
    /// documento, zera no inicio da tentativa e diz se a barra de procura
    /// apareceu.
    #[test]
    fn the_probe_records_trusted_non_modifier_keys_once_per_document() {
        let program = format!(
            r#"
const listeners = [];
let find = null;
const body = {{ tagName: 'BODY' }};
const frame = {{ tagName: 'IFRAME', blurred: 0, blur() {{ this.blurred += 1; document.activeElement = body; }} }};
globalThis.window = globalThis;
window.addEventListener = (type, fn, capture) => listeners.push({{ type, fn, capture }});
globalThis.location = {{ href: 'http://127.0.0.1:5123/fixture.html' }};
globalThis.document = {{
  readyState: 'complete',
  activeElement: frame,
  hasFocus: () => true,
  getElementById: (id) => (id === 'neuralia-find' ? find : null),
}};
window.__fixtureSeen = {{ down: [{{ key: 'x' }}], press: [] }};
const arm = () => eval({arm});
const pull = () => eval({pull});
const fire = (type, init) => {{
  for (const l of listeners) if (l.type === type) l.fn(Object.assign({{ isTrusted: true, repeat: false }}, init));
}};

const first = arm();
const second = arm();
const out = {{}};
out.listeners = listeners.length;
out.captureOnly = listeners.every((l) => l.capture === true);
out.sameToken = first.token === second.token;
out.blurred = frame.blurred;
out.armFocus = first.focus;
out.ready = first.ready;

arm();
out.fixtureCleared = window.__fixtureSeen.down.length;
fire('keydown', {{ key: 'Control', code: 'ControlLeft', ctrlKey: true }});
fire('keydown', {{ key: 'Shift', code: 'ShiftLeft', ctrlKey: true, shiftKey: true }});
fire('keydown', {{ key: 'N', code: 'KeyN', ctrlKey: true, shiftKey: true }});
fire('keydown', {{ key: 'N', code: 'KeyN', ctrlKey: true, shiftKey: true, repeat: true }});
fire('keydown', {{ key: 'd', code: 'KeyD', ctrlKey: true, isTrusted: false }});
fire('keypress', {{ key: 'n', code: 'KeyN' }});
find = {{ removed: 0, remove() {{ this.removed += 1; find = null; }} }};
const seen = pull();
out.down = seen.down.map((k) => k.code + (k.repeat ? '+r' : '') + (k.ctrl ? '+c' : '') + (k.shift ? '+s' : ''));
out.press = seen.press.length;
out.find = seen.find;
out.token = seen.token === first.token;
out.fixtureDown = seen.fixtureDown.length;

const removed = find;
arm();
out.findRemoved = removed.removed;
out.afterBegin = pull().down.length;

// Outro documento (navegacao): a sonda do anterior nao esta la, e a
// tentativa nao pode passar por falta de quem veja as teclas.
globalThis.window = {{ addEventListener: () => {{}} }};
out.lostToken = pull().token;
console.log(JSON.stringify(out));
"#,
            arm = json_string(PROBE_ARM_SCRIPT),
            pull = json_string(PROBE_PULL_SCRIPT),
        );
        let output = run_node(&program);
        assert_eq!(
            output.trim(),
            concat!(
                r#"{"listeners":2,"captureOnly":true,"sameToken":true,"blurred":1,"armFocus":true,"ready":"complete","#,
                r#""fixtureCleared":0,"down":["KeyN+c+s","KeyN+r+c+s"],"press":1,"find":true,"token":true,"#,
                r#""fixtureDown":0,"findRemoved":1,"afterBegin":0,"lostToken":null}"#
            )
        );
    }

    /// Sem Node nao ha gate: falha, como os outros testes de scripts.
    fn run_node(program: &str) -> String {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = Command::new("node")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("node tem de estar no PATH para o gate da sonda do spike");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(program.as_bytes())
            .expect("escrever o programa");
        let output = child.wait_with_output().expect("node terminou");
        assert!(
            output.status.success(),
            "node falhou: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8")
    }
}
