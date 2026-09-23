//! Estrutura de um EPUB: container → OPF (metadados, manifest, spine), capa,
//! sumário e DRM.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{
    EpubArchive, EpubError, EpubResult, LimitKind,
    archive::resolve_href,
    toc::{self, TocEntry},
    xml::{self, Dom, ROOT, XmlFault},
};

/// Algoritmo de ofuscação de fontes do IDPF (EPUB 3, OCF §4).
pub const IDPF_FONT_OBFUSCATION: &str = "http://www.idpf.org/2008/embedding";
/// Algoritmo de ofuscação de fontes da Adobe.
pub const ADOBE_FONT_OBFUSCATION: &str = "http://ns.adobe.com/pdf/enc#RC";

const CONTAINER_PATH: &str = "META-INF/container.xml";
const ENCRYPTION_PATH: &str = "META-INF/encryption.xml";
const RIGHTS_PATH: &str = "META-INF/rights.xml";
const MIMETYPE_PATH: &str = "mimetype";
const EPUB_MIMETYPE: &str = "application/epub+zip";
const OPF_MEDIA_TYPE: &str = "application/oebps-package+xml";
/// Uma página de capa (XHTML) maior do que isto não é aberta para procurar a imagem.
const MAX_COVER_PAGE_BYTES: u64 = 1024 * 1024;
const MAX_FIELD_CHARS: usize = 1024;
const MAX_DESCRIPTION_CHARS: usize = 64 * 1024;
const FONT_EXTENSIONS: [&str; 5] = ["ttf", "otf", "woff", "woff2", "ttc"];
const IMAGE_EXTENSIONS: [(&str, &str); 7] = [
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("svg", "image/svg+xml"),
    ("bmp", "image/bmp"),
];

/// Autor, tradutor, ilustrador...: `role` é o código MARC (`aut`, `trl`, `ill`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creator {
    pub name: String,
    pub role: Option<String>,
    /// Forma de ordenação ("Assis, Machado de").
    pub file_as: Option<String>,
}

/// Metadados Dublin Core do OPF, já com os refinamentos do EPUB 3 aplicados.
/// Os textos vêm do livro: são dados, nunca HTML para injetar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EpubMetadata {
    pub title: Option<String>,
    pub creators: Vec<Creator>,
    pub contributors: Vec<Creator>,
    pub language: Option<String>,
    /// O identificador apontado por `unique-identifier` (ou o primeiro).
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub date: Option<String>,
    pub subjects: Vec<String>,
    /// `calibre:series` ou `belongs-to-collection` do EPUB 3.
    pub series: Option<String>,
    pub series_index: Option<f64>,
}

impl EpubMetadata {
    /// Nomes para exibir como autores: os criadores com papel `aut` ou sem
    /// papel; se nenhum servir, todos os criadores.
    pub fn authors(&self) -> Vec<String> {
        let authors: Vec<String> = self
            .creators
            .iter()
            .filter(|creator| {
                creator
                    .role
                    .as_deref()
                    .is_none_or(|role| role.eq_ignore_ascii_case("aut"))
            })
            .map(|creator| creator.name.clone())
            .collect();
        if authors.is_empty() {
            self.creators
                .iter()
                .map(|creator| creator.name.clone())
                .collect()
        } else {
            authors
        }
    }
}

/// Item do manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestItem {
    pub id: String,
    /// `href` tal como está no OPF.
    pub href: String,
    /// Nome da entrada no ZIP, se o `href` resolve para uma que existe.
    pub path: Option<String>,
    pub media_type: String,
    pub properties: Vec<String>,
    pub fallback: Option<String>,
}

impl ManifestItem {
    pub fn has_property(&self, property: &str) -> bool {
        self.properties.iter().any(|value| value == property)
    }
}

/// Item do spine (ordem de leitura). Só entram itens que existem no ZIP.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpineItem {
    pub idref: String,
    pub path: String,
    pub media_type: String,
    /// `linear="no"` → `false` (notas, extras fora do fluxo principal).
    pub linear: bool,
    pub properties: Vec<String>,
}

