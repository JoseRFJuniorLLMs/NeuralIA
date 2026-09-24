//! Sumário: `<nav epub:type="toc">` do EPUB 3 ou `navMap` do NCX do EPUB 2.
//!
//! Os dois formatos são lidos para uma lista plana em pré-ordem com a
//! profundidade de cada entrada, e só depois vira árvore, sem recursão e com a
//! profundidade cortada em [`MAX_TOC_DEPTH`] (níveis mais fundos sobem para o
//! último nível permitido, mantendo a ordem). Os `href` são relativos ao
//! documento do sumário, não ao OPF, e o fragmento (`#sec2`) é preservado.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{
    EpubArchive, EpubError, EpubResult,
    book::{ManifestItem, SpineItem, load_dom, shown, warn},
    xml::{Dom, ROOT},
};

/// Níveis da árvore do sumário.
pub const MAX_TOC_DEPTH: usize = 32;
/// Entradas do sumário; o resto é ignorado (com aviso).
pub const MAX_TOC_ENTRIES: usize = 10_000;
const MAX_LABEL_CHARS: usize = 512;
/// Um `href` do sumário maior do que isto não aponta para nada que exista
/// (os nomes do ZIP têm no máximo 1 KiB): é ignorado, não copiado.
const MAX_HREF_BYTES: usize = 4 * 1024;
const NCX_MEDIA_TYPE: &str = "application/x-dtbncx+xml";

/// Uma entrada do sumário.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TocEntry {
    pub label: String,
    /// Entrada do ZIP para onde aponta (se existe).
    pub path: Option<String>,
    /// Âncora dentro do documento, sem `#`.
    pub fragment: Option<String>,
    /// Posição de `path` no spine, para a UI marcar o capítulo atual.
    pub spine_index: Option<usize>,
    pub children: Vec<TocEntry>,
}

struct FlatEntry {
    depth: usize,
    label: String,
    href: Option<String>,
}

/// Nav do EPUB 3 primeiro; se não houver, estiver vazio ou mal formado, o
/// NCX. Um documento de sumário com DOCTYPE perigoso derruba o livro todo.
pub(crate) fn read_toc(
    archive: &EpubArchive,
    manifest: &[ManifestItem],
    ncx_id: Option<&str>,
    spine: &[SpineItem],
    warnings: &mut Vec<String>,
) -> EpubResult<Vec<TocEntry>> {
    let nav = manifest
        .iter()
        .find(|item| item.has_property("nav") && item.path.is_some());
    if let Some(path) = nav.and_then(|item| item.path.as_deref()) {
        match load_dom(archive, path) {
            Ok(dom) => {
                let flat = nav_entries(&dom);
                if !flat.is_empty() {
                    return Ok(finish(flat, archive, path, spine, warnings));
                }
                warn(warnings, || format!("nav sem sumário: {}", shown(path)));
            }
            Err(error @ EpubError::UnsafeXml { .. }) => return Err(error),
            Err(error) => warn(warnings, || format!("nav ilegível: {error}")),
        }
    }
    let ncx = ncx_id
        .and_then(|id| {
            manifest
                .iter()
                .find(|item| item.id == id && item.path.is_some())
        })
        .or_else(|| {
            manifest
                .iter()
                .find(|item| item.media_type == NCX_MEDIA_TYPE && item.path.is_some())
        });
    if let Some(path) = ncx.and_then(|item| item.path.as_deref()) {
        match load_dom(archive, path) {
            Ok(dom) => {
                let flat = ncx_entries(&dom);
                if !flat.is_empty() {
                    return Ok(finish(flat, archive, path, spine, warnings));
                }
                warn(warnings, || format!("NCX sem navPoint: {}", shown(path)));
            }
            Err(error @ EpubError::UnsafeXml { .. }) => return Err(error),
            Err(error) => warn(warnings, || format!("NCX ilegível: {error}")),
        }
    }
    Ok(Vec::new())
}

/// Entradas do `<nav epub:type="toc">`: cada `<li>` com `<a href>` (ou
/// `<span>` para títulos de seção sem link) e um `<ol>` aninhado opcional.
fn nav_entries(dom: &Dom) -> Vec<FlatEntry> {
    let Some(nav) = dom.descendants(ROOT).into_iter().find(|&node| {
        dom.is(node, "nav")
            && dom
                .prefixed_attr(node, "type")
                .is_some_and(|kinds| kinds.split_whitespace().any(|kind| kind == "toc"))
    }) else {
        return Vec::new();
    };
    let Some(list) = dom.find(nav, "ol") else {
        return Vec::new();
    };
    walk(dom, list, "li", "ol", |dom, item| {
        let link = dom
            .elements(item)
            .find(|&node| dom.is(node, "a") || dom.is(node, "span"));
        match link {
            Some(node) if dom.is(node, "a") => (
                dom.text_capped(node, MAX_LABEL_CHARS),
                dom.attr(node, "href")
                    .filter(|href| href.len() <= MAX_HREF_BYTES)
                    .map(|href| href.trim().to_string()),
            ),
            Some(node) => (dom.text_capped(node, MAX_LABEL_CHARS), None),
            None => (dom.own_text(item), None),
        }
    })
}

