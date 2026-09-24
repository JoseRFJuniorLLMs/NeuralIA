use super::*;

/// Para onde o cartao manda a resposta: o proxy do event loop no app, um
/// registo nos gates. Em caixa dupla: o `reference_data` da subclasse e um
/// ponteiro fino.
pub(super) type SearchCardSink = Box<dyn Fn(UserEvent)>;

/// Os dois botoes do cartao: o de confirmar ("Mandar", "Traduzir") e o
/// Cancelar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchCardButton {
    Confirm,
    Cancel,
}

impl SearchCardButton {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Confirm => 0,
            Self::Cancel => 1,
        }
    }

    pub(super) fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Confirm),
            1 => Some(Self::Cancel),
            _ => None,
        }
    }

    /// O rotulo no cartao do botao da barra que o pediu.
    pub(super) fn label(self, intent: SearchIntent) -> &'static str {
        match (self, intent) {
            (Self::Confirm, SearchIntent::Ask) => "Mandar",
            (Self::Confirm, SearchIntent::Translate) => "Traduzir",
            (Self::Cancel, _) => "Cancelar",
        }
    }
}

/// O titulo do cartao, pelo botao da barra que o pediu.
pub(super) fn search_card_title(intent: SearchIntent) -> &'static str {
    match intent {
        SearchIntent::Ask => "Mandar para as 3 IAs?",
        SearchIntent::Translate => "Traduzir nas 3 IAs?",
    }
}

/// O que as tres IAs recebem depois do clique em confirmar, a partir do que
/// o cartao pintou (`seen`): a pergunta tal e qual, ou o pedido fixo de
/// traducao, uma linha em branco e o texto. O pedido e sempre este, escrito
/// aqui: a pagina so escolhe o botao.
pub(super) fn selection_prompt(intent: SearchIntent, seen: &str) -> String {
    match intent {
        SearchIntent::Ask => seen.to_string(),
        SearchIntent::Translate => format!("{TRANSLATE_PROMPT}\n\n{seen}"),
    }
}

/// O comando da omnibox que traduz nas tres IAs: e o que o Historico guarda
/// de um Traduzir, e o clique la refaz o pedido (`route_input`).
pub(super) const TRANSLATE_COMMAND: &str = "traduzir:";

/// Uma comparacao nas tres IAs: o que elas recebem (`prompt`) e como fica no
/// Historico, na memoria e na sessao de pesquisa. Numa pergunta e tudo o
/// mesmo texto. No Traduzir as IAs recebem o pedido fixo, mas o nome e o do
/// texto de quem le ("Traduzir: <texto>") -- o pedido fixo a frente deixava
/// todas as traducoes com o mesmo titulo -- e o Historico guarda
/// `traduzir:<texto>`, que reabre refazendo o pedido (com o pedido inteiro,
/// um texto longo passava do tecto do painel e ja nao reabria).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompareRequest {
    /// O que as tres IAs recebem.
    pub(super) prompt: String,
    /// O nome da sessao de pesquisa e da memoria.
    pub(super) label: String,
    /// O que o Historico guarda e o clique la volta a abrir (`handle_input`).
    pub(super) reopen: String,
}

impl CompareRequest {
    /// Uma pergunta: o mesmo texto para as IAs, o nome e o Historico.
    pub(super) fn ask(query: String) -> Self {
        Self {
            label: query.clone(),
            reopen: format!("compare:{query}"),
            prompt: query,
        }
    }

    /// O Traduzir de `text` (`selection_prompt`).
    pub(super) fn translate(text: &str) -> Self {
        Self {
            prompt: selection_prompt(SearchIntent::Translate, text),
            label: format!("Traduzir: {text}"),
            reopen: format!("{TRANSLATE_COMMAND}{text}"),
        }
    }

    /// O que o clique em confirmar no cartao manda: o texto que ele pintou,
    /// pelo botao da barra que o pediu.
    pub(super) fn selection(intent: SearchIntent, seen: &str) -> Self {
        match intent {
            SearchIntent::Ask => Self::ask(seen.to_string()),
            SearchIntent::Translate => Self::translate(seen),
        }
    }
}

