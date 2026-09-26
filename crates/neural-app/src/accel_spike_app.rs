//! A cola do spike do `AcceleratorKeyPressed` com o `App` e o COM do
//! WebView2 (item infra-accel-spike do plano 2.3). So compila com
//! `--features accel-spike`, a feature do job `accel-spike`
//! (`.github/workflows/accel-spike.yml`); o exe publicado e compilado sem ela.
//!
//! Vive fora de `src/windows_app/` de proposito: e um modulo filho de
//! `windows_app` (declarado la com `#[path]`, para chegar aos itens
//! `pub(in crate::windows_app)`), mas NAO embarca, e por isso nao entra em
//! `ALL_MODULES` nem no `shipped_source()` dos gates do produto.
//!
//! Tres pecas, nada mais:
//! - `install_accel_spike`: um `add_AcceleratorKeyPressed` por WebView, com
//!   `Handled = TRUE` so para os atalhos de `SPIKE_CHORDS`
//!   (`spike_verdict`), a registar no ficheiro do condutor cada tecla da
//!   tabela que o lado nativo viu;
//! - `accel_spike_filter`: os comandos do condutor e os eventos que um
//!   `act()` da pagina produz, contados e engolidos (um Ctrl+Shift+P que
//!   escape nao abre o dialogo de impressao a meio da tabela);
//! - os comandos: abrir cada hospedeiro, pôr-lhe o teclado, e a sonda de
//!   teclas (`PROBE_*_SCRIPT`) no documento de topo.
//!
//! E uma excecao, so aqui: `install_column_fixture_navigation` deixa a
//! coluna carregar a fixture de 127.0.0.1 (a origem exata que o condutor
//! pediu) sem mexer no gate de navegacao da coluna que embarca, e escreve no
//! registo o que o WebView2 fez dessa navegacao (`colnav`); se a fixture nao
//! carregar, `open Column` sem URL repoe a pagina ao vivo do fornecedor.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;

use neural_core::{ReaderArticle, ReaderBlock};

use crate::accel_spike::{
    ColumnFixture, KeyEventKind, PROBE_ARM_SCRIPT, PROBE_PULL_SCRIPT, SPIKE_COMMAND_FILE,
    SPIKE_DIR_ENV, SPIKE_LOG_FILE, SpikeCommand, SpikeHost, SpikeKey, SpikeVerb, ack_line,
    act_line, colnav_done_line, colnav_start_line, hello_line, hooked_line, native_line, page_line,
    parse_spike_command, ping_detail, spike_verdict, tiny_pdf,
};
use crate::windows_app::*;

/// O registo do condutor; `None` sem `NEURALIA_ACCEL_SPIKE_DIR`.
static SPIKE_LOG: Mutex<Option<File>> = Mutex::new(None);
/// O pedido do condutor na coluna (`open Column <url>`): a origem da
/// fixture, a unica que a excecao de navegacao da coluna deixa passar, e a
/// navegacao que a leva. Vazio ate la.
static SPIKE_COLUMN_FIXTURE: Mutex<ColumnFixture> = Mutex::new(ColumnFixture::NONE);

/// O pedido da coluna, so pelo tempo de uma expressao: os handlers de
/// navegacao correm na mesma thread e tambem o tomam.
fn column_fixture() -> std::sync::MutexGuard<'static, ColumnFixture> {
    SPIKE_COLUMN_FIXTURE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A tentativa em curso (o `begin` do condutor): vai em cada linha nativa e
/// de `act()`, para o condutor as juntar a tentativa certa.
static SPIKE_TRIAL: AtomicU32 = AtomicU32::new(0);
/// O condutor esta a correr: so entao os eventos de `act()` sao engolidos.
static SPIKE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Uma linha no registo, se ele existir, com o fim de linha na mesma
/// escrita (o condutor le o ficheiro enquanto ele cresce). Um erro de
/// escrita nao sobe: o condutor da pela falta da linha.
fn spike_log(line: &str) {
    let mut slot = SPIKE_LOG
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(file) = slot.as_mut() {
        let _ = file.write_all(format!("{line}\n").as_bytes());
        let _ = file.flush();
    }
}

