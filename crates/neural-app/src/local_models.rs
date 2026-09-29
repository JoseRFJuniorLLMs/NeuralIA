//! Product-side lifecycle adapter for SPEC-0102 optional model packs.
//!
//! The adapter itself is cheap startup state: it stores only the pack root.
//! `ModelPackManager` is constructed on the first explicit lifecycle action
//! or when a feature actually asks to resolve the active pack. No inference
//! backend is constructed here.

use std::path::{Path, PathBuf};

use neural_core::{
    LocalBenchmark, ModelPackActivation, ModelPackManager, ModelPackManifest, ModelPackSelection,
};

#[derive(Debug)]
pub(crate) struct LocalModelPacks {
    root: PathBuf,
    manager: Option<ModelPackManager>,
}

impl LocalModelPacks {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("model-packs"),
            manager: None,
        }
    }

    fn manager(&mut self) -> &ModelPackManager {
        self.manager
            .get_or_insert_with(|| ModelPackManager::new(self.root.clone()))
    }

    /// Resolve only when a product feature actually needs local intelligence.
    /// Missing/corrupt/stale packs remain a deterministic fallback.
    pub(crate) fn resolve_for_task(&mut self) -> ModelPackSelection {
        self.manager().selection()
    }

    /// Installation is deliberately an explicit product action. This function
    /// never downloads anything and never activates the installed pack.
    pub(crate) fn install_explicit(
        &mut self,
        manifest: &ModelPackManifest,
        model_bytes: &[u8],
    ) -> Result<PathBuf, String> {
        self.manager().install(manifest, model_bytes)
    }

    pub(crate) fn record_benchmark_explicit(
        &mut self,
        id: &str,
        benchmark: &LocalBenchmark,
    ) -> Result<PathBuf, String> {
        self.manager().record_benchmark(id, benchmark)
    }

    /// Core activation refuses missing/invalid benchmark data before writing
    /// active state. The adapter preserves that contract unchanged.
    pub(crate) fn activate_explicit(&mut self, id: &str) -> Result<ModelPackActivation, String> {
        self.manager().activate(id)
    }

    pub(crate) fn deactivate_explicit(&mut self) -> Result<bool, String> {
        self.manager().deactivate()
    }

    pub(crate) fn uninstall_explicit(&mut self, id: &str) -> Result<bool, String> {
        self.manager().uninstall(id)
    }

    #[cfg(test)]
    fn manager_initialized(&self) -> bool {
        self.manager.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NONCE: AtomicU64 = AtomicU64::new(1);
    const MODEL: &[u8] = b"model-bytes";
    const MODEL_SHA256: &str = "357e5d6fafa34d27360fec24b4326d3534905e33c6acdee60198fb078b7b79e5";

    struct Fixture {
        root: PathBuf,
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
            Self { root }
        }

        fn packs(&self) -> LocalModelPacks {
            LocalModelPacks::new(&self.root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
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
            measured_at: 1,
        }
    }

    #[test]
    fn startup_state_does_not_construct_manager_or_touch_pack_directory() {
        let fixture = Fixture::new();
        let packs = fixture.packs();

        assert!(!packs.manager_initialized());
        assert!(!fixture.root.join("model-packs").exists());
    }

    #[test]
    fn activation_requires_a_recorded_benchmark() {
        let fixture = Fixture::new();
        let mut packs = fixture.packs();
        let manifest = manifest();

        packs.install_explicit(&manifest, MODEL).unwrap();
        let error = packs.activate_explicit(&manifest.id).unwrap_err();

        assert!(error.contains("benchmark"), "{error}");
        let selection = packs.resolve_for_task();
        assert!(selection.active.is_none());
    }

    #[test]
    fn explicit_lifecycle_activates_then_uninstall_returns_to_fallback() {
        let fixture = Fixture::new();
        let mut packs = fixture.packs();
        let manifest = manifest();

        packs.install_explicit(&manifest, MODEL).unwrap();
        packs
            .record_benchmark_explicit(&manifest.id, &benchmark())
            .unwrap();
        packs.activate_explicit(&manifest.id).unwrap();

        let selected = packs.resolve_for_task();
        assert_eq!(
            selected
                .active
                .as_ref()
                .map(|pack| pack.manifest.id.as_str()),
            Some("semantic-small")
        );
        assert!(selected.warning.is_none());

        assert!(packs.uninstall_explicit(&manifest.id).unwrap());
        let fallback = packs.resolve_for_task();
        assert!(fallback.active.is_none());
        assert!(fallback.warning.is_none());
    }

    #[test]
    fn corrupted_active_pack_degrades_to_diagnostic_fallback() {
        let fixture = Fixture::new();
        let mut packs = fixture.packs();
        let manifest = manifest();

        let model_path = packs.install_explicit(&manifest, MODEL).unwrap();
        packs
            .record_benchmark_explicit(&manifest.id, &benchmark())
            .unwrap();
        packs.activate_explicit(&manifest.id).unwrap();
        fs::write(model_path, b"tampered").unwrap();

        let fallback = packs.resolve_for_task();
        assert!(fallback.active.is_none());
        assert!(
            fallback
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("hash")),
            "{:?}",
            fallback.warning
        );
    }

    #[test]
    fn deactivation_is_explicit_and_idempotent() {
        let fixture = Fixture::new();
        let mut packs = fixture.packs();

        assert!(!packs.deactivate_explicit().unwrap());
        assert!(!packs.deactivate_explicit().unwrap());
    }
}
