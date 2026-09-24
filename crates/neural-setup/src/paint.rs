//! O desenho, em GDI. Recebe geometria do `ui` e tecido do `neural_core` e
//! nao decide nada -- assim o que se pode testar esta testado noutro sitio, e
//! aqui so fica o que precisa mesmo de um ecra.

#![cfg(windows)]

use std::sync::{Arc, Mutex, OnceLock};

use neural_core::tissue::{self, Tissue};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::*;

use crate::ui::{Hit, Layout, Rect, Screen};

pub type Rgb = (u8, u8, u8);

/// Paleta escura. O instalador nao segue o tema do sistema de proposito: e um
/// ecra que aparece uma vez, e a marca e esta.
pub const PAGE: Rgb = (8, 11, 22);
pub const PANEL: Rgb = (15, 21, 38);
pub const LINE: Rgb = (35, 48, 82);
pub const ACCENT: Rgb = (56, 189, 248);
pub const ACCENT_FAR: Rgb = (167, 139, 250);
pub const FG: Rgb = (232, 238, 250);
pub const MUTED: Rgb = (138, 154, 184);
pub const OK: Rgb = (52, 211, 153);
/// A cor de uma sinapse a disparar. Quente de proposito: num tecido todo azul,
/// e o unico sitio onde algo acontece, e tem de se ver a primeira vista.
pub const SPARK: Rgb = (255, 138, 76);
pub const BAD: Rgb = (248, 113, 113);

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| {
        (x as f64 + (y as f64 - x as f64) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2))
}

pub fn colorref(c: Rgb) -> u32 {
    c.0 as u32 | ((c.1 as u32) << 8) | ((c.2 as u32) << 16)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

pub unsafe fn font(height: i32, weight: i32) -> HFONT {
    let face = "Segoe UI\0".encode_utf16().collect::<Vec<u16>>();
    CreateFontW(
        height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        (DEFAULT_PITCH | FF_DONTCARE) as u32,
        face.as_ptr(),
    )
}

unsafe fn to_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.x.round() as i32,
        top: rect.y.round() as i32,
        right: rect.right().round() as i32,
        bottom: rect.bottom().round() as i32,
    }
}

pub unsafe fn fill(hdc: HDC, rect: Rect, color: Rgb) {
    let brush = CreateSolidBrush(colorref(color));
    let r = to_rect(rect);
    FillRect(hdc, &r, brush);
    DeleteObject(brush as _);
}

/// Retangulo de cantos redondos, com contorno opcional.
pub unsafe fn round(hdc: HDC, rect: Rect, radius: f64, fill_color: Rgb, edge: Option<Rgb>) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let brush = CreateSolidBrush(colorref(fill_color));
    let pen = CreatePen(PS_SOLID, 1, colorref(edge.unwrap_or(fill_color)));
    let old_brush = SelectObject(hdc, brush as _);
    let old_pen = SelectObject(hdc, pen as _);
    let r = to_rect(rect);
    let d = (radius * 2.0).round() as i32;
    RoundRect(hdc, r.left, r.top, r.right, r.bottom, d, d);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(brush as _);
    DeleteObject(pen as _);
}

pub unsafe fn text(hdc: HDC, value: &str, rect: Rect, color: Rgb, hfont: HFONT, format: u32) {
    let old = SelectObject(hdc, hfont as _);
    SetTextColor(hdc, colorref(color));
    SetBkMode(hdc, TRANSPARENT as i32);
    let mut r = to_rect(rect);
    let text = wide(value);
    if !text.is_empty() {
        DrawTextW(hdc, text.as_ptr(), text.len() as i32, &mut r, format);
    }
    SelectObject(hdc, old);
}

/// Deslocamentos do contorno do texto: um disco de raio `radius` px sem o
/// centro. Desenhar o texto na cor do fundo em cada um destes deslocamentos, e
/// so depois na sua cor, da-lhe uma orla escura fina -- le-se por cima dos
/// neuronios sem abrir uma zona escura a volta dele.
pub fn halo_offsets(radius: i32) -> Vec<(i32, i32)> {
    let mut offsets = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if (dx, dy) != (0, 0) && dx * dx + dy * dy <= radius * radius + radius {
                offsets.push((dx, dy));
            }
        }
    }
    offsets
}

