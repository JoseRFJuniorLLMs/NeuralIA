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

    /// Parte uma linha de comandos como o Windows a entrega ao processo, e
    /// devolve os argumentos sem o executavel.
    fn argv_after_exe(command: &str) -> Vec<String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
        let wide: Vec<u16> = command.encode_utf16().chain([0]).collect();
        let mut count = 0i32;
        unsafe {
            let argv = CommandLineToArgvW(wide.as_ptr(), &mut count);
            assert!(!argv.is_null(), "linha de comandos invalida: {command}");
            let args = (1..count as usize)
                .map(|i| {
                    let arg = *argv.add(i);
                    let len = (0..).take_while(|&n| *arg.add(n) != 0).count();
                    String::from_utf16_lossy(std::slice::from_raw_parts(arg, len))
                })
                .collect();
            LocalFree(argv as _);
            args
        }
    }

    #[test]
    fn the_commands_windows_runs_to_uninstall_really_uninstall() {
        // Definicoes > Aplicacoes > Desinstalar corre a `UninstallString`;
        // `winget uninstall --silent` corre a `QuietUninstallString`. Sem o
        // `--uninstall` as duas abriam o ecra de instalar.
        let uninstaller = std::path::Path::new(
            r"C:\Users\alguem\AppData\Local\Programs\NeuralIA\Desinstalar NeuralIA.exe",
        );
        let (normal, quiet) = winshell::uninstall_commands(uninstaller);
        for command in [&normal, &quiet] {
            let args = argv_after_exe(command);
            assert_eq!(
                mode_from(&args),
                app::Mode::Uninstall,
                "o Windows corre {command} e recebe {args:?}"
            );
        }
    }
}
