//! A proveniência do que vendorizamos tem de ser verificável, não declarada.
//!
//! O `release.yml` escreve no SBOM (CycloneDX) um componente `pdf.js` com a
//! versão à mão. Sem nada a ligar as pontas, uma atualização do `pdf.mjs`
//! deixava o SBOM publicado a declarar uma versão que já não embarca — uma
//! afirmação falsa dentro de um artefacto de release.
//!
//! Este teste obriga três sítios a dizer o mesmo número: o que está dentro do
//! `pdf.mjs`, o que está em `assets/pdfjs/UPSTREAM.md`, e o que o `release.yml`
//! põe no SBOM.

use std::{fs, path::PathBuf};

use sha2::{Digest, Sha256};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// O valor entre aspas a seguir à primeira ocorrência de `needle`.
fn quoted_after(haystack: &str, needle: &str) -> Option<String> {
    let rest = haystack.split_once(needle)?.1;
    let rest = rest.split_once('"')?.1;
    rest.split_once('"').map(|(value, _)| value.to_string())
}

#[test]
fn vendored_pdfjs_version_matches_provenance_and_release_sbom() {
    let shipped = read("assets/pdfjs/pdf.mjs");
    let shipped_version = quoted_after(&shipped, "const version =")
        .expect("pdf.mjs declara a sua versão numa `const version`");

    let upstream = read("assets/pdfjs/UPSTREAM.md");
    assert!(
        upstream.contains(&shipped_version),
        "UPSTREAM.md não menciona a versão que embarca ({shipped_version})"
    );

    let release = read(".github/workflows/release.yml");
    let component = release
        .split_once("name = \"pdf.js\"")
        .expect("release.yml acrescenta um componente pdf.js ao SBOM")
        .1;
    let sbom_version =
        quoted_after(component, "version =").expect("esse componente declara uma versão");
    assert_eq!(
        sbom_version, shipped_version,
        "o SBOM publicaria {sbom_version} para um pdf.js {shipped_version}"
    );
}

#[test]
fn vendored_pdfjs_files_match_the_recorded_hashes() {
    let upstream = read("assets/pdfjs/UPSTREAM.md");

    for name in ["pdf.mjs", "pdf.worker.mjs", "LICENSE"] {
        let path = repo_root().join("assets/pdfjs").join(name);
        let bytes = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert!(
            upstream.contains(&digest),
            "{name} tem SHA-256 {digest}, que não está em UPSTREAM.md"
        );
    }
}


fn listed_files(relative_dir: &str) -> Vec<String> {
    let root = repo_root();
    let dir = root.join(relative_dir);
    let mut files = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
                .path()
        })
        .filter(|path| path.is_file())
        .map(|path| {
            path.strip_prefix(&root)
                .expect("ficheiro dentro do repositório")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[test]
fn pdfjs_auxiliary_assets_match_the_recorded_manifest() {
    let upstream = read("assets/pdfjs/UPSTREAM.md");
    let manifest = read("assets/pdfjs/AUXILIARY_BLOBS.md");
    let source_commit = "1c8020a7d4e43668ac287a3ecf9a8dbea17e4c56";

    assert!(upstream.contains(source_commit));
    assert!(manifest.contains(source_commit));

    let families = [
        ("assets/pdfjs/cmaps", 168usize),
        ("assets/pdfjs/standard_fonts", 14usize),
        ("assets/pdfjs/wasm", 5usize),
        ("assets/pdfjs/icc", 1usize),
        ("assets/pdfjs/licenses", 10usize),
        ("assets/pdfjs/fixtures", 5usize),
    ];

    for (directory, expected_count) in families {
        let files = listed_files(directory);
        assert_eq!(
            files.len(),
            expected_count,
            "{directory} devia conter exatamente {expected_count} ficheiros"
        );
        for file in files {
            assert!(
                manifest.contains(&format!("`{file}`")),
                "{file} existe no pacote mas não está no manifesto upstream"
            );
        }
    }

    for critical in [
        "assets/pdfjs/wasm/openjpeg.wasm",
        "assets/pdfjs/wasm/jbig2.wasm",
        "assets/pdfjs/wasm/qcms_bg.wasm",
        "assets/pdfjs/cmaps/Adobe-Japan1-UCS2.bcmap",
        "assets/pdfjs/standard_fonts/LiberationSans-Regular.ttf",
        "assets/pdfjs/icc/CGATS001Compat-v2-micro.icc",
    ] {
        assert!(
            manifest.contains(&format!("`{critical}`")),
            "asset crítico sem proveniência: {critical}"
        );
    }
}