/// Texto desenhado diretamente por cima do tecido (titulo, versao, caminho,
/// etapa, legenda): primeiro a orla na cor da pagina, depois o texto. Os
/// botoes nao usam isto -- tem fundo proprio.
pub unsafe fn text_on_tissue(
    hdc: HDC,
    value: &str,
    rect: Rect,
    color: Rgb,
    hfont: HFONT,
    format: u32,
    scale: f64,
) {
    let radius = (2.0 * scale).round().max(1.0) as i32;
    for (dx, dy) in halo_offsets(radius) {
        let shifted = Rect {
            x: rect.x + f64::from(dx),
            y: rect.y + f64::from(dy),
            ..rect
        };
        text(hdc, value, shifted, PAGE, hfont, format);
    }
    text(hdc, value, rect, color, hfont, format);
}

/// O fundo: neuronios, sinapses e impulsos. E o mesmo tecido do navegador,
/// vindo do `neural-core`.
pub unsafe fn tissue_background(hdc: HDC, layout: &Layout, seconds: f64) {
    let field = crate::ui::tissue_field(layout);
    let Tissue {
        nodes,
        branches,
        links,
        pulses,
        bursts,
    } = tissue::tissue_at(&field, seconds);

    // A ramagem primeiro: e o que esta por tras de tudo. Tres canetas pela
    // espessura do ramo -- uma por segmento seria caro a 30 quadros por
    // segundo e nao se notaria.
    let twigs: [HPEN; 3] = std::array::from_fn(|step| {
        let weight = 0.07 + step as f64 * 0.11;
        CreatePen(
            PS_SOLID,
            1 + step as i32,
            colorref(mix(PAGE, ACCENT, weight)),
        )
    });
    let old_pen = SelectObject(hdc, twigs[0] as _);
    for branch in &branches {
        let bucket = ((branch.weight * 3.0) as usize).min(2);
        SelectObject(hdc, twigs[bucket] as _);
        MoveToEx(
            hdc,
            branch.ax.round() as i32,
            branch.ay.round() as i32,
            std::ptr::null_mut(),
        );
        LineTo(hdc, branch.bx.round() as i32, branch.by.round() as i32);
    }
    SelectObject(hdc, old_pen);
    for pen in twigs {
        DeleteObject(pen as _);
    }

    // As sinapses por cima da ramagem, mais acesas: sao ligacao, nao tecido.
    let pens: [HPEN; 3] = std::array::from_fn(|step| {
        let weight = 0.14 + (2 - step) as f64 * 0.11;
        CreatePen(PS_SOLID, 1, colorref(mix(PAGE, ACCENT, weight)))
    });
    let old_pen = SelectObject(hdc, pens[2] as _);
    for link in &links {
        let bucket = ((link.closeness * 3.0) as usize).min(2);
        SelectObject(hdc, pens[bucket] as _);
        MoveToEx(
            hdc,
            link.ax.round() as i32,
            link.ay.round() as i32,
            std::ptr::null_mut(),
        );
        LineTo(hdc, link.bx.round() as i32, link.by.round() as i32);
    }
    SelectObject(hdc, old_pen);
    for pen in pens {
        DeleteObject(pen as _);
    }

    for pulse in &pulses {
        let color = mix(PAGE, ACCENT_FAR, 0.35 + 0.6 * pulse.glow);
        let side = (2.0 + 2.0 * pulse.glow) * layout.scale;
        fill(
            hdc,
            Rect {
                x: pulse.x - side / 2.0,
                y: pulse.y - side / 2.0,
                width: side,
                height: side,
            },
            color,
        );
    }

    for node in &nodes {
        // O tamanho e o brilho seguem a profundidade: os da frente sao corpos,
        // os do fundo sao pontos. E o que da volume a folha.
        let radius = (1.4 + 4.6 * node.depth) * (0.75 + 0.25 * node.energy) * layout.scale;
        let color = mix(PAGE, ACCENT, 0.18 + 0.30 * node.depth + 0.30 * node.energy);
        round(
            hdc,
            Rect {
                x: node.x - radius,
                y: node.y - radius,
                width: radius * 2.0,
                height: radius * 2.0,
            },
            radius,
            color,
            None,
        );
    }

    // As descargas por cima de tudo: sao o que acontece agora, e o que
    // acontece agora fica a frente. Dois aneis e um nucleo -- o de fora
    // largo e fraco, o de dentro apertado e forte, que e como uma faisca se
    // le sem ter alpha nenhum a disposicao.
    for burst in &bursts {
        let ring = |factor: f64, weight: f64| {
            let r = burst.radius * factor;
            let pen = CreatePen(
                PS_SOLID,
                (1.0 + burst.glow).round() as i32,
                colorref(mix(PAGE, SPARK, weight * burst.glow)),
            );
            let old_pen = SelectObject(hdc, pen as _);
            let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH) as _);
            Ellipse(
                hdc,
                (burst.x - r).round() as i32,
                (burst.y - r).round() as i32,
                (burst.x + r).round() as i32,
                (burst.y + r).round() as i32,
            );
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(pen as _);
        };
        ring(1.0, 0.22);
        ring(0.55, 0.55);

        // Nucleo pequeno: uma faisca, nao um holofote. Grande demais lia-se
        // como um defeito de desenho e nao como uma descarga.
        let core = burst.radius * 0.11 * (0.4 + burst.glow);
        round(
            hdc,
            Rect {
                x: burst.x - core,
                y: burst.y - core,
                width: core * 2.0,
                height: core * 2.0,
            },
            core,
            mix(SPARK, (255, 245, 235), 0.20 + 0.55 * burst.glow),
            None,
        );
    }
}

