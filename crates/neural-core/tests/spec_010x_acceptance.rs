use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use neural_core::{
    ActionRisk, AgentPermissionPolicy, AgentSecurityAction, CaptureOutcome,
    HashingLocalIntelligence, IntentClass, LocalIntelligence, MemoryDocument, MemoryKind,
    MemoryQuery, MemorySourceKind, MemoryStore, ResearchItemKind, ResearchSession,
    SemanticAnchorKind, semantic_anchors_html,
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

#[test]
fn spec_0100_semantic_memory_is_local_searchable_and_private_safe() {
    let root = temp_root("memory");
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

    assert!(matches!(
        store.capture(public).unwrap(),
        CaptureOutcome::Stored(_)
    ));

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

    let mut query = MemoryQuery::new("otimização vetorial CPU");
    query.limit = 8;
    let hits = store.query(&query).unwrap();
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .any(|hit| hit.url.as_deref() == Some("https://example.com/simd"))
    );
    assert_eq!(store.documents().unwrap().len(), 1);

    let doctor = store.doctor(true).unwrap();
    assert_eq!(doctor.documents, 1);
    assert!(doctor.manifest_present);
    assert!(doctor.rebuilt);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn spec_0101_research_sessions_compare_synthesize_and_export_with_provenance() {
    let mut session = ResearchSession::new("Qual abordagem usar para observação Web?");
    let mut ids = Vec::new();

    for (provider, title, body) in [
        (
            "Gemini",
            "DOM",
            "DOM fornece estrutura e seletores estáveis.",
        ),
        (
            "ChatGPT",
            "Accessibility",
            "Accessibility Tree fornece papéis e nomes acessíveis.",
        ),
        (
            "Claude",
            "Vision",
            "Visão deve ser fallback quando sinais estruturados falham.",
        ),
        ("Gemini", "CDP", "DevTools fornece metadados adicionais."),
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
    assert_eq!(facts.len(), 5);
    assert!(facts.iter().any(|fact| fact.source == "Claude"));

    let synthesis = session.synthesize(&ids);
    assert_eq!(synthesis.item_ids.len(), 5);
    assert!(synthesis.output.contains("Fontes selecionadas"));

    let markdown = session.export_markdown();
    assert!(markdown.contains("Gemini"));
    assert!(markdown.contains("ChatGPT"));
    assert!(markdown.contains("Claude"));
    assert!(markdown.contains("## Sínteses"));
    assert_eq!(
        session
            .items
            .iter()
            .filter(|item| item.kind == ResearchItemKind::Source)
            .count(),
        5
    );
}

#[test]
fn spec_0102_local_intelligence_has_offline_fallback_capabilities() {
    let ai = HashingLocalIntelligence;
    let inputs = vec![
        "WebView2 semantic memory".to_string(),
        "memória semântica WebView2".to_string(),
    ];
    let vectors = ai.embed(&inputs).unwrap();
    assert_eq!(vectors.len(), 2);
    assert_eq!(vectors[0].len(), neural_core::EMBEDDING_DIM);
    assert!(vectors[0].iter().any(|value| *value != 0.0));

    assert_eq!(
        ai.classify("onde eu li aquilo sobre AVX-512?").unwrap(),
        IntentClass::MemoryRecall
    );
    assert!(
        !ai.entities("NeuralIA usa WebView2 e AVX-512")
            .unwrap()
            .is_empty()
    );
    assert!(
        !ai.summarize("um texto simples para resumir", 12)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn semantic_timeline_core_parser_maps_meaningful_page_units() {
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

    for kind in [
        SemanticAnchorKind::Question,
        SemanticAnchorKind::Answer,
        SemanticAnchorKind::Heading,
        SemanticAnchorKind::Code,
        SemanticAnchorKind::Table,
        SemanticAnchorKind::Quote,
        SemanticAnchorKind::Conclusion,
        SemanticAnchorKind::Source,
    ] {
        assert!(anchors.iter().any(|anchor| anchor.kind == kind));
    }
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

#[test]
fn roadmap_core_subsystems_are_available_together() {
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
