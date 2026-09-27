use super::*;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use neural_core::downloads::{
    DownloadEffect, DownloadEnd, DownloadEvent, DownloadId, DownloadLog, DownloadManager,
    DownloadSettings, DownloadStart, FinalizeOutcome, LOG_MAX_BYTES, LOG_VERSION, ProgressThrottle,
    SETTINGS_MAX_BYTES, SETTINGS_VERSION, WebViewKey, apply_finalize_download, finalize_download,
    finalize_download_with_commit, unique_path,
};
use neural_core::json_store::{SaveOutcome, StoreGrant, StoreSpec, VersionedJsonStore};

use crate::lazy_worker::{JobContext, LazyWorker};
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
//   interrompido), que larga a operacao e, se acabou, enfileira o fim na
//   thread `neural-download-finalize` (`DownloadFinalizer`): sniff, inspecao
//   do ZIP, «Permitir baixar programas» lida no commit, e marca da Web ou
//   apagar. O veredito volta como `UserEvent::Download(DownloadEvent::
//   Finalized)`; ate la a linha diz «Verificando o arquivo…», sem accoes, e
//   nao se cancela. Sem a thread, o fim corre sincrono (`download_app_step`).
//   O fim grava um registo pendente; a saida ordenada espera os vereditos
//   (`DownloadsState::finish_before_exit`) e o arranque retoma os que ficaram
//   (`DownloadManager::resume_pending`, no `DownloadsState::open`).
// - A WebView destruida: o handler do `DownloadStarting` guarda um
//   `WebViewLife`; quando o WebView2 o larga, o gestor recebe `WebViewGone`
//   e cancela e larga as operacoes dessa WebView.
// - A pasta (`downloads-settings.json`, ou a Transferencias do utilizador
//   quando nao ha escolha) vai para o perfil de cada WebView gerida com o
//   `SetDefaultDownloadFolderPath`: o WebView2 guarda-a no perfil de uma
//   sessao para a outra, por isso a do sistema e reposta quando a escolha
//   sai (`profile_download_folder`).
// - O registo (`downloads.json`, `StoreKind::GuardedAutomatic`) nunca
//   guarda um download do Split privado, de um servico InPrivate, nem um que
//   comecou ou acabou no Modo privado. O gestor exclui isso antes da loja;
//   por isso um veredito tardio de um download normal pode substituir o
//   `pending` mesmo se o modo global ficou privado. O Ctrl+Shift+Delete
//   apaga o historico; se ha verificacao em curso, conserva apenas o
//   `recovery_only` oculto necessario a uma retomada apos queda.
// - O que chega a quem usa -- os avisos, o «Baixar programa?», a lista, a
//   seta da barra -- e do `downloads_ui.rs` (downloads-ui): o braco
//   `download_event` entrega-lhe o que cada volta do gestor deixou.

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
    /// A pasta que vai para o perfil de cada WebView
    /// (`profile_download_folder`): a escolhida, ja conferida no disco, ou a
    /// Transferencias do utilizador.
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
        // Sempre, com a escolha ou sem ela (`profile_download_folder`): o
        // perfil guarda a pasta de uma sessao para a outra. Sem
        // ICoreWebView2_13 (sem perfil para guardar), a escolhida chega pelo
        // `SetResultFilePath` de cada download (`target_path`).
        let set = core
            .cast::<ICoreWebView2_13>()
            .and_then(|core13| unsafe { core13.Profile() })
            .and_then(|profile| unsafe {
                profile.SetDefaultDownloadFolderPath(&HSTRING::from(folder.as_path()))
            });
        if let Err(error) = set {
            debug_log(format_args!(
                "downloads: {} sem a pasta dos downloads no perfil ({error})",
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

/// A pasta que vai para o perfil de cada WebView: a escolhida (ja
/// conferida), senao a do sistema (`user_downloads_folder`). O WebView2
/// guarda o `SetDefaultDownloadFolderPath` no perfil de uma sessao para a
/// outra (e volta a criar a pasta, se faltar, no download seguinte): so
/// pondo-a quando ha escolha, a antiga ficava depois de a escolha sair ou
/// de a pasta deixar de existir.
pub(in crate::windows_app) fn profile_download_folder(
    chosen: Option<PathBuf>,
    system: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    chosen.or_else(system)
}

/// A pasta Transferencias do utilizador (`FOLDERID_Downloads`), `None` se o
/// Windows nao a da (nao existe, ou o perfil nao a tem).
pub(in crate::windows_app) fn user_downloads_folder() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Downloads, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };

    let mut raw: *mut u16 = std::ptr::null_mut();
    let hr = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_Downloads,
            KF_FLAG_DEFAULT as u32,
            std::ptr::null_mut(),
            &mut raw,
        )
    };
    if raw.is_null() {
        return None;
    }
    let path = (hr >= 0).then(|| {
        let mut len = 0usize;
        // SAFETY: o SHGetKnownFolderPath devolve um texto terminado em 0.
        while unsafe { *raw.add(len) } != 0 {
            len += 1;
        }
        let wide = unsafe { std::slice::from_raw_parts(raw, len) };
        PathBuf::from(std::ffi::OsString::from_wide(wide))
    });
    // O texto e do chamador mesmo quando a chamada falha.
    unsafe { CoTaskMemFree(raw as *const std::ffi::c_void) };
    path.filter(|path| path.is_absolute())
}

