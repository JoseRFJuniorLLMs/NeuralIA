use std::time::Duration;

use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{security::validate_web_url, NeuralError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReaderArticle {
    pub source_url: String,
    pub title: String,
    pub byline: Option<String>,
    pub excerpt: Option<String>,
    pub blocks: Vec<ReaderBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReaderBlock {
    Heading { level: u8, text: String },
    Paragraph(String),
    Quote(String),
    Code(String),
    ListItem(String),
}

#[derive(Clone)]
pub struct ReaderClient {
    agent: ureq::Agent,
    max_bytes: usize,
}

impl ReaderClient {
    pub fn new(timeout_secs:u64,max_bytes:usize)->Self{
        let config=ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(timeout_secs)))
            .max_redirects(5)
            .max_redirects_will_error(true)
            .build();
        Self{agent:ureq::Agent::new_with_config(config),max_bytes}
    }

    pub fn fetch(&self,input:&str)->Result<ReaderArticle>{
        let url=validate_web_url(input)?;
        let mut response=self.agent.get(url.as_str())
            .header("Accept","text/html,application/xhtml+xml;q=0.9,text/plain;q=0.5,*/*;q=0.1")
            .header("User-Agent","NeuralIA/0.1 (+https://github.com/JoseRFJuniorLLMs/NeuralIA)")
            .call()?;

        let content_type=response.headers().get("content-type")
            .and_then(|v|v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
        if !content_type.is_empty()
            && !content_type.contains("text/html")
            && !content_type.contains("application/xhtml+xml"){
            return Err(NeuralError::UnsupportedContentType(content_type));
        }

        let html=response.body_mut().with_config()
            .limit(self.max_bytes as u64)
            .lossy_utf8(true)
            .read_to_string()?;
        extract_article(&url,&html)
    }
}

impl Default for ReaderClient {
    fn default()->Self{ Self::new(12,2*1024*1024) }
}

pub fn extract_article(url:&Url,html:&str)->Result<ReaderArticle>{
    let document=Html::parse_document(html);
    let title=meta_content(&document,"meta[property='og:title']")
        .or_else(||text_of_first(&document,"title"))
        .or_else(||text_of_first(&document,"h1"))
        .unwrap_or_else(||url.host_str().unwrap_or("Untitled").to_string());
    let byline=meta_content(&document,"meta[name='author']");
    let excerpt=meta_content(&document,"meta[name='description']")
        .or_else(||meta_content(&document,"meta[property='og:description']"));

    let candidate_selector=Selector::parse(
        "article,main,[role='main'],.article,.post,.entry-content,.post-content,.article-body,.story-body,.content"
    ).expect("static selector");
    let link_selector=Selector::parse("a").expect("static selector");
    let root=document.select(&candidate_selector)
        .max_by_key(|candidate|score_candidate(candidate,&link_selector))
        .unwrap_or_else(||document.root_element());

    let block_selector=Selector::parse("h1,h2,h3,h4,h5,h6,p,blockquote,pre,li").expect("static selector");
    let mut blocks=Vec::new();
    let mut previous=String::new();

    for node in root.select(&block_selector){
        if inside_ignored_container(&node){ continue; }
        let text=normalize_text(node.text().collect::<Vec<_>>().join(" "));
        if text.len()<2 || text==previous { continue; }
        let tag=node.value().name();
        let block=match tag{
            "h1"=>ReaderBlock::Heading{level:1,text:text.clone()},
            "h2"=>ReaderBlock::Heading{level:2,text:text.clone()},
            "h3"=>ReaderBlock::Heading{level:3,text:text.clone()},
            "h4"=>ReaderBlock::Heading{level:4,text:text.clone()},
            "h5"=>ReaderBlock::Heading{level:5,text:text.clone()},
            "h6"=>ReaderBlock::Heading{level:6,text:text.clone()},
            "blockquote"=>ReaderBlock::Quote(text.clone()),
            "pre"=>ReaderBlock::Code(text.clone()),
            "li"=>ReaderBlock::ListItem(text.clone()),
            _=>ReaderBlock::Paragraph(text.clone()),
        };
        previous=text;
        blocks.push(block);
        if blocks.len()>=600{ break; }
    }

    if blocks.is_empty(){
        let fallback=normalize_text(root.text().collect::<Vec<_>>().join(" "));
        if fallback.len()<40{ return Err(NeuralError::ReaderExtraction); }
        blocks.push(ReaderBlock::Paragraph(fallback));
    }

    Ok(ReaderArticle{
        source_url:url.to_string(),
        title:normalize_text(title),
        byline:byline.map(normalize_text).filter(|s|!s.is_empty()),
        excerpt:excerpt.map(normalize_text).filter(|s|!s.is_empty()),
        blocks,
    })
}

fn score_candidate(candidate:&ElementRef<'_>,link_selector:&Selector)->usize{
    let total=normalize_text(candidate.text().collect::<Vec<_>>().join(" ")).len();
    let link_text:usize=candidate.select(link_selector)
        .map(|link|normalize_text(link.text().collect::<Vec<_>>().join(" ")).len()).sum();
    total.saturating_sub(link_text.saturating_mul(2))
}

fn inside_ignored_container(node:&ElementRef<'_>)->bool{
    node.ancestors().filter_map(ElementRef::wrap).any(|ancestor|
        matches!(ancestor.value().name(),"nav"|"footer"|"aside"|"script"|"style"|"form")
    )
}

fn meta_content(document:&Html,selector:&str)->Option<String>{
    let selector=Selector::parse(selector).ok()?;
    document.select(&selector).next()
        .and_then(|element|element.value().attr("content"))
        .map(ToString::to_string)
}
fn text_of_first(document:&Html,selector:&str)->Option<String>{
    let selector=Selector::parse(selector).ok()?;
    document.select(&selector).next()
        .map(|node|normalize_text(node.text().collect::<Vec<_>>().join(" ")))
        .filter(|text|!text.is_empty())
}
fn normalize_text(input:String)->String{
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn extracts_article(){
        let html=r#"<html><head><title>Teste</title><meta name="author" content="Eva"></head>
        <body><nav>menu</nav><article><h1>Um título</h1>
        <p>Este é um parágrafo suficientemente útil.</p>
        <blockquote>Uma citação.</blockquote><pre>let x = 1;</pre></article></body></html>"#;
        let url=Url::parse("https://example.com/a").unwrap();
        let article=extract_article(&url,html).unwrap();
        assert_eq!(article.title,"Teste");
        assert_eq!(article.byline.as_deref(),Some("Eva"));
        assert!(article.blocks.len()>=4);
    }
}