/// `page-progression-direction` do spine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageProgression {
    #[default]
    Default,
    Ltr,
    Rtl,
}

/// Imagem de capa encontrada no livro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverImage {
    /// Nome da entrada no ZIP.
    pub path: String,
    pub media_type: String,
    pub manifest_id: Option<String>,
}

/// Um EPUB lido. Construído por [`EpubBook::parse`]; os bytes continuam no
/// [`EpubArchive`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpubBook {
    /// `version` do `<package>` ("2.0", "3.0"...).
    pub version: String,
    pub opf_path: String,
    pub metadata: EpubMetadata,
    pub manifest: Vec<ManifestItem>,
    pub spine: Vec<SpineItem>,
    pub page_progression: PageProgression,
    pub toc: Vec<TocEntry>,
    pub cover: Option<CoverImage>,
    /// Fontes com ofuscação IDPF/Adobe (não é DRM). São servidas como estão:
    /// o NeuralIA não as desofusca, e o leitor cai na fonte padrão.
    pub obfuscated_fonts: Vec<String>,
    /// Problemas tolerados (sem `mimetype`, `href` que não existe...).
    pub warnings: Vec<String>,
}

impl EpubBook {
    /// Lê a estrutura do livro. Erros: [`EpubError::Drm`],
    /// [`EpubError::UnsafeXml`], [`EpubError::NotEpub`] (sem OPF ou sem nada
    /// legível no spine) e os de leitura do ZIP.
    pub fn parse(archive: &EpubArchive) -> EpubResult<Self> {
        let mut warnings = Vec::new();
        check_mimetype(archive, &mut warnings);
        if let Some(rights) = archive.entry(RIGHTS_PATH) {
            return Err(EpubError::Drm(rights.name().to_string()));
        }
        let opf_path = find_opf(archive, &mut warnings)?;
        let dom = load_dom(archive, &opf_path)?;
        let package = dom
            .find(ROOT, "package")
            .ok_or_else(|| EpubError::NotEpub(format!("{opf_path} não tem <package>")))?;
        let version = dom
            .attr(package, "version")
            .unwrap_or_default()
            .trim()
            .to_string();
        let metadata = parse_metadata(&dom, package);
        let manifest = parse_manifest(archive, &dom, package, &opf_path, &mut warnings);
        let obfuscated_fonts = check_encryption(archive, &manifest)?;
        let (spine, page_progression, toc_id) =
            parse_spine(&dom, package, &manifest, &mut warnings);
        if spine.is_empty() {
            return Err(EpubError::NotEpub(
                "o spine não tem nenhum documento que exista no arquivo".into(),
            ));
        }
        let toc = toc::read_toc(archive, &manifest, toc_id.as_deref(), &spine, &mut warnings)?;
        let cover = find_cover(archive, &dom, package, &opf_path, &manifest, &spine);
        Ok(Self {
            version,
            opf_path,
            metadata,
            manifest,
            spine,
            page_progression,
            toc,
            cover,
            obfuscated_fonts,
            warnings,
        })
    }

    pub fn manifest_item(&self, id: &str) -> Option<&ManifestItem> {
        self.manifest.iter().find(|item| item.id == id)
    }

    /// Item do manifest cujo `path` é esta entrada.
    pub fn item_for_path(&self, path: &str) -> Option<&ManifestItem> {
        self.manifest
            .iter()
            .find(|item| item.path.as_deref() == Some(path))
    }

    /// Primeira posição do spine que mostra esta entrada.
    pub fn spine_index_of(&self, path: &str) -> Option<usize> {
        self.spine.iter().position(|item| item.path == path)
    }
}