/// Grava o registo do gestor. No produto a loja e `GuardedAutomatic`:
/// downloads privados ja foram excluidos pelo gestor, e um veredito tardio
/// de um download normal pode concluir mesmo com o modo global `Private`.
pub(in crate::windows_app) fn persist_download_log(
    store: &mut VersionedJsonStore<DownloadLog>,
    manager: &DownloadManager,
) -> Result<SaveOutcome, String> {
    store
        .save(&manager.log())
        .map_err(|error| error.to_string())
}

/// Ctrl+Shift+Delete: tira do disco o `downloads.json`, a copia `.bak` que
/// a loja faz de um ficheiro que recusou (estragado ou de uma versao futura)
/// e o temporario de uma gravacao interrompida -- todos guardam nomes e
/// anfitrioes. Apaga direto, nunca pela loja: uma gravacao do registo vazio
/// nao escreve nada no Modo privado nem com a loja so de leitura. Depois a
/// loja rele o disco (sem ficheiro, volta a gravar).
pub(in crate::windows_app) fn erase_download_log(
    store: &mut VersionedJsonStore<DownloadLog>,
) -> std::io::Result<()> {
    let path = store.path().to_path_buf();
    let mut first_error = None;
    let mut remove = |target: &Path| {
        if let Err(error) = std::fs::remove_file(target)
            && error.kind() != std::io::ErrorKind::NotFound
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    };
    remove(&path);
    let name = neural_core::downloads::file_name_of(&path).to_string();
    remove(&path.with_file_name(format!("{name}.bak")));
    // O temporario da gravacao atomica: `.<nome>.<pid>-<n>.tmp`.
    if let Some(dir) = path.parent()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        let prefix = format!(".{name}.");
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|file| file.starts_with(&prefix) && file.ends_with(".tmp"))
            {
                remove(&entry.path());
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => {
            // Sem ficheiro, a loja deixa de estar so de leitura; nao escreve.
            store.load();
            Ok(())
        }
    }
}

/// O modo do `PrivacyGuard`, como o gestor o precisa: `true` no Modo
/// privado.
pub(in crate::windows_app) fn downloads_private_mode(mode: PrivacyMode) -> bool {
    mode == PrivacyMode::Private
}

/// Quanto a saida ordenada (`App::exiting`) espera pelos vereditos que a
/// thread do fim ainda deve. Um que nao chega a tempo fica no
/// `downloads.json` como pendente e volta no arranque seguinte.
pub(in crate::windows_app) const FINALIZE_EXIT_WAIT: Duration = Duration::from_secs(5);

/// Um fim na fila. A fila conserva todos os downloads que acabam juntos: o
/// `LazyWorker` tem uma vaga latest-wins, por isso recebe o receptor uma
/// unica vez e a thread consome esta fila FIFO ate o `DownloadsState` ser
/// largado.
struct FinalizeJob {
    id: DownloadId,
    path: PathBuf,
    confirmed_program: bool,
}

/// O que a thread do fim e a da interface partilham.
struct FinalizerShared {
    /// «Permitir baixar programas», o espelho da definicao do gestor: a
    /// thread le-a no commit (`finalize_download_with_commit`), nao quando
    /// o download acabou.
    allow_programs: AtomicBool,
    state: Mutex<FinalizerState>,
    /// Um veredito chegou, ou a thread perdeu-se.
    settled: Condvar,
}

