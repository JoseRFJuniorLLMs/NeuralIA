//! Atualizações do NeuralIA (SPEC-0013, SPEC-0016 Fase 4).
//!
//! A release vem da API oficial do GitHub. O parser aceita apenas o asset
//! versionado do instalador, prende o URL ao repositório oficial e conserva o
//! digest SHA-256 publicado pelo próprio GitHub. O download escreve primeiro
//! num arquivo temporário exclusivo, confere tamanho e digest e só então
//! promove os bytes para o caminho executável final.

use std::{
    cmp::Ordering,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{NeuralError, Result};

/// URL padrão do repositório oficial no GitHub.
pub const DEFAULT_GITHUB_REPO: &str = "JoseRFJuniorLLMs/NeuralIA";
const GITHUB_HOST: &str = "github.com";
const RELEASE_JSON_MAX_BYTES: u64 = 1024 * 1024;
const INSTALLER_MAX_BYTES: u64 = 150 * 1024 * 1024;
const DOWNLOAD_BUFFER_BYTES: usize = 32 * 1024;

static UPDATE_AGENT: OnceLock<ureq::Agent> = OnceLock::new();
static PART_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn update_agent() -> &'static ureq::Agent {
    UPDATE_AGENT.get_or_init(ureq::Agent::new_with_defaults)
}

/// Constrói o endpoint da API do GitHub para obter a última release pública.
pub fn github_latest_release_url(repo: &str) -> String {
    let clean_repo = repo.trim().trim_matches('/');
    format!("https://api.github.com/repos/{clean_repo}/releases/latest")
}

/// Instalador que pode ser aplicado: URL oficial e digest fornecido pelo GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerAsset {
    pub url: String,
    /// SHA-256 hexadecimal, 64 caracteres em minúsculas.
    pub sha256: String,
}

/// Metadados extraídos de uma release no GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: String,
    pub html_url: String,
    pub installer: Option<InstallerAsset>,
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

#[derive(Debug, Clone, Copy)]
struct ParsedSemver<'a> {
    core: [&'a str; 3],
    pre: Option<&'a str>,
}

fn numeric_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_identifier_list(value: &str, reject_numeric_leading_zero: bool) -> bool {
    !value.is_empty()
        && value.split('.').all(|identifier| {
            valid_identifier(identifier)
                && (!reject_numeric_leading_zero
                    || !identifier.bytes().all(|byte| byte.is_ascii_digit())
                    || numeric_identifier(identifier))
        })
}

fn parse_semver(value: &str) -> Option<ParsedSemver<'_>> {
    let clean = value.trim().strip_prefix('v').unwrap_or(value.trim());
    if clean.is_empty() {
        return None;
    }

    let (without_build, build) = match clean.split_once('+') {
        Some((version, build)) => {
            if build.contains('+') || !valid_identifier_list(build, false) {
                return None;
            }
            (version, Some(build))
        }
        None => (clean, None),
    };
    let _ = build;

    let (core, pre) = match without_build.split_once('-') {
        Some((core, pre)) => {
            if !valid_identifier_list(pre, true) {
                return None;
            }
            (core, Some(pre))
        }
        None => (without_build, None),
    };

    let mut parts = core.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;
    if parts.next().is_some()
        || !numeric_identifier(major)
        || !numeric_identifier(minor)
        || !numeric_identifier(patch)
    {
        return None;
    }

    Some(ParsedSemver {
        core: [major, minor, patch],
        pre,
    })
}