/// O evento que um `act()` (ou outra mensagem IPC de uma pagina) produziu,
/// pelo nome da contagem. `None`: um evento do proprio app, que segue.
fn spike_act(event: &UserEvent) -> Option<&'static str> {
    Some(match event {
        UserEvent::NewTab(_) => "newtab",
        UserEvent::PrintPage | UserEvent::PrintTarget(_) => "print",
        UserEvent::OpenDevTools | UserEvent::OpenDevToolsTarget(_) => "devtools",
        UserEvent::OpenSplit { .. }
        | UserEvent::OpenPrivateSplit { .. }
        | UserEvent::OpenSplitFromSplit { .. } => "split",
        UserEvent::ViewSource | UserEvent::ViewSourceTarget(_) => "viewsource",
        UserEvent::ShowHistory => "history",
        UserEvent::ClearHistory => "clearhistory",
        UserEvent::OpenPalette(_) | UserEvent::FocusOmnibox => "palette",
        UserEvent::HomeRequested => "home",
        UserEvent::BackRequested => "back",
        UserEvent::ReloadPage | UserEvent::ReloadTarget(_) => "reload",
        UserEvent::ToggleAutoScroll => "autoscroll",
        UserEvent::ZoomIn | UserEvent::ZoomOut | UserEvent::ZoomReset => "zoom",
        UserEvent::ExpandComparator(_)
        | UserEvent::MinimizeComparator(_)
        | UserEvent::ToggleColumnFullscreen
        | UserEvent::ToggleSplitFullscreen
        | UserEvent::RestoreComparator
        | UserEvent::CloseSplit => "layout",
        UserEvent::NoteRequested { .. } | UserEvent::NoteRefusedPrivate => "note",
        UserEvent::SearchSelection { .. } => "search",
        UserEvent::AskEverywhere { .. } => "ask",
        UserEvent::OpenExternal(_) | UserEvent::OpenEverywhere(_) | UserEvent::OpenInColumn(..) => {
            "navigate"
        }
        _ => return None,
    })
}

/// O `AcceleratorKeyPressed` do spike numa WebView acabada de construir.
/// `Handled` vem de `spike_verdict`; os modificadores, do `GetKeyState` da
/// thread da interface (o handler corre nela), como no exemplo do WebView2.
fn install_accel_spike(webview: &WebView, host: SpikeHost) -> Result<(), String> {
    use webview2_com::{
        AcceleratorKeyPressedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
        },
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_MENU};
    use wry::WebViewExtWindows;

    fn held(vk: u16) -> bool {
        unsafe { GetKeyState(i32::from(vk)) < 0 }
    }

    let controller = webview.controller();
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut raw_kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
        let mut vk = 0u32;
        let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
        unsafe {
            args.KeyEventKind(&mut raw_kind)?;
            args.VirtualKey(&mut vk)?;
            args.PhysicalKeyStatus(&mut status)?;
        }
        let Some(kind) = KeyEventKind::from_raw(raw_kind.0) else {
            return Ok(());
        };
        let verdict = spike_verdict(SpikeKey {
            vk,
            kind,
            ctrl: held(VK_CONTROL),
            shift: held(VK_SHIFT),
            alt: held(VK_MENU),
            was_down: status.WasKeyDown.as_bool(),
        });
        unsafe { args.SetHandled(verdict.handled)? };
        if let Some(line) = native_line(SPIKE_TRIAL.load(Ordering::Acquire), host, kind, verdict) {
            spike_log(&line);
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) }
        .map_err(|error| format!("add_AcceleratorKeyPressed falhou: {error}"))
}

