use super::*;

// Painel lateral (Ctrl+H): historico inteligente -- busca semantica, sugestoes
// de sites e os recentes. E uma WebView LOCAL com canal IPC proprio: so esta
// WebView fala por `parse_panel_message`, e ela so carrega o HTML abaixo
// (`panel_allows_navigation`). Nenhuma pagina da internet alcanca este canal,
// e os dados entram na pagina como texto (`textContent`), nunca como HTML.

/// O que a pagina do painel pode pedir. Lista fechada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum PanelMessage {
    Ready,
    Search(String),
    Open(String),
    Close,
    /// Notas: todas, pela ordem de atualizacao.
    NotesList,
    /// Notas com todos os termos (`ZettelStore::search`).
    NotesSearch(String),
    /// Abrir no editor; o id ja foi validado.
    NoteOpen(String),
    NoteSave(NoteEdit),
    /// Um `note-save` que o parser recusou (id invalido, campos a mais,
    /// acima dos tectos): o painel recebe um "failed" em vez de silencio --
    /// senao ficava em "A salvar…" para sempre e a nota nova nunca mais se
    /// salvava.
    NoteSaveRefused,
    /// O que o editor tem por salvar (`None`: nada). O lado nativo guarda a
    /// copia e grava-a se o painel fechar por fora -- botao Notas, Ctrl+H,
    /// outro painel, Home, uma pesquisa nova, fechar a janela --, porque ai
    /// a pagina ja nao corre (`take_note_draft_on_close`).
    NoteDraft(Option<NoteEdit>),
    /// Mover para `.trash`; o id ja foi validado.
    NoteDelete(String),
}

pub(in crate::windows_app) const PANEL_MESSAGE_MAX_BYTES: usize = 4 * 1024;
pub(in crate::windows_app) const PANEL_QUERY_MAX_CHARS: usize = 500;
pub(in crate::windows_app) const PANEL_INPUT_MAX_CHARS: usize = 2048;
/// Quantos recentes e quantas sugestoes o painel mostra.
pub(in crate::windows_app) const PANEL_RECENT_LIMIT: usize = 30;
pub(in crate::windows_app) const PANEL_SUGGESTION_LIMIT: usize = 6;
/// O parser de uma secao do painel: recebe a acao ja lida do envelope e os
/// `args` (ou a sua ausencia) e devolve o pedido, ou `None` para o recusar.
pub(in crate::windows_app) type PanelSectionParser =
    fn(&str, Option<&serde_json::Value>) -> Option<PanelMessage>;

/// Uma secao do painel lateral, dona das acoes cujo prefixo (o texto antes
/// do primeiro `-`) e um dos seus. `parse_panel_message` entrega-lhe a
/// acao inteira; a secao diz o tecto de bytes de cada acao e faz o resto
/// do parse. Uma secao nova e uma linha em `PANEL_SECTIONS` e um parser no
/// modulo dela -- o delegador nao muda.
pub(in crate::windows_app) struct PanelSection {
    pub(in crate::windows_app) prefixes: &'static [&'static str],
    /// O tecto de bytes de cada acao desta secao. So o `note-save` e o
    /// `note-draft` passam dos 4 KiB: o resto fica em
    /// `PANEL_MESSAGE_MAX_BYTES`, e o delegador prende-o antes de entregar.
    pub(in crate::windows_app) max_bytes: fn(&str) -> usize,
    pub(in crate::windows_app) parse: PanelSectionParser,
}

/// As secoes com prefixo. As acoes sem `-` (`ready`, `close`, `search`,
/// `open`) sao do proprio painel e do Historico (`PANEL_CORE`).
pub(in crate::windows_app) const PANEL_SECTIONS: &[PanelSection] = &[PanelSection {
    prefixes: &["note", "notes"],
    max_bytes: notes_message_max_bytes,
    parse: parse_notes_action,
}];

/// O painel em si e o Historico: as acoes sem prefixo.
pub(in crate::windows_app) static PANEL_CORE: PanelSection = PanelSection {
    prefixes: &[],
    max_bytes: |_| PANEL_MESSAGE_MAX_BYTES,
    parse: parse_core_action,
};

