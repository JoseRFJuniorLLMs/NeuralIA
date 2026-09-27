use super::*;

use neural_core::search::{AnswerReadSelector, ProviderId, is_login_host};
use neural_core::{
    AttemptStatus, ComparisonFact, ResearchSession, ResearchTurn, SnapshotAnswer, TurnOrigin,
    operation_key,
};

use crate::lazy_worker::{JobContext, LazyWorker};

// ===================== o leitor de respostas do Consenso (consensus-reader-turns, plano 2.5) =====================
//
// Modulo de feature (o padrao de `translation.rs`): `UserEvent::Consensus(ConsensusEvent)`,
// o campo `App::consensus` e o braco `consensus_event` no event loop. E o
// que substitui o caminho da caixa de texto do `research:compare`: em vez
// de a pagina EMPURRAR a resposta (o `research-answer` do IPC, que so leva
// 1 800 caracteres e chega quando a pagina quiser), o lado nativo LE a
// resposta de cada coluna quando o dono a pede, pelo `page_eval`:
//
// - `ANSWER_READ_SCRIPT` (§7, script so-leitura; o sim do dono para a 2.5)
//   corre SO em `comp.views[col]` -- nunca na fonte ao lado, normal ou
//   privada (`consensus_readable`) -- com a configuracao como ARGUMENTO
//   (`answer_read_config`: o seletor do registo dos provedores, o botao
//   de «a escrever», os marcadores de citacao U+E000 n U+E001, o tecto de
//   24 000 caracteres, de 60 ligacoes e de 2 048 bytes por ligacao). Devolve
//   `{v, ok, host, busy, cut, text, links}`: o Markdown da ULTIMA mensagem
//   do assistente, com cada ligacao trocada por um marcador que aponta
//   para `links`. Um U+E000 ou U+E001 que a pagina traga no texto vira
//   U+FFFD (os marcadores sao so os do script) e uma ligacao acima do tecto
//   vai vazia. Nao escuta nada, nao publica nada (nem `postMessage`):
//   o unico caminho de volta e o callback do `evaluate_script`.
// - `parse_answer_read` le a resposta como dado nao confiavel: tecto de
//   512 KiB ANTES do serde, as sete chaves exatas e nenhuma outra, `v` = 1,
//   o `host` igual ao da pagina da coluna e cada marcador a apontar para
//   uma ligacao que existe; so entram em `links` as ligacoes http(s) com
//   host e dentro do tecto -- cada uma que nao vale cai SOZINHA, com os
//   seus marcadores, e fica contada (`dropped_links`), sem levar a
//   leitura inteira com ela. E
//   antes de tudo `column_answer_read` exige que a pagina da coluna SEJA
//   do provedor dela pelas regras de host do registo
//   (`ProviderId::from_url`: o Modo IA so com `udm=50`) e nao uma pagina de
//   login (`is_login_host`): uma coluna que foi parar ao login da Google,
//   com um `<main>` que o seletor do Modo IA apanha, nao passa por resposta.
// - A sondagem: a cada 1,5 s (`Timers`) le-se outra vez cada coluna ate a
//   resposta ASSENTAR (duas leituras iguais, sem o botao de parar), ate a
//   coluna NAVEGAR para outra pergunta (a geracao de navegacao subiu:
//   `OtherQuestion`) ou ate 120 s -- ai o que se leu por ultimo fica como
//   `MaybeIncomplete`. Uma leitura que chega tarde, de outra pagina ou de
//   um run que ja acabou cai (`PageReads`, e o token que ja nao e de
//   ninguem).
// - Uma coluna traduzida (`TranslationState::column_translated`: desde o
//   `TRANSLATE_APPLY` ate o `TRANSLATE_RESTORE`) NAO se le: espera-se o
//   original voltar; se nao voltar, a coluna sai como «traduzida — não
//   comparada» (`Translated`) e nunca entra na comparacao (critica C9).
// - Um provedor sem seletor no registo sai como «não lida» (`Unreadable`).
// - No fim, o turno da sessao (`ResearchSession::begin_turn`, os
//   invariantes do rascunho da Chat Surface, numeracao pendente: OQ11)
//   recebe uma TENTATIVA por coluna que o turno perguntou (nunca por cima
//   da anterior; `record_consensus_run`, e so na sessao e no turno do run),
//   o texto lido vira um item com `links`, a
//   leitura fica como um `ConsensusSnapshot` (no maximo 8 por sessao) e a
//   sessao grava-se pelo `PrivacyGuard::save_session` (a loja `memory/`,
//   `Automatic`: no modo privado nao grava). A comparacao das entidades,
//   numeros e datas corre na thread `neural-consensus` (um `LazyWorker`:
//   nasce no primeiro pedido, nunca no `App::new`) e volta ao event loop
//   como `Compared`, que a mostra.
//
// Os turnos abrem-se onde a pergunta parte: `compare` (as tres IAs),
// `ask_other_columns` (a pergunta escrita numa coluna segue as outras; a
// coluna de origem responde-a tambem) e o `LoadProvider` da palette (uma so
// coluna) -- `begin_question_turn`, com a chave de operacao de
// `neural_core::operation_key` ANCORADA no turno mais recente
// (`anchored_operation_key`): repetir a pergunta enquanto ela ainda e a
// ultima devolve o mesmo turno; a mesma pergunta depois de outra e outro
// turno. O Consenso le sempre o turno mais recente e so as colunas que ele
// perguntou (`consensus_hosts`); um turno ou uma sessao novos acabam o run
// que ainda lia o anterior (`ConsensusState::supersede`): o que ja assentou
// fica, o resto sai como «outra pergunta — não comparada».

/// O nome da thread do Consenso.
pub(in crate::windows_app) const CONSENSUS_WORKER_NAME: &str = "neural-consensus";
/// O tecto do texto lido de uma coluna (caracteres), no script e no parser.
pub(in crate::windows_app) const ANSWER_READ_MAX_CHARS: usize = 24_000;
/// Quantas ligacoes uma leitura leva; as seguintes ficam so como texto.
pub(in crate::windows_app) const ANSWER_READ_MAX_LINKS: usize = 60;
/// O tecto do JSON cru que o WebView2 devolve, conferido ANTES do serde.
pub(in crate::windows_app) const ANSWER_READ_MAX_RAW_BYTES: usize = 512 * 1024;
/// O tecto de uma ligacao citada.
pub(in crate::windows_app) const ANSWER_LINK_MAX_LEN: usize = 2_048;
/// Quanto se espera por uma leitura do script.
pub(in crate::windows_app) const ANSWER_READ_DEADLINE: Duration = Duration::from_secs(8);
/// De quanto em quanto se volta a ler cada coluna.
pub(in crate::windows_app) const CONSENSUS_POLL_INTERVAL: Duration = Duration::from_millis(1_500);
/// Quanto se espera, no maximo, por uma resposta assentar.
pub(in crate::windows_app) const CONSENSUS_MAX_WAIT: Duration = Duration::from_secs(120);
/// Leituras seguidas que nao valem (JSON invalido, host errado) ate a
/// coluna sair como `Failed`.
pub(in crate::windows_app) const CONSENSUS_MAX_FAILURES: u8 = 3;
/// Os marcadores de citacao: `texto\u{E000}3\u{E001}` cita `links[3]`.
pub(in crate::windows_app) const CITATION_OPEN: char = '\u{E000}';
pub(in crate::windows_app) const CITATION_CLOSE: char = '\u{E001}';

pub(in crate::windows_app) const CONSENSUS_TITLE: &str = "NeuralIA — Consenso";
pub(in crate::windows_app) const CONSENSUS_NO_SESSION: &str =
    "Nenhuma sessão de pesquisa está ativa.";
pub(in crate::windows_app) const CONSENSUS_NO_COLUMNS: &str =
    "Abra a comparação (as três colunas) para ler as respostas.";
