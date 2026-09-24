//! A marca e o icone do projeto (2.1.8).
//!
//! O dono apagou as artes com fundo quadrado e deixou duas coisas: a
//! `assets/neuralia-home.png`, "usa ela para tudo", e o `assets/logo.ico`,
//! "novo icone do projeto". Estes testes prendem o que isso quer dizer no que
//! embarca: o icone tem todos os tamanhos, quadrados e com a arte; e a arte
//! regenerada pelo gerador commitado; o NeuralIA.exe compilado traz esse
//! icone; e nada no codigo volta a incluir os ficheiros apagados.

#[allow(dead_code)]
#[path = "../../../scripts/brand-assets/brand.rs"]
mod brand;
#[cfg(windows)]
#[path = "../../../scripts/brand-assets/exe_icon.rs"]
mod exe_icon;

use std::path::{Path, PathBuf};

use image::RgbaImage;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> Vec<u8> {
    std::fs::read(root().join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

fn art() -> RgbaImage {
    image::load_from_memory(&read("assets/neuralia-home.png"))
        .expect("assets/neuralia-home.png e um PNG")
        .to_rgba8()
}

/// Onde as duas imagens diferem, para a mensagem de falha dizer quanto.
fn differing_pixels(a: &RgbaImage, b: &RgbaImage) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels().zip(b.pixels()).filter(|(x, y)| x != y).count()
}

#[test]
fn the_project_icon_has_every_size_square_and_not_blank() {
    let bytes = read("assets/logo.ico");
    let entries = brand::parse_ico(&bytes).expect("assets/logo.ico e um .ico");
    let sizes: Vec<u32> = entries.iter().map(|entry| entry.width).collect();
    assert_eq!(
        sizes,
        brand::ICON_SIZES,
        "o icone do projeto tem de trazer os tamanhos que o Windows pede"
    );
    for entry in &entries {
        let side = entry.width;
        assert_eq!(entry.height, side, "entrada de {side} px nao e quadrada");
        assert_eq!(entry.bit_count, 32, "entrada de {side} px sem alfa");
        assert_eq!(
            entry.is_png(),
            side >= brand::PNG_FROM,
            "entrada de {side} px no formato errado (PNG so de {} px para cima)",
            brand::PNG_FROM
        );
        let image = entry
            .decode()
            .unwrap_or_else(|| panic!("a entrada de {side} px nao se descodifica"));
        assert_eq!(image.dimensions(), (side, side));

        // Sem quadrado: os cantos sao fundo transparente.
        for (x, y) in [(0, 0), (side - 1, 0), (0, side - 1), (side - 1, side - 1)] {
            assert_eq!(
                image.get_pixel(x, y)[3],
                0,
                "a entrada de {side} px tem fundo no canto ({x}, {y})"
            );
        }
        // Nao esta em branco: a arte ocupa uma parte real do quadrado (~27%
        // em todos os tamanhos), e e a arte azul da marca (~22%), nao uma
        // mancha de uma cor so.
        let area = (side * side) as usize;
        let ink = image.pixels().filter(|p| p[3] >= 128).count();
        let blue = image
            .pixels()
            .filter(|p| p[3] >= 128 && p[2] as i32 > p[0] as i32 + 60)
            .count();
        assert!(
            ink * 100 >= area * 20,
            "a entrada de {side} px so tem {ink} de {area} pixeis com tinta"
        );
        assert!(
            blue * 100 >= area * 15,
            "a entrada de {side} px so tem {blue} de {area} pixeis azuis da marca"
        );
    }
}

#[test]
fn the_project_icon_is_the_brand_art_regenerated() {
    // O icone nao e desenhado a mao: cada entrada tem de ser, pixel a pixel,
    // o que o gerador commitado tira da neuralia-home.png. Se a arte mudar e o
    // icone nao for regenerado, ou vice-versa, isto fica vermelho.
    let art = art();
    let bytes = read("assets/logo.ico");
    for entry in brand::parse_ico(&bytes).expect("assets/logo.ico e um .ico") {
        let committed = entry.decode().expect("entrada descodificavel");
        let expected = brand::icon_image(&art, entry.width);
        let differing = differing_pixels(&committed, &expected);
        assert_eq!(
            differing, 0,
            "a entrada de {} px difere da arte em {differing} pixeis: \
             cargo run --locked -p neural-app --example brand-assets",
            entry.width
        );
    }
}

#[test]
fn the_reader_brand_is_the_brand_art_regenerated() {
    let committed = image::load_from_memory(&read("assets/neuralia-home-reader.png"))
        .expect("assets/neuralia-home-reader.png e um PNG")
        .to_rgba8();
    assert_eq!(committed.height(), brand::READER_HEIGHT);
    let expected = brand::reader_image(&art());
    let differing = differing_pixels(&committed, &expected);
    assert_eq!(
        differing, 0,
        "a marca do Reader difere da arte em {differing} pixeis: \
         cargo run --locked -p neural-app --example brand-assets"
    );
}

#[test]
fn the_owners_icon_is_kept_and_is_the_same_art() {
    // A proveniencia: o icone que o dono deixou fica guardado tal e qual, e
    // e mesmo a arte da neuralia-home.png reduzida -- o icone gerado tem o
    // desenho dele, so que quadrado e em todos os tamanhos.
    let bytes = read("scripts/assets-src/logo.owner.ico");
    let entries = brand::parse_ico(&bytes).expect("o icone do dono e um .ico");
    assert_eq!(entries.len(), 1);
    let owner = entries[0]
        .decode()
        .expect("a entrada do dono descodifica-se");
    assert_eq!(owner.dimensions(), (32, 23));

    let (w, h) = owner.dimensions();
    let art = art();
    let distance = |candidate: &RgbaImage| {
        // Composto sobre o fundo escuro do instalador: o que se ve.
        let seen = |p: &image::Rgba<u8>, c: usize| {
            let a = p[3] as f64 / 255.0;
            p[c] as f64 * a + [8.0, 11.0, 22.0][c] * (1.0 - a)
        };
        let total: f64 = owner
            .pixels()
            .zip(candidate.pixels())
            .map(|(o, c)| (0..3).map(|k| (seen(o, k) - seen(c, k)).abs()).sum::<f64>())
            .sum();
        total / (w * h * 3) as f64
    };
    let full = |image: &RgbaImage| {
        let (iw, ih) = image.dimensions();
        brand::resample_area(image, (0.0, 0.0, iw as f64, ih as f64), w, h)
    };
    let same_art = distance(&full(&art));
    // Medido: 3,0 para a neuralia-home.png e 25,6 para o controlo -- outra
    // imagem do projeto, reduzida da mesma maneira.
    let other = image::load_from_memory(&read("assets/ai/gemini.png"))
        .expect("assets/ai/gemini.png")
        .to_rgba8();
    let other_art = distance(&full(&other));
    assert!(
        same_art < 8.0 && same_art * 3.0 < other_art,
        "o icone do dono nao parece a neuralia-home.png reduzida: \
         distancia {same_art:.1} (outra arte: {other_art:.1})"
    );
}

#[cfg(windows)]
#[test]
fn neuralia_exe_carries_the_project_icon() {
    // O `build.rs` compila o assets/logo.ico no NeuralIA.exe como grupo 1, que
    // e o que o Explorador, a barra de tarefas e o atalho mostram e o que a
    // janela carrega. Le-se do executavel ja compilado, nao do ficheiro.
    let exe = Path::new(env!("CARGO_BIN_EXE_NeuralIA"));
    let bytes = read("assets/logo.ico");
    let expected = brand::parse_ico(&bytes).expect("assets/logo.ico e um .ico");
    let embedded = exe_icon::group_icon(exe, 1).expect("o NeuralIA.exe tem de trazer o icone");
    assert_eq!(
        embedded.len(),
        expected.len(),
        "o NeuralIA.exe traz {} imagens de icone, o assets/logo.ico tem {}",
        embedded.len(),
        expected.len()
    );
    for ((group, data), entry) in embedded.iter().zip(&expected) {
        assert_eq!(
            (group.width, group.height, group.bit_count),
            (entry.width, entry.height, entry.bit_count)
        );
        assert!(
            data.as_slice() == entry.data,
            "a imagem de {} px no NeuralIA.exe nao e a do assets/logo.ico",
            entry.width
        );
    }
}

/// Os ficheiros de marca que o dono apagou por terem fundo quadrado.
const DELETED: [&str; 3] = [
    "assets/logo.png",
    "assets/neuralia-brand.jpg",
    "assets/neuralia-logo.svg",
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let path = item.path();
        let name = item.file_name();
        if path.is_dir() {
            if name != "target" && name != ".git" {
                walk(&path, out);
            }
        } else if path.extension().is_some_and(|ext| {
            ["rs", "toml", "ps1", "mjs", "yml", "html", "css", "js"]
                .iter()
                .any(|known| ext == *known)
        }) {
            out.push(path);
        }
    }
}

#[test]
fn nothing_includes_the_brand_files_the_owner_deleted() {
    // So proibe a presenca: nao prova que a marca aparece (isso e dos testes
    // de cima e dos do instalador e do Reader). Mas um `include_bytes!` de um
    // destes caminhos voltaria a meter no produto a arte com fundo quadrado
    // no dia em que alguem repusesse o ficheiro.
    let root = root();
    for deleted in DELETED {
        assert!(
            !root.join(deleted).exists(),
            "{deleted} voltou: o dono apagou-o por ter fundo quadrado"
        );
    }
    // Este ficheiro nomeia-os, para os proibir.
    let this_file = root.join("crates/neural-app/tests/brand_assets.rs");
    let mut files = Vec::new();
    for dir in ["crates", "scripts", ".github"] {
        walk(&root.join(dir), &mut files);
    }
    files.push(root.join("README.md"));
    assert!(
        files.len() > 20,
        "a busca nao encontrou o codigo: {files:?}"
    );
    let mut offenders = Vec::new();
    for file in files {
        if file == this_file {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for deleted in DELETED {
            if text.contains(deleted) {
                offenders.push(format!("{} -> {deleted}", file.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "ainda se referem ficheiros de marca apagados: {offenders:#?}"
    );
}