/// A barra: calha, parte acesa em degrade, brilho a correr e um halo por cima.
pub unsafe fn progress_bar(hdc: HDC, layout: &Layout, progress: f64, seconds: f64, tone: Rgb) {
    let radius = layout.track.height / 2.0;
    round(hdc, layout.track, radius, PANEL, Some(LINE));

    let filled = layout.filled(progress);
    if filled.width <= 0.0 {
        return;
    }

    // Degrade a mao: uma coluna por pixel. A `GradientFill` obrigava a carregar
    // a msimg32 so para isto.
    let steps = filled.width.round().max(1.0) as i32;
    for step in 0..steps {
        let t = step as f64 / steps as f64;
        let color = mix(ACCENT, tone, t);
        fill(
            hdc,
            Rect {
                x: filled.x + step as f64,
                y: filled.y,
                width: 1.0,
                height: filled.height,
            },
            color,
        );
    }

    if let Some(shimmer) = layout.shimmer(progress, seconds) {
        let steps = shimmer.width.round().max(1.0) as i32;
        for step in 0..steps {
            let t = step as f64 / steps as f64;
            // Acende no meio da banda e apaga nas pontas.
            let glow = (t * std::f64::consts::PI).sin();
            fill(
                hdc,
                Rect {
                    x: shimmer.x + step as f64,
                    y: shimmer.y,
                    width: 1.0,
                    height: shimmer.height,
                },
                mix(mix(ACCENT, tone, 0.5), (255, 255, 255), 0.55 * glow),
            );
        }
    }

    // Halo: uma linha fina por baixo da ponta acesa, a sugerir luz.
    let head = Rect {
        x: filled.x,
        y: filled.bottom() + 2.0 * layout.scale,
        width: filled.width,
        height: 1.0_f64.max(layout.scale),
    };
    fill(hdc, head, mix(PAGE, tone, 0.30));
}

#[allow(clippy::too_many_arguments)]
pub unsafe fn button(
    hdc: HDC,
    rect: Rect,
    label: &str,
    primary: bool,
    hovered: bool,
    enabled: bool,
    hfont: HFONT,
    scale: f64,
) {
    let radius = 10.0 * scale;
    let (fill_color, edge, ink) = match (primary, enabled) {
        (_, false) => (PANEL, LINE, MUTED),
        (true, true) => {
            let base = mix(ACCENT, ACCENT_FAR, 0.35);
            (
                if hovered {
                    mix(base, (255, 255, 255), 0.18)
                } else {
                    base
                },
                mix(base, (255, 255, 255), 0.25),
                (6, 12, 26),
            )
        }
        (false, true) => (
            if hovered {
                mix(PANEL, ACCENT, 0.22)
            } else {
                PANEL
            },
            LINE,
            FG,
        ),
    };
    round(hdc, rect, radius, fill_color, Some(edge));
    text(
        hdc,
        label,
        Rect {
            y: rect.y + rect.height * 0.5 - 11.0 * scale,
            ..rect
        },
        ink,
        hfont,
        DT_CENTER | DT_SINGLELINE,
    );
}