/// Lê, descodifica e monta o DOM de um documento do pacote, traduzindo as
/// falhas do XML para [`EpubError`].
pub(crate) fn load_dom(archive: &EpubArchive, path: &str) -> EpubResult<Dom> {
    let bytes = archive
        .read_capped(path, xml::MAX_XML_BYTES)
        .map_err(|error| match error {
            EpubError::Limit {
                kind: LimitKind::EntrySize,
                value,
                limit,
            } => EpubError::Limit {
                kind: LimitKind::XmlSize,
                value,
                limit,
            },
            other => other,
        })?;
    xml::parse(&xml::decode_document(&bytes)).map_err(|fault| match fault {
        XmlFault::Unsafe(reason) => EpubError::UnsafeXml {
            path: path.to_string(),
            reason,
        },
        XmlFault::Syntax(message) => EpubError::Xml {
            path: path.to_string(),
            message,
        },
        XmlFault::Limit(kind, value, limit) => EpubError::Limit { kind, value, limit },
    })
}

/// O `mimetype` deveria ser a primeira entrada e conter `application/epub+zip`;
/// muitos EPUBs reais erram isso, por isso é só aviso.
fn check_mimetype(archive: &EpubArchive, warnings: &mut Vec<String>) {
    let Some(entry) = archive.entry(MIMETYPE_PATH) else {
        warnings.push("sem o arquivo mimetype".into());
        return;
    };
    if archive.entries().first().map(|first| first.name()) != Some(entry.name()) {
        warnings.push("mimetype não é a primeira entrada do ZIP".into());
    }
    match archive.read_capped(MIMETYPE_PATH, 1024) {
        Ok(bytes) => {
            let value = String::from_utf8_lossy(&bytes);
            if value.trim() != EPUB_MIMETYPE {
                warnings.push(format!("mimetype inesperado: {:?}", value.trim()));
            }
        }
        Err(error) => warnings.push(format!("mimetype ilegível: {error}")),
    }
}

/// `META-INF/container.xml` → caminho do OPF. Sem container (ou com um que
/// aponta para o vazio), o primeiro `.opf` do ZIP.
fn find_opf(archive: &EpubArchive, warnings: &mut Vec<String>) -> EpubResult<String> {
    if archive.contains(CONTAINER_PATH) {
        let dom = load_dom(archive, CONTAINER_PATH)?;
        let rootfiles: Vec<usize> = dom
            .descendants(ROOT)
            .into_iter()
            .filter(|&node| dom.is(node, "rootfile"))
            .collect();
        let preferred = rootfiles
            .iter()
            .copied()
            .find(|&node| {
                dom.attr(node, "media-type")
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case(OPF_MEDIA_TYPE))
            })
            .or_else(|| rootfiles.first().copied());
        match preferred.and_then(|node| dom.attr(node, "full-path")) {
            Some(full_path) => match archive.locate("", full_path) {
                Some((path, _)) => return Ok(path),
                None => warnings.push(format!(
                    "container.xml aponta para {:?}, que não existe",
                    full_path.trim()
                )),
            },
            None => warnings.push("container.xml sem rootfile".into()),
        }
    } else {
        warnings.push("sem META-INF/container.xml".into());
    }
    archive
        .entries()
        .iter()
        .map(|entry| entry.name())
        .find(|name| name.to_ascii_lowercase().ends_with(".opf"))
        .map(str::to_string)
        .ok_or_else(|| EpubError::NotEpub("sem META-INF/container.xml e sem arquivo .opf".into()))
}

fn clip(text: String, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => text[..cut].to_string(),
        None => text,
    }
}

fn field(dom: &Dom, node: usize) -> Option<String> {
    let text = clip(dom.text(node), MAX_FIELD_CHARS);
    (!text.is_empty()).then_some(text)
}

