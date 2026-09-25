//! Ligações entre subsistemas no binário que embarca — NÃO são gates de
//! comportamento.
//!
//! Estes testes leem o **texto** de `windows_app.rs`. Isso prova uma coisa só:
//! que certas peças estão ligadas umas às outras no produto, e não apenas
//! construíveis numa biblioteca à parte. É útil, e é o limite.
//!
//! O que isto NÃO prova, medido a 19/09/2026: com o `policy.evaluate` retirado
//! do `decide_agent_step` — o agente a executar tudo sem política, sem
//! confirmação e sem parar em ações restritas — **os sete testes deste ficheiro
//! passavam**. A string continuava lá; o código já não corria. Ao mesmo tempo,
//! os testes de comportamento em `windows_app::tests::spec_0105_shipping_agent`
//! ficavam vermelhos, 5 de 7.
//!
//! E falham ao contrário também: quando o encaminhamento da omnibox saiu de
//! `handle_input` para `route_input`, sem uma única mudança de comportamento,
//! este ficheiro ficou vermelho.
//!
//! Regra (AGENTS.md §4.3): uma afirmação sobre o que o produto **faz** tem de
//! ser testada onde ela acontece — no bloco `#[cfg(test)] mod tests` dentro do
//! próprio `windows_app.rs`, que alcança as funções privadas, extraindo a
//! decisão para uma função sem UI quando ela estiver entalada num método
//! (`decide_agent_step` e `route_input` são os modelos). Aqui fica só o que o
//! texto consegue provar: presença e, sobretudo, **ausência**.

const ALL_SOURCES: &str = concat!(
    include_str!("../src/windows_app.rs"),
    "\n",
    include_str!("../src/windows_app/theme.rs"),
    "\n",
    include_str!("../src/windows_app/icons.rs"),
    "\n",
    include_str!("../src/windows_app/bar_layout.rs"),
    "\n",
    include_str!("../src/windows_app/tab_row.rs"),
    "\n",
    include_str!("../src/windows_app/native.rs"),
    "\n",
    include_str!("../src/windows_app/splash.rs"),
    "\n",
    include_str!("../src/windows_app/page_scripts.rs"),
    "\n",
    include_str!("../src/windows_app/notes.rs"),
    "\n",
    include_str!("../src/windows_app/side_panel.rs"),
    "\n",
    include_str!("../src/windows_app/services.rs"),
    "\n",
    include_str!("../src/windows_app/search_card.rs"),
    "\n",
    include_str!("../src/windows_app/app/mod.rs"),
    "\n",
    include_str!("../src/windows_app/app/gmail.rs"),
    "\n",
    include_str!("../src/windows_app/app/tools.rs"),
    "\n",
    include_str!("../src/windows_app/app/navigation.rs"),
    "\n",
    include_str!("../src/windows_app/app/panels.rs"),
    "\n",
    include_str!("../src/windows_app/app/split.rs"),
    "\n",
    include_str!("../src/windows_app/app/pages.rs"),
    "\n",
    include_str!("../src/windows_app/app/chrome.rs"),
    "\n",
    include_str!("../src/windows_app/app/compare.rs"),
    "\n",
    include_str!("../src/windows_app/app/tabs.rs"),
    "\n",
    include_str!("../src/windows_app/tests.rs"),
);
const APP: &str = ALL_SOURCES;
const IPC: &str = include_str!("../src/ipc.rs");
const CORE_MEMORY: &str = include_str!("../../neural-core/src/memory.rs");
const LOCAL_INTELLIGENCE: &str = include_str!("../../neural-core/src/local_intelligence.rs");

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .unwrap_or_else(|| panic!("missing start marker: {start}"))
        .1
        .split_once(end)
        .unwrap_or_else(|| panic!("missing end marker: {end}"))
        .0
}

