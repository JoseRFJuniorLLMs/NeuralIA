use super::*;

use std::collections::HashSet;
use std::path::Path;

use neural_core::bookmarks::{
    BookmarkOp, BookmarkStore, BookmarkTree, ChromiumBrowser, ChromiumProfile, Day, ImportError,
    ImportReport, NodeKind, OpOutcome, ROOT_ID, bookmark_key, bookmarkable_url,
    chromium_profiles_in, clean_title, export_file_name, export_netscape_html,
    import_chromium_profile, import_folder_title, parse_netscape_html, read_bookmarks_html,
};
use neural_core::json_store::{LoadOutcome, StoreGrant, StoreMode};

use crate::stores::BOOKMARKS_STORE;

// ===================== os favoritos no app (bookmarks) =====================
//
// O nucleo (`neural_core::bookmarks`) guarda e decide; aqui vive o que o
// liga ao produto:
//
// - o Ctrl+D e um atalho NATIVO (`CommandId::Bookmark`, ambito `Global` no
//   registo de comandos), nunca uma accao do IPC: a origem e o hospedeiro
//   da WebView que recebeu a tecla (`bookmark_target`). Da coluna 2, e a
//   pagina da coluna 2; do Split, a do Split; da janela na Home, abre os
//   Favoritos. A pagina nunca diz qual e o endereco: o lado nativo le-o
//   (`WebView::url()`, ou o `App::page_source` no Leitor e no PDF) e o
//   titulo pelo `DocumentTitle` do WebView2, limpo e cortado a 300;
// - so paginas http/https (`bookmark_candidate`): `about:blank`, `data:`,
//   o `neuralia-pdf` e as outras origens proprias nao;
// - a estrela ☆/★ depois do › de cada coluna e na gaveta do Split
//   (`ColumnButton::Bookmark`, `BarHit::SplitBookmark`): vazia acrescenta,
//   cheia abre «Remover dos favoritos · Mover para ▸ · Abrir Favoritos»;
// - a thread `neural-bookmarks` e a UNICA que escreve o `bookmarks.json`
//   (loja `Explicit` aberta pelo grant do registo): nasce no primeiro uso,
//   nunca no `App::new`, e responde com a arvore inteira;
// - a seccao Favoritos do painel do Ctrl+H: os pedidos dela levam ids,
//   nunca enderecos nem caminhos (`parse_bookmarks_action`);
// - o Ctrl+Shift+Delete nao toca nos favoritos (nao ha `ClearTarget` deles).

/// De onde um favorito foi pedido: a pagina de um hospedeiro, nunca nada que
/// a pagina tenha dito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum BookmarkTarget {
    /// A coluna `index` das IAs.
    Column(usize),
    /// A fonte aberta ao lado da coluna `source` (privada ou nao).
    Split { source: usize, private: bool },
    /// A WebView unica: a Web completa, o Leitor ou o PDF.
    Page,
    /// A janela ou a omnibox: na Home abre os Favoritos; com a WebView
    /// unica a vista, e a pagina dela.
    Window,
    /// Um painel ou os livros: abre os Favoritos.
    Panel,
}

/// Porque o favorito foi pedido: a tecla ou a estrela da barra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum BookmarkVia {
    Shortcut,
    Star,
}

/// O alvo do Ctrl+D pela origem do comando -- o hospedeiro da WebView que
/// recebeu a tecla, a janela ou a omnibox. Pura. O monitor escondido do
/// Gmail e uma coluna que nao existe: nada.
pub(in crate::windows_app) fn bookmark_target(origin: CommandOrigin) -> Option<BookmarkTarget> {
    let host = match origin {
        CommandOrigin::Window | CommandOrigin::Omnibox => return Some(BookmarkTarget::Window),
        CommandOrigin::Host(host) => host,
    };
    match host {
        WebViewHost::Column(index) => {
            (index < COMPARATOR_COLUMNS).then_some(BookmarkTarget::Column(index))
        }
        WebViewHost::Split(index) | WebViewHost::PrivateSplit(index) => {
            (index < COMPARATOR_COLUMNS).then_some(BookmarkTarget::Split {
                source: index,
                private: matches!(host, WebViewHost::PrivateSplit(_)),
            })
        }
        WebViewHost::External | WebViewHost::Reader | WebViewHost::Pdf => {
            Some(BookmarkTarget::Page)
        }
        WebViewHost::Epub
        | WebViewHost::Live
        | WebViewHost::SidePanel
        | WebViewHost::Service(_) => Some(BookmarkTarget::Panel),
        WebViewHost::GmailMonitor => None,
    }
}

