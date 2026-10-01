//! Verificação de atualizações do NeuralIA (SPEC-0013, SPEC-0016 Fase 4).
//!
//! Módulo determinístico e puro de comparação de versões e parsing dos
//! metadados de release do GitHub. Não realiza chamadas de rede no core:
//! recebe o payload ou as strings de versão e toma a decisão.

use serde::Deserialize;

use crate::{NeuralError, Result};

/// URL padrão do repositório oficial no GitHub.
pub const DEFAULT_GITHUB_REPO: &str = "JoseRFJuniorLLMs/NeuralIA";

/// Constrói o endpoint da API do GitHub para obter a última release pública.
pub fn github_latest_release_url(repo: &str) -> String {
    let clean_repo = repo.trim().trim_matches('/');
    format!("https://api.github.com/repos/{clean_repo}/releases/latest")
}

/// Metadados extraídos de uma release no GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: String,
    pub html_url: String,
    pub installer_url: Option<String>,
    pub published_at: Option<String>,
    pub body: String,
}

#[derive(Deserialize)]
struct RawGithubRelease {
    tag_name: Option<String>,
    html_url: Option<String>,
    published_at: Option<String>,
    body: Option<String>,
    #[serde(default)]
    assets: Vec<RawGithubAsset>,
}

#[derive(Deserialize)]
struct RawGithubAsset {
    name: Option<String>,
    browser_download_url: Option<String>,
}

/// Parseia o JSON retornado pela API de releases do GitHub.
pub fn parse_github_release_json(json_str: &str) -> Result<ReleaseInfo> {
    let raw: RawGithubRelease = serde_json::from_str(json_str)
        .map_err(|err| NeuralError::Config(format!("JSON de release inválido: {err}")))?;

    let tag = raw.tag_name.unwrap_or_default().trim().to_string();
    if tag.is_empty() {
        return Err(NeuralError::Config("Release sem tag_name".to_string()));
    }

    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    let html_url = raw.html_url.unwrap_or_default();
    let body = raw.body.unwrap_or_default();
    let published_at = raw.published_at;

    // Localiza o instalador executável nos assets (ex: NeuralIA-Setup.exe)
    let installer_url = raw.assets.into_iter().find_map(|asset| {
        let name = asset.name.as_deref().unwrap_or_default();
        if name.to_ascii_lowercase().ends_with(".exe") {
            asset.browser_download_url
        } else {
            None
        }
    });

    Ok(ReleaseInfo {
        tag,
        version,
        html_url,
        installer_url,
        published_at,
        body,
    })
}

/// Compara duas versões em formato semver (ex: "2.6.2" vs "2.6.3", ou "2.7.0" vs "2.7.0-rc1").
/// Segue a especificação SemVer 2.0.0:
/// - Uma versão com pré-release (ex: "2.7.0-beta.1") é inferior à versão final ("2.7.0").
/// - Retorna `Ordering::Greater` se `candidate` for estritamente mais recente que `current`.
pub fn compare_semver(current: &str, candidate: &str) -> std::cmp::Ordering {
    fn parse_semver(s: &str) -> ([u64; 3], Option<&str>) {
        let clean = s.trim().trim_start_matches('v');
        let (num_part, pre_part) = match clean.split_once('-') {
            Some((num, pre)) => (num, Some(pre)),
            None => (clean, None),
        };
        let mut nums = [0u64; 3];
        for (i, part) in num_part.split('.').take(3).enumerate() {
            if let Ok(val) = part.parse::<u64>() {
                nums[i] = val;
            }
        }
        (nums, pre_part)
    }

    let (cur_nums, cur_pre) = parse_semver(current);
    let (cand_nums, cand_pre) = parse_semver(candidate);

    match cand_nums.cmp(&cur_nums) {
        std::cmp::Ordering::Equal => match (cur_pre, cand_pre) {
            (None, None) => std::cmp::Ordering::Equal,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(cur_p), Some(cand_p)) => cand_p.cmp(cur_p),
        },
        non_eq => non_eq,
    }
}

/// Verdadeiro se `candidate` for uma versão superior a `current`.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    compare_semver(current, candidate) == std::cmp::Ordering::Greater
}

/// Estado do resultado da checagem de versão.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// O aplicativo está na versão mais recente ou superior (dev build).
    UpToDate { current_version: String },
    /// Há uma versão mais nova disponível no repositório.
    UpdateAvailable {
        current_version: String,
        latest: ReleaseInfo,
    },
}

/// Avalia o status de atualização comparando a versão atual do app com a release obtida.
pub fn check_update_status(current_version: &str, latest: ReleaseInfo) -> UpdateStatus {
    if is_newer_version(current_version, &latest.version) {
        UpdateStatus::UpdateAvailable {
            current_version: current_version.to_string(),
            latest,
        }
    } else {
        UpdateStatus::UpToDate {
            current_version: current_version.to_string(),
        }
    }
}
/// Consulta a última release pública no GitHub através da API REST oficial.
pub fn fetch_latest_release(repo: &str) -> Result<ReleaseInfo> {
    let url = github_latest_release_url(repo);
    let agent = ureq::Agent::new_with_defaults();
    let mut response = agent
        .get(&url)
        .header("accept", "application/vnd.github.v3+json")
        .header("user-agent", "NeuralIA-App")
        .call()
        .map_err(|e| NeuralError::Config(format!("Falha na consulta ao GitHub: {e}")))?;

    let mut body_str = String::new();
    use std::io::Read;
    response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .reader()
        .read_to_string(&mut body_str)
        .map_err(|e| NeuralError::Config(format!("Falha ao ler release: {e}")))?;
    parse_github_release_json(&body_str)
}

