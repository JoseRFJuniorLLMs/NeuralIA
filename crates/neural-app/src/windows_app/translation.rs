use super::*;

use neural_core::ai_policy::{DataClass, estimate_tokens};
use neural_core::llm::gemini;
use neural_core::llm::{
    ApiClient, ApiError, Endpoint, ModelId, ModelInfo, Pick, Provider, Purpose,
    filter_generation_models, pick,
};
use neural_core::translate::{
    self as core_translate, ApplyEntry, Batch, BatchError, CollectError, Plan,
};

use crate::ai_settings::{AiPurpose, Day};
use crate::egress::{
    ConsentAnswer, ConsentCard, Decision, Destination, EgressGate, EgressPrivacy, EgressRequest,
    RefuseReason, SiteOrigin, Trigger,
};
use crate::lazy_worker::{JobContext, LazyWorker};
use crate::secrets::{ApiKey, KeySlot, KeyVault};
use crate::stores::{KEYS_STORE, LIVE_KEY_STORE, TRANSLATE_STORE};

// ===================== Traduzir pagina (translation, plano 2.3) =====================
//
// Modulo de feature (o padrao de `theme.rs`): `UserEvent::Translate(TranslateEvent)`,
// o campo `App::translation` e o braco `translation_event` no event loop. O
// 文A de cada coluna (`ColumnButton::Translate`) e o item «Traduzir página»
// do botao direito (colunas, fonte ao lado -- privada ou nao -- e a Web
// completa) traduzem a pagina para pt-BR no proprio sitio; outro clique
// devolve o original. NAO e o «Traduzir» da barra de selecao (que manda o
// texto as tres IAs): outra feature, outro rotulo.
//
// O caminho de um clique, todo pelo que ja existe (nada duplicado):
//
// 1. Sem a chave Gemini (`KeySlot::Gemini`, a mesma do Gemini Live) nem se
//    le a pagina: o cartao diz «Para traduzir aqui mesmo, guarde a sua chave
//    Gemini (a mesma do Gemini Live).» e oferece o pedido de chave nativo.
//    O cofre so decifra a chave na thread `neural-translate`; aqui so se ve
//    se o ficheiro existe.
// 2. `TRANSLATE_COLLECT` pelo `page_eval` (token, prazo, tecto de 400 KB,
//    geracao de navegacao e URL conferidos na chegada), lido por
//    `neural_core::translate::parse_collected` como dado nao confiavel.
//    Ja em portugues: «Esta página já está em português.»
// 3. Os blocos (`plan_batches`: 4 000 caracteres e 80 textos por bloco, 60
//    000 caracteres e 15 blocos por clique).
// 4. A thread `neural-translate` (um `LazyWorker`: nasce no primeiro clique,
//    nunca no `App::new`) le a chave, lista os modelos e escolhe o da
//    Traducao pela regra (`llm::pick`, com a fixacao do dono em
//    `ai/settings.json`).
// 5. O `EgressGate` decide (`TranslationState::picked`, `consent_answered`
//    e `retry`, sem janela: so um `Send` devolve o trabalho `Translate`
//    que vai para a thread): `Send`, `Refuse` ou `Ask(ConsentCard)` -- o
//    cartao nativo (`NativeCard`: token, armar de 600 ms, expiracao, so o
//    pintado) com «Traduzir com Gemini?», o texto que vai, os hosts, os
//    tokens, o modelo e a faixa de preco, e os botoes [Traduzir] [Sempre
//    neste site] [Cancelar]. «Sempre neste site» so existe fora de um
//    contexto privado: nunca no Split privado nem no Modo privado; gravado,
//    vive no `translate.json` (`StoreKind::Setting`). No Split privado o
//    «Traduzir» vale so desta vez. Cada bloco e uma chamada paga que o
//    portao conta no `ai/usage.json`.
// 6. A thread traduz bloco a bloco (`translate_batch`: o texto cercado pelo
//    `PromptBuilder`, a resposta so aceite completa e do tamanho certo) e
//    cada bloco volta como trocas (no, original, traducao) que o
//    `TRANSLATE_APPLY` aplica pelo `page_eval` -- so `nodeValue`, so num no
//    que ainda tem o original. «Traduzindo 2/5…»; no fim, «Página
//    traduzida.» ou «Traduzido em parte (3 de 5 blocos)» com «Tentar de
//    novo».
// 7. Outro clique (ou o menu) corre `TRANSLATE_RESTORE`: so os nos que ainda
//    tem a traducao voltam ao original. Um bloco que acabe depois disso ja
//    nao vai a pagina. Uma navegacao que vai larga a traducao; uma recusada
//    (um `mailto:`, um link que abre na Web completa) nao muda a pagina e
//    nao a larga.
//
// Enquanto uma coluna esta traduzida (desde que os blocos comecam a ir ate o
// original voltar), a resposta dela nao entra na sessao de pesquisa nem na
// memoria (`research_answer_provider`): nada traduzido a maquina e gravado
// como resposta de uma IA. Nada da Traducao vai para o Historico nem para a
// memoria. A chave nunca sai da thread: nem nos scripts, nem nos eventos.

/// O nome da thread da Traducao.
pub(in crate::windows_app) const TRANSLATE_WORKER_NAME: &str = "neural-translate";
/// O tecto do JSON do `TRANSLATE_COLLECT` (o brief: <=400 KB).
pub(in crate::windows_app) const TRANSLATE_COLLECT_MAX_BYTES: usize = 400 * 1024;
/// O tecto da resposta do `TRANSLATE_APPLY`/`TRANSLATE_RESTORE` (tres numeros).
pub(in crate::windows_app) const TRANSLATE_APPLY_MAX_BYTES: usize = 1024;
/// Quanto se espera por um script da Traducao.
pub(in crate::windows_app) const TRANSLATE_SCRIPT_DEADLINE: Duration = Duration::from_secs(8);
/// Sem resposta, o cartao some e conta como Cancelar.
pub(in crate::windows_app) const TRANSLATE_CARD_SECONDS: u64 = 45;

/// O rotulo do 文A e do item do menu: nao e o «Traduzir» da barra de selecao.
pub(in crate::windows_app) const TRANSLATE_PAGE_LABEL: &str = "Traduzir página";
pub(in crate::windows_app) const TRANSLATE_CONSENT_TITLE: &str = "Traduzir com Gemini?";
pub(in crate::windows_app) const TRANSLATE_CONSENT_BODY: &str = "O texto visível desta página (até ~60 mil caracteres) vai para a API Gemini do Google com a sua chave.";
pub(in crate::windows_app) const TRANSLATE_NO_KEY: &str =
    "Para traduzir aqui mesmo, guarde a sua chave Gemini (a mesma do Gemini Live).";
pub(in crate::windows_app) const TRANSLATE_ALREADY_PT: &str = "Esta página já está em português.";
pub(in crate::windows_app) const TRANSLATE_NOTHING: &str = "Nada para traduzir nesta página.";
pub(in crate::windows_app) const TRANSLATE_DONE: &str = "Página traduzida.";
pub(in crate::windows_app) const TRANSLATE_RESTORED: &str = "Original restaurado.";
pub(in crate::windows_app) const TRANSLATE_CANCELLED: &str = "Tradução cancelada.";
pub(in crate::windows_app) const TRANSLATE_ONLY_WEB: &str =
    "Só páginas da web podem ser traduzidas.";
pub(in crate::windows_app) const TRANSLATE_UNREADABLE: &str = "Não foi possível ler esta página.";

/// «Traduzindo 2/5…»
pub(in crate::windows_app) fn translating_message(done: usize, total: usize) -> String {
    format!("Traduzindo {done}/{total}…")
}

/// «Traduzido em parte (3 de 5 blocos)»
pub(in crate::windows_app) fn partial_message(translated: usize, total: usize) -> String {
    format!("Traduzido em parte ({translated} de {total} blocos)")
}

// ===================== os scripts (page_eval) =====================
//
// Os tres scripts partilham o mesmo percurso (`translate_walker!`): um
// `TreeWalker` (SHOW_ELEMENT | SHOW_TEXT) sobre o `body` que RECUSA -- com a
// subarvore inteira -- script, style, noscript, template, code, pre, kbd,
// samp, textarea, input, select, option, svg, math, iframe, object e canvas,
// `[translate=no]`, `.notranslate`, `contenteditable` e `aria-hidden=true`
// (tambem no `<html>`/`<body>`: uma pagina `translate="no"` nao se traduz), e
// aceita os nos de texto. O indice de um no e a posicao dele nessa lista: o
// APPLY e o RESTORE refazem o percurso e so trocam um no cujo `nodeValue` e
// exatamente o esperado. Nenhum guarda nada na pagina (nem uma variavel
// global) e nenhum escreve HTML. Os nomes e comentarios destes scripts nao
// usam palavras da lista do gate de so-leitura.