pub unsafe fn checkbox(hdc: HDC, rect: Rect, checked: bool, hovered: bool, scale: f64) {
    let edge = if hovered {
        mix(LINE, ACCENT, 0.6)
    } else {
        LINE
    };
    round(
        hdc,
        rect,
        5.0 * scale,
        if checked {
            mix(PAGE, ACCENT, 0.75)
        } else {
            PANEL
        },
        Some(edge),
    );
    if !checked {
        return;
    }
    // Visto, em duas linhas.
    let pen = CreatePen(
        PS_SOLID,
        (2.0 * scale).round().max(1.0) as i32,
        colorref(PAGE),
    );
    let old = SelectObject(hdc, pen as _);
    let x = rect.x;
    let y = rect.y;
    let w = rect.width;
    let h = rect.height;
    MoveToEx(
        hdc,
        (x + w * 0.24).round() as i32,
        (y + h * 0.52).round() as i32,
        std::ptr::null_mut(),
    );
    LineTo(
        hdc,
        (x + w * 0.44).round() as i32,
        (y + h * 0.72).round() as i32,
    );
    LineTo(
        hdc,
        (x + w * 0.78).round() as i32,
        (y + h * 0.28).round() as i32,
    );
    SelectObject(hdc, old);
    DeleteObject(pen as _);
}

static BRAND: OnceLock<image::RgbaImage> = OnceLock::new();

/// A arte da marca: `assets/neuralia-home.png`, a mesma da Home do navegador.
/// Nao tem fundo -- o que nao e marca e transparente --, por isso nao ha nada
/// a recortar nem a dissolver: basta respeitar o alfa que ela traz.
fn brand_image() -> &'static image::RgbaImage {
    BRAND.get_or_init(|| {
        let raw = include_bytes!("../../../assets/neuralia-home.png");
        image::load_from_memory(raw)
            .expect("assets/neuralia-home.png tem de ser um PNG valido")
            .to_rgba8()
    })
}

/// (largura, altura, pixeis RGBA pre-multiplicados de cima para baixo).
type ScaledBrand = Option<(u32, u32, Arc<Vec<u8>>)>;
static SCALED_BRAND: Mutex<ScaledBrand> = Mutex::new(None);

/// A arte ja reduzida ao retangulo dela, com o alfa multiplicado nas cores.
///
/// Reduz-se uma vez por tamanho: o retangulo so muda quando muda o DPI, e
/// redimensionar 1200x868 com Lanczos a 30 quadros por segundo era o que mais
/// custava a cada pintura. Pre-multiplica-se ANTES de reduzir: a cor dos
/// pixeis transparentes nao se ve, e sem isto escorria para a orla da marca.
fn scaled_brand(width: u32, height: u32) -> Arc<Vec<u8>> {
    let mut cache = SCALED_BRAND.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((w, h, pixels)) = cache.as_ref()
        && (*w, *h) == (width, height)
    {
        return Arc::clone(pixels);
    }
    let mut premultiplied = brand_image().clone();
    for pixel in premultiplied.pixels_mut() {
        let alpha = pixel[3] as u32;
        for channel in 0..3 {
            pixel[channel] = ((pixel[channel] as u32 * alpha + 127) / 255) as u8;
        }
    }
    let mut scaled = image::imageops::resize(
        &premultiplied,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    // O Lanczos passa um pouco do sitio nas orlas; uma cor pre-multiplicada
    // acima do proprio alfa acenderia o tecido por baixo.
    for pixel in scaled.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] = pixel[channel].min(pixel[3]);
        }
    }
    let pixels = Arc::new(scaled.into_raw());
    *cache = Some((width, height, Arc::clone(&pixels)));
    pixels
}

/// Pousa a arte (RGBA pre-multiplicado) sobre o que ja esta pintado (BGRA,
/// como a seccao DIB o da): `arte + fundo * (1 - alfa)`. Onde a arte e
/// transparente, o fundo fica exatamente como estava.
fn compose_over(back: &mut [u8], art: &[u8]) {
    for (dst, src) in back
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(art.as_chunks::<4>().0)
    {
        let keep = 255 - src[3] as u32;
        let over =
            |ink: u8, under: u8| (ink as u32 + (under as u32 * keep + 127) / 255).min(255) as u8;
        dst[0] = over(src[2], dst[0]);
        dst[1] = over(src[1], dst[1]);
        dst[2] = over(src[0], dst[2]);
    }
}