#[derive(Default)]
struct FinalizerState {
    /// A thread nao nasceu, ou um fim rebentou nela: tudo o que vem a seguir
    /// corre sincrono, na thread da interface.
    lost: bool,
    /// Os vereditos ja aplicados ao disco que a interface ainda nao recebeu
    /// (a saida le-os daqui, sem o event loop).
    done: BTreeMap<DownloadId, FinalizeOutcome>,
    /// Os que a thread devolveu sem veredito (`FinalizeLost`).
    returned: BTreeSet<DownloadId>,
}

impl FinalizerShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, FinalizerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
type FinalizeCommitHook = Arc<dyn Fn(DownloadId) + Send + Sync>;
#[cfg(test)]
type FinalizeCommitProbe = Arc<Mutex<Option<FinalizeCommitHook>>>;

/// A thread `neural-download-finalize` (lazy: nasce no primeiro download que
/// acaba) e o que a interface sabe dela. Nenhuma verificacao se cancela: a
/// thread aplica sempre o veredito e devolve-o (`DownloadEvent::Finalized`).
/// Sem a thread -- nao nasceu, ou um fim rebentou nela --, o fim corre
/// sincrono na thread da interface, como antes da thread existir.
pub(in crate::windows_app) struct DownloadFinalizer {
    worker: LazyWorker<Receiver<FinalizeJob>>,
    sender: Sender<FinalizeJob>,
    receiver: Option<Receiver<FinalizeJob>>,
    shared: Arc<FinalizerShared>,
    /// Os fins a espera do veredito, com o que o fim sincrono precisa.
    pending: BTreeMap<DownloadId, (PathBuf, bool)>,
    #[cfg(test)]
    before_commit: FinalizeCommitProbe,
}

impl DownloadFinalizer {
    pub(in crate::windows_app) fn new(emit: impl Fn(DownloadEvent) + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::new(FinalizerShared {
            allow_programs: AtomicBool::new(false),
            state: Mutex::new(FinalizerState::default()),
            settled: Condvar::new(),
        });
        #[cfg(test)]
        let before_commit: FinalizeCommitProbe = Arc::new(Mutex::new(None));
        #[cfg(test)]
        let worker_hook = Arc::clone(&before_commit);
        let worker_shared = Arc::clone(&shared);
        let worker = LazyWorker::new(
            "neural-download-finalize",
            move |queue: Receiver<FinalizeJob>, _: &JobContext| {
                while let Ok(job) = queue.recv() {
                    // No release um panic aborta o processo (e o arranque
                    // seguinte retoma o pendente); com unwind, a thread nao
                    // morre calada: devolve este fim e os da fila.
                    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        finalize_download_with_commit(
                            &job.path,
                            job.confirmed_program,
                            || {
                                // Os gates param aqui: a inspecao ja correu,
                                // a definicao ainda nao foi lida.
                                #[cfg(test)]
                                if let Some(hook) = worker_hook
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .clone()
                                {
                                    hook(job.id);
                                }
                                worker_shared.allow_programs.load(Ordering::Acquire)
                            },
                            |action| apply_finalize_download(&job.path, action),
                        )
                    }));
                    match run {
                        Ok(outcome) => {
                            worker_shared.lock().done.insert(job.id, outcome);
                            worker_shared.settled.notify_all();
                            emit(DownloadEvent::Finalized {
                                id: job.id,
                                outcome,
                            });
                        }
                        Err(_) => {
                            // Sob o trinco que o `enqueue` tambem toma: um
                            // fim nunca entra na fila depois deste esvaziar.
                            let mut lost = vec![job.id];
                            let mut state = worker_shared.lock();
                            state.lost = true;
                            while let Ok(next) = queue.try_recv() {
                                lost.push(next.id);
                            }
                            state.returned.extend(lost.iter().copied());
                            drop(state);
                            worker_shared.settled.notify_all();
                            for id in lost {
                                emit(DownloadEvent::FinalizeLost { id });
                            }
                            return;
                        }
                    }
                }
            },
        );
        Self {
            worker,
            sender,
            receiver: Some(receiver),
            shared,
            pending: BTreeMap::new(),
            #[cfg(test)]
            before_commit,
        }
    }

    #[cfg(test)]
    pub(in crate::windows_app) fn set_before_commit(
        &mut self,
        hook: impl Fn(DownloadId) + Send + Sync + 'static,
    ) {
        *self
            .before_commit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(hook));
    }

    #[cfg(test)]
    pub(in crate::windows_app) fn threads_spawned(&self) -> usize {
        self.worker.threads_spawned()
    }

    /// O espelho de «Permitir baixar programas» que a thread le no commit.
    fn set_allow_programs(&self, on: bool) {
        self.shared.allow_programs.store(on, Ordering::Release);
    }

    /// Poe o fim na fila da thread. `false`: sem thread (nao nasceu, ou
    /// perdeu-se) -- quem chama corre-o sincrono.
    fn enqueue(&mut self, id: DownloadId, path: PathBuf, confirmed_program: bool) -> bool {
        if let Some(receiver) = self.receiver.take()
            && self.worker.submit(receiver).is_err()
        {
            self.shared.lock().lost = true;
            return false;
        }
        let mut state = self.shared.lock();
        if state.lost {
            return false;
        }
        let job = FinalizeJob {
            id,
            path: path.clone(),
            confirmed_program,
        };
        if self.sender.send(job).is_err() {
            state.lost = true;
            return false;
        }
        drop(state);
        self.pending.insert(id, (path, confirmed_program));
        true
    }

    /// O veredito (ou o `FinalizeLost`) de `id` chegou a interface.
    fn finished(&mut self, id: DownloadId) {
        self.pending.remove(&id);
        let mut state = self.shared.lock();
        state.done.remove(&id);
        state.returned.remove(&id);
    }

    /// A saida: espera (no maximo `wait`) que a thread de o veredito de cada
    /// fim pendente e devolve-os como eventos para o gestor -- os que a
    /// thread aplicou e os que ela devolveu, estes corridos agora, sincronos.
    /// Um que o prazo apanha a meio nao volta: fica pendente no
    /// `downloads.json` e o arranque seguinte retoma-o.
    fn settle(&mut self, wait: Duration) -> Vec<DownloadEvent> {
        let deadline = Instant::now() + wait;
        let mut state = self.shared.lock();
        loop {
            let open = self
                .pending
                .keys()
                .any(|id| !state.done.contains_key(id) && !state.returned.contains(id));
            let now = Instant::now();
            if !open || now >= deadline {
                break;
            }
            state = self
                .shared
                .settled
                .wait_timeout(state, deadline - now)
                .map(|(state, _)| state)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
        let mut events = Vec::new();
        let mut returned = Vec::new();
        for (id, (path, confirmed_program)) in &self.pending {
            if let Some(outcome) = state.done.get(id) {
                events.push(DownloadEvent::Finalized {
                    id: *id,
                    outcome: *outcome,
                });
            } else if state.returned.contains(id) {
                returned.push((*id, path.clone(), *confirmed_program));
            }
        }
        drop(state);
        let allow_programs = self.shared.allow_programs.load(Ordering::Acquire);
        for (id, path, confirmed_program) in returned {
            events.push(DownloadEvent::Finalized {
                id,
                outcome: finalize_download(&path, confirmed_program, allow_programs),
            });
        }
        events
    }
}