/// So no exe do spike e so nas colunas: um segundo `NavigationStarting`,
/// registado DEPOIS do que o `comparator_webview_builder` pos (o gate que
/// embarca, que este codigo nao toca), que volta a deixar passar a
/// navegacao para a origem exata da fixture que o condutor pediu
/// (`column_fixture_navigation`: http, 127.0.0.1, a porta da fixture). O
/// WebView2 chama os handlers pela ordem de registo com os mesmos args, e o
/// `Cancel` que fica e o do ultimo. Assim a coluna mede-se na fixture de
/// 127.0.0.1, sem rede, como os outros hospedeiros web.
///
/// O que o runtime fez dela vai para o registo: o `Cancel` antes e depois da
/// excecao (`colnav start`) e o `NavigationCompleted` dessa navegacao
/// (`colnav done`, com `IsSuccess` e o `WebErrorStatus`). Se ela nao
/// carregar, o condutor sabe-o logo por aqui, escreve o motivo na nota da
/// tabela e mede a coluna na pagina ao vivo (`open Column` sem URL).
fn install_column_fixture_navigation(webview: &WebView) -> Result<(), String> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_ERROR_STATUS,
        NavigationCompletedEventHandler, NavigationStartingEventHandler, take_pwstr,
    };
    use windows_core::{BOOL, PWSTR};
    use wry::WebViewExtWindows;

    let starting = NavigationStartingEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let uri = {
            let mut uri = PWSTR::null();
            unsafe { args.Uri(&mut uri)? };
            take_pwstr(uri)
        };
        let mut navigation = 0u64;
        unsafe { args.NavigationId(&mut navigation)? };
        if !column_fixture().starting(&uri, navigation) {
            return Ok(());
        }
        let mut gate = BOOL::default();
        let mut cancel = BOOL::default();
        unsafe {
            args.Cancel(&mut gate)?;
            args.SetCancel(false)?;
            args.Cancel(&mut cancel)?;
        }
        spike_log(&colnav_start_line(
            navigation,
            &uri,
            gate.as_bool(),
            cancel.as_bool(),
        ));
        Ok(())
    }));
    let completed = NavigationCompletedEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut navigation = 0u64;
        unsafe { args.NavigationId(&mut navigation)? };
        if !column_fixture().completed(navigation) {
            return Ok(());
        }
        let mut ok = BOOL::default();
        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
        unsafe {
            args.IsSuccess(&mut ok)?;
            args.WebErrorStatus(&mut status)?;
        }
        spike_log(&colnav_done_line(navigation, ok.as_bool(), status.0));
        Ok(())
    }));
    let core = webview.webview();
    let mut token = 0i64;
    unsafe { core.add_NavigationStarting(&starting, &mut token) }
        .map_err(|error| format!("add_NavigationStarting falhou: {error}"))?;
    unsafe { core.add_NavigationCompleted(&completed, &mut token) }
        .map_err(|error| format!("add_NavigationCompleted falhou: {error}"))
}

