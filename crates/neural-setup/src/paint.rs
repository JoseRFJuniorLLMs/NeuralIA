//! O desenho, em GDI. Recebe geometria do `ui` e tecido do `neural_core` e
//! nao decide nada -- assim o que se pode testar esta testado noutro sitio, e
//! aqui so fica o que precisa mesmo de um ecra.

#![cfg(windows)]

use std::sync::OnceLock;

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

/// O fundo: neuronios, sinapses e impulsos. E o mesmo tecido do navegador,
/// vindo do `neural-core`.
pub unsafe fn tissue_background(hdc: HDC, layout: &Layout, seconds: f64) {
    // A zona de silencio cobre a coluna inteira do texto, nao so a marca:
    // uma sinapse a passar por cima do caminho de instalacao torna-o ilegivel,
    // e o caminho e a unica coisa nesta janela que o utilizador precisa mesmo
    // de conseguir ler.
    let top = layout.logo.y;
    let bottom = layout.note.bottom();
    let field = tissue::Field::new(layout.client.width, layout.client.height, layout.scale)
        .with_quiet_ellipse(
            layout.client.center_x(),
            (top + bottom) / 2.0,
            layout.client.width * 0.42,
            (bottom - top) / 2.0 + 10.0 * layout.scale,
        );
    let Tissue {
        nodes,
        links,
        pulses,
        bursts,
    } = tissue::tissue_at(&field, seconds);

    // Tres canetas, escolhidas pela proximidade: uma por ligacao seria caro a
    // 30 quadros por segundo e nao se notaria.
    let pens: [HPEN; 3] = std::array::from_fn(|step| {
        let weight = 0.10 + (2 - step) as f64 * 0.10;
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
        let radius = (1.6 + 2.2 * node.energy) * layout.scale;
        let color = mix(PAGE, ACCENT, 0.30 + 0.45 * node.energy);
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
                colorref(mix(
                    PAGE,
                    mix(ACCENT_FAR, (255, 255, 255), 0.45),
                    weight * burst.glow,
                )),
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
            mix(ACCENT, (255, 255, 255), 0.20 + 0.45 * burst.glow),
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

/// A arte da marca.
fn brand_image() -> &'static image::RgbaImage {
    BRAND.get_or_init(|| {
        let raw = include_bytes!("../../../assets/logo.png");
        image::load_from_memory(raw)
            .expect("assets/logo.png tem de ser um PNG valido")
            .to_rgba8()
    })
}

/// Quanto da arte se ve num ponto, entre 0 na borda e 1 no interior.
///
/// A arte e opaca: o fundo dela e um degrade escuro que nao e exatamente a cor
/// desta janela, e desenhada tal e qual aparecia dentro de um retangulo
/// visivel. Em vez de a recortar -- o recorte deixa franjas brancas a volta da
/// esfera -- dissolve-se a moldura no fundo. O que fica e a arte inteira, sem
/// caixa.
fn feather_alpha(x: u32, y: u32, width: u32, height: u32, feather: f64) -> f64 {
    if feather <= 0.0 {
        return 1.0;
    }
    let dx = (x as f64).min((width.saturating_sub(1).saturating_sub(x)) as f64);
    let dy = (y as f64).min((height.saturating_sub(1).saturating_sub(y)) as f64);
    let t = (dx.min(dy) / feather).clamp(0.0, 1.0);
    // Rampa suave: uma rampa linear deixa-se ver como uma linha.
    t * t * (3.0 - 2.0 * t)
}

/// A largura da dissolucao, em pixeis da arte ja redimensionada.
fn feather_width(width: u32, height: u32) -> f64 {
    (width.min(height) as f64 * 0.16).max(1.0)
}

/// Abaixo desta luminancia a arte e fundo e nao se mostra; acima desta,
/// e marca e mostra-se inteira. No meio, desvanece.
const ART_FLOOR: f64 = 0.10;
const ART_CEILING: f64 = 0.26;

/// Quanto de um pixel da arte se ve, pela sua luminancia.
///
/// A arte tem um fundo proprio -- um degrade azul-escuro -- que **nao** e a
/// cor desta janela: desenhada tal e qual, a marca aparecia dentro de um
/// retangulo mais claro. Recortar pelo alfa nao resolve, porque a arte e
/// opaca; e recortar a mao deixa franjas. Entao usa-se a propria luminancia
/// como mascara: o que e escuro era fundo e da lugar a pagina, o que brilha e
/// a esfera e as letras e fica. O halo a volta da esfera, que esta no meio,
/// desvanece na proporcao certa -- que e o que faz a marca parecer pousada no
/// fundo em vez de colada por cima.
fn art_visibility(pixel: [u8; 4]) -> f64 {
    let luma =
        (0.2126 * pixel[0] as f64 + 0.7152 * pixel[1] as f64 + 0.0722 * pixel[2] as f64) / 255.0;
    let t = ((luma - ART_FLOOR) / (ART_CEILING - ART_FLOOR)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A marca, composta sobre a cor de fundo. Nao ha `AlphaBlend` nenhum: a zona
/// onde a marca cai e a zona de silencio do tecido, portanto e cor lisa, e
/// compor a mao da o mesmo resultado sem carregar a msimg32.
pub unsafe fn logo(hdc: HDC, rect: Rect) {
    let width = rect.width.round().max(1.0) as u32;
    let height = rect.height.round().max(1.0) as u32;
    let scaled = image::imageops::resize(
        brand_image(),
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );

    // GDI quer BGRA de baixo para cima; a `image` da RGBA de cima para baixo.
    let feather = feather_width(width, height);
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for y in (0..height).rev() {
        for x in 0..width {
            let px = scaled.get_pixel(x, y).0;
            let alpha = px[3] as f64 / 255.0
                * feather_alpha(x, y, width, height, feather)
                * art_visibility(px);
            let over = |channel: u8, back: u8| {
                (channel as f64 * alpha + back as f64 * (1.0 - alpha)).round() as u8
            };
            bgra.push(over(px[2], PAGE.2));
            bgra.push(over(px[1], PAGE.1));
            bgra.push(over(px[0], PAGE.0));
            bgra.push(255);
        }
    }

    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: height as i32,
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
    StretchDIBits(
        hdc,
        rect.x.round() as i32,
        rect.y.round() as i32,
        width as i32,
        height as i32,
        0,
        0,
        width as i32,
        height as i32,
        bgra.as_ptr() as *const _,
        &info,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
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
    fn the_border_of_the_art_dissolves_completely_into_the_page() {
        // A arte e opaca. Se a dissolucao nao chegar a zero na borda, fica um
        // retangulo visivel a volta da marca -- foi exatamente o que o dono
        // viu e rejeitou.
        let (w, h) = (240u32, 167u32);
        let feather = feather_width(w, h);
        for (x, y) in [
            (0, 0),
            (w - 1, 0),
            (0, h - 1),
            (w - 1, h - 1),
            (w / 2, 0),
            (0, h / 2),
            (w / 2, h - 1),
            (w - 1, h / 2),
        ] {
            assert_eq!(
                feather_alpha(x, y, w, h, feather),
                0.0,
                "a borda em ({x}, {y}) ainda mostra arte: vai aparecer a caixa"
            );
        }
        // E no meio a arte aparece inteira, senao o que se dissolveu foi ela.
        assert_eq!(feather_alpha(w / 2, h / 2, w, h, feather), 1.0);
    }

    #[test]
    fn no_square_survives_around_the_brand() {
        // O dono rejeitou isto duas vezes: "nao quero contorno quadrado ao
        // redor da logo marca". A arte e opaca, portanto o que nao pode
        // aparecer e o fundo DELA. Percorre-se a moldura da arte real e
        // exige-se que nenhum desses pixeis se veja.
        let art = brand_image();
        let (w, h) = art.dimensions();
        let mut worst = 0.0f64;
        let mut worst_at = (0u32, 0u32);
        for y in 0..h {
            for x in 0..w {
                // A moldura: a faixa exterior de 6% de cada lado.
                let band_x = (w as f64 * 0.06) as u32;
                let band_y = (h as f64 * 0.06) as u32;
                let on_frame = x < band_x || y < band_y || x >= w - band_x || y >= h - band_y;
                if !on_frame {
                    continue;
                }
                let seen = art_visibility(art.get_pixel(x, y).0);
                if seen > worst {
                    worst = seen;
                    worst_at = (x, y);
                }
            }
        }
        assert!(
            worst < 0.02,
            "o fundo da arte ainda se ve a {:.3} em {worst_at:?}: isso desenha \
             o quadrado a volta da marca",
            worst
        );
    }

    #[test]
    fn the_brand_itself_is_not_dissolved_along_with_its_background() {
        // A mascara nao pode comer a marca. O pixel mais claro da arte -- as
        // letras -- tem de aparecer inteiro.
        let art = brand_image();
        let brightest = art
            .pixels()
            .map(|p| art_visibility(p.0))
            .fold(0.0f64, f64::max);
        assert_eq!(
            brightest, 1.0,
            "a marca esta a ser apagada junto com o fundo dela"
        );
    }

    #[test]
    fn the_dissolve_never_goes_backwards_from_the_border_inwards() {
        // Uma rampa que sobe e desce desenha aneis a volta da marca.
        let (w, h) = (300u32, 208u32);
        let feather = feather_width(w, h);
        let mut last = -1.0;
        for x in 0..(w / 2) {
            let value = feather_alpha(x, h / 2, w, h, feather);
            assert!(value >= last, "recuou em x={x}: {last} -> {value}");
            assert!((0.0..=1.0).contains(&value));
            last = value;
        }
    }

    #[test]
    fn a_tiny_brand_still_dissolves_instead_of_dividing_by_zero() {
        // A janela pode ficar minuscula antes de o layout a travar.
        for (w, h) in [(1u32, 1u32), (2, 2), (8, 5)] {
            let feather = feather_width(w, h);
            assert!(feather > 0.0);
            let value = feather_alpha(0, 0, w, h, feather);
            assert!(value.is_finite() && (0.0..=1.0).contains(&value));
        }
    }
}
