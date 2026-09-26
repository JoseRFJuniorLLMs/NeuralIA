use super::*;

/// Slot 0..2 = icones das IAs, slot 3 = glifo da casa (pintado com a cor do tema).
pub(in crate::windows_app) const ICON_SLOT_HOME: usize = COMPARATOR_COLUMNS;
/// Icones dos botoes do canto direito (gerados por scripts/gen-ai-icons.py).
pub(in crate::windows_app) const ICON_SLOT_VIDEO: usize = COMPARATOR_COLUMNS + 1;
pub(in crate::windows_app) const ICON_SLOT_WHATSAPP: usize = COMPARATOR_COLUMNS + 2;
pub(in crate::windows_app) const ICON_SLOT_YOUTUBE: usize = COMPARATOR_COLUMNS + 3;
pub(in crate::windows_app) const ICON_SLOT_MAIL: usize = COMPARATOR_COLUMNS + 4;
pub(in crate::windows_app) const ICON_SLOT_INCOGNITO: usize = COMPARATOR_COLUMNS + 5;
/// Ferramentas: Pomodoro, Notas e Respiracao.
pub(in crate::windows_app) const ICON_SLOT_POMODORO: usize = COMPARATOR_COLUMNS + 6;
pub(in crate::windows_app) const ICON_SLOT_NOTES: usize = COMPARATOR_COLUMNS + 7;
pub(in crate::windows_app) const ICON_SLOT_BREATH: usize = COMPARATOR_COLUMNS + 8;
/// O olho do Gemini Live.
pub(in crate::windows_app) const ICON_SLOT_LIVE: usize = COMPARATOR_COLUMNS + 9;
/// A seta dos downloads (downloads-ui): o seu lugar no canto
/// (`RIGHT_CLUSTER`).
pub(in crate::windows_app) const ICON_SLOT_DOWNLOADS: usize = COMPARATOR_COLUMNS + 10;
pub(in crate::windows_app) static EXTRA_ICON_IMAGES: [OnceLock<RgbaImage>; 10] =
    [const { OnceLock::new() }; 10];

pub(in crate::windows_app) static AI_ICON_IMAGES: [OnceLock<RgbaImage>; COMPARATOR_COLUMNS] =
    [OnceLock::new(), OnceLock::new(), OnceLock::new()];
pub(in crate::windows_app) static HOME_ICON_IMAGE: OnceLock<RgbaImage> = OnceLock::new();
/// Tecto do cache de icones redimensionados. Ha 4 slots, mas o tamanho vem da
/// escala da janela: arrastar a borda gera um tamanho novo por pixel percorrido
/// e o cache antigo, sem limite, guardava um bitmap por cada um deles para
/// sempre. 24 entradas chegam para os tamanhos que a barra usa de facto.
pub(in crate::windows_app) const ICON_CACHE_CAPACITY: usize = 24;
/// (slot, lado em pixeis) -> bitmap ja redimensionado, partilhado por `Arc`
/// para o desenho nao copiar a imagem a cada WM_PAINT.
pub(in crate::windows_app) type IconCacheEntry = ((usize, u32), Arc<RgbaImage>);
pub(in crate::windows_app) static ICON_SCALE_CACHE: Mutex<Vec<IconCacheEntry>> =
    Mutex::new(Vec::new());

/// LRU minimo sobre um vector: o fim e o mais recentemente usado, o inicio e o
/// candidato a sair. Estao separadas do cache de icones de proposito — assim a
/// politica de eviccao testa-se sem GDI, sem PNGs e sem estado global.
///
/// Devolve o valor se a chave existir, promovendo a entrada a mais recente.
pub(in crate::windows_app) fn lru_promote<K: PartialEq, V: Clone>(
    entries: &mut Vec<(K, V)>,
    key: &K,
) -> Option<V> {
    let index = entries.iter().position(|(cached, _)| cached == key)?;
    let entry = entries.remove(index);
    let value = entry.1.clone();
    entries.push(entry);
    Some(value)
}

/// Insere como mais recente, deitando fora as mais antigas ate caber em
/// `capacity`. Uma chave repetida substitui a entrada antiga em vez de crescer.
pub(in crate::windows_app) fn lru_insert<K: PartialEq, V>(
    entries: &mut Vec<(K, V)>,
    key: K,
    value: V,
    capacity: usize,
) {
    if capacity == 0 {
        entries.clear();
        return;
    }
    if let Some(index) = entries.iter().position(|(cached, _)| *cached == key) {
        entries.remove(index);
    }
    while entries.len() >= capacity {
        entries.remove(0);
    }
    entries.push((key, value));
}

