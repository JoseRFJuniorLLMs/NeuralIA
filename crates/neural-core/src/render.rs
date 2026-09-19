use std::borrow::Cow;

use url::Url;

use crate::reader::{ReaderArticle, ReaderBlock};

const LOGO: &str = include_str!("../../../assets/neuralia-logo.svg");
const BASE_CSS: &str = r#":root{font-family:Inter,Segoe UI,system-ui,sans-serif;color:#17191b;background:#f8f9fa;color-scheme:light}*{box-sizing:border-box}body{margin:0}code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace}"#;
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'none'; img-src data:; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'; connect-src 'none'";

pub fn reader_html(article: &ReaderArticle) -> String {
    let mut body = String::new();
    let mut list_open = false;

    for block in &article.blocks {
        match block {
            ReaderBlock::ListItem(text) => {
                if !list_open {
                    body.push_str("<ul>");
                    list_open = true;
                }
                // `push_str` directo: o `format!` por bloco era mais uma
                // String alocada e copiada so para ser logo concatenada.
                body.push_str("<li>");
                body.push_str(&escape_html(text));
                body.push_str("</li>");
            }
            other => {
                if list_open {
                    body.push_str("</ul>");
                    list_open = false;
                }

                match other {
                    ReaderBlock::Heading { level, text } => {
                        let level = (*level).clamp(2, 6);
                        // Preso a 2..=6, logo o nivel e sempre um unico digito
                        // e dispensa o `format!`.
                        let digit = char::from(b'0' + level);
                        body.push_str("<h");
                        body.push(digit);
                        body.push('>');
                        body.push_str(&escape_html(text));
                        body.push_str("</h");
                        body.push(digit);
                        body.push('>');
                    }
                    ReaderBlock::Paragraph(text) => {
                        body.push_str("<p>");
                        body.push_str(&escape_html(text));
                        body.push_str("</p>");
                    }
                    ReaderBlock::Quote(text) => {
                        body.push_str("<blockquote>");
                        body.push_str(&escape_html(text));
                        body.push_str("</blockquote>");
                    }
                    ReaderBlock::Code(text) => {
                        body.push_str("<pre><code>");
                        body.push_str(&escape_html(text));
                        body.push_str("</code></pre>");
                    }
                    ReaderBlock::ListItem(_) => unreachable!(),
                }
            }
        }
    }

    if list_open {
        body.push_str("</ul>");
    }

    let byline = article
        .byline
        .as_ref()
        .map(|value| format!("<span>{}</span>", escape_html(value)))
        .unwrap_or_default();
    let excerpt = article
        .excerpt
        .as_ref()
        .map(|value| format!("<p class=\"excerpt\">{}</p>", escape_html(value)))
        .unwrap_or_default();
    let source = escape_html(&article.source_url);
    let full_url = reader_action_url("web", Some(&article.source_url));
    let home_url = reader_action_url("home", None);

    format!(
        r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{CSP}"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title>
<style>{BASE_CSS}.reader{{width:min(820px,90vw);margin:0 auto;padding:34px 0 100px}}.top{{display:flex;align-items:center;gap:12px;margin-bottom:44px}}.top svg{{width:38px;height:38px}}.action{{display:inline-block;text-decoration:none;border:1px solid #ddd;background:#fff;color:#17191b;border-radius:12px;padding:9px 13px}}h1{{font-size:42px;line-height:1.08;margin:0 0 12px}}h2{{margin-top:42px}}h3{{margin-top:34px}}.meta{{display:flex;gap:12px;color:#777;font-size:14px;margin-bottom:22px}}.excerpt{{font-size:19px;color:#555}}article p,article li,blockquote{{font-family:Georgia,serif;font-size:20px;line-height:1.75}}article ul{{padding-left:1.4em}}blockquote{{border-left:4px solid #111314;margin-left:0;padding-left:22px;color:#45484d}}pre{{overflow:auto;white-space:pre;background:#111314;color:#f5f5f5;padding:18px;border-radius:16px;font-size:14px;line-height:1.6}}.source{{color:#5a5f67;overflow-wrap:anywhere}}</style></head>
<body><main id="neural-shell" class="reader"><div class="top">{LOGO}<a class="action" href="{home_url}">Início</a><a class="action" href="{full_url}">Abrir página completa</a></div>
<header><h1>{title}</h1><div class="meta">{byline}<span class="source">{source}</span></div>{excerpt}</header><article>{body}</article></main></body></html>"#,
        title = escape_html(&article.title),
        home_url = escape_html(&home_url),
        full_url = escape_html(&full_url),
    )
}

fn reader_action_url(action: &str, value: Option<&str>) -> String {
    let mut url = Url::parse(&format!("neuralia:{action}")).expect("static reader action URL");
    if let Some(value) = value {
        url.query_pairs_mut().append_pair("url", value);
    }
    url.to_string()
}

/// Uma passagem unica em vez de cinco `replace` encadeados. Cada `replace`
/// alocava e copiava a string inteira: cinco copias por bloco, e o reader
/// chega a centenas de blocos de dezenas de milhares de chars. O caso comum e
/// nao haver nada a escapar: ai devolve-se o proprio input emprestado, sem
/// alocar nada.
///
/// O resultado e identico ao dos cinco `replace`: como o `&` era substituido
/// primeiro, os `&` que as proprias entidades introduzem nunca eram reescritos
/// pelas passagens seguintes, que e o que esta passagem unica faz por
/// construcao.
pub fn escape_html(input: &str) -> Cow<'_, str> {
    let Some(first) = input.find(['&', '<', '>', '"', '\'']) else {
        return Cow::Borrowed(input);
    };

    // O prefixo limpo copia-se de uma vez; a folga cobre as primeiras
    // entidades sem obrigar a realocar logo na primeira.
    let mut escaped = String::with_capacity(input.len() + 16);
    escaped.push_str(&input[..first]);
    for ch in input[first..].chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A versao anterior, palavra por palavra, para servir de referencia: a
    /// passagem unica so vale se der exactamente o mesmo resultado.
    fn escape_html_five_replaces(input: &str) -> String {
        input
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }

    #[test]
    fn escapes() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
    }

    #[test]
    fn escape_html_matches_the_five_replace_version() {
        let cases = [
            "",
            "texto simples",
            "<script>",
            "a & b",
            "\"aspas\"",
            "'apostrofo'",
            "&amp;",
            "&lt;ja escapado&gt;",
            "&<>\"'",
            "'\"><&",
            "acentuacao e emojis: ação,日本語, 🙂",
            "<a href=\"x\" onclick='alert(1)'>ação & cia</a>",
            "&&&&&",
            "fim com &",
            "& no inicio",
        ];

        for case in cases {
            assert_eq!(
                escape_html(case).as_ref(),
                escape_html_five_replaces(case),
                "divergiu em {case:?}"
            );
        }
    }

    #[test]
    fn escape_html_matches_on_large_mixed_input() {
        let big = "ação & <b>negrito</b> 'x' \"y\" > z\n".repeat(2_000);
        assert_eq!(escape_html(&big).as_ref(), escape_html_five_replaces(&big));
    }

    #[test]
    fn escape_html_borrows_when_there_is_nothing_to_escape() {
        assert!(matches!(
            escape_html("texto sem nada a escapar, com acentuação"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(escape_html(""), Cow::Borrowed(_)));
        assert!(matches!(escape_html("um < aqui"), Cow::Owned(_)));
    }

    #[test]
    fn escape_html_keeps_the_clean_prefix_intact() {
        // O prefixo antes do primeiro char perigoso e copiado em bloco: se o
        // offset estivesse errado, aqui perdia-se ou duplicava-se texto.
        let input = "prefixo longo e sem nada de especial <fim>";
        assert_eq!(
            escape_html(input).as_ref(),
            "prefixo longo e sem nada de especial &lt;fim&gt;"
        );
    }

    #[test]
    fn reader_uses_navigation_actions_not_javascript() {
        let article = ReaderArticle {
            source_url: "https://example.com/?x='</script><script>alert(1)</script>".into(),
            title: "Title".into(),
            byline: None,
            excerpt: None,
            blocks: vec![ReaderBlock::Paragraph("Body".into())],
        };
        let html = reader_html(&article);
        assert!(html.contains("neuralia:web?"));
        assert!(!html.contains("window.ipc"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn reader_renders_semantic_lists() {
        let article = ReaderArticle {
            source_url: "https://example.com/".into(),
            title: "Title".into(),
            byline: None,
            excerpt: None,
            blocks: vec![
                ReaderBlock::ListItem("A".into()),
                ReaderBlock::ListItem("B".into()),
                ReaderBlock::Paragraph("After".into()),
            ],
        };
        let html = reader_html(&article);
        assert!(html.contains("<ul><li>A</li><li>B</li></ul><p>After</p>"));
    }

    #[test]
    fn reader_has_restrictive_csp() {
        let article = ReaderArticle {
            source_url: "https://example.com/".into(),
            title: "Title".into(),
            byline: None,
            excerpt: None,
            blocks: vec![ReaderBlock::Paragraph("Body".into())],
        };
        let html = reader_html(&article);
        assert!(html.contains("default-src 'none'"));
        assert!(html.contains("script-src 'none'"));
        assert!(html.contains("connect-src 'none'"));
    }
}
