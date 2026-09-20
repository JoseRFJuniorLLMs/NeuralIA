//! Mete a NeuralIA dentro do instalador.
//!
//! A carga util vem de uma pasta apontada por `NEURALIA_PAYLOAD_DIR`. Sem essa
//! variavel -- um `cargo build` normal, um `cargo test`, o rust-analyzer -- sai
//! um pacote vazio e o instalador diz, em vez de instalar uma pasta vazia.
//!
//! O empacotador e o mesmo ficheiro que o instalador usa para desempacotar,
//! incluido aqui a letra. Se fossem duas copias, a que os testes exercitam nao
//! seria a que produz o artefacto.

use std::path::{Path, PathBuf};

#[allow(dead_code)]
mod archive {
    include!("src/archive.rs");
}
use archive::{Entry, is_safe_relative_path, pack};

fn main() {
    println!("cargo:rerun-if-env-changed=NEURALIA_PAYLOAD_DIR");
    println!("cargo:rerun-if-changed=src/archive.rs");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let payload = out_dir.join("payload.bin");

    let entries = match std::env::var("NEURALIA_PAYLOAD_DIR") {
        Ok(dir) if !dir.trim().is_empty() => {
            let root = PathBuf::from(dir);
            println!("cargo:rerun-if-changed={}", root.display());
            let mut entries = Vec::new();
            collect(&root, &root, &mut entries);
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            if entries.is_empty() {
                panic!(
                    "NEURALIA_PAYLOAD_DIR={} nao tem ficheiro nenhum",
                    root.display()
                );
            }
            let total: usize = entries.iter().map(|e| e.data.len()).sum();
            println!(
                "cargo:warning=carga util: {} ficheiros, {:.1} MiB",
                entries.len(),
                total as f64 / (1024.0 * 1024.0)
            );
            entries
        }
        _ => Vec::new(),
    };

    let packed = pack(&entries).expect("empacotar a carga util");
    std::fs::write(&payload, packed).expect("gravar payload.bin");

    windows_resources();
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<Entry>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let path = item.path();
        if path.is_dir() {
            collect(root, &path, out);
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("dentro da raiz")
            .to_string_lossy()
            .replace('\\', "/");
        if !is_safe_relative_path(&relative) {
            panic!("carga util com um caminho que o instalador recusaria: {relative}");
        }
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("ler {}: {e}", path.display()));
        out.push(Entry {
            path: relative,
            data,
        });
    }
}

#[cfg(target_os = "windows")]
fn windows_resources() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let icon = Path::new(&manifest_dir).join("../../assets/logo.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let mut res = winres::WindowsResource::new();
    res.set_icon(icon.to_str().expect("caminho do icone"));
    res.set("ProductName", "NeuralIA");
    res.set("FileDescription", "Instalador da NeuralIA");
    res.set("LegalCopyright", "© 2026 Jose Ribamar Ferreira Junior");
    // Sem isto o Windows pede elevacao a qualquer executavel chamado
    // "setup"/"install" -- e a instalacao e por utilizador, nao precisa.
    res.set_manifest(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#,
    );
    if let Err(e) = res.compile() {
        let profile = std::env::var("PROFILE").unwrap_or_default();
        if profile == "release" {
            panic!("recursos Windows falharam em release: {e}");
        }
        println!("cargo:warning=recursos Windows falharam: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn windows_resources() {}