pub(in crate::windows_app) fn ai_icon(index: usize) -> &'static RgbaImage {
    AI_ICON_IMAGES[index.min(COMPARATOR_COLUMNS - 1)].get_or_init(|| {
        let raw: &[u8] = match index {
            0 => include_bytes!("../../../../assets/ai/gemini.png"),
            1 => include_bytes!("../../../../assets/ai/chatgpt.png"),
            _ => include_bytes!("../../../../assets/ai/claude.png"),
        };
        image::load_from_memory(raw)
            .expect("assets/ai/*.png must be valid PNG")
            .to_rgba8()
    })
}

/// Redimensiona uma vez por (icone, tamanho): o Lanczos3 e caro de mais para
/// correr a cada WM_PAINT, e a barra redesenha-se a cada movimento do rato.
pub(in crate::windows_app) fn home_icon() -> &'static RgbaImage {
    HOME_ICON_IMAGE.get_or_init(|| {
        image::load_from_memory(include_bytes!("../../../../assets/ai/home.png"))
            .expect("assets/ai/home.png must be valid PNG")
            .to_rgba8()
    })
}

pub(in crate::windows_app) fn extra_icon(slot: usize) -> &'static RgbaImage {
    let index = slot
        .saturating_sub(ICON_SLOT_VIDEO)
        .min(EXTRA_ICON_IMAGES.len() - 1);
    EXTRA_ICON_IMAGES[index].get_or_init(|| {
        let raw: &[u8] = match slot {
            ICON_SLOT_VIDEO => include_bytes!("../../../../assets/ai/video.png"),
            ICON_SLOT_WHATSAPP => include_bytes!("../../../../assets/ai/whatsapp.png"),
            ICON_SLOT_YOUTUBE => include_bytes!("../../../../assets/ai/youtube.png"),
            ICON_SLOT_MAIL => include_bytes!("../../../../assets/ai/mail.png"),
            ICON_SLOT_POMODORO => include_bytes!("../../../../assets/ai/pomodoro.png"),
            ICON_SLOT_NOTES => include_bytes!("../../../../assets/ai/notes.png"),
            ICON_SLOT_BREATH => include_bytes!("../../../../assets/ai/breath.png"),
            ICON_SLOT_LIVE => include_bytes!("../../../../assets/ai/live.png"),
            ICON_SLOT_DOWNLOADS => include_bytes!("../../../../assets/ai/downloads.png"),
            _ => include_bytes!("../../../../assets/ai/incognito.png"),
        };
        image::load_from_memory(raw)
            .expect("assets/ai/*.png must be valid PNG")
            .to_rgba8()
    })
}

pub(in crate::windows_app) fn icon_scaled(slot: usize, size: u32) -> Arc<RgbaImage> {
    let key = (slot, size);
    let mut guard = ICON_SCALE_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let entries = &mut *guard;
    if let Some(image) = lru_promote(entries, &key) {
        return image;
    }

    let source = if slot == ICON_SLOT_HOME {
        home_icon()
    } else if slot >= ICON_SLOT_VIDEO {
        extra_icon(slot)
    } else {
        ai_icon(slot)
    };
    let scaled = Arc::new(image::imageops::resize(
        source,
        size,
        size,
        image::imageops::FilterType::Lanczos3,
    ));
    lru_insert(entries, key, Arc::clone(&scaled), ICON_CACHE_CAPACITY);
    scaled
}

/// `tint` substitui a cor do icone mantendo o alfa — e assim que o glifo da
/// casa segue o tema sem existirem dois PNGs.
pub(in crate::windows_app) unsafe fn draw_icon(
    hdc: *mut core::ffi::c_void,
    slot: usize,
    x: i32,
    y: i32,
    size: i32,
    background: Rgb,
    tint: Option<Rgb>,
) {
    if size <= 0 {
        return;
    }

    // O cache entrega o `Arc`; aqui so se le, por isso basta emprestar.
    let scaled = icon_scaled(slot, size as u32);
    let image: &RgbaImage = &scaled;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size as u32 {
        for px in 0..size as u32 {
            let pixel = image.get_pixel(px, py);
            let alpha = pixel[3] as f32 / 255.0;
            let source = tint.unwrap_or((pixel[0], pixel[1], pixel[2]));
            let channel = |value: u8, bg: u8| {
                (value as f32 * alpha + bg as f32 * (1.0 - alpha)).round() as u8
            };
            pixels.push(channel(source.2, background.2));
            pixels.push(channel(source.1, background.1));
            pixels.push(channel(source.0, background.0));
            pixels.push(0);
        }
    }
    blit_bgrx(hdc, &pixels, x, y, size, size);
}