/// O estado da feature no `App`.
pub(in crate::windows_app) struct DownloadsState {
    pub(in crate::windows_app) manager: DownloadManager,
    pub(in crate::windows_app) finalizer: DownloadFinalizer,
    pub(in crate::windows_app) shared: DownloadsShared,
    pub(in crate::windows_app) log_store: Option<VersionedJsonStore<DownloadLog>>,
    /// `downloads-settings.json` (`StoreKind::Setting`): a seccao Downloads
    /// grava aqui «Permitir baixar programas» (`App::set_allow_programs`).
    pub(in crate::windows_app) settings_store: Option<VersionedJsonStore<DownloadSettings>>,
}

impl DownloadsState {
    /// As definicoes e o registo, pelos grants que `grants` da (no produto,
    /// `PrivacyGuard::store`). Sem grant (nunca no produto), vale tudo por
    /// omissao e nada se grava.
    pub(in crate::windows_app) fn open(
        mut grants: impl FnMut(StoreSpec) -> Option<StoreGrant>,
        emit: impl Fn(DownloadEvent) + Send + 'static,
    ) -> Self {
        let mut settings_store = grants(DOWNLOADS_SETTINGS_STORE).and_then(|grant| {
            VersionedJsonStore::<DownloadSettings>::open(
                grant,
                SETTINGS_VERSION,
                SETTINGS_MAX_BYTES,
            )
            .ok()
        });
        let settings = settings_store
            .as_mut()
            .map(|store| store.load().into_value())
            .unwrap_or_default();
        let settings = checked_download_settings(settings, Path::is_dir);
        let mut log_store = grants(DOWNLOADS_LOG_STORE).and_then(|grant| {
            VersionedJsonStore::<DownloadLog>::open(grant, LOG_VERSION, LOG_MAX_BYTES).ok()
        });
        let log = log_store
            .as_mut()
            .map(|store| store.load().into_value())
            .unwrap_or_default();
        let allow_programs = settings.allow_programs;
        let mut state = Self {
            shared: DownloadsShared::new(profile_download_folder(
                settings.folder.clone(),
                user_downloads_folder,
            )),
            manager: DownloadManager::new(settings, log),
            finalizer: DownloadFinalizer::new(emit),
            log_store,
            settings_store,
        };
        state.finalizer.set_allow_programs(allow_programs);
        state.resume_pending();
        state
    }

