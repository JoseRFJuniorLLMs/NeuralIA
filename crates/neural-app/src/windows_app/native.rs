use super::*;

pub(in crate::windows_app) const EM_SETSEL: u32 = 0x00B1;
pub(in crate::windows_app) const EM_SETLIMITTEXT: u32 = 0x00C5;
pub(in crate::windows_app) const EM_SETCUEBANNER: u32 = 0x1501;
pub(in crate::windows_app) const EM_SETMARGINS: u32 = 0x00D3;
pub(in crate::windows_app) const WM_CTLCOLOREDIT: u32 = 0x0133;
pub(in crate::windows_app) const WM_ERASEBKGND: u32 = 0x0014;
pub(in crate::windows_app) const WM_KILLFOCUS: u32 = 0x0008;
pub(in crate::windows_app) const WM_CHAR: u32 = 0x0102;

/// Instante de arranque, para termos milissegundos monotonos num AtomicU64.
pub(in crate::windows_app) static START: OnceLock<Instant> = OnceLock::new();

pub(in crate::windows_app) fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Construir um WebView faz correr um ciclo de mensagens ANINHADO dentro do
/// nosso proprio callback (wry chama `wait_with_pump`). Enquanto isso dura, o
/// winit nao entrega `RedrawRequested` -- so revalida e reinvalida a janela em
/// ciclo -- por isso o ecra fica com os pixeis das janelas que acabamos de
/// destruir. Este sinalizador deixa o `WM_ERASEBKGND` apagar o fundo mesmo
/// nessas voltas, que e o unico ponto de pintura que ainda corre.
pub(in crate::windows_app) static ERASE_PENDING: AtomicBool = AtomicBool::new(false);
pub(in crate::windows_app) const WINDOW_SUBCLASS_ID: usize = 0x4E4A;
/// Mensagens privadas usadas somente pelo gate de lifecycle. Usamos
/// RegisterWindowMessageW em vez de IDs fixos em WM_APP para não colidir com
/// mensagens privadas do winit/WRY/WebView2. O script registra os mesmos nomes,
/// então Windows resolve os dois processos para os mesmos IDs de mensagem.
pub(in crate::windows_app) static LIFECYCLE_PROBE_HOME_MESSAGE: OnceLock<u32> = OnceLock::new();
pub(in crate::windows_app) static LIFECYCLE_PROBE_REOPEN_MESSAGE: OnceLock<u32> = OnceLock::new();
pub(in crate::windows_app) static LIFECYCLE_PROBE_READY_MESSAGE: OnceLock<u32> = OnceLock::new();
pub(in crate::windows_app) static LIFECYCLE_PROBE_HOME_READY_MESSAGE: OnceLock<u32> =
    OnceLock::new();
pub(in crate::windows_app) static LIFECYCLE_COMPARATOR_READY: AtomicBool = AtomicBool::new(false);
pub(in crate::windows_app) static LIFECYCLE_HOME_READY: AtomicBool = AtomicBool::new(false);
/// O probe transmite comandos a todas as janelas do processo porque o HWND
/// principal pode mudar com decorations. O nonce impede que o mesmo comando,
/// recebido por um HWND antigo e pelo atual, gere eventos duplicados.
pub(in crate::windows_app) static LIFECYCLE_LAST_HOME_NONCE: AtomicUsize = AtomicUsize::new(0);
pub(in crate::windows_app) static LIFECYCLE_LAST_REOPEN_NONCE: AtomicUsize = AtomicUsize::new(0);

pub(in crate::windows_app) fn lifecycle_probe_home_message() -> u32 {
    *LIFECYCLE_PROBE_HOME_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.Home"))
    })
}

pub(in crate::windows_app) fn lifecycle_probe_reopen_message() -> u32 {
    *LIFECYCLE_PROBE_REOPEN_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.Reopen"))
    })
}

pub(in crate::windows_app) fn lifecycle_probe_ready_message() -> u32 {
    *LIFECYCLE_PROBE_READY_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.Ready"))
    })
}

