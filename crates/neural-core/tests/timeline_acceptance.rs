use std::{hint::black_box, time::Instant};

use neural_core::{SemanticAnchorKind, semantic_anchors_html};

fn kinds(html: &str) -> Vec<SemanticAnchorKind> {
    semantic_anchors_html(html)
        .into_iter()
        .map(|anchor| anchor.kind)
        .collect()
}

#[test]
fn chatgpt_fixture_exposes_question_answer_heading_and_code() {
    let html = r#"
      <main>
        <div data-message-author-role="user">Como funciona Raft?</div>
        <div data-message-author-role="assistant">Raft replica um log.</div>
        <h2>Eleição</h2>
        <pre><code>term += 1;</code></pre>
      </main>
    "#;
    let anchors = semantic_anchors_html(html);
    assert!(anchors.iter().any(|anchor| {
        anchor.kind == SemanticAnchorKind::Question && anchor.label == "Como funciona Raft?"
    }));
    assert!(anchors.iter().any(|anchor| {
        anchor.kind == SemanticAnchorKind::Answer && anchor.label == "Raft replica um log."
    }));
    assert!(
        anchors
            .iter()
            .any(|anchor| anchor.kind == SemanticAnchorKind::Heading)
    );
    assert!(
        anchors
            .iter()
            .any(|anchor| anchor.kind == SemanticAnchorKind::Code)
    );
}

#[test]
fn gemini_fixture_exposes_answer_table_and_source() {
    let html = r#"
      <main role="main">
        <article>
          <h2>Comparação</h2>
          <table><tr><td>Opção A</td><td>Opção B</td></tr></table>
          <a href="https://example.com/paper">Fonte primária</a>
        </article>
      </main>
    "#;
    let got = kinds(html);
    assert!(got.contains(&SemanticAnchorKind::Answer));
    assert!(got.contains(&SemanticAnchorKind::Heading));
    assert!(got.contains(&SemanticAnchorKind::Table));
    assert!(got.contains(&SemanticAnchorKind::Source));
}

#[test]
fn claude_fixture_exposes_answer_quote_and_conclusion() {
    let html = r#"
      <main>
        <section role="article">Resposta longa do Claude.</section>
        <blockquote>Trecho citado da fonte.</blockquote>
        <h2>Conclusão</h2>
      </main>
    "#;
    let got = kinds(html);
    assert!(got.contains(&SemanticAnchorKind::Answer));
    assert!(got.contains(&SemanticAnchorKind::Quote));
    assert!(got.contains(&SemanticAnchorKind::Conclusion));
}

fn synthetic_document(sections: usize) -> String {
    let mut html = String::with_capacity(sections * 180);
    html.push_str("<main>");
    for i in 0..sections {
        html.push_str("<article><h2>Secao ");
        html.push_str(&i.to_string());
        html.push_str("</h2><p>");
        html.push_str("texto semantico repetido para manter o custo de parsing comparavel ");
        html.push_str("e evitar que o benchmark meca apenas overhead fixo");
        html.push_str("</p><a href=\"https://example.com/");
        html.push_str(&i.to_string());
        html.push_str("\">Fonte</a></article>");
    }
    html.push_str("</main>");
    html
}

fn elapsed_for(html: &str, iterations: usize) -> u128 {
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(semantic_anchors_html(black_box(html)));
    }
    started.elapsed().as_nanos().max(1)
}

#[test]
fn timeline_cost_scales_by_ratio_not_wall_clock_budget() {
    let small = synthetic_document(256);
    let large = synthetic_document(1024);

    // Pares medidos na mesma ronda reduzem ruído do runner. Com entrada 4x
    // maior, um algoritmo aproximadamente linear fica perto de 4x; O(n²)
    // tenderia para ~16x. Usamos a melhor de cinco rondas para não transformar
    // carga momentânea da VM em regressão algorítmica.
    let mut best_ratio = f64::INFINITY;
    for round in 0..5 {
        let (small_ns, large_ns) = if round % 2 == 0 {
            (elapsed_for(&small, 2), elapsed_for(&large, 2))
        } else {
            let large_ns = elapsed_for(&large, 2);
            let small_ns = elapsed_for(&small, 2);
            (small_ns, large_ns)
        };
        best_ratio = best_ratio.min(large_ns as f64 / small_ns as f64);
    }

    assert!(
        best_ratio < 8.0,
        "semantic timeline scaling regressed: 4x input cost {best_ratio:.2}x"
    );
}

#[test]
fn timeline_keeps_a_hard_anchor_cap() {
    let html = synthetic_document(1024);
    assert!(semantic_anchors_html(&html).len() <= 128);
}