/// O endereco de uma pagina como favorito: http/https e nunca uma origem
/// propria do NeuralIA -- nem `neuralia-pdf:` nem o
/// `http://neuralia-pdf.localhost` com que o WebView2 a mostra
/// (`is_custom_scheme_request`). `about:blank` e `data:` tambem nao.
pub(in crate::windows_app) fn bookmark_candidate(url: &str) -> Option<Url> {
    if is_custom_scheme_request(url) {
        return None;
    }
    bookmarkable_url(url)
}

/// O titulo de um favorito: o `DocumentTitle` limpo (sem controlo, ate
/// 300); vazio, o host.
pub(in crate::windows_app) fn bookmark_title(document_title: &str, url: &Url) -> String {
    let title = clean_title(document_title);
    if title.is_empty() {
        clean_title(url.host_str().unwrap_or_default())
    } else {
        title
    }
}

/// A dica da estrela: vazia, o que o clique e o Ctrl+D fazem; cheia, o menu.
pub(in crate::windows_app) fn bookmark_star_tooltip(bookmarked: bool) -> &'static str {
    if bookmarked {
        "Nos favoritos · clique: remover, mover ou abrir Favoritos"
    } else {
        "Adicionar aos favoritos (Ctrl+D)"
    }
}

/// O glifo da estrela: cheia quando a pagina ja e um favorito.
pub(in crate::windows_app) fn bookmark_star_glyph(bookmarked: bool) -> &'static str {
    if bookmarked { "★" } else { "☆" }
}

/// «Adicionado aos favoritos: <titulo>», e no privado o aviso de que ficou
/// gravado porque o utilizador pediu.
pub(in crate::windows_app) fn bookmark_added_notice(title: &str, private: bool) -> String {
    let mut text = format!("Adicionado aos favoritos: {title}");
    if private {
        text.push_str("\nModo privado: favorito guardado porque você pediu.");
    }
    text
}

// ===================== a thread `neural-bookmarks` =====================

/// Porque uma operacao foi pedida (o que o event loop diz quando ela volta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum BookmarkWhy {
    /// Ctrl+D ou a estrela: o titulo e se veio do privado.
    Add {
        title: String,
        private: bool,
    },
    Remove,
    Move,
}

/// Um perfil de outro navegador que se pode importar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct ProfileChoice {
    pub(in crate::windows_app) browser: ChromiumBrowser,
    pub(in crate::windows_app) user_data: PathBuf,
    pub(in crate::windows_app) profile: ChromiumProfile,
}

impl ProfileChoice {
    /// «Chrome · Pessoa 1».
    pub(in crate::windows_app) fn label(&self) -> String {
        format!("{} · {}", self.browser.label(), self.profile.name)
    }
}

/// O que a thread faz. So ela abre o `bookmarks.json`.
#[derive(Debug)]
pub(in crate::windows_app) enum BookmarkJob {
    Load,
    Apply {
        op: BookmarkOp,
        why: BookmarkWhy,
    },
    /// Os perfis do Chrome e do Edge debaixo de `%LOCALAPPDATA%`.
    Profiles {
        local_app_data: PathBuf,
    },
    ImportChromium {
        choice: ProfileChoice,
        folder_title: String,
        now_ms: u64,
    },
    /// Um HTML escolhido pelo utilizador no dialogo.
    ImportHtml {
        path: PathBuf,
        folder_title: String,
        now_ms: u64,
    },
    /// Para o ficheiro escolhido pelo utilizador no dialogo.
    Export {
        path: PathBuf,
    },
}

/// O que a thread responde.
#[derive(Debug)]
pub(in crate::windows_app) enum BookmarkReply {
    Loaded {
        tree: Arc<BookmarkTree>,
        notice: Option<String>,
    },
    Applied {
        tree: Arc<BookmarkTree>,
        outcome: OpOutcome,
        why: BookmarkWhy,
    },
    Imported {
        tree: Arc<BookmarkTree>,
        report: ImportReport,
    },
    Profiles(Vec<ProfileChoice>),
    Exported {
        links: usize,
    },
    Failed(String),
}

/// O que chega ao event loop dos favoritos.
#[derive(Debug)]
pub(in crate::windows_app) enum BookmarksEvent {
    /// O Ctrl+D (resolvido contra a origem) ou um clique numa estrela.
    Request {
        target: BookmarkTarget,
        via: BookmarkVia,
    },
    /// A resposta da thread `neural-bookmarks`.
    Reply(BookmarkReply),
}

/// Uma frase para o utilizador sobre um erro da loja.
fn store_failure(error: &neural_core::bookmarks::BookmarkStoreError) -> String {
    use neural_core::bookmarks::BookmarkStoreError;
    use neural_core::json_store::StoreError;
    match error {
        BookmarkStoreError::Store(StoreError::ReadOnly(_)) => {
            "Os favoritos estão só de leitura: o arquivo bookmarks.json está estragado (há uma cópia em bookmarks.json.bak).".to_string()
        }
        BookmarkStoreError::Op(op) => format!("Favoritos: {op}"),
        other => format!("Não foi possível gravar os favoritos: {other}"),
    }
}