/// A secao dona de `action`: a do prefixo (o texto antes do primeiro `-`),
/// ou o nucleo quando nao ha `-`. Um prefixo que nenhuma secao reclamou e
/// `None`: o pedido morre no delegador, sem chegar a parser nenhum.
pub(in crate::windows_app) fn panel_section_of(action: &str) -> Option<&'static PanelSection> {
    match action.split_once('-') {
        None => Some(&PANEL_CORE),
        Some((prefix, _)) => PANEL_SECTIONS
            .iter()
            .find(|section| section.prefixes.contains(&prefix)),
    }
}

/// O maior tecto de todas as secoes: acima disto o corpo nem se le como
/// JSON. E o do `note-save`, o unico pedido com o corpo de uma nota dentro.
pub(in crate::windows_app) const PANEL_MESSAGE_ABSOLUTE_MAX_BYTES: usize =
    NOTE_SAVE_MESSAGE_MAX_BYTES;

#[cfg(test)]
thread_local! {
    /// So nos testes: quantas vezes o delegador chegou a ler um corpo como
    /// JSON. E o que prova o tecto absoluto: o tecto da secao do
    /// `note-save` e o mesmo numero e corre DEPOIS do JSON, por isso o
    /// `None` sozinho nao distingue os dois (gate
    /// `panel_bodies_above_the_absolute_cap_are_never_read_as_json`).
    pub(in crate::windows_app) static PANEL_JSON_READS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// `args` e um objecto com EXACTAMENTE estas chaves -- nem uma a mais, nem
/// uma a menos -- ou nada. Sem `args` so passa quem nao pede chave nenhuma;
/// um `args` presente que nao e objecto (`null`, texto, numero) recusa
/// sempre, ate quem nao pede chave nenhuma: so a AUSENCIA vale como vazio.
/// E a mesma regra do canal IPC (`ipc::exact_keys`) e dos livros: um campo
/// a mais nunca e ignorado em silencio.
pub(in crate::windows_app) fn exact_keys<'a>(
    args: Option<&'a serde_json::Value>,
    keys: &[&str],
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    static EMPTY: std::sync::OnceLock<serde_json::Map<String, serde_json::Value>> =
        std::sync::OnceLock::new();
    let Some(args) = args else {
        return keys
            .is_empty()
            .then(|| EMPTY.get_or_init(serde_json::Map::new));
    };
    let map = args.as_object()?;
    (map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key))).then_some(map)
}

/// Um texto de `args[key]`, sem espacos a volta, com pelo menos um char e
/// no maximo `max`. `args` tem de ter SO essa chave.
pub(in crate::windows_app) fn panel_text(
    args: Option<&serde_json::Value>,
    key: &str,
    max: usize,
) -> Option<String> {
    let text = exact_keys(args, &[key])?.get(key)?.as_str()?.trim();
    (!text.is_empty() && text.chars().count() <= max).then(|| text.to_string())
}

/// O delegador do canal do painel. Le o envelope (`{"action", "args"}`),
/// prende o tamanho -- o tecto absoluto antes do JSON, o da secao depois de
/// saber a acao -- e entrega a acao a secao do prefixo dela. Tudo o que
/// nenhuma secao reclama morre aqui.
pub(in crate::windows_app) fn parse_panel_message(body: &str) -> Option<PanelMessage> {
    if body.len() > PANEL_MESSAGE_ABSOLUTE_MAX_BYTES {
        return None;
    }
    #[cfg(test)]
    PANEL_JSON_READS.with(|reads| reads.set(reads.get() + 1));
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let action = value.get("action")?.as_str()?;
    let section = panel_section_of(action)?;
    // Todos os outros pedidos continuam presos aos 4 KiB.
    if body.len() > (section.max_bytes)(action) {
        return None;
    }
    (section.parse)(action, value.get("args"))
}

