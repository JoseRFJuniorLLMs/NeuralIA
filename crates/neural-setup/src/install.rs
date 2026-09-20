//! O trabalho da instalacao: onde e que vai, o que se escreve, por que ordem,
//! e quanto e que ja foi feito.
//!
//! Tudo o que decide fica aqui e e portatil -- escrever ficheiros e
//! `std::fs`. Atalhos e registo, que so existem no Windows, vivem no
//! `winshell`. Assim a parte que se pode enganar em silencio (o caminho, a
//! ordem, a percentagem) testa-se sem ecra e sem Windows.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::archive::Entry;

/// O nome da pasta, do atalho e da chave de desinstalacao.
pub const PRODUCT: &str = "NeuralIA";
/// O executavel que o atalho aponta.
pub const EXECUTABLE: &str = "NeuralIA.exe";
/// O instalador copia-se para a pasta com este nome, para poder desinstalar.
pub const UNINSTALLER: &str = "Desinstalar NeuralIA.exe";

#[derive(Debug)]
pub enum InstallError {
    /// Este instalador foi construido sem carga util. Acontece num
    /// `cargo build` normal; nunca devia sair assim para o mundo.
    NoPayload,
    Io(PathBuf, io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPayload => write!(f, "este instalador foi construido sem a NeuralIA la dentro"),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

/// As fases, pela ordem em que acontecem. A percentagem no ecra sai daqui.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Preparing,
    Writing,
    Shortcuts,
    Registering,
    Done,
}

impl Stage {
    /// O que se le por baixo da barra.
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "A preparar a pasta",
            Self::Writing => "A instalar a NeuralIA",
            Self::Shortcuts => "A criar os atalhos",
            Self::Registering => "A registar no sistema",
            Self::Done => "Instalada",
        }
    }

    /// A fatia da barra que esta fase ocupa, `(inicio, fim)`. Escrever
    /// ficheiros e quase tudo o tempo real; o resto sao instantes, e uma barra
    /// que salta de 50% para 100% no fim parece avariada.
    fn span(self) -> (f64, f64) {
        match self {
            Self::Preparing => (0.00, 0.04),
            Self::Writing => (0.04, 0.88),
            Self::Shortcuts => (0.88, 0.95),
            Self::Registering => (0.95, 0.99),
            Self::Done => (1.00, 1.00),
        }
    }
}

/// A percentagem global, dada a fase e o quanto dela ja foi. Monotona: a barra
/// nunca anda para tras, que e a unica coisa que uma barra nao pode fazer.
pub fn overall_progress(stage: Stage, within: f64) -> f64 {
    let (start, end) = stage.span();
    start + (end - start) * within.clamp(0.0, 1.0)
}

/// Onde a NeuralIA vive. Por utilizador, dentro de `%LOCALAPPDATA%`: instalar
/// em `Program Files` pediria elevacao, e um navegador nao precisa de ser
/// administrador para existir.
pub fn install_root(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Programs").join(PRODUCT)
}

/// O que ha para fazer.
#[derive(Debug, Clone)]
pub struct Plan {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub desktop_shortcut: bool,
}

impl Plan {
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.data.len() as u64).sum()
    }

    pub fn executable(&self) -> PathBuf {
        self.root.join(EXECUTABLE)
    }
}

/// Escreve a carga util. `progress` recebe a fracao ja escrita, entre 0 e 1.
///
/// Escreve para um nome temporario e so depois renomeia: se o disco encher a
/// meio, fica um `.part` em vez de um `NeuralIA.exe` meio escrito que arranca
/// e estoira.
pub fn write_payload(plan: &Plan, mut progress: impl FnMut(f64)) -> Result<(), InstallError> {
    if plan.entries.is_empty() {
        return Err(InstallError::NoPayload);
    }
    let total = plan.total_bytes().max(1);
    let mut written = 0u64;

    fs::create_dir_all(&plan.root).map_err(|e| InstallError::Io(plan.root.clone(), e))?;

    for entry in &plan.entries {
        let target = plan
            .root
            .join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| InstallError::Io(parent.to_path_buf(), e))?;
        }
        let staging = target.with_extension("neuralia-part");
        fs::write(&staging, &entry.data).map_err(|e| InstallError::Io(staging.clone(), e))?;
        // Um executavel a correr nao se deixa substituir. Tirar o antigo do
        // caminho primeiro da uma mensagem melhor do que um erro de rename.
        let _ = fs::remove_file(&target);
        fs::rename(&staging, &target).map_err(|e| InstallError::Io(target.clone(), e))?;

        written += entry.data.len() as u64;
        progress(written as f64 / total as f64);
    }
    Ok(())
}