pub(in crate::windows_app) const CONSENSUS_READING: &str = "Lendo as respostas das colunas…";
/// «traduzida — não comparada»: a coluna estava traduzida (critica C9).
pub(in crate::windows_app) const CONSENSUS_TRANSLATED: &str = "traduzida — não comparada";
/// «não lida»: o provedor nao tem seletor de leitura.
pub(in crate::windows_app) const CONSENSUS_UNREADABLE: &str = "não lida";
pub(in crate::windows_app) const CONSENSUS_READ: &str = "lida";
pub(in crate::windows_app) const CONSENSUS_MAYBE_INCOMPLETE: &str = "talvez incompleta";
pub(in crate::windows_app) const CONSENSUS_OTHER_QUESTION: &str = "outra pergunta — não comparada";
pub(in crate::windows_app) const CONSENSUS_FAILED: &str = "sem leitura";

// ===================== o script (page_eval, §7) =====================
//
// Uma funcao que recebe a configuracao como argumento (`ScriptArg::Json`).
// Le a ULTIMA ocorrencia do seletor do provedor e devolve o Markdown dela:
// titulos, paragrafos, listas, negrito, italico, codigo, tabelas e as
// ligacoes http(s) como marcadores de citacao. Nao guarda nada na pagina,
// nao muda o DOM, nao escuta, nao agenda, nao publica: o gate de so-leitura
// (`page_eval_scripts_are_read_only`) le o texto inteiro, por isso os nomes
// e comentarios aqui evitam as palavras da lista dele.
pub(in crate::windows_app) const ANSWER_READ_SCRIPT: &str = r#"(function (config) {
  var out = { v: 1, ok: false, host: String(location.hostname || '').toLowerCase(), busy: false, cut: false, text: '', links: [] };
  var cfg = config && typeof config === 'object' ? config : {};
  var max = Math.floor(Number(cfg.max));
  if (!(max > 0)) max = 24000;
  var maxLinks = Math.floor(Number(cfg.maxLinks));
  if (!(maxLinks >= 0)) maxLinks = 60;
  var maxLink = Math.floor(Number(cfg.maxLink));
  if (!(maxLink > 0)) maxLink = 2048;
  var markers = cfg.markers && typeof cfg.markers === 'object' ? cfg.markers : [];
  var mark0 = typeof markers[0] === 'string' && markers[0] ? markers[0] : '';
  var mark1 = typeof markers[1] === 'string' && markers[1] ? markers[1] : '';
  var selector = typeof cfg.selector === 'string' ? cfg.selector : '';
  var busySelector = typeof cfg.busy === 'string' ? cfg.busy : '';
  if (!selector) return out;
  var found;
  try { found = document.querySelectorAll(selector); } catch (_) { return out; }
  if (!found || !found.length) return out;
  var last = found[found.length - 1];
  if (busySelector) {
    try { out.busy = !!document.querySelector(busySelector); } catch (_) { out.busy = false; }
  }
  var SKIP = { script: 1, style: 1, noscript: 1, template: 1, svg: 1, math: 1, canvas: 1, iframe: 1,
    object: 1, button: 1, textarea: 1, input: 1, select: 1, option: 1, img: 1, video: 1, audio: 1 };
  var BLOCK = { p: 1, div: 1, section: 1, article: 1, header: 1, footer: 1, main: 1, aside: 1,
    ul: 1, ol: 1, blockquote: 1, table: 1, tbody: 1, thead: 1, figure: 1, details: 1, summary: 1, hr: 1 };
  var links = [];
  var parts = [];
  function tagOf(el) { return String(el.localName || el.nodeName || '').toLowerCase(); }
  function attr(el, key) {
    return typeof el.getAttribute === 'function' ? el.getAttribute(key) : null;
  }
  function hidden(el) {
    var id = String(attr(el, 'id') || '');
    if (id === 'neuralia-comp-controls' || id === 'neuralia-palette') return true;
    return String(attr(el, 'aria-hidden') || '').toLowerCase() === 'true';
  }
  // Os bytes UTF-8 de uma ligacao, contados como o parser os conta (o par
  // de substitutos vale 1 + 3), ate passar do tecto.
  function overCap(value) {
    if (value.length > maxLink) return true;
    var bytes = 0;
    for (var j = 0; j < value.length; j++) {
      var code = value.charCodeAt(j);
      bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code >= 0xD800 && code <= 0xDBFF ? 1 : 3;
      if (bytes > maxLink) return true;
    }
    return false;
  }
  function walk(node, pre, depth) {
    if (!node || depth > 64) return;
    if (node.nodeType === 3) {
      var value = String(node.nodeValue || '');
      // Um marcador que a pagina traga no texto vira U+FFFD: os marcadores
      // do texto devolvido sao so os que este script escreve.
      if (mark0) value = value.split(mark0).join('\uFFFD');
      if (mark1) value = value.split(mark1).join('\uFFFD');
      if (!pre) {
        value = value.replace(/[\t\r\n ]+/g, ' ');
        var previous = parts.length ? parts[parts.length - 1] : '';
        if (!parts.length || /\n$/.test(previous)) value = value.replace(/^ +/, '');
      }
      if (value) parts.push(value);
      return;
    }
    if (node.nodeType !== 1) return;
    var tag = tagOf(node);
    if (SKIP[tag] === 1 || hidden(node)) return;
    var kids = node.childNodes || [];
    var i;
    if (tag === 'br') { parts.push('\n'); return; }
    if (tag === 'hr') { parts.push('\n\n---\n\n'); return; }
    if (tag === 'h1' || tag === 'h2' || tag === 'h3' || tag === 'h4' || tag === 'h5' || tag === 'h6') {
      parts.push('\n\n' + '######'.slice(0, Number(tag.charAt(1))) + ' ');
      for (i = 0; i < kids.length; i++) walk(kids[i], false, depth + 1);
      parts.push('\n\n');
      return;
    }
    if (tag === 'pre') {
      parts.push('\n\n```\n');
      for (i = 0; i < kids.length; i++) walk(kids[i], true, depth + 1);
      parts.push('\n```\n\n');
      return;
    }
    if (tag === 'code' && !pre) {
      parts.push('`');
      for (i = 0; i < kids.length; i++) walk(kids[i], false, depth + 1);
      parts.push('`');
      return;
    }
    if (tag === 'strong' || tag === 'b') {
      parts.push('**');
      for (i = 0; i < kids.length; i++) walk(kids[i], pre, depth + 1);
      parts.push('**');
      return;
    }
    if (tag === 'em' || tag === 'i') {
      parts.push('*');
      for (i = 0; i < kids.length; i++) walk(kids[i], pre, depth + 1);
      parts.push('*');
      return;
    }
    if (tag === 'li') {
      var parent = node.parentNode;
      var ordered = parent && tagOf(parent) === 'ol';
      var position = 1;
      if (ordered) {
        var siblings = parent.childNodes || [];
        for (i = 0; i < siblings.length; i++) {
          if (siblings[i] === node) break;
          if (siblings[i].nodeType === 1 && tagOf(siblings[i]) === 'li') position++;
        }
      }
      parts.push('\n' + (ordered ? position + '. ' : '- '));
      for (i = 0; i < kids.length; i++) walk(kids[i], pre, depth + 1);
      return;
    }
    if (tag === 'tr') {
      parts.push('\n| ');
      for (i = 0; i < kids.length; i++) {
        var cell = kids[i];
        if (cell.nodeType !== 1) continue;
        var cellTag = tagOf(cell);
        if (cellTag !== 'td' && cellTag !== 'th') continue;
        var cellKids = cell.childNodes || [];
        for (var k = 0; k < cellKids.length; k++) walk(cellKids[k], pre, depth + 1);
        parts.push(' | ');
      }
      parts.push('\n');
      return;
    }
    if (tag === 'a') {
      var href = String(attr(node, 'href') || '');
      var cited = /^https?:\/\//i.test(href) && links.length < maxLinks;
      for (i = 0; i < kids.length; i++) walk(kids[i], pre, depth + 1);
      if (cited) {
        parts.push(mark0 + links.length + mark1);
        // Acima do tecto a ligacao vai vazia: o parser deixa-a cair sozinha
        // e conta-a, e o JSON devolvido nunca a carrega.
        links.push(overCap(href) ? '' : href);
      }
      return;
    }
    if (BLOCK[tag] === 1) parts.push('\n\n');
    for (i = 0; i < kids.length; i++) walk(kids[i], pre, depth + 1);
    if (BLOCK[tag] === 1) parts.push('\n\n');
  }
  walk(last, false, 0);
  var text = parts.join('')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .replace(/^\s+|\s+$/g, '');
  if (text.length > max) {
    text = text.slice(0, max);
    var lastMark = text.lastIndexOf(mark0);
    if (lastMark >= 0 && text.indexOf(mark1, lastMark) < 0) text = text.slice(0, lastMark);
    var tail = text.charCodeAt(text.length - 1);
    if (tail >= 0xD800 && tail <= 0xDBFF) text = text.slice(0, text.length - 1);
    out.cut = true;
  }
  var used = 0;
  var scan = 0;
  while (true) {
    var at = text.indexOf(mark0, scan);
    if (at < 0) break;
    var end = text.indexOf(mark1, at);
    if (end < 0) break;
    var index = Number(text.slice(at + mark0.length, end));
    if (index + 1 > used) used = index + 1;
    scan = end + mark1.length;
  }
  out.links = links.slice(0, used);
  out.text = text;
  out.ok = true;
  return out;
})"#;

