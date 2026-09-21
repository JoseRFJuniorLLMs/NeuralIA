use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const EMBEDDING_DIM: usize = 384;

static MODEL_PACK_NONCE: AtomicU64 = AtomicU64::new(1);
const MODEL_PACK_ACTIVE_FILE: &str = "active.json";
const MODEL_PACK_BENCHMARK_FILE: &str = "benchmark.json";
const MODEL_PACK_HASH_HEX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentClass {
    MemoryRecall,
    Research,
    Navigate,
    AgentTask,
    General,
}

pub trait LocalIntelligence {
    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>, String>;
    fn classify(&self, input: &str) -> Result<IntentClass, String>;
    fn entities(&self, input: &str) -> Result<Vec<String>, String>;
    fn summarize(&self, input: &str, budget: usize) -> Result<String, String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HashingLocalIntelligence;

impl LocalIntelligence for HashingLocalIntelligence {
    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(input.iter().map(|text| hashed_embedding(text)).collect())
    }

    fn classify(&self, input: &str) -> Result<IntentClass, String> {
        let lower = input.to_lowercase();
        let memory = [
            "onde",
            "lembra",
            "lembre",
            "histórico",
            "historico",
            "pesquisei",
            "li ",
        ];
        let research = [
            "compare",
            "comparar",
            "pesquise",
            "pesquisar",
            "fontes",
            "evidência",
            "evidencia",
        ];
        let navigate = [
            "abra ", "abrir ", "vá para", "va para", "http://", "https://",
        ];
        let agent = [
            "faça por mim",
            "faca por mim",
            "preencha",
            "clique",
            "envie",
            "reserve",
            "extraia",
        ];

        Ok(if memory.iter().any(|term| lower.contains(term)) {
            IntentClass::MemoryRecall
        } else if research.iter().any(|term| lower.contains(term)) {
            IntentClass::Research
        } else if navigate.iter().any(|term| lower.contains(term)) {
            IntentClass::Navigate
        } else if agent.iter().any(|term| lower.contains(term)) {
            IntentClass::AgentTask
        } else {
            IntentClass::General
        })
    }

    fn entities(&self, input: &str) -> Result<Vec<String>, String> {
        Ok(extract_entities(input))
    }

    fn summarize(&self, input: &str, budget: usize) -> Result<String, String> {
        if budget == 0 {
            return Ok(String::new());
        }
        let clean = input.split_whitespace().collect::<Vec<_>>().join(" ");
        if clean.chars().count() <= budget {
            return Ok(clean);
        }

        // A elipse também consome budget. Reservá-la antes de escolher frases
        // evita que um resumo que caiba exactamente no limite cresça um
        // carácter ao indicar truncamento.
        let content_budget = budget.saturating_sub(1);
        let mut output = String::new();
        let mut used = 0usize;
        for sentence in clean.split_inclusive(['.', '!', '?']) {
            let sentence = sentence.trim();
            if sentence.is_empty() {
                continue;
            }
            let sentence_chars = sentence.chars().count();
            let separator = usize::from(!output.is_empty());
            if used + separator + sentence_chars > content_budget {
                break;
            }
            if separator != 0 {
                output.push(' ');
                used += 1;
            }
            output.push_str(sentence);
            used += sentence_chars;
        }
        if output.is_empty() {
            output = clean.chars().take(content_budget).collect();
        }
        output.push('…');
        Ok(output)
    }
}

pub fn hashed_embedding(text: &str) -> Vec<f32> {
    let normalized = text.to_lowercase();
    let chars = normalized.chars().collect::<Vec<_>>();
    let mut vector = vec![0.0f32; EMBEDDING_DIM];

    for word in normalized.split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-') {
        if word.is_empty() {
            continue;
        }
        add_feature(&mut vector, word.as_bytes(), 1.4);

        // Uma camada semântica mínima e determinística melhora o recall offline
        // entre português/inglês e equivalentes técnicos sem carregar modelo.
        // Ela é fallback: model packs podem fornecer embeddings reais depois.
        let canonical = canonical_semantic_token(word);
        if canonical != word {
            add_feature(&mut vector, canonical.as_bytes(), 1.8);
        }
    }

    for width in 3..=5 {
        if chars.len() < width {
            continue;
        }
        for gram in chars.windows(width) {
            let feature = gram.iter().collect::<String>();
            add_feature(&mut vector, feature.as_bytes(), 0.35);
        }
    }

    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