pub(in crate::windows_app) fn lifecycle_probe_home_ready_message() -> u32 {
    *LIFECYCLE_PROBE_HOME_READY_MESSAGE.get_or_init(|| unsafe {
        RegisterWindowMessageW(windows_sys::w!("NeuralIA.LifecycleProbe.HomeReady"))
    })
}
pub(in crate::windows_app) const EXIT_BUTTON_SUBCLASS_ID: usize = 0x4E4B;
pub(in crate::windows_app) const HOME_BUTTON_SUBCLASS_ID: usize = 0x4E4C;
pub(in crate::windows_app) const CAPTION_BUTTONS_SUBCLASS_ID: usize = 0x4E70;
pub(in crate::windows_app) const WM_PAINT: u32 = 0x000F;
pub(in crate::windows_app) const WM_LBUTTONUP: u32 = 0x0202;
pub(in crate::windows_app) const WM_NCHITTEST: u32 = 0x0084;
pub(in crate::windows_app) const HTCLIENT: u32 = 1;
pub(in crate::windows_app) const WM_MOUSEACTIVATE: u32 = 0x0021;
pub(in crate::windows_app) const MA_NOACTIVATE: u32 = 3;
/// Botao flutuante de saida, em pixeis logicos. Fica centrado no topo: nos
/// cantos chocava com a propria interface dos sites (o login do Google estava
/// exatamente por baixo dele).
pub(in crate::windows_app) const EXIT_BUTTON_WIDTH: f64 = 196.0;
pub(in crate::windows_app) const EXIT_BUTTON_HEIGHT: f64 = 38.0;
pub(in crate::windows_app) const EC_LEFTMARGIN: usize = 0x0001;
pub(in crate::windows_app) const EC_RIGHTMARGIN: usize = 0x0002;
pub(in crate::windows_app) const WM_SETFONT: u32 = 0x0030;
pub(in crate::windows_app) const OMNIBOX_SUBCLASS_ID: usize = 0x4E49;
pub(in crate::windows_app) const TAB_MENU_OPEN: usize = 1;
pub(in crate::windows_app) const TAB_MENU_FULLSCREEN: usize = 2;
pub(in crate::windows_app) const TAB_MENU_CLOSE: usize = 3;
pub(in crate::windows_app) const TAB_MENU_CLOSE_OTHERS: usize = 4;
pub(in crate::windows_app) const TAB_MENU_CLOSE_ALL: usize = 5;
pub(in crate::windows_app) const TAB_MENU_NEW_GROUP: usize = 6;
pub(in crate::windows_app) const TAB_MENU_UNGROUP: usize = 7;
/// Os grupos ja existentes ocupam ids a partir daqui, um por grupo da coluna.
pub(in crate::windows_app) const TAB_MENU_GROUP_BASE: usize = 100;
pub(in crate::windows_app) const SPLITTER_SUBCLASS_BASE: usize = 0x4E60;
pub(in crate::windows_app) const PANEL_HANDLE_SUBCLASS_ID: usize = 0x4E74;
pub(in crate::windows_app) const WM_SETCURSOR: u32 = 0x0020;

/// Ultima posicao (x de ecra) pedida pelo arrasto da pega do painel, e se ha
/// um pedido por atender -- o mesmo esquema do divisor das colunas.
pub(in crate::windows_app) static PANEL_RESIZE_X: AtomicI32 = AtomicI32::new(0);
pub(in crate::windows_app) static PANEL_RESIZE_PENDING: AtomicBool = AtomicBool::new(false);
/// A pega ja agendou a sua dica nesta passagem do rato.
pub(in crate::windows_app) static PANEL_HANDLE_HINT: AtomicBool = AtomicBool::new(false);

