use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const EMBEDDING_DIM: usize = 384;

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

        let mut output = String::new();
        for sentence in clean.split_inclusive(['.', '!', '?']) {
            if !output.is_empty() {
                output.push(' ');
            }
            if output.chars().count() + sentence.chars().count() > budget {
                break;
            }
            output.push_str(sentence.trim());
        }
        if output.is_empty() {
            output = clean.chars().take(budget.saturating_sub(1)).collect();
        }
        if output.chars().count() < clean.chars().count() {
            output.push('…');
        }
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

fn canonical_semantic_token(word: &str) -> &str {
    match word {
        "simd" | "vetor" | "vetores" | "vetorial" | "vector" | "vectors" | "vectorial" => {
            "simd-vector"
        }
        "cpu" | "cpus" | "processador" | "processadores" | "processor" | "processors" => {
            "cpu-processor"
        }
        "otimização" | "otimizacao" | "otimizar" | "optimization" | "optimize" | "optimise" => {
            "optimization"
        }
        "memória" | "memoria" | "memory" => "memory",
        "navegador" | "navegadores" | "browser" | "browsers" => "browser",
        "pesquisa" | "pesquisar" | "research" => "research",
        "agente" | "agentes" | "agent" | "agents" => "agent",
        "segurança" | "seguranca" | "security" => "security",
        "fonte" | "fontes" | "source" | "sources" => "source",
        "resposta" | "respostas" | "answer" | "answers" => "answer",
        "código" | "codigo" | "code" => "code",
        _ => word,
    }
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
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }

    pub fn verify(&self, manifest: &ModelPackManifest) -> Result<PathBuf, String> {
        validate_pack_component(&manifest.id)?;
        validate_pack_component(&manifest.file)?;

        let path = self.root.join(&manifest.id).join(&manifest.file);
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
        validate_pack_component(&manifest.id)?;
        validate_pack_component(&manifest.file)?;
        let actual = format!("{:x}", Sha256::digest(model_bytes));
        if !actual.eq_ignore_ascii_case(manifest.sha256.trim()) {
            return Err(format!("hash do model pack {} não confere", manifest.id));
        }

        let pack = self.root.join(&manifest.id);
        fs::create_dir_all(&pack).map_err(|error| error.to_string())?;
        let model = pack.join(&manifest.file);
        let temp = pack.join(format!(".{}.tmp", manifest.file));
        fs::write(&temp, model_bytes).map_err(|error| error.to_string())?;
        fs::rename(&temp, &model).map_err(|error| error.to_string())?;
        fs::write(
            pack.join("manifest.json"),
            serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok(model)
    }

    pub fn uninstall(&self, id: &str) -> Result<bool, String> {
        validate_pack_component(id)?;
        let path = self.root.join(id);
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(path).map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub fn record_benchmark(
        &self,
        id: &str,
        benchmark: &LocalBenchmark,
    ) -> Result<PathBuf, String> {
        validate_pack_component(id)?;
        let dir = self.root.join(id);
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let path = dir.join("benchmark.json");
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

fn validate_pack_component(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || value.contains("..")
        || value.contains('/')
        || value.contains('\\')
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
}
