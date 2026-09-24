//! Os derivados da marca, todos feitos a partir de `assets/neuralia-home.png`.
//!
//! O dono, na 2.1.8: "neuralia-home.png nao tem fundo usa ela para tudo" e
//! "logo.ico novo icone do projeto". O icone que ele deixou -- guardado tal e
//! qual em `scripts/assets-src/logo.owner.ico` -- e essa mesma arte inteira
//! reduzida a UMA entrada de 32x23: nao e quadrada, e a barra de tarefas, o
//! Iniciar e o instalador pedem 16..256 px quadrados, que o Windows tirava
//! dessa unica imagem esticando-a e desfocando-a.
//!
//! Daqui saem, sempre da mesma arte e sem nada desenhado a mao:
//!
//! - `assets/logo.ico`: a arte inteira (esfera, nome e frase) centrada num
//!   quadrado transparente, em [`ICON_SIZES`]; BMP de 32 bits abaixo de
//!   [`PNG_FROM`] px, PNG a partir dai (o formato que o Windows espera em cada
//!   faixa, e o que mantem o ficheiro pequeno);
//! - `assets/neuralia-home-reader.png`: a arte recortada ao que tem tinta, a
//!   [`READER_HEIGHT`] px de altura, para o Reader. O Reader chega ao WebView2
//!   por `NavigateToString`, que recusa mais de 2 MB; a arte original em
//!   base64 seriam ~550 KB em cada pagina, este derivado e uma fracao disso.
//!
//! A reducao e uma media por area com alfa pre-multiplicado, so com somas,
//! produtos e divisoes em `f64`: da o mesmo resultado em qualquer maquina, sem
//! depender do `sin` da libm de cada sistema (o Lanczos depende). E isso que
//! deixa os testes regenerarem tudo em memoria e exigirem igualdade exata com
//! os ficheiros commitados.
//!
//! Regenerar (Windows ou Linux, sem Python):
//!
//! ```text
//! cargo run --locked -p neural-app --example brand-assets            # grava
//! cargo run --locked -p neural-app --example brand-assets -- --check # so confere
//! ```

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

/// Os tamanhos do icone: os do Windows a 100/125/150/200% (16, 20, 24, 32, 40,
/// 48, 64) e os da vista de icones grandes, do Iniciar e do instalador.
pub const ICON_SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];
/// A partir daqui a entrada vai em PNG; abaixo, em BMP de 32 bits com mascara.
pub const PNG_FROM: u32 = 64;
/// Altura, em pixeis, da marca do Reader. A pagina mostra-a a metade disto
/// (72 px CSS), para ficar nitida em ecras de densidade 2x.
pub const READER_HEIGHT: u32 = 144;
/// Alfa a partir do qual um pixel conta como tinta ao recortar a arte. Abaixo
/// disto sao restos invisiveis da remocao do fundo, que so empurrariam a
/// caixa do recorte para fora e deixariam a marca mais pequena no quadrado.
pub const TRIM_ALPHA: u8 = 24;
/// Margem de cada lado do quadrado do icone, em fracao do lado.
pub const ICON_MARGIN: f64 = 1.0 / 32.0;