/// A configuracao que o script recebe como argumento: o seletor do
/// provedor, o botao de «a escrever», os marcadores e os tectos.
pub(in crate::windows_app) fn answer_read_config(
    selector: AnswerReadSelector,
) -> serde_json::Value {
    serde_json::json!({
        "selector": selector.answer,
        "busy": selector.busy,
        "markers": [CITATION_OPEN.to_string(), CITATION_CLOSE.to_string()],
        "max": ANSWER_READ_MAX_CHARS,
        "maxLinks": ANSWER_READ_MAX_LINKS,
        "maxLink": ANSWER_LINK_MAX_LEN,
    })
}

/// O pedido ao `page_eval`: o script registado, o tecto cru e o prazo.
pub(in crate::windows_app) fn answer_read_spec() -> PageEvalSpec {
    PageEvalSpec {
        script: &ANSWER_READ,
        max_raw_bytes: ANSWER_READ_MAX_RAW_BYTES,
        deadline: ANSWER_READ_DEADLINE,
    }
}

// ===================== a resposta, lida como dado nao confiavel =====================

/// O que o script devolveu, ja validado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct AnswerRead {
    /// Havia uma mensagem do assistente na pagina.
    pub(in crate::windows_app) ok: bool,
    /// O `location.hostname` da pagina, em minusculas.
    pub(in crate::windows_app) host: String,
    /// O provedor ainda escreve (o botao de parar esta na pagina).
    pub(in crate::windows_app) busy: bool,
    /// O texto foi cortado no tecto.
    pub(in crate::windows_app) cut: bool,
    /// O Markdown da ultima mensagem, com os marcadores de citacao.
    pub(in crate::windows_app) text: String,
    /// As ligacoes citadas, na ordem dos marcadores.
    pub(in crate::windows_app) links: Vec<String>,
    /// Quantas ligacoes a leitura trouxe que nao valiam (nao eram texto,
    /// nem http(s) com host, ou passavam o tecto; o script manda vazia a
    /// que passa o tecto): cada uma caiu sozinha, com os seus marcadores.
    pub(in crate::windows_app) dropped_links: usize,
}

/// Porque uma resposta do script foi recusada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum AnswerReadError {
    /// Acima do tecto cru, antes do serde.
    OverCap {
        bytes: usize,
    },
    NotJson,
    NotAnObject,
    /// As chaves nao sao exatamente as sete esperadas.
    Keys(Vec<String>),
    /// `v` nao e 1.
    Version,
    /// Um campo com o tipo errado.
    Type(&'static str),
    /// O `host` nao e o da pagina da coluna.
    HostMismatch {
        expected: String,
        got: String,
    },
    /// A pagina da coluna nao e do provedor dela (outro site, o Google sem
    /// o Modo IA) ou e uma pagina de login.
    NotProviderPage {
        provider: &'static str,
        host: String,
    },
    TooLong {
        chars: usize,
    },
    TooManyLinks {
        count: usize,
    },
    /// Um marcador de citacao mal formado, solto ou a apontar para fora de
    /// `links`: o script troca os da pagina por U+FFFD, por isso um destes
    /// e o proprio script que nao correu como embarca.
    Citation {
        at: usize,
    },
}

impl AnswerReadError {
    pub(in crate::windows_app) fn describe(&self) -> String {
        match self {
            Self::OverCap { bytes } => format!("{bytes} bytes acima do tecto"),
            Self::NotJson => "não é JSON".to_string(),
            Self::NotAnObject => "não é um objeto".to_string(),
            Self::Keys(keys) => format!("chaves {keys:?}"),
            Self::Version => "versão desconhecida".to_string(),
            Self::Type(field) => format!("tipo errado em {field}"),
            Self::HostMismatch { expected, got } => {
                format!("host {got:?} em vez de {expected:?}")
            }
            Self::NotProviderPage { provider, host } => {
                format!("a página {host:?} não é do {provider}")
            }
            Self::TooLong { chars } => format!("{chars} caracteres acima do tecto"),
            Self::TooManyLinks { count } => format!("{count} ligações acima do tecto"),
            Self::Citation { at } => format!("marcador de citação inválido em {at}"),
        }
    }
}

const ANSWER_READ_KEYS: [&str; 7] = ["busy", "cut", "host", "links", "ok", "text", "v"];

/// Le a resposta do `ANSWER_READ_SCRIPT` como dado nao confiavel: o tecto
/// cru antes do serde, as chaves exatas, os tipos, o `host` igual ao da
/// pagina da coluna (`expected_host`, em minusculas), os tectos do texto e
/// do numero de ligacoes, e cada marcador de citacao a apontar para uma
/// ligacao que existe. Uma ligacao que nao vale (`validate_answer_link`)
/// cai SOZINHA -- nunca entra em `links`, os marcadores dela saem do texto,
/// os das outras renumeram-se e fica contada em `dropped_links` --: uma
/// ligacao estranha nao deixa a coluna inteira sem leitura.
pub(in crate::windows_app) fn parse_answer_read(
    raw: &str,
    expected_host: &str,
) -> Result<AnswerRead, AnswerReadError> {
    if raw.len() > ANSWER_READ_MAX_RAW_BYTES {
        return Err(AnswerReadError::OverCap { bytes: raw.len() });
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| AnswerReadError::NotJson)?;
    let object = value.as_object().ok_or(AnswerReadError::NotAnObject)?;
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    if keys != ANSWER_READ_KEYS {
        return Err(AnswerReadError::Keys(
            keys.into_iter().map(str::to_string).collect(),
        ));
    }
    if object["v"].as_u64() != Some(1) {
        return Err(AnswerReadError::Version);
    }
    let flag = |name: &'static str| object[name].as_bool().ok_or(AnswerReadError::Type(name));
    let ok = flag("ok")?;
    let busy = flag("busy")?;
    let cut = flag("cut")?;
    let host = object["host"]
        .as_str()
        .ok_or(AnswerReadError::Type("host"))?
        .to_ascii_lowercase();
    let text = object["text"]
        .as_str()
        .ok_or(AnswerReadError::Type("text"))?
        .to_string();
    let raw_links = object["links"]
        .as_array()
        .ok_or(AnswerReadError::Type("links"))?;
    let expected = expected_host.trim().to_ascii_lowercase();
    if expected.is_empty() || host != expected {
        return Err(AnswerReadError::HostMismatch {
            expected,
            got: host,
        });
    }
    let chars = text.chars().count();
    if chars > ANSWER_READ_MAX_CHARS {
        return Err(AnswerReadError::TooLong { chars });
    }
    if raw_links.len() > ANSWER_READ_MAX_LINKS {
        return Err(AnswerReadError::TooManyLinks {
            count: raw_links.len(),
        });
    }
    // `kept[n]`: o indice novo de `links[n]` da leitura, ou `None` se caiu.
    let mut links = Vec::with_capacity(raw_links.len());
    let mut kept = Vec::with_capacity(raw_links.len());
    for link in raw_links {
        match link
            .as_str()
            .ok_or("não é texto")
            .and_then(validate_answer_link)
        {
            Ok(link) => {
                kept.push(Some(links.len()));
                links.push(link);
            }
            Err(_) => kept.push(None),
        }
    }
    let text = cite_kept_links(&text, &kept)?;
    Ok(AnswerRead {
        ok,
        host,
        busy,
        cut,
        text,
        dropped_links: raw_links.len() - links.len(),
        links,
    })
}