/// As acoes do painel em si e do Historico.
fn parse_core_action(action: &str, args: Option<&serde_json::Value>) -> Option<PanelMessage> {
    match action {
        "ready" => exact_keys(args, &[]).map(|_| PanelMessage::Ready),
        "close" => exact_keys(args, &[]).map(|_| PanelMessage::Close),
        "search" => panel_text(args, "query", PANEL_QUERY_MAX_CHARS).map(PanelMessage::Search),
        "open" => panel_text(args, "input", PANEL_INPUT_MAX_CHARS).map(PanelMessage::Open),
        _ => None,
    }
}

// O painel do Ctrl+H e o texto de uma nota a meio, num bloco so.
//
// O caminho normal de saida e `SidePanel::dismiss` (o `App` chama-o por
// `close_side_panel`): grava primeiro o que o editor tinha por salvar, e so
// DEPOIS larga a vista e devolve o teclado. Os campos sao privados deste
// modulo, por isso um `take()` direto da vista (o que o
// `destroy_web_surfaces` fazia) nao compila. Largar o painel INTEIRO por
// outro caminho compila -- uma atribuicao por cima
// (`self.side_panel = SidePanel::closed(..)`), um `mem::replace`, um `drop`,
// o `App` a sair --, mas nao perde o texto: o painel leva consigo quem grava
// (`DraftRescue`) e o `Drop` dele manda o rascunho para a fila das notas
// antes de a vista sair, uma vez so (depois de `dismiss` ja nao ha
// rascunho). O que esse caminho nao faz e devolver o teclado. So um
// `mem::forget` do painel, ou o processo morto a meio, passa por cima. E os
// pedidos da pagina so chegam ao `App` por `SidePanel::receive`, que segue
// a copia do editor antes de os entregar.
//
// Fica uma janela de ate 200 ms: a pagina manda a copia (`note-draft`) no
// maximo 200 ms depois de uma tecla, mesmo a escrever sem parar, e o que se
// escreveu nesse intervalo antes de um fecho nativo perde-se. Pedir a copia
// a pagina no fecho seria assincrono (`evaluate_script_with_callback`) e a
// pagina ja nao existe quando a resposta viesse.

/// Por onde o painel sai. Todas gravam primeiro o que o editor tinha por
/// salvar; muda so para onde vai o teclado depois.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum PanelExit {
    /// O X ou o Esc da pagina, e o botao Notas com as Notas a vista (a
    /// pagina salva e manda `close`).
    CloseButton,
    /// Ctrl+H de novo (ou o `ShowHistory` do menu).
    CtrlH,
    /// Um item do historico, ou a fonte de uma nota, aberto a partir do
    /// painel.
    OpenItem,
    /// Um servico da barra (a Respiracao incluida) ou o Gemini Live abrem
    /// no lugar dele.
    OtherPanel,
    /// A Home.
    Home,
    /// Uma pesquisa nova no comparador.
    NewSearch,
    /// `destroy_web_surfaces`: o ecra de erro (`show_native_error`), a
    /// Web completa, o Leitor, o PDF, um link externo, o agente.
    SurfaceChange,
    /// A janela fecha (`SidePanel::exit`).
    AppExit,
}

impl PanelExit {
    #[cfg(test)]
    pub(in crate::windows_app) const ALL: [Self; 8] = [
        Self::CloseButton,
        Self::CtrlH,
        Self::OpenItem,
        Self::OtherPanel,
        Self::Home,
        Self::NewSearch,
        Self::SurfaceChange,
        Self::AppExit,
    ];

    /// Uma troca de superficie trata do teclado ela propria (e na Home
    /// passaria pelo `show_home` a meio dela); a saida da app nao precisa.
    fn keyboard(self, surface: Surface) -> Option<PanelCloseFocus> {
        match self {
            Self::SurfaceChange | Self::AppExit => None,
            Self::CloseButton
            | Self::CtrlH
            | Self::OpenItem
            | Self::OtherPanel
            | Self::Home
            | Self::NewSearch => Some(focus_after_panel_close(surface)),
        }
    }
}