/// Faz o download do instalador da release reportando o progresso (de 0.0 a 1.0).
pub fn download_installer(
    installer_url: &str,
    target_path: &std::path::Path,
    mut on_progress: impl FnMut(f64),
) -> Result<()> {
    let agent = ureq::Agent::new_with_defaults();
    let mut response = agent
        .get(installer_url)
        .header("user-agent", "NeuralIA-App")
        .call()
        .map_err(|e| NeuralError::Config(format!("Falha ao conectar para download: {e}")))?;

    let total_bytes = response.body().content_length().unwrap_or(0);
    let mut file = std::fs::File::create(target_path)
        .map_err(|e| NeuralError::Config(format!("Falha ao criar arquivo de destino: {e}")))?;

    let mut reader = response
        .body_mut()
        .with_config()
        .limit(150 * 1024 * 1024)
        .reader();

    let mut buffer = [0u8; 32 * 1024];
    let mut downloaded: u64 = 0;
    use std::io::{Read, Write};
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| NeuralError::Config(format!("Erro no stream de download: {e}")))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| NeuralError::Config(format!("Erro ao gravar dados do instalador: {e}")))?;
        downloaded += read as u64;
        if total_bytes > 0 {
            on_progress((downloaded as f64 / total_bytes as f64).clamp(0.0, 1.0));
        } else {
            on_progress(0.5);
        }
    }
    file.flush()
        .map_err(|e| NeuralError::Config(format!("Erro ao finalizar instalador: {e}")))?;
    on_progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_latest_release_url() {
        assert_eq!(
            github_latest_release_url("JoseRFJuniorLLMs/NeuralIA"),
            "https://api.github.com/repos/JoseRFJuniorLLMs/NeuralIA/releases/latest"
        );
        assert_eq!(
            github_latest_release_url(" /foo/bar/ "),
            "https://api.github.com/repos/foo/bar/releases/latest"
        );
    }

    #[test]
    fn test_compare_semver() {
        assert_eq!(
            compare_semver("2.6.2", "2.6.3"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("v2.6.2", "v2.7.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_semver("2.6.2", "2.6.2"), std::cmp::Ordering::Equal);
        assert_eq!(compare_semver("2.6.2", "2.6.1"), std::cmp::Ordering::Less);
        assert_eq!(compare_semver("3.0.0", "2.9.9"), std::cmp::Ordering::Less);
        assert_eq!(compare_semver("2.6", "2.6.1"), std::cmp::Ordering::Greater);
        // Regra SemVer 2.0.0: pré-release é estritamente menor que versão final
        assert_eq!(
            compare_semver("2.7.0", "2.7.0-rc1"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_semver("2.7.0-rc1", "2.7.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("2.7.0-beta.1", "2.7.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("2.7.0-rc1", "2.7.0-rc2"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("2.6.2", "2.6.3"));
        assert!(is_newer_version("2.6.2", "3.0.0"));
        assert!(!is_newer_version("2.6.2", "2.6.2"));
        assert!(!is_newer_version("2.6.2", "2.6.1"));
        assert!(!is_newer_version("2.6.2", "1.9.0"));
    }

    #[test]
    fn test_parse_github_release_json() {
        let json = r#"{
            "tag_name": "v2.7.0",
            "html_url": "https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/tag/v2.7.0",
            "published_at": "2026-10-01T12:00:00Z",
            "body": "Novas funcionalidades implementadas.",
            "assets": [
                {
                    "name": "NeuralIA.exe.sha256",
                    "browser_download_url": "https://example.com/NeuralIA.exe.sha256"
                },
                {
                    "name": "NeuralIA-Setup-2.7.0.exe",
                    "browser_download_url": "https://example.com/NeuralIA-Setup-2.7.0.exe"
                }
            ]
        }"#;

        let info = parse_github_release_json(json).expect("parse release json");
        assert_eq!(info.tag, "v2.7.0");
        assert_eq!(info.version, "2.7.0");
        assert_eq!(
            info.html_url,
            "https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/tag/v2.7.0"
        );
        assert_eq!(
            info.installer_url.as_deref(),
            Some("https://example.com/NeuralIA-Setup-2.7.0.exe")
        );
        assert_eq!(info.body, "Novas funcionalidades implementadas.");

        let status = check_update_status("2.6.2", info);
        match status {
            UpdateStatus::UpdateAvailable {
                current_version,
                latest,
            } => {
                assert_eq!(current_version, "2.6.2");
                assert_eq!(latest.version, "2.7.0");
            }
            _ => panic!("deveria ter indicado update disponivel"),
        }
    }

    #[test]
    fn test_check_update_status_up_to_date() {
        let info = ReleaseInfo {
            tag: "v2.6.2".to_string(),
            version: "2.6.2".to_string(),
            html_url: "https://example.com".to_string(),
            installer_url: None,
            published_at: None,
            body: String::new(),
        };
        let status = check_update_status("2.6.2", info);
        assert_eq!(
            status,
            UpdateStatus::UpToDate {
                current_version: "2.6.2".to_string()
            }
        );
    }
}