#[test]
fn spec_0100_product_memory_is_worker_backed_reader_wired_and_private_safe() {
    let worker = between(APP, "struct MemoryWorker", "struct UiRect");
    assert!(worker.contains("sync_channel::<MemoryCommand>(128)"));
    assert!(worker.contains(".name(\"neural-memory\".into())"));
    assert!(worker.contains("MemoryStore::new(&root)"));
    assert!(worker.contains("store.capture(document)"));
    assert!(worker.contains("store.query(&MemoryQuery::new(query.clone()))"));
    assert!(worker.contains("ForgetScope::All"));
    assert!(worker.contains("store.rebuild()"));

    let reader = between(APP, "fn capture_reader_memory", "fn reader_webview_builder");
    assert!(reader.contains("MemorySourceKind::Reader"));
    assert!(reader.contains("self.memory.capture(document)"));

    // A regra "navegação privada nunca entra na memória semântica" deixou de
    // ser contada por ocorrências de `if !private` no texto: contar strings
    // passava com a condição invertida e falhava com um refactor inocente.
    // A decisão vive em `split_source_memory` e é testada pelo comportamento em
    // `windows_app::tests::private_split_source_never_becomes_a_memory_document`.
    let split = between(APP, "fn open_split_mode", "fn open_private_panel");
    assert!(split.contains("split_source_memory(&valid, source_name, private)"));
    // Se a fonte vira aba (e por isso chega ao `tabs.json`) decide-o
    // `record_split_context`, testado pelo comportamento em
    // `windows_app::tab_session_gates::a_private_split_never_reaches_the_tab_session_file`.
    assert!(split.contains("record_split_context("));
    assert!(!split.contains("self.record("));
}

#[test]
fn spec_0101_product_research_session_wires_capture_compare_synthesis_and_export() {
    // A sessao e a memoria de uma comparacao saem de `compare_records` (o
    // Traduzir leva o nome do texto, nao o do pedido fixo), testado pelo
    // comportamento em
    // `windows_app::tests::each_translation_is_named_by_its_text_and_reopens_from_history`.
    assert!(APP.contains("ResearchSession::new(request.prompt.clone()).titled(&request.label)"));
    assert!(APP.contains("let (session, question_memory, reopen) = compare_records(&request);"));
    assert!(APP.contains("self.memory.capture(question_memory);"));
    assert!(APP.contains("self.memory.save_session(session.clone());"));
    assert!(APP.contains("let facts = session.comparison(&ids);"));
    assert!(APP.contains("let snapshot = session.synthesize(&ids).clone();"));
    assert!(APP.contains("session.export_markdown()"));
    assert!(APP.contains("IpcAction::ResearchAnswer"));
}

#[test]
fn spec_0102_partial_product_uses_local_semantics_without_booting_model_packs() {
    assert!(CORE_MEMORY.contains("hashed_embedding(&entity_text)"));
    assert!(LOCAL_INTELLIGENCE.contains("pub struct ModelPackManager"));
    assert!(
        !APP.contains("ModelPackManager"),
        "model-pack lifecycle is not a shipped product feature yet; update SPEC-0102 when wiring it"
    );
}

#[test]
fn spec_0103_product_gate_targets_the_shipped_javascript_timeline() {
    assert!(
        APP.matches("function semanticAnchors()").count() >= 2,
        "split/reader and comparator must each keep their shipped semantic mapper"
    );
    for required in [
        "[data-message-author-role=\"user\"]",
        "[data-message-author-role=\"assistant\"]",
        "h1,h2,h3,h4,h5,h6,pre,table,blockquote,aside,article",
        "if (role === 'user') return 'pergunta';",
        "if (role === 'assistant') return 'resposta';",
        "if (tag === 'pre' || tag === 'code') return 'código';",
        "if (tag === 'table') return 'tabela';",
        "const fallbackCount =",
        "requestAnimationFrame(() =>",
    ] {
        assert!(
            APP.contains(required),
            "missing shipped timeline behavior: {required}"
        );
    }

    let reader = between(
        APP,
        "fn reader_webview_builder",
        "fn external_webview_builder",
    );
    assert!(reader.contains("SPLIT_SCROLL_RAIL_SCRIPT"));
    assert!(APP.contains("rail.id = 'neuralia-response-rail';"));
    assert!(APP.contains("rail.id = 'neuralia-split-scroll-rail';"));
}

