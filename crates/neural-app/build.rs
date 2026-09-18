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
    // Em release o recurso PE (icone, ProductName, copyright) faz parte do
    // artefacto: falhar aqui em silencio ja produziu um binario publicado sem
    // icone. Em debug continua a ser so um aviso, para nao travar o dia a dia.
    if let Err(e) = res.compile() {
        let profile = std::env::var("PROFILE").unwrap_or_default();
        if profile == "release" {
            panic!("Windows resources failed to compile in release: {e}");
        }
        println!("cargo:warning=Failed to compile Windows resources: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {}