/// Caixa `(x, y, largura, altura)` do que tem tinta na arte.
pub fn art_bounds(art: &RgbaImage) -> (u32, u32, u32, u32) {
    let (w, h) = art.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for (x, y, pixel) in art.enumerate_pixels() {
        if pixel[3] >= TRIM_ALPHA {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    assert!(x1 > x0 && y1 > y0, "a arte da marca nao tem tinta nenhuma");
    (x0, y0, x1 - x0, y1 - y0)
}

/// Reduz a regiao `(sx, sy, sw, sh)` da arte -- que pode sair dela; fora da
/// arte e transparente -- para `dw` x `dh`, pela media da area que cada pixel
/// de destino cobre. O alfa entra pre-multiplicado: sem isso, a cor dos
/// pixeis transparentes (que nao se ve) escorria para a orla da marca.
pub fn resample_area(art: &RgbaImage, region: (f64, f64, f64, f64), dw: u32, dh: u32) -> RgbaImage {
    let (sx, sy, sw, sh) = region;
    let (aw, ah) = art.dimensions();
    let step_x = sw / dw as f64;
    let step_y = sh / dh as f64;
    let area = step_x * step_y;
    let mut out = RgbaImage::new(dw, dh);
    for dy in 0..dh {
        let y0 = sy + dy as f64 * step_y;
        let y1 = y0 + step_y;
        for dx in 0..dw {
            let x0 = sx + dx as f64 * step_x;
            let x1 = x0 + step_x;
            let mut acc = [0.0f64; 4];
            let first_y = y0.floor().max(0.0) as u32;
            let last_y = (y1.ceil().max(0.0) as u32).min(ah);
            let first_x = x0.floor().max(0.0) as u32;
            let last_x = (x1.ceil().max(0.0) as u32).min(aw);
            for py in first_y..last_y {
                let wy = y1.min(py as f64 + 1.0) - y0.max(py as f64);
                if wy <= 0.0 {
                    continue;
                }
                for px in first_x..last_x {
                    let wx = x1.min(px as f64 + 1.0) - x0.max(px as f64);
                    if wx <= 0.0 {
                        continue;
                    }
                    let weight = wx * wy;
                    let pixel = art.get_pixel(px, py).0;
                    let alpha = pixel[3] as f64 * weight;
                    acc[0] += pixel[0] as f64 * alpha;
                    acc[1] += pixel[1] as f64 * alpha;
                    acc[2] += pixel[2] as f64 * alpha;
                    acc[3] += alpha;
                }
            }
            let alpha = (acc[3] / area).round().clamp(0.0, 255.0) as u8;
            let color = |sum: f64| {
                if alpha == 0 {
                    0
                } else {
                    (sum / acc[3]).round().clamp(0.0, 255.0) as u8
                }
            };
            out.put_pixel(
                dx,
                dy,
                Rgba([color(acc[0]), color(acc[1]), color(acc[2]), alpha]),
            );
        }
    }
    out
}

/// Uma entrada do icone: a arte inteira, centrada num quadrado transparente.
pub fn icon_image(art: &RgbaImage, size: u32) -> RgbaImage {
    let (bx, by, bw, bh) = art_bounds(art);
    let side = bw.max(bh) as f64 / (1.0 - 2.0 * ICON_MARGIN);
    let cx = bx as f64 + bw as f64 / 2.0;
    let cy = by as f64 + bh as f64 / 2.0;
    resample_area(
        art,
        (cx - side / 2.0, cy - side / 2.0, side, side),
        size,
        size,
    )
}

/// A marca do Reader: a arte recortada ao que tem tinta, a `READER_HEIGHT`.
pub fn reader_image(art: &RgbaImage) -> RgbaImage {
    let (bx, by, bw, bh) = art_bounds(art);
    let width = ((READER_HEIGHT as f64 * bw as f64 / bh as f64).round() as u32).max(1);
    resample_area(
        art,
        (bx as f64, by as f64, bw as f64, bh as f64),
        width,
        READER_HEIGHT,
    )
}

pub fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgba8,
        )
        .expect("codificar PNG em memoria");
    out
}

/// Bytes por linha da mascara AND de um icone BMP: 1 bit por pixel,
/// alinhado a 32 bits.
fn mask_stride(width: u32) -> usize {
    width.div_ceil(32) as usize * 4
}

