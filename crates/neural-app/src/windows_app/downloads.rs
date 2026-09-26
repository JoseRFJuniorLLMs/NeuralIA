use super::*;

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use neural_core::downloads::{
    DeleteReason, DownloadEffect, DownloadEnd, DownloadEvent, DownloadId, DownloadLog,
    DownloadManager, DownloadNotice, DownloadSettings, DownloadStart, LOG_MAX_BYTES, LOG_VERSION,
    ProgressThrottle, SETTINGS_MAX_BYTES, SETTINGS_VERSION, WebViewKey, finalize_download,
    unique_path,
};
use neural_core::file_risk::{BlockReason, display_label};
use neural_core::json_store::{SaveOutcome, VersionedJsonStore};

use crate::stores::{DOWNLOADS_LOG_STORE, DOWNLOADS_SETTINGS_STORE};

// ===================== o gestor de downloads (downloads-manager) =====================
//
// Modulo de feature (o padrao do infra-seams): `UserEvent::Download` com o
// `DownloadEvent` do `neural_core::downloads`, o campo `App::downloads` e o
// braco `download_event`. O que decide esta todo no `DownloadManager` puro;
// aqui fica o COM do WebView2 e o que o `App` faz com os efeitos.
//
// - Registo: `register_download_manager`, pela tabela dos ganchos
//   (`HookRegistrar::downloads`) em cada WebView com `DownloadPolicy::Managed`
//   -- as colunas, a fonte ao lado (normal e privada), a Web completa e os
//   servicos. As paginas locais recusam no builder (`Deny`) e nunca chegam
//   aqui.
// - `DownloadStarting`: toma SEMPRE o deferral e manda o `Starting` ao event
//   loop; o gestor responde recusar (`SetCancel`), seguir (com o caminho) ou
//   perguntar. Um erro antes da decisao recusa.
// - `BytesReceivedChanged`: filtrado a 250 ms (`ProgressThrottle`) antes de
//   sair do handler. `StateChanged`: o fim (acabado, cancelado,
//   interrompido), que larga a operacao e, se acabou, corre o
//   `finalize_download` (sniff, depois a marca da Web ou apagar).
// - A WebView destruida: o handler do `DownloadStarting` guarda um
//   `WebViewLife`; quando o WebView2 o larga, o gestor recebe `WebViewGone`
//   e cancela e larga as operacoes dessa WebView.
// - A pasta escolhida (`downloads-settings.json`) vai para o perfil de cada
//   WebView com o `SetDefaultDownloadFolderPath` -- o InPrivate incluido.
// - O registo (`downloads.json`, `StoreKind::Automatic`) nunca guarda um
//   download do Split privado nem de um servico InPrivate (o gestor), e no
//   Modo privado nao se escreve (a loja).

/// Um download desta WebView nunca vai para o registo: o Split privado e os
/// servicos InPrivate (a Respiracao).
pub(in crate::windows_app) fn download_host_is_private(host: WebViewHost) -> bool {
    match host {
        WebViewHost::PrivateSplit(_) => true,
        WebViewHost::Service(service) => service.private(),
        WebViewHost::Column(_)
        | WebViewHost::Split(_)
        | WebViewHost::External
        | WebViewHost::Reader
        | WebViewHost::Pdf
        | WebViewHost::Epub
        | WebViewHost::Live
        | WebViewHost::GmailMonitor
        | WebViewHost::SidePanel => false,
    }
}

// ===================== as operacoes do WebView2 =====================

/// O que o gestor faz a uma operacao de download. O produto passa o COM
/// (`ComDownload`); os testes um registo.
pub(in crate::windows_app) trait DownloadHandle {
    /// Completa o deferral do `DownloadStarting`: `Some(path)` segue para
    /// `path`, `None` recusa (`SetCancel`). Sem deferral a espera, nada.
    fn complete_start(&mut self, path: Option<&Path>) -> Result<(), String>;
    /// Cancela: o deferral, se ainda esta a espera; senao o `Cancel()` da
    /// operacao.
    fn cancel(&mut self) -> Result<(), String>;
}

/// As operacoes vivas, pelo numero do download, com a WebView de cada uma.
/// Largadas quando o download acaba e quando a WebView dele e destruida
/// (`ForgetOp`).
pub(in crate::windows_app) struct DownloadOps<H> {
    ops: BTreeMap<DownloadId, (WebViewKey, H)>,
}