/// Uma ligacao citada: http(s), com host, dentro do tecto de tamanho.
pub(in crate::windows_app) fn validate_answer_link(link: &str) -> Result<String, &'static str> {
    if link.len() > ANSWER_LINK_MAX_LEN {
        return Err("grande demais");
    }
    let url = Url::parse(link).map_err(|_| "não é um URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("só http(s)");
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("sem host");
    }
    Ok(url.to_string())
}

/// Cada `U+E000 n U+E001` do texto aponta para `links[n]` da leitura; um
/// marcador sem fecho, sem numero (1 a 3 algarismos), a apontar para fora ou
/// um fecho solto recusa a leitura inteira -- o script troca por U+FFFD os
/// que a pagina traga no texto, por isso os marcadores sao so os dele. Devolve
/// o texto com o marcador de cada ligacao que caiu (`kept[n]` = `None`)
/// tirado e os outros a apontar para o indice novo (`kept[n]`).
fn cite_kept_links(text: &str, kept: &[Option<usize>]) -> Result<String, AnswerReadError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut offset = 0;
    while let Some(at) = rest.find(CITATION_OPEN) {
        // Um fecho solto antes desta abertura.
        if let Some(stray) = rest[..at].find(CITATION_CLOSE) {
            return Err(AnswerReadError::Citation { at: offset + stray });
        }
        let after = &rest[at + CITATION_OPEN.len_utf8()..];
        let Some(end) = after.find(CITATION_CLOSE) else {
            return Err(AnswerReadError::Citation { at: offset + at });
        };
        let digits = &after[..end];
        let slot = Some(digits)
            .filter(|digits| {
                (1..=3).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
            })
            .and_then(|digits| digits.parse::<usize>().ok())
            .and_then(|index| kept.get(index).copied());
        let Some(slot) = slot else {
            return Err(AnswerReadError::Citation { at: offset + at });
        };
        out.push_str(&rest[..at]);
        if let Some(index) = slot {
            out.push(CITATION_OPEN);
            out.push_str(&index.to_string());
            out.push(CITATION_CLOSE);
        }
        let consumed = at + CITATION_OPEN.len_utf8() + end + CITATION_CLOSE.len_utf8();
        offset += consumed;
        rest = &rest[consumed..];
    }
    // Um fecho solto, sem abertura antes dele.
    if let Some(stray) = rest.find(CITATION_CLOSE) {
        return Err(AnswerReadError::Citation { at: offset + stray });
    }
    out.push_str(rest);
    Ok(out)
}

/// O host que a leitura da coluna do `provider` tem de trazer: o da pagina
/// da coluna (`page_url`, o mesmo que o `PageReads` conferiu a chegada), e
/// SO quando essa pagina e do provedor pelas regras de host do registo
/// (`ProviderId::from_url`: http(s), o host exato ou subdominio quando a
/// regra os aceita, o Google so com o primeiro `udm` igual a `50`) e nao e
/// uma pagina de login. Comparar a resposta com o host da propria pagina
/// nao prova nada -- o script le `location.hostname` dessa mesma pagina --;
/// e o provedor da coluna que diz se a pagina e uma resposta.
pub(in crate::windows_app) fn provider_page_host(
    provider: ProviderId,
    page_url: Option<&str>,
) -> Result<String, AnswerReadError> {
    let url = page_url.and_then(|page| Url::parse(page).ok());
    let host = url
        .as_ref()
        .and_then(Url::host_str)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let on_provider_page = url
        .as_ref()
        .is_some_and(|url| ProviderId::from_url(url) == Some(provider));
    if !on_provider_page || host.is_empty() || is_login_host(&host) {
        return Err(AnswerReadError::NotProviderPage {
            provider: provider.display_name(),
            host,
        });
    }
    Ok(host)
}

/// A decisao de `consensus_page_event` sobre uma leitura que chegou da
/// coluna do `provider`: a pagina tem de ser do provedor
/// (`provider_page_host`) e a resposta passa em `parse_answer_read` contra
/// o host dela.
pub(in crate::windows_app) fn column_answer_read(
    provider: ProviderId,
    page_url: Option<&str>,
    raw: &str,
) -> Result<AnswerRead, AnswerReadError> {
    let expected = provider_page_host(provider, page_url)?;
    parse_answer_read(raw, &expected)
}

// ===================== o que se le: so as colunas =====================

/// So uma coluna do comparador se le: nunca a fonte ao lado (normal ou
/// privada), nunca a Web completa, nunca um painel.
pub(in crate::windows_app) fn consensus_readable(host: WebViewHost) -> bool {
    matches!(host, WebViewHost::Column(index) if index < COMPARATOR_COLUMNS)
}

/// O provedor da coluna `index` (os slots padrao do registo).
pub(in crate::windows_app) fn column_provider(index: usize) -> Option<ProviderId> {
    ProviderId::default_slots().get(index).copied()
}

// ===================== a sondagem =====================

/// Como uma coluna acabou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum ColumnOutcome {
    /// A resposta assentou.
    Read(AnswerRead),
    /// Os 120 s passaram: o que se leu por ultimo, se houve.
    MaybeIncomplete(Option<AnswerRead>),
    /// A coluna estava traduzida: «traduzida — não comparada».
    Translated,
    /// O provedor nao tem seletor: «não lida».
    Unreadable,
    /// A coluna navegou para outra pergunta.
    OtherQuestion,
    /// A leitura nao valeu (a razao da ultima).
    Failed(String),
}

impl ColumnOutcome {
    pub(in crate::windows_app) fn status(&self) -> AttemptStatus {
        match self {
            Self::Read(_) => AttemptStatus::Read,
            Self::MaybeIncomplete(_) => AttemptStatus::MaybeIncomplete,
            Self::Translated => AttemptStatus::Translated,
            Self::Unreadable => AttemptStatus::Unreadable,
            Self::OtherQuestion => AttemptStatus::OtherQuestion,
            Self::Failed(_) => AttemptStatus::Failed,
        }
    }

    /// O texto que se leu, quando houve.
    pub(in crate::windows_app) fn read(&self) -> Option<&AnswerRead> {
        match self {
            Self::Read(read) => Some(read),
            Self::MaybeIncomplete(read) => read.as_ref(),
            _ => None,
        }
    }

    /// O rotulo do relatorio.
    pub(in crate::windows_app) fn label(&self) -> &'static str {
        match self {
            Self::Read(_) => CONSENSUS_READ,
            Self::MaybeIncomplete(_) => CONSENSUS_MAYBE_INCOMPLETE,
            Self::Translated => CONSENSUS_TRANSLATED,
            Self::Unreadable => CONSENSUS_UNREADABLE,
            Self::OtherQuestion => CONSENSUS_OTHER_QUESTION,
            Self::Failed(_) => CONSENSUS_FAILED,
        }
    }
}

