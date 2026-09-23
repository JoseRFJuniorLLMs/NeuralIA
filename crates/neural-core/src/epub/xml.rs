//! XML do EPUB (container, OPF, NCX, nav, encryption) num DOM mínimo.
//!
//! O DOM é uma arena (`Vec` de nós com índices): construir, percorrer e
//! descartar não recorre, por isso um documento fundo não estoura a pilha. Os
//! limites são [`MAX_XML_BYTES`], [`MAX_XML_NODES`] e [`MAX_XML_DEPTH`]
//! (elementos mais fundos do que isso são ignorados; o texto deles vai para o
//! último elemento guardado).
//!
//! Entidades: só as cinco do XML, referências numéricas e uma tabela curta de
//! nomes do HTML (`&nbsp;`, `&mdash;`...). Nada é expandido a partir do
//! documento: um `<!DOCTYPE>` com `<!ENTITY` (bomba de entidades, XXE) é
//! recusado, e uma referência desconhecida fica no texto tal como veio.
//! Nenhum DTD externo é lido.

use quick_xml::{
    XmlVersion,
    events::{BytesRef, BytesStart, Event},
    reader::Reader,
};

use super::LimitKind;

/// Tamanho máximo de um documento XML do pacote.
pub const MAX_XML_BYTES: u64 = 16 * 1024 * 1024;
/// Elementos por documento.
pub const MAX_XML_NODES: usize = 200_000;
/// Profundidade de elementos guardada.
pub const MAX_XML_DEPTH: usize = 256;

/// Índice do nó-documento (pai do elemento raiz).
pub(crate) const ROOT: usize = 0;

#[derive(Debug)]
pub(crate) enum XmlFault {
    /// DOCTYPE que declara entidades.
    Unsafe(String),
    Syntax(String),
    Limit(LimitKind, u64, u64),
}

#[derive(Debug)]
enum Child {
    Element(usize),
    Text(String),
}

#[derive(Debug)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Child>,
}

#[derive(Debug)]
pub(crate) struct Dom {
    nodes: Vec<Node>,
}

/// Bytes de um documento XML → texto. BOM UTF-8/UTF-16 e UTF-16 sem BOM são
/// reconhecidos; bytes que não são UTF-8 são lidos como Latin-1 (OPFs antigos
/// declaram `iso-8859-1`), para nunca perder o documento inteiro por um acento.
pub(crate) fn decode_document(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return utf8_or_latin1(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, u16::from_be_bytes);
    }
    if bytes.starts_with(&[b'<', 0, b'?', 0]) {
        return utf16(bytes, u16::from_le_bytes);
    }
    if bytes.starts_with(&[0, b'<', 0, b'?']) {
        return utf16(bytes, u16::from_be_bytes);
    }
    utf8_or_latin1(bytes)
}

fn utf16(bytes: &[u8], word: fn([u8; 2]) -> u16) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|pair| word(*pair));
    char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn utf8_or_latin1(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&byte| char::from(byte)).collect(),
    }
}

/// Monta o DOM. Tolerante com fechos trocados ou a mais e com `&` solto;
/// estrito com DOCTYPE que declara entidades e com os limites.
pub(crate) fn parse(text: &str) -> Result<Dom, XmlFault> {
    let mut reader = Reader::from_str(text);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.allow_unmatched_ends = true;
    config.allow_dangling_amp = true;

    let mut dom = Dom {
        nodes: vec![Node {
            name: String::new(),
            attrs: Vec::new(),
            children: Vec::new(),
        }],
    };
    let mut stack = vec![ROOT];
    // Elementos abertos além de MAX_XML_DEPTH: contados, não guardados.
    let mut ignored_open = 0usize;
    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(error) => {
                return Err(XmlFault::Syntax(format!(
                    "{error} (byte {})",
                    reader.error_position()
                )));
            }
        };
        let top = stack.last().copied().unwrap_or(ROOT);
        match event {
            Event::Start(start) => {
                if ignored_open > 0 || stack.len() > MAX_XML_DEPTH {
                    ignored_open += 1;
                } else {
                    let index = dom.push_element(top, &start)?;
                    stack.push(index);
                }
            }
            Event::Empty(start) => {
                if ignored_open == 0 && stack.len() <= MAX_XML_DEPTH {
                    dom.push_element(top, &start)?;
                }
            }
            Event::End(_) => {
                if ignored_open > 0 {
                    ignored_open -= 1;
                } else if stack.len() > 1 {
                    stack.pop();
                }
            }
            Event::Text(text) => {
                let text = text
                    .xml10_content()
                    .map_err(|error| XmlFault::Syntax(error.to_string()))?;
                dom.push_text(top, &text);
            }
            Event::CData(data) => {
                let text = data
                    .decode()
                    .map_err(|error| XmlFault::Syntax(error.to_string()))?;
                dom.push_text(top, &text);
            }
            Event::GeneralRef(reference) => dom.push_text(top, &resolve_reference(&reference)),
            Event::DocType(doctype) => {
                let doctype = doctype
                    .decode()
                    .map_err(|error| XmlFault::Syntax(error.to_string()))?;
                if declares_entities(&doctype) {
                    return Err(XmlFault::Unsafe(
                        "o DOCTYPE declara entidades (possível bomba de entidades ou XXE)".into(),
                    ));
                }
            }
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::Eof => break,
        }
    }
    Ok(dom)
}