/// A camada semântica mínima, numa tabela só: a forma canónica e as palavras
/// que lhe chegam. Duas funções leem daqui -- a que gera a feature do
/// embedding e a que alarga a pesquisa lexical -- porque duas tabelas com os
/// mesmos sinónimos divergem sem nada as apanhar.
const SEMANTIC_GROUPS: &[&[&str]] = &[
    &[
        "simd-vector",
        "simd",
        "vetor",
        "vetores",
        "vetorial",
        "vector",
        "vectors",
        "vectorial",
    ],
    &[
        "cpu-processor",
        "cpu",
        "cpus",
        "processador",
        "processadores",
        "processor",
        "processors",
    ],
    &[
        "optimization",
        "otimização",
        "otimizacao",
        "otimizar",
        "optimize",
        "optimise",
    ],
    &["memory", "memória", "memoria"],
    &["browser", "navegador", "navegadores", "browsers"],
    &["research", "pesquisa", "pesquisar"],
    &["agent", "agente", "agentes", "agents"],
    &["security", "segurança", "seguranca"],
    &["source", "fonte", "fontes", "sources"],
    &["answer", "resposta", "respostas", "answers"],
    &["code", "código", "codigo"],
];

fn semantic_group(word: &str) -> Option<&'static [&'static str]> {
    SEMANTIC_GROUPS
        .iter()
        .find(|group| group.contains(&word))
        .copied()
}

fn canonical_semantic_token(word: &str) -> &str {
    match semantic_group(word) {
        Some(group) => group[0],
        None => word,
    }
}

/// As palavras que partilham o sentido de `word`, ela incluída, ou nada quando
/// a palavra não está na tabela.
///
/// Serve o candidato lexical: sem isto, a ponte português/inglês que o
/// `hashed_embedding` constrói só chega ao reranking se o documento já tiver
/// entrado na lista de candidatos por partilhar palavras -- e um documento em
/// inglês nunca partilha a palavra portuguesa que se procurou.
pub fn semantic_expansion(word: &str) -> &'static [&'static str] {
    semantic_group(word).unwrap_or(&[])
}

