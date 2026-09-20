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

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

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

fn auxiliary_manifest_rows(manifest: &str) -> BTreeMap<String, (String, u64)> {
    manifest
        .lines()
        .filter_map(|line| {
            let cells = line
                .split('|')
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .collect::<Vec<_>>();
            if cells.len() != 4 {
                return None;
            }
            let path = cells[0].strip_prefix('`')?.strip_suffix('`')?;
            let hash = cells[2].strip_prefix('`')?.strip_suffix('`')?;
            if !path.starts_with("assets/pdfjs/")
                || hash.len() != 40
                || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            let size = cells[3].parse::<u64>().ok()?;
            Some((path.to_string(), (hash.to_ascii_lowercase(), size)))
        })
        .collect()
}

fn git_blob_hashes(paths: impl Iterator<Item = String>) -> Vec<String> {
    let root = repo_root();
    let paths = paths.collect::<Vec<_>>();
    let mut child = Command::new("git")
        .args(["hash-object", "--no-filters", "--stdin-paths"])
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git hash-object disponível para conferir blobs vendorizados");
    {
        let stdin = child.stdin.as_mut().expect("stdin de git hash-object");
        for path in &paths {
            writeln!(stdin, "{path}").expect("enviar caminho a git hash-object");
        }
    }
    let output = child
        .wait_with_output()
        .expect("aguardar git hash-object dos blobs vendorizados");
    assert!(
        output.status.success(),
        "git hash-object falhou: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hashes = String::from_utf8(output.stdout)
        .expect("hashes Git em ASCII")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        hashes.len(),
        paths.len(),
        "git hash-object deve devolver um hash por asset"
    );
    hashes
}

#[test]
fn pdfjs_auxiliary_assets_match_the_recorded_manifest() {
    let upstream = read("assets/pdfjs/UPSTREAM.md");
    let manifest = read("assets/pdfjs/AUXILIARY_BLOBS.md");
    let source_commit = "1c8020a7d4e43668ac287a3ecf9a8dbea17e4c56";

    assert!(upstream.contains(source_commit));
    assert!(manifest.contains(source_commit));
    let rows = auxiliary_manifest_rows(&manifest);

    let families = [
        ("assets/pdfjs/cmaps", 168usize),
        ("assets/pdfjs/standard_fonts", 14usize),
        ("assets/pdfjs/wasm", 5usize),
        ("assets/pdfjs/icc", 1usize),
        ("assets/pdfjs/licenses", 10usize),
        ("assets/pdfjs/fixtures", 6usize),
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
                rows.contains_key(&file),
                "{file} existe no pacote mas não está no manifesto upstream"
            );
        }
    }

    assert_eq!(
        rows.len(),
        204,
        "o manifesto deve conter exatamente todos os 204 blobs auxiliares"
    );
    let hashes = git_blob_hashes(rows.keys().cloned());
    for ((path, (expected_hash, expected_size)), actual_hash) in rows.iter().zip(hashes) {
        let full_path = repo_root().join(path);
        let actual_size = fs::metadata(&full_path)
            .unwrap_or_else(|error| panic!("{}: {error}", full_path.display()))
            .len();
        assert_eq!(
            actual_size, *expected_size,
            "{path} não tem o tamanho upstream registrado"
        );
        assert_eq!(
            actual_hash, *expected_hash,
            "{path} não tem os bytes do Git blob upstream registrado"
        );
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
            rows.contains_key(critical),
            "asset crítico sem proveniência: {critical}"
        );
    }
}

#[test]
fn release_sbom_names_every_pdfjs_auxiliary_family() {
    let release = read(".github/workflows/release.yml");
    for marker in [
        "pdf.js OpenJPEG decoder",
        "BSD-2-Clause",
        "pdf.js JBIG2 decoder",
        "pdf.js QCMS decoder",
        "Adobe binary CMaps",
        "Foxit standard fonts",
        "Liberation Sans",
        "1.07.4",
        "CGATS001Compat-v2-micro ICC profile",
        "CC0-1.0",
        "assets/pdfjs/AUXILIARY_BLOBS.md",
    ] {
        assert!(
            release.contains(marker),
            "SBOM de release não declara o componente/proveniência: {marker}"
        );
    }
}
