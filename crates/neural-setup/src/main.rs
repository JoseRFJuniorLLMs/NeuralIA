//! Instalador da NeuralIA.
//!
//! Um executavel so: com a NeuralIA dentro dele, instala; copiado para a pasta
//! de instalacao e corrido com `--uninstall`, remove.
//!
//! Linha de comandos:
//! - `/S` (ou `/silent`, `/quiet`): sem janela, o resultado no codigo de saida
//!   (0 correu bem, 1 falhou, 2 argumentos/pasta recusados, 3 a NeuralIA esta
//!   aberta, 4 instalador sem carga util);
//! - `--uninstall` (ou `/uninstall`, `-u`): desinstalar;
//! - `/D=<pasta>`: a pasta de instalacao, como no NSIS -- o ULTIMO argumento, e
//!   o caminho vai ate ao fim da linha, espacos incluidos, com ou sem aspas.

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

/// `/D=<pasta>` ou `"/D=<pasta>"`: o argumento que da a pasta de instalacao.
fn is_dir_argument(arg: &str) -> bool {
    let arg = arg.trim_start_matches('"');
    arg.starts_with("/D=") || arg.starts_with("/d=")
}

/// A pasta do `/D=`, lida da linha de comandos crua.
///
/// Como no NSIS, o `/D=` e o ultimo argumento e o caminho vai ate ao fim da
/// linha: `/D=C:\Program Files\NeuralIA` chega inteiro, sem aspas. Os
/// argumentos ja partidos pelo Windows nao servem para isto -- o caminho vinha
/// aos bocados, e um bocado chamado `uninstall` virava outra ordem. Aceita
/// tambem `"/D=C:\a b"` e `/D="C:\a b"`, que e o que um script escreve.
fn install_dir_from_command_line(command_line: &str) -> Option<String> {
    // O primeiro token e o executavel, e pode ter espacos entre aspas -- ate
    // um `/D=` no nome da pasta de onde foi corrido, que nao conta.
    let line = command_line.trim_start();
    let rest = match line.strip_prefix('"') {
        Some(quoted) => &quoted[quoted.find('"').map_or(quoted.len(), |end| end + 1)..],
        None => &line[line.find(char::is_whitespace).unwrap_or(line.len())..],
    };

    let mut previous_is_space = true;
    for (index, c) in rest.char_indices() {
        if previous_is_space {
            let token = &rest[index..];
            let (quoted, token) = match token.strip_prefix('"') {
                Some(inner) => (true, inner),
                None => (false, token),
            };
            if let Some(value) = token
                .strip_prefix("/D=")
                .or_else(|| token.strip_prefix("/d="))
            {
                let mut value = value.trim();
                if quoted {
                    value = value.strip_suffix('"').unwrap_or(value);
                }
                if let Some(inner) = value.strip_prefix('"') {
                    value = inner.strip_suffix('"').unwrap_or(inner);
                }
                let value = value.trim();
                return (!value.is_empty()).then(|| value.to_string());
            }
        }
        previous_is_space = c.is_whitespace();
    }
    None
}

/// As bandeiras que contam: as que vem antes do `/D=`. O que vem depois e o
/// caminho, e uma pasta chamada `C:\uninstall me` nao e uma ordem.
fn flags_of(args: &[String]) -> &[String] {
    let end = args
        .iter()
        .position(|arg| is_dir_argument(arg))
        .unwrap_or(args.len());
    &args[..end]
}

/// `/S` e o silencioso de sempre nos desinstaladores do Windows, e e o que a
/// chave `QuietUninstallString` manda (`winget uninstall --silent`, scripts):
/// quem o corre espera que o processo acabe sozinho, sem janela.
#[cfg(windows)]
fn launch_from(args: &[String], command_line: &str) -> app::Launch {
    let flags = flags_of(args);
    let quiet = flags.iter().any(|arg| {
        let arg = arg.trim_start_matches(['-', '/']).to_ascii_lowercase();
        arg == "s" || arg == "silent" || arg == "quiet"
    });
    app::Launch {
        mode: mode_from(flags),
        quiet,
        install_dir: install_dir_from_command_line(command_line).map(std::path::PathBuf::from),
    }
}

#[cfg(windows)]
fn mode_from(args: &[String]) -> app::Mode {
    let wants_removal = flags_of(args).iter().any(|arg| {
        let arg = arg.trim_start_matches(['-', '/']).to_ascii_lowercase();
        arg == "uninstall" || arg == "u" || arg == "remove"
    });
    if wants_removal {
        app::Mode::Uninstall
    } else {
        app::Mode::Install
    }
}