/// O rotulo de um estado gravado (o mesmo de `ColumnOutcome::label`).
pub(in crate::windows_app) fn attempt_label(status: AttemptStatus) -> &'static str {
    match status {
        AttemptStatus::Read => CONSENSUS_READ,
        AttemptStatus::MaybeIncomplete => CONSENSUS_MAYBE_INCOMPLETE,
        AttemptStatus::Translated => CONSENSUS_TRANSLATED,
        AttemptStatus::Unreadable => CONSENSUS_UNREADABLE,
        AttemptStatus::OtherQuestion => CONSENSUS_OTHER_QUESTION,
        AttemptStatus::Failed => CONSENSUS_FAILED,
    }
}

/// Uma coluna dentro de um run.
#[derive(Debug)]
pub(in crate::windows_app) struct ColumnRead {
    pub(in crate::windows_app) host: WebViewHost,
    pub(in crate::windows_app) provider: ProviderId,
    /// A geracao de navegacao da coluna quando o run comecou.
    pub(in crate::windows_app) generation: u64,
    /// A ultima leitura valida (a resposta assenta quando a seguinte e igual).
    pub(in crate::windows_app) last: Option<AnswerRead>,
    /// A leitura em voo, se ha uma.
    pub(in crate::windows_app) pending: Option<PageReadToken>,
    /// Leituras seguidas que nao valeram.
    pub(in crate::windows_app) failures: u8,
    pub(in crate::windows_app) outcome: Option<ColumnOutcome>,
}

impl ColumnRead {
    /// Uma leitura valida chegou: a resposta assentou se ha uma mensagem,
    /// o provedor ja nao escreve e o texto e o mesmo da leitura anterior.
    pub(in crate::windows_app) fn observe(&mut self, read: AnswerRead) -> Option<&ColumnOutcome> {
        self.pending = None;
        self.failures = 0;
        if self.outcome.is_some() {
            return self.outcome.as_ref();
        }
        let settled = read.ok
            && !read.busy
            && !read.text.is_empty()
            && self
                .last
                .as_ref()
                .is_some_and(|last| last.text == read.text && last.links == read.links);
        if settled {
            self.outcome = Some(ColumnOutcome::Read(read));
        } else if read.ok {
            self.last = Some(read);
        }
        self.outcome.as_ref()
    }

    /// Uma leitura que nao valeu (JSON, host): a coluna cai ao fim de
    /// `CONSENSUS_MAX_FAILURES` seguidas.
    pub(in crate::windows_app) fn refuse(&mut self, reason: String) -> Option<&ColumnOutcome> {
        self.pending = None;
        self.failures = self.failures.saturating_add(1);
        if self.outcome.is_none() && self.failures >= CONSENSUS_MAX_FAILURES {
            self.outcome = Some(ColumnOutcome::Failed(reason));
        }
        self.outcome.as_ref()
    }

    fn index(&self) -> Option<usize> {
        match self.host {
            WebViewHost::Column(index) => Some(index),
            _ => None,
        }
    }
}

/// O que uma sonda faz a uma coluna.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum PollAction {
    /// Ler a pagina outra vez.
    Read,
    /// Esperar (ja acabou, ha uma leitura em voo, ou esta traduzida).
    Wait,
    Finish(ColumnOutcome),
}

/// A decisao de uma sonda para uma coluna: `translated` e o
/// `column_translated` da Traducao AGORA, `navigated` diz se a geracao de
/// navegacao da coluna subiu desde o inicio do run, `elapsed` conta desde
/// o inicio do run. Uma coluna traduzida NUNCA se le (critica C9): espera o
/// original voltar, e no prazo sai como `Translated`.
pub(in crate::windows_app) fn decide_column_poll(
    column: &ColumnRead,
    translated: bool,
    navigated: bool,
    elapsed: Duration,
) -> PollAction {
    if column.outcome.is_some() {
        return PollAction::Wait;
    }
    if column.provider.is_unreadable() {
        return PollAction::Finish(ColumnOutcome::Unreadable);
    }
    if navigated {
        return PollAction::Finish(ColumnOutcome::OtherQuestion);
    }
    if elapsed >= CONSENSUS_MAX_WAIT {
        return PollAction::Finish(if translated {
            ColumnOutcome::Translated
        } else {
            ColumnOutcome::MaybeIncomplete(column.last.clone())
        });
    }
    if translated || column.pending.is_some() {
        return PollAction::Wait;
    }
    PollAction::Read
}

/// Uma leitura das colunas de um turno.
#[derive(Debug)]
pub(in crate::windows_app) struct ConsensusRun {
    pub(in crate::windows_app) id: u64,
    /// A sessao (`ResearchSession::id`) do turno: o run so grava nela.
    pub(in crate::windows_app) session: String,
    pub(in crate::windows_app) turn: u32,
    pub(in crate::windows_app) started: Instant,
    pub(in crate::windows_app) columns: Vec<ColumnRead>,
}

impl ConsensusRun {
    /// Um run sobre `hosts` (cada um com a geracao de navegacao de agora):
    /// so as colunas entram (`consensus_readable`); o resto -- a fonte ao
    /// lado, a privada -- fica de fora, por muito que venha na lista.
    pub(in crate::windows_app) fn begin(
        id: u64,
        session: &str,
        turn: u32,
        started: Instant,
        hosts: impl IntoIterator<Item = (WebViewHost, u64)>,
    ) -> Self {
        let columns = hosts
            .into_iter()
            .filter(|(host, _)| consensus_readable(*host))
            .filter_map(|(host, generation)| {
                let WebViewHost::Column(index) = host else {
                    return None;
                };
                Some(ColumnRead {
                    host,
                    provider: column_provider(index)?,
                    generation,
                    last: None,
                    pending: None,
                    failures: 0,
                    outcome: None,
                })
            })
            .collect();
        Self {
            id,
            session: session.to_string(),
            turn,
            started,
            columns,
        }
    }

    pub(in crate::windows_app) fn finished(&self) -> bool {
        self.columns.iter().all(|column| column.outcome.is_some())
    }

    /// A coluna dona da leitura `token`, se ainda e deste run.
    pub(in crate::windows_app) fn column_by_token(
        &mut self,
        token: PageReadToken,
    ) -> Option<&mut ColumnRead> {
        self.columns
            .iter_mut()
            .find(|column| column.pending == Some(token))
    }

    /// Uma pergunta nova chegou antes de o run acabar: cada coluna que
    /// ainda nao assentou sai como «outra pergunta — não comparada» -- sem
    /// texto, nem o da ultima leitura, que pode ja ser da pergunta nova --,
    /// e as que ja assentaram ficam como estavam.
    pub(in crate::windows_app) fn interrupt(&mut self) {
        for column in &mut self.columns {
            column.pending = None;
            if column.outcome.is_none() {
                column.outcome = Some(ColumnOutcome::OtherQuestion);
            }
        }
    }

    /// As colunas que so esperam o original voltar, quando todas as outras
    /// ja acabaram, saem como «traduzida — não comparada»: o relatorio nao
    /// fica preso 120 s por uma traducao que o dono nao vai desfazer.
    pub(in crate::windows_app) fn settle_translated_stragglers(
        &mut self,
        translated: impl Fn(usize) -> bool,
    ) {
        let only_translated = self
            .columns
            .iter()
            .all(|column| column.outcome.is_some() || column.index().is_some_and(&translated));
        if !only_translated {
            return;
        }
        for column in &mut self.columns {
            if column.outcome.is_none() {
                column.outcome = Some(ColumnOutcome::Translated);
            }
        }
    }
}

// ===================== o relatorio =====================

/// O que a thread `neural-consensus` devolve: as leituras e a comparacao.
#[derive(Debug, Clone)]
pub(in crate::windows_app) struct ConsensusReport {
    pub(in crate::windows_app) turn: u32,
    pub(in crate::windows_app) answers: Vec<SnapshotAnswer>,
    pub(in crate::windows_app) facts: Vec<ComparisonFact>,
}