fn import_failure(error: &ImportError) -> String {
    error.pt_br().to_string()
}

/// Os perfis que se podem importar: os do `Local State` do Chrome e do Edge
/// que tem o ficheiro `Bookmarks` (so se ve se existe; nada se le aqui).
pub(in crate::windows_app) fn importable_profiles(local_app_data: &Path) -> Vec<ProfileChoice> {
    let mut choices = Vec::new();
    for browser in ChromiumBrowser::ALL {
        let user_data = browser.user_data_dir(local_app_data);
        for profile in chromium_profiles_in(&user_data) {
            let file = user_data.join(&profile.dir).join("Bookmarks");
            if file.is_file() {
                choices.push(ProfileChoice {
                    browser,
                    user_data: user_data.clone(),
                    profile,
                });
            }
        }
    }
    choices
}

/// Uma tarefa da thread, contra a loja dela. Sem janela: os gates correm-na
/// sobre uma pasta temporaria.
pub(in crate::windows_app) fn run_bookmark_job(
    store: &mut BookmarkStore,
    job: BookmarkJob,
) -> BookmarkReply {
    match job {
        BookmarkJob::Load => {
            let notice = match store.load() {
                LoadOutcome::Degraded { .. } => Some(
                    "Os favoritos estão só de leitura: o arquivo bookmarks.json está estragado (há uma cópia em bookmarks.json.bak).".to_string(),
                ),
                LoadOutcome::Loaded(_) | LoadOutcome::Missing(_) => None,
            };
            BookmarkReply::Loaded {
                tree: Arc::new(store.tree().clone()),
                notice,
            }
        }
        BookmarkJob::Apply { op, why } => match store.apply(op) {
            Ok(applied) => BookmarkReply::Applied {
                tree: Arc::new(applied.tree),
                outcome: applied.outcome,
                why,
            },
            Err(error) => BookmarkReply::Failed(store_failure(&error)),
        },
        BookmarkJob::Profiles { local_app_data } => {
            BookmarkReply::Profiles(importable_profiles(&local_app_data))
        }
        BookmarkJob::ImportChromium {
            choice,
            folder_title,
            now_ms,
        } => match import_chromium_profile(&choice.user_data, &choice.profile) {
            Ok(items) => import_items(store, folder_title, items, now_ms),
            Err(error) => BookmarkReply::Failed(import_failure(&error)),
        },
        BookmarkJob::ImportHtml {
            path,
            folder_title,
            now_ms,
        } => match read_bookmarks_html(&path).and_then(|bytes| parse_netscape_html(&bytes)) {
            Ok(items) => import_items(store, folder_title, items, now_ms),
            Err(error) => BookmarkReply::Failed(import_failure(&error)),
        },
        BookmarkJob::Export { path } => {
            // O que esta no disco agora (outra janela pode ter mudado).
            store.load();
            let tree = store.tree();
            let links = tree
                .nodes()
                .iter()
                .filter(|node| node.kind == NodeKind::Link)
                .count();
            // Um ficheiro que o utilizador escolheu no dialogo "Salvar".
            match std::fs::write(&path, export_netscape_html(tree)) {
                Ok(()) => BookmarkReply::Exported { links },
                Err(error) => BookmarkReply::Failed(format!(
                    "Não foi possível exportar os favoritos: {error}"
                )),
            }
        }
    }
}

fn import_items(
    store: &mut BookmarkStore,
    folder_title: String,
    items: Vec<neural_core::bookmarks::ImportedItem>,
    now_ms: u64,
) -> BookmarkReply {
    match store.apply(BookmarkOp::Import {
        folder_title,
        items,
        added_ms: now_ms,
    }) {
        Ok(applied) => match applied.outcome {
            OpOutcome::Imported { report, .. } => BookmarkReply::Imported {
                tree: Arc::new(applied.tree),
                report,
            },
            other => BookmarkReply::Failed(format!("Importação inesperada: {other:?}")),
        },
        Err(error) => BookmarkReply::Failed(store_failure(&error)),
    }
}

/// Arranca a thread `neural-bookmarks` com o grant da loja e manda-lhe o
/// primeiro `Load`. `None` se a thread nao arrancou.
fn spawn_bookmarks_worker(
    grant: StoreGrant,
    proxy: EventLoopProxy<UserEvent>,
) -> Option<Sender<BookmarkJob>> {
    let (jobs, inbox) = channel::<BookmarkJob>();
    std::thread::Builder::new()
        .name("neural-bookmarks".into())
        .spawn(move || {
            let reply = |reply: BookmarkReply| {
                let _ = proxy.send_event(UserEvent::Bookmarks(BookmarksEvent::Reply(reply)));
            };
            let mut store = match BookmarkStore::open(grant) {
                Ok(store) => store,
                Err(error) => {
                    reply(BookmarkReply::Failed(format!(
                        "Favoritos indisponíveis: {error}"
                    )));
                    return;
                }
            };
            for job in inbox {
                reply(run_bookmark_job(&mut store, job));
            }
        })
        .map_err(|error| debug_log(format_args!("bookmarks: a thread nao arrancou ({error})")))
        .ok()?;
    jobs.send(BookmarkJob::Load).ok()?;
    Some(jobs)
}

