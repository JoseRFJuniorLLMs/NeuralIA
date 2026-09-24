//! Gera `assets/logo.ico` e `assets/neuralia-home-reader.png` a partir de
//! `assets/neuralia-home.png`. O porque e o como estao em `brand.rs`.
//!
//! ```text
//! cargo run --locked -p neural-app --example brand-assets            # grava
//! cargo run --locked -p neural-app --example brand-assets -- --check # so confere
//! ```

// O parser do `.ico` e do GRPICONDIR e para os testes; aqui so se gera.
#[allow(dead_code)]
mod brand;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn main() -> ExitCode {
    let check = std::env::args().any(|arg| arg == "--check");
    let assets = root().join("assets");
    let art = match image::open(assets.join("neuralia-home.png")) {
        Ok(art) => art.to_rgba8(),
        Err(error) => {
            eprintln!("assets/neuralia-home.png: {error}");
            return ExitCode::FAILURE;
        }
    };

    let (bx, by, bw, bh) = brand::art_bounds(&art);
    println!(
        "arte {}x{}, tinta em {bw}x{bh} a partir de ({bx}, {by})",
        art.width(),
        art.height()
    );

    let outputs = [
        ("logo.ico", brand::build_ico(&art)),
        (
            "neuralia-home-reader.png",
            brand::encode_png(&brand::reader_image(&art)),
        ),
    ];

    let mut stale = false;
    for (name, bytes) in &outputs {
        let path = assets.join(name);
        if check {
            let same = std::fs::read(&path).is_ok_and(|current| current == *bytes);
            println!(
                "assets/{name}: {}",
                if same { "igual" } else { "DIFERENTE" }
            );
            stale |= !same;
        } else if let Err(error) = std::fs::write(&path, bytes) {
            eprintln!("assets/{name}: {error}");
            return ExitCode::FAILURE;
        } else {
            println!("assets/{name}: {} bytes", bytes.len());
        }
    }
    if stale {
        eprintln!("regenerar: cargo run --locked -p neural-app --example brand-assets");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
