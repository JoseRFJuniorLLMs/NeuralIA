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
    /// O turno (`ResearchTurn::ordinal`) a que a resposta pertence; `None`
    /// nos itens de antes dos turnos e nos que não são de um turno.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    /// As ligações citadas na resposta (só `http`/`https`, já validadas por
    /// quem leu a página), na ordem dos marcadores de citação do texto.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
}

/// Quantas leituras do Consenso uma sessão guarda (as mais recentes).
pub const MAX_CONSENSUS_SNAPSHOTS: usize = 8;

/// De onde nasceu um turno: a pergunta às três IAs, a pergunta escrita numa
/// coluna (que segue às outras) ou a pergunta da palette a uma só coluna.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TurnOrigin {
    Compare,
    AskOtherColumns,
    LoadProvider,
}

/// Como acabou uma tentativa de ler a resposta de um provedor num turno.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttemptStatus {
    /// A resposta assentou e foi lida inteira.
    Read,
    /// O prazo passou com a resposta ainda a mudar (ou a página ainda sem
    /// ela): o que se leu por último fica marcado como talvez incompleto.
    MaybeIncomplete,
    /// O provedor não tem seletor de leitura («não lida»).
    Unreadable,
    /// A coluna estava traduzida («traduzida — não comparada»).
    Translated,
    /// A coluna navegou para outra pergunta antes de a leitura assentar.
    OtherQuestion,
    /// A leitura não chegou (a página recusou o script, o JSON não valia).
    Failed,
}

/// Uma tentativa de ler a resposta de um provedor num turno. Nunca se
/// escreve por cima de uma: cada leitura nova é mais uma linha, com o seu
/// número (`attempt`, a contar de 1 por provedor e por turno).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttempt {
    pub provider: String,
    pub attempt: u32,
    pub status: AttemptStatus,
    /// O item (`ResearchItemKind::ProviderAnswer`) com o texto lido, quando
    /// houve texto.
    pub item_id: Option<String>,
    pub created_at: u64,
}

/// Um turno da sessão: uma pergunta feita a um conjunto de provedores, com
/// os invariantes da SPEC-0109 §5.1 (OQ8): o `ordinal` é único na sessão e
/// nunca se reutiliza; a ordem dos provedores fica gravada na criação; as
/// tentativas por provedor só se acrescentam; e `begin_turn` é idempotente
/// pela chave de operação (`operation_key`): repetir a mesma operação
/// devolve o mesmo turno em vez de abrir outro.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchTurn {
    pub ordinal: u32,
    pub operation_key: String,
    pub origin: TurnOrigin,
    pub question: String,
    /// Os provedores perguntados, na ordem em que a pergunta seguiu.
    pub providers: Vec<String>,
    pub created_at: u64,
    #[serde(default)]
    pub attempts: Vec<ProviderAttempt>,
}

impl ResearchTurn {
    /// As tentativas de `provider`, na ordem em que foram feitas.
    pub fn attempts_of<'a>(
        &'a self,
        provider: &'a str,
    ) -> impl Iterator<Item = &'a ProviderAttempt> + 'a {
        self.attempts
            .iter()
            .filter(move |attempt| attempt.provider == provider)
    }
}

/// O que se leu de um provedor numa leitura do Consenso.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotAnswer {
    pub provider: String,
    pub status: AttemptStatus,
    /// O texto lido (Markdown com marcadores de citação), vazio sem leitura.
    pub text: String,
    #[serde(default)]
    pub links: Vec<String>,
    /// O texto foi cortado no tecto do leitor.
    #[serde(default)]
    pub cut: bool,
}