/// O texto do relatorio: uma linha por provedor com o estado, o tamanho e
/// as ligacoes, e depois as entidades, numeros e datas de cada resposta
/// lida (`ResearchSession::comparison`).
pub(in crate::windows_app) fn consensus_report_text(report: &ConsensusReport) -> String {
    let mut out = format!("Turno {}\r\n", report.turn);
    for answer in &report.answers {
        let label = attempt_label(answer.status);
        if answer.text.is_empty() {
            out.push_str(&format!("{}: {label}\r\n", answer.provider));
        } else {
            out.push_str(&format!(
                "{}: {label} ({} caracteres, {} ligação(ões){})\r\n",
                answer.provider,
                answer.text.chars().count(),
                answer.links.len(),
                if answer.cut { ", cortada" } else { "" }
            ));
        }
    }
    if report.facts.is_empty() {
        out.push_str("\r\nNenhuma resposta lida para comparar.");
        return out;
    }
    for fact in &report.facts {
        out.push_str(&format!(
            "\r\n{}\r\nEntidades: {}\r\nNúmeros: {}\r\nDatas: {}\r\n",
            fact.source,
            fact.entities.join(", "),
            fact.numbers.join(", "),
            fact.dates.join(", ")
        ));
    }
    out
}

/// A resposta de uma coluna como fica na leitura guardada.
fn snapshot_answer(provider: &str, outcome: &ColumnOutcome) -> SnapshotAnswer {
    let read = outcome.read();
    SnapshotAnswer {
        provider: provider.to_string(),
        status: outcome.status(),
        text: read.map(|read| read.text.clone()).unwrap_or_default(),
        links: read.map(|read| read.links.clone()).unwrap_or_default(),
        cut: read.is_some_and(|read| read.cut),
    }
}

/// O que um run gravou na sessao: as respostas da leitura guardada e os
/// itens com o texto lido (o que a thread compara).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct RecordedRun {
    pub(in crate::windows_app) answers: Vec<SnapshotAnswer>,
    pub(in crate::windows_app) item_ids: Vec<String>,
}

/// Grava um run acabado na sessao: uma tentativa e (com texto) um item por
/// coluna que o turno perguntou, e a leitura guardada. `None`, e a sessao
/// intacta, quando ela ja nao e a do run (o dono abriu outra pergunta as
/// tres entretanto, ou apagou o historico) ou o turno do run nao existe:
/// as leituras de uma pergunta nunca entram noutra. Uma coluna que o turno
/// nao perguntou nao grava nada (`ResearchTurn::asked`).
pub(in crate::windows_app) fn record_consensus_run(
    session: &mut ResearchSession,
    run: &ConsensusRun,
) -> Option<RecordedRun> {
    if session.id != run.session {
        return None;
    }
    let asked: Vec<&'static str> = {
        let turn = session.turn(run.turn)?;
        run.columns
            .iter()
            .map(|column| column.provider.display_name())
            .filter(|provider| turn.asked(provider))
            .collect()
    };
    let mut answers = Vec::with_capacity(asked.len());
    let mut item_ids = Vec::new();
    for column in &run.columns {
        let provider = column.provider.display_name();
        if !asked.contains(&provider) {
            continue;
        }
        let outcome = column
            .outcome
            .clone()
            .unwrap_or_else(|| ColumnOutcome::Failed(CONSENSUS_FAILED.to_string()));
        let item_id = outcome
            .read()
            .filter(|read| !read.text.is_empty())
            .and_then(|read| {
                session.add_turn_answer(run.turn, provider, read.text.clone(), read.links.clone())
            });
        if let Some(id) = &item_id {
            item_ids.push(id.clone());
        }
        session.record_attempt(run.turn, provider, outcome.status(), item_id);
        answers.push(snapshot_answer(provider, &outcome));
    }
    if answers.is_empty() {
        return None;
    }
    session.push_consensus(run.turn, answers.clone());
    Some(RecordedRun { answers, item_ids })
}

// ===================== os turnos: qual, e que colunas =====================

/// A chave de operacao de uma pergunta, ANCORADA no turno mais recente da
/// sessao: `base` (`operation_key`: origem, coluna, resumo do texto e
/// geracao de navegacao da coluna) mais `@<ordinal do turno mais recente>`.
/// Se o turno mais recente e a mesma operacao (um Enter repetido, um evento
/// em duplicado), devolve a chave dele -- o mesmo turno. Senao a ancora
/// muda: perguntar X, depois Y, depois X outra vez na mesma coluna (a
/// geracao dela nao sobe numa pergunta escrita na pagina) sao tres turnos,
/// nunca o X de volta ao turno de antes. Deterministica e guardada na
/// sessao: nada de relogio nem de contador que um reinicio zere.
pub(in crate::windows_app) fn anchored_operation_key(
    session: &ResearchSession,
    base: &str,
) -> String {
    match session.current_turn() {
        Some(turn) if operation_base(&turn.operation_key) == base => turn.operation_key.clone(),
        Some(turn) => format!("{base}@{}", turn.ordinal),
        None => format!("{base}@0"),
    }
}

/// A chave sem a ancora (uma chave gravada antes da ancora e toda base).
fn operation_base(key: &str) -> &str {
    key.split_once('@').map_or(key, |(base, _)| base)
}

/// Abre (ou reencontra) o turno de uma pergunta: a chave ancorada
/// (`anchored_operation_key`) e os provedores na ordem em que a pergunta
/// seguiu. Devolve o ordinal, que e sempre o do turno mais recente.
pub(in crate::windows_app) fn begin_question_turn(
    session: &mut ResearchSession,
    origin: TurnOrigin,
    source: Option<usize>,
    text: &str,
    epoch: u64,
    providers: &[ProviderId],
) -> u32 {
    let key = anchored_operation_key(session, &operation_key(origin, source, text, epoch));
    let names: Vec<&str> = providers.iter().map(|id| id.display_name()).collect();
    session.begin_turn(&key, origin, text, &names)
}

/// Os provedores de um turno do `ask_other_columns`: a coluna de origem
/// primeiro -- a pergunta foi escrita na pagina dela, e ela segue a
/// conversa com a mesma pergunta -- e depois as colunas para onde a
/// pergunta seguiu.
pub(in crate::windows_app) fn ask_turn_providers(
    source: usize,
    targets: impl IntoIterator<Item = usize>,
) -> Vec<ProviderId> {
    let mut providers: Vec<ProviderId> = Vec::new();
    for index in std::iter::once(source).chain(targets) {
        if let Some(provider) = column_provider(index)
            && !providers.contains(&provider)
        {
            providers.push(provider);
        }
    }
    providers
}

/// As colunas que o Consenso le para `turn`: so as `columns` do comparador
/// cujo provedor o turno perguntou, cada uma com a sua geracao de
/// navegacao de agora. Depois de uma pergunta da palette so ao ChatGPT, a
/// resposta velha que ficou nas outras colunas nao e lida para esse turno.
pub(in crate::windows_app) fn consensus_hosts(
    turn: &ResearchTurn,
    columns: usize,
    generation: impl Fn(WebViewHost) -> u64,
) -> Vec<(WebViewHost, u64)> {
    (0..columns.min(COMPARATOR_COLUMNS))
        .filter(|index| {
            column_provider(*index).is_some_and(|provider| turn.asked(provider.display_name()))
        })
        .map(|index| {
            let column = WebViewHost::Column(index);
            (column, generation(column))
        })
        .collect()
}

// ===================== o estado e o evento =====================

/// O trabalho da thread `neural-consensus`.
pub(in crate::windows_app) enum ConsensusJob {
    /// A comparacao das respostas lidas do turno.
    Compare {
        run: u64,
        session: Box<ResearchSession>,
        item_ids: Vec<String>,
        turn: u32,
        answers: Vec<SnapshotAnswer>,
    },
}