/// Entrada BMP de um `.ico`: BITMAPINFOHEADER com a altura dobrada, pixeis
/// BGRA de baixo para cima (alfa direito, nao pre-multiplicado) e a mascara
/// AND, com 1 onde o pixel e totalmente transparente.
pub fn encode_bmp_entry(image: &RgbaImage) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let stride = mask_stride(w);
    let xor_len = (w * h * 4) as usize;
    let mut out = Vec::with_capacity(40 + xor_len + stride * h as usize);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&((h * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((xor_len + stride * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..h).rev() {
        for x in 0..w {
            let p = image.get_pixel(x, y).0;
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    for y in (0..h).rev() {
        let mut row = vec![0u8; stride];
        for x in 0..w {
            if image.get_pixel(x, y)[3] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

/// O inverso de [`encode_bmp_entry`], para 32 bits. Devolve `None` para o
/// que nao for um BMP de icone de 32 bits bem formado.
pub fn decode_bmp_entry(data: &[u8]) -> Option<RgbaImage> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let u16_at = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
    if u32_at(0)? != 40 || u16_at(14)? != 32 || u32_at(16)? != 0 {
        return None;
    }
    let w = i32::from_le_bytes(data.get(4..8)?.try_into().ok()?);
    let doubled = i32::from_le_bytes(data.get(8..12)?.try_into().ok()?);
    if w <= 0 || doubled <= 0 || doubled % 2 != 0 {
        return None;
    }
    let (w, h) = (w as u32, (doubled / 2) as u32);
    let pixels = data.get(40..40 + (w * h * 4) as usize)?;
    let mut image = RgbaImage::new(w, h);
    for (row, line) in pixels.chunks_exact((w * 4) as usize).enumerate() {
        let y = h - 1 - row as u32;
        for (x, p) in line.chunks_exact(4).enumerate() {
            image.put_pixel(x as u32, y, Rgba([p[2], p[1], p[0], p[3]]));
        }
    }
    Some(image)
}

/// O `.ico` inteiro: ICONDIR, uma ICONDIRENTRY por tamanho e as imagens.
pub fn build_ico(art: &RgbaImage) -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = ICON_SIZES
        .iter()
        .map(|&size| {
            let image = icon_image(art, size);
            let data = if size >= PNG_FROM {
                encode_png(&image)
            } else {
                encode_bmp_entry(&image)
            };
            (size, data)
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // icone, nao cursor
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, data) in &images {
        // 256 escreve-se 0 num byte.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in &images {
        out.extend_from_slice(data);
    }
    out
}

/// Uma imagem dentro de um `.ico`, como o diretorio a descreve.
#[derive(Debug, Clone)]
pub struct IcoEntry<'a> {
    pub width: u32,
    pub height: u32,
    pub bit_count: u16,
    pub data: &'a [u8],
}

impl IcoEntry<'_> {
    pub fn is_png(&self) -> bool {
        self.data.starts_with(b"\x89PNG\r\n\x1a\n")
    }

    /// A imagem, seja PNG ou BMP de 32 bits.
    pub fn decode(&self) -> Option<RgbaImage> {
        if self.is_png() {
            image::load_from_memory_with_format(self.data, image::ImageFormat::Png)
                .ok()
                .map(|image| image.to_rgba8())
        } else {
            decode_bmp_entry(self.data)
        }
    }
}

pub fn parse_ico(bytes: &[u8]) -> Result<Vec<IcoEntry<'_>>, String> {
    let u16_at = |at: usize| {
        bytes
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| format!("ico truncado em {at}"))
    };
    let u32_at = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| format!("ico truncado em {at}"))
    };
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return Err("nao e um ICONDIR de icone".into());
    }
    let count = u16_at(4)? as usize;
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let at = 6 + 16 * index;
        let side = |byte: u8| if byte == 0 { 256 } else { byte as u32 };
        let width = side(*bytes.get(at).ok_or("entrada truncada")?);
        let height = side(*bytes.get(at + 1).ok_or("entrada truncada")?);
        let bit_count = u16_at(at + 6)?;
        let len = u32_at(at + 8)? as usize;
        let offset = u32_at(at + 12)? as usize;
        let data = bytes
            .get(offset..offset + len)
            .ok_or_else(|| format!("entrada {index} aponta para fora do ficheiro"))?;
        entries.push(IcoEntry {
            width,
            height,
            bit_count,
            data,
        });
    }
    Ok(entries)
}

/// Uma linha do GRPICONDIR que o compilador de recursos poe no executavel: a
/// mesma ICONDIRENTRY, mas com o id do recurso RT_ICON em vez do offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEntry {
    pub width: u32,
    pub height: u32,
    pub bit_count: u16,
    pub bytes: u32,
    pub id: u16,
}

pub fn parse_group_icon(bytes: &[u8]) -> Result<Vec<GroupEntry>, String> {
    let u16_at = |at: usize| {
        bytes
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| format!("GRPICONDIR truncado em {at}"))
    };
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return Err("nao e um GRPICONDIR de icone".into());
    }
    let count = u16_at(4)? as usize;
    (0..count)
        .map(|index| {
            let at = 6 + 14 * index;
            let side = |byte: u8| if byte == 0 { 256 } else { byte as u32 };
            let raw = bytes
                .get(at..at + 14)
                .ok_or_else(|| format!("GRPICONDIR truncado na entrada {index}"))?;
            Ok(GroupEntry {
                width: side(raw[0]),
                height: side(raw[1]),
                bit_count: u16::from_le_bytes([raw[6], raw[7]]),
                bytes: u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]),
                id: u16::from_le_bytes([raw[12], raw[13]]),
            })
        })
        .collect()
}
