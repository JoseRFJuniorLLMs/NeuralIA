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
    /// SHA-256 do asset, vindo do campo `digest` da API do GitHub.
    /// O updater falha fechado quando o digest não está disponível.
    pub installer_sha256: Option<String>,
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
    digest: Option<String>,
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

    // O contrato de release publica exatamente o instalador versionado.
    // Nunca aceite "o primeiro .exe": um asset extra não pode virar código executado.
    let expected_name = format!("NeuralIA-Setup-{version}-x64.exe");
    let installer = raw
        .assets
        .into_iter()
        .find(|asset| asset.name.as_deref() == Some(expected_name.as_str()));
    let (installer_url, installer_sha256) = installer.map_or((None, None), |asset| {
        let digest = asset
            .digest
            .as_deref()
            .and_then(|value| value.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase);
        (asset.browser_download_url, digest)
    });

    Ok(ReleaseInfo {
        tag,
        version,
        html_url,
        installer_url,
        installer_sha256,
        published_at,
        body,
    })
}

/// Compara duas versões SemVer 2.0.0.
/// Entradas inválidas são tratadas de forma conservadora como iguais: o updater
/// nunca instala algo só porque um identificador malformado foi interpretado como zero.
pub fn compare_semver(current: &str, candidate: &str) -> std::cmp::Ordering {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Identifier<'a> {
        Numeric(u64),
        Alpha(&'a str),
    }

    fn valid_identifier(value: &str) -> bool {
        !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }

    fn parse<'a>(input: &'a str) -> Option<([u64; 3], Vec<Identifier<'a>>)> {
        let clean = input.trim().strip_prefix('v').unwrap_or(input.trim());
        let (without_build, build) = clean
            .split_once('+')
            .map_or((clean, None), |(left, right)| (left, Some(right)));
        if build.is_some_and(|b| b.split('.').any(|id| !valid_identifier(id))) {
            return None;
        }
        let (core, pre) = without_build
            .split_once('-')
            .map_or((without_build, None), |(left, right)| (left, Some(right)));

        let mut parts = core.split('.');
        let major = parts.next()?.parse::<u64>().ok()?;
        let minor = parts.next()?.parse::<u64>().ok()?;
        let patch = parts.next()?.parse::<u64>().ok()?;
        if parts.next().is_some() {
            return None;
        }

        let mut identifiers = Vec::new();
        if let Some(pre) = pre {
            if pre.is_empty() {
                return None;
            }
            for id in pre.split('.') {
                if !valid_identifier(id) {
                    return None;
                }
                if id.bytes().all(|b| b.is_ascii_digit()) {
                    if id.len() > 1 && id.starts_with('0') {
                        return None;
                    }
                    identifiers.push(Identifier::Numeric(id.parse::<u64>().ok()?));
                } else {
                    identifiers.push(Identifier::Alpha(id));
                }
            }
        }
        Some(([major, minor, patch], identifiers))
    }

    fn cmp_pre(left: &[Identifier<'_>], right: &[Identifier<'_>]) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (left.is_empty(), right.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (l, r) in left.iter().zip(right) {
            let ord = match (l, r) {
                (Identifier::Numeric(a), Identifier::Numeric(b)) => a.cmp(b),
                (Identifier::Numeric(_), Identifier::Alpha(_)) => Ordering::Less,
                (Identifier::Alpha(_), Identifier::Numeric(_)) => Ordering::Greater,
                (Identifier::Alpha(a), Identifier::Alpha(b)) => a.cmp(b),
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        left.len().cmp(&right.len())
    }

    let Some((cur_core, cur_pre)) = parse(current) else {
        return std::cmp::Ordering::Equal;
    };
    let Some((cand_core, cand_pre)) = parse(candidate) else {
        return std::cmp::Ordering::Equal;
    };

    match cand_core.cmp(&cur_core) {
        std::cmp::Ordering::Equal => cmp_pre(&cand_pre, &cur_pre),
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

/// Faz o download do instalador, verifica o SHA-256 esperado e só então
/// publica o arquivo no caminho final. Um erro nunca deixa um EXE parcial no alvo.
pub fn download_installer(
    installer_url: &str,
    expected_sha256: &str,
    target_path: &std::path::Path,
    mut on_progress: impl FnMut(f64),
) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    if expected_sha256.len() != 64
        || !expected_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(NeuralError::Config(
            "Atualização recusada: SHA-256 ausente ou inválido".to_string(),
        ));
    }

    let parsed = url::Url::parse(installer_url)
        .map_err(|e| NeuralError::Config(format!("URL do instalador inválida: {e}")))?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("github.com") {
        return Err(NeuralError::Config(
            "Atualização recusada: instalador fora do GitHub oficial".to_string(),
        ));
    }

    let part_path = target_path.with_extension("exe.part");
    let _ = std::fs::remove_file(&part_path);

    let result = (|| -> Result<()> {
        let agent = ureq::Agent::new_with_defaults();
        let mut response = agent
            .get(installer_url)
            .header("user-agent", "NeuralIA-App")
            .call()
            .map_err(|e| NeuralError::Config(format!("Falha ao conectar para download: {e}")))?;

        let total_bytes = response.body().content_length().unwrap_or(0);
        if total_bytes > 150 * 1024 * 1024 {
            return Err(NeuralError::Config(
                "Atualização recusada: instalador excede 150 MiB".to_string(),
            ));
        }

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
            .map_err(|e| NeuralError::Config(format!("Falha ao criar arquivo temporário: {e}")))?;

        let mut reader = response
            .body_mut()
            .with_config()
            .limit(150 * 1024 * 1024)
            .reader();

        let mut buffer = [0u8; 32 * 1024];
        let mut downloaded: u64 = 0;
        let mut hasher = Sha256::new();
        let mut last_progress = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut last_percent = u64::MAX;

        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|e| NeuralError::Config(format!("Erro no stream de download: {e}")))?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(|e| {
                NeuralError::Config(format!("Erro ao gravar dados do instalador: {e}"))
            })?;
            hasher.update(&buffer[..read]);
            downloaded += read as u64;

            let percent = if total_bytes > 0 {
                ((downloaded.saturating_mul(100)) / total_bytes).min(100)
            } else {
                0
            };
            if percent != last_percent && last_progress.elapsed() >= Duration::from_millis(200) {
                on_progress(if total_bytes > 0 {
                    downloaded as f64 / total_bytes as f64
                } else {
                    0.0
                });
                last_progress = Instant::now();
                last_percent = percent;
            }
        }

        file.flush()
            .map_err(|e| NeuralError::Config(format!("Erro ao finalizar instalador: {e}")))?;
        file.sync_all()
            .map_err(|e| NeuralError::Config(format!("Erro ao sincronizar instalador: {e}")))?;

        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected_sha256) {
            return Err(NeuralError::Config(format!(
                "Atualização recusada: SHA-256 divergente (esperado {expected_sha256}, obtido {actual})"
            )));
        }

        if target_path.exists() {
            std::fs::remove_file(target_path).map_err(|e| {
                NeuralError::Config(format!("Falha ao substituir instalador anterior: {e}"))
            })?;
        }
        std::fs::rename(&part_path, target_path)
            .map_err(|e| NeuralError::Config(format!("Falha ao publicar instalador verificado: {e}")))?;
        on_progress(1.0);
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&part_path);
    }
    result
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
        // SemVer 2.0.0 exige major.minor.patch; entrada inválida não dispara update.
        assert_eq!(compare_semver("2.6", "2.6.1"), std::cmp::Ordering::Equal);
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
        assert_eq!(
            compare_semver("2.7.0-rc.9", "2.7.0-rc.10"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("2.7.0-alpha.2", "2.7.0-alpha.10"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("2.7.1+build.1", "2.7.1+build.9"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_semver("2.7.1", "2.7.1-01"),
            std::cmp::Ordering::Equal
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
                    "name": "NeuralIA-Setup-2.7.0-x64.exe",
                    "browser_download_url": "https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/download/v2.7.0/NeuralIA-Setup-2.7.0-x64.exe",
                    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
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
            Some("https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/download/v2.7.0/NeuralIA-Setup-2.7.0-x64.exe")
        );
        assert_eq!(
            info.installer_sha256.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
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
    fn release_parser_rejects_unexpected_executable_asset() {
        let json = r#"{
            "tag_name":"v9.9.9",
            "html_url":"https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/tag/v9.9.9",
            "assets":[{
                "name":"evil.exe",
                "browser_download_url":"https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/download/v9.9.9/evil.exe",
                "digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }]
        }"#;
        let info = parse_github_release_json(json).expect("parse");
        assert!(info.installer_url.is_none());
        assert!(info.installer_sha256.is_none());
    }

    #[test]
    fn test_check_update_status_up_to_date() {
        let info = ReleaseInfo {
            tag: "v2.6.2".to_string(),
            version: "2.6.2".to_string(),
            html_url: "https://example.com".to_string(),
            installer_url: None,
            installer_sha256: None,
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