impl<H> Default for DownloadOps<H> {
    fn default() -> Self {
        Self {
            ops: BTreeMap::new(),
        }
    }
}

/// O que `DownloadOps::apply` fez com um efeito.
#[derive(Debug, PartialEq)]
pub(in crate::windows_app) enum OpsOutcome {
    Done,
    /// A chamada COM falhou (a linha de log diz porque).
    Failed(String),
    /// Nao e das operacoes: o `App` trata dele.
    NotMine(DownloadEffect),
}

impl<H: DownloadHandle> DownloadOps<H> {
    pub(in crate::windows_app) fn insert(
        &mut self,
        id: DownloadId,
        webview: WebViewKey,
        handle: H,
    ) {
        self.ops.insert(id, (webview, handle));
    }

    pub(in crate::windows_app) fn forget(&mut self, id: DownloadId) -> bool {
        self.ops.remove(&id).is_some()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) fn len(&self) -> usize {
        self.ops.len()
    }

    /// Os downloads vivos de uma WebView.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) fn of_webview(&self, webview: WebViewKey) -> Vec<DownloadId> {
        self.ops
            .iter()
            .filter(|(_, (key, _))| *key == webview)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Aplica um efeito do gestor as operacoes: recusar, seguir, cancelar,
    /// largar. Os outros voltam para o `App`.
    pub(in crate::windows_app) fn apply(&mut self, effect: DownloadEffect) -> OpsOutcome {
        let result = match &effect {
            DownloadEffect::Refuse(id) => match self.ops.remove(id) {
                Some((_, mut handle)) => handle.complete_start(None),
                None => Ok(()),
            },
            DownloadEffect::Proceed { id, path } => match self.ops.get_mut(id) {
                Some((_, handle)) => handle.complete_start(Some(path)),
                None => Ok(()),
            },
            DownloadEffect::CancelRunning(id) => match self.ops.get_mut(id) {
                Some((_, handle)) => handle.cancel(),
                None => Ok(()),
            },
            DownloadEffect::ForgetOp(id) => {
                self.ops.remove(id);
                Ok(())
            }
            _ => return OpsOutcome::NotMine(effect),
        };
        match result {
            Ok(()) => OpsOutcome::Done,
            Err(error) => OpsOutcome::Failed(error),
        }
    }
}

/// Uma operacao de download do WebView2, com o deferral do
/// `DownloadStarting` enquanto o gestor nao decide. Um deferral nunca fica
/// pendurado: largado sem decisao, recusa.
pub(in crate::windows_app) struct ComDownload {
    operation: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation,
    /// O `ResultFilePath` que o WebView2 propos (ja com o numero dele, se o
    /// nome estava ocupado).
    proposed: PathBuf,
    start: Option<(
        webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadStartingEventArgs,
        webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Deferral,
    )>,
}

impl DownloadHandle for ComDownload {
    fn complete_start(&mut self, path: Option<&Path>) -> Result<(), String> {
        use windows_core::HSTRING;
        let Some((args, deferral)) = self.start.take() else {
            return Ok(());
        };
        let set = match path {
            // Outra pasta que a proposta: o nome livre nessa pasta (o
            // WebView2 escreveria por cima de um ficheiro que la esteja).
            Some(path) if path != self.proposed.as_path() => {
                let dir = path.parent().unwrap_or(Path::new(""));
                let name = neural_core::downloads::file_name_of(path);
                let free = unique_path(dir, name, |candidate| candidate.exists());
                unsafe { args.SetResultFilePath(&HSTRING::from(free.as_path())) }
                    .map_err(|error| format!("SetResultFilePath falhou: {error}"))
            }
            Some(_) => Ok(()),
            None => unsafe { args.SetCancel(true) }
                .map_err(|error| format!("SetCancel falhou: {error}")),
        };
        if set.is_err() {
            // Sem o caminho decidido, nao segue.
            let _ = unsafe { args.SetCancel(true) };
        }
        let completed = unsafe { deferral.Complete() }
            .map_err(|error| format!("Complete do deferral falhou: {error}"));
        set.and(completed)
    }

    fn cancel(&mut self) -> Result<(), String> {
        if self.start.is_some() {
            return self.complete_start(None);
        }
        unsafe { self.operation.Cancel() }.map_err(|error| format!("Cancel falhou: {error}"))
    }
}

impl Drop for ComDownload {
    fn drop(&mut self) {
        if self.start.is_some() {
            let _ = self.complete_start(None);
        }
    }
}