    /// Os fins que a sessao anterior deixou sem veredito (fechou ou caiu a
    /// meio da verificacao) voltam a correr, cada um numa linha nova a
    /// verificar. A marca `resumed` vai para o disco antes de o fim correr:
    /// um fim que derruba a NeuralIA nao a derruba em cada arranque. Sem
    /// pendentes (o normal), nada corre nem se escreve.
    fn resume_pending(&mut self) {
        let shared = &self.shared;
        let mut effects = self.manager.resume_pending(|| shared.next_download_id());
        if effects.is_empty() {
            return;
        }
        if let Some(store) = self.log_store.as_mut()
            && let Err(error) = persist_download_log(store, &self.manager)
        {
            debug_log(format_args!("downloads: pendentes nao gravados ({error})"));
        }
        effects.retain(|effect| *effect != DownloadEffect::Persist);
        let private_mode = self.manager.private_mode();
        let run = run_download_effects(
            &mut self.manager,
            &mut self.finalizer,
            &self.shared.ops,
            self.log_store.as_mut(),
            private_mode,
            effects,
        );
        debug_log(format_args!(
            "downloads: {} fim(ns) retomado(s) no arranque",
            run.later
                .iter()
                .filter(|effect| matches!(effect, DownloadEffect::Changed(_)))
                .count()
        ));
    }

    /// O braco `UserEvent::Download` do `App` inteiro menos os avisos: o
    /// gestor com o modo do registo das lojas, as operacoes do WebView2 e o
    /// disco (`run_download_event`).
    pub(in crate::windows_app) fn run(
        &mut self,
        private_mode: bool,
        event: DownloadEvent,
    ) -> DownloadRun {
        run_download_event(
            &mut self.manager,
            &mut self.finalizer,
            &self.shared.ops,
            self.log_store.as_mut(),
            private_mode,
            event,
        )
    }

    /// A saida ordenada (`App::exiting`): cada fim a meio acaba antes de a
    /// NeuralIA sair -- a thread tem ate `wait` para dar os vereditos que
    /// deve, os que ela devolveu correm ja, sincronos --, e cada veredito
    /// passa pelo gestor e pelo `downloads.json` como qualquer outro. O que
    /// o prazo apanha a meio fica pendente no registo e volta no arranque.
    pub(in crate::windows_app) fn finish_before_exit(
        &mut self,
        private_mode: bool,
        wait: Duration,
    ) -> DownloadRun {
        let mut total = DownloadRun::default();
        for event in self.finalizer.settle(wait) {
            let run = self.run(private_mode, event);
            total.later.extend(run.later);
        }
        total
    }

    /// «Permitir baixar programas» mudou: o gestor e o espelho que a thread
    /// do fim le no commit.
    pub(in crate::windows_app) fn allow_programs_changed(&mut self, on: bool) {
        let mut next = self.manager.settings().clone();
        next.allow_programs = on;
        self.manager.on_event(DownloadEvent::SettingsChanged(next));
        self.finalizer.set_allow_programs(on);
    }
}