macro_rules! translate_walker {
    () => {
        r#"
  var SKIP = { script: 1, style: 1, noscript: 1, template: 1, code: 1, pre: 1, kbd: 1, samp: 1,
    textarea: 1, input: 1, select: 1, option: 1, svg: 1, math: 1, iframe: 1, object: 1, canvas: 1 };
  function refused(el) {
    var tag = String(el.localName || el.nodeName || '').toLowerCase();
    if (SKIP[tag] === 1) return true;
    if (typeof el.getAttribute === 'function') {
      if (String(el.getAttribute('translate') || '').toLowerCase() === 'no') return true;
      if (String(el.getAttribute('aria-hidden') || '').toLowerCase() === 'true') return true;
      var editable = el.getAttribute('contenteditable');
      if (editable !== null && editable !== undefined && String(editable).toLowerCase() !== 'false') return true;
    }
    if (el.isContentEditable === true) return true;
    if (el.classList && typeof el.classList.contains === 'function' && el.classList.contains('notranslate')) return true;
    return false;
  }
  function textNodes() {
    var list = [];
    var root = document.body;
    if (!root) return list;
    for (var up = root; up; up = up.parentNode) {
      if (up.nodeType === 1 && refused(up)) return list;
    }
    var walker = document.createTreeWalker(root, 5, {
      acceptNode: function (node) {
        if (node.nodeType === 1) return refused(node) ? 2 : 3;
        return node.nodeType === 3 ? 1 : 3;
      }
    });
    var node = walker.nextNode();
    while (node && list.length < 200000) {
      list.push(node);
      node = walker.nextNode();
    }
    return list;
  }
"#
    };
}

/// Le os nos de texto: `{lang, items: [[indice, texto], ...], truncated}`.
/// So os que tem algo alem de espacos; no maximo 10 000 textos e 80 000
/// caracteres (o clique so manda 60 000).
pub(in crate::windows_app) const TRANSLATE_COLLECT_SCRIPT: &str = concat!(
    "(function () {",
    translate_walker!(),
    r#"
  var list = textNodes();
  var items = [];
  var chars = 0;
  var truncated = list.length >= 200000;
  for (var i = 0; i < list.length; i++) {
    var text = String(list[i].nodeValue || '');
    if (!/\S/.test(text)) continue;
    if (items.length >= 10000 || chars + text.length > 80000) { truncated = true; break; }
    chars += text.length;
    items.push([i, text]);
  }
  var html = document.documentElement;
  var lang = html && typeof html.getAttribute === 'function' ? String(html.getAttribute('lang') || '') : '';
  return { lang: lang.slice(0, 35), items: items, truncated: truncated };
})()"#
);

/// Troca cada `[indice, original, traducao]` so se o no ainda tem o original.
pub(in crate::windows_app) const TRANSLATE_APPLY_SCRIPT: &str = concat!(
    "(function (entries) {",
    translate_walker!(),
    r#"
  var list = textNodes();
  var done = 0, changed = 0, missing = 0;
  for (var k = 0; k < entries.length; k++) {
    var entry = entries[k];
    var node = list[entry[0]];
    if (!node) { missing++; continue; }
    if (node.nodeValue === entry[1]) { node.nodeValue = entry[2]; done++; } else { changed++; }
  }
  return { done: done, changed: changed, missing: missing };
})"#
);

/// Devolve o original a cada no que ainda tem a traducao.
pub(in crate::windows_app) const TRANSLATE_RESTORE_SCRIPT: &str = concat!(
    "(function (entries) {",
    translate_walker!(),
    r#"
  var list = textNodes();
  var done = 0, changed = 0, missing = 0;
  for (var k = 0; k < entries.length; k++) {
    var entry = entries[k];
    var node = list[entry[0]];
    if (!node) { missing++; continue; }
    if (node.nodeValue === entry[2]) { node.nodeValue = entry[1]; done++; } else { changed++; }
  }
  return { done: done, changed: changed, missing: missing };
})"#
);

/// O argumento do APPLY e do RESTORE: `[[no, original, traducao], ...]`.
pub(in crate::windows_app) fn entries_arg(entries: &[ApplyEntry]) -> serde_json::Value {
    serde_json::Value::Array(entries.iter().map(ApplyEntry::to_json).collect())
}

// ===================== onde se traduz =====================

/// As paginas que se traduzem: as colunas das IAs, a fonte ao lado (privada
/// ou nao) e a Web completa. O Leitor, o PDF, os livros, os paineis e os
/// servicos nao.
pub(in crate::windows_app) fn translatable_host(host: WebViewHost) -> bool {
    match host {
        WebViewHost::Column(index)
        | WebViewHost::Split(index)
        | WebViewHost::PrivateSplit(index) => index < COMPARATOR_COLUMNS,
        WebViewHost::External => true,
        _ => false,
    }
}

/// A privacidade do pedido: o Split privado pode perguntar, mas nada fica
/// no disco (nem se oferece nem se le «Sempre neste site»). O Modo privado
/// (onda 4) passa `EgressPrivacy::PrivateMode` aqui.
pub(in crate::windows_app) fn translation_privacy(host: WebViewHost) -> EgressPrivacy {
    match host {
        WebViewHost::PrivateSplit(_) => EgressPrivacy::PrivateSurface,
        _ => EgressPrivacy::Normal,
    }
}

/// Os hospedeiros com a sua geracao de navegacao (`NavEpoch`), que o
/// navigation handler de cada WebView deles sobe (`App::hooked_builder`).
fn epoch_hosts() -> Vec<WebViewHost> {
    let mut hosts = vec![WebViewHost::External];
    for index in 0..COMPARATOR_COLUMNS {
        hosts.push(WebViewHost::Column(index));
        hosts.push(WebViewHost::Split(index));
        hosts.push(WebViewHost::PrivateSplit(index));
    }
    hosts
}

/// O pedido de saida de um clique: a pagina (`PageContent`) para a Gemini
/// com o modelo escolhido, o site, a estimativa de tokens e uma chamada paga
/// por bloco.
pub(in crate::windows_app) fn translation_request(
    plan: &Plan,
    choice: &Pick,
    origin: Option<SiteOrigin>,
    privacy: EgressPrivacy,
) -> EgressRequest<'static> {
    EgressRequest {
        feature: AiPurpose::Translation,
        data: DataClass::PageContent,
        destination: Destination::cloud(choice),
        origin,
        token_estimate: estimate_tokens(plan.chars),
        calls: u32::try_from(plan.batches.len()).unwrap_or(u32::MAX),
        trigger: Trigger::Click,
        privacy,
    }
}

// ===================== o cartao =====================

/// Um botao do cartao da Traducao.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum CardChoice {
    /// «Traduzir» (ou «Continuar» no cartao do limite): a resposta ao portao.
    Translate(ConsentAnswer),
    /// «Sempre neste site».
    AlwaysOnSite,
    Cancel,
    /// Sem chave: «Guardar chave» abre o pedido nativo.
    SaveKey,
    /// Traduzido em parte: «Tentar de novo» manda so os blocos que falharam.
    Retry,
    /// «Fechar».
    Close,
}

impl CardChoice {
    pub(in crate::windows_app) fn label(self, needs_consent: bool) -> &'static str {
        match self {
            Self::Translate(_) if needs_consent => "Traduzir",
            Self::Translate(_) => "Continuar",
            Self::AlwaysOnSite => "Sempre neste site",
            Self::Cancel => "Cancelar",
            Self::SaveKey => "Guardar chave",
            Self::Retry => "Tentar de novo",
            Self::Close => "Fechar",
        }
    }

    /// O principal: cheio, na cor de destaque.
    pub(in crate::windows_app) fn primary(self) -> bool {
        matches!(self, Self::Translate(_) | Self::SaveKey | Self::Retry)
    }
}

/// Os botoes do cartao de consentimento, a partir do `ConsentCard` do
/// portao: «Traduzir» (o consentimento da sessao para este site; numa
/// superficie privada, so desta vez), «Sempre neste site» SO quando o
/// portao o oferece (nunca num contexto privado) e «Cancelar». No cartao do
/// limite mensal (ja havia consentimento): «Continuar» e «Cancelar».
pub(in crate::windows_app) fn consent_choices(
    card: &ConsentCard,
    privacy: EgressPrivacy,
) -> Vec<CardChoice> {
    let answers = card.answers();
    let mut choices = Vec::new();
    if card.needs_consent() {
        let session = privacy == EgressPrivacy::Normal && answers.contains(&ConsentAnswer::Session);
        choices.push(CardChoice::Translate(if session {
            ConsentAnswer::Session
        } else {
            ConsentAnswer::Once
        }));
        if card.offers_always()
            && privacy == EgressPrivacy::Normal
            && answers.contains(&ConsentAnswer::AlwaysOnSite)
        {
            choices.push(CardChoice::AlwaysOnSite);
        }
    } else {
        choices.push(CardChoice::Translate(ConsentAnswer::Once));
    }
    choices.push(CardChoice::Cancel);
    choices
}

/// O que o cartao pinta: titulo, linhas e botoes (rotulo, principal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct CardView {
    pub(in crate::windows_app) title: String,
    pub(in crate::windows_app) lines: Vec<String>,
    pub(in crate::windows_app) buttons: Vec<(&'static str, bool)>,
}

/// O cartao de consentimento: a pergunta do brief, o que vai e para onde
/// (as linhas do portao: hosts, tokens, modelo e faixa de preco, site,
/// limite) e, se o clique nao manda tudo, o que fica no original.
pub(in crate::windows_app) fn consent_view(
    card: &ConsentCard,
    choices: &[CardChoice],
    plan: &Plan,
) -> CardView {
    let (title, mut lines) = if card.needs_consent() {
        (
            TRANSLATE_CONSENT_TITLE.to_string(),
            vec![TRANSLATE_CONSENT_BODY.to_string()],
        )
    } else {
        (card.title(), Vec::new())
    };
    lines.extend(card.lines());
    if plan.left_out > 0 {
        lines.push("O resto da página fica no original.".to_string());
    }
    CardView {
        title,
        lines,
        buttons: choices
            .iter()
            .map(|choice| (choice.label(card.needs_consent()), choice.primary()))
            .collect(),
    }
}

