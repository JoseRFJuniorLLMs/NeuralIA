//! Product lifecycle boundary for SPEC-0102 optional model packs.
//!
//! Startup stores only a path. The manager is created on the first explicit
//! model command, and this module never creates an inference backend or grants
//! browser/agent authority to a model.

use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use neural_core::{
    LocalBenchmark, ModelPackActivation, ModelPackManager, ModelPackManifest, ModelPackSelection,
    json_store::StoreGrant,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalModelCommand {
    Status,
    Install(PathBuf),
    Activate(String),
    Deactivate,
    Uninstall(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalModelOutcome {
    Fallback { warning: Option<String> },
    Active { id: String, version: String },
    Installed { id: String },
    Activated { id: String },
    Deactivated { changed: bool },
    Uninstalled { id: String, removed: bool },
}

impl LocalModelOutcome {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Fallback {
                warning: Some(warning),
            } => {
                format!("Modelo local: fallback determinístico ({warning}).")
            }
            Self::Fallback { warning: None } => {
                "Modelo local: fallback determinístico; nenhum pack ativo.".to_string()
            }
            Self::Active { id, version } => format!("Modelo local ativo: {id} v{version}."),
            Self::Installed { id } => {
                format!("Model pack {id} instalado; não foi ativado automaticamente.")
            }
            Self::Activated { id } => format!("Model pack {id} ativado."),
            Self::Deactivated { changed: true } => "Model pack desativado.".to_string(),
            Self::Deactivated { changed: false } => "Nenhum model pack estava ativo.".to_string(),
            Self::Uninstalled { id, removed: true } => {
                format!("Model pack {id} removido; fallback determinístico ativo.")
            }
            Self::Uninstalled { id, removed: false } => {
                format!("Model pack {id} não estava instalado.")
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct LocalModelPacks {
    manager: Option<ModelPackManager>,
    root: PathBuf,
    /// Capacidade que prova que esta raiz e a loja Explicit declarada pelo produto.
    /// Mantemo-la viva em vez de degradar o boundary novamente a um PathBuf nu.
    _grant: StoreGrant,
}

const MAX_IMPORT_MANIFEST_BYTES: u64 = 64 * 1024;

impl LocalModelPacks {
    pub(crate) fn new(grant: StoreGrant) -> Result<Self, String> {
        let expected = crate::stores::MODEL_PACKS_STORE;
        if (grant.name(), grant.kind(), grant.shape())
            != (expected.name, expected.kind, expected.shape)
        {
            return Err("model-pack adapter exige o grant MODEL_PACKS_STORE".into());
        }
        let root = grant.path().to_path_buf();
        Ok(Self {
            manager: None,
            root,
            _grant: grant,
        })
    }

    fn manager(&mut self) -> &ModelPackManager {
        self.manager
            .get_or_insert_with(|| ModelPackManager::new(self.root.clone()))
    }

    /// No backend/model bytes are stored by this product adapter. It is useful
    /// to expose this invariant to the real startup E2E instead of pretending
    /// that an Option<ModelPackManager> is a memory measurement.
    pub(crate) const fn resident_model_bytes(&self) -> usize {
        0
    }

    pub(crate) fn manager_initialized(&self) -> bool {
        self.manager.is_some()
    }

    /// Missing/corrupt/stale packs degrade to the deterministic fallback.
    pub(crate) fn resolve_for_task(&mut self) -> ModelPackSelection {
        self.manager().selection()
    }

    /// Import a local manifest chosen by the user. The model named in the
    /// manifest must be a sibling plain filename; traversal is rejected before
    /// any model path is read. Core validates manifest/hash/license again.
    fn install_manifest(&mut self, manifest_path: &Path) -> Result<String, String> {
        let manifest_file = fs::File::open(manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        let mut manifest_bytes = Vec::new();
        manifest_file
            .take(MAX_IMPORT_MANIFEST_BYTES + 1)
            .read_to_end(&mut manifest_bytes)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        if manifest_bytes.len() as u64 > MAX_IMPORT_MANIFEST_BYTES {
            return Err("manifest do model pack excede 64 KiB".into());
        }
        let manifest: ModelPackManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
        if !plain_filename(&manifest.file) {
            return Err(
                "arquivo do model pack deve ser um nome simples ao lado do manifest".into(),
            );
        }
        let parent = manifest_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let model_path = parent.join(&manifest.file);
        self.manager().install_from_file(&manifest, &model_path)?;
        Ok(manifest.id)
    }

    pub(crate) fn record_benchmark_explicit(
        &mut self,
        id: &str,
        benchmark: &LocalBenchmark,
    ) -> Result<PathBuf, String> {
        self.manager().record_benchmark(id, benchmark)
    }

    fn activate_explicit(&mut self, id: &str) -> Result<ModelPackActivation, String> {
        self.manager().activate(id)
    }

    fn deactivate_explicit(&mut self) -> Result<bool, String> {
        self.manager().deactivate()
    }

    fn uninstall_explicit(&mut self, id: &str) -> Result<bool, String> {
        self.manager().uninstall(id)
    }
}

fn plain_filename(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !value.contains(':')
        && !value.contains('/')
        && !value.contains('\\')
}

pub(crate) fn parse_local_model_command(input: &str) -> Option<Result<LocalModelCommand, String>> {
    let trimmed = input.trim();
    let rest = trimmed
        .strip_prefix("model:")
        .or_else(|| trimmed.strip_prefix("modelo:"))?;

    if rest.eq_ignore_ascii_case("status") || rest.eq_ignore_ascii_case("resolve") {
        return Some(Ok(LocalModelCommand::Status));
    }
    if rest.eq_ignore_ascii_case("deactivate") || rest.eq_ignore_ascii_case("desativar") {
        return Some(Ok(LocalModelCommand::Deactivate));
    }

    for (prefix, build) in [
        (
            "install:",
            LocalModelCommand::Install as fn(PathBuf) -> LocalModelCommand,
        ),
        ("instalar:", LocalModelCommand::Install),
    ] {
        if let Some(value) = rest.strip_prefix(prefix) {
            let path = value.trim().trim_matches('"').trim();
            return Some(
                (!path.is_empty())
                    .then(|| build(PathBuf::from(path)))
                    .ok_or_else(|| "informe o caminho do manifest.json".to_string()),
            );
        }
    }
    for prefix in ["activate:", "ativar:"] {
        if let Some(value) = rest.strip_prefix(prefix) {
            let id = value.trim();
            return Some(
                (!id.is_empty())
                    .then(|| LocalModelCommand::Activate(id.to_string()))
                    .ok_or_else(|| "informe o id do model pack".to_string()),
            );
        }
    }
    for prefix in ["uninstall:", "remover:"] {
        if let Some(value) = rest.strip_prefix(prefix) {
            let id = value.trim();
            return Some(
                (!id.is_empty())
                    .then(|| LocalModelCommand::Uninstall(id.to_string()))
                    .ok_or_else(|| "informe o id do model pack".to_string()),
            );
        }
    }

    Some(Err(
        "Use model:status, model:install:<manifest>, model:activate:<id>, model:deactivate ou model:uninstall:<id>."
            .to_string(),
    ))
}

/// This is the same executor called by the shipped omnibox route. Keeping the
/// decision here lets behavior tests exercise the production path instead of
/// grepping source text.
pub(crate) fn execute_local_model_command(
    packs: &mut LocalModelPacks,
    command: LocalModelCommand,
) -> Result<LocalModelOutcome, String> {
    match command {
        LocalModelCommand::Status => {
            let selection = packs.resolve_for_task();
            Ok(match selection.active {
                Some(active) => LocalModelOutcome::Active {
                    id: active.manifest.id,
                    version: active.manifest.version,
                },
                None => LocalModelOutcome::Fallback {
                    warning: selection.warning,
                },
            })
        }
        LocalModelCommand::Install(path) => {
            let id = packs.install_manifest(&path)?;
            Ok(LocalModelOutcome::Installed { id })
        }
        LocalModelCommand::Activate(id) => {
            let activation = packs.activate_explicit(&id)?;
            Ok(LocalModelOutcome::Activated { id: activation.id })
        }
        LocalModelCommand::Deactivate => {
            let changed = packs.deactivate_explicit()?;
            Ok(LocalModelOutcome::Deactivated { changed })
        }
        LocalModelCommand::Uninstall(id) => {
            let removed = packs.uninstall_explicit(&id)?;
            Ok(LocalModelOutcome::Uninstalled { id, removed })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NONCE: AtomicU64 = AtomicU64::new(1);
    const MODEL: &[u8] = b"model-bytes";
    const MODEL_SHA256: &str = "357e5d6fafa34d27360fec24b4326d3534905e33c6acdee60198fb078b7b79e5";

    struct Fixture {
        data: PathBuf,
        import: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "neuralia-local-model-packs-{}-{stamp}-{nonce}",
                std::process::id()
            ));
            Self {
                data: root.join("data"),
                import: root.join("import"),
            }
        }

        fn packs(&self) -> LocalModelPacks {
            let registry = neural_core::json_store::StoreRegistry::mint_for_test(&self.data);
            let grant = registry
                .grant(crate::stores::MODEL_PACKS_STORE)
                .expect("model-pack store grant");
            LocalModelPacks::new(grant).expect("model-pack adapter")
        }

        fn write_import(&self) -> PathBuf {
            fs::create_dir_all(&self.import).unwrap();
            fs::write(self.import.join("semantic.bin"), MODEL).unwrap();
            let path = self.import.join("manifest.json");
            fs::write(&path, serde_json::to_vec_pretty(&manifest()).unwrap()).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(root) = self.data.parent() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }

    fn manifest() -> ModelPackManifest {
        ModelPackManifest {
            id: "semantic-small".into(),
            version: "1.0.0".into(),
            file: "semantic.bin".into(),
            sha256: MODEL_SHA256.into(),
            capabilities: vec!["embeddings".into(), "intent".into()],
            license: "Apache-2.0".into(),
        }
    }

    fn benchmark() -> LocalBenchmark {
        LocalBenchmark {
            backend: "fixture-backend".into(),
            samples: 4,
            embedding_dimension: 384,
            embed_micros_total: 120,
            classify_micros_total: 80,
            resident_model_bytes: Some(0),
            measured_at: 1,
        }
    }

    #[test]
    fn product_adapter_rejects_a_grant_for_another_store() {
        let fixture = Fixture::new();
        let registry = neural_core::json_store::StoreRegistry::mint_for_test(&fixture.data);
        let wrong = registry
            .grant(neural_core::json_store::StoreSpec::new(
                "other-model-packs",
                neural_core::json_store::StoreKind::Explicit,
                neural_core::json_store::StoreShape::Dir,
            ))
            .expect("wrong-store fixture grant");
        let error = LocalModelPacks::new(wrong).expect_err("wrong grant must be refused");
        assert!(error.contains("MODEL_PACKS_STORE"), "{error}");
    }

    #[test]
    fn startup_state_has_zero_model_residency_and_does_no_pack_io() {
        let fixture = Fixture::new();
        let packs = fixture.packs();
        assert_eq!(packs.resident_model_bytes(), 0);
        assert!(!packs.manager_initialized());
        assert!(!fixture.data.join("model-packs").exists());
    }

    #[test]
    fn command_parser_is_closed_and_requires_explicit_actions() {
        assert_eq!(
            parse_local_model_command("model:status"),
            Some(Ok(LocalModelCommand::Status))
        );
        assert_eq!(
            parse_local_model_command("modelo:desativar"),
            Some(Ok(LocalModelCommand::Deactivate))
        );
        assert_eq!(parse_local_model_command("pesquisa comum"), None);
        assert!(matches!(
            parse_local_model_command("model:install:"),
            Some(Err(_))
        ));
        assert!(matches!(
            parse_local_model_command("model:qualquer-coisa"),
            Some(Err(_))
        ));
    }

    #[test]
    fn shipped_executor_installs_without_auto_activation_and_requires_benchmark() {
        let fixture = Fixture::new();
        let mut packs = fixture.packs();
        let manifest_path = fixture.write_import();

        assert_eq!(
            execute_local_model_command(&mut packs, LocalModelCommand::Install(manifest_path))
                .unwrap(),
            LocalModelOutcome::Installed {
                id: "semantic-small".into()
            }
        );
        assert_eq!(
            execute_local_model_command(&mut packs, LocalModelCommand::Status).unwrap(),
            LocalModelOutcome::Fallback { warning: None }
        );
        let error = execute_local_model_command(
            &mut packs,
            LocalModelCommand::Activate("semantic-small".into()),
        )
        .unwrap_err();
        assert!(error.contains("benchmark"), "{error}");
    }

    #[test]
    fn oversized_import_manifest_is_rejected_before_pack_io() {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.import).unwrap();
        let path = fixture.import.join("manifest.json");
        fs::write(&path, vec![b' '; MAX_IMPORT_MANIFEST_BYTES as usize + 1]).unwrap();
        let mut packs = fixture.packs();
        let error =
            execute_local_model_command(&mut packs, LocalModelCommand::Install(path)).unwrap_err();
        assert!(error.contains("64 KiB"), "{error}");
        assert!(!packs.manager_initialized());
        assert!(!fixture.data.join("model-packs").exists());
    }

    #[test]
    fn shipped_executor_activates_then_uninstall_returns_to_fallback() {
        let fixture = Fixture::new();
        let mut packs = fixture.packs();
        let manifest_path = fixture.write_import();
        execute_local_model_command(&mut packs, LocalModelCommand::Install(manifest_path)).unwrap();
        packs
            .record_benchmark_explicit("semantic-small", &benchmark())
            .unwrap();

        assert_eq!(
            execute_local_model_command(
                &mut packs,
                LocalModelCommand::Activate("semantic-small".into())
            )
            .unwrap(),
            LocalModelOutcome::Activated {
                id: "semantic-small".into()
            }
        );
        assert!(matches!(
            execute_local_model_command(&mut packs, LocalModelCommand::Status).unwrap(),
            LocalModelOutcome::Active { .. }
        ));

        assert_eq!(
            execute_local_model_command(
                &mut packs,
                LocalModelCommand::Uninstall("semantic-small".into())
            )
            .unwrap(),
            LocalModelOutcome::Uninstalled {
                id: "semantic-small".into(),
                removed: true
            }
        );
        assert_eq!(
            execute_local_model_command(&mut packs, LocalModelCommand::Status).unwrap(),
            LocalModelOutcome::Fallback { warning: None }
        );
    }

    #[test]
    fn shipped_status_degrades_corruption_to_diagnostic_fallback() {
        let fixture = Fixture::new();
        let mut packs = fixture.packs();
        let manifest_path = fixture.write_import();
        execute_local_model_command(&mut packs, LocalModelCommand::Install(manifest_path)).unwrap();
        packs
            .record_benchmark_explicit("semantic-small", &benchmark())
            .unwrap();
        execute_local_model_command(
            &mut packs,
            LocalModelCommand::Activate("semantic-small".into()),
        )
        .unwrap();

        fs::write(
            fixture
                .data
                .join("model-packs")
                .join("semantic-small")
                .join("semantic.bin"),
            b"tampered",
        )
        .unwrap();

        let status = execute_local_model_command(&mut packs, LocalModelCommand::Status).unwrap();
        assert!(matches!(
            status,
            LocalModelOutcome::Fallback {
                warning: Some(ref warning)
            } if warning.contains("hash")
        ));
    }

    #[test]
    fn install_rejects_manifest_file_traversal_before_reading_outside_bundle() {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.import).unwrap();
        let mut bad = manifest();
        bad.file = "../outside.bin".into();
        let path = fixture.import.join("manifest.json");
        fs::write(&path, serde_json::to_vec_pretty(&bad).unwrap()).unwrap();
        let outside = fixture.import.parent().unwrap().join("outside.bin");
        fs::write(&outside, MODEL).unwrap();

        let mut packs = fixture.packs();
        let error =
            execute_local_model_command(&mut packs, LocalModelCommand::Install(path)).unwrap_err();
        assert!(error.contains("nome simples"), "{error}");
        assert!(!fixture.data.join("model-packs").exists());
    }
}
