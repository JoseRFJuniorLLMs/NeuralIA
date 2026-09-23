//! Custo da extraccao do Reader com fixtures geradas em codigo, mais o prazo
//! e a desistencia de `extract_article_bounded`. Os tempos medidos aqui sao
//! em debug e servem so para garantir que nada explode; em release o alvo
//! de produto e < 100 ms para a fixture de 2 MiB (SPEC-0008).

use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use neural_core::{
    NeuralError,
    reader::{ReaderArticle, extract_article, extract_article_bounded},
};
use url::Url;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;
const HOSTILE_UNIT: &str = r#"<div class="content"><div class="post"><p>x y z</p>"#;
const HOSTILE_CLOSE: &str = "</div></div>";

fn url() -> Url {
    Url::parse("https://example.com/artigo").expect("url estatica")
}

/// `.content > .post > p` aninhado `depth` niveis, em cadeias irmas ate
/// `target` bytes. Cada nivel era um candidato cujo score percorria a
/// subarvore inteira e cada no subia todos os antepassados: cubico no antigo
/// `extract_article` (19 KiB: 3,8 s; 39 KiB: 30 s; 79 KiB: 292 s, release).
fn hostile_fixture(target: usize, depth: usize) -> String {
    let chain = depth * (HOSTILE_UNIT.len() + HOSTILE_CLOSE.len());
    let mut html = String::from("<html><head><title>Hostil</title></head><body>");
    for _ in 0..(target / chain).max(1) {
        for _ in 0..depth {
            html.push_str(HOSTILE_UNIT);
        }
        for _ in 0..depth {
            html.push_str(HOSTILE_CLOSE);
        }
    }
    html.push_str("</body></html>");
    html
}

/// Uma unica cadeia aninhada do principio ao fim: o pior caso absoluto, que
/// tambem leva o html5ever ao limite -- cada `<div>`/`<p>` percorre a pilha
/// inteira de elementos abertos (`in_scope`), logo o parse e quadratico na
/// profundidade e nao ha nada a fazer deste lado.
fn fully_nested_hostile_fixture(target: usize) -> String {
    hostile_fixture(target, target / (HOSTILE_UNIT.len() + HOSTILE_CLOSE.len()))
}

/// Um artigo normal: muitos <p> com ligacoes, um <h2> de vez em quando, nav
/// e footer a volta.
fn benign_fixture(target: usize) -> String {
    let mut html = String::from(
        r#"<html><head><title>Benigno</title><meta name="description" content="Um artigo longo."></head><body><nav><a href="/">Inicio</a></nav><article><h1>Titulo</h1>"#,
    );
    let mut index = 0usize;
    while html.len() < target {
        index += 1;
        html.push_str(&format!(
            r#"<p>Paragrafo numero {index} com texto suficientemente longo para parecer um artigo a serio, com <a href="/x{index}">uma ligacao</a> e mais algumas palavras no fim.</p>"#
        ));
        if index.is_multiple_of(20) {
            html.push_str(&format!("<h2>Seccao {index}</h2>"));
        }
    }
    html.push_str("</article><footer>rodape</footer></body></html>");
    html
}

/// Mede o parse sozinho e a extraccao completa (que inclui outro parse),
/// para se ver quanto do tempo e do html5ever e quanto e nosso.
fn timed(label: &str, html: &str) -> (neural_core::Result<ReaderArticle>, Duration) {
    let started = Instant::now();
    let parsed = scraper::Html::parse_document(html);
    let parse = started.elapsed();
    drop(parsed);

    let started = Instant::now();
    let result = extract_article(&url(), html);
    let elapsed = started.elapsed();
    println!(
        "{label}: {} KiB -> parse {parse:?}, extraccao completa {elapsed:?}",
        html.len() / KIB
    );
    (result, elapsed)
}

#[test]
fn hostile_fully_nested_sizes_that_used_to_hang() {
    // Os tres tamanhos medidos com o algoritmo antigo (3,8 s, 30 s e 292 s
    // em release); agora cabem folgadamente em debug.
    for size in [19 * KIB, 39 * KIB, 79 * KIB] {
        let html = fully_nested_hostile_fixture(size);
        let (result, elapsed) = timed("hostil aninhado", &html);
        let article = result.expect("HTML hostil continua a extrair");
        assert!(!article.blocks.is_empty());
        assert!(
            elapsed < Duration::from_secs(2),
            "{size} bytes hostis demoraram {elapsed:?}"
        );
    }
}
#[test]
fn hostile_nested_containers_extract_in_linear_time() {
    // Cadeias de 200 niveis lado a lado: o ataque ao extractor sem entregar o
    // tempo todo ao html5ever. O que a SPEC-0004 exige e linearidade, e e isso
    // -- e so isso -- que aqui se mede.
    //
    // Terceira versao deste orcamento, e vale a pena dizer porque:
    //   1. teto absoluto em segundos -> media a velocidade da maquina; falhava
    //      a 4,7 s com o codigo certo e passaria a verde num runner rapido
    //      mesmo com regressao;
    //   2. minimo de tres medicoes POR TAMANHO -> melhor, mas ainda comparava
    //      instantes diferentes: os tres "pequenos" corriam numa janela de
    //      tempo e os tres "grandes" noutra, que e o pior arranjo possivel se a
    //      carga da maquina mudar pelo meio. Falhou na mesma.
    //   3. esta: cada RONDA mede os tres tamanhos seguidos e calcula a razao
    //      DENTRO da ronda. A carga afecta os tres numeradores e denominadores
    //      da mesma maneira, por isso a razao sobrevive ao ruido que as
    //      medicoes absolutas nao sobrevivem. Fica a menor razao das rondas.
    let fixtures: Vec<String> = [128 * KIB, 256 * KIB, 512 * KIB]
        .iter()
        .map(|size| hostile_fixture(*size, 100))
        .collect();

    let mut best_quadruple = f64::MAX;
    let mut best_double = f64::MAX;
    for round in 0..3 {
        let mut seconds = Vec::new();
        for html in &fixtures {
            let (result, elapsed) = timed(&format!("hostil em cadeias r{round}"), html);
            let article = result.expect("HTML hostil continua a extrair");
            assert!(!article.blocks.is_empty());
            seconds.push(elapsed.as_secs_f64().max(1e-6));
        }
        best_quadruple = best_quadruple.min(seconds[2] / seconds[0]);
        best_double = best_double.min(seconds[1] / seconds[0]);
    }

    // Quadruplicar a entrada com a mesma profundidade quadruplica o trabalho de
    // um extractor linear. O antigo era cubico -- 4x a entrada valia ~64x o
    // tempo (19 KiB 3,8 s -> 79 KiB 292 s, em release) -- por isso o tecto de
    // 8x deixa folga para o ruido e continua a apanhar isso de longe.
    assert!(
        best_quadruple <= 8.0,
        "4x a entrada custou {best_quadruple:.1}x o tempo -- crescimento super-linear"
    );

    // O passo intermedio prende o mesmo pela metade: 2x a entrada, <= 4x o
    // tempo. Sem ele, um salto so no ultimo tamanho escondia-se na folga do
    // tecto anterior.
    assert!(
        best_double <= 4.0,
        "2x a entrada custou {best_double:.1}x o tempo"
    );
}

