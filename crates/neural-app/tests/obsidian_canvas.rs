#[test]
fn shipped_obsidian_canvas_survives_layout_search_and_note_creation() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/test-obsidian-canvas.mjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .output()
        .expect("node is required for the shipped Obsidian canvas gate");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
