use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::local_intelligence::{HashingLocalIntelligence, LocalIntelligence};

static RESEARCH_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResearchItemKind {
    Question,
    ProviderAnswer,
    Source,
    Reader,
    Pdf,
    Note,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchItem {
    pub id: String,
    pub kind: ResearchItemKind,
    pub title: String,
    pub provider: Option<String>,
    pub url: Option<String>,
    pub memory_id: Option<String>,
    pub text: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisSnapshot {
    pub id: String,
    pub created_at: u64,
    pub item_ids: Vec<String>,
    pub generator: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonFact {
    pub item_id: String,
    pub source: String,
    pub entities: Vec<String>,
    pub numbers: Vec<String>,
    pub dates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchSession {
    pub id: String,
    pub title: String,
    pub question: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub items: Vec<ResearchItem>,
    #[serde(default)]
    pub syntheses: Vec<SynthesisSnapshot>,
}

impl ResearchSession {
    pub fn new(question: impl Into<String>) -> Self {
        let question = question.into();
        let now = unix_seconds();
        let id = unique_id(&[&question], 24);
        let title = short_title(&question, 72);

        let mut session = Self {
            id,
            title,
            question: question.clone(),
            created_at: now,
            updated_at: now,
            items: Vec::new(),
            syntheses: Vec::new(),
        };
        session.push_item(
            ResearchItemKind::Question,
            "Pergunta inicial",
            None,
            None,
            None,
            question,
        );
        session
    }

    pub fn add_provider_answer(
        &mut self,
        provider: impl Into<String>,
        text: impl Into<String>,
        memory_id: Option<String>,
    ) -> String {
        let provider = provider.into();
        self.push_item(
            ResearchItemKind::ProviderAnswer,
            format!("Resposta · {provider}"),
            Some(provider),
            None,
            memory_id,
            text.into(),
        )
    }

    pub fn upsert_provider_answer(
        &mut self,
        provider: impl Into<String>,
        text: impl Into<String>,
        memory_id: Option<String>,
    ) -> String {
        let provider = provider.into();
        let text = text.into();
        if let Some(item) = self.items.iter_mut().rev().find(|item| {
            item.kind == ResearchItemKind::ProviderAnswer
                && item.provider.as_deref() == Some(provider.as_str())
        }) {
            let updated_at = unix_seconds();
            item.text = text;
            item.memory_id = memory_id;
            self.updated_at = updated_at;
            return item.id.clone();
        }
        self.add_provider_answer(provider, text, memory_id)
    }

    pub fn add_source(
        &mut self,
        provider: Option<String>,
        title: impl Into<String>,
        url: impl Into<String>,
        memory_id: Option<String>,
        text: impl Into<String>,
    ) -> String {
        self.push_item(
            ResearchItemKind::Source,
            title.into(),
            provider,
            Some(url.into()),
            memory_id,
            text.into(),
        )
    }

    pub fn add_note(&mut self, title: impl Into<String>, text: impl Into<String>) -> String {
        self.push_item(
            ResearchItemKind::Note,
            title.into(),
            None,
            None,
            None,
            text.into(),
        )
    }

    fn push_item(
        &mut self,
        kind: ResearchItemKind,
        title: impl Into<String>,
        provider: Option<String>,
        url: Option<String>,
        memory_id: Option<String>,
        text: String,
    ) -> String {
        let title = title.into();
        let created_at = unix_seconds();
        let kind_name = format!("{kind:?}");
        let id = unique_id(
            &[
                &self.id,
                &kind_name,
                &title,
                provider.as_deref().unwrap_or_default(),
            ],
            20,
        );

        self.items.push(ResearchItem {
            id: id.clone(),
            kind,
            title,
            provider,
            url,
            memory_id,
            text,
            created_at,
        });
        self.updated_at = created_at;
        id
    }

    pub fn comparison(&self, item_ids: &[String]) -> Vec<ComparisonFact> {
        let selected_ids = selected_id_set(item_ids);
        let ai = HashingLocalIntelligence;
        self.items
            .iter()
            .filter(|item| {
                selected_ids
                    .as_ref()
                    .is_none_or(|ids| ids.contains(item.id.as_str()))
            })
            .map(|item| ComparisonFact {
                item_id: item.id.clone(),
                source: item
                    .provider
                    .clone()
                    .or_else(|| item.url.clone())
                    .unwrap_or_else(|| item.title.clone()),
                entities: ai.entities(&item.text).unwrap_or_default(),
                numbers: extract_numberish(&item.text),
                dates: extract_dates(&item.text),
            })
            .collect()
    }

    pub fn synthesize(&mut self, item_ids: &[String]) -> SynthesisSnapshot {
        let selected_ids = selected_id_set(item_ids);
        let selected = self
            .items
            .iter()
            .filter(|item| {
                selected_ids
                    .as_ref()
                    .is_none_or(|ids| ids.contains(item.id.as_str()))
            })
            .collect::<Vec<_>>();

        let mut output = format!("# {}\n\n", self.title);
        output.push_str("## Fontes selecionadas\n\n");
        for item in &selected {
            let provenance = item
                .provider
                .as_deref()
                .or(item.url.as_deref())
                .unwrap_or("NeuralIA");
            output.push_str(&format!("- **{}** ({provenance})\n", item.title));
        }

        output.push_str("\n## Síntese\n\n");
        let ai = HashingLocalIntelligence;
        for item in &selected {
            let summary = ai.summarize(&item.text, 360).unwrap_or_default();
            if !summary.is_empty() {
                output.push_str(&format!("- {}: {}\n", item.title, summary));
            }
        }

        let now = unix_seconds();
        let selected_material = selected
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let id = unique_id(&[&self.id, &selected_material], 20);
        let snapshot = SynthesisSnapshot {
            id,
            created_at: now,
            item_ids: selected.iter().map(|item| item.id.clone()).collect(),
            generator: "neuralia-deterministic-local".into(),
            output,
        };
        self.syntheses.push(snapshot.clone());
        self.updated_at = now;
        snapshot
    }

    pub fn export_markdown(&self) -> String {
        let mut output = format!(
            "# {}\n\n**Pergunta:** {}\n\n## Pesquisa\n\n",
            self.title, self.question
        );

        for item in &self.items {
            let provider = item.provider.as_deref().unwrap_or("local");
            let url = item.url.as_deref().unwrap_or("");
            output.push_str(&format!(
                "### {}\n\nProvedor: {}\n\nURL: {}\n\n{}\n\n",
                item.title, provider, url, item.text
            ));
        }

        if !self.syntheses.is_empty() {
            output.push_str("## Sínteses\n\n");
            for synthesis in &self.syntheses {
                output.push_str(&synthesis.output);
                output.push_str("\n\n");
            }
        }
        output
    }

    pub fn save(&self, root: impl AsRef<Path>) -> io::Result<PathBuf> {
        let root = root.as_ref().join("sessions");
        fs::create_dir_all(&root)?;
        let path = root.join(format!("{}.json", self.id));
        atomic_write(
            &path,
            &serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )?;
        Ok(path)
    }

    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
    }
}

fn selected_id_set(item_ids: &[String]) -> Option<HashSet<&str>> {
    (!item_ids.is_empty()).then(|| item_ids.iter().map(String::as_str).collect())
}

fn unique_id(parts: &[&str], hex_len: usize) -> String {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let nonce = RESEARCH_NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();

    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    digest.update(now_nanos.to_le_bytes());
    digest.update(pid.to_le_bytes());
    digest.update(nonce.to_le_bytes());

    let hex = format!("{:x}", digest.finalize());
    hex[..hex_len.min(hex.len())].to_string()
}

fn extract_numberish(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                !ch.is_alphanumeric() && !matches!(ch, '.' | ',' | '%' | '-')
            })
        })
        .filter(|token| token.chars().any(|ch| ch.is_ascii_digit()))
        .take(32)
        .map(str::to_string)
        .collect()
}