/// A linha de comandos tal como o processo a recebeu.
#[cfg(windows)]
fn raw_command_line() -> String {
    unsafe {
        let raw = windows_sys::Win32::System::Environment::GetCommandLineW();
        if raw.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&n| *raw.add(n) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(raw, len))
    }
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = launch_from(&args, &raw_command_line());
    if launch.quiet {
        std::process::exit(app::run_quiet(&launch));
    }
    app::run(&launch);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("O instalador da NeuralIA e para Windows.");
    std::process::exit(1);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn the_uninstall_flag_is_understood_in_every_shape_windows_uses() {
        // A `QuietUninstallString` que escrevemos no registo passa `/S` depois
        // do `--uninstall`; se so se lesse o primeiro argumento, o Windows
        // mandava desinstalar e o programa instalava por cima.
        for list in [
            args(&["--uninstall"]),
            args(&["/uninstall"]),
            args(&["-u"]),
            args(&["--uninstall", "/S"]),
            args(&["/S", "--uninstall"]),
            args(&["--UNINSTALL"]),
        ] {
            assert_eq!(
                mode_from(&list),
                app::Mode::Uninstall,
                "{list:?} devia desinstalar"
            );
        }
        for list in [vec![], args(&["/S"]), args(&["outra"])] {
            assert_eq!(
                mode_from(&list),
                app::Mode::Install,
                "{list:?} devia instalar"
            );
        }
    }

    #[test]
    fn the_silent_flag_runs_without_a_window() {
        // `winget uninstall --silent` e os scripts esperam que o processo
        // acabe sozinho; uma janela a espera de um clique pendura-os.
        for flag in ["/S", "/s", "-silent", "--quiet", "/QUIET"] {
            let list = args(&["--uninstall", flag]);
            assert_eq!(
                launch_from(&list, &format!("setup.exe --uninstall {flag}")),
                app::Launch {
                    mode: app::Mode::Uninstall,
                    quiet: true,
                    install_dir: None,
                },
                "{list:?} devia desinstalar sem janela"
            );
        }
        // E o comando que o registo manda para o modo silencioso.
        let (normal, quiet) =
            winshell::uninstall_commands(std::path::Path::new(r"C:\x\Desinstalar NeuralIA.exe"));
        assert!(launch_from(&argv_after_exe(&quiet), &quiet).quiet);
        assert!(!launch_from(&argv_after_exe(&normal), &normal).quiet);
        for list in [args(&["--uninstall"]), vec![]] {
            assert!(
                !launch_from(&list, "setup.exe").quiet,
                "{list:?} tem uma pessoa a frente do ecra"
            );
        }
    }

    #[test]
    fn the_install_dir_reaches_the_end_of_the_line_spaces_and_all() {
        // O CI instala em `...\Neural IA` com `/S /D=...`. Se o caminho fosse
        // cortado no primeiro espaco, a instalacao ia para outra pasta -- e o
        // teste do CI desinstalava-a de la.
        let cases = [
            (
                r#""C:\Users\eu\Downloads\NeuralIA-Setup.exe" /S /D=C:\Program Files\Neural IA"#,
                Some(r"C:\Program Files\Neural IA"),
            ),
            (
                r#"setup.exe /S "/D=C:\Temp\smoke dir\Neural IA""#,
                Some(r"C:\Temp\smoke dir\Neural IA"),
            ),
            (
                r#"setup.exe /D="C:\a b\NeuralIA""#,
                Some(r"C:\a b\NeuralIA"),
            ),
            (r"setup.exe /S /d=D:\x", Some(r"D:\x")),
            (r"setup.exe /S /D=D:\x   ", Some(r"D:\x")),
            (r"setup.exe /S", None),
            (r"setup.exe /S /D=", None),
            (r"setup.exe /S /D=   ", None),
            // Um `/D=` colado a outra coisa nao e o argumento.
            (r"setup.exe /S/D=C:\x", None),
            (r"setup.exe --uninstall /S", None),
            // O `/D=` no caminho do proprio executavel nao conta.
            (r#""C:\estranho /D=pasta\setup.exe" /S"#, None),
            (
                r"C:\sem-aspas\setup.exe /S /D=E:\Apps\NeuralIA",
                Some(r"E:\Apps\NeuralIA"),
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(
                install_dir_from_command_line(line).as_deref(),
                expected,
                "{line}"
            );
        }
    }

    #[test]
    fn a_path_after_d_is_never_read_as_flags() {
        // `/D=C:\Users\eu\uninstall s` parte-se em `uninstall` e `s` -- que
        // sao, letra a letra, as bandeiras de desinstalar em silencio.
        let line = r"setup.exe /D=C:\Users\eu\uninstall s";
        let list = argv_after_exe(line);
        let launch = launch_from(&list, line);
        assert_eq!(launch.mode, app::Mode::Install);
        assert!(!launch.quiet);
        assert_eq!(
            launch.install_dir,
            Some(std::path::PathBuf::from(r"C:\Users\eu\uninstall s"))
        );

        let line = r"setup.exe --uninstall /S /D=C:\Temp\x y";
        let launch = launch_from(&argv_after_exe(line), line);
        assert_eq!(launch.mode, app::Mode::Uninstall);
        assert!(launch.quiet);
        assert_eq!(
            launch.install_dir,
            Some(std::path::PathBuf::from(r"C:\Temp\x y"))
        );
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
            let list = argv_after_exe(command);
            assert_eq!(
                mode_from(&list),
                app::Mode::Uninstall,
                "o Windows corre {command} e recebe {list:?}"
            );
            assert_eq!(install_dir_from_command_line(command), None);
        }
    }
}
