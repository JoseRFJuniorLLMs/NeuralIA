//! Acceptance gates for SPEC-0100..0106 on the code that actually ships.
//!
//! These tests are intentionally source-wiring tests. `neural-app` is a binary
//! with Windows-only UI internals, so an integration test cannot import private
//! `windows_app` functions directly. Reading the compiled source path still
//! gives the property we need here: removing/bypassing the product wiring makes
//! this gate red instead of leaving a parallel library test green.

const APP: &str = include_str!("../src/windows_app.rs");
const IPC: &str = include_str!("../src/ipc.rs");
const CORE_MEMORY: &str = include_str!("../../neural-core/src/memory.rs");
const LOCAL_INTELLIGENCE: &str =
    include_str!("../../neural-core/src/local_intelligence.rs");

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
    assert!(worker.contains("store.forget(neural_core::ForgetScope::All)"));
    assert!(worker.contains("store.rebuild()"));

    let reader = between(APP, "fn capture_reader_memory", "fn reader_webview_builder");
    assert!(reader.contains("MemorySourceKind::Reader"));
    assert!(reader.contains("self.memory.capture(document)"));

    let split = between(APP, "fn open_split_mode", "fn open_private_panel");
    assert!(
        split.matches("if !private").count() >= 2,
        "private split must stay outside persistent history/memory paths"
    );
}

#[test]
fn spec_0101_product_research_session_wires_capture_compare_synthesis_and_export() {
    assert!(APP.contains("let session = ResearchSession::new(query.clone());"));
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
        assert!(APP.contains(required), "missing shipped timeline behavior: {required}");
    }

    let reader = between(APP, "fn reader_webview_builder", "fn external_webview_builder");
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

    let handler = between(APP, "fn handle_agent_observation", "fn extract_agent_observation");
    assert!(handler.contains("agent.policy.record_user_confirmation(&security, approved)"));
    assert!(handler.contains("self.execute_agent_action(&action)"));
}

#[test]
fn spec_0105_shipped_runtime_is_decide_agent_step_not_the_reference_harness() {
    let handler = between(APP, "fn handle_agent_observation", "fn extract_agent_observation");
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
    let input = between(APP, "fn handle_input", "fn submit_current");
    assert!(input.contains("input.strip_prefix(\"agent:\")"));
    assert!(input.contains("input.strip_prefix(\"memory:\")"));
    assert!(input.contains("research:compare"));
    assert!(input.contains("research:synthesize"));
    assert!(input.contains("research:export"));

    assert!(APP.contains("ResearchSession::new(query.clone())"));
    assert!(APP.contains("self.memory.capture(question_memory);"));
    assert!(APP.contains("decide_agent_step("));
    assert!(APP.contains("function semanticAnchors()"));
    assert!(APP.contains("parse_ipc_message(request.body()"));
    assert!(IPC.contains("pub fn parse_ipc_message("));
}