/// Entradas do `navMap`: `navPoint` → `navLabel/text` e `content@src`.
fn ncx_entries(dom: &Dom) -> Vec<FlatEntry> {
    let Some(map) = dom.find(ROOT, "navMap") else {
        return Vec::new();
    };
    walk(dom, map, "navPoint", "", |dom, point| {
        let label = dom
            .child(point, "navLabel")
            .map(|label| {
                dom.child(label, "text").map_or_else(
                    || dom.text_capped(label, MAX_LABEL_CHARS),
                    |text| dom.text_capped(text, MAX_LABEL_CHARS),
                )
            })
            .unwrap_or_default();
        let href = dom
            .child(point, "content")
            .and_then(|content| dom.attr(content, "src"))
            .filter(|src| src.len() <= MAX_HREF_BYTES)
            .map(|src| src.trim().to_string());
        (label, href)
    })
}

/// Pré-ordem iterativa: `item` são os elementos que viram entradas; os filhos
/// de uma entrada estão num elemento `nested` dentro dela (nav: `<ol>`) ou
/// diretamente nela (NCX: `nested` vazio).
fn walk(
    dom: &Dom,
    list: usize,
    item: &str,
    nested: &str,
    describe: impl Fn(&Dom, usize) -> (String, Option<String>),
) -> Vec<FlatEntry> {
    let items_of = |node: usize| -> Vec<usize> {
        dom.elements(node)
            .filter(|&child| dom.is(child, item))
            .collect()
    };
    let mut flat = Vec::new();
    let mut stack: Vec<(Vec<usize>, usize, usize)> = vec![(items_of(list), 0, 0)];
    while let Some((items, next, depth)) = stack.last_mut() {
        let Some(&node) = items.get(*next) else {
            stack.pop();
            continue;
        };
        *next += 1;
        let depth = *depth;
        if flat.len() >= MAX_TOC_ENTRIES {
            break;
        }
        let (label, href) = describe(dom, node);
        flat.push(FlatEntry {
            depth,
            label: clip(label),
            href: href.filter(|href| !href.is_empty()),
        });
        let children = if nested.is_empty() {
            items_of(node)
        } else {
            dom.elements(node)
                .find(|&child| dom.is(child, nested))
                .map(items_of)
                .unwrap_or_default()
        };
        if !children.is_empty() {
            stack.push((children, 0, depth + 1));
        }
    }
    flat
}

fn clip(label: String) -> String {
    match label.char_indices().nth(MAX_LABEL_CHARS) {
        Some((cut, _)) => label[..cut].to_string(),
        None => label,
    }
}

fn finish(
    flat: Vec<FlatEntry>,
    archive: &EpubArchive,
    toc_path: &str,
    spine: &[SpineItem],
    warnings: &mut Vec<String>,
) -> Vec<TocEntry> {
    if flat.len() >= MAX_TOC_ENTRIES {
        warn(warnings, || {
            format!("sumário cortado em {MAX_TOC_ENTRIES} entradas")
        });
    }
    let mut spine_index: HashMap<&str, usize> = HashMap::new();
    for (index, item) in spine.iter().enumerate() {
        spine_index.entry(item.path.as_str()).or_insert(index);
    }
    let entries = flat.into_iter().map(|entry| {
        let target = entry
            .href
            .as_deref()
            .and_then(|href| archive.locate(toc_path, href));
        if target.is_none()
            && let Some(href) = &entry.href
        {
            warn(warnings, || {
                format!("entrada do sumário aponta para o vazio: {}", shown(href))
            });
        }
        let (path, fragment) = match target {
            Some((path, fragment)) => (Some(path), fragment),
            None => (None, None),
        };
        let toc = TocEntry {
            label: entry.label,
            spine_index: path
                .as_deref()
                .and_then(|path| spine_index.get(path).copied()),
            path,
            fragment,
            children: Vec::new(),
        };
        (entry.depth, toc)
    });
    build_tree(entries)
}

/// Lista plana em pré-ordem → árvore, sem recursão, com a profundidade
/// cortada em [`MAX_TOC_DEPTH`].
pub(crate) fn build_tree(entries: impl IntoIterator<Item = (usize, TocEntry)>) -> Vec<TocEntry> {
    fn attach(open: &mut [TocEntry], roots: &mut Vec<TocEntry>, done: TocEntry) {
        match open.last_mut() {
            Some(parent) => parent.children.push(done),
            None => roots.push(done),
        }
    }
    let mut roots = Vec::new();
    let mut open: Vec<TocEntry> = Vec::new();
    for (depth, entry) in entries {
        let depth = depth.min(MAX_TOC_DEPTH - 1);
        while open.len() > depth {
            if let Some(done) = open.pop() {
                attach(&mut open, &mut roots, done);
            }
        }
        open.push(entry);
    }
    while let Some(done) = open.pop() {
        attach(&mut open, &mut roots, done);
    }
    roots
}