// ===================== o estado =====================

/// A arvore que a thread mandou, as chaves dela (o que as estrelas
/// consultam) e a pagina de cada coluna e da fonte ao lado.
#[derive(Default)]
pub(in crate::windows_app) struct BookmarksState {
    jobs: Option<Sender<BookmarkJob>>,
    tree: Option<Arc<BookmarkTree>>,
    keys: HashSet<String>,
    /// A chave da ultima pagina carregada em cada coluna (`PageLoaded`).
    columns: [Option<String>; COMPARATOR_COLUMNS],
    /// A da fonte aberta ao lado.
    split: Option<String>,
    /// A seccao Favoritos pediu a lista antes de a arvore chegar.
    panel_waiting: bool,
}

impl BookmarksState {
    /// A pagina com esta chave e um favorito?
    fn has(&self, key: Option<&String>) -> bool {
        key.is_some_and(|key| self.keys.contains(key))
    }

    /// As estrelas cheias das colunas, para a barra.
    pub(in crate::windows_app) fn column_stars(&self) -> [bool; COMPARATOR_COLUMNS] {
        std::array::from_fn(|index| self.has(self.columns[index].as_ref()))
    }

    pub(in crate::windows_app) fn split_star(&self) -> bool {
        self.has(self.split.as_ref())
    }

    fn set_tree(&mut self, tree: Arc<BookmarkTree>) {
        self.keys = tree.url_keys();
        self.tree = Some(tree);
    }
}

// ===================== a seccao Favoritos do painel =====================

/// O que a seccao Favoritos pode pedir. So ids -- nunca um endereco nem um
/// caminho.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum BookmarkPanelRequest {
    List,
    Open(u64),
    Remove(u64),
    ImportChrome,
    ImportFile,
    Export,
}

/// O id de um favorito vindo da pagina: um inteiro JSON acima da raiz e
/// dentro do que o JS representa exacto.
fn panel_bookmark_id(args: Option<&serde_json::Value>) -> Option<u64> {
    let id = exact_keys(args, &["id"])?.get("id")?.as_u64()?;
    (ROOT_ID < id && id <= (1u64 << 53)).then_some(id)
}

/// O parser da seccao Favoritos (prefixos `bookmark` e `bookmarks`). Os
/// pedidos levam `{}` ou `{"id": N}` e mais nada.
pub(in crate::windows_app) fn parse_bookmarks_action(
    action: &str,
    args: Option<&serde_json::Value>,
) -> Option<PanelMessage> {
    let request = match action {
        "bookmarks-list" => exact_keys(args, &[]).map(|_| BookmarkPanelRequest::List),
        "bookmark-open" => panel_bookmark_id(args).map(BookmarkPanelRequest::Open),
        "bookmark-remove" => panel_bookmark_id(args).map(BookmarkPanelRequest::Remove),
        "bookmarks-import-chrome" => {
            exact_keys(args, &[]).map(|_| BookmarkPanelRequest::ImportChrome)
        }
        "bookmarks-import-file" => exact_keys(args, &[]).map(|_| BookmarkPanelRequest::ImportFile),
        "bookmarks-export" => exact_keys(args, &[]).map(|_| BookmarkPanelRequest::Export),
        _ => None,
    }?;
    Some(PanelMessage::Bookmarks(request))
}

/// A arvore como a seccao a mostra: cada no com o id, a profundidade, o
/// tipo, o titulo e (nos favoritos) o endereco. Entra na pagina como texto.
pub(in crate::windows_app) fn bookmarks_panel_items(tree: &BookmarkTree) -> Vec<serde_json::Value> {
    tree.walk()
        .into_iter()
        .map(|(depth, node)| {
            let kind = match node.kind {
                NodeKind::Folder => "folder",
                NodeKind::Link => "link",
            };
            let title = if node.title.is_empty() {
                Url::parse(&node.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_string))
                    .unwrap_or_default()
            } else {
                node.title.clone()
            };
            serde_json::json!({
                "id": node.id,
                "depth": depth,
                "kind": kind,
                "title": title,
                "detail": node.url,
            })
        })
        .collect()
}