fn parse_metadata(dom: &Dom, package: usize) -> EpubMetadata {
    let mut metadata = EpubMetadata::default();
    let Some(section) = dom.child(package, "metadata") else {
        return metadata;
    };
    // OPF 2 às vezes embrulha tudo em <dc-metadata>/<x-metadata>: descendentes.
    let nodes = dom.descendants(section);

    // Refinamentos do EPUB 3: <meta refines="#id" property="role">aut</meta>.
    let mut refines: HashMap<String, Vec<(String, String)>> = HashMap::new();
    // Pares do EPUB 2: <meta name="cover" content="..."/>. Vale o primeiro.
    let mut named: HashMap<String, String> = HashMap::new();
    let mut collections: Vec<(Option<String>, String)> = Vec::new();
    for &node in &nodes {
        if !dom.is(node, "meta") {
            continue;
        }
        if let (Some(target), Some(property)) =
            (dom.attr(node, "refines"), dom.attr(node, "property"))
        {
            let target = target.trim().trim_start_matches('#').to_string();
            refines
                .entry(target)
                .or_default()
                .push((property.trim().to_string(), dom.text(node)));
        } else if dom.attr(node, "property").map(str::trim) == Some("belongs-to-collection") {
            let id = dom.attr(node, "id").map(|id| id.trim().to_string());
            collections.push((id, dom.text(node)));
        } else if let (Some(name), Some(content)) =
            (dom.attr(node, "name"), dom.attr(node, "content"))
        {
            named
                .entry(name.trim().to_ascii_lowercase())
                .or_insert_with(|| content.trim().to_string());
        }
    }
    let refined = |id: Option<&str>, property: &str| -> Option<String> {
        let values = refines.get(id?.trim())?;
        values
            .iter()
            .find(|(key, value)| key == property && !value.is_empty())
            .map(|(_, value)| value.clone())
    };
    let creator = |node: usize| -> Option<Creator> {
        let name = field(dom, node)?;
        let id = dom.attr(node, "id");
        let role = dom
            .attr(node, "role")
            .map(|role| role.trim().to_string())
            .filter(|role| !role.is_empty())
            .or_else(|| refined(id, "role"));
        let file_as = dom
            .attr(node, "file-as")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| refined(id, "file-as"));
        Some(Creator {
            name,
            role,
            file_as,
        })
    };

    let unique_identifier = dom.attr(package, "unique-identifier").map(str::trim);
    let mut titles: Vec<(Option<&str>, String)> = Vec::new();
    let mut identifiers: Vec<(Option<&str>, String)> = Vec::new();
    let mut dates: Vec<(bool, String)> = Vec::new();
    for &node in &nodes {
        match dom.local(node).to_ascii_lowercase().as_str() {
            "title" => {
                if let Some(title) = field(dom, node) {
                    titles.push((dom.attr(node, "id"), title));
                }
            }
            "creator" => metadata.creators.extend(creator(node)),
            "contributor" => metadata.contributors.extend(creator(node)),
            "language" => {
                if metadata.language.is_none() {
                    metadata.language = field(dom, node);
                }
            }
            "identifier" => {
                if let Some(value) = field(dom, node) {
                    identifiers.push((dom.attr(node, "id"), value));
                }
            }
            "publisher" => {
                if metadata.publisher.is_none() {
                    metadata.publisher = field(dom, node);
                }
            }
            "description" => {
                if metadata.description.is_none() {
                    let text = clip(dom.text(node), MAX_DESCRIPTION_CHARS);
                    metadata.description = (!text.is_empty()).then_some(text);
                }
            }
            "date" => {
                if let Some(value) = field(dom, node) {
                    let publication = dom
                        .attr(node, "event")
                        .is_some_and(|event| event.trim().eq_ignore_ascii_case("publication"));
                    dates.push((publication, value));
                }
            }
            "subject" => metadata.subjects.extend(field(dom, node)),
            _ => {}
        }
    }
    metadata.title = titles
        .iter()
        .find(|(id, _)| refined(*id, "title-type").as_deref() == Some("main"))
        .or_else(|| titles.first())
        .map(|(_, title)| title.clone());
    metadata.identifier = identifiers
        .iter()
        .find(|(id, _)| id.is_some_and(|id| Some(id.trim()) == unique_identifier))
        .or_else(|| identifiers.first())
        .map(|(_, value)| value.clone());
    metadata.date = dates
        .iter()
        .find(|(publication, _)| *publication)
        .or_else(|| dates.first())
        .map(|(_, value)| value.clone());

    // Série: primeiro a do Calibre, depois a coleção do EPUB 3.
    if let Some(series) = named
        .get("calibre:series")
        .filter(|value| !value.is_empty())
    {
        metadata.series = Some(clip(series.clone(), MAX_FIELD_CHARS));
        metadata.series_index = named
            .get("calibre:series_index")
            .and_then(|value| parse_index(value));
    } else if let Some((id, name)) = collections
        .iter()
        .filter(|(_, name)| !name.is_empty())
        .find(|(id, _)| {
            refined(id.as_deref(), "collection-type").is_none_or(|kind| kind == "series")
        })
    {
        metadata.series = Some(clip(name.clone(), MAX_FIELD_CHARS));
        metadata.series_index =
            refined(id.as_deref(), "group-position").and_then(|value| parse_index(&value));
    }
    metadata
}

