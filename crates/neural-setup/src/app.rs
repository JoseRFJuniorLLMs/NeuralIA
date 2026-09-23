//! A janela do instalador e a maquina de estados por tras dela.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::archive::{self, Entry};
use crate::install::{self, Failure, FailureKind, InstallError, Places, Plan, Stage};
use crate::paint;
use crate::ui::{FRAME_MS, Hit, Layout, Rect, Screen, TissueClock, WINDOW_H, WINDOW_W};
use crate::winshell;

/// A carga util que o `build.rs` empacotou.
const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
/// A versao que fica escrita em "Aplicacoes". E a do workspace: o script de
/// empacotamento recusa construir o instalador com outra.
const VERSION: &str = env!("CARGO_PKG_VERSION");
const TIMER_ID: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Install,
    Uninstall,
}

/// O que a linha de comandos pediu: o que fazer, se ha alguem a frente do ecra
/// para carregar em botoes, e a pasta, se foi dada com `/D=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub mode: Mode,
    pub quiet: bool,
    pub install_dir: Option<PathBuf>,
}

/// O que a thread de trabalho vai dizendo a janela. Numeros atomicos em vez de
/// mensagens: a janela le quando desenha, e um quadro atrasado nao faz mal
/// nenhum -- o que fazia mal era bloquear o desenho a espera do disco.
struct Shared {
    /// Fase (`Stage as u8`) e fracao dentro dela, em milesimos.
    stage: AtomicU64,
    within: AtomicU64,
    outcome: Mutex<Option<Result<(), Failure>>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            stage: AtomicU64::new(0),
            within: AtomicU64::new(0),
            outcome: Mutex::new(None),
        }
    }

    fn report(&self, stage: Stage, within: f64) {
        self.stage.store(stage as u64, Ordering::Relaxed);
        self.within
            .store((within.clamp(0.0, 1.0) * 1000.0) as u64, Ordering::Relaxed);
    }

    fn read(&self) -> (Stage, f64) {
        let stage = match self.stage.load(Ordering::Relaxed) {
            0 => Stage::Preparing,
            1 => Stage::Writing,
            2 => Stage::Shortcuts,
            3 => Stage::Registering,
            _ => Stage::Done,
        };
        (stage, self.within.load(Ordering::Relaxed) as f64 / 1000.0)
    }
}

/// Para onde vai esta corrida: a pasta, os sitios do sistema (os do
/// utilizador, ou os de uma pasta de ensaio) e a pasta dos dados, que nunca se
/// toca.
#[derive(Debug, Clone)]
struct Target {
    root: PathBuf,
    places: Places,
    data_dir: PathBuf,
    /// Para onde o desinstalador a correr se muda (a pasta temporaria).
    parking: PathBuf,
}

impl Target {
    /// Onde podem ter ficado desinstaladores estacionados de outras vezes.
    fn parking_folders(&self) -> Vec<PathBuf> {
        let mut folders = vec![self.parking.clone()];
        folders.extend(self.root.parent().map(Path::to_path_buf));
        folders
    }
}

fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Os sitios desta corrida. Com `NEURALIA_SETUP_SANDBOX` definida, os de uma
/// pasta de ensaio; senao, os do utilizador.
fn places() -> Result<Places, Failure> {
    match std::env::var_os(install::SANDBOX_ENV) {
        Some(sandbox) if !sandbox.is_empty() => install::sandbox_places(Path::new(&sandbox))
            .map_err(|why| Failure::new(FailureKind::BadArguments, why)),
        _ => Ok(Places {
            uninstall_base: install::UNINSTALL_BASE.to_string(),
            start_menu: winshell::start_menu_programs(),
            desktop: winshell::desktop(),
            default_root: install::install_root(&local_app_data()),
        }),
    }
}

fn resolve(launch: &Launch) -> Result<Target, Failure> {
    let places = places()?;
    // A mesma conta que a NeuralIA faz para saber onde guardar os dados.
    let data_dir = neural_core::config::default_data_dir();
    let own = winshell::registered_location(&places.key());
    let root = match launch.mode {
        Mode::Install => {
            let inno = winshell::inno_registration(&places.inno_key())
                .and_then(|registration| registration.install_location);
            install::choose_install_root(
                launch.install_dir.as_deref(),
                &[own, inno],
                &places.default_root,
                &data_dir,
            )
            .map_err(|why| Failure::new(FailureKind::BadArguments, why))?
        }
        Mode::Uninstall => {
            let me = std::env::current_exe().ok();
            let root = install::choose_uninstall_root(
                launch.install_dir.as_deref(),
                me.as_deref(),
                own.as_deref(),
                &places.default_root,
            );
            install::validate_install_root(&root, &data_dir)
                .map_err(|why| Failure::new(FailureKind::BadArguments, why))?;
            root
        }
    };
    Ok(Target {
        root,
        places,
        data_dir,
        parking: std::env::temp_dir(),
    })
}