/// O ciclo do gestor: os efeitos que ele deu (a um evento, ou aos fins
/// retomados no arranque), os das operacoes aplicados a `ops`, os outros a
/// `step` -- que devolve o evento que volta ao gestor na mesma volta (o
/// veredito de um fim sincrono, a resposta a uma pergunta). E o que o `App`
/// corre, com `download_app_step`.
pub(in crate::windows_app) fn drive_downloads<H: DownloadHandle>(
    manager: &mut DownloadManager,
    ops: &RefCell<DownloadOps<H>>,
    first: Vec<DownloadEffect>,
    mut step: impl FnMut(DownloadEffect) -> Option<DownloadEvent>,
) {
    let mut queue = VecDeque::new();
    let mut effects = first;
    loop {
        for effect in effects {
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
        let Some(event) = queue.pop_front() else {
            break;
        };
        effects = manager.on_event(event);
    }
}

/// O que o `App` faz a um efeito que nao e das operacoes. O fim do
/// ficheiro entra na fila da thread sem bloquear a janela -- sem a thread,
/// corre ja, sincrono, e o veredito volta na mesma volta; gravar, avisar, a
/// pergunta «Baixar programa?» (o cartao do downloads-ui, cuja resposta
/// chega mais tarde como `DownloadEvent::Answered`) e a linha que mudou
/// ficam em `later` (precisam do `App` inteiro).
pub(in crate::windows_app) fn download_app_step(
    effect: DownloadEffect,
    finalizer: &mut DownloadFinalizer,
    later: &mut Vec<DownloadEffect>,
) -> Option<DownloadEvent> {
    match effect {
        DownloadEffect::Ask { id, reason } => {
            debug_log(format_args!(
                "downloads: {} precisa de confirmacao ({reason:?})",
                id.0
            ));
            later.push(effect);
            None
        }
        DownloadEffect::Finalize {
            id,
            path,
            confirmed_program,
            allow_programs,
        } => {
            if finalizer.enqueue(id, path.clone(), confirmed_program) {
                return None;
            }
            debug_log(format_args!("downloads: {} sem worker: fim sincrono", id.0));
            Some(DownloadEvent::Finalized {
                id,
                outcome: finalize_download(&path, confirmed_program, allow_programs),
            })
        }
        DownloadEffect::Persist
        | DownloadEffect::EraseLog
        | DownloadEffect::Notice(_)
        | DownloadEffect::Changed(_) => {
            later.push(effect);
            None
        }
        // Os das operacoes nunca chegam aqui (`DownloadOps::apply`).
        DownloadEffect::Refuse(_)
        | DownloadEffect::Proceed { .. }
        | DownloadEffect::CancelRunning(_)
        | DownloadEffect::ForgetOp(_) => None,
    }
}

/// O que uma volta do gestor deixou para o `App`.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct DownloadRun {
    /// Os efeitos que ficaram para o fim, pela ordem: gravar e apagar (ja
    /// aplicados ao disco), os avisos, a pergunta e as linhas que mudaram
    /// (o downloads-ui trata deles).
    pub(in crate::windows_app) later: Vec<DownloadEffect>,
    /// O Ctrl+Shift+Delete nao conseguiu tirar o `downloads.json` do disco.
    pub(in crate::windows_app) erase_error: Option<String>,
}

/// Uma volta inteira do gestor, como o `App` a corre: o modo do registo das
/// lojas posto no gestor, o ciclo (`drive_downloads` com
/// `download_app_step`), e depois o registo gravado (`Persist`) ou tirado
/// do disco (`EraseLog`). Sem loja (nunca no produto), nada toca no disco.
pub(in crate::windows_app) fn run_download_event<H: DownloadHandle>(
    manager: &mut DownloadManager,
    finalizer: &mut DownloadFinalizer,
    ops: &RefCell<DownloadOps<H>>,
    log_store: Option<&mut VersionedJsonStore<DownloadLog>>,
    private_mode: bool,
    event: DownloadEvent,
) -> DownloadRun {
    manager.set_private_mode(private_mode);
    match &event {
        DownloadEvent::Finalized { id, outcome } => {
            debug_log(format_args!("downloads: {} acabou ({outcome:?})", id.0));
            finalizer.finished(*id);
        }
        DownloadEvent::FinalizeLost { id } => {
            debug_log(format_args!("downloads: {} perdeu o worker", id.0));
            finalizer.finished(*id);
        }
        _ => {}
    }
    let first = manager.on_event(event);
    run_download_effects(manager, finalizer, ops, log_store, private_mode, first)
}