fn add_feature(vector: &mut [f32], bytes: &[u8], weight: f32) {
    let digest = Sha256::digest(bytes);
    let index = u16::from_le_bytes([digest[0], digest[1]]) as usize % vector.len();
    let sign = if digest[2] & 1 == 0 { 1.0 } else { -1.0 };
    vector[index] += sign * weight;
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (left, right) in left.iter().zip(right) {
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }

    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

pub fn extract_entities(input: &str) -> Vec<String> {
    let mut entities = BTreeSet::new();

    for word in input
        .split(|ch: char| !ch.is_alphanumeric() && ch != '-' && ch != '_')
        .filter(|word| word.chars().count() >= 2)
    {
        let letters = word
            .chars()
            .filter(|ch| ch.is_alphabetic())
            .collect::<Vec<_>>();
        let upper = !letters.is_empty() && letters.iter().all(|ch| ch.is_uppercase());
        let capitalized = word.chars().next().is_some_and(char::is_uppercase);
        let technical =
            word.contains('-') || word.contains('_') || word.chars().any(|ch| ch.is_ascii_digit());

        if upper || capitalized || technical {
            entities.insert(word.to_string());
        }
    }

    entities.into_iter().take(64).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPackManifest {
    pub id: String,
    pub version: String,
    pub file: String,
    pub sha256: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub license: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalBenchmark {
    pub backend: String,
    pub samples: usize,
    pub embedding_dimension: usize,
    pub embed_micros_total: u128,
    pub classify_micros_total: u128,
    pub measured_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPackActivation {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub activated_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveModelPack {
    pub manifest: ModelPackManifest,
    pub model_path: PathBuf,
    pub benchmark: LocalBenchmark,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelPackSelection {
    pub active: Option<ActiveModelPack>,
    pub warning: Option<String>,
}

impl ModelPackSelection {
    pub fn deterministic_fallback() -> Self {
        Self {
            active: None,
            warning: None,
        }
    }

    fn fallback_with_warning(warning: impl Into<String>) -> Self {
        Self {
            active: None,
            warning: Some(warning.into()),
        }
    }
}

pub fn benchmark_local_intelligence(
    backend: &str,
    ai: &dyn LocalIntelligence,
    corpus: &[String],
) -> Result<LocalBenchmark, String> {
    let samples = corpus.len();
    let embed_started = Instant::now();
    let embeddings = ai.embed(corpus)?;
    let embed_micros_total = embed_started.elapsed().as_micros();

    let classify_started = Instant::now();
    for sample in corpus {
        let _ = ai.classify(sample)?;
    }
    let classify_micros_total = classify_started.elapsed().as_micros();

    let dimension = embeddings.first().map(Vec::len).unwrap_or(0);
    if embeddings
        .iter()
        .any(|embedding| embedding.len() != dimension)
    {
        return Err("backend returned inconsistent embedding dimensions".into());
    }

    Ok(LocalBenchmark {
        backend: backend.to_string(),
        samples,
        embedding_dimension: dimension,
        embed_micros_total,
        classify_micros_total,
        measured_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    })
}

/// Library-only filesystem utility for optional model-pack artifacts.
///
/// This type verifies, stages, lists and removes pack files. It deliberately
/// does not download packs, select an inference backend, activate a global
/// model or hook itself into browser startup. As of SPEC-0102's current
/// partial state, `neural-app` does not instantiate it; product lifecycle
/// wiring requires a separate measured integration.
#[derive(Debug, Clone)]
pub struct ModelPackManager {
    root: PathBuf,
}

impl ModelPackManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn load_manifest(&self, id: &str) -> Result<ModelPackManifest, String> {
        validate_pack_component(id)?;
        let path = self.root.join(id).join("manifest.json");
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let manifest: ModelPackManifest =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if manifest.id != id {
            return Err(format!(
                "manifest id {} não corresponde ao diretório {id}",
                manifest.id
            ));
        }
        validate_manifest(&manifest)?;
        Ok(manifest)
    }

    pub fn verify(&self, manifest: &ModelPackManifest) -> Result<PathBuf, String> {
        validate_manifest(manifest)?;

        let pack = self.root.join(&manifest.id);
        reject_symlink(&pack, "diretório do model pack")?;
        let manifest_path = pack.join("manifest.json");
        reject_symlink(&manifest_path, "manifest do model pack")?;

        let path = pack.join(&manifest.file);
        reject_symlink(&path, "ficheiro do model pack")?;
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let actual = format!("{:x}", Sha256::digest(bytes));
        if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
            return Err(format!("hash do model pack {} não confere", manifest.id));
        }
        Ok(path)
    }

    pub fn installed(&self) -> Result<Vec<ModelPackManifest>, String> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(Vec::new());
        };

        let mut packs = Vec::new();
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if id.starts_with('.') {
                continue;
            }
            if let Ok(manifest) = self.load_manifest(&id) {
                packs.push(manifest);
            }
        }
        packs.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(packs)
    }

    pub fn install(
        &self,
        manifest: &ModelPackManifest,
        model_bytes: &[u8],
    ) -> Result<PathBuf, String> {
        validate_manifest(manifest)?;
        let actual = format!("{:x}", Sha256::digest(model_bytes));
        if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
            return Err(format!("hash do model pack {} não confere", manifest.id));
        }

        fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let pack = self.root.join(&manifest.id);
        let nonce = MODEL_PACK_NONCE.fetch_add(1, Ordering::Relaxed);
        let suffix = format!("{}-{nonce}", std::process::id());
        let staging = self.root.join(format!(".{}.install-{suffix}", manifest.id));
        let backup = self.root.join(format!(".{}.backup-{suffix}", manifest.id));

        fs::create_dir(&staging).map_err(|error| error.to_string())?;
        let staged_model = staging.join(&manifest.file);
        let staged_manifest = staging.join("manifest.json");

        let stage_result = (|| -> Result<(), String> {
            fs::write(&staged_model, model_bytes).map_err(|error| error.to_string())?;
            fs::write(
                &staged_manifest,
                serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;

            let staged_bytes = fs::read(&staged_model).map_err(|error| error.to_string())?;
            let staged_hash = format!("{:x}", Sha256::digest(staged_bytes));
            if !staged_hash.eq_ignore_ascii_case(manifest.sha256.trim()) {
                return Err("model pack staging hash mismatch".into());
            }
            Ok(())
        })();

        if let Err(error) = stage_result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let had_existing = pack.exists();
        if had_existing {
            fs::rename(&pack, &backup).map_err(|error| {
                let _ = fs::remove_dir_all(&staging);
                error.to_string()
            })?;
        }

        if let Err(error) = fs::rename(&staging, &pack) {
            if had_existing {
                let _ = fs::rename(&backup, &pack);
            }
            let _ = fs::remove_dir_all(&staging);
            return Err(error.to_string());
        }

        if had_existing {
            let _ = fs::remove_dir_all(&backup);
        }

        Ok(pack.join(&manifest.file))
    }

    pub fn uninstall(&self, id: &str) -> Result<bool, String> {
        validate_pack_component(id)?;
        let path = self.root.join(id);
        if !path.exists() {
            return Ok(false);
        }

        if self
            .read_activation()?
            .as_ref()
            .is_some_and(|activation| activation.id == id)
        {
            self.deactivate()?;
        }

        fs::remove_dir_all(path).map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// Ativação explícita do artefacto, ainda no nível de biblioteca.
    ///
    /// A ativação só grava estado depois de manifest, licença, hash e benchmark
    /// terem sido verificados. Ela NÃO carrega um backend nem toca no browser.
    pub fn activate(&self, id: &str) -> Result<ModelPackActivation, String> {
        let manifest = self.load_manifest(id)?;
        self.verify(&manifest)?;
        let benchmark = self.load_benchmark(id)?;
        validate_benchmark(&benchmark)?;

        let activation = ModelPackActivation {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            sha256: manifest.sha256.to_ascii_lowercase(),
            activated_at: now_unix_seconds(),
        };
        self.write_activation(&activation)?;
        Ok(activation)
    }

    pub fn deactivate(&self) -> Result<bool, String> {
        let path = self.root.join(MODEL_PACK_ACTIVE_FILE);
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Resolve o pack ativo sem sacrificar o fallback determinístico.
    ///
    /// Estado ausente => fallback limpo. Estado corrompido, pack apagado,
    /// hash alterado ou benchmark inválido => fallback + warning diagnóstico.
    pub fn selection(&self) -> ModelPackSelection {
        let activation = match self.read_activation() {
            Ok(Some(activation)) => activation,
            Ok(None) => return ModelPackSelection::deterministic_fallback(),
            Err(error) => return ModelPackSelection::fallback_with_warning(error),
        };

        let manifest = match self.load_manifest(&activation.id) {
            Ok(manifest) => manifest,
            Err(error) => return ModelPackSelection::fallback_with_warning(error),
        };
        if manifest.version != activation.version
            || !manifest
                .sha256
                .eq_ignore_ascii_case(activation.sha256.trim())
        {
            return ModelPackSelection::fallback_with_warning(format!(
                "estado ativo do model pack {} não corresponde ao manifest instalado",
                activation.id
            ));
        }

        let model_path = match self.verify(&manifest) {
            Ok(path) => path,
            Err(error) => return ModelPackSelection::fallback_with_warning(error),
        };
        let benchmark = match self.load_benchmark(&manifest.id) {
            Ok(benchmark) => benchmark,
            Err(error) => return ModelPackSelection::fallback_with_warning(error),
        };
        if let Err(error) = validate_benchmark(&benchmark) {
            return ModelPackSelection::fallback_with_warning(error);
        }

        ModelPackSelection {
            active: Some(ActiveModelPack {
                manifest,
                model_path,
                benchmark,
            }),
            warning: None,
        }
    }

    fn load_benchmark(&self, id: &str) -> Result<LocalBenchmark, String> {
        validate_pack_component(id)?;
        let path = self.root.join(id).join(MODEL_PACK_BENCHMARK_FILE);
        reject_symlink(&path, "benchmark do model pack")?;
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }

    fn read_activation(&self) -> Result<Option<ModelPackActivation>, String> {
        let path = self.root.join(MODEL_PACK_ACTIVE_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let activation: ModelPackActivation =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        validate_pack_component(&activation.id)?;
        validate_pack_version(&activation.version)?;
        validate_sha256(&activation.sha256)?;
        Ok(Some(activation))
    }

    fn write_activation(&self, activation: &ModelPackActivation) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let nonce = MODEL_PACK_NONCE.fetch_add(1, Ordering::Relaxed);
        let temp = self.root.join(format!(
            ".active.json.tmp-{}-{nonce}",
            std::process::id()
        ));
        let final_path = self.root.join(MODEL_PACK_ACTIVE_FILE);
        let backup = self.root.join(format!(
            ".active.json.backup-{}-{nonce}",
            std::process::id()
        ));

        fs::write(
            &temp,
            serde_json::to_vec_pretty(activation).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;

        let had_existing = final_path.exists();
        if had_existing {
            fs::rename(&final_path, &backup).map_err(|error| {
                let _ = fs::remove_file(&temp);
                error.to_string()
            })?;
        }

        if let Err(error) = fs::rename(&temp, &final_path) {
            if had_existing {
                let _ = fs::rename(&backup, &final_path);
            }
            let _ = fs::remove_file(&temp);
            return Err(error.to_string());
        }

        if had_existing {
            let _ = fs::remove_file(&backup);
        }
        Ok(())
    }

    pub fn record_benchmark(
        &self,
        id: &str,
        benchmark: &LocalBenchmark,
    ) -> Result<PathBuf, String> {
        validate_pack_component(id)?;
        let manifest = self.load_manifest(id)?;
        self.verify(&manifest)?;
        validate_benchmark(benchmark)?;

        let dir = self.root.join(id);
        let path = dir.join(MODEL_PACK_BENCHMARK_FILE);
        let temp = dir.join(".benchmark.json.tmp");
        fs::write(
            &temp,
            serde_json::to_vec_pretty(benchmark).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        fs::rename(temp, &path).map_err(|error| error.to_string())?;
        Ok(path)
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_manifest(manifest: &ModelPackManifest) -> Result<(), String> {
    validate_pack_component(&manifest.id)?;
    validate_pack_component(&manifest.file)?;
    validate_pack_version(&manifest.version)?;
    validate_sha256(&manifest.sha256)?;

    if manifest.license.trim().is_empty() {
        return Err("model pack sem metadados de licença".to_string());
    }
    if manifest.capabilities.is_empty() {
        return Err("model pack sem capabilities declaradas".to_string());
    }
    for capability in &manifest.capabilities {
        let capability = capability.trim();
        if capability.is_empty()
            || !capability
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            return Err(format!("capability inválida no model pack: {capability:?}"));
        }
    }
    Ok(())
}

fn validate_pack_version(version: &str) -> Result<(), String> {
    let version = version.trim();
    if version.is_empty()
        || version.len() > 64
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return Err("versão inválida no model pack".to_string());
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.len() != MODEL_PACK_HASH_HEX_LEN || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("sha256 inválido no model pack".to_string());
    }
    Ok(())
}

fn validate_benchmark(benchmark: &LocalBenchmark) -> Result<(), String> {
    if benchmark.backend.trim().is_empty() {
        return Err("benchmark sem backend".to_string());
    }
    if benchmark.samples == 0 {
        return Err("benchmark sem amostras".to_string());
    }
    if benchmark.embedding_dimension == 0 {
        return Err("benchmark sem dimensão de embedding".to_string());
    }
    Ok(())
}

fn reject_symlink(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{label} não pode ser symlink: {}", path.display()));
    }
    Ok(())
}

fn validate_pack_component(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || value.contains("..")
        || value.contains('/')
        || value.contains('\\')
        // Um componente de model pack é também um nome de ficheiro Windows.
        // ':' fecha tanto caminhos drive-relative (C:foo), que Path::is_absolute
        // não apanha, como alternate data streams (model.bin:stream).
        || value.contains(':')
    {
        return Err("caminho inválido no model pack".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neuralia-local-ai-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn embeddings_are_local_deterministic_and_normalized() {
        let a = hashed_embedding("otimização AVX-512 no Rust");
        let b = hashed_embedding("otimização AVX-512 no Rust");
        assert_eq!(a, b);
        assert_eq!(a.len(), EMBEDDING_DIM);

        let norm = a.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    #[test]
    fn technical_portuguese_aliases_share_embedding_features() {
        let query = hashed_embedding("otimização vetorial CPU");
        let document = hashed_embedding("AVX-512 acelera operações SIMD em CPUs modernas");
        let unrelated = hashed_embedding("receita de bolo com chocolate");

        assert!(cosine_similarity(&query, &document) > cosine_similarity(&query, &unrelated));
    }

    #[test]
    fn related_text_scores_above_unrelated_text() {
        let query = hashed_embedding("WebView2 accessibility tree");
        let related = hashed_embedding("Accessibility Tree para automação no WebView2");
        let unrelated = hashed_embedding("receita de pão de queijo");

        assert!(cosine_similarity(&query, &related) > cosine_similarity(&query, &unrelated));
    }

    #[test]
    fn classifier_and_entities_need_no_model_pack() {
        let ai = HashingLocalIntelligence;
        assert_eq!(
            ai.classify("onde eu li sobre AVX-512?").unwrap(),
            IntentClass::MemoryRecall
        );

        let entities = ai.entities("NeuralIA usa WebView2 e AVX-512").unwrap();
        assert!(entities.iter().any(|item| item == "NeuralIA"));
        assert!(entities.iter().any(|item| item == "WebView2"));
    }

    #[test]
    fn summary_never_exceeds_character_budget() {
        let ai = HashingLocalIntelligence;
        let input = "Um. Dois. Três. Quatro com mais texto.";

        for budget in 0..=input.chars().count() {
            let summary = ai.summarize(input, budget).unwrap();
            assert!(
                summary.chars().count() <= budget,
                "budget={budget}, summary={summary:?}, chars={}",
                summary.chars().count()
            );
        }

        assert_eq!(ai.summarize(input, 1).unwrap(), "…");
        assert_eq!(ai.summarize(input, 3).unwrap(), "Um…");
    }

    #[test]
    fn model_pack_components_reject_windows_drive_and_stream_syntax() {
        for value in ["C:foo", "c:model.bin", "model.bin:stream"] {
            assert!(
                validate_pack_component(value).is_err(),
                "{value:?} must not escape Windows model-pack roots"
            );
        }
        assert!(validate_pack_component("model.bin").is_ok());
        assert!(validate_pack_component("semantic-small").is_ok());
    }

    #[test]
    fn benchmark_is_local_and_records_dimension() {
        let ai = HashingLocalIntelligence;
        let corpus = vec![
            "NeuralIA memória semântica".to_string(),
            "WebView2 accessibility tree".to_string(),
        ];
        let result = benchmark_local_intelligence("hashing-local", &ai, &corpus).unwrap();
        assert_eq!(result.samples, 2);
        assert_eq!(result.embedding_dimension, EMBEDDING_DIM);
    }

    #[test]
    fn model_pack_manifest_id_must_match_directory() {
        let root = temp_root("manifest-id");
        let pack = root.join("expected");
        fs::create_dir_all(&pack).unwrap();

        let manifest = ModelPackManifest {
            id: "other".into(),
            version: "1".into(),
            file: "model.bin".into(),
            sha256: format!("{:x}", Sha256::digest(b"model")),
            capabilities: vec!["embedding".into()],
            license: "test".into(),
        };
        fs::write(
            pack.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let manager = ModelPackManager::new(&root);
        let error = manager.load_manifest("expected").unwrap_err();
        assert!(error.contains("não corresponde"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn model_pack_install_is_staged_and_replaces_complete_pack() {
        let root = temp_root("staged-install");
        let manager = ModelPackManager::new(&root);

        let first = b"model-v1";
        let first_manifest = ModelPackManifest {
            id: "semantic-small".into(),
            version: "1".into(),
            file: "model.bin".into(),
            sha256: format!("{:x}", Sha256::digest(first)),
            capabilities: vec!["embedding".into()],
            license: "test".into(),
        };
        let model = manager.install(&first_manifest, first).unwrap();
        assert_eq!(fs::read(&model).unwrap(), first);
        assert_eq!(
            manager.load_manifest("semantic-small").unwrap().version,
            "1"
        );

        let second = b"model-v2";
        let second_manifest = ModelPackManifest {
            version: "2".into(),
            sha256: format!("{:x}", Sha256::digest(second)),
            ..first_manifest.clone()
        };
        let model = manager.install(&second_manifest, second).unwrap();
        assert_eq!(fs::read(&model).unwrap(), second);
        assert_eq!(
            manager.load_manifest("semantic-small").unwrap().version,
            "2"
        );

        let hidden = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| name.starts_with(".semantic-small."))
            .collect::<Vec<_>>();
        assert!(hidden.is_empty(), "{hidden:?}");

        let installed = manager.installed().unwrap();
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id, "semantic-small");
        assert_eq!(installed[0].version, "2");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_model_pack_hash_never_publishes_pack_directory() {
        let root = temp_root("bad-install");
        let manager = ModelPackManager::new(&root);
        let manifest = ModelPackManifest {
            id: "semantic-small".into(),
            version: "1".into(),
            file: "model.bin".into(),
            sha256: format!("{:x}", Sha256::digest(b"expected")),
            capabilities: vec!["embedding".into()],
            license: "test".into(),
        };

        assert!(manager.install(&manifest, b"tampered").is_err());
        assert!(!root.join("semantic-small").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn model_pack_hash_is_verified_before_use() {
        let root = temp_root("pack");
        let pack = root.join("semantic-small");
        fs::create_dir_all(&pack).unwrap();

        let bytes = b"fake-model";
        fs::write(pack.join("model.bin"), bytes).unwrap();
        let manifest = ModelPackManifest {
            id: "semantic-small".into(),
            version: "1".into(),
            file: "model.bin".into(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            capabilities: vec!["embedding".into()],
            license: "test".into(),
        };
        fs::write(
            pack.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let manager = ModelPackManager::new(&root);
        let loaded = manager.load_manifest("semantic-small").unwrap();
        assert_eq!(manager.verify(&loaded).unwrap(), pack.join("model.bin"));

        let benchmark = benchmark_local_intelligence(
            "hashing-local",
            &HashingLocalIntelligence,
            &["teste".to_string()],
        )
        .unwrap();
        assert!(
            manager
                .record_benchmark("semantic-small", &benchmark)
                .unwrap()
                .exists()
        );
        assert!(manager.uninstall("semantic-small").unwrap());
        assert!(!pack.exists());

        let _ = fs::remove_dir_all(root);
    }

    fn valid_manifest(id: &str, version: &str, bytes: &[u8]) -> ModelPackManifest {
        ModelPackManifest {
            id: id.into(),
            version: version.into(),
            file: "model.bin".into(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            capabilities: vec!["embedding".into()],
            license: "MIT".into(),
        }
    }

    #[test]
    fn model_pack_manifest_requires_license_capability_version_and_full_sha256() {
        let bytes = b"model";
        let mut manifest = valid_manifest("semantic-small", "1.0.0", bytes);
        assert!(validate_manifest(&manifest).is_ok());

        manifest.license.clear();
        assert!(validate_manifest(&manifest).unwrap_err().contains("licença"));

        manifest = valid_manifest("semantic-small", "1.0.0", bytes);
        manifest.capabilities.clear();
        assert!(validate_manifest(&manifest).unwrap_err().contains("capabilities"));

        manifest = valid_manifest("semantic-small", "", bytes);
        assert!(validate_manifest(&manifest).unwrap_err().contains("versão"));

        manifest = valid_manifest("semantic-small", "1.0.0", bytes);
        manifest.sha256 = "abcd".into();
        assert!(validate_manifest(&manifest).unwrap_err().contains("sha256"));
    }

    #[test]
    fn model_pack_activation_requires_verified_pack_and_recorded_benchmark() {
        let root = temp_root("activation-requires-benchmark");
        let manager = ModelPackManager::new(&root);
        let bytes = b"model-v1";
        let manifest = valid_manifest("semantic-small", "1.0.0", bytes);

        manager.install(&manifest, bytes).unwrap();
        assert!(
            manager.activate("semantic-small").unwrap_err().contains("benchmark"),
            "activation must fail before benchmark evidence exists"
        );

        let benchmark = benchmark_local_intelligence(
            "semantic-small",
            &HashingLocalIntelligence,
            &["NeuralIA".to_string()],
        )
        .unwrap();
        manager
            .record_benchmark("semantic-small", &benchmark)
            .unwrap();

        let activation = manager.activate("semantic-small").unwrap();
        assert_eq!(activation.id, "semantic-small");
        let selection = manager.selection();
        assert!(selection.warning.is_none());
        let active = selection.active.expect("pack must be active after validation");
        assert_eq!(active.manifest.version, "1.0.0");
        assert_eq!(active.benchmark.samples, 1);
        assert_eq!(fs::read(active.model_path).unwrap(), bytes);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_active_pack_degrades_to_deterministic_fallback_with_warning() {
        let root = temp_root("activation-corrupt-fallback");
        let manager = ModelPackManager::new(&root);
        let bytes = b"model-v1";
        let manifest = valid_manifest("semantic-small", "1.0.0", bytes);
        let model = manager.install(&manifest, bytes).unwrap();

        let benchmark = benchmark_local_intelligence(
            "semantic-small",
            &HashingLocalIntelligence,
            &["NeuralIA".to_string()],
        )
        .unwrap();
        manager
            .record_benchmark("semantic-small", &benchmark)
            .unwrap();
        manager.activate("semantic-small").unwrap();

        fs::write(model, b"tampered").unwrap();
        let selection = manager.selection();
        assert!(selection.active.is_none());
        assert!(
            selection
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("hash"))
        );

        // O fallback continua funcional sem qualquer pack.
        let fallback = HashingLocalIntelligence;
        assert_eq!(
            fallback.classify("onde eu li isso?").unwrap(),
            IntentClass::MemoryRecall
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn uninstalling_active_pack_deactivates_before_removing_files() {
        let root = temp_root("active-uninstall");
        let manager = ModelPackManager::new(&root);
        let bytes = b"model-v1";
        let manifest = valid_manifest("semantic-small", "1.0.0", bytes);
        manager.install(&manifest, bytes).unwrap();

        let benchmark = benchmark_local_intelligence(
            "semantic-small",
            &HashingLocalIntelligence,
            &["NeuralIA".to_string()],
        )
        .unwrap();
        manager
            .record_benchmark("semantic-small", &benchmark)
            .unwrap();
        manager.activate("semantic-small").unwrap();

        assert!(manager.uninstall("semantic-small").unwrap());
        assert!(!root.join("semantic-small").exists());
        assert!(!root.join(MODEL_PACK_ACTIVE_FILE).exists());
        assert_eq!(
            manager.selection(),
            ModelPackSelection::deterministic_fallback()
        );

        let _ = fs::remove_dir_all(root);
    }

}
