#[cfg(target_os = "windows")]
mod windows_app;

#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_app::run()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("NeuralIA desktop v0.1 é Windows-first.");
    println!("O núcleo continua portátil: cargo test -p neural-core");
}