/// A volta de `run_download_event` a partir dos efeitos que o gestor deu:
/// o ciclo, o espelho de «Permitir baixar programas» para a thread do fim,
/// e o registo gravado (`Persist`) ou tirado do disco (`EraseLog`).
fn run_download_effects<H: DownloadHandle>(
    manager: &mut DownloadManager,
    finalizer: &mut DownloadFinalizer,
    ops: &RefCell<DownloadOps<H>>,
    mut log_store: Option<&mut VersionedJsonStore<DownloadLog>>,
    private_mode: bool,
    first: Vec<DownloadEffect>,
) -> DownloadRun {
    manager.set_private_mode(private_mode);
    let mut run = DownloadRun::default();
    drive_downloads(manager, ops, first, |effect| {
        download_app_step(effect, finalizer, &mut run.later)
    });
    // A thread le a definicao no commit: a que o gestor tem depois desta
    // volta (um `SettingsChanged` pode ter chegado nela).
    finalizer.set_allow_programs(manager.settings().allow_programs);
    for effect in &run.later {
        let Some(store) = log_store.as_deref_mut() else {
            break;
        };
        match effect {
            DownloadEffect::Persist => match persist_download_log(store, manager) {
                Ok(SaveOutcome::Written) => {}
                Ok(SaveOutcome::SkippedPrivate) => {
                    debug_log(format_args!("downloads: modo privado, registo nao gravado"));
                }
                Err(error) => {
                    debug_log(format_args!("downloads: registo nao gravado ({error})"));
                }
            },
            DownloadEffect::EraseLog => {
                if let Err(error) = erase_download_log(store) {
                    debug_log(format_args!("downloads: registo nao apagado ({error})"));
                    run.erase_error = Some(error.to_string());
                }
            }
            _ => {}
        }
    }
    run
}

impl App {
    /// O braco `UserEvent::Download`: o gestor decide, e cada efeito vai
    /// para as operacoes do WebView2, para o disco ou para o ecra
    /// (`downloads_ui_after`).
    pub(in crate::windows_app) fn download_event(&mut self, event: DownloadEvent) {
        let private_mode = downloads_private_mode(self.privacy.mode());
        let run = self.downloads.run(private_mode, event);
        if let Some(error) = run.erase_error {
            self.show_splash(
                format!("Não foi possível apagar a lista de downloads: {error}"),
                4,
            );
        }
        // Os avisos, a pergunta, a lista e a seta (`downloads_ui.rs`).
        self.downloads_ui_after(run.later);
    }

    /// A saida ordenada (`exiting`, depois de qualquer `event_loop.exit`):
    /// os fins a meio acabam e ficam no `downloads.json` antes de a
    /// NeuralIA sair (`DownloadsState::finish_before_exit`). Os avisos ja
    /// nao se mostram.
    pub(in crate::windows_app) fn finish_downloads_before_exit(&mut self) {
        let private_mode = downloads_private_mode(self.privacy.mode());
        let run = self
            .downloads
            .finish_before_exit(private_mode, FINALIZE_EXIT_WAIT);
        debug_log(format_args!(
            "downloads: saida com {} veredito(s) aplicados",
            run.later
                .iter()
                .filter(|effect| matches!(effect, DownloadEffect::Changed(_)))
                .count()
        ));
    }
}

