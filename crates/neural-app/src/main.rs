#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

// O parser do canal IPC (SPEC-0108) não tem uma única chamada ao Windows: é
// JSON, validação de argumentos e comparação em tempo constante. Estava atrás
// de `cfg(target_os = "windows")` por arrastamento, o que deixava a superfície
// mais sensível do produto sem ser compilada nem testada fora do Windows.
// Fora do Windows ninguém o chama ainda; daí o `allow(dead_code)`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod ipc;
#[cfg(target_os = "windows")]
mod pdf_assets;
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
    println!("O parser do canal IPC também: cargo test -p neural-app");
}