struct Setup {
    mode: Mode,
    screen: Screen,
    hover: Option<Hit>,
    desktop_shortcut: bool,
    progress: f64,
    stage: Stage,
    message: String,
    /// A mensagem e um aviso (a NeuralIA aberta), nao uma informacao.
    warning: bool,
    started: Instant,
    clock: TissueClock,
    shared: Arc<Shared>,
    entries: Vec<Entry>,
    target: Result<Target, Failure>,
    /// Fica ligado quando o utilizador carrega em "Abrir a NeuralIA".
    open_after: bool,
}

impl Setup {
    fn new(launch: &Launch) -> Self {
        let entries = archive::unpack(PAYLOAD).unwrap_or_default();
        let target = resolve(launch);
        let (screen, message) = match (&target, launch.mode, entries.is_empty()) {
            (Err(failure), _, _) => (Screen::Failed, failure.message.clone()),
            (Ok(_), Mode::Install, true) => (
                Screen::Failed,
                "Este instalador foi construido sem a NeuralIA la dentro.".to_string(),
            ),
            (Ok(target), Mode::Install, false) => (
                Screen::Welcome,
                format!("Vai ficar em {}", target.root.display()),
            ),
            (Ok(target), Mode::Uninstall, _) => (
                Screen::Welcome,
                format!("Vai remover {}", target.root.display()),
            ),
        };
        Self {
            mode: launch.mode,
            screen,
            hover: None,
            desktop_shortcut: true,
            progress: 0.0,
            stage: Stage::Preparing,
            message,
            warning: false,
            started: Instant::now(),
            clock: TissueClock::default(),
            shared: Arc::new(Shared::new()),
            entries,
            target,
            open_after: false,
        }
    }

    /// O titulo diz o **estado**, nunca o nome: a arte ja diz "NeuralIA", e
    /// repeti-lo logo por baixo era a mesma palavra duas vezes no mesmo ecra.
    fn title(&self) -> &str {
        match (self.mode, self.screen) {
            (_, Screen::Failed) => "Nao foi possivel",
            (Mode::Install, Screen::Finished) => "Tudo pronto",
            (Mode::Uninstall, Screen::Finished) => "Removida",
            (Mode::Uninstall, Screen::Working) => "A remover",
            (Mode::Uninstall, _) => "Remover do computador",
            (Mode::Install, Screen::Working) => "A instalar",
            _ => "Pronta a instalar",
        }
    }

    fn primary_label(&self) -> &str {
        match (self.mode, self.screen) {
            (_, Screen::Failed) => "Fechar",
            (Mode::Install, Screen::Finished) => "Abrir a NeuralIA",
            (Mode::Uninstall, Screen::Finished) => "Fechar",
            (Mode::Uninstall, _) => "Desinstalar",
            _ => "Instalar",
        }
    }