#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct PillStyle {
    pub(in crate::windows_app) fill: Rgb,
    pub(in crate::windows_app) border: Rgb,
    pub(in crate::windows_app) text: Rgb,
    pub(in crate::windows_app) icon: Option<usize>,
    pub(in crate::windows_app) icon_tint: Option<Rgb>,
}

impl PillStyle {
    pub(in crate::windows_app) fn new(fill: Rgb, border: Rgb, text: Rgb) -> Self {
        Self {
            fill,
            border,
            text,
            icon: None,
            icon_tint: None,
        }
    }

    pub(in crate::windows_app) fn with_icon(mut self, slot: usize, tint: Option<Rgb>) -> Self {
        self.icon = Some(slot);
        self.icon_tint = tint;
        self
    }
}

/// Pilula com icone a esquerda e legenda; sem icone, a legenda fica centrada.
/// O icone de 18 px da pilula so entra com as margens dos dois lados; numa
/// pilula mais estreita (a coluna espremida) ele saia pela borda e caia na
/// folga ou debaixo do "+".
pub(in crate::windows_app) fn pill_fits_icon(width: f64, scale: f64) -> bool {
    let padding = 11.0 * scale;
    width >= padding + (18.0 * scale).round() + padding * 0.6
}

pub(in crate::windows_app) unsafe fn draw_pill(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    label: &str,
    style: PillStyle,
    scale: f64,
    font: *mut core::ffi::c_void,
    background: Rgb,
) {
    // Uma pilula que nao coube (a da coluna espremida pelos controlos da
    // direita) tem largura zero. O `fill_pill` ja nao a pintava, mas o icone
    // era desenhado na mesma, solto na barra -- e aparecia nas folgas entre
    // os botoes das ferramentas.
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    fill_pill(
        hdc,
        rect,
        rect.height / 2.0,
        style.fill,
        Some((style.border, scale)),
        background,
    );

    let padding = 11.0 * scale;
    let mut text_left = rect.x + padding;
    let mut text_right = rect.x + rect.width - padding * 0.6;
    let mut format = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX;

    match style.icon.filter(|_| pill_fits_icon(rect.width, scale)) {
        Some(slot) => {
            let size = (18.0 * scale).round() as i32;
            let icon_y = (rect.y + (rect.height - size as f64) / 2.0).round() as i32;
            draw_icon(
                hdc,
                slot,
                text_left.round() as i32,
                icon_y,
                size,
                style.fill,
                style.icon_tint,
            );
            text_left += size as f64 + 8.0 * scale;
        }
        // Texto centrado usa a pilula inteira. Com a margem de 11 px dos dois
        // lados, um botao redondo de ~30 px ficava com ~10 px para o "+" e o
        // DT_END_ELLIPSIS desenhava "-." no lugar dele.
        None => {
            text_left = rect.x;
            text_right = rect.x + rect.width;
            format |= DT_CENTER;
        }
    }

    SelectObject(hdc, font as _);
    SetTextColor(hdc, rgb3(style.text));
    // O DC de um BeginPaint nasce OPAQUE com fundo branco. Quem chamava sem
    // SetBkMode (botao Home, botoes -/□/x) pintava o texto num rectangulo
    // branco por cima da pilula -- os "icones" viravam quadrados brancos.
    SetBkMode(hdc, TRANSPARENT as i32);
    let mut text_rect = RECT {
        left: text_left.round() as i32,
        top: rect.y as i32,
        right: text_right as i32,
        bottom: (rect.y + rect.height) as i32,
    };
    draw_text(hdc, label, &mut text_rect, format);
}

pub(in crate::windows_app) unsafe fn draw_button(
    hdc: *mut core::ffi::c_void,
    rect: UiRect,
    label: &str,
    primary: bool,
    scale: f64,
    font: *mut core::ffi::c_void,
    theme: &Theme,
) {
    let style = if primary {
        PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
    } else {
        PillStyle::new(theme.surface, theme.surface_line, theme.fg)
    };
    draw_pill(hdc, rect, label, style, scale, font, theme.page_bg);
}

pub(in crate::windows_app) unsafe fn draw_text(
    hdc: *mut core::ffi::c_void,
    text: &str,
    rect: &mut RECT,
    format: u32,
) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    if !wide.is_empty() {
        DrawTextW(hdc, wide.as_ptr(), wide.len() as i32, rect, format);
    }
}

pub(in crate::windows_app) const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

pub(in crate::windows_app) const fn rgb3(color: Rgb) -> u32 {
    rgb(color.0, color.1, color.2)
}