// A roda do rato sobre o painel da direita. O Windows entrega a roda da
// janela ativa a janela com o FOCO do teclado: com uma coluna das IAs focada,
// girar a roda por cima do painel (historico, YouTube...) rolava a coluna --
// o Chromium rola a pagina dele mesmo com o ponto fora dela. O gancho de rato
// de baixo nivel so existe enquanto um painel da direita esta a vista, so age
// com o NeuralIA em primeiro plano e com o cursor dentro do painel
// (`panel_chrome::wheel_route`), e entrega o giro a janela do painel debaixo
// do cursor. Tudo o resto passa intocado.
pub(in crate::windows_app) static WHEEL_HOOK: AtomicUsize = AtomicUsize::new(0);
pub(in crate::windows_app) static WHEEL_APP_HWND: AtomicUsize = AtomicUsize::new(0);
pub(in crate::windows_app) static WHEEL_PANEL_HOST: AtomicUsize = AtomicUsize::new(0);
pub(in crate::windows_app) static WHEEL_PANEL_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(in crate::windows_app) static WHEEL_PANEL_LEFT: AtomicI32 = AtomicI32::new(0);
pub(in crate::windows_app) static WHEEL_PANEL_TOP: AtomicI32 = AtomicI32::new(0);
pub(in crate::windows_app) static WHEEL_PANEL_RIGHT: AtomicI32 = AtomicI32::new(0);
pub(in crate::windows_app) static WHEEL_PANEL_BOTTOM: AtomicI32 = AtomicI32::new(0);

pub(in crate::windows_app) fn wheel_panel_rect() -> Option<ScreenRect> {
    WHEEL_PANEL_ACTIVE
        .load(Ordering::Acquire)
        .then(|| ScreenRect {
            left: WHEEL_PANEL_LEFT.load(Ordering::Acquire),
            top: WHEEL_PANEL_TOP.load(Ordering::Acquire),
            right: WHEEL_PANEL_RIGHT.load(Ordering::Acquire),
            bottom: WHEEL_PANEL_BOTTOM.load(Ordering::Acquire),
        })
}

/// Teclas e botoes em baixo, no formato MK_* da palavra baixa do wParam.
pub(in crate::windows_app) fn wheel_key_state() -> u16 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_XBUTTON1, VK_XBUTTON2,
    };
    let down = |key: u16| unsafe { GetAsyncKeyState(key as i32) } < 0;
    [
        (VK_LBUTTON, 0x0001),
        (VK_RBUTTON, 0x0002),
        (VK_SHIFT, 0x0004),
        (VK_CONTROL, 0x0008),
        (VK_MBUTTON, 0x0010),
        (VK_XBUTTON1, 0x0020),
        (VK_XBUTTON2, 0x0040),
    ]
    .into_iter()
    .filter(|(key, _)| down(*key))
    .fold(0, |keys, (_, flag)| keys | flag)
}

/// A janela visivel mais funda de `host` debaixo do ponto de ecra: a do
/// WebView2 do painel (o Chrome_*), nao o contentor do wry.
pub(in crate::windows_app) fn panel_window_at(host: HWND, point: POINT) -> Option<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, ChildWindowFromPointEx, IsWindow,
    };
    if host.is_null() || unsafe { IsWindow(host) } == 0 {
        return None;
    }
    let mut current = host;
    for _ in 0..16 {
        let mut local = point;
        unsafe {
            ScreenToClient(current, &mut local);
        }
        let child = unsafe {
            ChildWindowFromPointEx(
                current,
                local,
                CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
            )
        };
        if child.is_null() || child == current {
            break;
        }
        current = child;
    }
    Some(current)
}

/// A janela `hit` (a que o Windows ve debaixo do cursor) e o contentor do
/// painel ou uma descendente dele -- e nao uma janela por cima do painel: um
/// popup do Chromium (lista de um <select>, menu de contexto), o seletor de
/// emojis, o historico da area de transferencia, outra aplicacao.
pub(in crate::windows_app) fn wheel_hit_in_panel(host: HWND, hit: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsChild;
    !host.is_null() && !hit.is_null() && (hit == host || unsafe { IsChild(host, hit) } != 0)
}

