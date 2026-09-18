#[cfg(target_os = "windows")]
fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let icon_path = std::path::Path::new(&manifest_dir).join("../../assets/logo.ico");
    println!("cargo:rerun-if-changed={}", icon_path.display());

    let mut res = winres::WindowsResource::new();
    res.set_icon(icon_path.to_str().unwrap());
    res.set("ProductName", "NeuralIA");
    res.set("FileDescription", "NeuralIA Desktop");
    res.set("LegalCopyright", "© 2026 Jose Ribamar Ferreira Junior");
    if let Err(e) = res.compile() {
        eprintln!("cargo:warning=Failed to compile Windows resources: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {}