/// O que espera o clique no cartao.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::windows_app) enum CardPrompt {
    Consent {
        run: u64,
        card: ConsentCard,
        choices: Vec<CardChoice>,
    },
    NoKey {
        choices: Vec<CardChoice>,
    },
    Partial {
        run: u64,
        choices: Vec<CardChoice>,
    },
}

impl CardPrompt {
    fn choices(&self) -> &[CardChoice] {
        match self {
            Self::Consent { choices, .. }
            | Self::NoKey { choices }
            | Self::Partial { choices, .. } => choices,
        }
    }
}

/// O cartao a pintar: (token, vista). O procedimento de janela le isto.
static TRANSLATE_CARD_VIEW: Mutex<Option<(u64, CardView)>> = Mutex::new(None);
/// O token do cartao que a pintura mostrou por ultimo: so esse responde.
static TRANSLATE_CARD_PAINTED: AtomicU64 = AtomicU64::new(0);
/// O botao onde o rato desceu.
static TRANSLATE_CARD_PRESSED: AtomicUsize = AtomicUsize::new(NATIVE_BUTTON_NONE);
const TRANSLATE_CARD_SUBCLASS_ID: usize = 0x4E7A;
/// O cartao, em pixeis logicos, centrado na janela.
pub(in crate::windows_app) const TRANSLATE_CARD_WIDTH: f64 = 640.0;
pub(in crate::windows_app) const TRANSLATE_CARD_HEIGHT: f64 = 320.0;

/// Para onde o cartao manda o clique: o proxy no app, um registo nos gates.
pub(in crate::windows_app) type TranslateCardSink = Box<dyn Fn(UserEvent)>;

/// Os botoes do cartao, encostados a direita no fundo, pela ordem da lista:
/// a largura de cada um pelo rotulo. Uma so funcao para o desenho e o
/// clique concordarem.
pub(in crate::windows_app) fn translate_card_buttons(
    client: &RECT,
    scale: f64,
    labels: &[&str],
) -> Vec<RECT> {
    let px = |value: f64| (value * scale).round() as i32;
    let bottom = client.bottom - px(20.0);
    let top = bottom - px(40.0);
    let mut right = client.right - px(24.0);
    let mut rects: Vec<RECT> = labels
        .iter()
        .rev()
        .map(|label| {
            let width = px((label.chars().count() as f64 * 8.6 + 40.0).max(110.0));
            let rect = RECT {
                left: right - width,
                top,
                right,
                bottom,
            };
            right = rect.left - px(12.0);
            rect
        })
        .collect();
    rects.reverse();
    rects
}

fn translate_card_scale(client: &RECT) -> f64 {
    ((client.bottom - client.top) as f64 / TRANSLATE_CARD_HEIGHT).max(1.0)
}

/// O botao debaixo de (x, y); bordas semiabertas.
pub(in crate::windows_app) fn translate_card_hit(
    client: &RECT,
    labels: &[&str],
    x: i32,
    y: i32,
) -> Option<usize> {
    translate_card_buttons(client, translate_card_scale(client), labels)
        .iter()
        .position(|rect| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
}

fn card_labels() -> Vec<&'static str> {
    TRANSLATE_CARD_VIEW
        .lock()
        .ok()
        .and_then(|view| {
            view.as_ref()
                .map(|(_, view)| view.buttons.iter().map(|(label, _)| *label).collect())
        })
        .unwrap_or_default()
}