    fn begin(&mut self, hwnd: HWND) {
        if self.screen != Screen::Welcome {
            return;
        }
        let Ok(target) = self.target.clone() else {
            return;
        };
        // A NeuralIA aberta: pede-se para a fechar, e o ecra fica onde esta.
        // Carregar outra vez depois de a fechar continua.
        let me = std::env::current_exe().ok();
        let busy = busy_files(self.mode, &target.root, &self.entries, me.as_deref());
        if !busy.is_empty() {
            self.message = install::in_use_message(&busy);
            self.warning = true;
            unsafe {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            return;
        }
        self.warning = false;
        self.screen = Screen::Working;
        self.progress = 0.0;

        let plan = Plan {
            root: target.root.clone(),
            entries: self.entries.clone(),
            desktop_shortcut: self.desktop_shortcut,
        };
        let shared = Arc::clone(&self.shared);
        let mode = self.mode;
        std::thread::spawn(move || {
            winshell::init_com();
            let result = work(mode, &plan, &target, &shared);
            shared.report(Stage::Done, 1.0);
            *shared.outcome.lock().expect("outcome") = Some(result);
        });
        unsafe {
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }

    /// Chamado pelo temporizador: trazer o que a thread disse para o ecra.
    fn tick(&mut self) {
        if self.screen != Screen::Working {
            return;
        }
        let (stage, within) = self.shared.read();
        self.stage = stage;
        // `max` e nao `=`: dois relatos fora de ordem nunca fazem a barra
        // recuar a vista de toda a gente.
        self.progress = self.progress.max(install::overall_progress(stage, within));

        let outcome = self.shared.outcome.lock().expect("outcome").take();
        match outcome {
            Some(Ok(())) => {
                self.progress = 1.0;
                self.screen = Screen::Finished;
                let root = self.target.as_ref().map(|t| t.root.display().to_string());
                self.message = match self.mode {
                    Mode::Install => format!("Instalada em {}", root.unwrap_or_default()),
                    Mode::Uninstall => "A NeuralIA foi removida deste computador.".to_string(),
                };
            }
            Some(Err(failure)) => {
                self.screen = Screen::Failed;
                self.message = failure.message;
            }
            None => {}
        }
    }
}

/// Os ficheiros que impedem de comecar: a carga util aberta (a NeuralIA a
/// correr) e, a instalar, o desinstalador da pasta aberto -- salvo se for este
/// mesmo programa, que se sabe substituir a si proprio.
fn busy_files(mode: Mode, root: &Path, entries: &[Entry], me: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = install::payload_files(root, entries);
    let uninstaller = root.join(install::UNINSTALLER);
    if mode == Mode::Install && !me.is_some_and(|me| install::same_path(me, &uninstaller)) {
        paths.push(uninstaller);
    }
    install::files_in_use(&paths)
}

/// O trabalho, sem janela: e o que o botao e o modo silencioso fazem.
fn work(mode: Mode, plan: &Plan, target: &Target, shared: &Shared) -> Result<(), Failure> {
    match mode {
        Mode::Install => {
            let me = std::env::current_exe().map_err(|e| {
                Failure::new(
                    FailureKind::Failed,
                    format!("nao sei onde estou no disco: {e}"),
                )
            })?;
            install_with(plan, target, &me, shared)
        }
        Mode::Uninstall => uninstall_with(plan, target, shared),
    }
}

/// O que o Inno das 2.1.x deixou nesta pasta e no registo, dado o estado de
/// agora.
fn legacy_cleanup(root: &Path, places: &Places) -> install::LegacyCleanup {
    let folder = install::read_folder(root);
    let registration = winshell::inno_registration(&places.inno_key());
    let present = registration
        .as_ref()
        .and_then(|r| r.install_location.as_deref())
        .is_some_and(|location| location.join(install::EXECUTABLE).is_file());
    install::plan_legacy_cleanup(root, &folder, registration.as_ref(), present)
}

fn remove_legacy(cleanup: &install::LegacyCleanup, places: &Places) {
    // Um `unins000.exe` que nao sai (aberto, por exemplo) fica para a proxima;
    // a nossa entrada ja esta escrita, e e a que "Aplicacoes" mostra.
    let _ = install::remove_legacy_files(cleanup);
    if cleanup.unregister {
        winshell::delete_key(&places.inno_key());
    }
}

/// A instalacao, com tudo o que toca no sistema a sair do `target`: a pasta, a
/// chave de "Aplicacoes" e as pastas dos atalhos. Os testes passam-lhe uma
/// pasta de ensaio e correm exatamente isto.
///
/// A ordem e a que nunca deixa o utilizador sem NeuralIA nem com duas: a
/// guarda dos dados, a NeuralIA aberta, a carga util (tudo ou nada), os
/// atalhos, o desinstalador e a nossa entrada -- e so no fim, com a nossa
/// entrada escrita, e que sai o que o Inno deixou.
fn install_with(plan: &Plan, target: &Target, me: &Path, shared: &Shared) -> Result<(), Failure> {
    shared.report(Stage::Preparing, 0.0);
    if plan.entries.is_empty() {
        return Err(InstallError::NoPayload.into());
    }
    let preview = legacy_cleanup(&plan.root, &target.places);
    install::guard_user_data(&plan.root, &plan.entries, &preview, &target.data_dir)?;
    let busy = busy_files(Mode::Install, &plan.root, &plan.entries, Some(me));
    if !busy.is_empty() {
        return Err(InstallError::InUse(busy).into());
    }
    install::sweep_parked(&target.parking_folders());

    install::write_payload(plan, |fraction| shared.report(Stage::Writing, fraction))?;

    shared.report(Stage::Shortcuts, 0.0);
    let exe = plan.executable();
    if let Some(programs) = &target.places.start_menu {
        winshell::create_shortcut(
            &programs.join(format!("{}.lnk", install::PRODUCT)),
            &exe,
            &plan.root,
            "NeuralIA",
            &exe,
        )
        .map_err(|why| Failure::new(FailureKind::Failed, why))?;
    }
    shared.report(Stage::Shortcuts, 0.5);
    if plan.desktop_shortcut
        && let Some(desktop) = &target.places.desktop
    {
        winshell::create_shortcut(
            &desktop.join(format!("{}.lnk", install::PRODUCT)),
            &exe,
            &plan.root,
            "NeuralIA",
            &exe,
        )
        .map_err(|why| Failure::new(FailureKind::Failed, why))?;
    }

    // O desinstalador e este mesmo programa, guardado ao lado da aplicacao.
    shared.report(Stage::Registering, 0.0);
    let uninstaller = plan.uninstaller();
    install::place_uninstaller(me, &uninstaller)?;
    let size_kb = (plan.total_bytes() / 1024).max(1) as u32;
    winshell::register_uninstall(
        &target.places.key(),
        &plan.root,
        &uninstaller,
        &exe,
        VERSION,
        size_kb,
    )
    .map_err(|why| Failure::new(FailureKind::Failed, why))?;

    shared.report(Stage::Registering, 0.5);
    remove_legacy(&legacy_cleanup(&plan.root, &target.places), &target.places);
    Ok(())
}

/// O atalho `link` abre a NeuralIA desta pasta? So esses se apagam: um atalho
/// para outra instalacao nao e deste desinstalador.
fn shortcut_points_into(link: &Path, root: &Path) -> bool {
    winshell::shortcut_details(link)
        .is_ok_and(|details| install::same_path(&details.target, &root.join(install::EXECUTABLE)))
}

/// A desinstalacao, com o que toca no sistema a sair do `target` -- e assim
/// que os testes a correm sem apagar a instalacao verdadeira de quem os corre.
fn uninstall_with(plan: &Plan, target: &Target, shared: &Shared) -> Result<(), Failure> {
    shared.report(Stage::Preparing, 0.0);
    let preview = legacy_cleanup(&plan.root, &target.places);
    install::guard_user_data(&plan.root, &plan.entries, &preview, &target.data_dir)?;
    let busy = busy_files(Mode::Uninstall, &plan.root, &plan.entries, None);
    if !busy.is_empty() {
        return Err(InstallError::InUse(busy).into());
    }
    install::sweep_parked(&target.parking_folders());

    // Se algum ficheiro ficou, para aqui: os atalhos e a entrada em
    // Aplicacoes continuam, para a NeuralIA que ficou poder ser aberta e
    // desinstalada outra vez depois de fechada.
    install::remove_installed(&plan.root, &plan.entries, &target.parking, |fraction| {
        shared.report(Stage::Writing, fraction)
    })
    .map_err(|left| {
        let first = left
            .first()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Failure::new(
            FailureKind::InUse,
            format!(
                "Feche a NeuralIA e tente outra vez. {} ficheiro(s) em uso nao sairam, a comecar por {first}.",
                left.len()
            ),
        )
    })?;

    shared.report(Stage::Shortcuts, 0.0);
    for folder in target.places.shortcut_folders() {
        let link = folder.join(format!("{}.lnk", install::PRODUCT));
        if shortcut_points_into(&link, &plan.root) {
            let _ = std::fs::remove_file(link);
        }
    }

    shared.report(Stage::Registering, 0.0);
    let key = target.places.key();
    if install::registration_points_here(winshell::registered_location(&key).as_deref(), &plan.root)
    {
        winshell::delete_key(&key);
    }
    // Uma NeuralIA que veio do Inno e se remove por aqui tambem leva a
    // entrada e o desinstalador dele: senao "Aplicacoes" ficava com uma
    // NeuralIA que ja nao existe.
    let cleanup = legacy_cleanup(&plan.root, &target.places);
    remove_legacy(&cleanup, &target.places);
    install::remove_empty_folders(&plan.root, &cleanup.files);
    Ok(())
}

/// O modo silencioso (`/S`): o mesmo trabalho que o botao faria, sem janela, e
/// o resultado no codigo de saida (ver `install::exit_code`). Quem o corre (um
/// script, o `winget --silent`) nao tem ecra onde carregar num botao.
pub fn run_quiet(launch: &Launch) -> i32 {
    winshell::init_com();
    let entries = archive::unpack(PAYLOAD).unwrap_or_default();
    let outcome = resolve(launch).and_then(|target| {
        let plan = Plan {
            root: target.root.clone(),
            entries,
            desktop_shortcut: true,
        };
        work(launch.mode, &plan, &target, &Shared::new())
    });
    install::exit_code(&outcome)
}

pub fn run(launch: &Launch) {
    unsafe {
        winshell::init_com();
        let instance = GetModuleHandleW(std::ptr::null());
        let class = "NeuralIASetup\0".encode_utf16().collect::<Vec<u16>>();
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            // O icone 1 do recurso PE que o `build.rs` compilou. O Win32 quer o
            // id como se fosse um ponteiro; nao se desreferencia.
            hIcon: LoadIconW(instance, std::ptr::without_provenance(1)),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);

        let title = "NeuralIA\0".encode_utf16().collect::<Vec<u16>>();
        let state = Box::into_raw(Box::new(Setup::new(launch)));
        // `WS_MINIMIZEBOX` deixa minimizar pela barra de tarefas; minimizada,
        // a janela deixa de desenhar o tecido.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_W as i32,
            WINDOW_H as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            state as *mut _,
        );
        if hwnd.is_null() {
            drop(Box::from_raw(state));
            return;
        }

        center_and_scale(hwnd);
        SetTimer(hwnd, TIMER_ID, FRAME_MS, None);
        ShowWindow(hwnd, SW_SHOW);

        let mut message = std::mem::zeroed::<MSG>();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// Tamanho certo para o DPI do monitor onde a janela nasceu, e ao centro dele.
unsafe fn center_and_scale(hwnd: HWND) {
    let dpi = GetDpiForWindow(hwnd);
    let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
    let width = (WINDOW_W * scale) as i32;
    let height = (WINDOW_H * scale) as i32;
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    SetWindowPos(
        hwnd,
        std::ptr::null_mut(),
        (screen_w - width) / 2,
        (screen_h - height) / 2,
        width,
        height,
        SWP_NOZORDER,
    );
}

unsafe fn state_of(hwnd: HWND) -> Option<&'static mut Setup> {
    let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Setup;
    if raw.is_null() { None } else { Some(&mut *raw) }
}

unsafe fn layout_of(hwnd: HWND) -> Layout {
    let mut client = std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>();
    GetClientRect(hwnd, &mut client);
    let dpi = GetDpiForWindow(hwnd);
    let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
    Layout::new(client.right as f64, client.bottom as f64, scale)
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CREATE => {
            let create = lparam as *const CREATESTRUCTW;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize);
            0
        }
        WM_ERASEBKGND => 1,
        WM_TIMER => {
            if let Some(state) = state_of(hwnd) {
                // O tecido anda com o relogio, mas so enquanto a janela se ve:
                // minimizada nao se desenha nada, e ao voltar continua de onde
                // estava. O temporizador so pede um redesenho -- nunca poe a
                // janela a frente de nada.
                let visible = IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0;
                state.clock = state
                    .clock
                    .advance(state.started.elapsed().as_secs_f64(), visible);
                state.tick();
                if visible {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(state) = state_of(hwnd) {
                let layout = layout_of(hwnd);
                let (x, y) = (
                    (lparam & 0xffff) as i16 as f64,
                    ((lparam >> 16) & 0xffff) as i16 as f64,
                );
                let hover = layout.hit(state.screen, x, y);
                let hover = if hover == Some(Hit::Caption) {
                    None
                } else {
                    hover
                };
                if hover != state.hover {
                    state.hover = hover;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            let Some(state) = state_of(hwnd) else {
                return 0;
            };
            let layout = layout_of(hwnd);
            let (x, y) = (
                (lparam & 0xffff) as i16 as f64,
                ((lparam >> 16) & 0xffff) as i16 as f64,
            );
            match layout.hit(state.screen, x, y) {
                Some(Hit::Close) => {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                Some(Hit::Primary) => match state.screen {
                    Screen::Welcome => state.begin(hwnd),
                    Screen::Finished if state.mode == Mode::Install => {
                        state.open_after = true;
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                    _ => {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                },
                Some(Hit::Secondary) => {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                Some(Hit::DesktopShortcut) => {
                    state.desktop_shortcut = !state.desktop_shortcut;
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                Some(Hit::Caption) => {
                    // Arrastar a janela sem moldura: devolver o rato ao
                    // Windows e fingir um clique na barra de titulo.
                    ReleaseCapture();
                    SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
                }
                None => {}
            }
            0
        }
        WM_PAINT => {
            if let Some(state) = state_of(hwnd) {
                paint_window(hwnd, state);
            }
            0
        }
        WM_CLOSE => {
            // A meio da instalacao, fechar deixaria uma pasta pela metade.
            if state_of(hwnd).is_some_and(|s| s.screen == Screen::Working) {
                return 0;
            }
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_ID);
            let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Setup;
            if !raw.is_null() {
                let state = Box::from_raw(raw);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if state.open_after
                    && let Ok(target) = &state.target
                {
                    let _ = std::process::Command::new(target.root.join(install::EXECUTABLE))
                        .current_dir(&target.root)
                        .spawn();
                }
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn paint_window(hwnd: HWND, state: &Setup) {
    let mut ps = std::mem::zeroed::<PAINTSTRUCT>();
    let hdc = BeginPaint(hwnd, &mut ps);
    let layout = layout_of(hwnd);
    let width = layout.client.width.round().max(1.0) as i32;
    let height = layout.client.height.round().max(1.0) as i32;

    // Fora do ecra e um BitBlt so no fim: a 30 quadros por segundo, desenhar
    // direto na janela pisca.
    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, width, height);
    let buffered = !mem_dc.is_null() && !mem_bmp.is_null();
    let target = if buffered { mem_dc } else { hdc };
    let old_bmp = if buffered {
        SelectObject(mem_dc, mem_bmp as _)
    } else {
        std::ptr::null_mut()
    };

    let seconds = state.clock.seconds;
    let tone = paint::tone_for(state.screen);
    paint::fill(target, layout.client, paint::PAGE);
    paint::tissue_background(target, &layout, seconds);

    paint::logo(target, layout.logo);

    let title_font = paint::font(-(30.0 * layout.scale) as i32, 600);
    let body_font = paint::font(-(15.0 * layout.scale) as i32, 400);
    let small_font = paint::font(-(13.0 * layout.scale) as i32, 400);

    paint::text(
        target,
        state.title(),
        layout.title,
        paint::FG,
        title_font,
        DT_CENTER | DT_SINGLELINE,
    );
    paint::text(
        target,
        &format!("versao {VERSION}"),
        layout.subtitle,
        paint::MUTED,
        small_font,
        DT_CENTER | DT_SINGLELINE,
    );
    paint::text(
        target,
        &state.message,
        layout.note,
        if state.screen == Screen::Failed || state.warning {
            paint::BAD
        } else {
            paint::MUTED
        },
        small_font,
        DT_CENTER | DT_WORDBREAK,
    );

    if state.screen != Screen::Welcome {
        paint::progress_bar(target, &layout, state.progress, seconds, tone);
        paint::text(
            target,
            state.stage.label(),
            layout.stage,
            paint::FG,
            body_font,
            DT_LEFT | DT_SINGLELINE,
        );
        paint::text(
            target,
            &format!("{:.0}%", state.progress * 100.0),
            layout.percent,
            tone,
            body_font,
            DT_RIGHT | DT_SINGLELINE,
        );
    }

    if state.screen == Screen::Welcome && state.mode == Mode::Install {
        paint::checkbox(
            target,
            layout.checkbox,
            state.desktop_shortcut,
            paint::hovered(state.hover, Hit::DesktopShortcut),
            layout.scale,
        );
        paint::text(
            target,
            "Criar atalho no ambiente de trabalho",
            Rect {
                y: layout.checkbox_label.y + layout.checkbox_label.height * 0.5
                    - 10.0 * layout.scale,
                ..layout.checkbox_label
            },
            paint::MUTED,
            small_font,
            DT_LEFT | DT_SINGLELINE,
        );
    }

    if state.screen != Screen::Working {
        paint::button(
            target,
            layout.primary,
            state.primary_label(),
            true,
            paint::hovered(state.hover, Hit::Primary),
            true,
            body_font,
            layout.scale,
        );
        if state.screen == Screen::Welcome {
            paint::button(
                target,
                layout.secondary,
                "Cancelar",
                false,
                paint::hovered(state.hover, Hit::Secondary),
                true,
                body_font,
                layout.scale,
            );
        }
    }

    paint::close_button(target, &layout, paint::hovered(state.hover, Hit::Close));

    DeleteObject(title_font as _);
    DeleteObject(body_font as _);
    DeleteObject(small_font as _);

    if buffered {
        BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);
        SelectObject(mem_dc, old_bmp);
        DeleteObject(mem_bmp as _);
        DeleteDC(mem_dc);
    } else if !mem_dc.is_null() {
        DeleteDC(mem_dc);
    }
    EndPaint(hwnd, &ps);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Uma pasta de ensaio com o seu proprio ramo de registo, que desaparece
    /// no fim do teste -- mesmo que ele falhe.
    struct Sandbox {
        dir: PathBuf,
        places: Places,
        registry: String,
    }

    impl Sandbox {
        fn new(tag: &str) -> Self {
            // Os atalhos passam pelo COM, como na thread de trabalho.
            winshell::init_com();
            let dir = std::env::temp_dir().join(format!(
                "neuralia-setup-sandbox-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("pasta de ensaio");
            let places = install::sandbox_places(&dir).expect("sitios de ensaio");
            let leaf = dir
                .file_name()
                .expect("nome")
                .to_string_lossy()
                .into_owned();
            let registry = format!("{}\\{leaf}", install::SANDBOX_REGISTRY_ROOT);
            winshell::delete_key(&registry);
            Self {
                dir,
                places,
                registry,
            }
        }

        fn target(&self, root: &Path) -> Target {
            Target {
                root: root.to_path_buf(),
                places: self.places.clone(),
                data_dir: self.dir.join("dados").join(install::PRODUCT),
                parking: self.dir.join("parking"),
            }
        }

        fn start_menu_link(&self) -> PathBuf {
            self.places
                .start_menu
                .clone()
                .expect("menu")
                .join(format!("{}.lnk", install::PRODUCT))
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            winshell::delete_key(&self.registry);
            winshell::delete_empty_key(install::SANDBOX_REGISTRY_ROOT);
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn plan(root: &Path, exe: &[u8]) -> Plan {
        Plan {
            root: root.to_path_buf(),
            entries: vec![
                Entry {
                    path: install::EXECUTABLE.into(),
                    data: exe.to_vec(),
                },
                Entry {
                    path: "resources/a/b.bin".into(),
                    data: vec![7; 64],
                },
            ],
            desktop_shortcut: false,
        }
    }

    /// O que o instalador Inno das 2.1.x deixou numa pasta: a NeuralIA, o
    /// `unins000.*` e a chave `{AppId}_is1`, com os valores que ele escreve.
    fn seed_inno(sandbox: &Sandbox, root: &Path) {
        fs::create_dir_all(root).expect("pasta do Inno");
        fs::write(root.join(install::EXECUTABLE), b"NeuralIA 2.1.5 do Inno").expect("exe");
        fs::write(root.join("unins000.exe"), b"MZ desinstalador do Inno").expect("unins");
        let mut log = b"Inno Setup Uninstall Log (b) 64-bit".to_vec();
        log.resize(64, 0);
        log.extend_from_slice(install::INNO_APP_ID.as_bytes());
        log.resize(1681, 0);
        fs::write(root.join("unins000.dat"), log).expect("dat");
        let key = sandbox.places.inno_key();
        winshell::write_string(&key, "DisplayName", "NeuralIA");
        winshell::write_string(&key, "DisplayVersion", "2.1.5");
        winshell::write_string(&key, "InstallLocation", &format!("{}\\", root.display()));
        winshell::write_string(
            &key,
            "UninstallString",
            &format!("\"{}\"", root.join("unins000.exe").display()),
        );
        // O atalho do menu Iniciar do Inno: o mesmo sitio que o nosso.
        winshell::create_shortcut(
            &sandbox.start_menu_link(),
            &root.join(install::EXECUTABLE),
            root,
            "NeuralIA",
            &root.join(install::EXECUTABLE),
        )
        .expect("atalho do Inno");
    }

    /// Os dados do dono: tem de estar exatamente assim no fim de tudo.
    fn seed_data(target: &Target) -> Vec<(PathBuf, Vec<u8>)> {
        let files = vec![
            (target.data_dir.join("history.jsonl"), b"historico".to_vec()),
            (target.data_dir.join("gemini-live.key"), b"chave".to_vec()),
            (
                target.data_dir.join("memory").join("memory.sqlite"),
                b"memoria".to_vec(),
            ),
            (
                target.data_dir.join("WebView2").join("Local State"),
                b"sessoes".to_vec(),
            ),
        ];
        for (path, data) in &files {
            fs::create_dir_all(path.parent().expect("pai")).expect("pasta dos dados");
            fs::write(path, data).expect("dados");
        }
        files
    }

    fn assert_data_untouched(files: &[(PathBuf, Vec<u8>)]) {
        for (path, data) in files {
            assert_eq!(
                fs::read(path).ok().as_deref(),
                Some(data.as_slice()),
                "os dados do dono mudaram: {}",
                path.display()
            );
        }
    }

    fn setup_exe(sandbox: &Sandbox) -> PathBuf {
        let me = sandbox.dir.join("NeuralIA-Setup.exe");
        fs::write(&me, b"MZ instalador novo").expect("instalador");
        me
    }

    #[test]
    fn upgrading_the_inno_install_leaves_one_neuralia_and_the_owners_data_as_it_was() {
        // A maquina do dono: a 2.1.5 do Inno na pasta de sempre. Depois da
        // atualizacao, "Aplicacoes" mostra UMA NeuralIA (a nossa entrada, com
        // a versao nova), o `unins000.*` desapareceu, o atalho do menu
        // Iniciar abre a NeuralIA nova e os dados nao mudaram um byte.
        let sandbox = Sandbox::new("inno");
        let root = sandbox.dir.join("Programs").join("Neural IA");
        seed_inno(&sandbox, &root);
        let target = sandbox.target(&root);
        let data = seed_data(&target);
        let me = setup_exe(&sandbox);
        let plan = plan(&root, b"NeuralIA nova");

        install_with(&plan, &target, &me, &Shared::new()).expect("atualizar");

        assert_eq!(
            fs::read(root.join(install::EXECUTABLE)).expect("exe"),
            b"NeuralIA nova"
        );
        assert_eq!(
            fs::read(root.join(install::UNINSTALLER)).expect("desinstalador"),
            b"MZ instalador novo"
        );
        for stale in ["unins000.exe", "unins000.dat"] {
            assert!(!root.join(stale).exists(), "{stale} do Inno ficou");
        }
        assert!(
            !winshell::key_exists(&sandbox.places.inno_key()),
            "a entrada do Inno ficou: Aplicacoes mostrava duas NeuralIA"
        );
        let key = sandbox.places.key();
        assert_eq!(
            winshell::read_string(&key, "DisplayName").as_deref(),
            Some("NeuralIA")
        );
        assert_eq!(
            winshell::read_string(&key, "DisplayVersion").as_deref(),
            Some(VERSION)
        );
        assert!(install::same_path(
            &winshell::registered_location(&key).expect("InstallLocation"),
            &root
        ));
        let link = winshell::shortcut_details(&sandbox.start_menu_link()).expect("atalho");
        assert!(install::same_path(
            &link.target,
            &root.join(install::EXECUTABLE)
        ));
        assert_data_untouched(&data);

        // E desinstalar a seguir tira tudo o que e nosso e nada do que e dele.
        uninstall_with(&plan, &target, &Shared::new()).expect("desinstalar");
        assert!(!root.exists(), "a pasta ficou: {}", root.display());
        assert!(!winshell::key_exists(&key));
        assert!(!sandbox.start_menu_link().exists());
        assert_data_untouched(&data);
    }

    #[test]
    fn installing_with_neuralia_open_changes_nothing_and_says_why() {
        use std::os::windows::fs::OpenOptionsExt;
        let sandbox = Sandbox::new("open");
        let root = sandbox.dir.join("Programs").join("NeuralIA");
        seed_inno(&sandbox, &root);
        let target = sandbox.target(&root);
        let me = setup_exe(&sandbox);
        let exe = root.join(install::EXECUTABLE);
        let hold = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&exe)
            .expect("a NeuralIA aberta");

        let result = install_with(&plan(&root, b"nova"), &target, &me, &Shared::new());
        drop(hold);

        let failure = result.expect_err("com a NeuralIA aberta nao se instala");
        assert_eq!(failure.kind, FailureKind::InUse);
        assert_eq!(install::exit_code(&Err(failure)), install::EXIT_IN_USE);
        // Nada mudou: nem meia instalacao, nem a entrada do Inno apagada.
        assert_eq!(fs::read(&exe).expect("exe"), b"NeuralIA 2.1.5 do Inno");
        assert!(root.join("unins000.exe").exists());
        assert!(winshell::key_exists(&sandbox.places.inno_key()));
        assert!(!winshell::key_exists(&sandbox.places.key()));
        assert!(!root.join(install::UNINSTALLER).exists());
    }

    #[test]
    fn installing_into_the_data_folder_is_refused_before_anything_is_written() {
        let sandbox = Sandbox::new("data");
        let probe = sandbox.target(&sandbox.dir);
        let data = seed_data(&probe);
        let target = sandbox.target(&probe.data_dir);
        let me = setup_exe(&sandbox);
        let result = install_with(
            &plan(&probe.data_dir, b"nova"),
            &target,
            &me,
            &Shared::new(),
        );
        let failure = result.expect_err("instalar dentro dos dados");
        assert_eq!(
            install::exit_code(&Err(failure)),
            install::EXIT_BAD_ARGUMENTS
        );
        assert!(!probe.data_dir.join(install::EXECUTABLE).exists());
        assert!(!winshell::key_exists(&sandbox.places.key()));
        assert_data_untouched(&data);
    }

    #[test]
    fn uninstalling_a_stale_copy_keeps_the_real_installs_entry_and_shortcut() {
        // Uma copia velha noutra pasta desinstala-se a si propria e so a si:
        // a entrada de "Aplicacoes" e o atalho sao da instalacao boa.
        let sandbox = Sandbox::new("stale");
        let good = sandbox.dir.join("Programs").join("NeuralIA");
        let stale = sandbox.dir.join("Velha").join("NeuralIA");
        let me = setup_exe(&sandbox);
        install_with(
            &plan(&good, b"boa"),
            &sandbox.target(&good),
            &me,
            &Shared::new(),
        )
        .expect("instalacao boa");
        let stale_plan = plan(&stale, b"velha");
        install::write_payload(&stale_plan, |_| {}).expect("copia velha");

        uninstall_with(&stale_plan, &sandbox.target(&stale), &Shared::new())
            .expect("desinstalar a velha");

        assert!(!stale.exists());
        assert!(
            winshell::key_exists(&sandbox.places.key()),
            "a entrada da instalacao boa foi apagada"
        );
        assert!(
            sandbox.start_menu_link().exists(),
            "o atalho da instalacao boa foi apagado"
        );
        assert!(good.join(install::EXECUTABLE).exists());
    }

    #[test]
    fn an_uninstall_that_cannot_delete_the_browser_fails_and_keeps_the_apps_entry() {
        use std::os::windows::fs::OpenOptionsExt;
        let sandbox = Sandbox::new("locked");
        let root = sandbox.dir.join("Programs").join(install::PRODUCT);
        let me = setup_exe(&sandbox);
        let plan = plan(&root, &[9; 2048]);
        let target = sandbox.target(&root);
        install_with(&plan, &target, &me, &Shared::new()).expect("instalar");
        let exe = plan.executable();
        // O navegador aberto: o executavel nao se deixa apagar.
        let hold = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&exe)
            .expect("segurar o executavel");

        let result = uninstall_with(&plan, &target, &Shared::new());
        drop(hold);

        assert!(
            matches!(&result, Err(failure) if failure.kind == FailureKind::InUse
                && failure.message.contains(install::EXECUTABLE)),
            "o NeuralIA.exe ficou no disco e a desinstalacao disse {result:?}"
        );
        assert!(exe.exists());
        assert!(
            root.join("resources/a/b.bin").exists(),
            "a desinstalacao apagou metade com a NeuralIA aberta"
        );
        assert!(
            winshell::key_exists(&sandbox.places.key()),
            "a entrada de Aplicacoes foi apagada com a NeuralIA ainda instalada"
        );
        assert!(
            sandbox.start_menu_link().exists(),
            "os atalhos foram apagados com a NeuralIA ainda instalada"
        );
    }

    #[test]
    fn uninstalling_from_the_install_folder_leaves_nothing_behind() {
        let sandbox = Sandbox::new("self");
        let root = sandbox.dir.join("Programs").join(install::PRODUCT);
        let plan = plan(&root, &[9; 2048]);
        let target = sandbox.target(&root);
        let me = setup_exe(&sandbox);
        install_with(&plan, &target, &me, &Shared::new()).expect("instalar");
        // O desinstalador que o Windows corre e o que esta dentro da pasta.
        // Um executavel a correr de verdade (nao um ficheiro aberto) e o que
        // o Windows recusa apagar.
        let uninstaller = plan.uninstaller();
        let system = std::env::var_os("SystemRoot").expect("SystemRoot");
        fs::copy(
            PathBuf::from(system).join("System32").join("sort.exe"),
            &uninstaller,
        )
        .expect("copiar um executavel");
        let mut running = std::process::Command::new(&uninstaller)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("correr o desinstalador");
        assert!(
            fs::remove_file(&uninstaller).is_err(),
            "o Windows devia recusar apagar um executavel a correr"
        );

        let result = uninstall_with(&plan, &target, &Shared::new());
        let root_left = root.exists();
        drop(running.stdin.take());
        let _ = running.kill();
        let _ = running.wait();

        assert_eq!(result, Ok(()));
        assert!(!winshell::key_exists(&sandbox.places.key()));
        assert!(
            !root_left,
            "a pasta de instalacao ficou para tras: {}",
            root.display()
        );
        assert!(!sandbox.start_menu_link().exists());
    }
}