/// Quem grava o rascunho: o worker das notas (`ZettelWorker`). O painel
/// leva o seu, para o `Drop` ter por onde gravar.
pub(in crate::windows_app) trait DraftRescue {
    /// Poe a gravacao no fim da fila, sem esperar pelo disco e sem a
    /// deitar fora com a fila cheia. `Err`: o aviso para o utilizador.
    fn rescue(&self, command: NotesCommand) -> Result<(), String>;
    /// Espera, ate `limit`, que tudo o que ja esta na fila -- o que
    /// `rescue` pos la antes incluido -- chegue ao disco.
    fn settle(&self, limit: Duration);
}

/// O numero de uma pagina do painel: vai no canal dela
/// (`PanelPost::parse`) e distingue-a das que ja sairam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct PanelTicket(u64);

/// Um pedido da pagina do painel, ja pelo parser do canal e com o numero
/// da pagina que o mandou.
#[derive(Debug)]
pub(in crate::windows_app) struct PanelPost {
    page: u64,
    message: PanelMessage,
}

impl PanelPost {
    /// O unico caminho de um pedido da pagina ate ao `App`:
    /// `parse_panel_message`, carimbado com a pagina.
    pub(in crate::windows_app) fn parse(ticket: PanelTicket, body: &str) -> Option<Self> {
        parse_panel_message(body).map(|message| Self {
            page: ticket.0,
            message,
        })
    }
}

/// O que `receive` entrega ao `App`.
#[derive(Debug)]
pub(in crate::windows_app) enum Received {
    /// Da pagina viva: o `App` trata.
    Current(PanelMessage),
    /// De uma pagina que ja saiu (o pedido estava na fila do event loop
    /// quando ela fechou). O texto que trazia -- um `note-draft` ou um
    /// `note-save` -- ja foi para a fila das notas (`Some`, com o
    /// resultado); o resto morre aqui, e nunca chega a outra pagina.
    Late(Option<Result<(), String>>),
}

/// O painel acabou de sair. `saved`: o que o editor tinha por salvar foi
/// para a fila das notas (ou nao havia nada), ou o aviso do erro.
#[derive(Debug)]
pub(in crate::windows_app) struct Dismissed {
    pub(in crate::windows_app) saved: Result<(), String>,
}

/// O painel do Ctrl+H: a vista (a `WebView` no app, uma de mentira nos
/// gates), a copia do que o editor tem por salvar, quem a grava e os
/// scripts que esperam pelo "ready" da pagina.
pub(in crate::windows_app) struct SidePanel<W, N: DraftRescue> {
    view: Option<W>,
    /// Quem grava o rascunho: o `dismiss`, o `receive` de uma pagina que
    /// ja saiu e o `Drop`.
    notes: N,
    /// A pagina viva.
    page: u64,
    /// O ultimo numero dado por `ticket`.
    issued: u64,
    /// Copia do que o editor tem por salvar (`track_note_draft`).
    draft: Option<NoteEdit>,
    /// A pagina ja correu o script dela (mandou "ready"). Antes disso um
    /// `evaluate_script` corria no documento vazio e perdia-se.
    ready: bool,
    /// Scripts que esperam pelo "ready", pela ordem em que foram pedidos.
    pending: Vec<String>,
}

impl<W: PanelView, N: DraftRescue> SidePanel<W, N> {
    /// Sem pagina, com `notes` para gravar o que as paginas deixarem.
    pub(in crate::windows_app) fn closed(notes: N) -> Self {
        Self {
            view: None,
            notes,
            page: 0,
            issued: 0,
            draft: None,
            ready: false,
            pending: Vec::new(),
        }
    }

    pub(in crate::windows_app) fn is_open(&self) -> bool {
        self.view.is_some()
    }

    pub(in crate::windows_app) fn view(&self) -> Option<&W> {
        self.view.as_ref()
    }

    /// O numero da proxima pagina, para o canal dela, antes de a criar.
    pub(in crate::windows_app) fn ticket(&mut self) -> PanelTicket {
        self.issued += 1;
        PanelTicket(self.issued)
    }