/// A marca, pousada em cima do tecido.
///
/// Ate a 2.1.7 a arte era opaca e ia para o ecra com `SRCCOPY`, composta
/// sobre a cor da pagina: um retangulo liso que apagava os neuronios que
/// passavam por tras -- o quadrado que o dono via a volta da marca. Agora le-se
/// o que ja esta pintado naquele retangulo, compoe-se a arte por cima com o
/// alfa dela e devolve-se: o tecido continua a ver-se por todo o lado onde a
/// arte e transparente. Sem `AlphaBlend`, que obrigaria a carregar a msimg32.
pub unsafe fn logo(hdc: HDC, rect: Rect) {
    let width = rect.width.round().max(1.0) as u32;
    let height = rect.height.round().max(1.0) as u32;
    let (x, y) = (rect.x.round() as i32, rect.y.round() as i32);
    let art = scaled_brand(width, height);

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // Negativo: de cima para baixo, como a `image` da os pixeis.
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }],
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(
        hdc,
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        std::ptr::null_mut(),
        0,
    );
    let scratch = CreateCompatibleDC(hdc);
    if dib.is_null() || bits.is_null() || scratch.is_null() {
        if !dib.is_null() {
            DeleteObject(dib as _);
        }
        if !scratch.is_null() {
            DeleteDC(scratch);
        }
        return;
    }
    let old = SelectObject(scratch, dib as _);
    // O que ja la esta: o tecido.
    BitBlt(
        scratch,
        0,
        0,
        width as i32,
        height as i32,
        hdc,
        x,
        y,
        SRCCOPY,
    );
    GdiFlush();
    let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (width * height * 4) as usize);
    compose_over(pixels, &art);
    BitBlt(
        hdc,
        x,
        y,
        width as i32,
        height as i32,
        scratch,
        0,
        0,
        SRCCOPY,
    );
    SelectObject(scratch, old);
    DeleteDC(scratch);
    DeleteObject(dib as _);
}

pub unsafe fn close_button(hdc: HDC, layout: &Layout, hovered: bool) {
    if hovered {
        fill(hdc, layout.close, mix(PAGE, BAD, 0.55));
    }
    let hfont = font(-(16.0 * layout.scale) as i32, 400);
    text(
        hdc,
        "\u{00D7}",
        Rect {
            y: layout.close.y + layout.close.height * 0.5 - 12.0 * layout.scale,
            ..layout.close
        },
        if hovered { FG } else { MUTED },
        hfont,
        DT_CENTER | DT_SINGLELINE,
    );
    DeleteObject(hfont as _);
}

/// A cor da barra e do que se le: o estado tem de ver-se de longe, sem ler.
pub fn tone_for(screen: Screen) -> Rgb {
    match screen {
        Screen::Failed => BAD,
        Screen::Finished => OK,
        _ => ACCENT_FAR,
    }
}