/// O que um `compare` deixa, sem janela: a sessao de pesquisa (com o nome
/// de `label`), a memoria da pergunta e a entrada do Historico.
pub(super) fn compare_records(
    request: &CompareRequest,
) -> (ResearchSession, MemoryDocument, String) {
    let session = ResearchSession::new(request.prompt.clone()).titled(&request.label);
    let memory = MemoryDocument::new(
        MemoryKind::ResearchResult,
        MemorySourceKind::Note,
        format!("Pesquisa · {}", session.title),
        None,
        request.prompt.clone(),
    )
    .session(session.id.clone());
    (session, memory, request.reopen.clone())
}

/// O texto do cartao e texto simples: DrawTextW com DT_NOPREFIX (um "&" da
/// pagina e um "&", nao um sublinhado), quebra por palavras e, numa palavra
/// maior que a linha, por caracteres. Sem DT_END_ELLIPSIS: o corte e o de
/// `search_card_fit`, medido, nunca um que o GDI faca em silencio.
pub(super) const SEARCH_CARD_TEXT_FORMAT: u32 = DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX;

/// Onde fica cada coisa no cartao, em pixeis do cliente. Uma so funcao para o
/// desenho e o clique concordarem sempre.
pub(super) struct SearchCardLayout {
    pub(super) title: RECT,
    /// A caixa do texto e, dentro dela, o texto.
    pub(super) quote: RECT,
    pub(super) text: RECT,
    /// A conta do que ficou de fora, a esquerda dos botoes.
    pub(super) note: RECT,
    pub(super) search: RECT,
    pub(super) cancel: RECT,
}

pub(super) fn search_card_scale(client: &RECT) -> f64 {
    ((client.bottom - client.top) as f64 / SEARCH_CARD_HEIGHT).max(1.0)
}

pub(super) fn search_card_layout(client: &RECT, scale: f64) -> SearchCardLayout {
    let px = |value: f64| (value * scale).round() as i32;
    let pad = px(24.0);
    let right = client.right - pad;
    let bottom = client.bottom - px(20.0);
    let top = bottom - px(40.0);
    let cancel = RECT {
        left: right - px(124.0),
        top,
        right,
        bottom,
    };
    let search = RECT {
        left: cancel.left - px(12.0) - px(140.0),
        top,
        right: cancel.left - px(12.0),
        bottom,
    };
    let title = RECT {
        left: client.left + pad,
        top: client.top + px(16.0),
        right,
        bottom: client.top + px(50.0),
    };
    let quote = RECT {
        left: title.left,
        top: title.bottom + px(6.0),
        right,
        bottom: top - px(14.0),
    };
    let text = RECT {
        left: quote.left + px(12.0),
        top: quote.top + px(8.0),
        right: quote.right - px(12.0),
        bottom: quote.bottom - px(8.0),
    };
    let note = RECT {
        left: title.left,
        top,
        right: search.left - px(12.0),
        bottom,
    };
    SearchCardLayout {
        title,
        quote,
        text,
        note,
        search,
        cancel,
    }
}

/// O botao do cartao debaixo de (x, y); bordas semiabertas, como o resto da UI.
pub(super) fn search_card_hit(
    client: &RECT,
    scale: f64,
    x: i32,
    y: i32,
) -> Option<SearchCardButton> {
    let layout = search_card_layout(client, scale);
    let inside = |rect: &RECT| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
    if inside(&layout.search) {
        Some(SearchCardButton::Confirm)
    } else if inside(&layout.cancel) {
        Some(SearchCardButton::Cancel)
    } else {
        None
    }
}

/// O clique que o cartao aceita: o botao desceu E subiu no mesmo botao, com o
/// cartao a segurar o rato desde que desceu. Arrastar de fora para cima do
/// confirmar, ou premir e sair, nao responde nada.
pub(super) fn search_card_release(
    pressed: Option<usize>,
    captured: bool,
    client: &RECT,
    x: i32,
    y: i32,
) -> Option<SearchCardButton> {
    if !captured {
        return None;
    }
    let released = search_card_hit(client, search_card_scale(client), x, y);
    native_release_matches(pressed, released.map(SearchCardButton::index))
        .and_then(SearchCardButton::from_index)
}

