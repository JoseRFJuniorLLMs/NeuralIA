use super::*;

// ===================== tema do sistema (cor de destaque + claro/escuro) =====================

pub(super) type Rgb = (u8, u8, u8);

/// Le um DWORD do HKEY_CURRENT_USER; None se a chave nao existir.
pub(super) fn registry_dword(subkey: &str, value: &str) -> Option<u32> {
    let subkey = wide_null(subkey);
    let value = wide_null(value);
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut core::ffi::c_void,
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

/// Cor de destaque escolhida em Definicoes > Personalizacao > Cores.
/// O Windows guarda-a como 0xAABBGGRR.
pub(super) fn system_accent() -> Rgb {
    match registry_dword("Software\\Microsoft\\Windows\\DWM", "AccentColor") {
        Some(value) => (
            (value & 0xFF) as u8,
            ((value >> 8) & 0xFF) as u8,
            ((value >> 16) & 0xFF) as u8,
        ),
        None => (0, 120, 212),
    }
}

/// O tema que o utilizador escolheu: acompanhar o Windows (o padrao) ou
/// forcar claro ou escuro. Guarda-se em `<data_dir>/theme`, uma palavra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThemeChoice {
    System,
    Light,
    Dark,
}

/// Indice em `ThemeChoice::ALL` da escolha em vigor.
pub(super) static THEME_CHOICE: AtomicUsize = AtomicUsize::new(0);

impl ThemeChoice {
    pub(super) const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::System => "Tema do sistema",
            Self::Light => "Tema claro",
            Self::Dark => "Tema escuro",
        }
    }

    pub(super) fn word(self) -> &'static str {
        match self {
            Self::System => "sistema",
            Self::Light => "claro",
            Self::Dark => "escuro",
        }
    }

    pub(super) fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "sistema" | "system" | "auto" => Some(Self::System),
            "claro" | "light" => Some(Self::Light),
            "escuro" | "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub(super) fn current() -> Self {
        Self::ALL
            .get(THEME_CHOICE.load(Ordering::Acquire))
            .copied()
            .unwrap_or(Self::System)
    }

    /// Escuro ou claro, dado o que o Windows diz agora.
    pub(super) fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Self::System => system_dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }

    /// O `prefers-color-scheme` das paginas no WebView2.
    pub(super) fn webview_theme(self) -> wry::Theme {
        match self {
            Self::System => wry::Theme::Auto,
            Self::Light => wry::Theme::Light,
            Self::Dark => wry::Theme::Dark,
        }
    }

    /// Sem ficheiro, ou com lixo dentro, vale o padrao: acompanhar o Windows.
    pub(super) fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| Self::parse(&text))
            .unwrap_or(Self::System)
    }

    /// Escreve num temporario ao lado e renomeia: um arranque a meio de uma
    /// escrita nunca le meia palavra.
    pub(super) fn save(self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, self.word())?;
        std::fs::rename(&temp, path)
    }

    /// Passa a valer ja: a proxima leitura do tema usa esta escolha.
    pub(super) fn apply(self) {
        let index = Self::ALL
            .iter()
            .position(|choice| *choice == self)
            .unwrap_or(0);
        THEME_CHOICE.store(index, Ordering::Release);
        Theme::invalidate();
    }
}

/// Um WebViewBuilder que ja nasce com o tema escolhido nas paginas.
pub(super) fn themed_webview_builder<'a>() -> WebViewBuilder<'a> {
    use wry::WebViewBuilderExtWindows;
    WebViewBuilder::new().with_theme(ThemeChoice::current().webview_theme())
}

pub(super) fn system_dark_mode() -> bool {
    registry_dword(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    )
    .map(|value| value == 0)
    .unwrap_or(false)
}

pub(super) fn mix(base: Rgb, tint: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    let blend = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    (
        blend(base.0, tint.0),
        blend(base.1, tint.1),
        blend(base.2, tint.2),
    )
}

