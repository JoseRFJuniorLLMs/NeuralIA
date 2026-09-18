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
                body.push_str(&format!("<li>{}</li>", escape_html(text)));
            }
            other => {
                if list_open {
                    body.push_str("</ul>");
                    list_open = false;
                }

                match other {
                    ReaderBlock::Heading { level, text } => {
                        let level = (*level).clamp(2, 6);
                        body.push_str(&format!("<h{level}>{}</h{level}>", escape_html(text)));
                    }
                    ReaderBlock::Paragraph(text) => {
                        body.push_str(&format!("<p>{}</p>", escape_html(text)));
                    }
                    ReaderBlock::Quote(text) => {
                        body.push_str(&format!("<blockquote>{}</blockquote>", escape_html(text)));
                    }
                    ReaderBlock::Code(text) => {
                        body.push_str(&format!("<pre><code>{}</code></pre>", escape_html(text)));
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

pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
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