pub fn hovered(hover: Option<Hit>, what: Hit) -> bool {
    hover == Some(what)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_over_the_tissue_gets_a_thin_ring_not_a_box() {
        // Sem a zona de silencio, o texto le-se gracas a uma orla fina na cor
        // da pagina: um anel a volta de cada letra, simetrico, que cobre as
        // oito direcoes e nao passa do raio pedido.
        assert!(halo_offsets(0).is_empty());
        for radius in 1..=4 {
            let ring = halo_offsets(radius);
            assert!(!ring.contains(&(0, 0)), "o centro e o proprio texto");
            for &(dx, dy) in &ring {
                assert!(dx.abs() <= radius && dy.abs() <= radius);
                assert!(ring.contains(&(-dx, -dy)), "orla torta em ({dx}, {dy})");
            }
            for direction in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
            ] {
                assert!(ring.contains(&direction), "raio {radius} sem {direction:?}");
            }
        }
    }

    #[test]
    fn the_brand_rect_matches_the_art_it_draws() {
        // `BRAND_ASPECT` e um numero escrito a mao. Se a arte for trocada por
        // outra de proporcao diferente, a marca passa a sair esticada sem que
        // nada se queixe -- excepto isto.
        let (w, h) = brand_image().dimensions();
        let actual = w as f64 / h as f64;
        assert!(
            (actual - crate::ui::BRAND_ASPECT).abs() < 0.01,
            "a arte e {w}x{h} ({actual:.4}), mas BRAND_ASPECT diz {:.4}",
            crate::ui::BRAND_ASPECT
        );
    }

    #[test]
    fn the_installer_brand_is_the_home_art() {
        // O dono: "neuralia-home.png nao tem fundo usa ela para tudo". A arte
        // que o instalador desenha tem de ser essa -- a do disco, nao so a que
        // o `include_bytes!` diz ser.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/neuralia-home.png"
        );
        let on_disk = image::open(path)
            .expect("assets/neuralia-home.png")
            .to_rgba8();
        let drawn = brand_image();
        assert_eq!(drawn.dimensions(), (1200, 868));
        assert!(
            drawn.as_raw() == on_disk.as_raw(),
            "o instalador nao desenha a assets/neuralia-home.png"
        );
        // E essa arte nao traz fundo: os cantos sao transparentes.
        for (x, y) in [(0, 0), (1199, 0), (0, 867), (1199, 867)] {
            assert_eq!(drawn.get_pixel(x, y)[3], 0, "fundo no canto ({x}, {y})");
        }
    }

    /// Um tecido que se reconhece pixel a pixel (BGR): um xadrez de duas
    /// cores que a marca nao tem.
    fn probe(x: u32, y: u32) -> [u8; 3] {
        if (x / 5 + y / 5).is_multiple_of(2) {
            [90, 30, 200]
        } else {
            [40, 160, 20]
        }
    }

    #[test]
    fn the_brand_lands_on_the_tissue_without_a_square() {
        // "nao quero contorno quadrado ao redor da logo marca". Pinta-se a
        // marca com o `logo` que embarca por cima de um tecido conhecido, numa
        // seccao DIB, e le-se o que ficou: onde a arte e transparente o tecido
        // tem de continuar la, igual; onde e opaca, tem de estar a arte.
        const W: u32 = 520;
        const H: u32 = 400;
        let rect = Rect {
            x: 30.0,
            y: 40.0,
            width: 440.0,
            height: 440.0 / crate::ui::BRAND_ASPECT,
        };
        let (rx, ry) = (rect.x.round() as u32, rect.y.round() as u32);
        let (rw, rh) = (rect.width.round() as u32, rect.height.round() as u32);
        let painted = unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            assert!(!dc.is_null());
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: W as i32,
                    biHeight: -(H as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [RGBQUAD {
                    rgbBlue: 0,
                    rgbGreen: 0,
                    rgbRed: 0,
                    rgbReserved: 0,
                }],
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            assert!(!dib.is_null() && !bits.is_null());
            let old = SelectObject(dc, dib as _);
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (W * H * 4) as usize);
            for y in 0..H {
                for x in 0..W {
                    let at = ((y * W + x) * 4) as usize;
                    pixels[at..at + 3].copy_from_slice(&probe(x, y));
                    pixels[at + 3] = 255;
                }
            }
            logo(dc, rect);
            GdiFlush();
            let painted = pixels.to_vec();
            SelectObject(dc, old);
            DeleteObject(dib as _);
            DeleteDC(dc);
            painted
        };

        let art = scaled_brand(rw, rh);
        let (mut clear, mut inked) = (0usize, 0usize);
        for y in 0..H {
            for x in 0..W {
                let at = ((y * W + x) * 4) as usize;
                let seen = [painted[at], painted[at + 1], painted[at + 2]];
                let inside = (rx..rx + rw).contains(&x) && (ry..ry + rh).contains(&y);
                let ink = if inside {
                    let a = (((y - ry) * rw + (x - rx)) * 4) as usize;
                    Some([art[a + 2], art[a + 1], art[a], art[a + 3]])
                } else {
                    None
                };
                match ink {
                    None | Some([_, _, _, 0]) => {
                        assert_eq!(
                            seen,
                            probe(x, y),
                            "em ({x}, {y}) a arte e transparente e o tecido \
                             desapareceu: e o quadrado a volta da marca"
                        );
                        clear += usize::from(inside);
                    }
                    Some([b, g, r, 255]) => {
                        assert_eq!(seen, [b, g, r], "em ({x}, {y}) a marca nao foi desenhada");
                        inked += 1;
                    }
                    Some(_) => {}
                }
            }
        }
        // A arte tem mesmo margem transparente, e a marca esta la.
        let area = (rw * rh) as usize;
        assert!(
            clear * 100 >= area * 30,
            "so {clear} de {area} pixeis do retangulo mostram o tecido"
        );
        assert!(
            inked * 100 >= area * 10,
            "so {inked} de {area} pixeis do retangulo tem a marca inteira"
        );
    }
}