fn cmp_numeric(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn cmp_prerelease(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        // Sem pré-release = versão final, portanto maior.
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => {
            let mut left = left.split('.');
            let mut right = right.split('.');
            loop {
                match (left.next(), right.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(a), Some(b)) => {
                        let a_numeric = a.bytes().all(|byte| byte.is_ascii_digit());
                        let b_numeric = b.bytes().all(|byte| byte.is_ascii_digit());
                        let order = match (a_numeric, b_numeric) {
                            (true, true) => cmp_numeric(a, b),
                            (true, false) => Ordering::Less,
                            (false, true) => Ordering::Greater,
                            (false, false) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                }
            }
        }
    }
}

fn cmp_parsed(left: ParsedSemver<'_>, right: ParsedSemver<'_>) -> Ordering {
    for index in 0..3 {
        let order = cmp_numeric(left.core[index], right.core[index]);
        if order != Ordering::Equal {
            return order;
        }
    }
    cmp_prerelease(left.pre, right.pre)
}

/// Compara versões segundo SemVer 2.0.0.
///
/// O retorno descreve `candidate` em relação a `current`. `None` indica
/// versão inválida; metadado de build (`+...`) é validado, mas não participa
/// da precedência.
pub fn compare_semver(current: &str, candidate: &str) -> Option<Ordering> {
    let current = parse_semver(current)?;
    let candidate = parse_semver(candidate)?;
    Some(cmp_parsed(candidate, current))
}

/// Verdadeiro somente quando as duas versões são válidas e `candidate` é maior.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    compare_semver(current, candidate) == Some(Ordering::Greater)
}

fn normalized_sha256(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

fn installer_name(version: &str) -> String {
    format!("NeuralIA-Setup-{version}-x64.exe")
}

fn official_installer_url(raw: &str, tag: &str, expected_name: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some(GITHUB_HOST)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(segments) = url.path_segments() else {
        return false;
    };
    let segments: Vec<&str> = segments.collect();
    segments
        == [
            "JoseRFJuniorLLMs",
            "NeuralIA",
            "releases",
            "download",
            tag,
            expected_name,
        ]
}

fn official_release_download_url(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str() != Some(GITHUB_HOST)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(segments) = url.path_segments() else {
        return false;
    };
    let segments: Vec<&str> = segments.collect();
    segments.len() == 6
        && segments[0] == "JoseRFJuniorLLMs"
        && segments[1] == "NeuralIA"
        && segments[2] == "releases"
        && segments[3] == "download"
        && segments[5].starts_with("NeuralIA-Setup-")
        && segments[5].ends_with("-x64.exe")
}

/// Parseia a última release e aceita somente o instalador versionado oficial.
///
/// Um asset com o nome esperado mas sem digest SHA-256 válido ou com URL fora
/// do repositório oficial torna a release inválida em vez de degradar para
/// execução sem verificação.
pub fn parse_github_release_json(json_str: &str) -> Result<ReleaseInfo> {
    let raw: RawGithubRelease = serde_json::from_str(json_str)
        .map_err(|err| NeuralError::Config(format!("JSON de release inválido: {err}")))?;

    let tag = raw.tag_name.unwrap_or_default().trim().to_string();
    if tag.is_empty() {
        return Err(NeuralError::Config("Release sem tag_name".to_string()));
    }

    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    if parse_semver(&version).is_none() {
        return Err(NeuralError::Config(format!(
            "Release com versão SemVer inválida: {tag}"
        )));
    }

    let expected_name = installer_name(&version);
    let installer = match raw
        .assets
        .into_iter()
        .find(|asset| asset.name.as_deref() == Some(expected_name.as_str()))
    {
        None => None,
        Some(asset) => {
            let url = asset.browser_download_url.ok_or_else(|| {
                NeuralError::Config("Asset do instalador sem browser_download_url".to_string())
            })?;
            if !official_installer_url(&url, &tag, &expected_name) {
                return Err(NeuralError::Config(
                    "URL do instalador não pertence à release oficial".to_string(),
                ));
            }
            let sha256 = asset
                .digest
                .as_deref()
                .and_then(normalized_sha256)
                .ok_or_else(|| {
                    NeuralError::Config(
                        "Asset do instalador sem digest SHA-256 válido do GitHub".to_string(),
                    )
                })?;
            Some(InstallerAsset { url, sha256 })
        }
    };

    Ok(ReleaseInfo {
        tag,
        version,
        html_url: raw.html_url.unwrap_or_default(),
        installer,
        published_at: raw.published_at,
        body: raw.body.unwrap_or_default(),
    })
}

/// Estado do resultado da checagem de versão.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate { current_version: String },
    UpdateAvailable {
        current_version: String,
        latest: ReleaseInfo,
    },
}