#[test]
fn spec_0104_product_agent_reaches_the_native_permission_policy() {
    let start = between(APP, "fn start_browser_agent", "fn handle_agent_observation");
    assert!(start.contains("is_local_network_target(&valid)"));
    assert!(start.contains("AgentPermissionPolicy::new(Some(origin))"));

    let decision = between(APP, "fn decide_agent_step", "fn app_agent_security_action");
    assert!(decision.contains("let decision = policy.evaluate(&security);"));
    assert!(decision.contains("decision.risk == ActionRisk::Restricted"));
    assert!(decision.contains("policy.record_user_confirmation(&security, false)"));

    let handler = between(
        APP,
        "fn handle_agent_observation",
        "fn extract_agent_observation",
    );
    assert!(handler.contains("agent.policy.record_user_confirmation(&security, approved)"));
    assert!(handler.contains("self.execute_agent_action(&action)"));
}

#[test]
fn spec_0105_shipped_runtime_is_decide_agent_step_not_the_reference_harness() {
    let handler = between(
        APP,
        "fn handle_agent_observation",
        "fn extract_agent_observation",
    );
    assert!(handler.contains("decide_agent_step("));
    assert!(handler.contains("AgentStepDecision::Stop(reason)"));
    assert!(handler.contains("AgentStepDecision::Act(act)"));
    assert!(handler.contains("self.confirm_agent_action(&reason, &action)"));
    assert!(handler.contains("self.execute_agent_action(&action)"));

    let decision = between(APP, "fn decide_agent_step", "fn app_agent_security_action");
    assert!(decision.contains("let budget = AgentRuntimeConfig::default();"));
    assert!(decision.contains("steps >= budget.max_steps"));
    assert!(decision.contains("elapsed >= budget.max_wall_time"));
    assert!(decision.contains("policy.evaluate(&security)"));

    assert!(
        !APP.contains("AgentRuntime::new"),
        "if neural-app starts using AgentRuntime, SPEC-0105 must be updated to that architecture"
    );
}

#[test]
fn spec_0106_roadmap_product_composition_is_wired_not_just_constructible() {
    // O encaminhamento da omnibox saiu daqui: a decisão vive em `route_input`
    // e é testada pelo comportamento, em
    // `windows_app::tests::route_input_sends_each_command_where_it_belongs` e
    // `::memory_rebuild_is_not_swallowed_by_the_memory_prefix`. Foi essa
    // passagem que mostrou que `memory:rebuild` nunca chegava a reconstruir
    // nada -- o prefixo `memory:` apanhava-o primeiro -- com estas asserções
    // de texto todas verdes.
    assert!(APP.contains("fn route_input("));
    assert!(APP.contains("match route_input(&input)"));

    assert!(APP.contains("ResearchSession::new(request.prompt.clone())"));
    assert!(APP.contains("self.memory.capture(question_memory);"));
    assert!(APP.contains("decide_agent_step("));
    assert!(APP.contains("function semanticAnchors()"));
    assert!(APP.contains("parse_ipc_message(request.body()"));
    assert!(IPC.contains("pub fn parse_ipc_message("));
}

/// `ALL_SOURCES` e uma lista a mao, e os gates de ausencia deste ficheiro
/// (SPEC-0102, SPEC-0105) so valem sobre o que ela contem: um modulo que
/// ficasse de fora podia trazer o `ModelPackManager` ou o `AgentRuntime` sem
/// que nada aqui ficasse vermelho. Este gate le a arvore `src/windows_app`
/// no disco e exige o texto de cada ficheiro, e o da raiz, dentro de
/// `ALL_SOURCES`.
#[test]
fn all_sources_holds_every_file_of_the_windows_app_tree() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read src/windows_app") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![manifest.join("src/windows_app.rs")];
    walk(&manifest.join("src/windows_app"), &mut files);
    files.sort();
    assert!(
        files.len() >= 20,
        "so {} ficheiros em src/windows_app?",
        files.len()
    );

    let all = ALL_SOURCES.replace("\r\n", "\n");
    let mut missing = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .expect("read module")
            .replace("\r\n", "\n");
        if !all.contains(text.trim_end()) {
            missing.push(file.display().to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "ficheiros de src/windows_app fora de ALL_SOURCES: {missing:?}"
    );
}