/// O que cada WebView gerida partilha com o `App`: as operacoes vivas, os
/// numeros e a pasta escolhida. Tudo na thread da interface (os handlers do
/// WebView2 correm nela).
#[derive(Clone)]
pub(in crate::windows_app) struct DownloadsShared {
    pub(in crate::windows_app) ops: Rc<RefCell<DownloadOps<ComDownload>>>,
    next_id: Rc<Cell<u64>>,
    next_webview: Rc<Cell<u64>>,
    /// A pasta escolhida, ja conferida no disco.
    pub(in crate::windows_app) folder: Rc<RefCell<Option<PathBuf>>>,
}

impl DownloadsShared {
    fn new(folder: Option<PathBuf>) -> Self {
        Self {
            ops: Rc::default(),
            next_id: Rc::new(Cell::new(0)),
            next_webview: Rc::new(Cell::new(0)),
            folder: Rc::new(RefCell::new(folder)),
        }
    }

    fn next_download_id(&self) -> DownloadId {
        let id = self.next_id.get().wrapping_add(1);
        self.next_id.set(id);
        DownloadId(id)
    }

    fn next_webview_key(&self) -> WebViewKey {
        let key = self.next_webview.get().wrapping_add(1);
        self.next_webview.set(key);
        WebViewKey(key)
    }
}

/// Vive dentro do handler do `DownloadStarting` de uma WebView: quando o
/// WebView2 larga o handler (a WebView foi destruida), o gestor recebe
/// `WebViewGone`.
struct WebViewLife {
    key: WebViewKey,
    proxy: EventLoopProxy<UserEvent>,
}

