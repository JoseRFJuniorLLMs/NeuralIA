//! O instalador traz o icone do projeto.
//!
//! E o icone que o Explorador mostra no NeuralIA-Setup.exe descarregado e o
//! da barra de tarefas enquanto instala. O `build.rs` compila la o
//! `assets/logo.ico`; le-se do executavel ja compilado, nao do ficheiro.

#![cfg(windows)]

#[allow(dead_code)]
#[path = "../../../scripts/brand-assets/brand.rs"]
mod brand;
#[path = "../../../scripts/brand-assets/exe_icon.rs"]
mod exe_icon;

use std::path::Path;

#[test]
fn the_installer_exe_carries_the_project_icon() {
    let exe = Path::new(env!("CARGO_BIN_EXE_NeuralIA-Setup"));
    let ico = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/logo.ico"
    ))
    .expect("assets/logo.ico");
    let expected = brand::parse_ico(&ico).expect("assets/logo.ico e um .ico");
    let embedded = exe_icon::group_icon(exe, 1).expect("o instalador tem de trazer o icone");
    assert_eq!(
        embedded.len(),
        expected.len(),
        "o instalador traz {} imagens de icone, o assets/logo.ico tem {}",
        embedded.len(),
        expected.len()
    );
    for ((group, data), entry) in embedded.iter().zip(&expected) {
        assert_eq!(
            (group.width, group.height, group.bit_count),
            (entry.width, entry.height, entry.bit_count)
        );
        assert!(
            data.as_slice() == entry.data,
            "a imagem de {} px no instalador nao e a do assets/logo.ico",
            entry.width
        );
    }
}