/// Avalia o status da atualização. Versões inválidas falham fechadas.
pub fn check_update_status(current_version: &str, latest: ReleaseInfo) -> Result<UpdateStatus> {
    let Some(order) = compare_semver(current_version, &latest.version) else {
        return Err(NeuralError::Config(
            "Versão atual ou release não é SemVer válida".to_string(),
        ));
    };
    if order == Ordering::Greater {
        Ok(UpdateStatus::UpdateAvailable {
            current_version: current_version.to_string(),
            latest,
        })
    } else {
        Ok(UpdateStatus::UpToDate {
            current_version: current_version.to_string(),
        })
    }
}

/// Consulta a última release pública no GitHub através da API REST oficial.
pub fn fetch_latest_release(repo: &str) -> Result<ReleaseInfo> {
    let url = github_latest_release_url(repo);
    let mut response = update_agent()
        .get(&url)
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2026-03-10")
        .header("user-agent", "NeuralIA-App")
        .call()
        .map_err(|e| NeuralError::Config(format!("Falha na consulta ao GitHub: {e}")))?;

    let mut body_str = String::new();
    response
        .body_mut()
        .with_config()
        .limit(RELEASE_JSON_MAX_BYTES)
        .reader()
        .read_to_string(&mut body_str)
        .map_err(|e| NeuralError::Config(format!("Falha ao ler release: {e}")))?;
    parse_github_release_json(&body_str)
}

fn part_path(target_path: &Path) -> Result<PathBuf> {
    let name = target_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| NeuralError::Config("Nome de instalador inválido".to_string()))?;
    let sequence = PART_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    Ok(target_path.with_file_name(format!(
        ".{name}.part-{}-{sequence}",
        std::process::id()
    )))
}

fn sha256_hex(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_verified_installer(
    mut reader: impl Read,
    total_bytes: Option<u64>,
    expected_sha256: &str,
    target_path: &Path,
    mut on_progress: impl FnMut(f64),
) -> Result<()> {
    let expected_sha256 = normalized_sha256(&format!("sha256:{expected_sha256}"))
        .ok_or_else(|| NeuralError::Config("Digest SHA-256 inválido".to_string()))?;
    if total_bytes.is_some_and(|bytes| bytes > INSTALLER_MAX_BYTES) {
        return Err(NeuralError::Config(
            "Instalador excede o limite de 150 MiB".to_string(),
        ));
    }

    let temporary = part_path(target_path)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| {
                NeuralError::Config(format!("Falha ao criar arquivo temporário: {e}"))
            })?;

        let mut buffer = [0u8; DOWNLOAD_BUFFER_BYTES];
        let mut downloaded = 0u64;
        let mut hasher = Sha256::new();
        let mut last_percent = 0u64;
        on_progress(0.0);

        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|e| NeuralError::Config(format!("Erro no stream de download: {e}")))?;
            if read == 0 {
                break;
            }

            downloaded = downloaded.saturating_add(read as u64);
            if downloaded > INSTALLER_MAX_BYTES {
                return Err(NeuralError::Config(
                    "Instalador excede o limite de 150 MiB".to_string(),
                ));
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read]).map_err(|e| {
                NeuralError::Config(format!("Erro ao gravar dados do instalador: {e}"))
            })?;

            if let Some(total) = total_bytes.filter(|total| *total > 0) {
                let percent = downloaded.saturating_mul(100).saturating_div(total).min(99);
                if percent > last_percent {
                    last_percent = percent;
                    on_progress(percent as f64 / 100.0);
                }
            }
        }

        if let Some(total) = total_bytes.filter(|total| *total > 0)
            && downloaded != total
        {
            return Err(NeuralError::Config(format!(
                "Download truncado: esperado {total} bytes, recebido {downloaded}"
            )));
        }

        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|e| NeuralError::Config(format!("Erro ao finalizar instalador: {e}")))?;
        drop(file);

        let actual = sha256_hex(hasher.finalize());
        if actual != expected_sha256 {
            return Err(NeuralError::Config(
                "SHA-256 do instalador não confere com o digest da release".to_string(),
            ));
        }

        match fs::remove_file(target_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(NeuralError::Config(format!(
                    "Falha ao substituir instalador anterior: {error}"
                )));
            }
        }
        fs::rename(&temporary, target_path).map_err(|e| {
            NeuralError::Config(format!("Falha ao promover instalador verificado: {e}"))
        })?;
        on_progress(1.0);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Baixa e verifica o instalador. Nenhum byte é executável pelo chamador antes