impl Drop for WebViewLife {
    fn drop(&mut self) {
        debug_log(format_args!("downloads: webview {} destruida", self.key.0));
        let _ = self
            .proxy
            .send_event(UserEvent::Download(DownloadEvent::WebViewGone {
                webview: self.key,
            }));
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// O gestor numa WebView acabada de construir: a pasta escolhida no perfil
/// dela e o `DownloadStarting`. So `ComHookRegistrar::downloads` o chama.
pub(in crate::windows_app) fn register_download_manager(
    webview: &WebView,
    host: WebViewHost,
    shared: &DownloadsShared,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    use webview2_com::{
        DownloadStartingEventHandler,
        Microsoft::Web::WebView2::Win32::{ICoreWebView2_4, ICoreWebView2_13},
    };
    use windows_core::{HSTRING, Interface};
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let core4 = core
        .cast::<ICoreWebView2_4>()
        .map_err(|error| format!("ICoreWebView2_4 indisponível: {error}"))?;
    if let Some(folder) = shared.folder.borrow().clone() {
        // Sem ICoreWebView2_13 a pasta chega pelo `SetResultFilePath` de
        // cada download (`target_path`).
        let set = core
            .cast::<ICoreWebView2_13>()
            .and_then(|core13| unsafe { core13.Profile() })
            .and_then(|profile| unsafe {
                profile.SetDefaultDownloadFolderPath(&HSTRING::from(folder.as_path()))
            });
        if let Err(error) = set {
            debug_log(format_args!(
                "downloads: {} sem a pasta escolhida no perfil ({error})",
                host.describe()
            ));
        }
    }
    let key = shared.next_webview_key();
    let private = download_host_is_private(host);
    let life = WebViewLife {
        key,
        proxy: proxy.clone(),
    };
    let shared = shared.clone();
    let handler = DownloadStartingEventHandler::create(Box::new(move |_, args| {
        let _alive = &life;
        let Some(args) = args else {
            return Ok(());
        };
        if let Err(error) = download_starting(&args, key, private, &shared, &proxy) {
            debug_log(format_args!(
                "downloads: recusado antes da decisao ({error})"
            ));
            let _ = unsafe { args.SetCancel(true) };
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core4.add_DownloadStarting(&handler, &mut token) }
        .map_err(|error| format!("add_DownloadStarting falhou: {error}"))
}

/// Um download novo: o deferral, os avisos da operacao e o `Starting` para
/// o gestor. Sem o gestor a responder (event loop fechado), recusa.
fn download_starting(
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadStartingEventArgs,
    webview: WebViewKey,
    private: bool,
    shared: &DownloadsShared,
    proxy: &EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    use webview2_com::take_pwstr;
    use windows_core::PWSTR;

    let operation = unsafe { args.DownloadOperation() }.map_err(|error| error.to_string())?;
    let proposed = {
        let mut path = PWSTR::null();
        unsafe { args.ResultFilePath(&mut path) }.map_err(|error| error.to_string())?;
        PathBuf::from(take_pwstr(path))
    };
    let uri = {
        let mut uri = PWSTR::null();
        unsafe { operation.Uri(&mut uri) }.map_err(|error| error.to_string())?;
        take_pwstr(uri)
    };
    let mut total = 0i64;
    unsafe { operation.TotalBytesToReceive(&mut total) }.map_err(|error| error.to_string())?;
    let id = shared.next_download_id();
    watch_download(&operation, id, proxy.clone())?;
    let deferral = unsafe { args.GetDeferral() }.map_err(|error| error.to_string())?;
    let handle = ComDownload {
        operation,
        proposed: proposed.clone(),
        start: Some((args.clone(), deferral)),
    };
    // Um `DownloadStarting` dentro de uma chamada que ja tem as operacoes
    // (nao acontece: nenhum handler as segura) recusa em vez de rebentar;
    // o `Drop` do `handle` completa o deferral a recusar.
    shared
        .ops
        .try_borrow_mut()
        .map_err(|_| "operacoes ocupadas".to_string())?
        .insert(id, webview, handle);
    let start = DownloadStart {
        id,
        webview,
        private,
        proposed,
        host: Url::parse(&uri)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string)),
        total: u64::try_from(total).ok().filter(|total| *total > 0),
        at: unix_now(),
    };
    debug_log(format_args!(
        "downloads: {} comecou ({}, privado={private})",
        id.0,
        if start.host.is_some() {
            "web"
        } else {
            "sem anfitriao"
        }
    ));
    if proxy
        .send_event(UserEvent::Download(DownloadEvent::Starting(start)))
        .is_err()
        && let Ok(mut ops) = shared.ops.try_borrow_mut()
    {
        ops.forget(id);
    }
    Ok(())
}

/// O progresso (filtrado a 250 ms) e o fim de uma operacao, como eventos do
/// gestor.
fn watch_download(
    operation: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation,
    id: DownloadId,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    use webview2_com::{
        BytesReceivedChangedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED, COREWEBVIEW2_DOWNLOAD_STATE,
            COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
        },
        StateChangedEventHandler, take_pwstr,
    };
    use windows_core::PWSTR;

    let progress_proxy = proxy.clone();
    let mut throttle = ProgressThrottle::default();
    let progress = BytesReceivedChangedEventHandler::create(Box::new(move |operation, _| {
        let Some(operation) = operation else {
            return Ok(());
        };
        if !throttle.admit(Instant::now()) {
            return Ok(());
        }
        let (mut received, mut total) = (0i64, 0i64);
        unsafe {
            operation.BytesReceived(&mut received)?;
            operation.TotalBytesToReceive(&mut total)?;
        }
        let _ = progress_proxy.send_event(UserEvent::Download(DownloadEvent::Progress {
            id,
            received: u64::try_from(received).unwrap_or(0),
            total: u64::try_from(total).ok().filter(|total| *total > 0),
        }));
        Ok(())
    }));
    let state = StateChangedEventHandler::create(Box::new(move |operation, _| {
        let Some(operation) = operation else {
            return Ok(());
        };
        let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
        unsafe { operation.State(&mut state)? };
        let end = if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
            let mut path = PWSTR::null();
            unsafe { operation.ResultFilePath(&mut path)? };
            DownloadEnd::Completed {
                path: PathBuf::from(take_pwstr(path)),
            }
        } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
            let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
            unsafe { operation.InterruptReason(&mut reason)? };
            // O motivo vai para o log: e o que o spike de CI le para saber
            // se destruir a WebView cancela o download dela.
            debug_log(format_args!(
                "downloads: {} interrompido (motivo {})",
                id.0, reason.0
            ));
            // Pausado (no painel de downloads do WebView2) nao acabou: a
            // operacao fica viva para o retomar e para o fim dele.
            if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
                return Ok(());
            }
            if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
                DownloadEnd::Cancelled
            } else {
                DownloadEnd::Interrupted
            }
        } else {
            // A correr outra vez (retomado): nada acabou.
            return Ok(());
        };
        let _ = proxy.send_event(UserEvent::Download(DownloadEvent::Ended { id, end }));
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { operation.add_BytesReceivedChanged(&progress, &mut token) }
        .map_err(|error| format!("add_BytesReceivedChanged falhou: {error}"))?;
    unsafe { operation.add_StateChanged(&state, &mut token) }
        .map_err(|error| format!("add_StateChanged falhou: {error}"))
}

// ===================== as lojas =====================

/// A pasta escolhida so conta se existir: senao fica a do WebView2.
pub(in crate::windows_app) fn checked_download_settings(
    mut settings: DownloadSettings,
    is_dir: impl Fn(&Path) -> bool,
) -> DownloadSettings {
    if settings.folder.is_some() && settings.usable_folder(is_dir).is_none() {
        debug_log(format_args!(
            "downloads: a pasta escolhida nao existe; fica a do WebView2"
        ));
        settings.folder = None;
    }
    settings
}

/// Grava o registo do gestor. A loja e `Automatic`: com o modo em `Private`
/// nao escreve nada (`SkippedPrivate`).
pub(in crate::windows_app) fn persist_download_log(
    store: &mut VersionedJsonStore<DownloadLog>,
    manager: &DownloadManager,
) -> Result<SaveOutcome, String> {
    store
        .save(&manager.log())
        .map_err(|error| error.to_string())
}

/// O estado da feature no `App`.
pub(in crate::windows_app) struct DownloadsState {
    pub(in crate::windows_app) manager: DownloadManager,
    pub(in crate::windows_app) shared: DownloadsShared,
    pub(in crate::windows_app) log_store: Option<VersionedJsonStore<DownloadLog>>,
}

impl DownloadsState {
    /// As definicoes e o registo, pelos grants do registo das lojas. Sem
    /// registo (nunca no produto), vale tudo por omissao e nada se grava.
    pub(in crate::windows_app) fn open(stores: Option<&StoreRegistry>) -> Self {
        let settings = stores
            .and_then(|stores| stores.grant(DOWNLOADS_SETTINGS_STORE).ok())
            .and_then(|grant| {
                VersionedJsonStore::<DownloadSettings>::open(
                    grant,
                    SETTINGS_VERSION,
                    SETTINGS_MAX_BYTES,
                )
                .ok()
            })
            .map(|mut store| store.load().into_value())
            .unwrap_or_default();
        let settings = checked_download_settings(settings, Path::is_dir);
        let mut log_store = stores
            .and_then(|stores| stores.grant(DOWNLOADS_LOG_STORE).ok())
            .and_then(|grant| {
                VersionedJsonStore::<DownloadLog>::open(grant, LOG_VERSION, LOG_MAX_BYTES).ok()
            });
        let log = log_store
            .as_mut()
            .map(|store| store.load().into_value())
            .unwrap_or_default();
        Self {
            shared: DownloadsShared::new(settings.folder.clone()),
            manager: DownloadManager::new(settings, log),
            log_store,
        }
    }
}

// ===================== os avisos =====================

/// O tipo de fachada de um disfarce (`fatura.pdf.exe` -> `PDF`).
fn decoy_label(name: &str) -> Option<String> {
    let clean = name.trim_end_matches([' ', '.']);
    let (stem, _) = clean.rsplit_once('.')?;
    let (_, decoy) = stem.trim_end().rsplit_once('.')?;
    let decoy = decoy.trim();
    (!decoy.is_empty()).then(|| decoy.to_uppercase())
}

/// O texto provisorio de cada aviso (o downloads-ui leva-os para o toast).
/// O nome passa por `display_label`: um bidi ou um invisivel aparece
/// marcado, nunca a trocar a ordem do texto.
pub(in crate::windows_app) fn download_notice_text(notice: &DownloadNotice) -> String {
    match notice {
        DownloadNotice::Blocked { name, reason, .. } => {
            let label = display_label(name);
            let what = match reason {
                BlockReason::Program => "é um programa",
                BlockReason::Script => "é um script",
                BlockReason::Shortcut => "é um atalho do Windows",
                BlockReason::DiskImage => "é uma imagem de disco",
                BlockReason::DatabaseApp => "é uma base do Access",
                BlockReason::Masquerade => {
                    return match decoy_label(name) {
                        Some(decoy) => {
                            format!("Download bloqueado — {label} finge ser um {decoy}.")
                        }
                        None => format!("Download bloqueado — {label} é um disfarce."),
                    };
                }
                BlockReason::BadName => {
                    return format!("Download bloqueado — o nome «{label}» não é seguro.");
                }
            };
            format!("Download bloqueado — {label} {what}.")
        }
        DownloadNotice::Deleted { name, reason, .. } => {
            let label = display_label(name);
            match reason {
                DeleteReason::Unreadable => {
                    format!("Download apagado — não deu para verificar {label}.")
                }
                DeleteReason::DangerousContent | DeleteReason::BlockedName(_) => {
                    format!("Download apagado — {label} era um programa disfarçado.")
                }
            }
        }
        DownloadNotice::NotDeleted { name, .. } => format!(
            "Atenção: {} é perigoso e não deu para apagar. Não o abra.",
            display_label(name)
        ),
    }
}

/// O ciclo do gestor: cada evento, os efeitos das operacoes aplicados a
/// `ops`, os outros a `step` -- que devolve o evento que volta ao gestor na
/// mesma volta (o fim de um ficheiro, a resposta a uma pergunta). E o que o
/// `App` corre, com `download_app_step`.
pub(in crate::windows_app) fn drive_downloads<H: DownloadHandle>(
    manager: &mut DownloadManager,
    ops: &RefCell<DownloadOps<H>>,
    event: DownloadEvent,
    mut step: impl FnMut(DownloadEffect) -> Option<DownloadEvent>,
) {
    let mut queue = VecDeque::from([event]);
    while let Some(event) = queue.pop_front() {
        for effect in manager.on_event(event) {
            let outcome = ops.borrow_mut().apply(effect);
            match outcome {
                OpsOutcome::Done => {}
                OpsOutcome::Failed(error) => debug_log(format_args!("downloads: {error}")),
                OpsOutcome::NotMine(effect) => {
                    if let Some(next) = step(effect) {
                        queue.push_back(next);
                    }
                }
            }
        }
    }
}

/// O que o `App` faz a um efeito que nao e das operacoes. A pergunta e o
/// fim do ficheiro correm ja e devolvem o evento seguinte; gravar e avisar
/// ficam em `later` (precisam do `App` inteiro).
pub(in crate::windows_app) fn download_app_step(
    effect: DownloadEffect,
    later: &mut Vec<DownloadEffect>,
) -> Option<DownloadEvent> {
    match effect {
        DownloadEffect::Ask { id, reason } => {
            // O cartao «Baixar programa?» chega com o downloads-ui; ate la a
            // pergunta responde «nao».
            debug_log(format_args!(
                "downloads: {} precisa de confirmacao ({reason:?}); recusado",
                id.0
            ));
            Some(DownloadEvent::Answered { id, allow: false })
        }
        DownloadEffect::Finalize {
            id,
            path,
            confirmed_program,
        } => {
            let outcome = finalize_download(&path, confirmed_program);
            debug_log(format_args!("downloads: {} acabou ({outcome:?})", id.0));
            Some(DownloadEvent::Finalized { id, outcome })
        }
        DownloadEffect::Persist | DownloadEffect::Notice(_) => {
            later.push(effect);
            None
        }
        // A linha da lista: o downloads-ui repinta-a. Os das operacoes nunca
        // chegam aqui (`DownloadOps::apply`).
        DownloadEffect::Changed(_)
        | DownloadEffect::Refuse(_)
        | DownloadEffect::Proceed { .. }
        | DownloadEffect::CancelRunning(_)
        | DownloadEffect::ForgetOp(_) => None,
    }
}

impl App {
    /// O braco `UserEvent::Download`: o gestor decide, e cada efeito vai
    /// para as operacoes do WebView2 ou para o `App`.
    pub(in crate::windows_app) fn download_event(&mut self, event: DownloadEvent) {
        let mut later = Vec::new();
        drive_downloads(
            &mut self.downloads.manager,
            &self.downloads.shared.ops,
            event,
            |effect| download_app_step(effect, &mut later),
        );
        for effect in later {
            match effect {
                DownloadEffect::Persist => self.persist_downloads(),
                DownloadEffect::Notice(notice) => {
                    self.show_background_splash(download_notice_text(&notice), 6);
                }
                _ => {}
            }
        }
    }

    fn persist_downloads(&mut self) {
        let Some(store) = self.downloads.log_store.as_mut() else {
            return;
        };
        match persist_download_log(store, &self.downloads.manager) {
            Ok(SaveOutcome::Written) => {}
            Ok(SaveOutcome::SkippedPrivate) => {
                debug_log(format_args!("downloads: modo privado, registo nao gravado"));
            }
            Err(error) => debug_log(format_args!("downloads: registo nao gravado ({error})")),
        }
    }
}
