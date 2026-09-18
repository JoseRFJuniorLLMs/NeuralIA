#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod windows_app;

#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_app::run()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!(
        "NeuralIA desktop v{} é Windows-first.",
        env!("CARGO_PKG_VERSION")
    );
    println!("O núcleo continua portátil: cargo test -p neural-core");
}