pub(in crate::windows_app) unsafe extern "system" fn wheel_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, MSLLHOOKSTRUCT, PostMessageW, WM_MOUSEHWHEEL, WM_MOUSEWHEEL,
        WindowFromPoint,
    };
    let message = wparam as u32;
    if code == HC_ACTION as i32
        && (message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL)
        && lparam != 0
    {
        let info = &*(lparam as *const MSLLHOOKSTRUCT);
        let app = WHEEL_APP_HWND.load(Ordering::Acquire) as HWND;
        let foreground = !app.is_null() && GetForegroundWindow() == app;
        let host = WHEEL_PANEL_HOST.load(Ordering::Acquire) as HWND;
        let panel = wheel_panel_rect();
        // So se pergunta ao Windows quem esta debaixo do cursor quando o
        // resto ja apontava para o painel: fora dele o gancho nao gasta nada.
        let hit_is_panel = foreground
            && panel.is_some_and(|rect| rect.contains(info.pt.x, info.pt.y))
            && wheel_hit_in_panel(host, WindowFromPoint(info.pt));
        if wheel_route((info.pt.x, info.pt.y), panel, foreground, hit_is_panel) == WheelRoute::Panel
            && let Some(target) = panel_window_at(host, info.pt)
        {
            let delta = (info.mouseData >> 16) as u16 as i16;
            let (w, l) = wheel_message_params(delta, wheel_key_state(), info.pt.x, info.pt.y);
            // PostMessage e nao SendMessage: o gancho nunca espera pelo
            // processo do WebView2.
            if PostMessageW(target, message, w, l) != 0 {
                return 1;
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

pub(in crate::windows_app) fn install_wheel_hook() {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_MOUSE_LL};
    if WHEEL_HOOK.load(Ordering::Acquire) != 0 {
        return;
    }
    unsafe {
        let hook = SetWindowsHookExW(
            WH_MOUSE_LL,
            Some(wheel_hook),
            GetModuleHandleW(std::ptr::null()),
            0,
        );
        if !hook.is_null() {
            WHEEL_HOOK.store(hook as usize, Ordering::Release);
        }
    }
}

pub(in crate::windows_app) fn uninstall_wheel_hook() {
    use windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx;
    let hook = WHEEL_HOOK.swap(0, Ordering::AcqRel);
    if hook != 0 {
        unsafe {
            UnhookWindowsHookEx(hook as _);
        }
    }
}

/// A pega de arrastar a borda do painel da direita. Mesma mecanica do divisor
/// das colunas: SetCapture no premir, posicao mais recente num static, um so
/// pedido em fila; o largar grava.
pub(in crate::windows_app) unsafe extern "system" fn panel_handle_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDC_SIZEWE, LoadCursorW, SetCursor};
    let send = |event: UserEvent| {
        if reference_data != 0 {
            let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
            proxy.send_event(event).is_ok()
        } else {
            false
        }
    };
    match message {
        // STATIC devolve HTTRANSPARENT: sem isto o rato ia para o WebView.
        WM_NCHITTEST => return HTCLIENT as LRESULT,
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_SIZEWE));
            return 1;
        }
        WM_LBUTTONDOWN => {
            hover_tooltip(hwnd, "");
            SetCapture(hwnd);
            return 0;
        }
        WM_MOUSEMOVE => {
            if GetCapture() == hwnd {
                let mut point = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut point) != 0 {
                    PANEL_RESIZE_X.store(point.x, Ordering::Release);
                    if !PANEL_RESIZE_PENDING.swap(true, Ordering::AcqRel)
                        && !send(UserEvent::ResizePanel)
                    {
                        PANEL_RESIZE_PENDING.store(false, Ordering::Release);
                    }
                }
            } else if !PANEL_HANDLE_HINT.swap(true, Ordering::AcqRel) {
                track_mouse_leave(hwnd);
                hover_tooltip(hwnd, "Arraste para alargar ou estreitar o painel");
            }
            return 0;
        }
        WM_MOUSELEAVE => {
            PANEL_HANDLE_HINT.store(false, Ordering::Release);
            hover_tooltip(hwnd, "");
        }
        WM_LBUTTONUP => {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            return 0;
        }
        // Fim do arrasto (largar, ou o rato levado por outra janela): grava.
        WM_CAPTURECHANGED => {
            send(UserEvent::PanelResizeDone);
            return 0;
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0 {
                    let theme = Theme::system();
                    let bg = CreateSolidBrush(rgb3(theme.bar_bg));
                    FillRect(hdc, &client, bg);
                    DeleteObject(bg as _);
                    let center = (client.right - client.left) / 2;
                    let line = RECT {
                        left: center,
                        top: 0,
                        right: center + 1,
                        bottom: client.bottom,
                    };
                    let brush = CreateSolidBrush(rgb3(theme.surface_line));
                    FillRect(hdc, &line, brush);
                    DeleteObject(brush as _);
                }
                EndPaint(hwnd, &paint);
            }
            return 0;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}