/// O que chega ao event loop para o Consenso.
#[derive(Debug)]
pub(in crate::windows_app) enum ConsensusEvent {
    /// A sonda de 1,5 s do run `id`.
    Poll(u64),
    /// Uma leitura do `page_eval` chegou (ou passou do prazo).
    Page(PageEvalEvent),
    /// A thread acabou a comparacao do run.
    Compared { run: u64, report: ConsensusReport },
}

/// O estado do Consenso no `App`. Nasce sem thread: a `neural-consensus`
/// so no primeiro relatorio.
pub(in crate::windows_app) struct ConsensusState {
    pub(in crate::windows_app) reads: PageReads,
    pub(in crate::windows_app) run: Option<ConsensusRun>,
    next_run: u64,
    worker: Option<LazyWorker<ConsensusJob>>,
}

impl Default for ConsensusState {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsensusState {
    pub(in crate::windows_app) fn new() -> Self {
        Self {
            reads: PageReads::default(),
            run: None,
            next_run: 0,
            worker: None,
        }
    }

    /// Quantas threads o Consenso criou (0 ate o primeiro relatorio).
    #[cfg(test)]
    pub(in crate::windows_app) fn worker_threads_spawned(&self) -> usize {
        self.worker.as_ref().map_or(0, LazyWorker::threads_spawned)
    }

    /// Um run novo do turno `turn` da sessao `session` sobre `hosts`; o
    /// anterior, se havia, cai com as suas leituras em voo.
    pub(in crate::windows_app) fn begin_run(
        &mut self,
        session: &str,
        turn: u32,
        started: Instant,
        hosts: impl IntoIterator<Item = (WebViewHost, u64)>,
    ) -> u64 {
        self.reads.cancel_all();
        self.next_run = self.next_run.wrapping_add(1).max(1);
        let id = self.next_run;
        self.run = Some(ConsensusRun::begin(id, session, turn, started, hosts));
        id
    }

    /// Abriu-se o turno `turn` da sessao `session`: um run que lia OUTRO
    /// turno (ou outra sessao) acaba aqui -- as leituras em voo caem, e o
    /// run sai interrompido (`ConsensusRun::interrupt`) para se gravar no
    /// turno dele. `None` sem run, ou quando o run ja e deste turno (a mesma
    /// operacao repetida).
    pub(in crate::windows_app) fn supersede(
        &mut self,
        session: &str,
        turn: u32,
    ) -> Option<ConsensusRun> {
        let superseded = self
            .run
            .as_ref()
            .is_some_and(|run| run.session != session || run.turn != turn);
        if !superseded {
            return None;
        }
        self.reads.cancel_all();
        let mut run = self.run.take()?;
        run.interrupt();
        Some(run)
    }

    /// O run `id`, se ainda e o de agora (os gates; a sonda vai pelo campo
    /// porque precisa de `reads` ao mesmo tempo).
    #[cfg(test)]
    pub(in crate::windows_app) fn run_mut(&mut self, id: u64) -> Option<&mut ConsensusRun> {
        self.run.as_mut().filter(|run| run.id == id)
    }

