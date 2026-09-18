#[cfg(target_os = "windows")]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("../../assets/logo.ico");
    res.set("ProductName", "NeuralIA");
    res.set("FileDescription", "NeuralIA Desktop");
    res.set("LegalCopyright", "© 2026 Jose Ribamar Ferreira Junior");
    if let Err(e) = res.compile() {
        eprintln!("cargo:warning=Failed to compile Windows resources: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {}
