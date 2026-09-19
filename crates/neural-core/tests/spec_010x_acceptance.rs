use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use neural_core::{
    ActionRisk, AgentAction, AgentElement, AgentOutcome, AgentPermissionPolicy, AgentPlanner,
    AgentRuntime, AgentRuntimeConfig, AgentSecurityAction, AgentToolExecutor, CaptureOutcome,
    FieldKind, ForgetScope, HashingLocalIntelligence, IntentClass, LocalIntelligence,
    MemoryDocument, MemoryKind, MemoryQuery, MemorySourceKind, MemoryStore, ObservedPage,
    ResearchItemKind, ResearchSession, SemanticAnchorKind, ToolResult, cosine_similarity,
    semantic_anchors_html,
};

fn temp_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "neuralia-spec-acceptance-{name}-{}-{nonce}",
        std::process::id()
    ))
}

fn regular_file_count(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                regular_file_count(&path)
            } else if path.is_file() {
                1
            } else {
                0
            }
        })
        .sum()
}

fn manifest_document_count(root: &Path) -> usize {
    let bytes = fs::read(root.join("db").join("index-manifest.json")).unwrap();
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .unwrap()
        .get("documents")
        .and_then(serde_json::Value::as_u64)
        .unwrap() as usize
}

#[test]
fn spec_0100_semantic_memory_is_conceptually_searchable_with_provenance() {
    let root = temp_root("memory-search");
    let store = MemoryStore::new(&root).unwrap();

    let public = MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Reader,
        "Microarquitetura em Rust",
        Some("https://example.com/simd".into()),
        "AVX-512 reduz o custo de certas operações SIMD em CPUs compatíveis.",
    )
    .provider("Reader")
    .session("session-a");
    let public_id = public.id.clone();

    assert_eq!(
        store.capture(public).unwrap(),
        CaptureOutcome::Stored(public_id.clone())
    );

    let mut query = MemoryQuery::new("otimização vetorial CPU");
    query.limit = 8;
    let hits = store.query(&query).unwrap();
    let hit = hits
        .iter()
        .find(|hit| hit.id == public_id)
        .expect("conceptual query must find the stored SIMD document");

    assert_eq!(hit.url.as_deref(), Some("https://example.com/simd"));
    assert_eq!(hit.provider.as_deref(), Some("Reader"));
    assert_eq!(hit.session_id.as_deref(), Some("session-a"));
    assert!(
        hit.matched_by.iter().any(|kind| kind == "semantic"),
        "conceptual query must exercise the semantic retrieval stream"
    );

    let doctor = store.doctor(true).unwrap();
    assert_eq!(doctor.documents, 1);
    assert!(doctor.manifest_present);
    assert!(doctor.rebuilt);
    assert_eq!(manifest_document_count(&root), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn spec_0100_private_capture_has_zero_persistent_delta() {
    let root = temp_root("memory-private-zero");
    let store = MemoryStore::new(&root).unwrap();
    let files_before = regular_file_count(&root);
    let rows_before = store.documents().unwrap().len();

    let private = MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Web,
        "Segredo",
        Some("https://private.example/".into()),
        "conteúdo que nunca deve persistir",
    )
    .private(true);

    assert_eq!(
        store.capture(private).unwrap(),
        CaptureOutcome::SkippedPrivate
    );
    assert_eq!(store.documents().unwrap().len(), rows_before);
    assert_eq!(regular_file_count(&root), files_before);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn spec_0100_delete_and_clear_remove_searchable_semantic_state() {
    let root = temp_root("memory-delete-clear");
    let store = MemoryStore::new(&root).unwrap();

    let deleted = MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Reader,
        "Documento apagável",
        Some("https://delete.example/one".into()),
        "quasar-delete-only-token",
    );
    let deleted_id = deleted.id.clone();
    store.capture(deleted).unwrap();
    assert!(
        store
            .query(&MemoryQuery::new("quasar-delete-only-token"))
            .unwrap()
            .iter()
            .any(|hit| hit.id == deleted_id)
    );

    let report = store
        .forget(ForgetScope::Document(deleted_id.clone()))
        .unwrap();
    assert_eq!(report.documents, 1);
    assert!(store.get(&deleted_id).unwrap().is_none());
    assert!(
        store
            .query(&MemoryQuery::new("quasar-delete-only-token"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(manifest_document_count(&root), 0);

    for (title, token) in [
        ("Clear A", "clear-history-token-alpha"),
        ("Clear B", "clear-history-token-beta"),
    ] {
        store
            .capture(MemoryDocument::new(
                MemoryKind::Source,
                MemorySourceKind::Web,
                title,
                None,
                token,
            ))
            .unwrap();
    }
    assert_eq!(store.documents().unwrap().len(), 2);

    let cleared = store.forget(ForgetScope::All).unwrap();
    assert_eq!(cleared.documents, 2);
    assert!(store.documents().unwrap().is_empty());
    assert!(
        store
            .query(&MemoryQuery::new("clear-history-token-alpha"))
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .query(&MemoryQuery::new("clear-history-token-beta"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(manifest_document_count(&root), 0);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn spec_0100_forgotten_domain_stays_absent_after_reindex() {
    let root = temp_root("memory-domain-reindex");
    let store = MemoryStore::new(&root).unwrap();

    let excluded = MemoryDocument::new(
        MemoryKind::Source,
        MemorySourceKind::Web,
        "Excluded domain",
        Some("https://news.example.com/article".into()),
        "domain-exclusion-sentinel",
    );
    let excluded_id = excluded.id.clone();
    store.capture(excluded).unwrap();
    store
        .capture(MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Allowed domain",
            Some("https://keep.example.org/article".into()),
            "allowed-domain-sentinel",
        ))
        .unwrap();

    let report = store
        .forget(ForgetScope::Domain("example.com".into()))
        .unwrap();
    assert_eq!(report.documents, 1);
    store.rebuild().unwrap();

    let excluded_hits = store
        .query(&MemoryQuery::new("domain-exclusion-sentinel"))
        .unwrap();
    assert!(
        excluded_hits.iter().all(|hit| hit.id != excluded_id),
        "reindex must not resurrect the removed document id"
    );
    assert!(
        excluded_hits.iter().all(|hit| {
            hit.url
                .as_deref()
                .is_none_or(|url| !url.contains("example.com"))
        }),
        "reindex must not resurrect a removed domain URL"
    );
    assert_eq!(
        store
            .query(&MemoryQuery::new("allowed-domain-sentinel"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(manifest_document_count(&root), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn spec_0101_research_sessions_roundtrip_exact_source_provenance() {
    let root = temp_root("research-roundtrip");
    let mut session = ResearchSession::new("Qual abordagem usar para observação Web?");
    let mut ids = Vec::new();

    for (provider, title, body) in [
        (
            "Gemini",
            "DOM",
            "DOM fornece estrutura e seletores estáveis. Latência 10 ms em 2026 referência.",
        ),
        (
            "ChatGPT",
            "Accessibility",
            "Accessibility Tree fornece papéis e nomes acessíveis. Latência 12 ms em 2026 referência.",
        ),
        (
            "Claude",
            "Vision",
            "Visão deve ser fallback quando sinais estruturados falham. Latência 20 ms em 2026 referência.",
        ),
        (
            "Gemini",
            "CDP",
            "DevTools fornece metadados adicionais. Latência 8 ms.",
        ),
        (
            "Claude",
            "Security",
            "Conteúdo remoto é dado não confiável e não concede ferramentas.",
        ),
    ] {
        ids.push(session.add_source(
            Some(provider.into()),
            title,
            format!("https://example.com/{}", title.to_lowercase()),
            None,
            body,
        ));
    }

    let facts = session.comparison(&ids);
    assert_eq!(
        facts
            .iter()
            .map(|fact| fact.item_id.as_str())
            .collect::<Vec<_>>(),
        ids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        facts
            .iter()
            .map(|fact| fact.source.as_str())
            .collect::<Vec<_>>(),
        vec!["Gemini", "ChatGPT", "Claude", "Gemini", "Claude"]
    );
    assert!(facts[0].numbers.iter().any(|value| value == "10"));
    assert!(facts[0].dates.iter().any(|value| value == "2026"));

    let synthesis = session.synthesize(&ids);
    assert_eq!(synthesis.item_ids, ids);
    assert_eq!(synthesis.generator, "neuralia-deterministic-local");
    for title in ["DOM", "Accessibility", "Vision", "CDP", "Security"] {
        assert!(synthesis.output.contains(title));
    }

    let path = session.save(&root).unwrap();
    let loaded = ResearchSession::load(&path).unwrap();
    let sources = loaded
        .items
        .iter()
        .filter(|item| item.kind == ResearchItemKind::Source)
        .collect::<Vec<_>>();
    assert_eq!(sources.len(), 5);
    assert_eq!(sources[0].provider.as_deref(), Some("Gemini"));
    assert_eq!(sources[1].provider.as_deref(), Some("ChatGPT"));
    assert_eq!(sources[2].provider.as_deref(), Some("Claude"));
    assert_eq!(sources[0].url.as_deref(), Some("https://example.com/dom"));
    assert_eq!(loaded.syntheses.len(), 1);
    assert_eq!(loaded.syntheses[0].item_ids, synthesis.item_ids);

    let markdown = loaded.export_markdown();
    for needle in [
        "https://example.com/dom",
        "Gemini",
        "ChatGPT",
        "Claude",
        "## Sínteses",
    ] {
        assert!(markdown.contains(needle));
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn spec_0102_local_intelligence_fallback_is_deterministic_and_discriminative() {
    let ai = HashingLocalIntelligence;
    let inputs = vec![
        "WebView2 semantic memory".to_string(),
        "memória semântica WebView2".to_string(),
    ];
    let first = ai.embed(&inputs).unwrap();
    let second = ai.embed(&inputs).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].len(), neural_core::EMBEDDING_DIM);
    assert_ne!(first[0], first[1]);
    for vector in &first {
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    let query = ai.embed(&["otimização vetorial CPU".to_string()]).unwrap();
    let related = ai.embed(&["SIMD em CPUs modernas".to_string()]).unwrap();
    let unrelated = ai.embed(&["receita de bolo".to_string()]).unwrap();
    assert!(
        cosine_similarity(&query[0], &related[0]) > cosine_similarity(&query[0], &unrelated[0])
    );

    assert_eq!(
        ai.classify("onde eu li aquilo sobre AVX-512?").unwrap(),
        IntentClass::MemoryRecall
    );
    assert_eq!(
        ai.classify("compare estas fontes").unwrap(),
        IntentClass::Research
    );
    assert_eq!(
        ai.classify("abra https://example.com").unwrap(),
        IntentClass::Navigate
    );
    assert_eq!(
        ai.classify("clique e extraia os resultados").unwrap(),
        IntentClass::AgentTask
    );
    assert_eq!(ai.classify("bom dia").unwrap(), IntentClass::General);

    let entities = ai.entities("NeuralIA usa WebView2 e AVX-512").unwrap();
    assert!(entities.iter().any(|item| item == "NeuralIA"));
    assert!(entities.iter().any(|item| item == "WebView2"));
    assert!(entities.iter().any(|item| item == "AVX-512"));

    let summary = ai
        .summarize("um texto simples e comprido para resumir sem modelo", 12)
        .unwrap();
    assert!(summary.chars().count() <= 12);
    assert_ne!(
        summary,
        "um texto simples e comprido para resumir sem modelo"
    );
}

#[test]
fn spec_0103_semantic_timeline_preserves_kind_order_labels_and_geometry() {
    let html = r#"
        <div data-message-author-role="user">Pergunta</div>
        <div data-message-author-role="assistant">Resposta</div>
        <h2>Arquitetura</h2>
        <pre><code>let x = 1;</code></pre>
        <table><tr><td>A</td></tr></table>
        <blockquote>Fonte</blockquote>
        <h2>Conclusão</h2>
        <a href="https://example.com">Documento</a>
    "#;
    let anchors = semantic_anchors_html(html);

    let expected_kinds = vec![
        SemanticAnchorKind::Question,
        SemanticAnchorKind::Answer,
        SemanticAnchorKind::Heading,
        SemanticAnchorKind::Code,
        SemanticAnchorKind::Table,
        SemanticAnchorKind::Quote,
        SemanticAnchorKind::Conclusion,
        SemanticAnchorKind::Source,
    ];
    assert_eq!(
        anchors.iter().map(|anchor| anchor.kind).collect::<Vec<_>>(),
        expected_kinds
    );
    assert_eq!(
        anchors
            .iter()
            .map(|anchor| anchor.label.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Pergunta",
            "Resposta",
            "Arquitetura",
            "let x = 1;",
            "A",
            "Fonte",
            "Conclusão",
            "Documento",
        ]
    );
    assert_eq!(
        anchors
            .iter()
            .map(|anchor| anchor.ordinal)
            .collect::<Vec<_>>(),
        (0..anchors.len()).collect::<Vec<_>>()
    );
    assert_eq!(anchors.first().unwrap().position, 0.0);
    assert!((anchors.last().unwrap().position - 1.0).abs() < f32::EPSILON);
    assert!(
        anchors
            .windows(2)
            .all(|pair| pair[0].position < pair[1].position)
    );
}

#[test]
fn spec_0104_agent_security_never_grants_restricted_authority() {
    let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
    policy.grant_reversible_session_actions(true);

    let local_pivot = policy.evaluate(&AgentSecurityAction::Navigate {
        url: "http://127.0.0.1:9000/admin".into(),
    });
    assert!(!local_pivot.allowed);
    assert!(local_pivot.requires_confirmation);

    let password = AgentSecurityAction::Password {
        origin: "https://example.com".into(),
    };
    let restricted = policy.evaluate(&password);
    assert_eq!(restricted.risk, ActionRisk::Restricted);
    assert!(!restricted.allowed);
    assert!(restricted.requires_confirmation);

    policy.record_user_confirmation(&password, true);
    assert!(!policy.audit().last().unwrap().allowed);

    policy.stop();
    let after_stop = policy.evaluate(&AgentSecurityAction::Extract {
        origin: "https://example.com".into(),
    });
    assert!(!after_stop.allowed);
}

struct QueuePlanner {
    actions: VecDeque<AgentAction>,
}

impl AgentPlanner for QueuePlanner {
    fn plan(
        &mut self,
        _goal: &str,
        _page: &ObservedPage,
        _trace: &[neural_core::agent_runtime::AgentStep],
    ) -> Result<AgentAction, String> {
        self.actions
            .pop_front()
            .ok_or_else(|| "planner exhausted".to_string())
    }
}

struct MockExecutor {
    page: ObservedPage,
}

impl AgentToolExecutor for MockExecutor {
    fn execute(&mut self, action: &AgentAction) -> Result<ToolResult, String> {
        if matches!(action, AgentAction::Navigate { .. }) {
            self.page.generation = self.page.generation.saturating_add(1);
        }
        Ok(ToolResult {
            page: self.page.clone(),
            output: Some("ok".into()),
        })
    }
}

fn observed_page() -> ObservedPage {
    ObservedPage {
        generation: 1,
        url: "https://example.com/search".into(),
        title: "Search".into(),
        text_excerpt: "results".into(),
        elements: vec![AgentElement {
            id: "query".into(),
            generation: 1,
            role: "textbox".into(),
            name: "query".into(),
            text: String::new(),
            origin: "https://example.com".into(),
            frame: "top".into(),
            visible: true,
            interactable: true,
        }],
    }
}

#[test]
fn spec_0105_agent_runtime_is_bounded_structured_and_human_gated() {
    let page = observed_page();
    let input = page.elements[0].clone();

    let planner = QueuePlanner {
        actions: VecDeque::from([
            AgentAction::TypeText {
                target: input,
                text: "rust webview".into(),
                field: FieldKind::Search,
            },
            AgentAction::Finish {
                summary: "done".into(),
            },
        ]),
    };
    let executor = MockExecutor { page: page.clone() };
    let mut policy = AgentPermissionPolicy::new(Some("https://example.com".into()));
    policy.grant_reversible_session_actions(true);

    let config = AgentRuntimeConfig {
        max_steps: 4,
        max_wall_time: Duration::from_secs(2),
        max_wait: Duration::from_millis(250),
    };
    let mut runtime = AgentRuntime::new(planner, executor, policy, config);

    match runtime.run("pesquisar", page) {
        AgentOutcome::Completed { summary, trace } => {
            assert_eq!(summary, "done");
            assert_eq!(trace.len(), 1);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn spec_0106_roadmap_gate_has_all_core_subsystems_available_together() {
    let root = temp_root("roadmap");
    let _memory = MemoryStore::new(&root).unwrap();
    let _ai = HashingLocalIntelligence;
    let _policy = AgentPermissionPolicy::new(None);
    let anchors = semantic_anchors_html("<h1>NeuralIA</h1>");
    assert!(!anchors.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn spec_0107_ai_memory_provenance_is_vendored_and_rebuildable() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(root.join("third_party/ai-memory/LICENSE").exists());
    assert!(root.join("third_party/ai-memory/UPSTREAM.md").exists());
    assert!(root.join("third_party/ai-memory/PATCHES.md").exists());
    assert!(
        root.join("third_party/ai-memory/upstream/crates/ai-memory-core/src/sanitize.rs")
            .exists()
    );
    assert!(
        root.join("third_party/ai-memory/upstream/crates/ai-memory-store/src/fts_query.rs")
            .exists()
    );
}