fn declares_entities(doctype: &str) -> bool {
    doctype.to_ascii_uppercase().contains("<!ENTITY")
}

fn resolve_reference(reference: &BytesRef<'_>) -> String {
    let Ok(name) = reference.decode() else {
        return String::new();
    };
    if reference.is_char_ref() {
        return match reference.resolve_char_ref() {
            Ok(Some(ch)) => ch.to_string(),
            _ => char::REPLACEMENT_CHARACTER.to_string(),
        };
    }
    match resolve_entity(&name) {
        Some(value) => value.to_string(),
        // Desconhecida: fica literal. Nunca se expande nada vindo do documento.
        None => format!("&{name};"),
    }
}

/// As cinco entidades do XML e os nomes do HTML que aparecem de fato em
/// títulos e sumários.
fn resolve_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "apos" => "'",
        "quot" => "\"",
        "nbsp" => "\u{a0}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "thinsp" => "\u{2009}",
        "shy" => "\u{ad}",
        "ndash" => "\u{2013}",
        "mdash" => "\u{2014}",
        "hellip" => "\u{2026}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "sbquo" => "\u{201a}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "bdquo" => "\u{201e}",
        "laquo" => "\u{ab}",
        "raquo" => "\u{bb}",
        "bull" => "\u{2022}",
        "middot" => "\u{b7}",
        "copy" => "\u{a9}",
        "reg" => "\u{ae}",
        "trade" => "\u{2122}",
        "deg" => "\u{b0}",
        "sect" => "\u{a7}",
        "para" => "\u{b6}",
        "times" => "\u{d7}",
        "divide" => "\u{f7}",
        "iexcl" => "\u{a1}",
        "iquest" => "\u{bf}",
        "ordf" => "\u{aa}",
        "ordm" => "\u{ba}",
        "Aacute" => "\u{c1}",
        "Agrave" => "\u{c0}",
        "Acirc" => "\u{c2}",
        "Atilde" => "\u{c3}",
        "Auml" => "\u{c4}",
        "Ccedil" => "\u{c7}",
        "Eacute" => "\u{c9}",
        "Egrave" => "\u{c8}",
        "Ecirc" => "\u{ca}",
        "Iacute" => "\u{cd}",
        "Ntilde" => "\u{d1}",
        "Oacute" => "\u{d3}",
        "Ocirc" => "\u{d4}",
        "Otilde" => "\u{d5}",
        "Ouml" => "\u{d6}",
        "Uacute" => "\u{da}",
        "Uuml" => "\u{dc}",
        "aacute" => "\u{e1}",
        "agrave" => "\u{e0}",
        "acirc" => "\u{e2}",
        "atilde" => "\u{e3}",
        "auml" => "\u{e4}",
        "ccedil" => "\u{e7}",
        "eacute" => "\u{e9}",
        "egrave" => "\u{e8}",
        "ecirc" => "\u{ea}",
        "iacute" => "\u{ed}",
        "ntilde" => "\u{f1}",
        "oacute" => "\u{f3}",
        "ocirc" => "\u{f4}",
        "otilde" => "\u{f5}",
        "ouml" => "\u{f6}",
        "uacute" => "\u{fa}",
        "uuml" => "\u{fc}",
        "szlig" => "\u{df}",
        _ => return None,
    })
}