fn extract_dates(input: &str) -> Vec<String> {
    extract_numberish(input)
        .into_iter()
        .filter(|token| {
            let separators = token.matches(['/', '-']).count();
            separators == 2 || (token.len() == 4 && token.chars().all(|ch| ch.is_ascii_digit()))
        })
        .collect()
}

fn short_title(input: &str, max: usize) -> String {
    let clean = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max {
        clean
    } else {
        format!(
            "{}…",
            clean
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    fs::create_dir_all(parent)?;
    let temp_nonce = RESEARCH_NONCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session"),
        std::process::id(),
        temp_nonce
    ));
    fs::write(&temp, bytes)?;

    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_sessions_with_same_question_have_unique_ids() {
        let ids = (0..2_048)
            .map(|_| ResearchSession::new("mesma pergunta").id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 2_048);
    }

    #[test]
    fn repeated_items_and_syntheses_have_unique_ids() {
        let mut session = ResearchSession::new("unicidade");
        let item_ids = (0..1_024)
            .map(|_| session.add_note("Mesmo título", "Mesmo texto"))
            .collect::<HashSet<_>>();
        assert_eq!(item_ids.len(), 1_024);

        let selected = session.items[1].id.clone();
        let synthesis_ids = (0..512)
            .map(|_| session.synthesize(std::slice::from_ref(&selected)).id)
            .collect::<HashSet<_>>();
        assert_eq!(synthesis_ids.len(), 512);
    }

    #[test]
    fn session_preserves_provider_source_provenance() {
        let mut session = ResearchSession::new("DOM ou Accessibility Tree?");
        let source = session.add_source(
            Some("Claude".into()),
            "WebView2 docs",
            "https://example.com/webview2",
            Some("memory-1".into()),
            "Accessibility Tree complementa o DOM.",
        );

        let fact = session.comparison(&[source]).remove(0);
        assert_eq!(fact.source, "Claude");
        assert!(fact.entities.iter().any(|entity| entity == "Accessibility"));
    }

    #[test]
    fn provider_upsert_preserves_original_created_at() {
        let mut session = ResearchSession::new("preservar criação");
        session.upsert_provider_answer("Claude", "primeira", None);

        let answer = session
            .items
            .iter_mut()
            .find(|item| item.kind == ResearchItemKind::ProviderAnswer)
            .expect("provider answer");
        answer.created_at = 42;

        let id_before = answer.id.clone();
        session.upsert_provider_answer("Claude", "segunda", Some("memory-2".into()));

        let answer = session
            .items
            .iter()
            .find(|item| item.kind == ResearchItemKind::ProviderAnswer)
            .expect("provider answer");
        assert_eq!(answer.id, id_before);
        assert_eq!(answer.created_at, 42);
        assert_eq!(answer.text, "segunda");
        assert_eq!(answer.memory_id.as_deref(), Some("memory-2"));
    }

    #[test]
    fn comparison_accepts_five_sources_and_provider_answer_is_upserted() {
        let mut session = ResearchSession::new("comparar cinco fontes");
        for index in 0..5 {
            session.add_source(
                Some("Claude".into()),
                format!("Fonte {index}"),
                format!("https://example.com/{index}"),
                None,
                format!("Entidade{index} valor {}", index + 10),
            );
        }
        let ids = session
            .items
            .iter()
            .filter(|item| item.kind == ResearchItemKind::Source)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 5);
        assert_eq!(session.comparison(&ids).len(), 5);

        session.upsert_provider_answer("Claude", "primeira versão", None);
        session.upsert_provider_answer("Claude", "versão final", None);
        let answers = session
            .items
            .iter()
            .filter(|item| item.kind == ResearchItemKind::ProviderAnswer)
            .collect::<Vec<_>>();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].text, "versão final");
    }

    #[test]
    fn markdown_export_contains_sources_and_synthesis() {
        let mut session = ResearchSession::new("exportar");
        let source = session.add_source(
            Some("Gemini".into()),
            "Fonte",
            "https://example.com",
            None,
            "conteúdo",
        );
        session.synthesize(&[source]);
        let markdown = session.export_markdown();
        assert!(markdown.contains("https://example.com"));
        assert!(markdown.contains("Gemini"));
        assert!(markdown.contains("## Sínteses"));
    }

    #[test]
    fn synthesis_records_exact_source_set() {
        let mut session = ResearchSession::new("comparar");
        let a = session.add_note("A", "latência 10 ms");
        let b = session.add_note("B", "latência 20 ms");
        let snapshot = session.synthesize(&[a.clone(), b.clone()]);

        assert_eq!(snapshot.item_ids, vec![a, b]);
        assert!(snapshot.output.contains("latência"));
    }

    #[test]
    fn export_is_self_contained_markdown() {
        let mut session = ResearchSession::new("pesquisa");
        session.add_source(
            Some("Gemini".into()),
            "Fonte",
            "https://example.com",
            None,
            "conteúdo",
        );

        let markdown = session.export_markdown();
        assert!(markdown.contains("https://example.com"));
        assert!(markdown.contains("Gemini"));
    }
}