/// Caracteres que o cartao pintaria como nada (ou que mudam a ordem do que
/// pinta) e que as IAs leem na mesma: os Default_Ignorable_Code_Point do
/// Unicode -- tags U+E0000.. ("ASCII smuggling"), seletores de variacao,
/// ZWSP/ZWJ, marcas e controlos bidi, soft hyphen, BOM, preenchimentos
/// Hangul --, o braille vazio, as ancoras de anotacao e o U+FFFC, a area
/// privada e os nao-caracteres. A pagina escolhe o texto; nao escolhe mandar
/// as IAs uma coisa que o cartao nao mostra.
pub(super) fn invisible_in_card(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{E000}'..='\u{F8FF}'
            | '\u{FDD0}'..='\u{FDEF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFFC}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
            | '\u{F0000}'..='\u{10FFFF}'
    ) || (c as u32 & 0xFFFE) == 0xFFFE
}

/// A pergunta que o "Mandar para IA" (ou o texto que o "Traduzir") leva, tal
/// como o cartao a mostra: sem os
/// caracteres invisiveis, com quebras de linha, tabs e outros espacos ou
/// controlos reduzidos a um espaco, aparada. E ESTE texto -- nao o que a
/// pagina mandou -- que o cartao pinta e que a confirmacao leva.
pub(super) fn selection_question(text: &str) -> String {
    let mut question = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if invisible_in_card(c) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            gap = !question.is_empty();
            continue;
        }
        if gap {
            question.push(' ');
            gap = false;
        }
        question.push(c);
    }
    question
}

/// Os primeiros `shown` caracteres de `text`, sem o espaco do fim: o que o
/// cartao pintou e, portanto, tudo o que um clique em confirmar leva.
pub(super) fn search_card_shown(text: &str, shown: usize) -> &str {
    let end = text
        .char_indices()
        .nth(shown)
        .map_or(text.len(), |(at, _)| at);
    text[..end].trim_end()
}

/// O que se pinta na caixa: o texto inteiro, ou o inicio que coube e "…".
pub(super) fn search_card_body(text: &str, shown: usize) -> String {
    let part = search_card_shown(text, shown);
    if part == text.trim_end() {
        part.to_string()
    } else {
        format!("{part}…")
    }
}

/// A conta, debaixo da caixa, do que nao coube (e por isso nao vai).
pub(super) fn search_card_left_out(chars: usize) -> String {
    if chars == 1 {
        "+1 caractere fica de fora".to_string()
    } else {
        format!("+{chars} caracteres ficam de fora")
    }
}

