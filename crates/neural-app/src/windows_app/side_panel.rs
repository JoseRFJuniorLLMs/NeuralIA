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
pub(in crate::windows_app) fn parse_panel_message(body: &str) -> Option<PanelMessage> {
    if body.len() > NOTE_SAVE_MESSAGE_MAX_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let action = value.get("action")?.as_str()?;
    // Todos os outros pedidos continuam presos aos 4 KiB.
    if body.len() > PANEL_MESSAGE_MAX_BYTES && !matches!(action, "note-save" | "note-draft") {
        return None;
    }
    let text = |key: &str, max: usize| -> Option<String> {
        let text = value.get("args")?.get(key)?.as_str()?.trim();
        (!text.is_empty() && text.chars().count() <= max).then(|| text.to_string())
    };
    // `{"id": "<id valido>"}` e mais nada: um `../x` ou `C:\x` nunca chega ao
    // disco, nem sequer ao worker das notas.
    let note_id = || -> Option<String> {
        let args = value.get("args")?.as_object()?;
        if args.len() != 1 {
            return None;
        }
        let id = args.get("id")?.as_str()?;
        is_valid_note_id(id).then(|| id.to_string())
    };
    match action {
        "ready" => Some(PanelMessage::Ready),
        "close" => Some(PanelMessage::Close),
        "search" => text("query", PANEL_QUERY_MAX_CHARS).map(PanelMessage::Search),
        "open" => text("input", PANEL_INPUT_MAX_CHARS).map(PanelMessage::Open),
        "notes-list" => Some(PanelMessage::NotesList),
        "notes-search" => text("query", PANEL_QUERY_MAX_CHARS).map(PanelMessage::NotesSearch),
        "note-open" => note_id().map(PanelMessage::NoteOpen),
        "note-delete" => note_id().map(PanelMessage::NoteDelete),
        // Um note-save recusado responde "failed" (`NoteSaveRefused`); o
        // resto do que o parser recusa continua a morrer aqui.
        "note-save" => Some(
            value
                .get("args")
                .and_then(parse_note_edit)
                .map_or(PanelMessage::NoteSaveRefused, PanelMessage::NoteSave),
        ),
        "note-draft" => {
            let args = value.get("args")?;
            if args.as_object()?.is_empty() {
                Some(PanelMessage::NoteDraft(None))
            } else {
                parse_note_edit(args).map(|edit| PanelMessage::NoteDraft(Some(edit)))
            }
        }
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
pub(in crate::windows_app) struct PanelTicket(pub(in crate::windows_app) u64);

/// Um pedido da pagina do painel, ja pelo parser do canal e com o numero
/// da pagina que o mandou.
#[derive(Debug)]
pub(in crate::windows_app) struct PanelPost {
    pub(in crate::windows_app) page: u64,
    pub(in crate::windows_app) message: PanelMessage,
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

pub(in crate::windows_app) const PANEL_HTML: &str = r#"<!doctype html>
<html lang="pt-BR"><head><meta charset="utf-8"><title>Histórico e notas</title>
<style>
*{box-sizing:border-box}
html,body{margin:0;height:100%;background:var(--bg);color:var(--fg);font:15px "Segoe UI",system-ui,sans-serif}
body{display:flex;flex-direction:column;border-left:1px solid var(--line)}
header{display:flex;align-items:center;justify-content:space-between;padding:10px 12px 8px 12px}
.tabs{display:flex;gap:4px}
.tab{background:none;border:0;color:var(--muted);font:inherit;font-weight:600;padding:7px 16px;border-radius:999px;cursor:pointer}
.tab:hover{background:var(--surface)}
.tab[aria-selected="true"]{background:var(--surface);color:var(--fg);box-shadow:inset 0 0 0 1px var(--line)}
#close{background:none;border:0;color:var(--muted);font-size:18px;cursor:pointer;border-radius:8px;width:32px;height:32px}
#close:hover{background:#e81123;color:#fff}
.view{display:flex;flex-direction:column;flex:1;min-height:0}
.view[hidden]{display:none}
.search{padding:4px 16px 10px}
.field{width:100%;padding:10px 14px;border-radius:12px;border:1px solid var(--line);background:var(--surface);color:var(--fg);font:inherit;outline:none}
.field:focus{border-color:var(--accent)}
#q,#nq{border-radius:999px}
main{overflow:auto;flex:1;padding:0 8px 16px}
::-webkit-scrollbar{width:10px;height:10px}
::-webkit-scrollbar-track,::-webkit-scrollbar-corner{background:transparent}
::-webkit-scrollbar-thumb{background-color:var(--line);border:3px solid transparent;border-radius:999px;background-clip:padding-box}
::-webkit-scrollbar-thumb:hover{background-color:var(--muted)}
::-webkit-scrollbar-button{display:none;width:0;height:0}
section[hidden]{display:none}
h2{font-size:12px;letter-spacing:.06em;text-transform:uppercase;color:var(--muted);margin:14px 10px 6px;font-weight:600}
.item{display:block;width:100%;text-align:left;background:none;border:0;color:inherit;font:inherit;padding:8px 10px;border-radius:10px;cursor:pointer}
.item:hover,.item:focus{background:var(--surface);outline:none}
.title,.detail{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.detail{font-size:12px;color:var(--muted);margin-top:2px}
.empty{color:var(--muted);font-size:13px;padding:6px 10px}
.row{display:flex;gap:8px;align-items:center;padding:4px 16px 10px}
.btn{flex:none;background:var(--surface);border:1px solid var(--line);color:var(--fg);font:inherit;font-size:13px;padding:8px 14px;border-radius:999px;cursor:pointer}
.btn:hover{border-color:var(--accent)}
.btn.primary{background:var(--accent);border-color:var(--accent);color:#fff}
.btn.danger:hover{background:#e81123;border-color:#e81123;color:#fff}
#notes-msg{font-size:12px;color:var(--muted);min-height:18px;padding:0 18px 4px}
#note-editor{display:flex;flex-direction:column;gap:8px;overflow:auto;flex:1;padding:0 16px 16px}
#note-editor h2{margin:10px 2px 0}
#note-title{font-weight:600}
#note-body{min-height:200px;resize:vertical;font:14px/1.45 "Segoe UI",system-ui,sans-serif}
.meta{font-size:12px;color:var(--muted);word-break:break-all}
.link{background:none;border:0;padding:0;color:var(--accent);font:inherit;cursor:pointer;text-decoration:underline;text-align:left}
#note-preview{white-space:pre-wrap;word-break:break-word;font-size:13px;line-height:1.5;max-height:30vh;overflow:auto;padding:8px 10px;border-radius:10px;background:var(--surface)}
.confirm{display:flex;flex-wrap:wrap;gap:8px;align-items:center;padding:10px;border-radius:12px;border:1px solid #e81123;font-size:13px}
.confirm[hidden]{display:none}
</style></head><body>
<header><nav class="tabs" role="tablist"><button class="tab" id="tab-history" role="tab" aria-selected="true">Histórico</button><button class="tab" id="tab-notes" role="tab" aria-selected="false">Notas</button></nav><button id="close" title="Fechar (Esc)">✕</button></header>
<div class="view" id="view-history">
<div class="search"><input id="q" class="field" placeholder="Descreva o que quer reencontrar e tecle Enter" autocomplete="off" spellcheck="false"></div>
<main>
<section id="busca" hidden><h2></h2><div></div></section>
<section id="sugestoes" hidden><h2></h2><div></div></section>
<section id="recentes" hidden><h2></h2><div></div></section>
</main>
</div>
<div class="view" id="view-notes" hidden>
<div id="notes-msg" role="status"></div>
<div class="view" id="notes-browse">
<div class="row"><input id="nq" class="field" placeholder="Buscar nas notas" autocomplete="off" spellcheck="false"><button id="note-new" class="btn primary">Nova nota</button></div>
<main><div id="notes-list"></div></main>
</div>
<div class="view" id="notes-edit" hidden>
<div class="row"><button id="note-back" class="btn" title="Voltar à lista">← Notas</button><button id="note-save" class="btn primary" title="Salvar (Ctrl+S)">Salvar</button><button id="note-delete" class="btn danger">Excluir</button></div>
<div id="note-editor">
<div id="note-confirm" class="confirm" hidden><span>Excluir esta nota? Ela vai para a lixeira (.trash) da pasta das notas.</span><button id="note-confirm-yes" class="btn danger">Excluir</button><button id="note-confirm-no" class="btn">Cancelar</button></div>
<input id="note-title" class="field" placeholder="Título" autocomplete="off">
<textarea id="note-body" class="field" placeholder="Escreva em Markdown. Ligue outra nota com [[id]] ou [[id|nome]]."></textarea>
<input id="note-tags" class="field" placeholder="Tags, separadas por vírgula" autocomplete="off" spellcheck="false">
<div id="note-source-row" class="meta" hidden><span>Fonte: </span><button id="note-source" class="link"></button></div>
<div id="note-meta" class="meta"></div>
<section id="note-preview-box" hidden><h2>Pré-visualização</h2><div id="note-preview"></div></section>
<section id="note-backlinks-box" hidden><h2>Notas que ligam para esta</h2><div id="note-backlinks"></div></section>
</div>
</div>
</div>
<script>
(() => {
  if (window.top !== window) return;
  const post = (action, args) => window.ipc.postMessage(JSON.stringify({ action, args: args || {} }));
  const byId = (id) => document.getElementById(id);
  const make = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  const q = byId('q');
  q.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && q.value.trim()) { e.preventDefault(); post('search', { query: q.value.trim() }); }
  });
  const theme = (vars) => { for (const k of Object.keys(vars)) document.documentElement.style.setProperty(k, vars[k]); };
  window.__neuraliaPanel = {
    theme,
    render(data) {
      const section = byId(data.id);
      if (!section) return;
      section.hidden = false;
      section.querySelector('h2').textContent = data.title;
      const box = section.querySelector('div');
      box.textContent = '';
      if (!data.items.length) {
        box.appendChild(make('div', 'empty', data.empty));
        return;
      }
      for (const item of data.items) {
        const button = make('button', 'item');
        button.append(make('span', 'title', item.title), make('span', 'detail', item.detail));
        button.addEventListener('click', () => post('open', { input: item.input }));
        box.appendChild(button);
      }
    }
  };

  // Notas (Zettelkasten). Tudo o que vem de uma nota -- e o corpo pode ser
  // texto copiado de qualquer pagina -- entra como texto: textContent,
  // value e createTextNode. Nunca como HTML.
  const notes = (() => {
    const ID = /^[0-9][0-9-]{0,63}$/;
    const WIKI = /\[\[([^\[\]|\n]+)(?:\|([^\[\]\n]*))?\]\]/g;
    const BODY_MAX_BYTES = 200 * 1024;
    const TITLE_MAX = 300;
    const TAGS_MAX = 20;
    const TAG_MAX = 60;
    const QUERY_MAX = 500;
    const SOURCE_MAX = 2048;
    const nq = byId('nq'), list = byId('notes-list'), msg = byId('notes-msg');
    const browse = byId('notes-browse'), edit = byId('notes-edit');
    const title = byId('note-title'), body = byId('note-body'), tags = byId('note-tags');
    const sourceRow = byId('note-source-row'), source = byId('note-source'), meta = byId('note-meta');
    const previewBox = byId('note-preview-box'), preview = byId('note-preview');
    const backBox = byId('note-backlinks-box'), backlinks = byId('note-backlinks');
    const confirmBox = byId('note-confirm');
    // Nota no editor: o id dela, ou null para uma nota nova ainda por salvar.
    let openId = null;
    // A revisao da nota que o editor mostra (do disco, ou do nosso ultimo
    // salvar): o salvar leva-a, e o worker nao esmaga uma nota que mudou fora
    // daqui desde entao.
    let openRev = null;
    let openSource = '';
    let dirty = false;
    let savingNew = false;
    // Um note-save a caminho (o id da nota): se voltar "failed", o texto
    // volta a estar por salvar em vez de se perder ao sair do editor.
    let saving = undefined;
    // O lado nativo guarda uma copia do que esta por salvar (note-draft) e
    // grava-a quando o painel fecha por fora -- botao Notas, Ctrl+H, outro
    // painel, Home, fechar a janela --, porque ai esta pagina ja nao corre.
    let draftPosted = false;
    let draftTimer = 0;
    let searchTimer = 0;
    let previewTimer = 0;

    const say = (text) => { msg.textContent = text || ''; };
    const when = (unix) => {
      if (!unix) return '';
      try { return new Date(unix * 1000).toLocaleString('pt-BR', { dateStyle: 'short', timeStyle: 'short' }); } catch (e) { return ''; }
    };
    const query = () => nq.value.trim().slice(0, QUERY_MAX);
    const refresh = () => {
      const text = query();
      if (text) post('notes-search', { query: text }); else post('notes-list');
    };
    const open = (id) => { if (ID.test(id) && leave()) post('note-open', { id }); };
    // Texto que o JSON leva inteiro (sem metades de um par UTF-16, que o
    // parser do lado nativo recusava) e titulo e tags numa linha: o TAB de
    // uma tabela colada ou do titulo de uma nota do Obsidian vira espaco.
    const whole = (text) => (typeof text.toWellFormed === 'function' ? text.toWellFormed() : text);
    const line = (text) => whole(String(text)).replace(/[\u0000-\u001f\u007f-\u009f]+/g, ' ').trim();
    const edited = () => ({
      id: openId,
      rev: openId === null ? null : openRev,
      title: line(title.value),
      body: whole(body.value),
      tags: tags.value.split(',').map(line).filter(Boolean),
    });
    // Manda (ou limpa) a copia do lado nativo.
    function postDraft() {
      clearTimeout(draftTimer);
      draftTimer = 0;
      const note = edited();
      if (dirty && !edit.hidden && (note.title || note.body.trim())) {
        post('note-draft', note);
        draftPosted = true;
      } else if (draftPosted) {
        post('note-draft', {});
        draftPosted = false;
      }
    }
    const showList = () => { confirmBox.hidden = true; edit.hidden = true; browse.hidden = false; };
    const showEditor = () => { browse.hidden = true; edit.hidden = false; };

    function renderList(data) {
      // Resposta a uma busca que ja nao e a da caixa: a seguinte vem a caminho.
      if ((data.query || '') !== query()) return;
      list.textContent = '';
      if (!data.notes.length) {
        list.appendChild(make('div', 'empty', data.query
          ? 'Nenhuma nota encontrada.'
          : 'Nenhuma nota ainda. Crie uma em Nova nota, ou selecione um texto numa página e tecle Ctrl+Shift+Z.'));
        return;
      }
      if (data.total > data.notes.length) {
        list.appendChild(make('div', 'empty', 'Mostrando ' + data.notes.length + ' de ' + data.total + ' notas. Refine a busca.'));
      }
      for (const note of data.notes) {
        const button = make('button', 'item');
        const detail = [when(note.updated), note.tags.map((tag) => '#' + tag).join(' ')].filter(Boolean).join(' · ');
        button.append(make('span', 'title', note.title), make('span', 'detail', detail));
        button.addEventListener('click', () => open(note.id));
        list.appendChild(button);
      }
    }

    // O corpo com os [[id]] e [[id|nome]] clicaveis; o resto e texto.
    function renderPreview() {
      preview.textContent = '';
      const text = body.value;
      let last = 0;
      let match;
      WIKI.lastIndex = 0;
      while ((match = WIKI.exec(text)) !== null) {
        const id = match[1].trim();
        if (!ID.test(id)) continue;
        if (match.index > last) preview.appendChild(document.createTextNode(text.slice(last, match.index)));
        const link = make('button', 'link', (match[2] || '').trim() || id);
        link.title = 'Abrir a nota ' + id;
        link.addEventListener('click', () => open(id));
        preview.appendChild(link);
        last = match.index + match[0].length;
      }
      if (last < text.length) preview.appendChild(document.createTextNode(text.slice(last)));
      previewBox.hidden = !text.trim();
    }

    // Fonte, datas e backlinks: o que o editor mostra da nota sem ser editavel.
    function describe(note, links) {
      openSource = note && note.source ? note.source : '';
      source.textContent = openSource;
      source.disabled = !/^https?:\/\//i.test(openSource) || openSource.length > SOURCE_MAX;
      sourceRow.hidden = !openSource;
      meta.textContent = note
        ? ['Criada ' + when(note.created), 'atualizada ' + when(note.updated), 'id ' + note.id].join(' · ')
        : 'Nota nova: ainda não foi salva.';
      backlinks.textContent = '';
      for (const other of links || []) {
        const button = make('button', 'item', other.title);
        button.addEventListener('click', () => open(other.id));
        backlinks.appendChild(button);
      }
      backBox.hidden = !(links && links.length);
    }

    function fill(note, links) {
      openId = note ? note.id : null;
      openRev = note && note.rev ? note.rev : null;
      // Outra nota no editor: a resposta ao salvar de uma nota nova que
      // ainda venha a caminho ja nao e desta.
      savingNew = false;
      title.value = note ? note.title : '';
      body.value = note ? note.body : '';
      tags.value = note ? note.tags.join(', ') : '';
      describe(note, links);
      confirmBox.hidden = true;
      dirty = false;
      saving = undefined;
      postDraft();
      renderPreview();
    }

    function save() {
      const note = edited();
      if (note.tags.length > TAGS_MAX || note.tags.some((tag) => tag.length > TAG_MAX)) {
        say('Até ' + TAGS_MAX + ' tags, cada uma com até ' + TAG_MAX + ' caracteres.');
        return false;
      }
      if (note.title.length > TITLE_MAX) { say('O título passa de ' + TITLE_MAX + ' caracteres.'); return false; }
      if (new TextEncoder().encode(note.body).length > BODY_MAX_BYTES) {
        say('A nota passa de 200 KiB. Divida-a em duas.');
        return false;
      }
      // Uma nota nova ainda sem id: um segundo pedido criava outra nota. O
      // que se escrever entretanto fica por salvar ate o id chegar.
      if (openId === null && savingNew) { say('A salvar…'); return false; }
      clearTimeout(draftTimer);
      draftTimer = 0;
      post('note-save', note);
      savingNew = openId === null;
      saving = openId;
      dirty = false;
      // O note-save leva o texto todo: o lado nativo larga a copia.
      draftPosted = false;
      say('A salvar…');
      return true;
    }

    // Sair do editor nao deita fora o que se escreveu. `false`: nao da para
    // salvar agora (acima dos tectos, ou a nota nova ainda sem id) -- quem
    // ia sair fica, com o aviso a vista.
    function leave() {
      if (!dirty || edit.hidden) return true;
      const note = edited();
      if (!note.title && !note.body.trim()) return true;
      return save();
    }

    function newNote() {
      if (!leave()) return;
      fill(null, []);
      showEditor();
      say('');
      title.focus();
    }

    function receive(data) {
      switch (data.kind) {
        case 'listed':
          renderList(data);
          break;
        case 'opened': {
          const note = data.note;
          if (data.cause === 'saved') {
            // So mexe no editor se ele ainda mostra esta nota (ou a nova que
            // acabou de ganhar id); senao o utilizador ja seguiu em frente.
            // O texto nao e reescrito: o cursor ficava no fim a cada Ctrl+S.
            const same = openId === note.id || (openId === null && savingNew);
            savingNew = false;
            if (same) {
              saving = undefined;
              openId = note.id;
              openRev = note.rev || null;
              if (!title.value.trim()) title.value = note.title;
              describe(note, data.backlinks);
              // O que se escreveu enquanto a nota nova esperava pelo id: a
              // copia do lado nativo passa a ser a desta nota.
              if (dirty) postDraft();
            }
            say('Nota salva.');
            refresh();
          } else if (openId !== note.id && !leave()) {
            // Uma nota que chega de fora (Ctrl+Shift+Z) com o editor por
            // salvar e que nao da para salvar agora: o editor fica como esta
            // e a nota nova aparece na lista.
            say(data.cause === 'created' ? 'Nota criada a partir da seleção; está na lista.' : '');
            if (data.cause === 'created') refresh();
          } else {
            // Uma nota que chega de fora (Ctrl+Shift+Z) nao deita fora o que
            // estava por salvar no editor: o `leave` acima ja o salvou.
            fill(note, data.backlinks);
            showEditor();
            say(data.cause === 'created' ? 'Nota criada a partir da seleção.' : '');
            if (data.cause === 'created') refresh();
          }
          break;
        }
        case 'deleted':
          if (openId === data.id) { fill(null, []); showList(); }
          say('Nota movida para a lixeira (.trash).');
          refresh();
          break;
        case 'missing':
          say('Essa nota já não existe.');
          refresh();
          break;
        case 'conflict': {
          // A nota mudou fora deste editor (outra janela, o Obsidian): o
          // texto dele ficou numa copia, e o editor passa a mostra-la.
          const note = data.note;
          if (openId === data.original) {
            saving = undefined;
            openId = note.id;
            openRev = note.rev || null;
            title.value = note.title;
            describe(note, []);
          }
          say('Esta nota mudou fora deste editor (outra janela ou o Obsidian). O seu texto ficou numa cópia: ' + note.title);
          refresh();
          break;
        }
        case 'failed':
          savingNew = false;
          // Um salvar que falhou (pasta ocupada, disco cheio, ficheiro preso
          // por outro programa, pedido recusado): o texto continua por salvar
          // -- sair do editor tenta outra vez -- e o lado nativo volta a ter
          // a copia.
          if (saving !== undefined && saving === openId && !edit.hidden) {
            dirty = true;
            postDraft();
          }
          saving = undefined;
          say(data.message);
          break;
      }
    }

    nq.addEventListener('input', () => {
      clearTimeout(searchTimer);
      searchTimer = setTimeout(refresh, 250);
    });
    nq.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); clearTimeout(searchTimer); refresh(); }
    });
    byId('note-new').addEventListener('click', newNote);
    byId('note-back').addEventListener('click', () => { if (leave()) { showList(); refresh(); } });
    byId('note-save').addEventListener('click', save);
    byId('note-delete').addEventListener('click', () => { confirmBox.hidden = false; });
    byId('note-confirm-no').addEventListener('click', () => { confirmBox.hidden = true; });
    byId('note-confirm-yes').addEventListener('click', () => {
      confirmBox.hidden = true;
      if (openId === null) { fill(null, []); showList(); return; }
      post('note-delete', { id: openId });
    });
    // A fonte abre fora do painel, que fecha: salva antes, como o X.
    source.addEventListener('click', () => {
      if (!source.disabled && openSource && leave()) post('open', { input: openSource });
    });
    for (const field of [title, body, tags]) {
      field.addEventListener('input', () => {
        dirty = true;
        say('Alterações por salvar.');
        // A copia do lado nativo nunca fica mais de 200 ms atras, mesmo a
        // escrever sem parar: um temporizador que ja corre nao recomeca
        // (recomecar a cada tecla deixava uma rajada inteira sem copia).
        if (!draftTimer) draftTimer = setTimeout(postDraft, 200);
      });
    }
    body.addEventListener('input', () => {
      clearTimeout(previewTimer);
      previewTimer = setTimeout(renderPreview, 200);
    });
    edit.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && (e.key || '').toLowerCase() === 's') {
        e.preventDefault();
        save();
      }
    });
    return { refresh, receive, newNote, leave, focus: () => (edit.hidden ? nq : title).focus() };
  })();

  // Abas: Historico e Notas.
  const tabs = { history: byId('tab-history'), notes: byId('tab-notes') };
  const views = { history: byId('view-history'), notes: byId('view-notes') };
  function showSection(name) {
    const key = name === 'notes' || name === 'notas' ? 'notes'
      : name === 'history' || name === 'historico' ? 'history' : '';
    if (!key) return false;
    // Sair das Notas salva o editor; se nao der agora, fica-se nas Notas.
    if (key === 'history' && !views.notes.hidden && !notes.leave()) return false;
    for (const other of Object.keys(views)) {
      views[other].hidden = other !== key;
      tabs[other].setAttribute('aria-selected', other === key ? 'true' : 'false');
    }
    if (key === 'notes') { notes.refresh(); notes.focus(); } else { q.focus(); }
    return true;
  }
  const close = () => { if (notes.leave()) post('close'); };
  window.neuraliaShowSection = showSection;
  window.__neuraliaNotes = {
    receive: notes.receive,
    newNote() { if (showSection('notes')) notes.newNote(); },
    // O botao Notas da barra/Home com o painel aberto: nas Notas fecha
    // (como o X, salvando antes), no Historico mostra as Notas.
    button() { if (views.notes.hidden) showSection('notes'); else close(); }
  };
  tabs.history.addEventListener('click', () => showSection('history'));
  tabs.notes.addEventListener('click', () => showSection('notes'));
  byId('close').addEventListener('click', close);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); close(); }
  });

  theme(__THEME__);
  q.focus();
  post('ready');
})();
</script></body></html>"#;
