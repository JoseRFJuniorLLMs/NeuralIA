//! Um bloco do Reader deixa de receber texto quando o seu elemento fecha.
//!
//! O binário embarca sem `debug-assertions`, por isso estes testes também têm
//! de correr com `cargo test --release -p neural-core --test reader_block_scope`:
//! um efeito colateral escondido num `debug_assert!` passa em debug e duplica
//! texto no produto.

use neural_core::reader::{ReaderBlock, extract_article};
use url::Url;

fn blocks(html: &str) -> Vec<ReaderBlock> {
    let url = Url::parse("https://example.com/a").expect("url");
    extract_article(&url, html).expect("extrai").blocks
}

#[test]
fn image_only_paragraph_does_not_swallow_the_rest_of_the_article() {
    let got = blocks(
        "<html><body><article><p><img src=a.jpg></p><h2>Secao</h2>\
         <p>Primeiro paragrafo real e longo.</p>\
         <p>Segundo paragrafo real e longo.</p></article></body></html>",
    );
    assert_eq!(
        got,
        vec![
            ReaderBlock::Heading {
                level: 2,
                text: "Secao".into()
            },
            ReaderBlock::Paragraph("Primeiro paragrafo real e longo.".into()),
            ReaderBlock::Paragraph("Segundo paragrafo real e longo.".into()),
        ]
    );
}

#[test]
fn text_after_a_nested_list_belongs_to_the_parent_item() {
    let got = blocks("<html><body><ul><li>Pai<ul><li>Filho</li></ul>resto</li></ul></body></html>");
    assert_eq!(
        got,
        vec![
            ReaderBlock::ListItem("Pai resto".into()),
            ReaderBlock::ListItem("Filho".into()),
        ]
    );
}