/// Quantos caracteres de `text` cabem na caixa `width` x `height`, medidos
/// com DT_CALCRECT no `hdc` (com a fonte do texto ja escolhida) e o mesmo
/// formato do desenho: todos, se cabem; senao o maior inicio que cabe com o
/// "…" (cortado no ultimo espaco, se estiver perto). Uma linha mais larga que
/// a caixa (uma palavra que o GDI nao partisse) conta como nao caber.
pub(super) unsafe fn search_card_fit(
    hdc: *mut core::ffi::c_void,
    text: &str,
    width: i32,
    height: i32,
) -> usize {
    let fits = |shown: usize| {
        let wide: Vec<u16> = search_card_body(text, shown).encode_utf16().collect();
        if wide.is_empty() {
            return true;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: 0,
        };
        let height_used = DrawTextW(
            hdc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rect,
            SEARCH_CARD_TEXT_FORMAT | DT_CALCRECT,
        );
        height_used > 0 && rect.right - rect.left <= width && rect.bottom - rect.top <= height
    };
    // Mede-se de inicios cada vez maiores, nunca o texto todo de uma vez: o
    // GDI parte palavras enormes (e CJK) devagar, e 2000 caracteres assim
    // custavam centenas de ms numa pintura. fits(low) e !fits(high); o "…"
    // sozinho cabe em qualquer cartao.
    let total = text.chars().count();
    let (mut low, mut high) = (0, total.min(128));
    while fits(high) {
        if high == total {
            return total;
        }
        low = high;
        high = (high * 2).min(total);
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    let prefix: Vec<char> = text.chars().take(low).collect();
    match prefix.iter().rposition(|c| *c == ' ') {
        Some(space) if low - space <= 24 => space,
        _ => low,
    }
}

/// "Abrir" e "Nao" no canto direito do aviso do Gmail, em pixeis do cliente.
pub(super) fn gmail_toast_buttons(client: &RECT, scale: f64) -> (RECT, RECT) {
    let height = (26.0 * scale).round() as i32;
    let top = (client.bottom - height) / 2;
    let gap = (6.0 * scale).round() as i32;
    let right = client.right - (12.0 * scale).round() as i32;
    let no_width = (54.0 * scale).round() as i32;
    let open_width = (70.0 * scale).round() as i32;
    let no = RECT {
        left: right - no_width,
        top,
        right,
        bottom: top + height,
    };
    let open = RECT {
        left: no.left - gap - open_width,
        top,
        right: no.left - gap,
        bottom: top + height,
    };
    (open, no)
}

/// Log de depuracao em tempo de execucao, pedido pelo dono para achar bugs
/// intermitentes. Desligado por padrao; `NEURALIA_DEBUG_LOG=<ficheiro>` liga.
/// Cada linha: milissegundos desde o arranque e o evento. Nunca leva URLs,
/// texto de paginas nem nada da memoria -- so transicoes da janela.
pub(super) fn debug_log(event: std::fmt::Arguments<'_>) {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let Some(path) = std::env::var_os("NEURALIA_DEBUG_LOG") else {
        return;
    };
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    append_debug_line(std::path::Path::new(&path), elapsed, event);
}

/// Acrescenta uma linha ao log; um log que nao abre nunca derruba o app.
pub(super) fn append_debug_line(
    path: &std::path::Path,
    elapsed_ms: u128,
    event: std::fmt::Arguments<'_>,
) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // A linha passa pela redacao antes do disco: se alguma um dia levar
        // a chave do Gemini Live, sai com um marcador no lugar dela.
        let line = event.to_string();
        let _ = writeln!(file, "{elapsed_ms:>8} ms  {}", redact_debug_secrets(&line));
    }
}