fn parse_index(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|index| index.is_finite())
}

fn parse_manifest(
    archive: &EpubArchive,
    dom: &Dom,
    package: usize,
    opf_path: &str,
    warnings: &mut Vec<String>,
) -> Vec<ManifestItem> {
    let Some(section) = dom.child(package, "manifest") else {
        warnings.push("OPF sem <manifest>".into());
        return Vec::new();
    };
    let mut items: Vec<ManifestItem> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for node in dom.descendants(section) {
        if !dom.is(node, "item") {
            continue;
        }
        let id = dom.attr(node, "id").unwrap_or_default().trim().to_string();
        let href = dom
            .attr(node, "href")
            .unwrap_or_default()
            .trim()
            .to_string();
        if id.is_empty() || href.is_empty() {
            warnings.push(format!("item do manifest sem id ou href: {id:?} {href:?}"));
            continue;
        }
        if seen.contains_key(&id) {
            warnings.push(format!("id repetido no manifest (vale o primeiro): {id}"));
            continue;
        }
        let path = archive.locate(opf_path, &href).map(|(path, _)| path);
        if path.is_none() && resolve_href(opf_path, &href).is_some() {
            warnings.push(format!("item do manifest não existe no arquivo: {href}"));
        }
        seen.insert(id.clone(), items.len());
        items.push(ManifestItem {
            id,
            href,
            path,
            media_type: dom
                .attr(node, "media-type")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase(),
            properties: tokens(dom.attr(node, "properties")),
            fallback: dom
                .attr(node, "fallback")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        });
    }
    items
}

fn tokens(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn parse_spine(
    dom: &Dom,
    package: usize,
    manifest: &[ManifestItem],
    warnings: &mut Vec<String>,
) -> (Vec<SpineItem>, PageProgression, Option<String>) {
    let Some(section) = dom.child(package, "spine") else {
        warnings.push("OPF sem <spine>".into());
        return (Vec::new(), PageProgression::Default, None);
    };
    let progression = match dom
        .attr(section, "page-progression-direction")
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("rtl") => PageProgression::Rtl,
        Some("ltr") => PageProgression::Ltr,
        _ => PageProgression::Default,
    };
    let toc_id = dom
        .attr(section, "toc")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let by_id: HashMap<&str, &ManifestItem> = manifest
        .iter()
        .rev()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let mut spine = Vec::new();
    for node in dom.elements(section) {
        if !dom.is(node, "itemref") {
            continue;
        }
        let idref = dom.attr(node, "idref").unwrap_or_default().trim();
        let Some(item) = by_id.get(idref) else {
            warnings.push(format!("itemref sem item no manifest: {idref:?}"));
            continue;
        };
        let Some(path) = &item.path else {
            warnings.push(format!("itemref para arquivo ausente: {}", item.href));
            continue;
        };
        spine.push(SpineItem {
            idref: idref.to_string(),
            path: path.clone(),
            media_type: item.media_type.clone(),
            linear: dom
                .attr(node, "linear")
                .is_none_or(|value| !value.trim().eq_ignore_ascii_case("no")),
            properties: tokens(dom.attr(node, "properties")),
        });
    }
    (spine, progression, toc_id)
}