pub(in crate::windows_app) const SPLITTER_WIDTH: f64 = 7.0;
pub(in crate::windows_app) const MIN_PANEL_WIDTH: f64 = 180.0;

/// Ultima posicao pedida pelo arrasto de um divisor, e se ja ha um pedido por
/// atender. O rato manda WM_MOUSEMOVE a mais de 100 Hz e cada um reposiciona
/// tres WebView2: enfileirar um evento por movimento enche a fila de pedidos
/// que nascem velhos e o divisor fica a arrastar-se atras do cursor. Em vez
/// disso a subclasse escreve SEMPRE aqui a posicao mais recente e so acorda o
/// event loop quando nao ha nenhum pedido pendente -- o que chega ao handler
/// e o estado de agora, nao o de ha dez eventos.
pub(in crate::windows_app) static RESIZE_DIVIDER: AtomicUsize = AtomicUsize::new(0);
pub(in crate::windows_app) static RESIZE_X: AtomicI32 = AtomicI32::new(0);
pub(in crate::windows_app) static RESIZE_PENDING: AtomicBool = AtomicBool::new(false);
pub(in crate::windows_app) const WM_LBUTTONDOWN: u32 = 0x0201;
pub(in crate::windows_app) const WM_MOUSEMOVE: u32 = 0x0200;
/// Chega depois de `track_mouse_leave`: o rato saiu de um botao nativo.
pub(in crate::windows_app) const WM_MOUSELEAVE: u32 = 0x02A3;
pub(in crate::windows_app) const WM_RBUTTONUP: u32 = 0x0205;

/// Consulta opcional para automacao/benchmarks. Em producao a Home abre em
/// repouso e nao envia texto a nenhum fornecedor sem acao do utilizador.
pub(in crate::windows_app) fn startup_input() -> String {
    if std::env::var_os("NEURALIA_NO_STARTUP").is_some() {
        return String::new();
    }
    std::env::var("NEURALIA_STARTUP_INPUT")
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub(in crate::windows_app) fn lifecycle_probe_enabled() -> bool {
    std::env::var_os("NEURALIA_LIFECYCLE_PROBE").is_some()
}

#[link(name = "comctl32")]
unsafe extern "system" {
    pub(in crate::windows_app) fn SetWindowSubclass(
        hwnd: HWND,
        callback: Option<
            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT,
        >,
        subclass_id: usize,
        reference_data: usize,
    ) -> i32;
    pub(in crate::windows_app) fn DefSubclassProc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT;
}

#[link(name = "user32")]
unsafe extern "system" {
    pub(in crate::windows_app) fn GetCapture() -> HWND;
    pub(in crate::windows_app) fn SetCapture(hwnd: HWND) -> HWND;
    pub(in crate::windows_app) fn ReleaseCapture() -> i32;
    pub(in crate::windows_app) fn RegisterWindowMessageW(lp_string: *const u16) -> u32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "SystemFunction036"]
    pub(in crate::windows_app) fn rtl_gen_random(buffer: *mut core::ffi::c_void, length: u32)
    -> u8;
}

/// Pincel de fundo da omnibox, um por cor. Criar um a cada WM_CTLCOLOREDIT
/// vazaria objetos GDI a cada repintura.
pub(in crate::windows_app) static OMNIBOX_BRUSH: Mutex<Option<(Rgb, usize)>> = Mutex::new(None);

pub(in crate::windows_app) fn omnibox_brush(color: Rgb) -> *mut core::ffi::c_void {
    let mut slot = OMNIBOX_BRUSH.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((cached, handle)) = *slot
        && cached == color
    {
        return handle as *mut core::ffi::c_void;
    }
    unsafe {
        let brush = CreateSolidBrush(rgb3(color));
        if let Some((_, previous)) = slot.replace((color, brush as usize)) {
            DeleteObject(previous as _);
        }
        brush
    }
}