/// Uma leitura do Consenso: o que cada coluna dizia quando o turno assentou.
/// A sessão guarda as `MAX_CONSENSUS_SNAPSHOTS` mais recentes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusSnapshot {
    pub id: String,
    pub turn: u32,
    pub created_at: u64,
    pub answers: Vec<SnapshotAnswer>,
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
    /// Os turnos (consensus-reader-turns, plano 2.5). Uma sessão gravada
    /// antes deles carrega sem nenhum (`default`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<ResearchTurn>,
    /// As leituras do Consenso mais recentes (`MAX_CONSENSUS_SNAPSHOTS`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consensus: Vec<ConsensusSnapshot>,
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
            turns: Vec::new(),
            consensus: Vec::new(),
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

    /// A mesma sessao com o nome de `label` em vez do da pergunta (o
    /// Traduzir: as IAs recebem o pedido de traducao, a lista mostra o texto).
    pub fn titled(mut self, label: &str) -> Self {
        self.title = short_title(label, 72);
        self
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

    // ----- os turnos (SPEC-0109 §5.1) -----

    /// Abre um turno para a operação `operation_key`, ou devolve o que essa
    /// chave já abriu (idempotente: um clique repetido, um evento entregue
    /// duas vezes, nunca abre um turno a mais). O ordinal de um turno novo
    /// é o maior de sempre mais um: único na sessão, nunca reutilizado. A
    /// ordem de `providers` fica gravada tal como veio.
    pub fn begin_turn(
        &mut self,
        operation_key: &str,
        origin: TurnOrigin,
        question: &str,
        providers: &[&str],
    ) -> u32 {
        if let Some(turn) = self
            .turns
            .iter()
            .find(|turn| turn.operation_key == operation_key)
        {
            return turn.ordinal;
        }
        let ordinal = self
            .turns
            .iter()
            .map(|turn| turn.ordinal)
            .max()
            .map_or(1, |max| max.saturating_add(1));
        let created_at = unix_seconds();
        self.turns.push(ResearchTurn {
            ordinal,
            operation_key: operation_key.to_string(),
            origin,
            question: question.to_string(),
            providers: providers.iter().map(|name| name.to_string()).collect(),
            created_at,
            attempts: Vec::new(),
        });
        self.updated_at = created_at;
        ordinal
    }

    pub fn turn(&self, ordinal: u32) -> Option<&ResearchTurn> {
        self.turns.iter().find(|turn| turn.ordinal == ordinal)
    }

    /// O turno mais recente (o de maior ordinal).
    pub fn current_turn(&self) -> Option<&ResearchTurn> {
        self.turns.iter().max_by_key(|turn| turn.ordinal)
    }

    /// Mais uma tentativa de `provider` no turno `ordinal`: acrescenta
    /// sempre uma linha nova (numerada a seguir à última desse provedor) e
    /// nunca escreve por cima de uma anterior. `None` se o turno não existe.
    pub fn record_attempt(
        &mut self,
        ordinal: u32,
        provider: &str,
        status: AttemptStatus,
        item_id: Option<String>,
    ) -> Option<u32> {
        let created_at = unix_seconds();
        let turn = self.turns.iter_mut().find(|turn| turn.ordinal == ordinal)?;
        let attempt = turn
            .attempts
            .iter()
            .filter(|attempt| attempt.provider == provider)
            .map(|attempt| attempt.attempt)
            .max()
            .map_or(1, |max| max.saturating_add(1));
        turn.attempts.push(ProviderAttempt {
            provider: provider.to_string(),
            attempt,
            status,
            item_id,
            created_at,
        });
        self.updated_at = created_at;
        Some(attempt)
    }

    /// A resposta lida de `provider` no turno `ordinal`, como um item novo
    /// (uma leitura nova nunca substitui a anterior: cada tentativa tem o
    /// seu item), com as ligações citadas.
    pub fn add_turn_answer(
        &mut self,
        ordinal: u32,
        provider: &str,
        text: impl Into<String>,
        links: Vec<String>,
    ) -> String {
        let id = self.push_item(
            ResearchItemKind::ProviderAnswer,
            format!("Resposta · {provider} · turno {ordinal}"),
            Some(provider.to_string()),
            None,
            None,
            text.into(),
        );
        if let Some(item) = self.items.iter_mut().rev().find(|item| item.id == id) {
            item.turn = Some(ordinal);
            item.links = links;
        }
        id
    }

    /// Guarda uma leitura do Consenso, largando a mais antiga quando já há
    /// `MAX_CONSENSUS_SNAPSHOTS`. Devolve o id dela.
    pub fn push_consensus(&mut self, turn: u32, answers: Vec<SnapshotAnswer>) -> String {
        let created_at = unix_seconds();
        let id = unique_id(&[&self.id, &turn.to_string()], 20);
        self.consensus.push(ConsensusSnapshot {
            id: id.clone(),
            turn,
            created_at,
            answers,
        });
        if self.consensus.len() > MAX_CONSENSUS_SNAPSHOTS {
            let excess = self.consensus.len() - MAX_CONSENSUS_SNAPSHOTS;
            self.consensus.drain(..excess);
        }
        self.updated_at = created_at;
        id
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
            turn: None,
            links: Vec::new(),
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

    /// So do crate: fora dele a sessao grava-se por `MemoryStore::save_session`,
    /// e a `MemoryStore` so a abre quem e dono da memoria (no NeuralIA, o
    /// worker que o `PrivacyGuard` arranca). Assim nenhum modulo do app grava
    /// `memory/sessions/` por fora do modo privado.
    pub(crate) fn save(&self, root: impl AsRef<Path>) -> io::Result<PathBuf> {
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

/// A chave de operação de um turno: a origem, a coluna de onde partiu
/// (`source`, `None` na pergunta às três), o texto (só o seu resumo SHA-256)
/// e a geração de navegação da coluna de origem no momento do pedido. A
/// mesma pergunta repetida da mesma coluna antes de ela navegar (um Enter
/// repetido, um evento em duplicado) dá a mesma chave -- e `begin_turn`
/// devolve o mesmo turno; depois de a coluna navegar, a mesma pergunta é
/// outra operação. Determinística: nada de relógio nem de nonce.
pub fn operation_key(origin: TurnOrigin, source: Option<usize>, text: &str, epoch: u64) -> String {
    let origin = match origin {
        TurnOrigin::Compare => "compare",
        TurnOrigin::AskOtherColumns => "ask",
        TurnOrigin::LoadProvider => "load",
    };
    let source = source.map_or_else(|| "-".to_string(), |index| index.to_string());
    let mut digest = Sha256::new();
    digest.update(text.trim().as_bytes());
    let hex = format!("{:x}", digest.finalize());
    format!("{origin}:{source}:{}:{epoch}", &hex[..16])
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

    // ----- os turnos (consensus-reader-turns; SPEC-0109 §5.1) -----

    const THREE: [&str; 3] = ["Google IA", "ChatGPT", "Claude"];

    /// Gate (crítico, dados do utilizador): o ordinal de um turno é único
    /// na sessão e nunca se reutiliza -- mesmo numa sessão carregada do
    /// disco com um buraco na numeração, o próximo é o maior mais um.
    /// Sabotagem: `begin_turn` a numerar por `turns.len() + 1` (o buraco
    /// dá um ordinal repetido).
    #[test]
    fn turn_ordinals_are_unique_and_never_reused() {
        let mut session = ResearchSession::new("ordinais");
        let first = session.begin_turn("compare:a", TurnOrigin::Compare, "a", &THREE);
        let second = session.begin_turn("ask:b", TurnOrigin::AskOtherColumns, "b", &THREE[1..]);
        let third = session.begin_turn("load:c", TurnOrigin::LoadProvider, "c", &THREE[..1]);
        assert_eq!((first, second, third), (1, 2, 3));

        // Uma sessão gravada com os turnos 1 e 3 (o 2 foi de outra versão
        // ou de outra janela): o próximo é o 4, nunca o 2 outra vez.
        let mut json: serde_json::Value = serde_json::to_value(&session).expect("json");
        let turns = json["turns"].as_array_mut().expect("turns");
        turns.remove(1);
        let mut reloaded: ResearchSession = serde_json::from_value(json).expect("reload");
        assert_eq!(
            reloaded
                .turns
                .iter()
                .map(|turn| turn.ordinal)
                .collect::<Vec<_>>(),
            [1, 3]
        );
        let next = reloaded.begin_turn("ask:d", TurnOrigin::AskOtherColumns, "d", &THREE);
        assert_eq!(next, 4);
        let ordinals: HashSet<u32> = reloaded.turns.iter().map(|turn| turn.ordinal).collect();
        assert_eq!(ordinals.len(), reloaded.turns.len(), "ordinal repetido");
        assert_eq!(reloaded.current_turn().map(|turn| turn.ordinal), Some(4));
    }

    /// Gate (crítico, dados do utilizador): `begin_turn` é idempotente pela
    /// chave de operação -- a mesma operação entregue duas vezes (um clique
    /// repetido, um evento em duplicado) devolve o mesmo turno e não abre
    /// outro; a ordem dos provedores é a da criação, mesmo que a repetição
    /// venha com outra. Sabotagem: `begin_turn` sem a procura pela chave.
    #[test]
    fn begin_turn_is_idempotent_by_operation_key() {
        let mut session = ResearchSession::new("idempotente");
        let first = session.begin_turn("ask:1:abc", TurnOrigin::AskOtherColumns, "q", &THREE);
        let again = session.begin_turn(
            "ask:1:abc",
            TurnOrigin::AskOtherColumns,
            "q",
            &["Claude", "ChatGPT", "Google IA"],
        );
        assert_eq!(first, again);
        assert_eq!(session.turns.len(), 1, "a repetição abriu outro turno");
        assert_eq!(session.turn(first).expect("turno").providers, THREE);
        assert_eq!(
            session.turn(first).expect("turno").origin,
            TurnOrigin::AskOtherColumns
        );
        // Outra chave: outro turno.
        let other = session.begin_turn("ask:1:abd", TurnOrigin::AskOtherColumns, "q", &THREE);
        assert_ne!(other, first);
        assert_eq!(session.turns.len(), 2);
    }

    /// Gate (crítico, dados do utilizador): as tentativas por provedor só
    /// se acrescentam -- a segunda leitura do ChatGPT é a tentativa 2 ao
    /// lado da 1, nunca por cima dela, e cada provedor conta as suas.
    /// Sabotagem: `record_attempt` a substituir a linha do provedor.
    #[test]
    fn attempts_append_and_never_overwrite() {
        let mut session = ResearchSession::new("tentativas");
        let turn = session.begin_turn("compare:x", TurnOrigin::Compare, "x", &THREE);
        assert_eq!(
            session.record_attempt(turn, "ChatGPT", AttemptStatus::MaybeIncomplete, None),
            Some(1)
        );
        let item = session.add_turn_answer(
            turn,
            "ChatGPT",
            "texto lido",
            vec!["https://example.com/a".into()],
        );
        assert_eq!(
            session.record_attempt(turn, "ChatGPT", AttemptStatus::Read, Some(item.clone())),
            Some(2)
        );
        assert_eq!(
            session.record_attempt(turn, "Claude", AttemptStatus::Translated, None),
            Some(1)
        );
        assert_eq!(
            session.record_attempt(turn, "ChatGPT", AttemptStatus::OtherQuestion, None),
            Some(3)
        );
        assert_eq!(
            session.record_attempt(99, "ChatGPT", AttemptStatus::Read, None),
            None
        );

        let recorded = session.turn(turn).expect("turno");
        let chatgpt: Vec<(u32, AttemptStatus)> = recorded
            .attempts_of("ChatGPT")
            .map(|attempt| (attempt.attempt, attempt.status))
            .collect();
        assert_eq!(
            chatgpt,
            [
                (1, AttemptStatus::MaybeIncomplete),
                (2, AttemptStatus::Read),
                (3, AttemptStatus::OtherQuestion),
            ]
        );
        assert_eq!(
            recorded.attempts.len(),
            4,
            "uma tentativa foi escrita por cima"
        );
        assert_eq!(
            recorded
                .attempts_of("ChatGPT")
                .nth(1)
                .and_then(|attempt| attempt.item_id.as_deref()),
            Some(item.as_str())
        );
        // A resposta do turno é um item próprio, com o turno e as ligações.
        let answer = session
            .items
            .iter()
            .find(|candidate| candidate.id == item)
            .expect("item");
        assert_eq!(answer.turn, Some(turn));
        assert_eq!(answer.links, ["https://example.com/a"]);
        assert_eq!(answer.kind, ResearchItemKind::ProviderAnswer);
    }

    /// Gate (dados do utilizador): uma sessão gravada pela 2.4 -- sem
    /// `turns`, `consensus`, `turn` nem `links` -- carrega tal e qual, e
    /// volta a gravar-se com o que ganhou. Sabotagem: tirar o
    /// `#[serde(default)]` de `turns`.
    #[test]
    fn a_session_saved_before_turns_still_loads() {
        let old = r#"{
          "id": "abc123",
          "title": "pergunta antiga",
          "question": "pergunta antiga",
          "created_at": 1700000000,
          "updated_at": 1700000001,
          "items": [
            {
              "id": "item1",
              "kind": "question",
              "title": "Pergunta inicial",
              "provider": null,
              "url": null,
              "memory_id": null,
              "text": "pergunta antiga",
              "created_at": 1700000000
            },
            {
              "id": "item2",
              "kind": "provider-answer",
              "title": "Resposta · ChatGPT",
              "provider": "ChatGPT",
              "url": null,
              "memory_id": "mem1",
              "text": "resposta",
              "created_at": 1700000001
            }
          ],
          "syntheses": []
        }"#;
        let mut session: ResearchSession =
            serde_json::from_str(old).expect("a sessão da 2.4 carrega");
        assert_eq!(session.id, "abc123");
        assert!(session.turns.is_empty());
        assert!(session.consensus.is_empty());
        assert_eq!(session.current_turn().map(|turn| turn.ordinal), None);
        assert_eq!(session.items[1].turn, None);
        assert!(session.items[1].links.is_empty());
        // O `upsert` de antes continua a valer para o caminho antigo.
        session.upsert_provider_answer("ChatGPT", "resposta 2", None);
        assert_eq!(session.items.len(), 2);

        let turn = session.begin_turn(
            "compare:abc123",
            TurnOrigin::Compare,
            "pergunta antiga",
            &THREE,
        );
        assert_eq!(turn, 1);
        let json = serde_json::to_string(&session).expect("grava");
        let again: ResearchSession = serde_json::from_str(&json).expect("relê");
        assert_eq!(again.turns.len(), 1);
        assert_eq!(
            again.turn(1).expect("turno").operation_key,
            "compare:abc123"
        );
        // E a mesma sessão sem nenhum campo dos turnos (a 2.4 a ler o que a
        // 2.5 gravou sem turnos) fica com o JSON de sempre.
        let bare: serde_json::Value = serde_json::from_str(old).expect("json");
        let round: serde_json::Value =
            serde_json::to_value(serde_json::from_str::<ResearchSession>(old).expect("carrega"))
                .expect("json");
        assert_eq!(round, bare);
    }

    /// A chave de operação: determinística, a mesma para a mesma pergunta
    /// da mesma coluna na mesma geração de navegação, outra quando qualquer
    /// um deles muda; nunca leva o texto (só o resumo).
    #[test]
    fn operation_key_is_deterministic_and_changes_with_source_text_and_epoch() {
        let key = operation_key(
            TurnOrigin::AskOtherColumns,
            Some(1),
            "  qual é a capital?  ",
            7,
        );
        assert_eq!(
            key,
            operation_key(TurnOrigin::AskOtherColumns, Some(1), "qual é a capital?", 7)
        );
        assert!(key.starts_with("ask:1:"));
        assert!(key.ends_with(":7"));
        assert!(!key.contains("capital"));
        for other in [
            operation_key(TurnOrigin::AskOtherColumns, Some(2), "qual é a capital?", 7),
            operation_key(TurnOrigin::AskOtherColumns, Some(1), "qual é a capital!", 7),
            operation_key(TurnOrigin::AskOtherColumns, Some(1), "qual é a capital?", 8),
            operation_key(TurnOrigin::LoadProvider, Some(1), "qual é a capital?", 7),
            operation_key(TurnOrigin::Compare, None, "qual é a capital?", 7),
        ] {
            assert_ne!(key, other);
        }
        assert!(operation_key(TurnOrigin::Compare, None, "x", 0).starts_with("compare:-:"));
    }

    /// Só as `MAX_CONSENSUS_SNAPSHOTS` leituras mais recentes ficam.
    #[test]
    fn consensus_snapshots_keep_the_most_recent_eight() {
        let mut session = ResearchSession::new("leituras");
        let mut ids = HashSet::new();
        for turn in 1..=10 {
            let id = session.push_consensus(
                turn,
                vec![SnapshotAnswer {
                    provider: "ChatGPT".into(),
                    status: AttemptStatus::Read,
                    text: format!("leitura {turn}"),
                    links: Vec::new(),
                    cut: false,
                }],
            );
            assert!(ids.insert(id));
        }
        assert_eq!(session.consensus.len(), MAX_CONSENSUS_SNAPSHOTS);
        assert_eq!(
            session
                .consensus
                .iter()
                .map(|snapshot| snapshot.turn)
                .collect::<Vec<_>>(),
            (3..=10).collect::<Vec<_>>()
        );
    }
}