/// O script que entrega a seccao a lista (ou so um aviso).
pub(in crate::windows_app) fn bookmarks_panel_script(
    tree: Option<&BookmarkTree>,
    notice: Option<&str>,
) -> String {
    let mut data = serde_json::Map::new();
    if let Some(tree) = tree {
        data.insert(
            "items".into(),
            serde_json::Value::Array(bookmarks_panel_items(tree)),
        );
    }
    if let Some(notice) = notice {
        data.insert("notice".into(), serde_json::Value::String(notice.into()));
    }
    format!(
        "window.__neuraliaBookmarks && window.__neuraliaBookmarks.receive({});",
        serde_json::Value::Object(data)
    )
}

// ===================== o menu da estrela cheia =====================

/// «Remover dos favoritos».
pub(in crate::windows_app) const STAR_MENU_REMOVE: usize = 1;
/// «Abrir Favoritos».
pub(in crate::windows_app) const STAR_MENU_OPEN: usize = 2;
/// «Mover para ▸»: a pasta N da lista e este id mais N.
pub(in crate::windows_app) const STAR_MENU_MOVE_BASE: usize = 100;

/// O que se escolheu no menu da estrela cheia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum StarChoice {
    Remove,
    MoveTo(u64),
    OpenPanel,
}

/// O menu da estrela cheia do favorito `id`: «Remover dos favoritos»,
/// «Mover para ▸» com as pastas (a de agora marcada e cinzenta) e «Abrir
/// Favoritos». Devolve tambem os ids das pastas, pela ordem do submenu.
pub(in crate::windows_app) fn star_menu(tree: &BookmarkTree, id: u64) -> (PopupMenu, Vec<u64>) {
    let current = tree.get(id).map(|node| node.parent);
    let folders: Vec<(usize, u64, String)> = tree
        .folders()
        .into_iter()
        .map(|(depth, node)| (depth, node.id, node.title.clone()))
        .collect();
    let mut menu = PopupMenu::default();
    menu.push(MenuCommand::new(STAR_MENU_REMOVE, "Remover dos favoritos"));
    let entries = folders
        .iter()
        .enumerate()
        .map(|(index, (depth, folder, title))| {
            let label = format!("{}{title}", "   ".repeat(*depth));
            let mut command = MenuCommand::new(STAR_MENU_MOVE_BASE + index, label)
                .checked(current == Some(*folder));
            if current == Some(*folder) {
                command = command.disabled("");
            }
            MenuEntry::Command(command)
        })
        .collect();
    menu.push(MenuEntry::Submenu {
        label: "Mover para".into(),
        icon: None,
        entries,
    });
    menu.separator();
    menu.push(MenuCommand::new(STAR_MENU_OPEN, "Abrir Favoritos").moves_focus());
    (
        menu,
        folders.into_iter().map(|(_, folder, _)| folder).collect(),
    )
}

/// O id devolvido pelo `TrackPopupMenu` como escolha. 0 (fechado) ou um id
/// fora do menu: nada.
pub(in crate::windows_app) fn star_menu_choice(
    picked: usize,
    folders: &[u64],
) -> Option<StarChoice> {
    match picked {
        STAR_MENU_REMOVE => Some(StarChoice::Remove),
        STAR_MENU_OPEN => Some(StarChoice::OpenPanel),
        other => other
            .checked_sub(STAR_MENU_MOVE_BASE)
            .and_then(|index| folders.get(index))
            .map(|folder| StarChoice::MoveTo(*folder)),
    }
}

// ===================== o dia local e os dialogos =====================

/// Hoje, no relogio do Windows (o dia local, para a pasta da importacao e o
/// nome do ficheiro exportado).
fn local_day() -> Day {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut now = SYSTEMTIME::default();
    // SAFETY: `now` e um SYSTEMTIME valido que o Windows preenche.
    unsafe { GetLocalTime(&mut now) };
    Day {
        year: now.wYear,
        month: now.wMonth as u8,
        day: now.wDay as u8,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// O filtro dos dialogos: pares descricao/padrao terminados em NUL e a
/// lista inteira terminada por um NUL a mais.
fn html_filter() -> Vec<u16> {
    let mut filter = Vec::new();
    for part in ["Favoritos em HTML (*.html; *.htm)", "*.html;*.htm"] {
        filter.extend(part.encode_utf16());
        filter.push(0);
    }
    filter.push(0);
    filter
}

/// O dialogo "Abrir" do Windows para um HTML de favoritos.
fn pick_bookmarks_file(owner: HWND) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let filter = html_filter();
    let title = wide_null("Importar favoritos de um arquivo HTML");
    let mut buffer = vec![0u16; 4096];
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: title.as_ptr(),
        Flags: OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_HIDEREADONLY,
        ..Default::default()
    };
    // SAFETY: `dialog` aponta para buffers vivos ate ao fim da chamada; o
    // Windows escreve no maximo `nMaxFile` unidades em `buffer`.
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        return None;
    }
    let len = buffer.iter().position(|unit| *unit == 0)?;
    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