pub(super) fn show_popup_without_activation(hwnd: HWND) {
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// Um cartao de cada vez: pendente -> confirmado, cancelado, expirado ou
/// trocado por um pedido novo.
#[derive(Default)]
pub(super) struct SearchCard {
    pub(super) pending: Option<PendingSearch>,
    pub(super) last_token: u64,
}

impl SearchCard {
    pub(super) fn step(&mut self, input: SearchCardInput, now: Instant) -> SearchCardOutcome {
        match input {
            SearchCardInput::Request { text, intent } => {
                let Some(SelectionSearch::Compare(question)) = selection_search(&text) else {
                    return SearchCardOutcome::Ignored;
                };
                self.last_token = self.last_token.wrapping_add(1).max(1);
                let token = self.last_token;
                let text = question.clone();
                let replaced = self
                    .pending
                    .replace(PendingSearch {
                        token,
                        intent,
                        question,
                        shown_at: now,
                    })
                    .is_some();
                SearchCardOutcome::Show {
                    token,
                    intent,
                    text,
                    replaced,
                }
            }
            SearchCardInput::Answer {
                token,
                button,
                shown,
            } => {
                let Some(pending) = self.pending.take_if(|pending| pending.token == token) else {
                    return SearchCardOutcome::Ignored;
                };
                // O que o cartao pintou deste texto: o resto nao se viu e nao
                // vai, por mais que a pagina o tenha posto la.
                let seen = search_card_shown(&pending.question, shown);
                match button {
                    SearchCardButton::Cancel => SearchCardOutcome::Cancelled,
                    SearchCardButton::Confirm
                        if !seen.is_empty()
                            && now.saturating_duration_since(pending.shown_at)
                                >= SEARCH_CARD_ARM =>
                    {
                        SearchCardOutcome::Confirmed(CompareRequest::selection(
                            pending.intent,
                            seen,
                        ))
                    }
                    SearchCardButton::Confirm => {
                        // Cedo demais, ou nada a vista: o cartao fica, a
                        // espera de um clique a serio.
                        self.pending = Some(pending);
                        SearchCardOutcome::Ignored
                    }
                }
            }
            SearchCardInput::Expire(token) => {
                if self
                    .pending
                    .take_if(|pending| pending.token == token)
                    .is_some()
                {
                    SearchCardOutcome::Expired
                } else {
                    SearchCardOutcome::Ignored
                }
            }
        }
    }
}

/// Quem executa o cartao: o App no produto, um registo nos gates.
pub(super) trait SearchCardHost {
    fn show_search_card(&mut self, token: u64, intent: SearchIntent, text: &str);
    fn hide_search_card(&mut self);
    fn expire_search_card_after(&mut self, token: u64, delay: Duration);
    fn compare_selection(&mut self, request: CompareRequest);
}

pub(super) fn apply_search_card(host: &mut impl SearchCardHost, outcome: SearchCardOutcome) {
    match outcome {
        SearchCardOutcome::Show {
            token,
            intent,
            text,
            ..
        } => {
            host.show_search_card(token, intent, &text);
            host.expire_search_card_after(token, Duration::from_secs(SEARCH_CARD_SECONDS));
        }
        SearchCardOutcome::Confirmed(request) => {
            host.hide_search_card();
            host.compare_selection(request);
        }
        SearchCardOutcome::Cancelled | SearchCardOutcome::Expired => host.hide_search_card(),
        SearchCardOutcome::Ignored => {}
    }
}

impl SearchCardHost for App {
    fn show_search_card(&mut self, token: u64, intent: SearchIntent, text: &str) {
        if let Ok(mut view) = SEARCH_CARD_VIEW.lock() {
            *view = Some((token, intent, text.to_string()));
        }
        // Um clique a meio no cartao anterior nao passa para o novo.
        SEARCH_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
        if self.search_card_popup.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let Some(owner) = window_hwnd(window) else {
                return;
            };
            let scale = window.scale_factor().max(1.0);
            let width = (SEARCH_CARD_WIDTH * scale).round() as i32;
            let height = (SEARCH_CARD_HEIGHT * scale).round() as i32;
            unsafe {
                // Owned pela janela principal, como o splash e o aviso do
                // Gmail: acima do WebView2 e da pagina, nao acima das outras
                // aplicacoes; nasce invisivel e sem ativacao.
                let created = CreateWindowExW(
                    AUX_POPUP_EX_STYLE,
                    windows_sys::w!("STATIC"),
                    windows_sys::w!(""),
                    AUX_POPUP_STYLE,
                    0,
                    0,
                    width,
                    height,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if created.is_null() {
                    return;
                }
                if SetWindowSubclass(
                    created,
                    Some(search_card_subclass),
                    SEARCH_CARD_SUBCLASS_ID,
                    (&*self.search_card_sink as *const SearchCardSink) as usize,
                ) == 0
                {
                    DestroyWindow(created);
                    return;
                }
                let corner = (18.0 * scale).round() as i32;
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, corner, corner);
                if !region.is_null() {
                    SetWindowRgn(created, region, 1);
                }
                self.search_card_popup = Some(created);
            }
        }
        self.position_search_card();
    }

    fn hide_search_card(&mut self) {
        if let Some(card) = self.search_card_popup.take() {
            unsafe {
                DestroyWindow(card);
            }
        }
        if let Ok(mut view) = SEARCH_CARD_VIEW.lock() {
            *view = None;
        }
        if let Ok(mut painted) = SEARCH_CARD_PAINTED.lock() {
            *painted = (0, 0);
        }
        SEARCH_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
    }

    fn expire_search_card_after(&mut self, token: u64, delay: Duration) {
        self.timers
            .after(delay, UserEvent::SearchCardExpired(token));
    }

    fn compare_selection(&mut self, request: CompareRequest) {
        self.compare(request);
    }
}
