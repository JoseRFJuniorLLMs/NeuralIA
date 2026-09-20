use std::collections::HashSet;

const MAX_SEMANTIC_ANCHORS: usize = 128;

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

        let dedup_key = if tag == "article" || element.value().attr("role") == Some("article") {
            Some(format!("wrapper:{text}"))
        } else if tag == "a" {
            let href = element.value().attr("href").unwrap_or_default();
            Some(format!("source:{href}:{text}"))
        } else {
            None
        };

        if dedup_key
            .as_ref()
            .is_none_or(|key| seen.insert(key.clone()))
        {
            raw.push((kind, text));
        }
        if raw.len() >= MAX_SEMANTIC_ANCHORS {
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
    fn duplicate_wrappers_do_not_hide_legitimate_repeated_sections() {
        let anchors = semantic_anchors_html(
            "<article><h1>Título</h1><p>Texto</p></article><article><h1>Título</h1><p>Texto</p></article>",
        );

        let wrapper_count = anchors
            .iter()
            .filter(|item| item.kind == SemanticAnchorKind::Answer && item.label == "Título Texto")
            .count();
        let heading_count = anchors
            .iter()
            .filter(|item| item.kind == SemanticAnchorKind::Heading && item.label == "Título")
            .count();

        assert_eq!(wrapper_count, 1);
        assert_eq!(heading_count, 2);
    }

    #[test]
    fn repeated_conclusion_headings_remain_distinct_anchors() {
        let anchors = semantic_anchors_html(
            "<section><h2>Conclusão</h2><p>A</p></section><section><h2>Conclusão</h2><p>B</p></section>",
        );

        let conclusions = anchors
            .iter()
            .filter(|item| item.kind == SemanticAnchorKind::Conclusion)
            .collect::<Vec<_>>();

        assert_eq!(conclusions.len(), 2);
        assert_ne!(conclusions[0].ordinal, conclusions[1].ordinal);
    }

    #[test]
    fn identical_source_links_are_deduplicated_by_href_and_label() {
        let anchors = semantic_anchors_html(
            "<a href=\"https://example.com/a\">Fonte</a><a href=\"https://example.com/a\">Fonte</a><a href=\"https://example.com/b\">Fonte</a>",
        );

        let sources = anchors
            .iter()
            .filter(|item| item.kind == SemanticAnchorKind::Source)
            .collect::<Vec<_>>();

        assert_eq!(sources.len(), 2);
    }
}