    pub(in crate::windows_app) fn abandon(&mut self) {
        self.reads.cancel_all();
        self.run = None;
    }
}

impl App {
    /// O unico braco do Consenso no `user_event`.
    pub(in crate::windows_app) fn consensus_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::Poll(run) => self.consensus_poll(run),
            ConsensusEvent::Page(event) => self.consensus_page_event(event),
            ConsensusEvent::Compared { run, report } => self.consensus_compared(run, report),
        }
    }

    /// Abre (ou reencontra) o turno de uma pergunta na sessao viva -- sem
    /// sessao, nasce uma com a pergunta. `source` e a coluna de onde a
    /// pergunta partiu (`None` na pergunta as tres), e a chave de operacao
    /// leva a geracao de navegacao dela de AGORA, ancorada no turno mais
    /// recente (`begin_question_turn`). Um run que ainda lia outro turno
    /// acaba aqui (`consensus_turn_begun`).
    pub(in crate::windows_app) fn consensus_begin_turn(
        &mut self,
        origin: TurnOrigin,
        source: Option<usize>,
        text: &str,
        providers: &[ProviderId],
    ) -> u32 {
        let epoch = source
            .and_then(|index| self.translation.epoch(WebViewHost::Column(index)))
            .map_or(0, |epoch| epoch.current());
        let session = self
            .current_research
            .get_or_insert_with(|| ResearchSession::new(text.to_string()));
        let before = session.turns.len();
        let turn = begin_question_turn(session, origin, source, text, epoch, providers);
        let session_id = session.id.clone();
        if session.turns.len() != before {
            self.privacy.save_session(session.clone());
        }
        self.consensus_turn_begun(&session_id, turn);
        turn
    }

    /// Abriu-se o turno `turn` da sessao `session_id`: o run que lia outro
    /// turno ou outra sessao acaba (`ConsensusState::supersede`) e grava-se
    /// no turno DELE, sem relatorio -- o dono ja fez outra pergunta. Em
    /// `compare` chama-se ANTES de a sessao nova tomar o lugar da anterior,
    /// para o run se gravar na sessao dele; se ela ja nao for a viva, cai.
    pub(in crate::windows_app) fn consensus_turn_begun(&mut self, session_id: &str, turn: u32) {
        if let Some(run) = self.consensus.supersede(session_id, turn) {
            debug_log(format_args!(
                "consensus: run {} do turno {} interrompido pelo turno {turn}",
                run.id, run.turn
            ));
            self.record_consensus(&run, false);
        }
    }

    /// `research:compare`: le as respostas das colunas do turno mais
    /// recente pelo lado nativo -- so as que esse turno perguntou
    /// (`consensus_hosts`). Sem sessao ou sem colunas a vista, diz porque.
    /// Devolve se um run comecou.
    pub(in crate::windows_app) fn read_consensus(&mut self) -> bool {
        let Some(session) = self.current_research.as_ref() else {
            self.show_native_text(CONSENSUS_TITLE, CONSENSUS_NO_SESSION);
            return false;
        };
        let columns = match (&self.surface, &self.comparator) {
            (Surface::Comparator, Some(comp)) => comp.views.len(),
            _ => 0,
        };
        if columns == 0 {
            self.show_native_text(CONSENSUS_TITLE, CONSENSUS_NO_COLUMNS);
            return false;
        }
        if session.current_turn().is_none() {
            // Uma sessao de antes dos turnos: a pergunta dela e o turno 1.
            let question = session.question.clone();
            self.consensus_begin_turn(
                TurnOrigin::Compare,
                None,
                &question,
                &ProviderId::default_slots(),
            );
        }
        let translation = &self.translation;
        let Some((session_id, turn, hosts)) = self.current_research.as_ref().and_then(|session| {
            let turn = session.current_turn()?;
            // `column`, nao `host`: o gate `every_webview_gets_the_hooks`
            // prende o nome `host` ao sitio de nascimento da fonte ao lado.
            let hosts = consensus_hosts(turn, columns, |column| {
                translation.epoch(column).map_or(0, |epoch| epoch.current())
            });
            Some((session.id.clone(), turn.ordinal, hosts))
        }) else {
            self.show_native_text(CONSENSUS_TITLE, CONSENSUS_NO_SESSION);
            return false;
        };
        if hosts.is_empty() {
            self.show_native_text(CONSENSUS_TITLE, CONSENSUS_NO_COLUMNS);
            return false;
        }
        let id = self
            .consensus
            .begin_run(&session_id, turn, Instant::now(), hosts);
        debug_log(format_args!("consensus: run {id} do turno {turn} começou"));
        self.show_splash(CONSENSUS_READING.to_string(), 2);
        self.consensus_poll(id);
        true
    }

    /// A sonda: por coluna, decide (`decide_column_poll`) e le, espera ou
    /// fecha; quando todas acabaram, o relatorio; senao, outra sonda em
    /// 1,5 s. Sem comparador a vista, o run cai.
    fn consensus_poll(&mut self, id: u64) {
        let Some(comp) = self.comparator.as_ref() else {
            self.consensus.abandon();
            return;
        };
        // Pelo campo, nao por `run_mut`: a sonda precisa de `reads` ao
        // mesmo tempo que do run.
        let Some(run) = self.consensus.run.as_mut().filter(|run| run.id == id) else {
            return;
        };
        let elapsed = run.started.elapsed();
        let now = Instant::now();
        for column in &mut run.columns {
            let Some(index) = column.index() else {
                continue;
            };
            let translated = self.translation.column_translated(index);
            let navigated = self
                .translation
                .epoch(column.host)
                .is_none_or(|epoch| epoch.current() != column.generation);
            match decide_column_poll(column, translated, navigated, elapsed) {
                PollAction::Wait => {}
                PollAction::Finish(outcome) => {
                    debug_log(format_args!(
                        "consensus: coluna {index} acabou: {}",
                        outcome.label()
                    ));
                    column.outcome = Some(outcome);
                }
                PollAction::Read => {
                    let Some(view) = comp.views.get(index) else {
                        column.outcome = Some(ColumnOutcome::Failed("sem coluna".into()));
                        continue;
                    };
                    let Some(selector) = column.provider.answer_read() else {
                        column.outcome = Some(ColumnOutcome::Unreadable);
                        continue;
                    };
                    let Some(epoch) = self.translation.epoch(column.host) else {
                        continue;
                    };
                    let proxy = self.proxy.clone();
                    let timers = &self.timers;
                    let started = self.consensus.reads.read_with_arg(
                        &view.webview,
                        answer_read_spec(),
                        &answer_read_config(selector),
                        &epoch,
                        now,
                        move |event| {
                            let _ =
                                proxy.send_event(UserEvent::Consensus(ConsensusEvent::Page(event)));
                        },
                        |delay, event| {
                            timers.after(delay, UserEvent::Consensus(ConsensusEvent::Page(event)));
                        },
                    );
                    match started {
                        Ok(token) => column.pending = Some(token),
                        Err(refusal) => {
                            debug_log(format_args!(
                                "consensus: coluna {index} não pôde ser lida: {refusal:?}"
                            ));
                            if let Some(outcome) = column.refuse(format!("{refusal:?}")) {
                                debug_log(format_args!(
                                    "consensus: coluna {index} acabou: {}",
                                    outcome.label()
                                ));
                            }
                        }
                    }
                }
            }
        }
        let translation = &self.translation;
        run.settle_translated_stragglers(|index| translation.column_translated(index));
        if run.finished() {
            self.finish_consensus_run();
        } else {
            self.timers.after(
                CONSENSUS_POLL_INTERVAL,
                UserEvent::Consensus(ConsensusEvent::Poll(id)),
            );
        }
    }

    /// Uma leitura chegou (ou passou do prazo): o `PageReads` confere o
    /// prazo, o tecto, a geracao e o URL; depois `column_answer_read` exige
    /// que a pagina seja do provedor da coluna e le a resposta contra o host
    /// dela. Uma leitura de um token que ja nao e de nenhuma coluna (o run
    /// acabou, ou e outro) cai.
    fn consensus_page_event(&mut self, event: PageEvalEvent) {
        let token = match &event {
            PageEvalEvent::Arrived { token, .. } | PageEvalEvent::Expired(token) => *token,
        };
        let index = self
            .consensus
            .run
            .as_mut()
            .and_then(|run| run.column_by_token(token))
            .and_then(|column| column.index());
        let page_url = index.and_then(|index| {
            self.comparator
                .as_ref()
                .and_then(|comp| comp.views.get(index))
                .and_then(|view| view.webview.page_url())
        });
        let outcome = self
            .consensus
            .reads
            .settle(event, || page_url.clone(), Instant::now());
        let Some(index) = index else {
            return;
        };
        let Some(column) = self
            .consensus
            .run
            .as_mut()
            .and_then(|run| run.column_by_token(token))
        else {
            return;
        };
        match outcome {
            PageEvalOutcome::Delivered { raw, .. } => {
                match column_answer_read(column.provider, page_url.as_deref(), &raw) {
                    Ok(read) => {
                        if read.dropped_links > 0 {
                            debug_log(format_args!(
                                "consensus: coluna {index}: {} ligação(ões) inválida(s) deixada(s) de fora",
                                read.dropped_links
                            ));
                        }
                        if let Some(outcome) = column.observe(read) {
                            debug_log(format_args!(
                                "consensus: coluna {index} acabou: {}",
                                outcome.label()
                            ));
                        }
                    }
                    Err(error) => {
                        debug_log(format_args!(
                            "consensus: coluna {index} recusada: {}",
                            error.describe()
                        ));
                        column.refuse(error.describe());
                    }
                }
            }
            PageEvalOutcome::Dropped { reason, .. } => {
                debug_log(format_args!(
                    "consensus: leitura da coluna {index} caiu: {reason:?}"
                ));
                column.pending = None;
            }
            PageEvalOutcome::Stale(_) => {}
        }
        if self
            .consensus
            .run
            .as_ref()
            .is_some_and(ConsensusRun::finished)
        {
            self.finish_consensus_run();
        }
    }

    /// Todas as colunas acabaram: grava o run (`record_consensus`) e manda
    /// a comparacao para a thread.
    fn finish_consensus_run(&mut self) {
        let Some(run) = self.consensus.run.take() else {
            return;
        };
        self.record_consensus(&run, true);
    }

    /// Grava um run na sessao viva (`record_consensus_run`: as tentativas no
    /// turno, os itens com as ligacoes, a leitura guardada, no maximo 8) e
    /// grava a sessao pelo guard; com `report`, a comparacao vai para a
    /// thread. Se a sessao viva ja nao e a do run, nada se grava.
    fn record_consensus(&mut self, run: &ConsensusRun, report: bool) {
        let Some(session) = self.current_research.as_mut() else {
            debug_log(format_args!("consensus: run {} sem sessão: caiu", run.id));
            return;
        };
        let Some(recorded) = record_consensus_run(session, run) else {
            debug_log(format_args!(
                "consensus: run {} de outra sessão ou turno: caiu",
                run.id
            ));
            return;
        };
        let snapshot = session.clone();
        self.privacy.save_session(snapshot.clone());
        if report {
            self.submit_consensus_job(ConsensusJob::Compare {
                run: run.id,
                session: Box::new(snapshot),
                item_ids: recorded.item_ids,
                turn: run.turn,
                answers: recorded.answers,
            });
        }
    }

    /// A thread `neural-consensus`, criada na primeira vez que e precisa.
    fn submit_consensus_job(&mut self, job: ConsensusJob) {
        if self.consensus.worker.is_none() {
            let proxy = self.proxy.clone();
            self.consensus.worker = Some(LazyWorker::new(
                CONSENSUS_WORKER_NAME,
                move |job: ConsensusJob, context: &JobContext| {
                    if context.cancelled() {
                        return;
                    }
                    match job {
                        ConsensusJob::Compare {
                            run,
                            session,
                            item_ids,
                            turn,
                            answers,
                        } => {
                            let facts = if item_ids.is_empty() {
                                Vec::new()
                            } else {
                                session.comparison(&item_ids)
                            };
                            let _ =
                                proxy.send_event(UserEvent::Consensus(ConsensusEvent::Compared {
                                    run,
                                    report: ConsensusReport {
                                        turn,
                                        answers,
                                        facts,
                                    },
                                }));
                        }
                    }
                },
            ));
        }
        let submitted = self
            .consensus
            .worker
            .as_mut()
            .is_some_and(|worker| worker.submit(job).is_ok());
        if !submitted {
            self.show_splash("Consenso: a thread não arrancou.".to_string(), 4);
        }
    }

    fn consensus_compared(&mut self, run: u64, report: ConsensusReport) {
        debug_log(format_args!(
            "consensus: run {run} comparado ({} resposta(s))",
            report.answers.len()
        ));
        let text = consensus_report_text(&report);
        self.show_native_text(CONSENSUS_TITLE, &text);
    }
}
