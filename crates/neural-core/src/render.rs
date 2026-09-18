use crate::reader::{ReaderArticle, ReaderBlock};

const LOGO:&str=r#"<svg viewBox="0 0 64 64" aria-hidden="true"><rect width="64" height="64" rx="12" fill="#111314"/><g fill="#fff" transform="translate(32 32)"><ellipse rx="6" ry="16" transform="translate(0 -10) rotate(45)"/><ellipse rx="6" ry="16" transform="translate(10 0) rotate(135)"/><ellipse rx="6" ry="16" transform="translate(0 10) rotate(45)"/><ellipse rx="6" ry="16" transform="translate(-10 0) rotate(135)"/><circle r="4"/></g></svg>"#;
const BASE_CSS:&str=r#":root{font-family:Inter,Segoe UI,system-ui,sans-serif;color:#17191b;background:#f8f9fa;color-scheme:light}*{box-sizing:border-box}body{margin:0}code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace}"#;

pub fn home_html()->String{
    format!(r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>NeuralIA</title>
<style>{BASE_CSS}.shell{{min-height:100vh;display:grid;place-items:center;padding:32px}}.card{{width:min(760px,92vw);text-align:center}}.logo{{width:112px;height:112px;margin:0 auto 26px}}h1{{font-size:42px;margin:0 0 8px}}.tag{{color:#72767d;margin-bottom:34px}}.row{{display:flex;gap:10px}}input{{flex:1;background:#fff;border:1px solid #d9dce1;border-radius:16px;padding:17px 18px;font-size:17px;outline:none}}input:focus{{border-color:#111314;box-shadow:0 0 0 3px rgba(17,19,20,.08)}}button{{border:0;background:#111314;color:#fff;border-radius:16px;padding:0 22px;font-weight:650;cursor:pointer}}.modes{{display:flex;justify-content:center;gap:10px;margin-top:14px}}.modes button{{background:#eceef1;color:#17191b;padding:11px 16px}}.hint{{font-size:13px;color:#858991;margin-top:22px;line-height:1.6}}</style></head>
<body><main id="neural-shell" class="shell"><section class="card"><div class="logo">{LOGO}</div><h1>NeuralIA</h1><div class="tag">Pergunte. Leia. Continue.</div>
<div class="row"><input id="q" autofocus placeholder="Pergunte algo ou cole uma URL"><button onclick="go()">Ir</button></div>
<div class="modes"><button onclick="mode('ask')">IA</button><button onclick="mode('read')">Reader</button><button onclick="mode('web')">Web</button></div>
<div class="hint">Texto → Google AI · URL → Reader · <code>web:https://...</code> → página completa<br>NeuralIA não inclui Chromium nem um modelo local.</div></section></main>
<script>const q=document.getElementById('q');function send(action,value=''){{window.ipc.postMessage(JSON.stringify({{action,value}}));}}function go(){{send('go',q.value)}}function mode(m){{send(m,q.value)}}q.addEventListener('keydown',e=>{{if(e.key==='Enter')go();}});</script></body></html>"#)
}

pub fn reader_html(article:&ReaderArticle)->String{
    let mut body=String::new();
    for block in &article.blocks{
        match block{
            ReaderBlock::Heading{level,text}=>{
                let level=(*level).clamp(2,6);
                body.push_str(&format!("<h{level}>{}</h{level}>",escape_html(text)));
            }
            ReaderBlock::Paragraph(text)=>body.push_str(&format!("<p>{}</p>",escape_html(text))),
            ReaderBlock::Quote(text)=>body.push_str(&format!("<blockquote>{}</blockquote>",escape_html(text))),
            ReaderBlock::Code(text)=>body.push_str(&format!("<pre><code>{}</code></pre>",escape_html(text))),
            ReaderBlock::ListItem(text)=>body.push_str(&format!("<div class=\"li\">• {}</div>",escape_html(text))),
        }
    }
    let byline=article.byline.as_ref().map(|v|format!("<span>{}</span>",escape_html(v))).unwrap_or_default();
    let excerpt=article.excerpt.as_ref().map(|v|format!("<p class=\"excerpt\">{}</p>",escape_html(v))).unwrap_or_default();
    format!(r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title>
<style>{BASE_CSS}.reader{{width:min(820px,90vw);margin:0 auto;padding:34px 0 100px}}.top{{display:flex;align-items:center;gap:12px;margin-bottom:44px}}.top svg{{width:38px;height:38px}}.top button{{border:1px solid #ddd;background:#fff;border-radius:12px;padding:9px 13px;cursor:pointer}}h1{{font-size:42px;line-height:1.08;margin:0 0 12px}}h2{{margin-top:42px}}h3{{margin-top:34px}}.meta{{display:flex;gap:12px;color:#777;font-size:14px;margin-bottom:22px}}.excerpt{{font-size:19px;color:#555}}article p,article .li,blockquote{{font-family:Georgia,serif;font-size:20px;line-height:1.75}}blockquote{{border-left:4px solid #111314;margin-left:0;padding-left:22px;color:#45484d}}pre{{overflow:auto;background:#111314;color:#f5f5f5;padding:18px;border-radius:16px;font-size:14px;line-height:1.6}}.source{{color:#5a5f67;overflow-wrap:anywhere}}</style></head>
<body><main id="neural-shell" class="reader"><div class="top">{LOGO}<button onclick="send('home')">Início</button><button onclick="send('web','{source_js}')">Abrir página completa</button></div>
<header><h1>{title}</h1><div class="meta">{byline}<span class="source">{source}</span></div>{excerpt}</header><article>{body}</article></main>
<script>function send(action,value=''){{window.ipc.postMessage(JSON.stringify({{action,value}}));}}</script></body></html>"#,
        title=escape_html(&article.title),source=escape_html(&article.source_url),source_js=escape_js_single(&article.source_url))
}

pub fn escape_html(input:&str)->String{
    input.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;").replace('\'',"&#39;")
}
fn escape_js_single(input:&str)->String{
    input.replace('\\',"\\\\").replace('\'',"\\'").replace('\n',"\\n").replace('\r',"")
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test] fn escapes(){ assert_eq!(escape_html("<script>"),"&lt;script&gt;"); }
}