/// Tudo o que a desinstalacao apaga dentro da pasta. Nao apaga a pasta do
/// perfil do utilizador -- historico, memoria e sessoes sao dele, nao nossas.
pub fn installed_files(root: &Path, entries: &[Entry]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = entries
        .iter()
        .map(|entry| root.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR)))
        .collect();
    paths.push(root.join(UNINSTALLER));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: usize) -> Entry {
        Entry {
            path: path.to_string(),
            data: vec![9u8; size],
        }
    }

    #[test]
    fn the_bar_only_ever_moves_forward() {
        // Uma barra que anda para tras e a forma mais rapida de a instalacao
        // parecer avariada.
        let mut last = -1.0;
        for stage in [
            Stage::Preparing,
            Stage::Writing,
            Stage::Shortcuts,
            Stage::Registering,
            Stage::Done,
        ] {
            for step in 0..=10 {
                let value = overall_progress(stage, step as f64 / 10.0);
                assert!(
                    value >= last,
                    "{stage:?} em {step}/10 recuou de {last} para {value}"
                );
                assert!((0.0..=1.0).contains(&value));
                last = value;
            }
        }
        assert_eq!(overall_progress(Stage::Done, 1.0), 1.0);
    }

    #[test]
    fn progress_outside_the_range_does_not_escape_the_bar() {
        assert_eq!(
            overall_progress(Stage::Writing, -5.0),
            overall_progress(Stage::Writing, 0.0)
        );
        assert_eq!(
            overall_progress(Stage::Writing, 9.0),
            overall_progress(Stage::Writing, 1.0)
        );
    }

    #[test]
    fn writing_files_is_most_of_the_bar() {
        // Se escrever 40 MB ocupasse um quinto da barra, ela ficaria parada
        // quase todo o tempo e depois saltava. E o tempo real que manda.
        let (start, end) = Stage::Writing.span();
        assert!(
            end - start > 0.7,
            "escrever ocupa so {:.0}% da barra",
            (end - start) * 100.0
        );
    }

    #[test]
    fn the_install_folder_is_per_user_and_needs_no_administrator() {
        let root = install_root(Path::new("C:/Users/alguem/AppData/Local"));
        assert!(root.ends_with("Programs/NeuralIA") || root.ends_with("Programs\\NeuralIA"));
        let shown = root.to_string_lossy().to_lowercase();
        assert!(
            !shown.contains("program files"),
            "instalar aqui pedia elevacao: {shown}"
        );
    }

    #[test]
    fn an_installer_built_without_a_payload_refuses_instead_of_making_an_empty_folder() {
        let dir = std::env::temp_dir().join("neuralia-setup-empty");
        let plan = Plan {
            root: dir,
            entries: Vec::new(),
            desktop_shortcut: false,
        };
        assert!(matches!(
            write_payload(&plan, |_| {}),
            Err(InstallError::NoPayload)
        ));
    }

    #[test]
    fn every_file_lands_where_the_package_said_and_the_bar_reaches_the_end() {
        let dir = std::env::temp_dir().join(format!("neuralia-setup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plan = Plan {
            root: dir.clone(),
            entries: vec![
                entry("NeuralIA.exe", 2048),
                entry("assets/logo.ico", 256),
                entry("a/b/c.dll", 64),
            ],
            desktop_shortcut: true,
        };

        let mut seen: Vec<f64> = Vec::new();
        write_payload(&plan, |fraction| seen.push(fraction)).expect("instalar");

        for path in installed_files(&dir, &plan.entries) {
            if path.ends_with(UNINSTALLER) {
                continue;
            }
            assert!(path.is_file(), "faltou {}", path.display());
        }
        // Nenhum `.neuralia-part` sobrevive a uma instalacao que correu bem.
        let leftovers: Vec<_> = walk(&dir)
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "neuralia-part"))
            .collect();
        assert!(leftovers.is_empty(), "ficheiros a meio: {leftovers:?}");

        assert_eq!(seen.last().copied(), Some(1.0), "a barra nao chegou ao fim");
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "a barra recuou");
        let _ = fs::remove_dir_all(&dir);
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(read) = fs::read_dir(dir) else {
            return out;
        };
        for item in read.flatten() {
            let path = item.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    #[test]
    fn reinstalling_over_a_previous_copy_replaces_it() {
        let dir = std::env::temp_dir().join(format!("neuralia-setup-again-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let old = Plan {
            root: dir.clone(),
            entries: vec![entry("NeuralIA.exe", 4096)],
            desktop_shortcut: false,
        };
        write_payload(&old, |_| {}).expect("primeira");
        let new = Plan {
            root: dir.clone(),
            entries: vec![Entry {
                path: "NeuralIA.exe".into(),
                data: b"nova versao".to_vec(),
            }],
            desktop_shortcut: false,
        };
        write_payload(&new, |_| {}).expect("segunda");
        assert_eq!(
            fs::read(dir.join("NeuralIA.exe")).expect("ler"),
            b"nova versao",
            "a segunda instalacao devia substituir a primeira"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
