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

#[cfg(test)]
thread_local! {
    static DOCUMENT_FILE_COUNT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn reset_document_file_count_calls() {
    DOCUMENT_FILE_COUNT_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
fn document_file_count_calls() -> usize {
    DOCUMENT_FILE_COUNT_CALLS.with(std::cell::Cell::get)
}

use crate::{
    agent_security::{redact_sensitive_text, redact_url},
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
        // O titulo e texto livre vindo da pagina e a URL costuma carregar
        // credenciais na query: os dois passam pelo mesmo cuidado que o corpo
        // sempre teve. A redaccao acontece ANTES do id, para o mesmo documento
        // dar sempre o mesmo id, com ou sem segredo na URL de origem.
        let title = redact_sensitive_text(&title.into());
        let url = url.map(|value| redact_url(&value));
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
        // Indice perdido com corpus em disco: um upsert de um so documento
        // criaria um indice parcial e as consultas deixariam de ver o resto.
        // So neste caminho se paga O(corpus); com indice presente e constante.
        let rebuild_index = !sqlite_existed && self.document_file_count()? > 0;

        document.title = redact_sensitive_text(&document.title);
        document.url = document.url.take().map(|value| redact_url(&value));
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
        // Um `stat` constante em vez de listar o directorio inteiro para saber
        // se este documento e novo.
        let is_new = !json_path.exists();
        atomic_write(
            &json_path,
            &serde_json::to_vec_pretty(&document).map_err(io::Error::other)?,
        )?;

        let wiki_path = self.wiki_path(&document);
        if let Some(parent) = wiki_path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(&wiki_path, self.markdown(&document).as_bytes())?;

        if rebuild_index {
            self.rebuild()?;
            return Ok(CaptureOutcome::Stored(document.id));
        }

        let session = self.session_for_document(&document);
        match sqlite_v01::upsert(&self.sqlite_path(), &document, session.as_ref()) {
            // Indice da v2.0.x: migra-se reconstruindo do corpus em disco,
            // que ja inclui este documento.
            Err(error) if sqlite_v01::is_legacy_index_error(&error) => {
                self.rebuild()?;
                return Ok(CaptureOutcome::Stored(document.id));
            }
            result => result?,
        }
        if !sqlite_existed && !tombstones.is_empty() {
            sqlite_v01::sync_tombstones(&self.sqlite_path(), &tombstones)?;
        }
        self.write_index_manifest(Some(usize::from(is_new)))?;
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

    /// Todos os documentos legiveis em documents/, sem filtro de tombstones.
    /// So o forget os usa: tem de voltar a apagar o que um forget anterior
    /// escondeu mas nao conseguiu remover.
    fn documents_on_disk(&self) -> Vec<MemoryDocument> {
        let Ok(entries) = fs::read_dir(self.documents_dir()) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .filter_map(|entry| fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<MemoryDocument>(&bytes).ok())
            .collect()
    }

    pub fn query(&self, query: &MemoryQuery) -> io::Result<Vec<MemoryHit>> {
        let query_text = query.text.trim();
        if query_text.is_empty() {
            return Ok(Vec::new());
        }

        let candidate_limit = query.limit.clamp(1, 100).saturating_mul(16).min(512);
        let candidates = || {
            sqlite_v01::candidate_ids(
                &self.sqlite_path(),
                query_text,
                query.provider.as_deref(),
                query.session_id.as_deref(),
                candidate_limit,
            )
        };
        let candidate_ids = match candidates() {
            // Indice da v2.0.x: migra-se uma vez, do corpus, e responde-se
            // com o indice novo.
            Err(error) if sqlite_v01::is_legacy_index_error(&error) => {
                self.rebuild()?;
                candidates()
            }
            result => result,
        };
        let candidate_ids = match candidate_ids {
            Ok(ids) => ids,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && self.document_file_count()? == 0 =>
            {
                Vec::new()
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "semantic candidate index unavailable; run memory:rebuild before retrying: {error}"
                    ),
                ));
            }
        };

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

        let now = unix_seconds();
        let mut ranked = scores
            .into_iter()
            .map(|(index, (mut score, matched))| {
                let doc = &docs[index];
                score += recency_bonus(doc.last_seen_at, now);
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
        let sessions = self.research_sessions_unfiltered()?;
        let mut report = ForgetReport::default();
        let mut failures = RemovalFailures::default();

        // Persist the deny policy before deleting source files. If the process
        // dies halfway through forget(), stale source cannot be re-imported.
        let previous = self.load_tombstones()?;
        let mut tombstones = previous.clone();
        extend_tombstones_for_scope(&mut tombstones, &scope, &sessions);
        self.save_tombstones(&tombstones)?;

        if matches!(scope, ForgetScope::All) {
            // Tudo o que esta por baixo destes directorios e derivado das
            // paginas: JSON truncados e `.tmp` de escritas interrompidas
            // tambem guardam o corpo, e nao passam por `documents()`.
            report.documents = self.documents()?.len();
            for dir in ["documents", "wiki", "sessions"] {
                remove_files_under(&self.root.join(dir), &mut report.files, &mut failures);
            }
            let _ = fs::remove_file(self.sqlite_path());
            // Copias de rebuilds interrompidos, seja qual for o pid: um pid
            // reutilizado nao pode esconder uma copia do corpus do forget.
            for (leftover, _) in sqlite_v01::rebuild_temp_files(&self.sqlite_path()) {
                remove_file_counted(&leftover, &mut report.files, &mut failures);
            }
        } else {
            // Ficheiros crus, nao `documents()`: um documento que um forget
            // anterior nao conseguiu apagar ja esta escondido pela tombstone
            // e tem de ser tentado outra vez.
            for document in self.documents_on_disk() {
                let in_scope = matches_scope(&document, &scope);
                if !in_scope && !tombstone_blocks_document(&document, &tombstones) {
                    continue;
                }
                for path in [self.document_path(&document.id), self.wiki_path(&document)] {
                    remove_file_counted(&path, &mut report.files, &mut failures);
                }
                if in_scope && !tombstone_blocks_document(&document, &previous) {
                    report.documents += 1;
                }
            }
            if let ForgetScope::Session(id) = &scope {
                let path = self.root.join("sessions").join(format!("{id}.json"));
                remove_file_counted(&path, &mut report.files, &mut failures);
            }
        }

        self.rebuild()?;
        failures.into_result()?;
        Ok(report)
    }

    pub fn rebuild(&self) -> io::Result<()> {
        self.ensure_layout()?;
        let tombstones = self.load_tombstones()?;
        let docs = self.documents()?;
        let sessions = self.research_sessions()?;
        sqlite_v01::rebuild(&self.sqlite_path(), &docs, &sessions)?;
        sqlite_v01::sync_tombstones(&self.sqlite_path(), &tombstones)?;
        self.write_index_manifest(None)
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

    /// Quantos documentos estao guardados, sem abrir nenhum.
    fn document_file_count(&self) -> io::Result<usize> {
        #[cfg(test)]
        DOCUMENT_FILE_COUNT_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));

        let Ok(entries) = fs::read_dir(self.documents_dir()) else {
            return Ok(0);
        };
        Ok(entries
            .flatten()
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .count())
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join("db").join("index-manifest.json")
    }

    /// Quantos documentos o manifesto anterior declarava, ou `None` quando nao
    /// ha manifesto legivel.
    fn manifest_documents(&self) -> Option<usize> {
        #[derive(Deserialize)]
        struct Counted {
            documents: usize,
        }
        let bytes = fs::read(self.manifest_path()).ok()?;
        serde_json::from_slice::<Counted>(&bytes)
            .ok()
            .map(|counted| counted.documents)
    }

    /// `delta` e quantos documentos NOVOS esta escrita acrescenta: `Some(0)`
    /// quando se reescreveu um que ja existia, `Some(1)` quando nasceu um, e
    /// `None` quando se quer recontar do zero (rebuild, forget).
    fn write_index_manifest(&self, delta: Option<usize>) -> io::Result<()> {
        #[derive(Serialize)]
        struct Manifest {
            schema: u32,
            generated_at: u64,
            documents: usize,
            sqlite: &'static str,
            retrieval: [&'static str; 4],
        }

        // Contar ficheiros custa uma leitura do directorio, e este manifesto
        // escreve-se A CADA CAPTURA: com 600 paginas no corpus, o CI do Windows
        // media 115 ms na primeira captura contra 559 ms na ultima, e o gate de
        // `memory_capture_cost` apanhou-o. Foi a minha propria correccao
        // anterior que deixou isto para tras: troquei "desserializar o corpus
        // todo" por "listar o directorio", que e muito mais barato mas continua
        // a ser linear.
        //
        // O numero nao e lido por codigo nenhum (o `doctor` so verifica que o
        // ficheiro existe), por isso a captura soma ao que ja estava escrito e
        // so o `rebuild`/`forget` reconta.
        let documents = match delta {
            Some(delta) => self.manifest_documents().unwrap_or(0).saturating_add(delta),
            None => self.document_file_count()?,
        };

        let manifest = Manifest {
            schema: MEMORY_SCHEMA_VERSION,
            generated_at: unix_seconds(),
            documents,
            sqlite: "rusqlite-v01-derived",
            retrieval: ["lexical", "entity", "graph", "semantic"],
        };
        atomic_write(
            &self.manifest_path(),
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

fn recency_bonus(last_seen_at: u64, now: u64) -> f32 {
    const RECENT_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;
    if last_seen_at > now || now.saturating_sub(last_seen_at) >= RECENT_WINDOW_SECS {
        0.0
    } else {
        0.002
    }
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
        .ok()
        .filter(|url| url.host_str().is_some())
        .or_else(|| Url::parse(&format!("https://{}", trimmed.trim_start_matches('.'))).ok())?;
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

/// Deletes que falharam durante um forget. O forget apaga tudo o que consegue
/// e so depois devolve o erro: a UI nao pode dizer "apagado" com o corpo de
/// uma pagina ainda no disco.
#[derive(Default)]
struct RemovalFailures {
    count: usize,
    first: Option<(PathBuf, io::Error)>,
}

impl RemovalFailures {
    fn push(&mut self, path: &Path, error: io::Error) {
        self.count += 1;
        if self.first.is_none() {
            self.first = Some((path.to_path_buf(), error));
        }
    }

    fn into_result(self) -> io::Result<()> {
        match self.first {
            None => Ok(()),
            Some((path, error)) => Err(io::Error::new(
                error.kind(),
                format!(
                    "forget incomplete: {} file(s) could not be deleted and will be retried by the next forget; first {}: {error}",
                    self.count,
                    path.display()
                ),
            )),
        }
    }
}

fn remove_file_counted(path: &Path, removed: &mut usize, failures: &mut RemovalFailures) {
    match fs::remove_file(path) {
        Ok(()) => *removed += 1,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => failures.push(path, error),
    }
}

/// Apaga todos os ficheiros por baixo de `dir`, seja qual for o conteudo;
/// mantem os directorios, que fazem parte do layout.
fn remove_files_under(dir: &Path, removed: &mut usize, failures: &mut RemovalFailures) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => return failures.push(dir, error),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failures.push(dir, error);
                continue;
            }
        };
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => remove_files_under(&path, removed, failures),
            Ok(_) => remove_file_counted(&path, removed, failures),
            Err(error) => failures.push(&path, error),
        }
    }
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

    #[test]
    fn redact_url_removes_credentials_and_keeps_the_rest() {
        // O caso que motivou isto: um callback OAuth capturado pela memoria.
        let out =
            redact_url("https://app.exemplo.com/callback?access_token=ya29.SEGREDO&state=abc");
        assert!(!out.contains("ya29.SEGREDO"), "{out}");
        assert!(out.contains("state=abc"), "a query util sobrevive: {out}");

        // Fluxo implicito: o token vem depois do `#`.
        let out =
            redact_url("https://app.exemplo.com/#access_token=ya29.SEGREDO&token_type=bearer");
        assert!(!out.contains("ya29.SEGREDO"), "{out}");

        // Sufixos: `x_api_key`, `user_password`.
        let out = redact_url("https://api.exemplo.com/v1?user_password=hunter2&page=3");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(out.contains("page=3"), "{out}");

        // Credenciais embutidas.
        let out = redact_url("https://ana:hunter2@exemplo.com/privado");
        assert!(!out.contains("hunter2"), "{out}");

        // Uma URL normal nao se mexe.
        assert_eq!(
            redact_url("https://exemplo.pt/artigo?q=rust+ownership&page=2"),
            "https://exemplo.pt/artigo?q=rust+ownership&page=2"
        );

        // Nao sendo URL, cai no redactor de texto.
        assert!(!redact_url("password=hunter2").contains("hunter2"));
    }

    #[test]
    fn captured_document_never_stores_a_secret_in_its_url_or_title() {
        let document = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Sessao aberta com token=ya29.SEGREDO",
            Some("https://app.exemplo.com/cb?access_token=ya29.SEGREDO&state=ok".into()),
            "corpo qualquer",
        );

        let url = document.url.clone().unwrap_or_default();
        assert!(!url.contains("ya29.SEGREDO"), "url: {url}");
        assert!(url.contains("state=ok"), "url: {url}");
        assert!(
            !document.title.contains("ya29.SEGREDO"),
            "title: {}",
            document.title
        );
    }

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
    fn recency_bonus_uses_wall_clock_not_newest_document() {
        let now = 2_000_000u64;
        let six_days = 6 * 24 * 60 * 60;
        let eight_days = 8 * 24 * 60 * 60;

        assert_eq!(recency_bonus(now - six_days, now), 0.002);
        assert_eq!(recency_bonus(now - eight_days, now), 0.0);
        assert_eq!(recency_bonus(now + 60, now), 0.0);
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
    fn query_reports_missing_sqlite_index_instead_of_silent_full_scan() {
        let root = temp_root("missing-candidate-index");
        let store = MemoryStore::new(&root).unwrap();
        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Index visibility",
                None,
                "semantic candidate fallback must never be silent",
            ))
            .unwrap();

        fs::remove_file(store.sqlite_path()).unwrap();
        let error = store
            .query(&MemoryQuery::new("semantic candidate"))
            .expect_err("missing derived index must be visible to the caller");
        let message = error.to_string();
        assert!(message.contains("candidate index"), "{message}");
        assert!(message.contains("rebuild"), "{message}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capture_after_index_loss_does_not_hide_the_older_corpus() {
        let root = temp_root("missing-index-recreated-by-capture");
        let store = MemoryStore::new(&root).unwrap();
        let CaptureOutcome::Stored(id_a) = store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Rust ownership",
                None,
                "rust ownership borrowing lifetimes",
            ))
            .unwrap()
        else {
            panic!("A devia ficar guardado");
        };
        fs::remove_file(store.sqlite_path()).unwrap();

        let CaptureOutcome::Stored(id_b) = store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Rust async",
                None,
                "rust async runtime tokio",
            ))
            .unwrap()
        else {
            panic!("B devia ficar guardado");
        };

        match store.query(&MemoryQuery::new("rust")) {
            Err(error) => assert!(error.to_string().contains("rebuild"), "{error}"),
            Ok(hits) => {
                let ids = hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>();
                assert!(ids.contains(&id_a.as_str()), "A sumiu: {ids:?}");
                assert!(ids.contains(&id_b.as_str()), "B sumiu: {ids:?}");
            }
        }

        let _ = fs::remove_dir_all(root);
    }

    /// O indice que a v2.0.x deixava em db/: schema_meta, documents e uma
    /// tabela FTS5 (que cria as suas tabelas-sombra memory_fts_*).
    fn legacy_2_0_store(name: &str) -> (PathBuf, MemoryStore, String) {
        let root = temp_root(name);
        let store = MemoryStore::new(&root).unwrap();
        let CaptureOutcome::Stored(id_a) = store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Rust ownership",
                None,
                "rust ownership borrowing lifetimes",
            ))
            .unwrap()
        else {
            panic!("A devia ficar guardado");
        };
        sqlite_v01::remove_sqlite_sidecars(&store.sqlite_path());
        let legacy = rusqlite::Connection::open(store.sqlite_path()).unwrap();
        legacy
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE schema_meta(version INTEGER NOT NULL);
                 INSERT INTO schema_meta VALUES(1);
                 CREATE TABLE documents(id TEXT PRIMARY KEY,title TEXT NOT NULL,url TEXT,
                   body TEXT NOT NULL,provider TEXT,session_id TEXT,entities TEXT,
                   last_seen INTEGER NOT NULL);
                 CREATE VIRTUAL TABLE memory_fts USING fts5(id UNINDEXED,title,body,entities,
                   tokenize='unicode61 remove_diacritics 2');",
            )
            .unwrap();
        drop(legacy);
        (root, store, id_a)
    }

    #[test]
    fn capture_migrates_a_2_0_index_without_losing_the_older_corpus() {
        let (root, store, id_a) = legacy_2_0_store("legacy-capture");

        let CaptureOutcome::Stored(id_b) = store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Rust async",
                None,
                "rust async runtime tokio",
            ))
            .expect("captura depois do upgrade")
        else {
            panic!("B devia ficar guardado");
        };

        let hits = store.query(&MemoryQuery::new("rust")).expect("consulta");
        let ids = hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>();
        assert!(ids.contains(&id_a.as_str()), "A sumiu: {ids:?}");
        assert!(ids.contains(&id_b.as_str()), "B sumiu: {ids:?}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn query_migrates_a_2_0_index_before_answering() {
        let (root, store, id_a) = legacy_2_0_store("legacy-query");

        let hits = store
            .query(&MemoryQuery::new("rust"))
            .expect("consulta depois do upgrade");
        let ids = hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>();
        assert!(ids.contains(&id_a.as_str()), "A sumiu: {ids:?}");

        // Depois da migracao o indice serve candidatos por FTS, sem cair no
        // full scan: uma captura nova nao pode esconder o corpus antigo.
        let CaptureOutcome::Stored(id_b) = store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Rust async",
                None,
                "rust async runtime tokio",
            ))
            .unwrap()
        else {
            panic!("B devia ficar guardado");
        };
        let hits = store.query(&MemoryQuery::new("rust")).unwrap();
        let ids = hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>();
        assert!(ids.contains(&id_a.as_str()), "A sumiu: {ids:?}");
        assert!(ids.contains(&id_b.as_str()), "B sumiu: {ids:?}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn query_reports_corrupt_sqlite_index_instead_of_silent_full_scan() {
        let root = temp_root("corrupt-candidate-index");
        let store = MemoryStore::new(&root).unwrap();
        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Corrupt index visibility",
                None,
                "corrupt sqlite must not silently change retrieval semantics",
            ))
            .unwrap();

        fs::write(store.sqlite_path(), b"not a sqlite database").unwrap();
        let error = store
            .query(&MemoryQuery::new("corrupt sqlite"))
            .expect_err("corrupt derived index must be visible to the caller");
        let message = error.to_string();
        assert!(message.contains("candidate index"), "{message}");
        assert!(message.contains("rebuild"), "{message}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn query_reports_unopenable_sqlite_index_instead_of_silent_full_scan() {
        let root = temp_root("unopenable-candidate-index");
        let store = MemoryStore::new(&root).unwrap();
        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Reader,
                "Unopenable index visibility",
                None,
                "any candidate-index failure must be visible to the caller",
            ))
            .unwrap();

        fs::remove_file(store.sqlite_path()).unwrap();
        fs::create_dir(store.sqlite_path()).unwrap();

        let error = store
            .query(&MemoryQuery::new("candidate index failure"))
            .expect_err("unopenable derived index must be visible to the caller");
        let message = error.to_string();
        assert!(message.contains("candidate index"), "{message}");
        assert!(message.contains("rebuild"), "{message}");

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
    fn domain_normalization_accepts_host_port_and_full_url() {
        assert_eq!(
            normalize_domain("Example.COM.:443").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            normalize_domain("https://Sub.Example.COM/path?q=1").as_deref(),
            Some("sub.example.com")
        );
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
        let session_path = session.save(&root).unwrap();
        assert_eq!(
            session_path,
            root.join("sessions").join("session-forget.json")
        );

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
    fn capture_never_scans_the_existing_corpus_or_validates_the_whole_index() {
        let root = temp_root("capture-constant-work");
        let store = MemoryStore::new(&root).unwrap();

        const CORPUS: usize = 32;
        for index in 0..CORPUS {
            store
                .capture(MemoryDocument::new(
                    MemoryKind::Source,
                    MemorySourceKind::Web,
                    format!("Documento {index}"),
                    Some(format!("https://example.com/pagina/{index}")),
                    format!("corpo indexavel do documento {index}"),
                ))
                .unwrap();
        }
        assert_eq!(store.manifest_documents(), Some(CORPUS));

        reset_document_file_count_calls();
        sqlite_v01::reset_validate_integrity_calls();

        let fresh = MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Documento novo",
            Some("https://example.com/pagina/nova".into()),
            "captura incremental nao pode percorrer o corpus existente",
        );
        store.capture(fresh.clone()).unwrap();

        assert_eq!(
            document_file_count_calls(),
            0,
            "capture must update the manifest incrementally, never enumerate documents/"
        );
        assert_eq!(
            sqlite_v01::validate_integrity_calls(),
            0,
            "capture must not run whole-index SQLite integrity validation"
        );
        assert_eq!(store.manifest_documents(), Some(CORPUS + 1));

        reset_document_file_count_calls();
        sqlite_v01::reset_validate_integrity_calls();
        store.capture(fresh).unwrap();

        assert_eq!(document_file_count_calls(), 0);
        assert_eq!(sqlite_v01::validate_integrity_calls(), 0);
        assert_eq!(
            store.manifest_documents(),
            Some(CORPUS + 1),
            "rewriting an existing document must not inflate the manifest"
        );

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
