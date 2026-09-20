//! Instalador da NeuralIA.
//!
//! Um executavel so: com a NeuralIA dentro dele, instala; copiado para a pasta
//! de instalacao e corrido com `--uninstall`, remove.

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
// Mesma escolha do `neural-app`: as funcoes `unsafe` deste crate sao paredes
// de chamadas Win32, e marcar cada uma outra vez so acrescentava ruido.
#![allow(unsafe_op_in_unsafe_fn)]

mod archive;
mod install;
mod ui;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod paint;
#[cfg(windows)]
mod winshell;

/// O que o utilizador pediu na linha de comandos. `/S` e o silencioso de
/// sempre nos desinstaladores do Windows, e e o que a chave
/// `QuietUninstallString` manda -- por isso tem de ser entendido.
#[cfg(windows)]
fn mode_from(args: &[String]) -> app::Mode {
    let wants_removal = args.iter().any(|arg| {
        let arg = arg.trim_start_matches(['-', '/']).to_ascii_lowercase();
        arg == "uninstall" || arg == "u" || arg == "remove"
    });
    if wants_removal {
        app::Mode::Uninstall
    } else {
        app::Mode::Install
    }
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    app::run(mode_from(&args));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("O instalador da NeuralIA e para Windows.");
    std::process::exit(1);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_uninstall_flag_is_understood_in_every_shape_windows_uses() {
        // A `QuietUninstallString` que escrevemos no registo passa `/S` depois
        // do `--uninstall`; se so se lesse o primeiro argumento, o Windows
        // mandava desinstalar e o programa instalava por cima.
        for args in [
            vec!["--uninstall".to_string()],
            vec!["/uninstall".to_string()],
            vec!["-u".to_string()],
            vec!["--uninstall".to_string(), "/S".to_string()],
            vec!["/S".to_string(), "--uninstall".to_string()],
            vec!["--UNINSTALL".to_string()],
        ] {
            assert_eq!(
                mode_from(&args),
                app::Mode::Uninstall,
                "{args:?} devia desinstalar"
            );
        }
        for args in [vec![], vec!["/S".to_string()], vec!["outra".to_string()]] {
            assert_eq!(
                mode_from(&args),
                app::Mode::Install,
                "{args:?} devia instalar"
            );
        }
    }
}