    /// A pagina nova, com o numero que o canal dela leva. Com um painel
    /// ja aberto a vista nova volta para o chamador: a aberta nao e
    /// substituida aqui.
    pub(in crate::windows_app) fn open(&mut self, ticket: PanelTicket, view: W) -> Result<(), W> {
        if self.view.is_some() {
            return Err(view);
        }
        self.view = Some(view);
        self.page = ticket.0;
        self.ready = false;
        self.pending.clear();
        Ok(())
    }

    /// Um pedido da pagina. Da viva: a copia do editor fica seguida e o
    /// pedido vai para o `App`. De uma que ja saiu: o texto que trazia vai
    /// ja para o disco -- nao fica a espera de um painel que nao volta, e
    /// nunca passa por copia do painel novo.
    pub(in crate::windows_app) fn receive(&mut self, post: PanelPost) -> Received {
        let PanelPost { page, message } = post;
        if self.view.is_some() && page == self.page {
            track_note_draft(&mut self.draft, &message);
            return Received::Current(message);
        }
        let text = match message {
            PanelMessage::NoteDraft(Some(edit)) | PanelMessage::NoteSave(edit) => Some(edit),
            PanelMessage::NoteDraft(None)
            | PanelMessage::Ready
            | PanelMessage::Search(_)
            | PanelMessage::Open(_)
            | PanelMessage::Close
            | PanelMessage::NotesList
            | PanelMessage::NotesSearch(_)
            | PanelMessage::NoteOpen(_)
            | PanelMessage::NoteSaveRefused
            | PanelMessage::NoteDelete(_) => None,
        };
        Received::Late(text.map(|edit| self.notes.rescue(NotesCommand::Save(edit))))
    }

    /// `Some(script)`: correr ja. Sem painel nao ha onde; antes do
    /// "ready" fica a espera dele.
    pub(in crate::windows_app) fn run(&mut self, script: String) -> Option<String> {
        self.view.as_ref()?;
        if self.ready {
            return Some(script);
        }
        self.pending.push(script);
        None
    }

    /// A pagina correu o script dela: o que esperava, pela ordem.
    pub(in crate::windows_app) fn mark_ready(&mut self) -> Vec<String> {
        self.ready = true;
        std::mem::take(&mut self.pending)
    }

    /// A saida normal do painel. Primeiro o que o editor tinha por salvar
    /// vai para a fila das notas (`take_note_draft_on_close`, que o
    /// tira do painel: o `Drop` ja nao o grava outra vez), e so depois a
    /// vista sai (`release_panel`, que devolve o teclado). `None`: nao
    /// havia painel.
    pub(in crate::windows_app) fn dismiss(
        &mut self,
        exit: PanelExit,
        surface: Surface,
        omnibox: Option<HWND>,
    ) -> Option<Dismissed> {
        let view = self.view.take()?;
        let saved = match take_note_draft_on_close(&mut self.draft) {
            Some(command) => self.notes.rescue(command),
            None => Ok(()),
        };
        self.ready = false;
        self.pending.clear();
        release_panel(view, exit.keyboard(surface), omnibox);
        Some(Dismissed { saved })
    }

    /// A janela fecha. O processo acaba com o event loop e levava a
    /// thread das notas a meio: o rascunho do painel aberto vai para a
    /// fila como em qualquer fecho, e so DEPOIS a marca do `settle`, pela
    /// mesma fila -- espera-se (ate `limit`) que tudo ate ela, o
    /// rascunho e o que fechos anteriores ainda tinham por gravar,
    /// chegue ao disco.
    pub(in crate::windows_app) fn exit(&mut self, limit: Duration) -> Result<(), String> {
        let saved = self
            .dismiss(PanelExit::AppExit, Surface::Home, None)
            .map_or(Ok(()), |closed| closed.saved);
        self.notes.settle(limit);
        saved
    }

    #[cfg(test)]
    pub(in crate::windows_app) fn draft(&self) -> Option<&NoteEdit> {
        self.draft.as_ref()
    }
}

