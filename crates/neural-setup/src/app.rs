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
use crate::install::{self, Plan, Stage};
use crate::paint;
use crate::ui::{Hit, Layout, Rect, Screen, WINDOW_H, WINDOW_W};
use crate::winshell;

/// A carga util que o `build.rs` empacotou.
const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
const VERSION: &str = env!("CARGO_PKG_VERSION");
const TIMER_ID: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Install,
    Uninstall,
}

/// O que a thread de trabalho vai dizendo a janela. Numeros atomicos em vez de
/// mensagens: a janela le quando desenha, e um quadro atrasado nao faz mal
/// nenhum -- o que fazia mal era bloquear o desenho a espera do disco.
struct Shared {
    /// Fase (`Stage as u8`) e fracao dentro dela, em milesimos.
    stage: AtomicU64,
    within: AtomicU64,
    outcome: Mutex<Option<Result<(), String>>>,
}

impl Shared {
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

struct Setup {
    mode: Mode,
    screen: Screen,
    hover: Option<Hit>,
    desktop_shortcut: bool,
    progress: f64,
    stage: Stage,
    message: String,
    started: Instant,
    shared: Arc<Shared>,
    entries: Vec<Entry>,
    root: PathBuf,
    /// Fica ligado quando o utilizador carrega em "Abrir a NeuralIA".
    launch: bool,
}

impl Setup {
    fn new(mode: Mode) -> Self {
        let entries = archive::unpack(PAYLOAD).unwrap_or_default();
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let root = install::install_root(&local);
        let (screen, message) = match (mode, entries.is_empty()) {
            (Mode::Install, true) => (
                Screen::Failed,
                "Este instalador foi construido sem a NeuralIA la dentro.".to_string(),
            ),
            (Mode::Install, false) => (Screen::Welcome, format!("Vai ficar em {}", root.display())),
            (Mode::Uninstall, _) => (Screen::Welcome, format!("Vai remover {}", root.display())),
        };
        Self {
            mode,
            screen,
            hover: None,
            desktop_shortcut: true,
            progress: 0.0,
            stage: Stage::Preparing,
            message,
            started: Instant::now(),
            shared: Arc::new(Shared {
                stage: AtomicU64::new(0),
                within: AtomicU64::new(0),
                outcome: Mutex::new(None),
            }),
            entries,
            root,
            launch: false,
        }
    }

    fn seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
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
        self.screen = Screen::Working;
        self.progress = 0.0;

        let plan = Plan {
            root: self.root.clone(),
            entries: self.entries.clone(),
            desktop_shortcut: self.desktop_shortcut,
        };
        let shared = Arc::clone(&self.shared);
        let mode = self.mode;
        std::thread::spawn(move || {
            winshell::init_com();
            let result = match mode {
                Mode::Install => do_install(&plan, &shared),
                Mode::Uninstall => do_uninstall(&plan, &shared),
            };
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
                self.message = match self.mode {
                    Mode::Install => format!("Instalada em {}", self.root.display()),
                    Mode::Uninstall => "A NeuralIA foi removida deste computador.".to_string(),
                };
            }
            Some(Err(why)) => {
                self.screen = Screen::Failed;
                self.message = why;
            }
            None => {}
        }
    }
}

fn do_install(plan: &Plan, shared: &Shared) -> Result<(), String> {
    shared.report(Stage::Preparing, 0.0);
    install::write_payload(plan, |fraction| shared.report(Stage::Writing, fraction))
        .map_err(|e| e.to_string())?;

    shared.report(Stage::Shortcuts, 0.0);
    let exe = plan.executable();
    let icon = exe.clone();
    if let Some(programs) = winshell::start_menu_programs() {
        winshell::create_shortcut(
            &programs.join(format!("{}.lnk", install::PRODUCT)),
            &exe,
            &plan.root,
            "NeuralIA",
            &icon,
        )?;
    }
    shared.report(Stage::Shortcuts, 0.5);
    if plan.desktop_shortcut
        && let Some(desktop) = winshell::desktop()
    {
        winshell::create_shortcut(
            &desktop.join(format!("{}.lnk", install::PRODUCT)),
            &exe,
            &plan.root,
            "NeuralIA",
            &icon,
        )?;
    }

    // O desinstalador e este mesmo programa, guardado ao lado da aplicacao.
    shared.report(Stage::Registering, 0.0);
    let uninstaller = plan.root.join(install::UNINSTALLER);
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::remove_file(&uninstaller);
        std::fs::copy(&me, &uninstaller).map_err(|e| format!("copiar o desinstalador: {e}"))?;
    }
    let size_kb = (plan.total_bytes() / 1024).max(1) as u32;
    winshell::register_uninstall(&plan.root, &uninstaller, &exe, VERSION, size_kb)?;
    Ok(())
}

fn do_uninstall(plan: &Plan, shared: &Shared) -> Result<(), String> {
    let shortcut_folders: Vec<PathBuf> = [winshell::start_menu_programs(), winshell::desktop()]
        .into_iter()
        .flatten()
        .collect();
    uninstall_with(
        plan,
        shared,
        &std::env::temp_dir(),
        &shortcut_folders,
        winshell::unregister_uninstall,
    )
}