/// Le `cmd.txt` da pasta do condutor (que o escreve por rename) e manda cada
/// linha ao event loop. O rename para `cmd.taken` e a posse: um ficheiro a
/// meio de ser escrito nunca e lido.
fn spawn_command_reader(dir: PathBuf, proxy: EventLoopProxy<UserEvent>) {
    let _ = thread::Builder::new()
        .name("accel-spike-commands".to_string())
        .spawn(move || {
            let command = dir.join(SPIKE_COMMAND_FILE);
            let taken = dir.join("cmd.taken");
            loop {
                thread::sleep(Duration::from_millis(25));
                if std::fs::rename(&command, &taken).is_err() {
                    continue;
                }
                let text = std::fs::read_to_string(&taken).unwrap_or_default();
                let _ = std::fs::remove_file(&taken);
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    if proxy
                        .send_event(UserEvent::AccelSpike(line.trim().to_string()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
}

fn open_spike_log(dir: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(SPIKE_LOG_FILE))
}

impl App {
    /// No arranque (depois de a janela existir): com
    /// `NEURALIA_ACCEL_SPIKE_DIR`, abre o registo, escreve a tabela, comeca a
    /// ler comandos e da a pergunta da rolagem automatica por respondida (a
    /// pergunta pintada por cima das paginas nao faz parte do que se mede).
    pub(in crate::windows_app) fn accel_spike_start(&mut self) {
        let Some(dir) = std::env::var_os(SPIKE_DIR_ENV).map(PathBuf::from) else {
            return;
        };
        match open_spike_log(&dir) {
            Ok(file) => {
                *SPIKE_LOG
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(file);
            }
            Err(error) => {
                debug_log(format_args!("accel spike: sem registo ({error})"));
                return;
            }
        }
        SPIKE_ACTIVE.store(true, Ordering::Release);
        self.auto_scroll_answered = true;
        spike_log(&hello_line(std::process::id()));
        spawn_command_reader(dir, self.proxy.clone());
    }

    /// Chamado em cada sitio onde uma WebView de um hospedeiro da tabela
    /// nasce. Uma falha fica no registo (`hooked` com `ok:false`) e o
    /// condutor da esse hospedeiro por falhado.
    pub(in crate::windows_app) fn accel_spike_hook(&self, webview: &WebView, host: SpikeHost) {
        let result = install_accel_spike(webview, host);
        if let Err(error) = &result {
            debug_log(format_args!(
                "accel spike: {} sem handler ({error})",
                host.name()
            ));
        }
        spike_log(&hooked_line(host, result.err().as_deref()));
        // A excecao da fixture nao e o que se mede: sem ela a coluna fica na
        // pagina ao vivo (o condutor ve o endereco na sonda e anota-o).
        if host == SpikeHost::Column
            && let Err(error) = install_column_fixture_navigation(webview)
        {
            debug_log(format_args!(
                "accel spike: coluna sem a excecao da fixture ({error})"
            ));
        }
    }

    /// O primeiro passo de `user_event`: os comandos do condutor correm aqui
    /// e os eventos de `act()` das paginas sao contados e engolidos enquanto
    /// o condutor corre. O resto segue.
    pub(in crate::windows_app) fn accel_spike_filter(
        &mut self,
        event: UserEvent,
    ) -> Option<UserEvent> {
        if let UserEvent::AccelSpike(line) = event {
            self.accel_spike_command(&line);
            return None;
        }
        if !SPIKE_ACTIVE.load(Ordering::Acquire) {
            return Some(event);
        }
        match spike_act(&event) {
            Some(act) => {
                spike_log(&act_line(SPIKE_TRIAL.load(Ordering::Acquire), act));
                None
            }
            None => Some(event),
        }
    }

    fn accel_spike_command(&mut self, line: &str) {
        let SpikeCommand { seq, host, verb } = match parse_spike_command(line) {
            Ok(command) => command,
            Err(error) => {
                spike_log(&ack_line(0, false, &error));
                return;
            }
        };
        let result = match verb {
            SpikeVerb::Ping => Ok(ping_detail(
                host,
                self.accel_spike_webview(host).is_some(),
                &format!("{:?}", self.surface),
            )),
            SpikeVerb::Open(url) => self.accel_spike_open(host, url.as_deref()),
            SpikeVerb::Focus => self.accel_spike_focus(host),
            SpikeVerb::Arm => self.accel_spike_eval(seq, 0, host, "arm", PROBE_ARM_SCRIPT),
            SpikeVerb::Begin(trial) => {
                SPIKE_TRIAL.store(trial, Ordering::Release);
                self.accel_spike_eval(seq, trial, host, "begin", PROBE_ARM_SCRIPT)
            }
            SpikeVerb::Pull(trial) => {
                self.accel_spike_eval(seq, trial, host, "pull", PROBE_PULL_SCRIPT)
            }
        };
        match result {
            Ok(detail) => spike_log(&ack_line(seq, true, &detail)),
            Err(error) => spike_log(&ack_line(seq, false, &error)),
        }
    }

    /// A WebView de cada hospedeiro, se ele estiver aberto agora.
    fn accel_spike_webview(&self, host: SpikeHost) -> Option<&WebView> {
        let single = |surface: Surface| {
            (self.surface == surface)
                .then_some(self.webview.as_ref())
                .flatten()
        };
        match host {
            SpikeHost::Column => (self.surface == Surface::Comparator)
                .then(|| {
                    self.comparator
                        .as_ref()?
                        .views
                        .first()
                        .map(|view| &view.webview)
                })
                .flatten(),
            SpikeHost::Split | SpikeHost::PrivateSplit => self
                .comparator
                .as_ref()?
                .split
                .as_ref()
                .filter(|split| split.private == (host == SpikeHost::PrivateSplit))
                .map(|split| &split.webview),
            SpikeHost::SidePanel => self.side_panel.view(),
            SpikeHost::Service => self.service_panel.as_ref().map(|panel| &panel.webview),
            SpikeHost::External => single(Surface::External),
            SpikeHost::Reader => single(Surface::Reader),
            SpikeHost::Pdf => single(Surface::Pdf),
            SpikeHost::Epub => single(Surface::Epub),
        }
    }

    /// Abre o hospedeiro pelo mesmo metodo que o produto usa. A coluna e a
    /// primeira do comparador que o `NEURALIA_STARTUP_INPUT` abriu; com a
    /// URL da fixture, ela navega para la pela excecao do spike
    /// (`install_column_fixture_navigation`). Sem ela fica na pagina ao
    /// vivo -- e, se a fixture tinha sido pedida (a coluna pode ter ficado a
    /// meio dela, ou sem pagina), volta a pagina ao vivo do fornecedor pelo
    /// gate que embarca, que a deixa passar.
    fn accel_spike_open(&mut self, host: SpikeHost, url: Option<&str>) -> Result<String, String> {
        let fixture = url.unwrap_or_default().to_string();
        let mut detail = String::new();
        match host {
            SpikeHost::Column => {
                let column = self
                    .comparator
                    .as_ref()
                    .and_then(|comparator| comparator.views.first())
                    .ok_or_else(|| {
                        format!(
                            "sem coluna do comparador (surface {:?}): o NEURALIA_STARTUP_INPUT nao o abriu",
                            self.surface
                        )
                    })?;
                if let Some(url) = url {
                    column_fixture().request(url);
                    column
                        .webview
                        .load_url(url)
                        .map_err(|error| format!("load_url da fixture: {error}"))?;
                    detail = ": fixture pedida".to_string();
                } else if column_fixture().release() {
                    let live = self
                        .provider_query_url(0, &startup_input())
                        .map_err(|error| format!("pagina ao vivo: {error}"))?;
                    column
                        .webview
                        .load_url(live.as_str())
                        .map_err(|error| format!("load_url da pagina ao vivo: {error}"))?;
                    detail = format!(
                        ": pagina ao vivo reposta ({}://{})",
                        live.scheme(),
                        live.host_str().unwrap_or_default()
                    );
                } else {
                    detail = ": pagina ao vivo".to_string();
                }
            }
            SpikeHost::Split => {
                let _ = self.open_split(0, fixture, true);
            }
            SpikeHost::PrivateSplit => {
                let _ = self.open_split_mode(0, fixture, true, true, None);
            }
            SpikeHost::SidePanel => {
                if !self.side_panel.is_open() {
                    self.open_side_panel();
                }
            }
            SpikeHost::Service => {
                if self.service_panel.is_none() {
                    self.open_service_panel(Service::YouTube);
                }
                if let Some(panel) = &self.service_panel {
                    panel
                        .webview
                        .load_url(&fixture)
                        .map_err(|error| format!("load_url: {error}"))?;
                }
            }
            SpikeHost::External => self.open_external(&fixture),
            SpikeHost::Reader => self.open_reader(&ReaderArticle {
                source_url: "http://127.0.0.1/accel-spike-reader".to_string(),
                title: "Spike de aceleradores".to_string(),
                byline: None,
                excerpt: None,
                blocks: vec![
                    ReaderBlock::Heading {
                        level: 1,
                        text: "Spike de aceleradores".to_string(),
                    },
                    ReaderBlock::Paragraph(
                        "Pagina do Leitor para o spike do AcceleratorKeyPressed.".to_string(),
                    ),
                ],
            }),
            SpikeHost::Pdf => self.open_pdf("http://127.0.0.1/accel-spike.pdf", tiny_pdf()),
            SpikeHost::Epub => self.open_library(),
        }
        if self.accel_spike_webview(host).is_some() {
            Ok(format!("{} aberto{detail}", host.name()))
        } else {
            Err(format!(
                "{} nao abriu (surface {:?})",
                host.name(),
                self.surface
            ))
        }
    }

    /// Janela em primeiro plano e o teclado na WebView do hospedeiro. O
    /// detalhe diz se a janela ficou mesmo em primeiro plano: o condutor so
    /// carrega em teclas com ela a frente.
    fn accel_spike_focus(&self, host: SpikeHost) -> Result<String, String> {
        let window = self.window.as_ref().ok_or("sem janela")?;
        window.focus_window();
        let webview = self
            .accel_spike_webview(host)
            .ok_or_else(|| format!("{} nao esta aberto", host.name()))?;
        webview.focus().map_err(|error| format!("focus: {error}"))?;
        let foreground = unsafe { GetForegroundWindow() };
        let ours = window_hwnd(window).is_some_and(|hwnd| hwnd == foreground);
        Ok(format!("foreground={ours}"))
    }

    /// Corre um script da sonda no documento de topo do hospedeiro. O
    /// resultado chega depois, numa linha `page` com o mesmo `seq`.
    fn accel_spike_eval(
        &self,
        seq: u64,
        trial: u32,
        host: SpikeHost,
        phase: &'static str,
        script: &str,
    ) -> Result<String, String> {
        let webview = self
            .accel_spike_webview(host)
            .ok_or_else(|| format!("{} nao esta aberto", host.name()))?;
        webview
            .evaluate_script_with_callback(script, move |raw| {
                spike_log(&page_line(seq, trial, host, phase, &raw));
            })
            .map_err(|error| format!("evaluate_script: {error}"))?;
        Ok(phase.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O que os atalhos da tabela fazem hoje quando a pagina os recebe --
    /// Ctrl+Shift+N (aba nova), Ctrl+Shift+P (imprimir) -- e as outras
    /// saidas do mapa de teclas contam como `act()` e nao correm durante a
    /// tabela; os eventos do proprio app seguem.
    #[test]
    fn page_actions_are_counted_and_app_events_pass() {
        let counted = [
            (UserEvent::NewTab(0), "newtab"),
            (UserEvent::PrintTarget(PageTarget::Column(0)), "print"),
            (UserEvent::PrintPage, "print"),
            (UserEvent::OpenDevToolsTarget(PageTarget::Split), "devtools"),
            (
                UserEvent::OpenSplit {
                    source_index: 0,
                    url: "https://example.com/".to_string(),
                },
                "split",
            ),
            (UserEvent::ClearHistory, "clearhistory"),
            (UserEvent::BackRequested, "back"),
        ];
        for (event, name) in &counted {
            assert_eq!(spike_act(event), Some(*name), "{event:?}");
        }
        for event in [
            UserEvent::RelayoutComparator,
            UserEvent::SubmitText("pergunta".to_string()),
            UserEvent::AccelSpike("1 arm Column".to_string()),
            UserEvent::HideSplash(1),
        ] {
            assert_eq!(spike_act(&event), None, "{event:?}");
        }
    }
}
