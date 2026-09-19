//! Sentinelas de documentação: não provam comportamento do produto.
//!
//! Servem para impedir que README/specs normativas voltem a descrever uma
//! arquitetura que já não existe. Gates comportamentais continuam nos módulos
//! que executam o comportamento.

const CARGO_ROOT: &str = include_str!("../../../Cargo.toml");
const README: &str = include_str!("../../../README.md");
const ARCH: &str = include_str!("../../../docs/specs/SPEC-0001-architecture.md");
const THREAT: &str = include_str!("../../../docs/specs/SPEC-0015-threat-model.md");
const ROADMAP: &str = include_str!("../../../docs/specs/SPEC-0016-roadmap.md");
const TESTING_CI: &str = include_str!("../../../docs/specs/SPEC-0012-testing-ci.md");
const LOCAL_AI_SPEC: &str = include_str!("../../../md/SPEC-0102-local-intelligence.md");

fn workspace_version() -> &'static str {
    CARGO_ROOT
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("version = \"")
                .and_then(|value| value.strip_suffix('\"'))
        })
        .expect("workspace.package version missing")
}

#[test]
fn readme_version_matches_workspace_version() {
    let expected = format!("NeuralIA **v{}**", workspace_version());
    assert!(
        README.contains(&expected),
        "README must advertise the workspace version: {expected}"
    );
}

#[test]
fn normative_architecture_describes_current_bounded_ipc_and_webview_model() {
    assert!(ARCH.contains("SPEC-0108"));
    assert!(ARCH.contains("authenticated WebView2 message channel"));
    assert!(ARCH.contains("at most three provider"));
    assert!(!ARCH.contains("receive no NeuralIA IPC object"));
    assert!(!ARCH.contains("AI, Reader, and Full Web create at most one system WebView"));

    assert!(THREAT.contains("hard ceiling of three provider surfaces"));
    assert!(!THREAT.contains("one-WebView ceiling"));

    assert!(ROADMAP.contains("authenticated"));
    assert!(ROADMAP.contains("closed-schema WebView2 IPC"));
    assert!(!ROADMAP.contains("no external IPC"));

    assert!(TESTING_CI.contains("portable `neural-app` tests"));
    assert!(TESTING_CI.contains("clippy with `-D warnings` for both crates"));
}

#[test]
fn model_pack_docs_do_not_claim_product_wiring() {
    assert!(LOCAL_AI_SPEC.contains("not a NeuralIA product feature"));
    assert!(README.contains("ainda não está ligada ao produto"));
}

#[test]
fn readme_status_does_not_reintroduce_pre_comparator_ipc_claims() {
    assert!(!README.contains("lazy one-WebView lifecycle"));
    assert!(!README.contains("external IPC isolation"));
    assert!(!README.contains("External Web pages receive no NeuralIA IPC."));
    assert!(README.contains("bounded lazy WebView lifecycle"));
    assert!(README.contains("bounded authenticated WebView2 IPC"));
}