#[test]
fn benign_two_mib_fixture_extracts_quickly() {
    // Em release o alvo de produto e < 100 ms (SPEC-0008); em debug so se
    // garante a ordem de grandeza.
    let html = benign_fixture(2 * MIB);
    let (result, elapsed) = timed("benigno", &html);
    let article = result.expect("artigo benigno extrai");
    assert_eq!(article.title, "Benigno");
    assert!(article.blocks.len() >= 600, "{}", article.blocks.len());
    assert!(
        elapsed < Duration::from_secs(3),
        "2 MiB benignos demoraram {elapsed:?}"
    );
}

#[test]
fn deeply_nested_blocks_preserve_inner_text() {
    // blockquote dentro de blockquote: cada bloco lia a subarvore toda e o
    // texto identico nunca contava para MAX_BLOCKS, logo quadratico. A
    // linearidade do extractor e medida por razoes em
    // hostile_nested_containers_extract_in_linear_time. Nesta fixture, o parse
    // do html5ever domina o tempo e depende da velocidade do runner; o que
    // importa aqui e preservar o texto do bloco mais profundo.
    let depth = 4_000;
    let mut html = String::from("<html><head><title>Fundo</title></head><body><article>");
    html.push_str(&"<blockquote>".repeat(depth));
    html.push_str("<p>Texto no fundo do poco, suficientemente longo para contar.</p>");
    html.push_str(&"</blockquote>".repeat(depth));
    html.push_str("</article></body></html>");

    let article = extract_article(&url(), &html).expect("blocos aninhados extraem");
    assert!(format!("{:?}", article.blocks).contains("fundo do poco"));
}

#[test]
fn expired_deadline_is_reported_before_parsing() {
    let html = benign_fixture(4 * KIB);
    let result = extract_article_bounded(&url(), &html, Some(Instant::now()), &|| false);
    assert!(
        matches!(result, Err(NeuralError::ReaderDeadline)),
        "{result:?}"
    );
}

#[test]
fn deadline_is_checked_after_parsing() {
    // Um parse de 2 MiB demora sempre mais de 1 ms: o prazo expira durante
    // ele e a verificacao seguinte tem de o apanhar.
    let html = benign_fixture(2 * MIB);
    let deadline = Instant::now() + Duration::from_millis(1);
    let result = extract_article_bounded(&url(), &html, Some(deadline), &|| false);
    assert!(
        matches!(result, Err(NeuralError::ReaderDeadline)),
        "{result:?}"
    );
}

#[test]
fn cancellation_is_reported() {
    let html = benign_fixture(4 * KIB);
    let result = extract_article_bounded(&url(), &html, None, &|| true);
    assert!(
        matches!(result, Err(NeuralError::ReaderCancelled)),
        "{result:?}"
    );
}

#[test]
fn cancellation_is_checked_during_traversal() {
    // As duas primeiras consultas (antes e depois do parse) passam; a
    // terceira ja vem de dentro da travessia da arvore.
    let html = benign_fixture(256 * KIB);
    let calls = Cell::new(0usize);
    let cancelled = || {
        calls.set(calls.get() + 1);
        calls.get() > 2
    };
    let result = extract_article_bounded(&url(), &html, None, &cancelled);
    assert!(
        matches!(result, Err(NeuralError::ReaderCancelled)),
        "{result:?}"
    );
    assert_eq!(calls.get(), 3);
}

#[test]
fn unbounded_wrapper_matches_bounded_result() {
    let html = benign_fixture(64 * KIB);
    let plain = extract_article(&url(), &html).expect("sem limites");
    let bounded = extract_article_bounded(&url(), &html, None, &|| false).expect("com limites");
    assert_eq!(plain, bounded);
}

#[test]
#[ignore = "medicao: cadeia unica aninhada, dominada pelo in_scope do html5ever"]
fn fully_nested_measurement() {
    for size in [256 * KIB, 2 * MIB] {
        let html = fully_nested_hostile_fixture(size);
        let (result, _) = timed("hostil totalmente aninhado", &html);
        assert!(result.is_ok(), "{result:?}");
    }
}