/// A desinstalacao, com o que toca no sistema (as pastas dos atalhos e a
/// entrada do registo) passado de fora -- e assim que os testes a correm sem
/// apagar a instalacao verdadeira de quem os corre.
fn uninstall_with(
    plan: &Plan,
    shared: &Shared,
    parking: &Path,
    shortcut_folders: &[PathBuf],
    unregister: impl FnOnce(),
) -> Result<(), String> {
    shared.report(Stage::Preparing, 0.0);
    // Se algum ficheiro ficou, para aqui: os atalhos e a entrada em
    // Aplicacoes continuam, para a NeuralIA que ficou poder ser aberta e
    // desinstalada outra vez depois de fechada.
    install::remove_installed(&plan.root, &plan.entries, parking, |fraction| {
        shared.report(Stage::Writing, fraction)
    })
    .map_err(|left| {
        let first = left
            .first()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!(
            "Feche a NeuralIA e tente outra vez. {} ficheiro(s) em uso nao sairam, a comecar por {first}.",
            left.len()
        )
    })?;

    shared.report(Stage::Shortcuts, 0.0);
    for folder in shortcut_folders {
        let _ = std::fs::remove_file(folder.join(format!("{}.lnk", install::PRODUCT)));
    }

    shared.report(Stage::Registering, 0.0);
    unregister();
    Ok(())
}

pub fn run(mode: Mode) {
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
        let state = Box::into_raw(Box::new(Setup::new(mode)));
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
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
        SetTimer(hwnd, TIMER_ID, 33, None);
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
                state.tick();
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);
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
                        state.launch = true;
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
                if state.launch {
                    let _ = std::process::Command::new(state.root.join(install::EXECUTABLE))
                        .current_dir(&state.root)
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

    let seconds = state.seconds();
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
        if state.screen == Screen::Failed {
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
    use std::cell::Cell;
    use std::fs;

    fn shared() -> Shared {
        Shared {
            stage: AtomicU64::new(0),
            within: AtomicU64::new(0),
            outcome: Mutex::new(None),
        }
    }

    /// Uma instalacao de verdade numa pasta temporaria: a carga util, o
    /// desinstalador ao lado, e um atalho numa "pasta de atalhos" falsa.
    fn installed(tag: &str) -> (PathBuf, Plan, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("neuralia-uninst-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("Programs").join(install::PRODUCT);
        let plan = Plan {
            root: root.clone(),
            entries: vec![
                Entry {
                    path: install::EXECUTABLE.into(),
                    data: vec![9; 2048],
                },
                Entry {
                    path: "resources/a/b.bin".into(),
                    data: vec![7; 64],
                },
            ],
            desktop_shortcut: false,
        };
        install::write_payload(&plan, |_| {}).expect("instalar");
        fs::write(root.join(install::UNINSTALLER), b"desinstalador").expect("desinstalador");
        let shortcuts = base.join("atalhos");
        fs::create_dir_all(&shortcuts).expect("atalhos");
        fs::write(shortcuts.join(format!("{}.lnk", install::PRODUCT)), b"lnk").expect("lnk");
        (base, plan, shortcuts)
    }

    #[test]
    fn an_uninstall_that_cannot_delete_the_browser_fails_and_keeps_the_apps_entry() {
        use std::os::windows::fs::OpenOptionsExt;
        let (base, plan, shortcuts) = installed("locked");
        let exe = plan.executable();
        // O navegador aberto: o executavel nao se deixa apagar.
        let hold = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&exe)
            .expect("segurar o executavel");

        let unregistered = Cell::new(false);
        let result = uninstall_with(
            &plan,
            &shared(),
            &base.join("parking"),
            std::slice::from_ref(&shortcuts),
            || unregistered.set(true),
        );
        drop(hold);

        assert!(
            matches!(&result, Err(why) if why.contains(install::EXECUTABLE)),
            "o NeuralIA.exe ficou no disco e a desinstalacao disse {result:?}"
        );
        assert!(exe.exists());
        assert!(
            !unregistered.get(),
            "a entrada de Aplicacoes foi apagada com a NeuralIA ainda instalada"
        );
        assert!(
            shortcuts.join(format!("{}.lnk", install::PRODUCT)).exists(),
            "os atalhos foram apagados com a NeuralIA ainda instalada"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn uninstalling_from_the_install_folder_leaves_nothing_behind() {
        let (base, plan, shortcuts) = installed("self");
        // O desinstalador que o Windows corre e o que esta dentro da pasta.
        // Um executavel a correr de verdade (nao um ficheiro aberto) e o que
        // o Windows recusa apagar.
        let uninstaller = plan.root.join(install::UNINSTALLER);
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

        let parking = base.join("parking");
        let unregistered = Cell::new(false);
        let result = uninstall_with(
            &plan,
            &shared(),
            &parking,
            std::slice::from_ref(&shortcuts),
            || unregistered.set(true),
        );
        let root_left = plan.root.exists();
        drop(running.stdin.take());
        let _ = running.kill();
        let _ = running.wait();

        assert_eq!(result, Ok(()));
        assert!(unregistered.get());
        assert!(
            !root_left,
            "a pasta de instalacao ficou para tras: {}",
            plan.root.display()
        );
        assert!(!shortcuts.join(format!("{}.lnk", install::PRODUCT)).exists());
        let _ = fs::remove_dir_all(&base);
    }
}