/// de o SHA-256 publicado pelo GitHub ter sido conferido.
pub fn download_installer(
    installer_url: &str,
    expected_sha256: &str,
    target_path: &Path,
    on_progress: impl FnMut(f64),
) -> Result<()> {
    if !official_release_download_url(installer_url) {
        return Err(NeuralError::Config(
            "URL de download fora da release oficial".to_string(),
        ));
    }
    if normalized_sha256(&format!("sha256:{expected_sha256}")).is_none() {
        return Err(NeuralError::Config(
            "Digest SHA-256 do instalador é inválido".to_string(),
        ));
    }

    let mut response = update_agent()
        .get(installer_url)
        .header("user-agent", "NeuralIA-App")
        .call()
        .map_err(|e| NeuralError::Config(format!("Falha ao conectar para download: {e}")))?;
    let total_bytes = response.body().content_length();
    let reader = response
        .body_mut()
        .with_config()
        .limit(INSTALLER_MAX_BYTES + 1)
        .reader();

    write_verified_installer(
        reader,
        total_bytes,
        expected_sha256,
        target_path,
        on_progress,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    fn digest(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        sha256_hex(hasher.finalize())
    }

    fn temp_target(name: &str) -> PathBuf {
        let id = PART_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
        let target = std::env::temp_dir().join(format!(
            "neuralia-update-test-{name}-{}-{id}.exe",
            std::process::id()
        ));
        let _ = fs::remove_file(&target);
        target
    }

    fn part_files(target: &Path) -> usize {
        let Some(parent) = target.parent() else {
            return 0;
        };
        let Some(name) = target.file_name().and_then(|name| name.to_str()) else {
            return 0;
        };
        let prefix = format!(".{name}.part-");
        fs::read_dir(parent)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .count()
    }

    #[test]
    fn github_latest_release_url_is_canonical() {
        assert_eq!(
            github_latest_release_url(DEFAULT_GITHUB_REPO),
            "https://api.github.com/repos/JoseRFJuniorLLMs/NeuralIA/releases/latest"
        );
    }

    #[test]
    fn semver_2_precedence_and_build_metadata_are_exact() {
        let cases = [
            ("2.6.2", "2.6.3", Ordering::Greater),
            ("v2.6.2", "v2.7.0", Ordering::Greater),
            ("2.6.2", "2.6.2", Ordering::Equal),
            ("2.6.2", "2.6.1", Ordering::Less),
            ("2.7.0-rc.9", "2.7.0-rc.10", Ordering::Greater),
            ("2.7.0-alpha.2", "2.7.0-alpha.10", Ordering::Greater),
            ("2.7.0-alpha", "2.7.0-alpha.1", Ordering::Greater),
            ("2.7.0-1", "2.7.0-alpha", Ordering::Greater),
            ("2.7.0-rc.1", "2.7.0", Ordering::Greater),
            ("2.7.1+build.1", "2.7.1+build.9", Ordering::Equal),
        ];
        for (current, candidate, expected) in cases {
            assert_eq!(compare_semver(current, candidate), Some(expected), "{current} -> {candidate}");
        }
        for invalid in [
            "2.7",
            "2.7.0.1",
            "02.7.0",
            "2.07.0",
            "2.7.00",
            "2.7.0-01",
            "2.7.0-",
            "2.7.0+",
            "2.7.0+build+again",
        ] {
            assert_eq!(compare_semver("2.7.0", invalid), None, "{invalid}");
        }
    }

    #[test]
    fn release_selects_only_the_exact_versioned_asset_and_requires_digest() {
        let json = r#"{
            "tag_name": "v2.7.2",
            "html_url": "https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/tag/v2.7.2",
            "assets": [
                {
                    "name": "evil.exe",
                    "browser_download_url": "https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/download/v2.7.2/evil.exe",
                    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                {
                    "name": "NeuralIA-Setup-2.7.2-x64.exe",
                    "browser_download_url": "https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/download/v2.7.2/NeuralIA-Setup-2.7.2-x64.exe",
                    "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }
            ]
        }"#;
        let info = parse_github_release_json(json).expect("release válida");
        let installer = info.installer.expect("instalador");
        assert!(installer.url.ends_with("/NeuralIA-Setup-2.7.2-x64.exe"));
        assert_eq!(
            installer.sha256,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );

        let no_digest = json.replace(
            r#""digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef""#,
            r#""digest": null"#,
        );
        assert!(parse_github_release_json(&no_digest).is_err());

        let foreign = json.replace(
            "https://github.com/JoseRFJuniorLLMs/NeuralIA/releases/download/v2.7.2/NeuralIA-Setup-2.7.2-x64.exe",
            "https://example.com/NeuralIA-Setup-2.7.2-x64.exe",
        );
        assert!(parse_github_release_json(&foreign).is_err());
    }

    #[test]
    fn check_update_status_rejects_invalid_current_version() {
        let info = ReleaseInfo {
            tag: "v2.7.2".into(),
            version: "2.7.2".into(),
            html_url: String::new(),
            installer: None,
            published_at: None,
            body: String::new(),
        };
        assert!(check_update_status("2.7", info).is_err());
    }

    #[test]
    fn verified_installer_is_atomic_and_hash_checked() {
        let bytes = b"MZ verified installer";
        let target = temp_target("valid");
        let mut progress = Vec::new();
        write_verified_installer(
            Cursor::new(bytes),
            Some(bytes.len() as u64),
            &digest(bytes),
            &target,
            |value| progress.push(value),
        )
        .expect("instalador válido");

        assert_eq!(fs::read(&target).expect("arquivo final"), bytes);
        assert_eq!(part_files(&target), 0);
        assert_eq!(progress.first().copied(), Some(0.0));
        assert_eq!(progress.last().copied(), Some(1.0));
        let _ = fs::remove_file(target);
    }

    #[test]
    fn digest_mismatch_never_leaves_an_executable_or_part_file() {
        let bytes = b"MZ tampered installer";
        let target = temp_target("digest");
        let wrong = digest(b"outro arquivo");
        let result = write_verified_installer(
            Cursor::new(bytes),
            Some(bytes.len() as u64),
            &wrong,
            &target,
            |_| {},
        );
        assert!(result.is_err());
        assert!(!target.exists());
        assert_eq!(part_files(&target), 0);
    }

    struct FailingReader {
        first: bool,
    }

    impl Read for FailingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.first {
                self.first = false;
                let bytes = b"MZ partial";
                buffer[..bytes.len()].copy_from_slice(bytes);
                Ok(bytes.len())
            } else {
                Err(io::Error::other("synthetic read failure"))
            }
        }
    }

    #[test]
    fn stream_failure_cleans_partial_download() {
        let target = temp_target("stream");
        let result = write_verified_installer(
            FailingReader { first: true },
            None,
            &digest(b"MZ partial"),
            &target,
            |_| {},
        );
        assert!(result.is_err());
        assert!(!target.exists());
        assert_eq!(part_files(&target), 0);
    }

    #[test]
    fn progress_is_coalesced_to_percentage_changes() {
        let bytes = vec![0x5a; DOWNLOAD_BUFFER_BYTES * 4];
        let target = temp_target("progress");
        let mut progress = Vec::new();
        write_verified_installer(
            Cursor::new(&bytes),
            Some(bytes.len() as u64),
            &digest(&bytes),
            &target,
            |value| progress.push(value),
        )
        .expect("download");
        assert!(progress.len() <= 102);
        assert!(progress.windows(2).all(|pair| pair[0] < pair[1]));
        let _ = fs::remove_file(target);
    }
}