/// `rights.xml` já foi tratado; aqui, `encryption.xml`. Ofuscação IDPF/Adobe
/// aplicada a uma fonte não é DRM; qualquer outra coisa cifrada é.
fn check_encryption(archive: &EpubArchive, manifest: &[ManifestItem]) -> EpubResult<Vec<String>> {
    if !archive.contains(ENCRYPTION_PATH) {
        return Ok(Vec::new());
    }
    let dom = load_dom(archive, ENCRYPTION_PATH)?;
    let mut fonts = Vec::new();
    for data in dom.descendants(ROOT) {
        if !dom.is(data, "EncryptedData") {
            continue;
        }
        let inside = dom.descendants(data);
        let algorithm = inside
            .iter()
            .find(|&&node| dom.is(node, "EncryptionMethod"))
            .and_then(|&node| dom.attr(node, "Algorithm"))
            .unwrap_or_default()
            .trim();
        let uri = inside
            .iter()
            .find(|&&node| dom.is(node, "CipherReference"))
            .and_then(|&node| dom.attr(node, "URI"))
            .unwrap_or_default()
            .trim();
        // As URIs do encryption.xml são relativas à raiz do contêiner.
        let target = archive
            .locate("", uri)
            .map(|(path, _)| path)
            .or_else(|| resolve_href("", uri).map(|(path, _)| path))
            .unwrap_or_else(|| uri.to_string());
        let obfuscation = algorithm == IDPF_FONT_OBFUSCATION || algorithm == ADOBE_FONT_OBFUSCATION;
        if obfuscation && is_font(&target, manifest) {
            fonts.push(target);
        } else {
            return Err(EpubError::Drm(format!(
                "{target} cifrado com {algorithm:?}"
            )));
        }
    }
    Ok(fonts)
}

fn is_font(path: &str, manifest: &[ManifestItem]) -> bool {
    if let Some(item) = manifest
        .iter()
        .find(|item| item.path.as_deref() == Some(path))
    {
        let media_type = item.media_type.as_str();
        if media_type.starts_with("font/")
            || media_type.contains("font")
            || media_type == "application/vnd.ms-opentype"
        {
            return true;
        }
    }
    extension(path).is_some_and(|ext| FONT_EXTENSIONS.contains(&ext.as_str()))
}

fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (_, ext) = name.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Tipo de imagem pelo manifest ou, na falta, pela extensão.
fn image_media_type(path: &str, manifest: &[ManifestItem]) -> Option<String> {
    let declared = manifest
        .iter()
        .find(|item| item.path.as_deref() == Some(path))
        .map(|item| item.media_type.clone())
        .filter(|media_type| !media_type.is_empty());
    match declared {
        Some(media_type) => media_type.starts_with("image/").then_some(media_type),
        None => {
            let ext = extension(path)?;
            IMAGE_EXTENSIONS
                .iter()
                .find(|(known, _)| *known == ext)
                .map(|(_, media_type)| media_type.to_string())
        }
    }
}

fn cover_from_item(item: &ManifestItem) -> Option<CoverImage> {
    if !item.media_type.starts_with("image/") {
        return None;
    }
    Some(CoverImage {
        path: item.path.clone()?,
        media_type: item.media_type.clone(),
        manifest_id: Some(item.id.clone()),
    })
}

fn cover_from_path(path: &str, manifest: &[ManifestItem]) -> Option<CoverImage> {
    let media_type = image_media_type(path, manifest)?;
    Some(CoverImage {
        path: path.to_string(),
        media_type,
        manifest_id: manifest
            .iter()
            .find(|item| item.path.as_deref() == Some(path))
            .map(|item| item.id.clone()),
    })
}