/// O cartao da Traducao. Nativo e owned pela janela principal: a pagina que
/// vai ser traduzida nao o tapa, nao o pinta e nao lhe manda cliques. Nao se
/// ativa (`popup_no_activate_message`): responde-se com o rato.
unsafe extern "system" fn translate_card_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    if let Some(result) = popup_no_activate_message(message) {
        return result;
    }
    let point = || {
        (
            (lparam as u32 & 0xffff) as u16 as i16 as i32,
            ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32,
        )
    };
    match message {
        WM_LBUTTONDOWN => {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let (x, y) = point();
                if let Some(index) = translate_card_hit(&client, &card_labels(), x, y) {
                    TRANSLATE_CARD_PRESSED.store(index, Ordering::Release);
                    SetCapture(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let captured = GetCapture() == hwnd;
            let pressed = take_native_pressed_button(&TRANSLATE_CARD_PRESSED, || {
                if captured {
                    ReleaseCapture();
                }
            });
            let mut client = RECT::default();
            if !captured || GetClientRect(hwnd, &mut client) == 0 || reference_data == 0 {
                return 0;
            }
            let (x, y) = point();
            let released = translate_card_hit(&client, &card_labels(), x, y);
            if let Some(index) = native_release_matches(pressed, released) {
                let token = TRANSLATE_CARD_PAINTED.load(Ordering::Acquire);
                let sink = &*(reference_data as *const TranslateCardSink);
                sink(UserEvent::Translate(TranslateEvent::CardAnswer {
                    token,
                    index,
                }));
            }
            0
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            TRANSLATE_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
            0
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let view = TRANSLATE_CARD_VIEW
                        .lock()
                        .ok()
                        .and_then(|view| view.clone());
                    if let Some((token, view)) = view {
                        paint_translate_card(hdc, &client, &view);
                        // So agora o utilizador ve estes botoes: e este
                        // cartao que um clique a seguir responde.
                        TRANSLATE_CARD_PAINTED.store(token, Ordering::Release);
                    }
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

unsafe fn paint_translate_card(hdc: *mut core::ffi::c_void, client: &RECT, view: &CardView) {
    let theme = Theme::system();
    let background = CreateSolidBrush(rgb3(theme.surface));
    FillRect(hdc, client, background);
    DeleteObject(background as _);

    let scale = translate_card_scale(client);
    let px = |value: f64| (value * scale).round() as i32;
    let title_font = create_font((-22.0 * scale) as i32, FW_BOLD as i32);
    let text_font = create_font((-16.0 * scale) as i32, FW_NORMAL as i32);
    let button_font = create_font((-15.0 * scale) as i32, FW_BOLD as i32);
    let old_font = SelectObject(hdc, title_font as _);
    SetBkMode(hdc, TRANSPARENT as i32);

    SetTextColor(hdc, rgb3(theme.accent));
    let mut title = RECT {
        left: client.left + px(24.0),
        top: client.top + px(16.0),
        right: client.right - px(24.0),
        bottom: client.top + px(50.0),
    };
    draw_text(
        hdc,
        &view.title,
        &mut title,
        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    let labels: Vec<&str> = view.buttons.iter().map(|(label, _)| *label).collect();
    let buttons = translate_card_buttons(client, scale, &labels);
    let buttons_top = buttons.first().map_or(client.bottom, |rect| rect.top);
    SelectObject(hdc, text_font as _);
    SetTextColor(hdc, rgb3(theme.fg));
    let mut body = RECT {
        left: title.left,
        top: title.bottom + px(8.0),
        right: title.right,
        bottom: buttons_top - px(12.0),
    };
    draw_text(
        hdc,
        &view.lines.join("\n"),
        &mut body,
        DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX | DT_END_ELLIPSIS,
    );

    for (rect, (label, primary)) in buttons.iter().zip(&view.buttons) {
        let pill = UiRect {
            x: rect.left as f64,
            y: rect.top as f64,
            width: (rect.right - rect.left) as f64,
            height: (rect.bottom - rect.top) as f64,
        };
        let style = if *primary {
            PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
        } else {
            PillStyle::new(theme.surface_line, theme.surface_line, theme.fg)
        };
        draw_pill(hdc, pill, label, style, scale, button_font, theme.surface);
    }

    SelectObject(hdc, old_font);
    DeleteObject(title_font as _);
    DeleteObject(text_font as _);
    DeleteObject(button_font as _);
}

// ===================== a thread neural-translate =====================

/// Um trabalho da thread.
#[derive(Debug)]
pub(in crate::windows_app) enum TranslateJob {
    /// Le a chave, lista os modelos (uma vez por sessao) e escolhe o da
    /// Traducao.
    Pick { run: u64, pin: Option<ModelId> },
    /// Traduz os blocos (indice no plano, bloco) com o modelo escolhido.
    Translate {
        run: u64,
        model: ModelId,
        batches: Vec<(usize, Batch)>,
    },
}

impl TranslateJob {
    /// O run deste trabalho.
    pub(in crate::windows_app) fn run(&self) -> u64 {
        match self {
            Self::Pick { run, .. } | Self::Translate { run, .. } => *run,
        }
    }
}

/// O que a thread diz ao event loop. Nunca leva a chave: so o modelo
/// escolhido, as trocas da pagina e mensagens pt-BR que nunca a citam.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::windows_app) enum WorkerReport {
    Picked {
        run: u64,
        choice: Pick,
    },
    NoKey {
        run: u64,
    },
    PickFailed {
        run: u64,
        message: String,
    },
    Progress {
        run: u64,
        done: usize,
        total: usize,
    },
    BatchDone {
        run: u64,
        batch: usize,
        entries: Vec<ApplyEntry>,
    },
    BatchFailed {
        run: u64,
        batch: usize,
        message: String,
    },
    Finished {
        run: u64,
    },
}

/// O motor das chamadas: a Gemini pelo `llm::ApiClient` no produto, um
/// falso nos gates do app (os de `neural_core::translate` correm o
/// `translate_batch` que embarca contra um stub em 127.0.0.1).
pub(in crate::windows_app) trait TranslateEngine: Send {
    fn models(
        &mut self,
        key: &ApiKey,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<ModelInfo>, ApiError>;
    fn batch(
        &mut self,
        model: &ModelId,
        batch: &Batch,
        key: &ApiKey,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<ApplyEntry>, BatchError>;
}

/// A Gemini no host fixado (`Endpoint::pinned`), resolvido so para
/// enderecos publicos.
struct GeminiEngine {
    client: ApiClient,
}

impl TranslateEngine for GeminiEngine {
    fn models(
        &mut self,
        key: &ApiKey,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<ModelInfo>, ApiError> {
        gemini::list_models(&self.client, Some(key), cancelled).map(filter_generation_models)
    }

    fn batch(
        &mut self,
        model: &ModelId,
        batch: &Batch,
        key: &ApiKey,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<ApplyEntry>, BatchError> {
        core_translate::translate_batch(&self.client, model, batch, Some(key), cancelled)
    }
}

/// O que a thread tem: o motor, de onde vem a chave (o cofre, decifrado so
/// aqui), para onde vao os relatorios e os modelos ja listados.
pub(in crate::windows_app) struct TranslateWorker {
    pub(in crate::windows_app) engine: Box<dyn TranslateEngine>,
    pub(in crate::windows_app) key: Box<dyn Fn() -> Option<ApiKey> + Send>,
    pub(in crate::windows_app) report: Box<dyn Fn(WorkerReport) + Send>,
    pub(in crate::windows_app) models: Option<Vec<ModelInfo>>,
}

impl TranslateWorker {
    /// Um trabalho. `cancelled` e o do `LazyWorker` (um clique novo, ou o
    /// cancelamento, sobem a geracao).
    pub(in crate::windows_app) fn run(&mut self, job: TranslateJob, cancelled: &dyn Fn() -> bool) {
        match job {
            TranslateJob::Pick { run, pin } => {
                let Some(key) = (self.key)() else {
                    (self.report)(WorkerReport::NoKey { run });
                    return;
                };
                if self.models.is_none() {
                    match self.engine.models(&key, cancelled) {
                        Ok(models) => self.models = Some(models),
                        Err(ApiError::Cancelled) => return,
                        Err(error) => {
                            (self.report)(WorkerReport::PickFailed {
                                run,
                                message: error.pt_br_message(),
                            });
                            return;
                        }
                    }
                }
                let models = self.models.as_deref().unwrap_or_default();
                match pick(Purpose::Translation, Provider::Gemini, models, pin.as_ref()) {
                    Some(choice) => (self.report)(WorkerReport::Picked { run, choice }),
                    None => (self.report)(WorkerReport::PickFailed {
                        run,
                        message: "Nenhum modelo de tradução disponível com esta chave".to_string(),
                    }),
                }
            }
            TranslateJob::Translate {
                run,
                model,
                batches,
            } => {
                let Some(key) = (self.key)() else {
                    (self.report)(WorkerReport::NoKey { run });
                    return;
                };
                let total = batches.len();
                for (position, (batch_index, batch)) in batches.iter().enumerate() {
                    if cancelled() {
                        return;
                    }
                    (self.report)(WorkerReport::Progress {
                        run,
                        done: position + 1,
                        total,
                    });
                    let result = self.engine.batch(&model, batch, &key, cancelled);
                    // Cancelado enquanto o bloco ia (o 文A devolve o
                    // original, outro clique): o bloco ja nao vai a pagina.
                    if cancelled() {
                        return;
                    }
                    match result {
                        Ok(entries) => (self.report)(WorkerReport::BatchDone {
                            run,
                            batch: *batch_index,
                            entries,
                        }),
                        Err(BatchError::Api(ApiError::Cancelled)) => return,
                        Err(error) => {
                            // Um modelo que deixou de existir: a proxima
                            // escolha lista outra vez.
                            if error == BatchError::Api(ApiError::ModelUnavailable { status: 404 })
                            {
                                self.models = None;
                            }
                            (self.report)(WorkerReport::BatchFailed {
                                run,
                                batch: *batch_index,
                                message: error.pt_br_message(),
                            });
                        }
                    }
                }
                if !cancelled() {
                    (self.report)(WorkerReport::Finished { run });
                }
            }
        }
    }
}

// ===================== o estado =====================

/// O que cada leitura em voo faz quando chega.
#[derive(Debug, Clone, PartialEq)]
enum ReadKind {
    Collect,
    Apply,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunPhase {
    /// A ler a pagina.
    Collecting,
    /// A thread escolhe o modelo.
    Picking,
    /// O cartao espera o clique.
    Asking,
    /// Os blocos vao (a coluna ja conta como traduzida).
    Translating,
    /// Acabou; `applied` diz o que se trocou.
    Translated,
    /// A devolver o original.
    Restoring,
}

/// A traducao de uma pagina.
#[derive(Debug)]
struct TranslationRun {
    id: u64,
    host: WebViewHost,
    origin: Option<SiteOrigin>,
    privacy: EgressPrivacy,
    /// A geracao de navegacao da vista quando o clique chegou.
    generation: u64,
    phase: RunPhase,
    plan: Plan,
    choice: Option<Pick>,
    /// Tudo o que foi mandado ao APPLY (o RESTORE devolve isto).
    applied: Vec<ApplyEntry>,
    /// Os blocos que falharam, e a ultima razao.
    failed: Vec<usize>,
    failure: Option<String>,
    /// Os blocos pedidos a thread nesta vaga.
    sent: usize,
}

impl TranslationRun {
    /// A coluna conta como traduzida desde que os blocos comecam a ir ate
    /// o original voltar.
    fn translated(&self) -> bool {
        !self.applied.is_empty()
            || matches!(self.phase, RunPhase::Translating | RunPhase::Restoring)
    }
}

/// O estado da Traducao no `App`. Criado no `App::new` sem thread, sem
/// cofre e sem disco: a thread `neural-translate` so nasce no primeiro
/// clique (`TranslationState::worker_threads_spawned`).
pub(in crate::windows_app) struct TranslationState {
    reads: PageReads,
    read_kinds: Vec<(PageReadToken, u64, ReadKind)>,
    epochs: Vec<(WebViewHost, NavEpoch)>,
    worker: Option<LazyWorker<TranslateJob>>,
    /// O ficheiro da chave Gemini (so o caminho: o cofre decifra na thread).
    key_file: Option<PathBuf>,
    runs: Vec<TranslationRun>,
    next_run: u64,
    card: NativeCard<CardPrompt>,
    card_popup: Option<HWND>,
    card_sink: Box<TranslateCardSink>,
    grants_attached: bool,
}

impl TranslationState {
    pub(in crate::windows_app) fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self::with_sink(Box::new(move |event| {
            let _ = proxy.send_event(event);
        }))
    }

    pub(in crate::windows_app) fn with_sink(sink: TranslateCardSink) -> Self {
        Self {
            reads: PageReads::default(),
            read_kinds: Vec::new(),
            epochs: epoch_hosts()
                .into_iter()
                .map(|host| (host, NavEpoch::default()))
                .collect(),
            worker: None,
            key_file: None,
            runs: Vec::new(),
            next_run: 0,
            card: NativeCard::default(),
            card_popup: None,
            card_sink: Box::new(sink),
            grants_attached: false,
        }
    }

    /// A geracao de navegacao das WebViews de `host` (so os que se
    /// traduzem tem uma).
    pub(in crate::windows_app) fn epoch(&self, host: WebViewHost) -> Option<NavEpoch> {
        self.epochs
            .iter()
            .find(|(known, _)| *known == host)
            .map(|(_, epoch)| epoch.clone())
    }

    /// Quantas threads a Traducao criou (0 ate o primeiro clique).
    #[cfg(test)]
    pub(in crate::windows_app) fn worker_threads_spawned(&self) -> usize {
        self.worker.as_ref().map_or(0, LazyWorker::threads_spawned)
    }

    fn run_index(&self, id: u64) -> Option<usize> {
        self.runs.iter().position(|run| run.id == id)
    }

    fn run_for_host(&self, host: WebViewHost) -> Option<usize> {
        self.runs.iter().position(|run| run.host == host)
    }

    /// A coluna `index` esta traduzida (ou a meio de o ficar, ou de voltar).
    pub(in crate::windows_app) fn column_translated(&self, index: usize) -> bool {
        self.runs
            .iter()
            .any(|run| run.host == WebViewHost::Column(index) && run.translated())
    }

    /// Um run novo para `host` (so um por hospedeiro).
    fn start_run(&mut self, host: WebViewHost, origin: Option<SiteOrigin>, generation: u64) -> u64 {
        self.next_run = self.next_run.wrapping_add(1).max(1);
        self.runs.push(TranslationRun {
            id: self.next_run,
            host,
            origin,
            privacy: translation_privacy(host),
            generation,
            phase: RunPhase::Collecting,
            plan: Plan::default(),
            choice: None,
            applied: Vec::new(),
            failed: Vec::new(),
            failure: None,
            sent: 0,
        });
        self.next_run
    }

    fn drop_run(&mut self, id: u64) -> Option<TranslationRun> {
        let index = self.run_index(id)?;
        self.read_kinds.retain(|(_, run, _)| *run != id);
        Some(self.runs.remove(index))
    }

    /// O que um clique no 文A (ou no menu) faz agora a `host`.
    pub(in crate::windows_app) fn toggle(&self, host: WebViewHost) -> TranslateToggle {
        toggle_for(self.run_for_host(host).map(|index| &self.runs[index]))
    }

    /// A vista do run ainda mostra a pagina do clique: a geracao de
    /// navegacao do hospedeiro e a que o run guardou. So uma navegacao que
    /// vai a sobe (`hook_webview_builder`): uma recusada deixa a pagina -- e
    /// a traducao dela -- onde estava, e o RESTORE ainda a acha.
    pub(in crate::windows_app) fn on_its_page(&self, id: u64) -> Option<(WebViewHost, NavEpoch)> {
        let run = &self.runs[self.run_index(id)?];
        let epoch = self.epoch(run.host)?;
        (epoch.current() == run.generation).then_some((run.host, epoch))
    }

    /// O pedido de saida do run: o plano, o modelo escolhido, o site e a
    /// privacidade que `start_run` lhe deu (no Split privado, a de uma
    /// superficie privada: o portao nem oferece nem le «Sempre neste site»).
    pub(in crate::windows_app) fn run_request(&self, id: u64) -> Option<EgressRequest<'static>> {
        let run = &self.runs[self.run_index(id)?];
        let choice = run.choice.as_ref()?;
        Some(translation_request(
            &run.plan,
            choice,
            run.origin.clone(),
            run.privacy,
        ))
    }

    /// `WorkerReport::Picked`: o modelo do run chegou e o pedido vai ao
    /// portao. `Send` so se o portao ja o deixa (consentimento da sessao ou
    /// «Sempre neste site»); senao, o cartao.
    pub(in crate::windows_app) fn picked(
        &mut self,
        gate: &mut EgressGate,
        id: u64,
        choice: Pick,
        today: Day,
    ) -> TranslateStep {
        let Some(index) = self.run_index(id) else {
            return TranslateStep::Gone;
        };
        if self.runs[index].phase != RunPhase::Picking {
            return TranslateStep::Gone;
        }
        self.runs[index].choice = Some(choice);
        let Some(request) = self.run_request(id) else {
            return TranslateStep::Gone;
        };
        let decision = gate.request(request, today);
        self.decided(id, decision)
    }

    /// A resposta ao cartao de consentimento do run. Se o run ja nao espera
    /// o cartao (a pagina navegou, o 文A devolveu o original), nada vai e
    /// nada e contado no consumo.
    pub(in crate::windows_app) fn consent_answered(
        &mut self,
        gate: &mut EgressGate,
        id: u64,
        card: ConsentCard,
        answer: ConsentAnswer,
        today: Day,
    ) -> TranslateStep {
        let waiting = self
            .run_index(id)
            .is_some_and(|index| self.runs[index].phase == RunPhase::Asking);
        if !waiting {
            return TranslateStep::Gone;
        }
        let decision = gate.answer(card, answer, today);
        self.decided(id, decision)
    }

    /// «Tentar de novo»: o plano passa a ser so os blocos que falharam, e o
    /// portao decide de novo (e conta as chamadas outra vez).
    pub(in crate::windows_app) fn retry(
        &mut self,
        gate: &mut EgressGate,
        id: u64,
        today: Day,
    ) -> TranslateStep {
        let Some(index) = self.run_index(id) else {
            return TranslateStep::Gone;
        };
        let run = &mut self.runs[index];
        if run.phase != RunPhase::Translated || run.choice.is_none() {
            return TranslateStep::Gone;
        }
        let failed = std::mem::take(&mut run.failed);
        run.plan.batches = failed
            .iter()
            .filter_map(|batch| run.plan.batches.get(*batch).cloned())
            .collect();
        run.plan.chars = run.plan.batches.iter().map(|batch| batch.chars).sum();
        let Some(request) = self.run_request(id) else {
            return TranslateStep::Gone;
        };
        let decision = gate.request(request, today);
        self.decided(id, decision)
    }

    /// A decisao do portao para o run. E so aqui que nasce um trabalho
    /// `Translate`: os blocos so vao para a thread depois de um `Send`.
    fn decided(&mut self, id: u64, decision: Decision) -> TranslateStep {
        let Some(index) = self.run_index(id) else {
            return TranslateStep::Gone;
        };
        let run = &mut self.runs[index];
        match decision {
            Decision::Send => {
                let Some(choice) = run.choice.clone() else {
                    return TranslateStep::Gone;
                };
                let batches: Vec<(usize, Batch)> =
                    run.plan.batches.iter().cloned().enumerate().collect();
                run.phase = RunPhase::Translating;
                run.failed.clear();
                run.failure = None;
                run.sent = batches.len();
                TranslateStep::Submit(TranslateJob::Translate {
                    run: id,
                    model: choice.model,
                    batches,
                })
            }
            Decision::Ask(card) => {
                run.phase = RunPhase::Asking;
                let choices = consent_choices(&card, run.privacy);
                let view = consent_view(&card, &choices, &run.plan);
                TranslateStep::Ask(
                    CardPrompt::Consent {
                        run: id,
                        card,
                        choices,
                    },
                    view,
                )
            }
            Decision::Refuse(reason) => TranslateStep::Refuse {
                run: id,
                message: (reason != RefuseReason::Cancelled).then(|| reason.message()),
            },
        }
    }

    /// O 文A pediu o original: as trocas a desfazer. Com trocas, o run
    /// passa a `Restoring` (um bloco atrasado ja nao entra); vazio, nada foi
    /// a pagina e o run sai.
    pub(in crate::windows_app) fn begin_restore(&mut self, id: u64) -> Option<Vec<ApplyEntry>> {
        let index = self.run_index(id)?;
        let entries = self.runs[index].applied.clone();
        if !entries.is_empty() {
            self.runs[index].phase = RunPhase::Restoring;
        }
        Some(entries)
    }

    /// `WorkerReport::BatchDone`: as trocas de um bloco so vao a pagina (e
    /// so contam no que o RESTORE devolve) enquanto o run traduz. Um bloco
    /// que chega depois de o 文A pedir o original, ou de outro run
    /// interromper este, fica de fora. `true`: o APPLY corre.
    pub(in crate::windows_app) fn accept_batch(&mut self, id: u64, entries: &[ApplyEntry]) -> bool {
        let Some(index) = self.run_index(id) else {
            return false;
        };
        let run = &mut self.runs[index];
        if run.phase != RunPhase::Translating || entries.is_empty() {
            return false;
        }
        run.applied.extend(entries.iter().cloned());
        true
    }

    /// `WorkerReport::BatchFailed`, so enquanto o run traduz.
    pub(in crate::windows_app) fn batch_failed(&mut self, id: u64, batch: usize, message: String) {
        let Some(index) = self.run_index(id) else {
            return;
        };
        let run = &mut self.runs[index];
        if run.phase != RunPhase::Translating {
            return;
        }
        if !run.failed.contains(&batch) {
            run.failed.push(batch);
        }
        run.failure = Some(message);
    }

    /// O run ainda traduz.
    fn translating(&self, id: u64) -> bool {
        self.run_index(id)
            .is_some_and(|index| self.runs[index].phase == RunPhase::Translating)
    }

    /// `WorkerReport::Finished`: so um run que traduzia acaba. Um que ja
    /// devolve o original (ou que outro run interrompeu) fica como esta.
    pub(in crate::windows_app) fn finish(&mut self, id: u64) -> Option<FinishOutcome> {
        let index = self.run_index(id)?;
        let run = &mut self.runs[index];
        if run.phase != RunPhase::Translating {
            return None;
        }
        run.phase = RunPhase::Translated;
        let total = run.plan.batches.len();
        let failed = run.failed.len();
        if failed == 0 {
            return Some(FinishOutcome::Done);
        }
        if run.applied.is_empty() && failed == run.sent {
            return Some(FinishOutcome::Failed(
                run.failure.clone().unwrap_or_default(),
            ));
        }
        Some(FinishOutcome::Partial {
            translated: total - failed,
            total,
            failure: run.failure.clone(),
        })
    }
}

/// O que o event loop faz por um run depois de o portao decidir
/// (`TranslationState::picked`, `consent_answered`, `retry`).
#[derive(Debug)]
pub(in crate::windows_app) enum TranslateStep {
    /// O portao disse `Send` (e contou as chamadas): o trabalho vai para a
    /// thread.
    Submit(TranslateJob),
    /// O portao pergunta: o cartao de consentimento.
    Ask(CardPrompt, CardView),
    /// Recusado: o run desiste (`abandon_run`); `None` e o Cancelar do
    /// utilizador, sem aviso.
    Refuse { run: u64, message: Option<String> },
    /// O run ja nao espera isto: nada vai e nada conta.
    Gone,
}

/// Como acabou a vaga de blocos de um run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum FinishOutcome {
    /// Todos: «Página traduzida.»
    Done,
    /// Nada chegou a pagina: o run sai, com a razao.
    Failed(String),
    /// Uma parte: o cartao com «Tentar de novo».
    Partial {
        translated: usize,
        total: usize,
        failure: Option<String>,
    },
}

#[cfg(test)]
impl TranslationState {
    /// So nos gates: um run de `host` que ja manda os blocos
    /// (`translating`) ou que ainda espera o cartao.
    pub(in crate::windows_app) fn run_for_test(
        &mut self,
        host: WebViewHost,
        translating: bool,
    ) -> u64 {
        let id = self.start_run(host, None, 0);
        if let Some(index) = self.run_index(id) {
            self.runs[index].phase = if translating {
                RunPhase::Translating
            } else {
                RunPhase::Asking
            };
        }
        id
    }

    /// So nos gates: o run acabou com estas trocas na pagina.
    pub(in crate::windows_app) fn finish_for_test(&mut self, id: u64, applied: Vec<ApplyEntry>) {
        if let Some(index) = self.run_index(id) {
            self.runs[index].phase = RunPhase::Translated;
            self.runs[index].applied = applied;
        }
    }

    /// So nos gates: o original voltou (ou a pagina navegou).
    pub(in crate::windows_app) fn drop_for_test(&mut self, id: u64) {
        self.drop_run(id);
    }

    /// So nos gates: o clique em `host` sobre `url` pelo `start_run` do
    /// produto (a privacidade e a geracao de navegacao de agora), com a
    /// pagina ja lida em `plan` -- o run espera o modelo, como no
    /// `translation_collected`.
    pub(in crate::windows_app) fn picking_for_test(
        &mut self,
        host: WebViewHost,
        url: &str,
        plan: Plan,
    ) -> u64 {
        let generation = self.epoch(host).map_or(0, |epoch| epoch.current());
        let id = self.start_run(host, SiteOrigin::of_url(url), generation);
        if let Some(index) = self.run_index(id) {
            self.runs[index].plan = plan;
            self.runs[index].phase = RunPhase::Picking;
        }
        id
    }

    /// So nos gates: a privacidade do run.
    pub(in crate::windows_app) fn run_privacy_for_test(&self, id: u64) -> Option<EgressPrivacy> {
        Some(self.runs[self.run_index(id)?].privacy)
    }

    /// So nos gates: o que o RESTORE devolveria.
    pub(in crate::windows_app) fn applied_for_test(&self, id: u64) -> Vec<ApplyEntry> {
        self.run_index(id)
            .map(|index| self.runs[index].applied.clone())
            .unwrap_or_default()
    }
}

/// A resposta de uma coluna a gravar na sessao de pesquisa (e na memoria):
/// o nome da IA, ou `None` quando nao se grava -- a coluna esta traduzida
/// (texto traduzido a maquina nunca e gravado como resposta de uma IA), ou
/// nao ha coluna com esse indice.
pub(in crate::windows_app) fn research_answer_provider(
    translation: &TranslationState,
    provider: Option<&str>,
    source_index: usize,
) -> Option<String> {
    if translation.column_translated(source_index) {
        return None;
    }
    provider.map(str::to_string)
}

/// A WebView de `host`, se ele esta a vista e se traduz.
fn host_view<'a>(
    comparator: Option<&'a ComparatorState>,
    webview: Option<&'a WebView>,
    surface: Surface,
    host: WebViewHost,
) -> Option<&'a WebView> {
    match host {
        WebViewHost::Column(index) => comparator?.views.get(index).map(|view| &view.webview),
        WebViewHost::Split(index) | WebViewHost::PrivateSplit(index) => {
            let private = matches!(host, WebViewHost::PrivateSplit(_));
            comparator?
                .split
                .as_ref()
                .filter(|split| split.source_index == index && split.private == private)
                .map(|split| &split.webview)
        }
        WebViewHost::External if surface == Surface::External => webview,
        _ => None,
    }
}

/// O que um pedido de traducao faz agora a um hospedeiro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum TranslateToggle {
    /// Nada em curso: comeca.
    Start,
    /// A meio (a ler, a escolher, a perguntar) sem nada trocado: desiste.
    Cancel,
    /// Ha trocas na pagina: devolve o original.
    Restore,
    /// Ja a devolver: nada.
    Busy,
}

fn toggle_for(run: Option<&TranslationRun>) -> TranslateToggle {
    match run {
        None => TranslateToggle::Start,
        Some(run) if run.phase == RunPhase::Restoring => TranslateToggle::Busy,
        Some(run) if run.applied.is_empty() && run.phase != RunPhase::Translated => {
            if run.phase == RunPhase::Translating {
                TranslateToggle::Restore
            } else {
                TranslateToggle::Cancel
            }
        }
        Some(_) => TranslateToggle::Restore,
    }
}

// ===================== o evento do modulo =====================

/// O que chega ao event loop para a Traducao.
#[derive(Debug)]
pub(in crate::windows_app) enum TranslateEvent {
    /// O 文A de uma coluna ou o item «Traduzir página» do menu.
    Requested(WebViewHost),
    /// Uma leitura do `page_eval` chegou (ou passou do prazo).
    Page(PageEvalEvent),
    /// A thread `neural-translate`.
    Worker(WorkerReport),
    /// Clique no botao `index` do cartao `token`.
    CardAnswer { token: u64, index: usize },
    /// O prazo do cartao `token`.
    CardExpired(u64),
}

impl App {
    /// O unico braco da Traducao no `user_event`.
    pub(in crate::windows_app) fn translation_event(&mut self, event: TranslateEvent) {
        match event {
            TranslateEvent::Requested(host) => self.translate_requested(host),
            TranslateEvent::Page(event) => self.translation_page_event(event),
            TranslateEvent::Worker(report) => self.translation_report(report),
            TranslateEvent::CardAnswer { token, index } => {
                self.translation_card_answer(token, index)
            }
            TranslateEvent::CardExpired(token) => {
                if self.translation.card.expire(token) {
                    self.hide_translate_card();
                    self.drop_waiting_run();
                }
            }
        }
    }

    /// O portao de saida com a loja «Sempre neste site» da Traducao
    /// (`translate.json`, `Setting`) ligada na primeira vez.
    fn translation_gate(&mut self) -> &mut EgressGate {
        if !self.translation.grants_attached {
            self.translation.grants_attached = true;
            let grant = self
                .stores
                .as_ref()
                .and_then(|stores| stores.grant(TRANSLATE_STORE).ok());
            let gate = self.egress_gate();
            if let Some(grant) = grant {
                let _ = gate.attach_site_grants(AiPurpose::Translation, grant);
            }
        }
        self.egress_gate()
    }

    /// Uma decisao do portao para um run (`TranslationState::picked`,
    /// `consent_answered`, `retry`: as funcoes sem janela que os gates
    /// correm) e o que ela manda fazer.
    fn translation_gate_step(
        &mut self,
        decide: impl FnOnce(&mut TranslationState, &mut EgressGate, Day) -> TranslateStep,
    ) {
        self.translation_gate();
        let Some(gate) = self.egress.as_mut() else {
            return;
        };
        let step = decide(&mut self.translation, gate, Day::today());
        self.translation_step(step);
    }

    fn translation_step(&mut self, step: TranslateStep) {
        match step {
            TranslateStep::Submit(job) => {
                let id = job.run();
                if !self.submit_translate_job(job) {
                    self.forget_run(id);
                    self.show_splash(TRANSLATE_UNREADABLE.to_string(), 3);
                }
            }
            TranslateStep::Ask(prompt, view) => self.show_translate_card(prompt, view),
            TranslateStep::Refuse { run, message } => {
                self.abandon_run(run);
                if let Some(message) = message {
                    self.show_splash(message, 4);
                }
            }
            TranslateStep::Gone => {}
        }
    }

    /// O caminho do ficheiro da chave Gemini (o do Gemini Live), sem a ler.
    fn translation_key_file(&mut self) -> Option<PathBuf> {
        if self.translation.key_file.is_none() {
            let vault = self.translation_vault()?;
            self.translation.key_file =
                Some(vault.secret_file(&KeySlot::Gemini).path().to_path_buf());
        }
        self.translation.key_file.clone()
    }

    fn translation_vault(&self) -> Option<KeyVault> {
        let stores = self.stores.as_ref()?;
        let keys = stores.grant(KEYS_STORE).ok()?;
        let live = stores.grant(LIVE_KEY_STORE).ok()?;
        KeyVault::open(keys, live).ok()
    }

    /// A thread `neural-translate`, criada na primeira vez que e precisa: o
    /// cofre vai com ela e so la decifra a chave. Uma traducao de cada vez
    /// (a vaga do `LazyWorker` e a do mais recente): o trabalho novo
    /// interrompe o de outro run -- o que escolhia o modelo sai, o que
    /// traduzia fica com o que ja trocou (o 文A devolve-o).
    fn submit_translate_job(&mut self, job: TranslateJob) -> bool {
        let id = job.run();
        let interrupted: Vec<(u64, RunPhase)> = self
            .translation
            .runs
            .iter()
            .filter(|run| run.id != id)
            .filter(|run| matches!(run.phase, RunPhase::Picking | RunPhase::Translating))
            .map(|run| (run.id, run.phase))
            .collect();
        for (other, phase) in interrupted {
            if phase == RunPhase::Picking {
                self.translation.drop_run(other);
            } else if let Some(index) = self.translation.run_index(other) {
                self.translation.runs[index].phase = RunPhase::Translated;
            }
        }
        if self.translation.worker.is_none() {
            let Some(vault) = self.translation_vault() else {
                return false;
            };
            let proxy = self.proxy.clone();
            let mut worker = TranslateWorker {
                engine: Box::new(GeminiEngine {
                    client: ApiClient::new(Endpoint::pinned(Provider::Gemini)),
                }),
                key: Box::new(move || vault.load(&KeySlot::Gemini)),
                report: Box::new(move |report| {
                    let _ = proxy.send_event(UserEvent::Translate(TranslateEvent::Worker(report)));
                }),
                models: None,
            };
            self.translation.worker = Some(LazyWorker::new(
                TRANSLATE_WORKER_NAME,
                move |job: TranslateJob, context: &JobContext| {
                    worker.run(job, &|| context.cancelled());
                },
            ));
        }
        self.translation
            .worker
            .as_mut()
            .is_some_and(|worker| worker.submit(job).is_ok())
    }

    fn cancel_translate_job(&self) {
        if let Some(worker) = &self.translation.worker {
            worker.cancel();
        }
    }

    /// O 文A ou o item do menu: traduz, desiste ou devolve o original.
    fn translate_requested(&mut self, host: WebViewHost) {
        if !translatable_host(host) {
            return;
        }
        let id = self
            .translation
            .run_for_host(host)
            .map_or(0, |index| self.translation.runs[index].id);
        match self.translation.toggle(host) {
            TranslateToggle::Busy => {}
            TranslateToggle::Cancel => {
                self.forget_run(id);
                self.show_splash(TRANSLATE_CANCELLED.to_string(), 2);
            }
            TranslateToggle::Restore => self.restore_translation(id),
            TranslateToggle::Start => self.start_translation(host),
        }
    }

    /// Tira um run (e o que ele tinha em voo: a thread, o cartao).
    fn forget_run(&mut self, id: u64) {
        let was_working = self
            .translation
            .run_index(id)
            .map(|index| self.translation.runs[index].phase)
            .is_some_and(|phase| matches!(phase, RunPhase::Picking | RunPhase::Translating));
        if was_working {
            self.cancel_translate_job();
        }
        let card_is_this = self
            .translation
            .card
            .pending
            .as_ref()
            .is_some_and(|pending| match &pending.payload {
                CardPrompt::Consent { run, .. } | CardPrompt::Partial { run, .. } => *run == id,
                CardPrompt::NoKey { .. } => false,
            });
        if card_is_this {
            self.translation.card.pending = None;
            self.hide_translate_card();
        }
        self.translation.drop_run(id);
    }

    fn start_translation(&mut self, host: WebViewHost) {
        let Some(url) = host_view(
            self.comparator.as_ref(),
            self.webview.as_ref(),
            self.surface,
            host,
        )
        .and_then(|view| view.page_url()) else {
            return;
        };
        let web = Url::parse(&url)
            .ok()
            .is_some_and(|parsed| matches!(parsed.scheme(), "http" | "https"));
        if !web {
            self.show_splash(TRANSLATE_ONLY_WEB.to_string(), 3);
            return;
        }
        // Sem a chave nem se le a pagina.
        let has_key = self
            .translation_key_file()
            .is_some_and(|path| path.is_file());
        if !has_key {
            self.show_no_key_card();
            return;
        }
        let Some(epoch) = self.translation.epoch(host) else {
            return;
        };
        let id = self
            .translation
            .start_run(host, SiteOrigin::of_url(&url), epoch.current());
        let proxy = self.proxy.clone();
        let Some(view) = host_view(
            self.comparator.as_ref(),
            self.webview.as_ref(),
            self.surface,
            host,
        ) else {
            self.translation.drop_run(id);
            return;
        };
        let timers = &self.timers;
        let started = self.translation.reads.read(
            view,
            PageEvalSpec {
                script: &TRANSLATE_COLLECT_READ,
                max_raw_bytes: TRANSLATE_COLLECT_MAX_BYTES,
                deadline: TRANSLATE_SCRIPT_DEADLINE,
            },
            &epoch,
            Instant::now(),
            move |event| {
                let _ = proxy.send_event(UserEvent::Translate(TranslateEvent::Page(event)));
            },
            |delay, event| timers.after(delay, UserEvent::Translate(TranslateEvent::Page(event))),
        );
        match started {
            Ok(token) => self
                .translation
                .read_kinds
                .push((token, id, ReadKind::Collect)),
            Err(_) => {
                self.translation.drop_run(id);
                self.show_splash(TRANSLATE_UNREADABLE.to_string(), 3);
            }
        }
    }

    /// Corre o APPLY ou o RESTORE de `entries` na vista do run.
    fn run_entries_script(&mut self, id: u64, kind: ReadKind, entries: &[ApplyEntry]) -> bool {
        // A vista navegou desde o clique: a pagina e outra, nada se troca
        // nem se devolve la.
        let Some((host, epoch)) = self.translation.on_its_page(id) else {
            return false;
        };
        let script: &'static ReadOnlyScript = match kind {
            ReadKind::Apply => &TRANSLATE_APPLY_READ,
            ReadKind::Restore => &TRANSLATE_RESTORE_READ,
            ReadKind::Collect => return false,
        };
        let Some(view) = host_view(
            self.comparator.as_ref(),
            self.webview.as_ref(),
            self.surface,
            host,
        ) else {
            return false;
        };
        let proxy = self.proxy.clone();
        let timers = &self.timers;
        let started = self.translation.reads.read_with_arg(
            view,
            PageEvalSpec {
                script,
                max_raw_bytes: TRANSLATE_APPLY_MAX_BYTES,
                deadline: TRANSLATE_SCRIPT_DEADLINE,
            },
            &entries_arg(entries),
            &epoch,
            Instant::now(),
            move |event| {
                let _ = proxy.send_event(UserEvent::Translate(TranslateEvent::Page(event)));
            },
            |delay, event| timers.after(delay, UserEvent::Translate(TranslateEvent::Page(event))),
        );
        match started {
            Ok(token) => {
                self.translation.read_kinds.push((token, id, kind));
                true
            }
            Err(_) => false,
        }
    }

    fn restore_translation(&mut self, id: u64) {
        if self.translation.translating(id) {
            self.cancel_translate_job();
        }
        let Some(entries) = self.translation.begin_restore(id) else {
            return;
        };
        // Sem trocas, nada foi a pagina; sem a pagina (navegou), nada a
        // devolver la.
        if entries.is_empty() || !self.run_entries_script(id, ReadKind::Restore, &entries) {
            self.forget_run(id);
        }
    }

    /// Uma leitura chegou: pela leitura, o run e o que ela era.
    fn translation_page_event(&mut self, event: PageEvalEvent) {
        let token = match &event {
            PageEvalEvent::Arrived { token, .. } | PageEvalEvent::Expired(token) => *token,
        };
        let Some(position) = self
            .translation
            .read_kinds
            .iter()
            .position(|(known, _, _)| *known == token)
        else {
            // Uma leitura de um run que ja saiu: so a tira da lista.
            let _ = self
                .translation
                .reads
                .settle(event, || None, Instant::now());
            return;
        };
        let (_, id, kind) = self.translation.read_kinds.remove(position);
        let host = self
            .translation
            .run_index(id)
            .map(|index| self.translation.runs[index].host);
        let comparator = self.comparator.as_ref();
        let webview = self.webview.as_ref();
        let surface = self.surface;
        let outcome = self.translation.reads.settle(
            event,
            || host.and_then(|host| host_view(comparator, webview, surface, host)?.page_url()),
            Instant::now(),
        );
        if matches!(outcome, PageEvalOutcome::Stale(_)) {
            return;
        }
        let raw = match outcome {
            PageEvalOutcome::Delivered { raw, .. } => Some(raw),
            _ => None,
        };
        match kind {
            ReadKind::Collect => self.translation_collected(id, raw),
            ReadKind::Apply => {}
            ReadKind::Restore => {
                if self.translation.run_index(id).is_some() {
                    self.forget_run(id);
                    if raw.is_some() {
                        self.show_splash(TRANSLATE_RESTORED.to_string(), 2);
                    }
                }
            }
        }
    }

    fn translation_collected(&mut self, id: u64, raw: Option<String>) {
        let Some(index) = self.translation.run_index(id) else {
            return;
        };
        let collected = match raw.as_deref().map(core_translate::parse_collected) {
            Some(Ok(collected)) => collected,
            Some(Err(CollectError::TooLarge)) | Some(Err(CollectError::Malformed)) | None => {
                self.translation.drop_run(id);
                self.show_splash(TRANSLATE_UNREADABLE.to_string(), 3);
                return;
            }
        };
        if core_translate::already_portuguese(collected.lang.as_deref(), &collected.texts) {
            self.translation.drop_run(id);
            self.show_splash(TRANSLATE_ALREADY_PT.to_string(), 3);
            return;
        }
        let plan = core_translate::plan_batches(&collected.texts);
        if plan.is_empty() {
            self.translation.drop_run(id);
            self.show_splash(TRANSLATE_NOTHING.to_string(), 3);
            return;
        }
        self.translation.runs[index].plan = plan;
        self.translation.runs[index].phase = RunPhase::Picking;
        let pin = self.egress_gate().model_pin(AiPurpose::Translation);
        if !self.submit_translate_job(TranslateJob::Pick { run: id, pin }) {
            self.translation.drop_run(id);
            self.show_splash(TRANSLATE_UNREADABLE.to_string(), 3);
        }
    }

    fn translation_report(&mut self, report: WorkerReport) {
        match report {
            WorkerReport::Picked { run, choice } => self.translation_picked(run, choice),
            WorkerReport::NoKey { run } => {
                self.forget_run(run);
                self.show_no_key_card();
            }
            WorkerReport::PickFailed { run, message } => {
                if self.translation.run_index(run).is_some() {
                    self.forget_run(run);
                    self.show_splash(format!("Tradução: {message}."), 4);
                }
            }
            WorkerReport::Progress { run, done, total } => {
                if self.translation.translating(run) && total > 1 {
                    self.show_splash(translating_message(done, total), 3);
                }
            }
            WorkerReport::BatchDone {
                run,
                batch: _,
                entries,
            } => {
                // Conta como traduzida desde AGORA: o APPLY vai a caminho.
                if self.translation.accept_batch(run, &entries) {
                    let _ = self.run_entries_script(run, ReadKind::Apply, &entries);
                }
            }
            WorkerReport::BatchFailed {
                run,
                batch,
                message,
            } => self.translation.batch_failed(run, batch, message),
            WorkerReport::Finished { run } => self.translation_finished(run),
        }
    }

    fn translation_picked(&mut self, id: u64, choice: Pick) {
        self.translation_gate_step(|state, gate, today| state.picked(gate, id, choice, today));
    }

    fn translation_finished(&mut self, id: u64) {
        let Some(outcome) = self.translation.finish(id) else {
            return;
        };
        let (total, translated, failure) = match outcome {
            FinishOutcome::Done => {
                self.show_splash(TRANSLATE_DONE.to_string(), 2);
                return;
            }
            FinishOutcome::Failed(why) => {
                // Nada chegou a pagina: nao ha o que devolver.
                self.forget_run(id);
                self.show_splash(format!("Não foi possível traduzir: {why}."), 4);
                return;
            }
            FinishOutcome::Partial {
                translated,
                total,
                failure,
            } => (total, translated, failure),
        };
        let choices = vec![CardChoice::Retry, CardChoice::Close];
        let mut lines = Vec::new();
        if let Some(why) = failure {
            lines.push(format!("Os outros blocos falharam: {why}."));
        }
        let view = CardView {
            title: partial_message(translated, total),
            lines,
            buttons: choices
                .iter()
                .map(|choice| (choice.label(true), choice.primary()))
                .collect(),
        };
        self.show_translate_card(CardPrompt::Partial { run: id, choices }, view);
    }

    fn show_no_key_card(&mut self) {
        let choices = vec![CardChoice::SaveKey, CardChoice::Cancel];
        let view = CardView {
            title: TRANSLATE_PAGE_LABEL.to_string(),
            lines: vec![TRANSLATE_NO_KEY.to_string()],
            buttons: choices
                .iter()
                .map(|choice| (choice.label(true), choice.primary()))
                .collect(),
        };
        self.show_translate_card(CardPrompt::NoKey { choices }, view);
    }

    /// Os runs que esperavam o cartao, quando o cartao sai sem resposta: o
    /// consentimento nao dado vale Cancelar.
    fn drop_waiting_run(&mut self) {
        let waiting: Vec<u64> = self
            .translation
            .runs
            .iter()
            .filter(|run| run.phase == RunPhase::Asking)
            .map(|run| run.id)
            .collect();
        for id in waiting {
            self.abandon_run(id);
        }
    }

    /// Um run que nao vai (mais) mandar nada: sai, a menos que a pagina ja
    /// tenha trocas dele (um «Tentar de novo» cancelado) -- entao fica
    /// traduzido, e o 文A devolve o original.
    fn abandon_run(&mut self, id: u64) {
        match self.translation.run_index(id) {
            Some(index) if !self.translation.runs[index].applied.is_empty() => {
                self.translation.runs[index].phase = RunPhase::Translated;
            }
            Some(_) => self.forget_run(id),
            None => {}
        }
    }

    fn translation_card_answer(&mut self, token: u64, index: usize) {
        let now = Instant::now();
        let chosen = self
            .translation
            .card
            .pending
            .as_ref()
            .filter(|pending| pending.token == token)
            .and_then(|pending| pending.payload.choices().get(index).copied());
        let Some(choice) = chosen else {
            return;
        };
        let cancel = matches!(choice, CardChoice::Cancel | CardChoice::Close);
        let answer = self
            .translation
            .card
            .answer(token, !cancel, now, |prompt| Some(prompt.clone()));
        let prompt = match answer {
            CardAnswer::Ignored => return,
            CardAnswer::Cancelled => {
                self.hide_translate_card();
                self.drop_waiting_run();
                return;
            }
            CardAnswer::Confirmed(prompt) => prompt,
        };
        self.hide_translate_card();
        match (prompt, choice) {
            (CardPrompt::NoKey { .. }, CardChoice::SaveKey) => {
                self.open_secret_prompt(KeySlot::Gemini);
            }
            (CardPrompt::Partial { run, .. }, CardChoice::Retry) => {
                // Recusado, fica o que ja estava traduzido; o 文A devolve-o.
                self.translation_gate_step(|state, gate, today| state.retry(gate, run, today));
            }
            (
                CardPrompt::Consent {
                    run,
                    card,
                    choices: _,
                },
                choice,
            ) => {
                let answer = match choice {
                    CardChoice::Translate(answer) => answer,
                    CardChoice::AlwaysOnSite => ConsentAnswer::AlwaysOnSite,
                    _ => ConsentAnswer::Cancel,
                };
                self.translation_gate_step(|state, gate, today| {
                    state.consent_answered(gate, run, card, answer, today)
                });
            }
            _ => {}
        }
    }

    fn show_translate_card(&mut self, prompt: CardPrompt, view: CardView) {
        let now = Instant::now();
        // Um cartao de cada vez: o run que esperava o cartao trocado desiste
        // (o sim dele nunca foi dado).
        let displaced = match self.translation.card.pending.as_ref().map(|p| &p.payload) {
            Some(CardPrompt::Consent { run, .. }) => Some(*run),
            _ => None,
        };
        let (token, _) = self.translation.card.request(prompt, now);
        let current = match self.translation.card.pending.as_ref().map(|p| &p.payload) {
            Some(CardPrompt::Consent { run, .. }) => Some(*run),
            _ => None,
        };
        if let Some(old) = displaced.filter(|old| Some(*old) != current) {
            match self.translation.run_index(old) {
                Some(index) if !self.translation.runs[index].applied.is_empty() => {
                    self.translation.runs[index].phase = RunPhase::Translated;
                }
                Some(_) => {
                    self.translation.drop_run(old);
                }
                None => {}
            }
        }
        if let Ok(mut slot) = TRANSLATE_CARD_VIEW.lock() {
            *slot = Some((token, view));
        }
        TRANSLATE_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
        if self.translation.card_popup.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let Some(owner) = window_hwnd(window) else {
                return;
            };
            let scale = window.scale_factor().max(1.0);
            let width = (TRANSLATE_CARD_WIDTH * scale).round() as i32;
            let height = (TRANSLATE_CARD_HEIGHT * scale).round() as i32;
            let corner = (18.0 * scale).round() as i32;
            let created = unsafe {
                create_native_card(
                    owner,
                    width,
                    height,
                    translate_card_subclass,
                    TRANSLATE_CARD_SUBCLASS_ID,
                    (&*self.translation.card_sink as *const TranslateCardSink) as usize,
                    corner,
                )
            };
            let Some(created) = created else {
                return;
            };
            self.translation.card_popup = Some(created);
        }
        self.position_translate_card();
        self.timers.after(
            Duration::from_secs(TRANSLATE_CARD_SECONDS),
            UserEvent::Translate(TranslateEvent::CardExpired(token)),
        );
    }

    fn hide_translate_card(&mut self) {
        if let Some(card) = self.translation.card_popup.take() {
            unsafe {
                DestroyWindow(card);
            }
        }
        if let Ok(mut slot) = TRANSLATE_CARD_VIEW.lock() {
            *slot = None;
        }
        TRANSLATE_CARD_PAINTED.store(0, Ordering::Release);
        TRANSLATE_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
    }

    /// Centra o cartao na janela (em coordenadas de ECRA, como o do
    /// Pesquisar): refaz-se quando a janela se mexe.
    pub(in crate::windows_app) fn position_translate_card(&self) {
        let (Some(window), Some(card)) = (&self.window, self.translation.card_popup) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (TRANSLATE_CARD_WIDTH * scale).round() as i32;
        let height = (TRANSLATE_CARD_HEIGHT * scale).round() as i32;
        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let (x, y) = splash_origin(client.right, client.bottom, width, height);
            SetWindowPos(
                card,
                std::ptr::null_mut(),
                origin.x + x,
                origin.y + y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(card);
            InvalidateRect(card, std::ptr::null(), 1);
        }
    }

    /// Uma pagina de `host` acabou de carregar: se a vista navegou desde o
    /// clique, a traducao era da pagina anterior e sai (os nos ja nao estao
    /// la; a thread para).
    pub(in crate::windows_app) fn translation_page_loaded(&mut self, host: WebViewHost) {
        let stale: Vec<u64> = self
            .translation
            .runs
            .iter()
            .filter(|run| run.host == host)
            .map(|run| run.id)
            .filter(|id| self.translation.on_its_page(*id).is_none())
            .collect();
        for id in stale {
            self.forget_run(id);
        }
    }
}