/// Largar o painel sem `dismiss` -- uma atribuicao por cima, um
/// `mem::replace`, um `drop`, o `App` a sair -- nao perde o texto: o que
/// o editor tinha por salvar vai para a fila das notas, e so depois a
/// vista sai (os campos largam-se depois deste `drop`). Depois de um
/// `dismiss` ja nao ha rascunho e nada se grava outra vez. O teclado,
/// esse, so o `dismiss` devolve.
impl<W, N: DraftRescue> Drop for SidePanel<W, N> {
    fn drop(&mut self) {
        if let Some(command) = take_note_draft_on_close(&mut self.draft)
            && let Err(error) = self.notes.rescue(command)
        {
            debug_log(format_args!(
                "side panel: largado sem dismiss e o rascunho nao foi gravado: {error}"
            ));
        }
    }
}

/// So o proprio HTML local (NavigateToString chega como about:blank ou
/// data:). Um link, um redirect, um file: ou um javascript: nao passam.
pub(in crate::windows_app) fn panel_allows_navigation(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    lower == "about:blank" || lower.starts_with("data:text/html")
}

/// Encostado a direita, abaixo da barra do comparador (ou do topo, fora
/// dele): 34% da largura, entre 320 e 440 px logicos, nunca mais que a janela.
#[cfg(test)]
pub(in crate::windows_app) fn side_panel_bounds(
    logical_w: f64,
    logical_h: f64,
    top: f64,
) -> (f64, f64, f64, f64) {
    panel_bounds(PanelKind::History, None, logical_w, logical_h, top)
}

/// Um painel da direita com a largura `chosen` (arrastada pela borda e
/// gravada) ou, sem escolha, a de sempre -- presa sempre a [300 px, 60% da
/// janela] (`panel_chrome::panel_width`).
pub(in crate::windows_app) fn panel_bounds(
    kind: PanelKind,
    chosen: Option<f64>,
    logical_w: f64,
    logical_h: f64,
    top: f64,
) -> (f64, f64, f64, f64) {
    let area = panel_area(
        panel_width(kind, chosen, logical_w),
        logical_w,
        logical_h,
        top,
    );
    (area.x, area.y, area.width, area.height)
}

/// A largura das colunas do comparador: a janela menos o painel da direita.
/// Layout, divisores e o arrasto dos divisores usam TODOS esta conta; o
/// arrasto usava a janela inteira e o divisor fugia do rato com o painel
/// aberto.
pub(in crate::windows_app) fn comparator_logical_width(
    window_logical_w: f64,
    panel_width: f64,
) -> f64 {
    (window_logical_w - panel_width.max(0.0)).max(1.0)
}

/// Um item do painel: o que se le e o que o clique volta a abrir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct PanelItem {
    pub(in crate::windows_app) title: String,
    pub(in crate::windows_app) detail: String,
    pub(in crate::windows_app) input: String,
}

pub(in crate::windows_app) fn history_panel_items(entries: &[HistoryEntry]) -> Vec<PanelItem> {
    entries
        .iter()
        .filter(|entry| !entry.input.trim().is_empty())
        .map(|entry| {
            let kind = match entry.kind {
                HistoryKind::Ask => "IA",
                HistoryKind::Read => "Leitor",
                HistoryKind::Web => "Web",
            };
            let detail = if entry.target.trim().is_empty() || entry.target == entry.input {
                kind.to_string()
            } else {
                format!("{kind} · {}", entry.target)
            };
            PanelItem {
                title: entry.input.clone(),
                detail,
                input: entry.input.clone(),
            }
        })
        .collect()
}

pub(in crate::windows_app) fn memory_panel_items(hits: &[MemoryHit]) -> Vec<PanelItem> {
    hits.iter()
        .map(|hit| {
            let source = hit
                .provider
                .as_deref()
                .or(hit.url.as_deref())
                .unwrap_or("memória local");
            PanelItem {
                title: hit.title.clone(),
                detail: if hit.excerpt.trim().is_empty() {
                    source.to_string()
                } else {
                    format!("{source} · {}", hit.excerpt)
                },
                // Com endereco, o clique abre a pagina; sem, repete a busca.
                input: hit.url.clone().unwrap_or_else(|| hit.title.clone()),
            }
        })
        .collect()
}

