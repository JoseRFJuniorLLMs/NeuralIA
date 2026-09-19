use std::collections::HashSet;

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticAnchorKind {
    Question,
    Answer,
    Heading,
    Code,
    Table,
    Quote,
    Source,
    Conclusion,
    Note,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticAnchor {
    pub ordinal: usize,
    pub kind: SemanticAnchorKind,
    pub label: String,
    pub position: f32,
}

pub fn semantic_anchors_html(input: &str) -> Vec<SemanticAnchor> {
    let document = Html::parse_document(input);
    let selector = Selector::parse(
        "h1,h2,h3,h4,h5,h6,pre,table,blockquote,aside,article,\
         [role='article'],[data-message-author-role],a[href]",
    )
    .expect("static semantic selector");

    let mut raw = Vec::new();
    let mut seen = HashSet::new();

    for element in document.select(&selector) {
        let tag = element.value().name();
        let role = element.value().attr("data-message-author-role");
        let class = element
            .value()
            .attr("class")
            .unwrap_or_default()
            .to_lowercase();
        let text = normalize_label(&element.text().collect::<Vec<_>>().join(" "), 96);

        if text.is_empty() {
            continue;
        }

        let kind = if role == Some("user") {
            SemanticAnchorKind::Question
        } else if role == Some("assistant") {
            SemanticAnchorKind::Answer
        } else if matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
            if text.to_lowercase().contains("conclus") {
                SemanticAnchorKind::Conclusion
            } else {
                SemanticAnchorKind::Heading
            }
        } else if tag == "pre" {
            SemanticAnchorKind::Code
        } else if tag == "table" {
            SemanticAnchorKind::Table
        } else if tag == "blockquote" {
            SemanticAnchorKind::Quote
        } else if tag == "aside" {
            SemanticAnchorKind::Note
        } else if tag == "a" {
            SemanticAnchorKind::Source
        } else if class.contains("conclusion") || class.contains("conclusao") {
            SemanticAnchorKind::Conclusion
        } else {
            SemanticAnchorKind::Answer
        };

        let dedup_key = format!("{kind:?}:{text}");
        if seen.insert(dedup_key) {
            raw.push((kind, text));
        }
        if raw.len() >= 128 {
            break;
        }
    }

    let denominator = raw.len().saturating_sub(1).max(1) as f32;
    raw.into_iter()
        .enumerate()
        .map(|(ordinal, (kind, label))| SemanticAnchor {
            ordinal,
            kind,
            label,
            position: ordinal as f32 / denominator,
        })
        .collect()
}

fn normalize_label(input: &str, limit: usize) -> String {
    let clean = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= limit {
        clean
    } else {
        format!(
            "{}…",
            clean
                .chars()
                .take(limit.saturating_sub(1))
                .collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_blocks_become_navigation_anchors() {
        let html = r#"
            <main>
              <div data-message-author-role="user">Como funciona Raft?</div>
              <div data-message-author-role="assistant">Raft replica um log.</div>
              <h2>Eleição</h2>
              <pre><code>term += 1;</code></pre>
              <table><tr><td>Leader</td><td>Follower</td></tr></table>
              <blockquote>Fonte primária</blockquote>
              <h2>Conclusão</h2>
              <a href="https://raft.github.io/">Raft site</a>
            </main>
        "#;
        let anchors = semantic_anchors_html(html);

        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Question)
        );
        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Answer)
        );
        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Heading)
        );
        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Code)
        );
        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Table)
        );
        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Quote)
        );
        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Conclusion)
        );
        assert!(
            anchors
                .iter()
                .any(|item| item.kind == SemanticAnchorKind::Source)
        );
    }

    #[test]
    fn positions_are_monotonic_and_bounded() {
        let anchors =
            semantic_anchors_html("<h1>A</h1><h2>B</h2><pre>C</pre><blockquote>D</blockquote>");
        assert!(!anchors.is_empty());
        assert!(
            anchors
                .windows(2)
                .all(|pair| pair[0].position <= pair[1].position)
        );
        assert!(
            anchors
                .iter()
                .all(|item| (0.0..=1.0).contains(&item.position))
        );
    }

    #[test]
    fn duplicate_nested_content_does_not_flood_timeline() {
        let anchors = semantic_anchors_html(
            "<article><h1>Título</h1><p>Texto</p></article><article><h1>Título</h1><p>Texto</p></article>",
        );
        let labels = anchors.iter().map(|item| item.label.as_str()).collect::<HashSet<_>>();
        assert_eq!(labels.len(), anchors.len());
    }
}