pub(super) fn channel_luminance(channel: u8) -> f32 {
    let c = channel as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub(super) fn luminance(color: Rgb) -> f32 {
    0.2126 * channel_luminance(color.0)
        + 0.7152 * channel_luminance(color.1)
        + 0.0722 * channel_luminance(color.2)
}

pub(super) fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Preto ou branco — o que for legivel por cima de `background`.
pub(super) fn on_color(background: Rgb) -> Rgb {
    if contrast((255, 255, 255), background) >= contrast((17, 19, 20), background) {
        (255, 255, 255)
    } else {
        (17, 19, 20)
    }
}

/// Clareia/escurece `color` ate ter contraste suficiente com o fundo: a cor de
/// destaque do utilizador pode ser preta num tema escuro.
pub(super) fn readable(color: Rgb, background: Rgb, minimum: f32) -> Rgb {
    let target = if luminance(background) > 0.35 {
        (0, 0, 0)
    } else {
        (255, 255, 255)
    };
    let mut amount = 0.0;
    let mut out = color;
    while contrast(out, background) < minimum && amount < 1.0 {
        amount += 0.05;
        out = mix(color, target, amount);
    }
    out
}

/// Cores de marca das tres IAs comparadas.
/// Os degraus de zoom do Chrome, para o gesto ser o que a pessoa ja conhece.
pub(super) const ZOOM_STEPS: [f64; 16] = [
    0.25, 0.33, 0.50, 0.67, 0.75, 0.80, 0.90, 1.00, 1.10, 1.25, 1.50, 1.75, 2.00, 2.50, 3.00, 4.00,
];

/// altura / largura da arte da marca (assets/neuralia-home.png, 1200x868).
pub(super) const BRAND_ASPECT: f64 = 868.0 / 1200.0;

pub(super) const BRAND_COLORS: [Rgb; COMPARATOR_COLUMNS] =
    [(66, 133, 244), (16, 163, 127), (217, 119, 87)];

/// Quanto tempo um tema lido do registo continua a valer. `Theme::system()` e
/// chamada em WM_CTLCOLOREDIT, WM_ERASEBKGND, em cada divisor pintado e a cada
/// frame da Home (15 FPS) — e cada chamada fazia DUAS leituras de registo. Com
/// 1 s de validade o registo passa a ser lido uma vez por segundo, e a mudanca
/// de tema nao fica por notar porque `ThemeChanged` invalida isto de imediato.
pub(super) const THEME_CACHE_TTL: Duration = Duration::from_secs(1);
pub(super) static THEME_CACHE: Mutex<Option<(Instant, Theme)>> = Mutex::new(None);

#[derive(Debug, Clone, Copy)]
pub(super) struct Theme {
    pub(super) accent: Rgb,
    pub(super) page_bg: Rgb,
    pub(super) bar_bg: Rgb,
    pub(super) bar_line: Rgb,
    pub(super) surface: Rgb,
    pub(super) surface_line: Rgb,
    pub(super) fg: Rgb,
    pub(super) fg_muted: Rgb,
    /// Guardado com o resto do tema para quem desenha nao voltar ao registo so
    /// para saber se esta escuro (o fundo neural fazia-o duas vezes por frame).
    pub(super) dark: bool,
}

impl Theme {
    pub(super) fn system() -> Self {
        let mut cache = THEME_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((stamp, theme)) = *cache
            && stamp.elapsed() < THEME_CACHE_TTL
        {
            return theme;
        }
        let theme = Self::read_system();
        *cache = Some((Instant::now(), theme));
        theme
    }

    /// A leitura verdadeira do registo; quem decide quando ela acontece e o
    /// cache acima.
    pub(super) fn read_system() -> Self {
        Self::read_for(ThemeChoice::current())
    }

    /// O tema que `choice` da agora: a escolha manda, o Windows so desempata
    /// em `ThemeChoice::System`.
    pub(super) fn read_for(choice: ThemeChoice) -> Self {
        let accent = system_accent();
        if choice.is_dark(system_dark_mode()) {
            Self::dark(accent)
        } else {
            Self::light(accent)
        }
    }

    /// Obriga a proxima `system()` a reler o registo. Chamada quando o Windows
    /// avisa que o tema mudou: esperar ate 1 s daria um piscar de cores velhas.
    pub(super) fn invalidate() {
        *THEME_CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    pub(super) fn dark(accent: Rgb) -> Self {
        let bar_bg = (27, 30, 32);
        Self {
            accent: readable(accent, bar_bg, 3.2),
            page_bg: (22, 24, 26),
            bar_bg,
            bar_line: (44, 48, 51),
            surface: (37, 41, 44),
            surface_line: (54, 59, 64),
            fg: (233, 236, 239),
            fg_muted: (152, 159, 166),
            dark: true,
        }
    }

    pub(super) fn light(accent: Rgb) -> Self {
        let bar_bg = (255, 255, 255);
        Self {
            accent: readable(accent, bar_bg, 3.2),
            page_bg: (248, 249, 250),
            bar_bg,
            bar_line: (226, 229, 233),
            surface: (242, 244, 246),
            surface_line: (219, 223, 228),
            fg: (26, 29, 32),
            fg_muted: (106, 112, 119),
            dark: false,
        }
    }

    pub(super) fn brand(&self, index: usize) -> Rgb {
        BRAND_COLORS[index.min(COMPARATOR_COLUMNS - 1)]
    }
}