/// O dialogo "Salvar como" do Windows, com `default_name` ja escrito.
fn pick_export_file(owner: HWND, default_name: &str) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_EXPLORER, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let filter = html_filter();
    let title = wide_null("Exportar favoritos");
    let extension = wide_null("html");
    let mut buffer = vec![0u16; 4096];
    for (slot, unit) in buffer.iter_mut().zip(default_name.encode_utf16()) {
        *slot = unit;
    }
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: extension.as_ptr(),
        Flags: OFN_EXPLORER
            | OFN_OVERWRITEPROMPT
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_HIDEREADONLY,
        ..Default::default()
    };
    // SAFETY: como no dialogo "Abrir".
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        return None;
    }
    let len = buffer.iter().position(|unit| *unit == 0)?;
    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

/// O `DocumentTitle` do WebView2 de uma WebView (vazio se nao o der).
fn document_title(webview: &WebView) -> String {
    use webview2_com::take_pwstr;
    use windows_core::PWSTR;
    use wry::WebViewExtWindows;
    let core = webview.webview();
    let mut title = PWSTR::null();
    // SAFETY: `title` recebe uma string alocada pelo WebView2, que o
    // `take_pwstr` liberta.
    match unsafe { core.DocumentTitle(&mut title) } {
        Ok(()) => take_pwstr(title),
        Err(_) => String::new(),
    }
}

// ===================== o App =====================

impl App {
    /// O unico braco dos favoritos no `user_event`.
    pub(in crate::windows_app) fn bookmarks_event(&mut self, event: BookmarksEvent) {
        match event {
            BookmarksEvent::Request { target, via } => self.bookmark_request(target, via),
            BookmarksEvent::Reply(reply) => self.bookmark_reply(reply),
        }
    }

    /// A thread, arrancada no primeiro uso. `false`: sem pasta de dados.
    fn ensure_bookmarks_worker(&mut self) -> bool {
        if self.bookmarks.jobs.is_some() {
            return true;
        }
        let Some(grant) = self
            .stores
            .as_ref()
            .and_then(|stores| stores.grant(BOOKMARKS_STORE).ok())
        else {
            return false;
        };
        self.bookmarks.jobs = spawn_bookmarks_worker(grant, self.proxy.clone());
        self.bookmarks.jobs.is_some()
    }