#[cfg(test)]
mod recovery_regression_tests {
    use super::*;
    use neural_core::json_store::{StoreMode, StoreRegistry};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(1);
    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neuralia-download-regression-{name}-{}-{}",
                std::process::id(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn open_state(
        dir: &Path,
    ) -> (
        DownloadsState,
        std::sync::mpsc::Receiver<DownloadEvent>,
        StoreRegistry,
    ) {
        let stores = StoreRegistry::mint_for_test(dir);
        let (sender, events) = std::sync::mpsc::channel();
        let state = DownloadsState::open(
            |spec| stores.grant(spec).ok(),
            move |event| {
                let _ = sender.send(event);
            },
        );
        (state, events, stores)
    }

    fn start_and_finish(state: &mut DownloadsState, private_mode: bool, id: u64, path: &Path) {
        state.run(
            private_mode,
            DownloadEvent::Starting(DownloadStart {
                id: DownloadId(id),
                webview: WebViewKey(1),
                private: false,
                proposed: path.to_path_buf(),
                host: Some("example.com".to_string()),
                total: Some(PDF.len() as u64),
                at: id,
            }),
        );
        std::fs::write(path, PDF).expect("pdf");
        state.run(
            private_mode,
            DownloadEvent::Ended {
                id: DownloadId(id),
                end: DownloadEnd::Completed {
                    path: path.to_path_buf(),
                },
            },
        );
    }

    fn log_json(dir: &Path) -> serde_json::Value {
        let text =
            std::fs::read_to_string(dir.join(DOWNLOADS_LOG_STORE.name)).expect("downloads.json");
        serde_json::from_str(&text).expect("json")
    }

    /// Gate critico: apagar o historico enquanto o arquivo ja esta em
    /// verificacao deixa apenas o journal oculto necessario a recovery.
    /// O verdict remove esse journal e nao ressuscita uma linha no painel.
    #[test]
    fn clear_history_during_verification_preserves_recovery_without_restoring_history() {
        let dir = Scratch::new("clear");
        let (mut state, events, _stores) = open_state(&dir.0);
        let path = dir.0.join("relatorio.pdf");

        start_and_finish(&mut state, false, 1, &path);
        let before = log_json(&dir.0);
        assert_eq!(before["data"]["entries"][0]["outcome"]["kind"], "pending");

        // Artefactos que poderiam conservar o historico apagado. O ClearLog
        // com recovery tem de os remover ANTES de gravar o journal minimo.
        let backup = dir.0.join("downloads.json.bak");
        let temp = dir.0.join(".downloads.json.regression.tmp");
        std::fs::write(&backup, b"OLD-HISTORY").expect("backup fixture");
        std::fs::write(&temp, b"OLD-HISTORY").expect("temp fixture");

        state.run(false, DownloadEvent::ClearLog);
        assert!(!backup.exists(), "backup antigo sobreviveu ao ClearLog");
        assert!(!temp.exists(), "temporario antigo sobreviveu ao ClearLog");
        let cleared = log_json(&dir.0);
        let entries = cleared["data"]["entries"].as_array().expect("entries");
        assert_eq!(entries.len(), 1, "{cleared}");
        assert_eq!(entries[0]["outcome"]["kind"], "pending");
        assert_eq!(entries[0]["recovery_only"], true);
        assert!(
            DownloadRows::default()
                .list(&state.manager, &BTreeMap::new())
                .is_empty(),
            "recovery oculto reapareceu no painel"
        );

        let verdict = events
            .recv_timeout(Duration::from_secs(15))
            .expect("verdict");
        state.run(false, verdict);
        let after = log_json(&dir.0);
        assert!(
            after["data"]["entries"]
                .as_array()
                .expect("entries")
                .is_empty(),
            "{after}"
        );
        assert!(
            DownloadRows::default()
                .list(&state.manager, &BTreeMap::new())
                .is_empty(),
            "verdict ressuscitou o historico"
        );
    }

    /// Gate critico: um download que acabou no modo normal ja decidiu que
    /// pertence ao journal. Se o modo global ficar privado antes de o evento
    /// do verdict chegar, esse verdict ainda substitui pending no disco.
    /// Assim um restart nao reexecuta uma decisao de seguranca ja aplicada.
    #[test]
    fn normal_download_verdict_persists_even_if_global_mode_turns_private() {
        let dir = Scratch::new("private-verdict");
        let (mut state, events, stores) = open_state(&dir.0);
        let path = dir.0.join("relatorio.pdf");

        start_and_finish(&mut state, false, 1, &path);
        let pending = log_json(&dir.0);
        assert_eq!(pending["data"]["entries"][0]["outcome"]["kind"], "pending");

        let verdict = events
            .recv_timeout(Duration::from_secs(15))
            .expect("verdict");
        stores.set_mode(StoreMode::Private);
        state.run(true, verdict);

        let committed = log_json(&dir.0);
        assert_eq!(
            committed["data"]["entries"][0]["outcome"]["kind"], "completed",
            "{committed}"
        );
        assert_ne!(
            committed["data"]["entries"][0]["outcome"]["kind"],
            "pending"
        );
    }
    /// Gate critico: recovery_only tem uma unica tentativa. Depois de um
    /// segundo crash ele nao aparece como historico e nao conserva o caminho
    /// escondido para sempre.
    #[test]
    fn exhausted_hidden_recovery_is_purged_after_its_single_retry() {
        let log = DownloadLog {
            entries: vec![neural_core::downloads::DownloadRecord {
                name: "segredo.pdf".to_string(),
                path: Some(PathBuf::from(r"C:\Baixados\segredo.pdf")),
                host: Some("example.com".to_string()),
                bytes: Some(42),
                recovery_only: true,
                outcome: neural_core::downloads::RecordOutcome::Pending {
                    confirmed_program: false,
                    resumed: true,
                },
                at: 1,
            }],
        };
        let mut manager = DownloadManager::new(DownloadSettings::default(), log);
        assert_eq!(
            manager.resume_pending(|| DownloadId(99)),
            vec![DownloadEffect::Persist]
        );
        assert!(manager.log().entries.is_empty());
        assert_eq!(manager.entries().count(), 0);
    }

}