/// O EDIT nativo nao tem cantos redondos nem cor de fundo propria. Pintamos a
/// pilula suavizada por tras dele e respondemos aqui com a mesma cor, para o
/// retangulo do controlo desaparecer dentro dela.
pub(in crate::windows_app) unsafe extern "system" fn window_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    let lifecycle_home = lifecycle_probe_home_message();
    let lifecycle_reopen = lifecycle_probe_reopen_message();
    let lifecycle_ready = lifecycle_probe_ready_message();
    let lifecycle_home_ready = lifecycle_probe_home_ready_message();
    if message == lifecycle_home_ready {
        return if lifecycle_probe_enabled() && LIFECYCLE_HOME_READY.load(Ordering::Acquire) {
            1
        } else {
            0
        };
    }
    if message == lifecycle_ready {
        return if lifecycle_probe_enabled() && LIFECYCLE_COMPARATOR_READY.load(Ordering::Acquire) {
            1
        } else {
            0
        };
    }
    if message == lifecycle_home || message == lifecycle_reopen {
        if lifecycle_probe_enabled() && reference_data != 0 {
            let nonce = wparam;
            let seen = if message == lifecycle_home {
                &LIFECYCLE_LAST_HOME_NONCE
            } else {
                &LIFECYCLE_LAST_REOPEN_NONCE
            };

            // O script faz broadcast process-wide por desenho: decorations pode
            // deixar mais de um HWND transitório vivo. Um único comando lógico
            // não pode virar duas Homes/SubmitText. Sem este filtro,
            // um Reopen atrasado podia chegar depois da Home do ciclo seguinte
            // e recriar exactamente as três superfícies que o gate acabara de
            // derrubar.
            if nonce == 0 || seen.swap(nonce, Ordering::AcqRel) != nonce {
                let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                if message == lifecycle_home {
                    let _ = proxy.send_event(UserEvent::LifecycleProbeHome);
                } else {
                    let input = startup_input();
                    if !input.is_empty() {
                        let _ = proxy.send_event(UserEvent::SubmitText(input));
                    }
                }
            }
        }
        return 0;
    }

    // O rato foi para outra janela a meio de um gesto na fila de abas. O
    // aviso vai pela fila de eventos: o WM_CAPTURECHANGED do ReleaseCapture
    // normal chega antes do largar, e o App tem de ver o largar primeiro. A
    // mensagem segue para o winit, que tambem conta com ela.
    if message == WM_CAPTURECHANGED
        && reference_data != 0
        && let Some(gesture) = tab_gesture_capture_lost(&TAB_GESTURE_LIVE, hwnd, lparam as HWND)
    {
        let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
        let _ = proxy.send_event(UserEvent::TabCaptureLost(gesture));
    }

    if message == WM_ERASEBKGND {
        if ERASE_PENDING.swap(false, Ordering::SeqCst) {
            let hdc = wparam as *mut core::ffi::c_void;
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let brush = CreateSolidBrush(rgb3(Theme::system().page_bg));
                FillRect(hdc, &client, brush);
                DeleteObject(brush as _);
            }
        }
        // Damos sempre a mensagem por tratada: fora das transicoes nao ha nada
        // a apagar, e apagar a cada repintura faria a barra piscar.
        return 1;
    }

    if message == WM_CTLCOLOREDIT {
        let theme = Theme::system();
        let hdc = wparam as *mut core::ffi::c_void;
        SetTextColor(hdc, rgb3(theme.fg));
        SetBkColor(hdc, rgb3(theme.surface));
        return omnibox_brush(theme.surface) as LRESULT;
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

/// Pisca o botao da janela na barra de tarefas ate ela voltar a frente. Nao
/// ativa nada nem rouba o foco (e so o aviso que o Windows da a qualquer
/// app); com o som desligado no Windows, e o que resta de um fim de fase
/// com a janela minimizada.
pub(in crate::windows_app) fn flash_taskbar(owner: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx,
    };
    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd: owner,
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    unsafe {
        FlashWindowEx(&info);
    }
}

pub(in crate::windows_app) fn window_hwnd(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}