    fn submit_bookmark_job(&mut self, job: BookmarkJob) {
        if !self.ensure_bookmarks_worker() {
            self.show_splash(
                "Favoritos indisponíveis: sem pasta de dados.".to_string(),
                4,
            );
            return;
        }
        let sent = self
            .bookmarks
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());
        if !sent {
            // A thread morreu: a proxima tentativa arranca outra.
            self.bookmarks.jobs = None;
            self.show_splash("Os favoritos não responderam.".to_string(), 4);
        }
    }

    /// Uma pagina acabou de carregar: a chave dela para a estrela da
    /// coluna ou do Split. A thread arranca aqui (primeira pagina web), para
    /// as estrelas saberem o que ja e favorito.
    pub(in crate::windows_app) fn bookmarks_page_loaded(&mut self, page: WebViewHost, url: &str) {
        let key = bookmark_candidate(url).map(|url| bookmark_key(&url));
        match page {
            WebViewHost::Column(index) if index < COMPARATOR_COLUMNS => {
                self.bookmarks.columns[index] = key;
            }
            WebViewHost::Split(_) | WebViewHost::PrivateSplit(_) => self.bookmarks.split = key,
            _ => return,
        }
        self.ensure_bookmarks_worker();
        self.request_redraw();
    }

    /// A WebView de um alvo e o endereco que o lado nativo conhece dela.
    fn bookmark_page(&self, target: BookmarkTarget) -> Option<(&WebView, String, bool)> {
        match target {
            BookmarkTarget::Column(index) => {
                let view = self.comparator.as_ref()?.views.get(index)?;
                Some((&view.webview, view.webview.url().ok()?, false))
            }
            BookmarkTarget::Split { source, private } => {
                let split = self.comparator.as_ref()?.split.as_ref()?;
                (split.source_index == source && split.private == private).then_some(())?;
                Some((&split.webview, split.webview.url().ok()?, private))
            }
            BookmarkTarget::Page => {
                let webview = self.webview.as_ref()?;
                let url = match self.surface {
                    // O HTML do Leitor e local e o visualizador do PDF e
                    // nosso: o endereco verdadeiro e o `page_source`.
                    Surface::Reader | Surface::Pdf => self.page_source.clone()?,
                    Surface::External => webview.url().ok()?,
                    Surface::Home | Surface::Comparator | Surface::Epub => return None,
                };
                Some((webview, url, false))
            }
            BookmarkTarget::Window | BookmarkTarget::Panel => None,
        }
    }

    /// O Ctrl+D ou um clique numa estrela.
    fn bookmark_request(&mut self, target: BookmarkTarget, via: BookmarkVia) {
        let target = match target {
            BookmarkTarget::Panel => {
                self.show_bookmarks_panel();
                return;
            }
            BookmarkTarget::Window => match self.surface {
                Surface::External | Surface::Reader | Surface::Pdf => BookmarkTarget::Page,
                Surface::Home | Surface::Comparator | Surface::Epub => {
                    self.show_bookmarks_panel();
                    return;
                }
            },
            other => other,
        };
        let Some((raw_url, document, private)) = self
            .bookmark_page(target)
            .map(|(webview, url, private)| (url, document_title(webview), private))
        else {
            return;
        };
        let Some(url) = bookmark_candidate(&raw_url) else {
            self.show_splash(
                "Só páginas da web (http ou https) podem ser favoritos.".to_string(),
                3,
            );
            return;
        };
        let title = bookmark_title(&document, &url);
        let key = bookmark_key(&url);
        // A estrela tambem segue o endereco de agora (uma navegacao dentro
        // da pagina nao da `PageLoaded`).
        match target {
            BookmarkTarget::Column(index) => self.bookmarks.columns[index] = Some(key.clone()),
            BookmarkTarget::Split { .. } => self.bookmarks.split = Some(key.clone()),
            _ => {}
        }
        let existing = self
            .bookmarks
            .tree
            .as_ref()
            .and_then(|tree| tree.find_url(&url));
        if let Some(id) = existing {
            match via {
                BookmarkVia::Star => self.show_star_menu(id),
                BookmarkVia::Shortcut => self.show_splash(
                    format!("Já está nos favoritos: {title} · clique na ★ para remover ou mover"),
                    3,
                ),
            }
            return;
        }
        // O modo privado (o Split privado, ou o registo em `Private`) guarda
        // na mesma: foi o utilizador que pediu (loja `Explicit`).
        let private = private
            || self
                .stores
                .as_ref()
                .is_some_and(|stores| stores.mode() == StoreMode::Private);
        self.submit_bookmark_job(BookmarkJob::Apply {
            op: BookmarkOp::AddLink {
                parent: ROOT_ID,
                title: title.clone(),
                url: url.to_string(),
                added_ms: now_ms(),
            },
            why: BookmarkWhy::Add { title, private },
        });
    }

    /// O menu da estrela cheia, na estrela (o cursor).
    fn show_star_menu(&mut self, id: u64) {
        let Some(tree) = self.bookmarks.tree.clone() else {
            return;
        };
        let (menu, folders) = star_menu(&tree, id);
        let picked = self.track_menu(&menu, cursor_point(), MenuButton::Left);
        match star_menu_choice(picked, &folders) {
            Some(StarChoice::Remove) => self.submit_bookmark_job(BookmarkJob::Apply {
                op: BookmarkOp::Delete { id },
                why: BookmarkWhy::Remove,
            }),
            Some(StarChoice::MoveTo(parent)) => self.submit_bookmark_job(BookmarkJob::Apply {
                op: BookmarkOp::Move { id, parent },
                why: BookmarkWhy::Move,
            }),
            Some(StarChoice::OpenPanel) => self.show_bookmarks_panel(),
            None => {}
        }
    }

    /// Abre (se preciso) o painel do Ctrl+H ja nos Favoritos.
    pub(in crate::windows_app) fn show_bookmarks_panel(&mut self) {
        if !self.side_panel.is_open() {
            self.open_side_panel();
        }
        if !self.side_panel.is_open() {
            return;
        }
        self.panel_run(PANEL_SHOW_BOOKMARKS_SCRIPT.to_string());
    }

    /// A seccao mostra a lista (ou pede-a a thread, e mostra quando chegar).
    fn bookmarks_panel_render(&mut self, notice: Option<&str>) {
        let script = bookmarks_panel_script(self.bookmarks.tree.as_deref(), notice);
        self.panel_run(script);
    }

    /// Um pedido da seccao Favoritos (so ids).
    pub(in crate::windows_app) fn bookmark_panel_request(&mut self, request: BookmarkPanelRequest) {
        match request {
            BookmarkPanelRequest::List => {
                if self.bookmarks.tree.is_some() {
                    self.bookmarks_panel_render(None);
                } else {
                    self.bookmarks.panel_waiting = true;
                    if !self.ensure_bookmarks_worker() {
                        self.bookmarks.panel_waiting = false;
                        self.bookmarks_panel_render(Some(
                            "Favoritos indisponíveis: sem pasta de dados.",
                        ));
                    }
                }
            }
            BookmarkPanelRequest::Open(id) => {
                let url = self
                    .bookmarks
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.get(id))
                    .filter(|node| node.kind == NodeKind::Link)
                    .map(|node| node.url.clone());
                if let Some(url) = url {
                    self.close_side_panel(PanelExit::OpenItem);
                    self.web(url);
                }
            }
            BookmarkPanelRequest::Remove(id) => {
                let known = self
                    .bookmarks
                    .tree
                    .as_ref()
                    .is_some_and(|tree| tree.get(id).is_some());
                if known {
                    self.submit_bookmark_job(BookmarkJob::Apply {
                        op: BookmarkOp::Delete { id },
                        why: BookmarkWhy::Remove,
                    });
                }
            }
            BookmarkPanelRequest::ImportChrome => {
                let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
                else {
                    self.bookmarks_panel_render(Some(
                        "Não foram encontrados favoritos do Chrome nem do Edge.",
                    ));
                    return;
                };
                self.bookmarks_panel_render(Some("A procurar os favoritos do Chrome e do Edge…"));
                self.submit_bookmark_job(BookmarkJob::Profiles { local_app_data });
            }
            BookmarkPanelRequest::ImportFile => {
                let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
                    return;
                };
                if let Some(path) = pick_bookmarks_file(owner) {
                    self.bookmarks_panel_render(Some("A importar…"));
                    self.submit_bookmark_job(BookmarkJob::ImportHtml {
                        path,
                        folder_title: import_folder_title("arquivo HTML", local_day()),
                        now_ms: now_ms(),
                    });
                }
            }
            BookmarkPanelRequest::Export => {
                let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
                    return;
                };
                if let Some(path) = pick_export_file(owner, &export_file_name(local_day())) {
                    self.submit_bookmark_job(BookmarkJob::Export { path });
                }
            }
        }
    }

    /// Os perfis que a thread achou: nenhum, um (importa ja) ou varios (um
    /// menu para escolher).
    fn bookmark_profiles(&mut self, choices: Vec<ProfileChoice>) {
        let choice = match choices.len() {
            0 => {
                self.bookmarks_panel_render(Some(
                    "Não foram encontrados favoritos do Chrome nem do Edge.",
                ));
                return;
            }
            1 => choices.into_iter().next(),
            _ => {
                let mut menu = PopupMenu::default();
                menu.push(MenuCommand::new(0, "Importar de").disabled(""));
                for (index, choice) in choices.iter().enumerate() {
                    menu.push(MenuCommand::new(index + 1, choice.label()));
                }
                let picked = self.track_menu(&menu, cursor_point(), MenuButton::Left);
                picked
                    .checked_sub(1)
                    .and_then(|index| choices.into_iter().nth(index))
            }
        };
        let Some(choice) = choice else {
            self.bookmarks_panel_render(Some(""));
            return;
        };
        let folder_title = import_folder_title(choice.browser.label(), local_day());
        self.bookmarks_panel_render(Some("A importar…"));
        self.submit_bookmark_job(BookmarkJob::ImportChromium {
            choice,
            folder_title,
            now_ms: now_ms(),
        });
    }

    /// A resposta da thread.
    fn bookmark_reply(&mut self, reply: BookmarkReply) {
        match reply {
            BookmarkReply::Loaded { tree, notice } => {
                self.bookmarks.set_tree(tree);
                if std::mem::take(&mut self.bookmarks.panel_waiting) || notice.is_some() {
                    self.bookmarks_panel_render(notice.as_deref());
                }
                self.request_redraw();
            }
            BookmarkReply::Applied { tree, outcome, why } => {
                self.bookmarks.set_tree(tree);
                match (why, outcome) {
                    (BookmarkWhy::Add { title, private }, OpOutcome::Added(_)) => {
                        self.show_splash(bookmark_added_notice(&title, private), 3);
                    }
                    (BookmarkWhy::Add { title, .. }, _) => {
                        self.show_splash(format!("Já está nos favoritos: {title}"), 3);
                    }
                    (BookmarkWhy::Remove, _) => {
                        self.show_splash("Removido dos favoritos".to_string(), 2);
                    }
                    (BookmarkWhy::Move, _) => {
                        self.show_splash("Favorito movido".to_string(), 2);
                    }
                }
                self.bookmarks_panel_render(None);
                self.request_redraw();
            }
            BookmarkReply::Imported { tree, report } => {
                self.bookmarks.set_tree(tree);
                let summary = report.summary();
                self.bookmarks_panel_render(Some(&summary));
                self.show_splash(summary, 4);
                self.request_redraw();
            }
            BookmarkReply::Profiles(choices) => self.bookmark_profiles(choices),
            BookmarkReply::Exported { links } => {
                let text = format!("{links} favoritos exportados");
                self.bookmarks_panel_render(Some(&text));
                self.show_splash(text, 3);
            }
            BookmarkReply::Failed(message) => {
                self.bookmarks.panel_waiting = false;
                self.bookmarks_panel_render(Some(&message));
                self.show_splash(message, 5);
            }
        }
    }
}