impl Dom {
    fn push_element(&mut self, parent: usize, start: &BytesStart<'_>) -> Result<usize, XmlFault> {
        // O nó 0 é o documento: contam só os elementos.
        let elements = self.nodes.len() - 1;
        if elements >= MAX_XML_NODES {
            return Err(XmlFault::Limit(
                LimitKind::XmlNodes,
                elements as u64 + 1,
                MAX_XML_NODES as u64,
            ));
        }
        let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
        let mut attrs = Vec::new();
        for attr in start.attributes().with_checks(false) {
            let Ok(attr) = attr else { continue };
            let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
            let value = match attr.normalized_value_with(XmlVersion::Implicit1_0, 1, resolve_entity)
            {
                Ok(value) => value.into_owned(),
                // Entidade desconhecida no atributo: o valor cru, sem expandir.
                Err(_) => String::from_utf8_lossy(&attr.value).into_owned(),
            };
            attrs.push((key, value));
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            name,
            attrs,
            children: Vec::new(),
        });
        self.nodes[parent].children.push(Child::Element(index));
        Ok(index)
    }

    fn push_text(&mut self, parent: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        let children = &mut self.nodes[parent].children;
        if let Some(Child::Text(last)) = children.last_mut() {
            last.push_str(text);
        } else {
            children.push(Child::Text(text.to_string()));
        }
    }

    /// Nome sem prefixo (`dc:title` → `title`).
    pub(crate) fn local(&self, node: usize) -> &str {
        local_part(&self.nodes[node].name)
    }

    /// O nome local é `local`, sem distinguir maiúsculas (há OPFs com `<Package>`).
    pub(crate) fn is(&self, node: usize, local: &str) -> bool {
        self.local(node).eq_ignore_ascii_case(local)
    }

    /// Valor do atributo `name` (nome qualificado exato ou, na falta, o
    /// primeiro com o mesmo nome local: `opf:role` responde a `role`).
    /// Declarações `xmlns` nunca contam.
    pub(crate) fn attr(&self, node: usize, name: &str) -> Option<&str> {
        let attrs = &self.nodes[node].attrs;
        attrs
            .iter()
            .find(|(key, _)| key == name)
            .or_else(|| {
                attrs
                    .iter()
                    .find(|(key, _)| !is_xmlns(key) && local_part(key) == name)
            })
            .map(|(_, value)| value.as_str())
    }

    /// Atributo com prefixo e o nome local dado (`epub:type`), para não
    /// confundir com um `type` sem prefixo.
    pub(crate) fn prefixed_attr(&self, node: usize, local: &str) -> Option<&str> {
        self.nodes[node]
            .attrs
            .iter()
            .find(|(key, _)| key.contains(':') && !is_xmlns(key) && local_part(key) == local)
            .map(|(_, value)| value.as_str())
    }

    /// Filhos que são elementos, em ordem.
    pub(crate) fn elements(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes[node]
            .children
            .iter()
            .filter_map(|child| match child {
                Child::Element(index) => Some(*index),
                Child::Text(_) => None,
            })
    }

    pub(crate) fn child(&self, node: usize, local: &str) -> Option<usize> {
        self.elements(node).find(|&child| self.is(child, local))
    }

    /// Descendentes em pré-ordem (ordem do documento), sem recursão.
    pub(crate) fn descendants(&self, node: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = self.elements(node).collect();
        stack.reverse();
        while let Some(next) = stack.pop() {
            out.push(next);
            let first = stack.len();
            stack.extend(self.elements(next));
            stack[first..].reverse();
        }
        out
    }

    /// Primeiro descendente com o nome local dado.
    pub(crate) fn find(&self, node: usize, local: &str) -> Option<usize> {
        self.descendants(node)
            .into_iter()
            .find(|&index| self.is(index, local))
    }

    /// Todo o texto dentro do nó, com espaços colapsados.
    pub(crate) fn text(&self, node: usize) -> String {
        let mut raw = String::new();
        let mut stack: Vec<&Child> = self.nodes[node].children.iter().rev().collect();
        while let Some(child) = stack.pop() {
            match child {
                Child::Text(text) => raw.push_str(text),
                Child::Element(index) => stack.extend(self.nodes[*index].children.iter().rev()),
            }
        }
        collapse_whitespace(&raw)
    }

    /// Só o texto que é filho direto do nó (sem o dos elementos dentro dele).
    pub(crate) fn own_text(&self, node: usize) -> String {
        let raw: String = self.nodes[node]
            .children
            .iter()
            .filter_map(|child| match child {
                Child::Text(text) => Some(text.as_str()),
                Child::Element(_) => None,
            })
            .collect();
        collapse_whitespace(&raw)
    }
}

fn local_part(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, local)| local)
}

fn is_xmlns(key: &str) -> bool {
    key == "xmlns" || key.starts_with("xmlns:")
}

pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