/// Sugestoes de sites: os resultados da memoria que tem endereco web, um por
/// dominio, na ordem de relevancia.
pub(in crate::windows_app) fn suggestion_panel_items(
    hits: &[MemoryHit],
    limit: usize,
) -> Vec<PanelItem> {
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for hit in hits {
        let Some(url) = hit.url.as_deref() else {
            continue;
        };
        let Ok(parsed) = Url::parse(url) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let domain = host.trim_start_matches("www.").to_string();
        if !seen.insert(domain.clone()) {
            continue;
        }
        items.push(PanelItem {
            title: if hit.title.trim().is_empty() {
                domain.clone()
            } else {
                hit.title.clone()
            },
            detail: domain,
            input: url.to_string(),
        });
        if items.len() >= limit {
            break;
        }
    }
    items
}

/// O JS que preenche uma secao. Os dados vao como JSON (literal JS valido) e a
/// pagina so os usa com `textContent`.
pub(in crate::windows_app) fn panel_render_script(
    section: &str,
    title: &str,
    empty: &str,
    items: &[PanelItem],
) -> String {
    let items: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "title": item.title,
                "detail": item.detail,
                "input": item.input,
            })
        })
        .collect();
    let data = serde_json::json!({
        "id": section,
        "title": title,
        "empty": empty,
        "items": items,
    });
    format!("window.__neuraliaPanel && window.__neuraliaPanel.render({data});")
}

pub(in crate::windows_app) fn css_color(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.0, color.1, color.2)
}

/// As cores do tema em vigor, como variaveis CSS do painel.
pub(in crate::windows_app) fn panel_theme_vars(theme: &Theme) -> serde_json::Value {
    serde_json::json!({
        "--bg": css_color(theme.page_bg),
        "--surface": css_color(theme.surface),
        "--fg": css_color(theme.fg),
        "--muted": css_color(theme.fg_muted),
        "--line": css_color(theme.surface_line),
        "--accent": css_color(theme.accent),
    })
}

pub(in crate::windows_app) fn panel_html(theme: &Theme) -> String {
    PANEL_HTML.replace("__THEME__", &panel_theme_vars(theme).to_string())
}

/// A pagina do painel, montada em tempo de compilacao a partir de
/// `assets/panel/`: a folha (`panel.css`), a marcacao de cada secao
/// (`history.html`, `notes.html`) e o script -- uma IIFE so, em pedacos
/// pela ordem: `core.js` (o `post`, o `byId`, o `make`, a caixa de busca e o
/// `theme`), `history.js` (o `render` do Historico), `notes.js` (a secao
/// Notas) e `tabs.js` (as abas, o fechar e o `ready` final, que fecha a
/// IIFE). Uma secao nova traz o seu `<secao>.html` e `<secao>.js` e uma
/// linha em cada `include_str!` -- o resto da pagina nao muda. Os bytes sao
/// os da pagina de sempre (os assets sao LF por `.gitattributes`; o gate
/// `panel_html_is_assembled_from_its_section_assets` prende-o).
pub(in crate::windows_app) const PANEL_HTML: &str = concat!(
    r#"<!doctype html>
<html lang="pt-BR"><head><meta charset="utf-8"><title>Histórico e notas</title>
<style>
"#,
    include_str!("../../../../assets/panel/panel.css"),
    r#"</style></head><body>
<header><nav class="tabs" role="tablist"><button class="tab" id="tab-history" role="tab" aria-selected="true">Histórico</button><button class="tab" id="tab-notes" role="tab" aria-selected="false">Notas</button></nav><button id="close" title="Fechar (Esc)">✕</button></header>
"#,
    include_str!("../../../../assets/panel/history.html"),
    include_str!("../../../../assets/panel/notes.html"),
    "<script>\n",
    include_str!("../../../../assets/panel/core.js"),
    include_str!("../../../../assets/panel/history.js"),
    include_str!("../../../../assets/panel/notes.js"),
    include_str!("../../../../assets/panel/tabs.js"),
    "</script></body></html>"
);
