use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

mod sqlite_v01;

const MEMORY_SCHEMA_VERSION: u32 = 1;

use crate::{
    agent_security::redact_sensitive_text,
    local_intelligence::{EMBEDDING_DIM, cosine_similarity, extract_entities, hashed_embedding},
    research::ResearchSession,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryKind {
    Concept,
    Decision,
    Source,
    Comparison,
    Procedure,
    Note,
    SessionSummary,
    Gotcha,
    ResearchResult,
}

impl MemoryKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Concept => "concepts",
            Self::Decision => "decisions",
            Self::Source => "sources",
            Self::Comparison => "comparisons",
            Self::Procedure => "procedures",
            Self::Note => "notes",
            Self::SessionSummary => "sessions",
            Self::Gotcha => "gotchas",
            Self::ResearchResult => "research-results",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySourceKind {
    Reader,
    Web,
    Pdf,
    ProviderAnswer,
    Note,
    Synthesis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRelation {
    pub kind: String,
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDocument {
    pub id: String,
    pub kind: MemoryKind,
    pub source_kind: MemorySourceKind,
    pub title: String,
    pub url: Option<String>,
    pub body: String,
    pub provider: Option<String>,
    pub session_id: Option<String>,
    pub created_at: u64,
    pub last_seen_at: u64,
    pub content_hash: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub relations: Vec<MemoryRelation>,
    #[serde(default)]
    pub embedding: Vec<f32>,
    #[serde(default)]
    pub private: bool,
}

impl MemoryDocument {
    pub fn new(
        kind: MemoryKind,
        source_kind: MemorySourceKind,
        title: impl Into<String>,
        url: Option<String>,
        body: impl Into<String>,
    ) -> Self {
        let title = title.into();
        let body = redact_sensitive_text(&body.into());
        let now = unix_seconds();
        let content_hash = sha256_hex(body.as_bytes());
        let id_material = format!(
            "{:?}\n{:?}\n{}\n{}\n{}",
            kind,
            source_kind,
            title,
            url.as_deref().unwrap_or_default(),
            content_hash
        );
        let id = sha256_hex(id_material.as_bytes())[..24].to_string();
        let entity_text = format!("{title}\n{body}");

        Self {
            id,
            kind,
            source_kind,
            title,
            url,
            body,
            provider: None,
            session_id: None,
            created_at: now,
            last_seen_at: now,
            content_hash,
            entities: extract_entities(&entity_text),
            relations: Vec::new(),
            embedding: hashed_embedding(&entity_text),
            private: false,
        }
    }

    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn relation(mut self, kind: impl Into<String>, target_id: impl Into<String>) -> Self {
        self.relations.push(MemoryRelation {
            kind: kind.into(),
            target_id: target_id.into(),
        });
        self
    }

    pub fn private(mut self, value: bool) -> Self {
        self.private = value;
        self
    }
}

#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub text: String,
    pub limit: usize,
    pub provider: Option<String>,
    pub session_id: Option<String>,
}

impl MemoryQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            limit: 12,
            provider: None,
            session_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHit {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub provider: Option<String>,
    pub session_id: Option<String>,
    pub excerpt: String,
    pub score: f32,
    pub matched_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    Stored(String),
    SkippedPrivate,
    SkippedForgotten,
}

#[derive(Debug, Clone)]
pub enum ForgetScope {
    All,
    Document(String),
    Session(String),
    Domain(String),
    Before(u64),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForgetReport {
    pub documents: usize,
    pub files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct MemoryTombstone {
    object_type: String,
    object_id: String,
    scope: Option<String>,
    reason: Option<String>,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDoctorReport {
    pub schema: u32,
    pub documents: usize,
    pub corrupt_documents: usize,
    pub manifest_present: bool,
    pub sqlite_present: bool,
    pub rebuilt: bool,
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    root: PathBuf,
}

impl MemoryStore {
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let store = Self { root: root.into() };
        store.ensure_layout()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn capture(&self, mut document: MemoryDocument) -> io::Result<CaptureOutcome> {
        if document.private {
            return Ok(CaptureOutcome::SkippedPrivate);
        }
        self.ensure_layout()?;
        let tombstones = self.load_tombstones()?;
        if tombstone_blocks_document(&document, &tombstones) {
            return Ok(CaptureOutcome::SkippedForgotten);
        }
        let sqlite_existed = self.sqlite_path().exists();

        document.body = redact_sensitive_text(&document.body);
        document.content_hash = sha256_hex(document.body.as_bytes());
        document.last_seen_at = unix_seconds();
        if document.embedding.len() != EMBEDDING_DIM {
            document.embedding =
                hashed_embedding(&format!("{}\n{}", document.title, document.body));
        }
        if document.entities.is_empty() {
            document.entities = extract_entities(&format!("{}\n{}", document.title, document.body));
        }

        let json_path = self.document_path(&document.id);
        atomic_write(
            &json_path,
            &serde_json::to_vec_pretty(&document).map_err(io::Error::other)?,
        )?;

        let wiki_path = self.wiki_path(&document);
        if let Some(parent) = wiki_path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&wiki_path, self.markdown(&document).as_bytes())?;

        let session = self.session_for_document(&document);
        sqlite_v01::upsert(&self.sqlite_path(), &document, session.as_ref())?;
        if !sqlite_existed && !tombstones.is_empty() {
            sqlite_v01::sync_tombstones(&self.sqlite_path(), &tombstones)?;
        }
        self.write_index_manifest()?;
        Ok(CaptureOutcome::Stored(document.id))
    }

    pub fn get(&self, id: &str) -> io::Result<Option<MemoryDocument>> {
        let path = self.document_path(id);
        match fs::read(&path) {
            Ok(bytes) => {
                let document =
                    serde_json::from_slice::<MemoryDocument>(&bytes).map_err(io::Error::other)?;
                let tombstones = self.load_tombstones()?;
                if tombstone_blocks_document(&document, &tombstones) {
                    Ok(None)
                } else {
                    Ok(Some(document))
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn documents(&self) -> io::Result<Vec<MemoryDocument>> {
        let tombstones = self.load_tombstones()?;
        let mut documents = Vec::new();
        let Ok(entries) = fs::read_dir(self.documents_dir()) else {
            return Ok(documents);
        };

        for entry in entries.flatten() {
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = fs::read(entry.path())
                && let Ok(document) = serde_json::from_slice::<MemoryDocument>(&bytes)
                && !document.private
                && !tombstone_blocks_document(&document, &tombstones)
            {
                documents.push(document);
            }
        }

        documents.sort_by_key(|document| std::cmp::Reverse(document.last_seen_at));
        Ok(documents)
    }

    pub fn query(&self, query: &MemoryQuery) -> io::Result<Vec<MemoryHit>> {
        let query_text = query.text.trim();
        if query_text.is_empty() {
            return Ok(Vec::new());
        }

        let candidate_limit = query.limit.clamp(1, 100).saturating_mul(16).min(512);
        let candidate_ids = sqlite_v01::candidate_ids(
            &self.sqlite_path(),
            query_text,
            query.provider.as_deref(),
            query.session_id.as_deref(),
            candidate_limit,
        )
        .unwrap_or_default();

        let used_sqlite_candidates = !candidate_ids.is_empty();
        let docs = if candidate_ids.is_empty() {
            self.documents()?
                .into_iter()
                .filter(|doc| {
                    query
                        .provider
                        .as_ref()
                        .is_none_or(|provider| doc.provider.as_ref() == Some(provider))
                        && query
                            .session_id
                            .as_ref()
                            .is_none_or(|session| doc.session_id.as_ref() == Some(session))
                })
                .collect::<Vec<_>>()
        } else {
            candidate_ids
                .into_iter()
                .map(|id| self.get(&id))
                .collect::<io::Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .filter(|doc| !doc.private)
                .collect::<Vec<_>>()
        };
        if docs.is_empty() {
            return Ok(Vec::new());
        }

        let persisted_embeddings = if used_sqlite_candidates {
            let ids = docs.iter().map(|doc| doc.id.clone()).collect::<Vec<_>>();
            sqlite_v01::embeddings_for_ids(&self.sqlite_path(), &ids).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let terms = tokenize(query_text);
        let query_entities = extract_entities(query_text)
            .into_iter()
            .map(|item| item.to_lowercase())
            .collect::<HashSet<_>>();
        let query_embedding = hashed_embedding(query_text);

        let lexical = docs
            .iter()
            .enumerate()
            .map(|(index, doc)| (index, lexical_score(doc, &terms)))
            .filter(|(_, score)| *score > 0.0)
            .collect::<Vec<_>>();
        let semantic = docs
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                let embedding = persisted_embeddings.get(&doc.id).unwrap_or(&doc.embedding);
                (
                    index,
                    cosine_similarity(&query_embedding, embedding).max(0.0),
                )
            })
            .filter(|(_, score)| *score > 0.05)
            .collect::<Vec<_>>();
        let entities = docs
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                let count = doc
                    .entities
                    .iter()
                    .filter(|entity| query_entities.contains(&entity.to_lowercase()))
                    .count();
                (index, count as f32)
            })
            .filter(|(_, score)| *score > 0.0)
            .collect::<Vec<_>>();

        let mut scores: HashMap<usize, (f32, BTreeSet<&'static str>)> = HashMap::new();
        add_rrf(&mut scores, lexical, "lexical");
        add_rrf(&mut scores, semantic, "semantic");
        add_rrf(&mut scores, entities, "entity");

        let seed_ids = scores
            .keys()
            .filter_map(|index| docs.get(*index).map(|doc| doc.id.as_str()))
            .collect::<HashSet<_>>();
        for (index, doc) in docs.iter().enumerate() {
            if doc
                .relations
                .iter()
                .any(|relation| seed_ids.contains(relation.target_id.as_str()))
            {
                let entry = scores.entry(index).or_default();
                entry.0 += 1.0 / 64.0;
                entry.1.insert("graph");
            }
        }

        let newest = docs.iter().map(|doc| doc.last_seen_at).max().unwrap_or(0);
        let mut ranked = scores
            .into_iter()
            .map(|(index, (mut score, matched))| {
                let doc = &docs[index];
                if newest.saturating_sub(doc.last_seen_at) < 7 * 24 * 60 * 60 {
                    score += 0.002;
                }
                (index, score, matched)
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| docs[right.0].last_seen_at.cmp(&docs[left.0].last_seen_at))
        });

        Ok(ranked
            .into_iter()
            .take(query.limit.clamp(1, 100))
            .map(|(index, score, matched)| {
                let doc = &docs[index];
                MemoryHit {
                    id: doc.id.clone(),
                    title: doc.title.clone(),
                    url: doc.url.clone(),
                    provider: doc.provider.clone(),
                    session_id: doc.session_id.clone(),
                    excerpt: excerpt(&doc.body, 240),
                    score,
                    matched_by: matched.into_iter().map(str::to_string).collect(),
                }
            })
            .collect())
    }

    pub fn doctor(&self, rebuild: bool) -> io::Result<MemoryDoctorReport> {
        self.ensure_layout()?;
        let mut documents = 0usize;
        let mut corrupt_documents = 0usize;

        if let Ok(entries) = fs::read_dir(self.documents_dir()) {
            for entry in entries.flatten() {
                if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                match fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<MemoryDocument>(&bytes).ok())
                {
                    Some(document) if !document.private => documents += 1,
                    Some(_) => {}
                    None => corrupt_documents += 1,
                }
            }
        }

        if rebuild {
            self.rebuild()?;
        }

        Ok(MemoryDoctorReport {
            schema: MEMORY_SCHEMA_VERSION,
            documents,
            corrupt_documents,
            manifest_present: self.root.join("db").join("index-manifest.json").exists(),
            sqlite_present: self.sqlite_path().exists(),
            rebuilt: rebuild,
        })
    }

    pub fn forget(&self, scope: ForgetScope) -> io::Result<ForgetReport> {
        self.ensure_layout()?;
        let documents = self.documents()?;
        let sessions = self.research_sessions_unfiltered()?;
        let mut report = ForgetReport::default();

        // Persist the deny policy before deleting source files. If the process
        // dies halfway through forget(), stale source cannot be re-imported.
        let mut tombstones = self.load_tombstones()?;
        extend_tombstones_for_scope(&mut tombstones, &scope, &sessions);
        self.save_tombstones(&tombstones)?;

        for document in documents {
            if !matches_scope(&document, &scope) {
                continue;
            }
            for path in [self.document_path(&document.id), self.wiki_path(&document)] {
                if fs::remove_file(&path).is_ok() {
                    report.files += 1;
                }
            }
            report.documents += 1;
        }

        match &scope {
            ForgetScope::Session(id) => {
                let path = self.root.join("sessions").join(format!("{id}.json"));
                if fs::remove_file(path).is_ok() {
                    report.files += 1;
                }
            }
            ForgetScope::All => {
                if let Ok(entries) = fs::read_dir(self.root.join("sessions")) {
                    for entry in entries.flatten() {
                        if entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                            && fs::remove_file(entry.path()).is_ok()
                        {
                            report.files += 1;
                        }
                    }
                }
                let _ = fs::remove_file(self.sqlite_path());
            }
            _ => {}
        }

        self.rebuild()?;
        Ok(report)
    }

    pub fn rebuild(&self) -> io::Result<()> {
        self.ensure_layout()?;
        let tombstones = self.load_tombstones()?;
        let docs = self.documents()?;
        let sessions = self.research_sessions()?;
        sqlite_v01::rebuild(&self.sqlite_path(), &docs, &sessions)?;
        sqlite_v01::sync_tombstones(&self.sqlite_path(), &tombstones)?;
        self.write_index_manifest()
    }

    fn session_for_document(&self, document: &MemoryDocument) -> Option<ResearchSession> {
        let id = document.session_id.as_deref()?;
        let path = self.root.join("sessions").join(format!("{id}.json"));
        ResearchSession::load(path)
            .ok()
            .filter(|session| session.id == id)
    }

    fn research_sessions(&self) -> io::Result<Vec<ResearchSession>> {
        let tombstones = self.load_tombstones()?;
        Ok(self
            .research_sessions_unfiltered()?
            .into_iter()
            .filter(|session| !tombstone_blocks_session(session, &tombstones))
            .collect())
    }

    fn research_sessions_unfiltered(&self) -> io::Result<Vec<ResearchSession>> {
        let mut sessions = Vec::new();
        let Ok(entries) = fs::read_dir(self.root.join("sessions")) else {
            return Ok(sessions);
        };

        for entry in entries.flatten() {
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if let Ok(session) = ResearchSession::load(entry.path()) {
                sessions.push(session);
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    fn ensure_layout(&self) -> io::Result<()> {
        for path in [
            self.root.join("wiki"),
            self.root.join("evidence"),
            self.root.join("documents"),
            self.root.join("db"),
            self.root.join("models"),
            self.root.join("logs"),
            self.root.join("sessions"),
        ] {
            fs::create_dir_all(path)?;
        }
        Ok(())
    }

    fn documents_dir(&self) -> PathBuf {
        self.root.join("documents")
    }

    fn document_path(&self, id: &str) -> PathBuf {
        self.documents_dir().join(format!("{id}.json"))
    }

    fn wiki_path(&self, document: &MemoryDocument) -> PathBuf {
        self.root
            .join("wiki")
            .join(document.kind.directory())
            .join(format!("{}.md", document.id))
    }

    fn sqlite_path(&self) -> PathBuf {
        self.root.join("db").join("neural-memory.sqlite")
    }

    fn tombstones_path(&self) -> PathBuf {
        self.root.join("tombstones.json")
    }

    fn load_tombstones(&self) -> io::Result<Vec<MemoryTombstone>> {
        match fs::read(self.tombstones_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::other(format!(
                    "memory tombstone registry is corrupt; refusing to bypass forget policy: {error}"
                ))
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    fn save_tombstones(&self, tombstones: &[MemoryTombstone]) -> io::Result<()> {
        atomic_write(
            &self.tombstones_path(),
            &serde_json::to_vec_pretty(tombstones).map_err(io::Error::other)?,
        )
    }

    fn markdown(&self, document: &MemoryDocument) -> String {
        format!(
            "# {}\n\n- id: {}\n- kind: {:?}\n- source: {:?}\n- provider: {}\n- session: {}\n- url: {}\n- content-sha256: {}\n\n{}\n",
            document.title,
            document.id,
            document.kind,
            document.source_kind,
            document.provider.as_deref().unwrap_or(""),
            document.session_id.as_deref().unwrap_or(""),
            document.url.as_deref().unwrap_or(""),
            document.content_hash,
            document.body
        )
    }

    fn write_index_manifest(&self) -> io::Result<()> {
        #[derive(Serialize)]
        struct Manifest {
            schema: u32,
            generated_at: u64,
            documents: usize,
            sqlite: &'static str,
            retrieval: [&'static str; 4],
        }

        let manifest = Manifest {
            schema: MEMORY_SCHEMA_VERSION,
            generated_at: unix_seconds(),
            documents: self.documents()?.len(),
            sqlite: "rusqlite-v01-derived",
            retrieval: ["lexical", "entity", "graph", "semantic"],
        };
        atomic_write(
            &self.root.join("db").join("index-manifest.json"),
            &serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?,
        )
    }
}

fn lexical_score(document: &MemoryDocument, terms: &[String]) -> f32 {
    if terms.is_empty() {
        return 0.0;
    }

    let title = tokenize(&document.title);
    let body = tokenize(&document.body);
    let entities = document
        .entities
        .iter()
        .flat_map(|entity| tokenize(entity))
        .collect::<Vec<_>>();

    let mut score = 0.0f32;
    for term in terms {
        score += title.iter().filter(|token| *token == term).count() as f32 * 4.0;
        score += entities.iter().filter(|token| *token == term).count() as f32 * 2.0;
        score += body.iter().filter(|token| *token == term).count() as f32;
    }
    score / (1.0 + body.len() as f32 / 500.0)
}

fn add_rrf(
    scores: &mut HashMap<usize, (f32, BTreeSet<&'static str>)>,
    mut stream: Vec<(usize, f32)>,
    name: &'static str,
) {
    stream.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
    for (rank, (index, _)) in stream.into_iter().enumerate() {
        let entry = scores.entry(index).or_default();
        entry.0 += 1.0 / (61.0 + rank as f32);
        entry.1.insert(name);
    }
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .filter(|token| token.chars().count() >= 2)
        .map(str::to_string)
        .collect()
}

fn excerpt(body: &str, max_chars: usize) -> String {
    let clean = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max_chars {
        clean
    } else {
        format!(
            "{}…",
            clean
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn normalize_domain(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = Url::parse(trimmed)
        .or_else(|_| Url::parse(&format!("https://{}", trimmed.trim_start_matches('.'))))
        .ok()?;
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn document_domain(document: &MemoryDocument) -> Option<String> {
    document.url.as_deref().and_then(normalize_domain)
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host.eq_ignore_ascii_case(domain)
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn tombstone_blocks_document(document: &MemoryDocument, tombstones: &[MemoryTombstone]) -> bool {
    tombstones
        .iter()
        .any(|tombstone| match tombstone.object_type.as_str() {
            "document" => tombstone.object_id == document.id,
            "session" => document.session_id.as_deref() == Some(tombstone.object_id.as_str()),
            "domain" => document_domain(document)
                .is_some_and(|host| domain_matches(&host, &tombstone.object_id)),
            "before" => tombstone
                .object_id
                .parse::<u64>()
                .ok()
                .is_some_and(|timestamp| document.last_seen_at < timestamp),
            _ => false,
        })
}

fn tombstone_blocks_session(session: &ResearchSession, tombstones: &[MemoryTombstone]) -> bool {
    tombstones
        .iter()
        .any(|tombstone| tombstone.object_type == "session" && tombstone.object_id == session.id)
}

fn push_tombstone(
    tombstones: &mut Vec<MemoryTombstone>,
    object_type: &str,
    object_id: String,
    scope: &str,
) {
    if tombstones
        .iter()
        .any(|item| item.object_type == object_type && item.object_id == object_id)
    {
        return;
    }
    tombstones.push(MemoryTombstone {
        object_type: object_type.to_string(),
        object_id,
        scope: Some(scope.to_string()),
        reason: Some("user-forget".to_string()),
        created_at: unix_seconds(),
    });
    tombstones.sort_by(|left, right| {
        left.object_type
            .cmp(&right.object_type)
            .then_with(|| left.object_id.cmp(&right.object_id))
    });
}

fn extend_tombstones_for_scope(
    tombstones: &mut Vec<MemoryTombstone>,
    scope: &ForgetScope,
    sessions: &[ResearchSession],
) {
    match scope {
        ForgetScope::Document(id) => {
            push_tombstone(tombstones, "document", id.clone(), "document");
        }
        ForgetScope::Session(id) => {
            push_tombstone(tombstones, "session", id.clone(), "session");
        }
        ForgetScope::Domain(domain) => {
            if let Some(domain) = normalize_domain(domain) {
                push_tombstone(tombstones, "domain", domain, "domain");
            }
        }
        ForgetScope::Before(timestamp) => {
            push_tombstone(tombstones, "before", timestamp.to_string(), "before");
        }
        ForgetScope::All => {
            push_tombstone(
                tombstones,
                "before",
                unix_seconds().saturating_add(1).to_string(),
                "all",
            );
            for session in sessions {
                push_tombstone(tombstones, "session", session.id.clone(), "all");
            }
        }
    }
}

fn matches_scope(document: &MemoryDocument, scope: &ForgetScope) -> bool {
    match scope {
        ForgetScope::All => true,
        ForgetScope::Document(id) => &document.id == id,
        ForgetScope::Session(id) => document.session_id.as_ref() == Some(id),
        ForgetScope::Before(timestamp) => document.last_seen_at < *timestamp,
        ForgetScope::Domain(domain) => normalize_domain(domain)
            .and_then(|domain| document_domain(document).map(|host| (host, domain)))
            .is_some_and(|(host, domain)| domain_matches(&host, &domain)),
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("path without parent"));
    };
    fs::create_dir_all(parent)?;

    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("memory"),
        std::process::id()
    ));
    fs::write(&temp, bytes)?;

    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            if path.exists() {
                fs::remove_file(path)?;
                fs::rename(temp, path)
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neuralia-memory-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn query_uses_fts_candidates_without_losing_provider_filter() {
        let root = temp_root("fts-query-filter");
        let store = MemoryStore::new(&root).unwrap();

        store
            .capture(
                MemoryDocument::new(
                    MemoryKind::Source,
                    MemorySourceKind::ProviderAnswer,
                    "Rust A",
                    None,
                    "ownership borrowing lifetimes rust",
                )
                .provider("Claude"),
            )
            .unwrap();
        store
            .capture(
                MemoryDocument::new(
                    MemoryKind::Source,
                    MemorySourceKind::ProviderAnswer,
                    "Rust B",
                    None,
                    "ownership borrowing lifetimes rust",
                )
                .provider("Gemini"),
            )
            .unwrap();

        let mut query = MemoryQuery::new("ownership rust");
        query.provider = Some("Claude".into());
        let hits = store.query(&query).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].provider.as_deref(), Some("Claude"));
        assert!(hits[0].matched_by.iter().any(|source| source == "lexical"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_embedding_rerank_matches_document_embedding_results() {
        let root = temp_root("sqlite-rerank");
        let store = MemoryStore::new(&root).unwrap();
        let target = MemoryDocument::new(
            MemoryKind::Concept,
            MemorySourceKind::Note,
            "Borrow checker",
            None,
            "Rust ownership borrowing lifetimes compiler",
        );
        let target_id = target.id.clone();
        store.capture(target).unwrap();
        store
            .capture(MemoryDocument::new(
                MemoryKind::Concept,
                MemorySourceKind::Note,
                "Cooking",
                None,
                "recipe tomato basil pasta kitchen",
            ))
            .unwrap();

        let hits = store
            .query(&MemoryQuery::new("rust ownership compiler"))
            .unwrap();

        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, target_id);
        assert!(hits[0].matched_by.iter().any(|source| source == "semantic"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forgotten_domain_blocks_subdomain_recapture_and_survives_sqlite_loss() {
        let root = temp_root("domain-tombstone");
        let store = MemoryStore::new(&root).unwrap();
        let original = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Example",
            Some("https://news.Example.COM./article".into()),
            "first capture",
        );
        store.capture(original).unwrap();

        let report = store
            .forget(ForgetScope::Domain("EXAMPLE.com.".into()))
            .unwrap();
        assert_eq!(report.documents, 1);
        assert!(store.documents().unwrap().is_empty());

        let _ = fs::remove_file(store.sqlite_path());
        let recapture = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Example again",
            Some("https://deep.sub.example.com/other".into()),
            "future capture",
        );
        assert_eq!(
            store.capture(recapture).unwrap(),
            CaptureOutcome::SkippedForgotten
        );
        assert!(root.join("tombstones.json").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forgotten_session_removes_session_file_and_blocks_recapture() {
        let root = temp_root("session-tombstone");
        let store = MemoryStore::new(&root).unwrap();
        let mut session = ResearchSession::new("forget me");
        session.id = "session-forget".into();
        session
            .save(root.join("sessions").join("session-forget.json"))
            .unwrap();

        store
            .capture(
                MemoryDocument::new(
                    MemoryKind::Source,
                    MemorySourceKind::Reader,
                    "Session doc",
                    None,
                    "session body",
                )
                .session("session-forget"),
            )
            .unwrap();

        store
            .forget(ForgetScope::Session("session-forget".into()))
            .unwrap();
        assert!(!root.join("sessions").join("session-forget.json").exists());

        let recapture = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Reader,
            "Session doc 2",
            None,
            "new body",
        )
        .session("session-forget");
        assert_eq!(
            store.capture(recapture).unwrap(),
            CaptureOutcome::SkippedForgotten
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rebuild_does_not_resurrect_stale_tombstoned_source() {
        let root = temp_root("tombstone-rebuild");
        let store = MemoryStore::new(&root).unwrap();
        let doc = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Stale",
            Some("https://forgot.example/page".into()),
            "must stay forgotten",
        );
        let stale = serde_json::to_vec_pretty(&doc).unwrap();
        let stale_path = store.document_path(&doc.id);
        store.capture(doc).unwrap();
        store
            .forget(ForgetScope::Domain("forgot.example".into()))
            .unwrap();

        fs::write(&stale_path, stale).unwrap();
        store.rebuild().unwrap();

        assert!(store.documents().unwrap().is_empty());
        assert!(
            store
                .query(&MemoryQuery::new("must stay forgotten"))
                .unwrap()
                .is_empty()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_tombstone_registry_fails_closed() {
        let root = temp_root("tombstone-corrupt");
        let store = MemoryStore::new(&root).unwrap();
        fs::write(root.join("tombstones.json"), b"{not-json").unwrap();

        let result = store.capture(MemoryDocument::new(
            MemoryKind::Note,
            MemorySourceKind::Note,
            "Should fail",
            None,
            "body",
        ));
        assert!(result.is_err());
        assert!(store.documents().is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn conceptual_query_finds_document_without_exact_title() {
        let root = temp_root("semantic");
        let store = MemoryStore::new(&root).unwrap();
        let doc = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Reader,
            "Arquitetura de automação no WebView2",
            Some("https://example.com/webview-agent".into()),
            "A árvore de acessibilidade pode ser usada depois do DOM e antes de recorrer a visão por pixels.",
        );
        store.capture(doc).unwrap();

        let hits = store
            .query(&MemoryQuery::new("accessibility tree automação navegador"))
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].title.contains("WebView2"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn private_documents_never_touch_persistent_store() {
        let root = temp_root("private");
        let store = MemoryStore::new(&root).unwrap();
        let doc = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Privado",
            Some("https://example.com/private".into()),
            "segredo",
        )
        .private(true);

        assert_eq!(store.capture(doc).unwrap(), CaptureOutcome::SkippedPrivate);
        assert!(store.documents().unwrap().is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stored_secrets_are_redacted_before_disk() {
        let root = temp_root("redact");
        let store = MemoryStore::new(&root).unwrap();
        let doc = MemoryDocument::new(
            MemoryKind::Note,
            MemorySourceKind::Note,
            "Credenciais não entram",
            None,
            "body: ok\nAuthorization: Bearer secret\npassword=hunter2",
        );
        let id = match store.capture(doc).unwrap() {
            CaptureOutcome::Stored(id) => id,
            CaptureOutcome::SkippedPrivate | CaptureOutcome::SkippedForgotten => {
                panic!("document should be stored")
            }
        };

        let loaded = store.get(&id).unwrap().unwrap();
        assert!(!loaded.body.contains("secret"));
        assert!(!loaded.body.contains("hunter2"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forget_session_removes_source_and_derived_files() {
        let root = temp_root("forget");
        let store = MemoryStore::new(&root).unwrap();
        for title in ["A", "B"] {
            store
                .capture(
                    MemoryDocument::new(
                        MemoryKind::Source,
                        MemorySourceKind::Reader,
                        title,
                        None,
                        "conteúdo",
                    )
                    .session("session-x"),
                )
                .unwrap();
        }

        let report = store
            .forget(ForgetScope::Session("session-x".into()))
            .unwrap();
        assert_eq!(report.documents, 2);
        assert!(store.documents().unwrap().is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_capture_is_idempotent_and_no_vector_fallback_still_finds_text() {
        let root = temp_root("dedup");
        let store = MemoryStore::new(&root).unwrap();
        let mut doc = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Reader,
            "Raft e consenso",
            Some("https://example.com/raft".into()),
            "Leader election replica o log distribuído.",
        );
        doc.embedding.clear();

        let first = doc.id.clone();
        store.capture(doc.clone()).unwrap();
        store.capture(doc).unwrap();

        let documents = store.documents().unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].id, first);

        let hits = store.query(&MemoryQuery::new("Leader election")).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].matched_by.iter().any(|source| source == "lexical"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn memory_doctor_reports_corruption_and_rebuilds_healthy_index() {
        let root = temp_root("doctor");
        let store = MemoryStore::new(&root).unwrap();
        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Documento saudável",
                None,
                "conteúdo indexável",
            ))
            .unwrap();

        fs::write(store.documents_dir().join("corrupt.json"), b"{not-json").unwrap();
        let report = store.doctor(true).unwrap();
        assert_eq!(report.schema, MEMORY_SCHEMA_VERSION);
        assert_eq!(report.documents, 1);
        assert_eq!(report.corrupt_documents, 1);
        assert!(report.manifest_present);
        assert!(report.rebuilt);

        let manifest = fs::read_to_string(root.join("db").join("index-manifest.json")).unwrap();
        assert!(manifest.contains(&format!("\"schema\": {}", MEMORY_SCHEMA_VERSION)));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sqlite_v01_index_is_lazy_and_rebuildable() {
        let root = temp_root("sqlite-v01");
        let store = MemoryStore::new(&root).unwrap();
        assert!(
            !store.sqlite_path().exists(),
            "constructing MemoryStore must not open/create SQLite"
        );

        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Web,
                "SQLite V01",
                None,
                "rebuildable derived index",
            ))
            .unwrap();
        assert!(store.sqlite_path().exists());

        store.rebuild().unwrap();
        assert!(store.sqlite_path().exists());

        let _ = fs::remove_dir_all(root);
    }
}