/// Um caminho que pode ser a imagem ou uma página (XHTML/SVG) que a mostra.
fn cover_from_target(
    archive: &EpubArchive,
    path: &str,
    manifest: &[ManifestItem],
) -> Option<CoverImage> {
    cover_from_path(path, manifest).or_else(|| image_in_page(archive, path, manifest))
}

/// Primeira `<img src>` ou `<image xlink:href>` de uma página pequena.
fn image_in_page(
    archive: &EpubArchive,
    page: &str,
    manifest: &[ManifestItem],
) -> Option<CoverImage> {
    let bytes = archive.read_capped(page, MAX_COVER_PAGE_BYTES).ok()?;
    let dom = xml::parse(&xml::decode_document(&bytes)).ok()?;
    dom.descendants(ROOT).into_iter().find_map(|node| {
        let href = if dom.is(node, "img") {
            dom.attr(node, "src")
        } else if dom.is(node, "image") {
            dom.attr(node, "href")
        } else {
            None
        }?;
        let (path, _) = archive.locate(page, href)?;
        cover_from_path(&path, manifest)
    })
}

/// Capa, pela ordem: `properties="cover-image"` (EPUB 3); `<meta name="cover">`
/// (EPUB 2, pelo id, pelo id sem caixa ou por um caminho); `<guide>` com
/// `type="cover"`; imagem do manifest com "cover" no id ou no nome; primeira
/// imagem do primeiro documento do spine.
fn find_cover(
    archive: &EpubArchive,
    dom: &Dom,
    package: usize,
    opf_path: &str,
    manifest: &[ManifestItem],
    spine: &[SpineItem],
) -> Option<CoverImage> {
    if let Some(cover) = manifest
        .iter()
        .filter(|item| item.has_property("cover-image"))
        .find_map(cover_from_item)
    {
        return Some(cover);
    }

    let metadata = dom.child(package, "metadata");
    let meta_cover = metadata.and_then(|section| {
        dom.descendants(section).into_iter().find_map(|node| {
            (dom.is(node, "meta")
                && dom
                    .attr(node, "name")
                    .is_some_and(|name| name.trim().eq_ignore_ascii_case("cover")))
            .then(|| dom.attr(node, "content"))
            .flatten()
            .map(str::trim)
        })
    });
    if let Some(content) = meta_cover.filter(|content| !content.is_empty()) {
        let item = manifest.iter().find(|item| item.id == content).or_else(|| {
            manifest
                .iter()
                .find(|item| item.id.eq_ignore_ascii_case(content))
        });
        let found = match item {
            Some(item) => item
                .path
                .as_deref()
                .and_then(|path| cover_from_target(archive, path, manifest)),
            None => archive
                .locate(opf_path, content)
                .and_then(|(path, _)| cover_from_target(archive, &path, manifest)),
        };
        if found.is_some() {
            return found;
        }
    }

    if let Some(guide) = dom.child(package, "guide") {
        let reference = dom.elements(guide).find(|&node| {
            dom.is(node, "reference")
                && dom
                    .attr(node, "type")
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("cover"))
        });
        if let Some(found) = reference
            .and_then(|node| dom.attr(node, "href"))
            .and_then(|href| archive.locate(opf_path, href))
            .and_then(|(path, _)| cover_from_target(archive, &path, manifest))
        {
            return Some(found);
        }
    }

    if let Some(cover) = manifest
        .iter()
        .filter(|item| {
            item.id.to_ascii_lowercase().contains("cover")
                || item
                    .path
                    .as_deref()
                    .is_some_and(|path| file_name(path).to_ascii_lowercase().contains("cover"))
        })
        .find_map(cover_from_item)
    {
        return Some(cover);
    }

    spine
        .first()
        .and_then(|first| image_in_page(archive, &first.path, manifest))
}
