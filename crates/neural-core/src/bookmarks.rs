//! Favoritos (bookmarks, plano 2.3): a arvore, as operacoes, a loja e a
//! importacao/exportacao. Portavel: nada de Windows aqui, e nada de rede.
//!
//! **A arvore.** Uma lista plana de `BookmarkNode {id, parent, kind, title,
//! url, added_ms, order}`. A raiz e o id 1, «Favoritos», uma pasta sem pai
//! (`parent` 0). `BookmarkTree::validate` e a regra inteira: raiz unica,
//! ids unicos e abaixo de `next_id`, cada pai existe e e uma pasta, sem
//! ciclos, profundidade ate `MAX_DEPTH`, ate `MAX_NODES` nos, titulos ate
//! `MAX_TITLE_CHARS` sem caracteres de controlo, enderecos so http/https ate
//! `MAX_URL_BYTES`. O `Deserialize` passa por ela (`try_from`): um ficheiro
//! com um ciclo nao e uma arvore, e a loja trata-o como estragado (so
//! leitura, copia `.bak`, nunca reescrito).
//!
//! **As operacoes.** `BookmarkOp {AddLink, AddFolder, Rename, Move, Delete,
//! Import}` sobre uma copia: ou a operacao inteira vale e a arvore que sai
//! passa na regra, ou a arvore fica como estava. Um endereco repetido e o
//! mesmo favorito: a chave e `domains::canonical_url_key` (a mesma do
//! bloqueio de anuncios -- sem segunda copia).
//!
//! **A loja.** `BookmarkStore` e o `bookmarks.json` num
//! `VersionedJsonStore` partilhado entre janelas (trinco
//! `bookmarks.json.lock`): cada operacao rele o ficheiro DEBAIXO do trinco e
//! aplica-se ao que esta no disco agora, nunca a uma copia em memoria -- duas
//! janelas nunca perdem o favorito uma da outra. So abre com um grant
//! `StoreKind::Explicit` (o utilizador pediu para guardar): nao ha outra
//! maneira de a abrir sem o registo das lojas.
//!
//! **Importar.** `parse_chromium_bookmarks` le o ficheiro `Bookmarks` de um
//! perfil do Chrome/Edge (so leitura, ate `IMPORT_MAX_BYTES`; `date_added`
//! vem em microssegundos desde 1601 e passa a ms Unix) e `chromium_profiles`
//! le os perfis do `Local State`. Os unicos ficheiros de um perfil que este
//! modulo abre sao esses dois (`ChromiumFile`): nunca o historico, as senhas
//! nem os cookies. `parse_netscape_html` le o HTML exportado por qualquer
//! navegador (o formato «NETSCAPE-Bookmark-file-1»). **Exportar**:
//! `export_netscape_html`, com `& < > " '` escapados no texto e nos
//! atributos.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::domains::canonical_url_key;
use crate::json_store::{
    Degraded, LoadOutcome, SaveOutcome, StoreError, StoreGrant, StoreKind, VersionedJsonStore,
};

/// O id da raiz, «Favoritos».
pub const ROOT_ID: u64 = 1;
/// O nome da raiz.
pub const ROOT_TITLE: &str = "Favoritos";
/// O maior titulo, em caracteres.
pub const MAX_TITLE_CHARS: usize = 300;
/// O maior endereco, em bytes.
pub const MAX_URL_BYTES: usize = 8192;
/// A maior profundidade (a raiz esta a 0).
pub const MAX_DEPTH: usize = 16;
/// O maior numero de nos, a raiz incluida.
pub const MAX_NODES: usize = 20_000;
/// O maior ficheiro que a importacao le (Chrome, Edge ou HTML).
pub const IMPORT_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// O maior encaixe de pastas que a importacao segue; abaixo disso o
/// conteudo sobe para a ultima pasta aceite.
pub const IMPORT_MAX_NESTING: usize = 64;
/// Quantos itens uma importacao le no maximo (antes do tecto da arvore).
pub const IMPORT_MAX_ITEMS: usize = 100_000;
/// A versao do `bookmarks.json` que este codigo escreve.
pub const STORE_VERSION: u32 = 1;
/// O tecto do `bookmarks.json`.
pub const STORE_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// Microssegundos entre 1601-01-01 (a origem do Chrome) e 1970-01-01.
pub const CHROME_EPOCH_OFFSET_US: u64 = 11_644_473_600_000_000;

// ===================== a arvore =====================

/// Pasta ou favorito.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Folder,
    Link,
}

/// Um no da arvore. Numa pasta o `url` e vazio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkNode {
    pub id: u64,
    /// O pai; 0 so na raiz.
    pub parent: u64,
    pub kind: NodeKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Quando foi acrescentado, em ms desde 1970 (0: nao se sabe).
    #[serde(default)]
    pub added_ms: u64,
    /// A posicao entre os irmaos (0, 1, 2...).
    #[serde(default)]
    pub order: u32,
}

/// Porque uma arvore nao e valida.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    #[error("sem a raiz «Favoritos»")]
    NoRoot,
    #[error("{0} nos passam o tecto de {MAX_NODES}")]
    TooManyNodes(usize),
    #[error("um no com o id 0")]
    ZeroId,
    #[error("o id {0} repete-se")]
    DuplicateId(u64),
    #[error("o id {0} nao esta abaixo do proximo id")]
    IdAheadOfNext(u64),
    #[error("o no {0} tem um pai que nao existe ou nao e uma pasta")]
    BadParent(u64),
    #[error("o no {0} esta num ciclo")]
    Cycle(u64),
    #[error("o no {0} passa a profundidade de {MAX_DEPTH}")]
    TooDeep(u64),
    #[error("o titulo do no {0} passa o tecto ou tem caracteres de controlo")]
    BadTitle(u64),
    #[error("o endereco do no {0} nao e http(s) ou passa o tecto")]
    BadUrl(u64),
}

/// A arvore dos favoritos. So se constroi valida: `Default` e so a raiz, e
/// o `Deserialize` passa por `validate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawTree")]
pub struct BookmarkTree {
    next_id: u64,
    nodes: Vec<BookmarkNode>,
}

/// O que esta no ficheiro, antes da regra.
#[derive(Deserialize)]
struct RawTree {
    next_id: u64,
    nodes: Vec<BookmarkNode>,
}

impl TryFrom<RawTree> for BookmarkTree {
    type Error = TreeError;

    fn try_from(raw: RawTree) -> Result<Self, Self::Error> {
        let tree = BookmarkTree {
            next_id: raw.next_id,
            nodes: raw.nodes,
        };
        tree.validate()?;
        Ok(tree)
    }
}

impl Default for BookmarkTree {
    fn default() -> Self {
        Self {
            next_id: ROOT_ID + 1,
            nodes: vec![BookmarkNode {
                id: ROOT_ID,
                parent: 0,
                kind: NodeKind::Folder,
                title: ROOT_TITLE.to_string(),
                url: String::new(),
                added_ms: 0,
                order: 0,
            }],
        }
    }
}

/// Um titulo como a arvore o guarda: sem caracteres de controlo (viram
/// espaco), sem espacos a volta, ate `MAX_TITLE_CHARS`.
pub fn clean_title(raw: &str) -> String {
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced.trim().chars().take(MAX_TITLE_CHARS).collect()
}

fn title_is_clean(title: &str) -> bool {
    title.chars().count() <= MAX_TITLE_CHARS && !title.chars().any(char::is_control)
}

/// Um host das origens proprias do NeuralIA (`neuralia-pdf.localhost` e as
/// irmas): o WebView2 serve os esquemas proprios assim, em http.
fn is_neuralia_local_host(host: &str) -> bool {
    host.strip_suffix(".localhost")
        .is_some_and(|label| label.starts_with("neuralia-"))
}

/// Pode ser um favorito: http ou https, com host, ate `MAX_URL_BYTES`, e
/// nunca uma origem propria do NeuralIA. `about:blank`, `data:`, `file:`,
/// `javascript:` e `neuralia-pdf:` nao.
pub fn is_bookmarkable(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.as_str().len() <= MAX_URL_BYTES
        && url
            .host_str()
            .is_some_and(|host| !host.is_empty() && !is_neuralia_local_host(host))
}

/// O endereco de uma pagina como candidato a favorito, ou `None`.
pub fn bookmarkable_url(raw: &str) -> Option<Url> {
    Url::parse(raw.trim()).ok().filter(is_bookmarkable)
}

/// A chave que junta dois enderecos do mesmo favorito.
pub fn bookmark_key(url: &Url) -> String {
    canonical_url_key(url)
}

impl BookmarkTree {
    /// Todos os nos, pela ordem em que estao guardados.
    pub fn nodes(&self) -> &[BookmarkNode] {
        &self.nodes
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// So a raiz.
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub fn get(&self, id: u64) -> Option<&BookmarkNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Os filhos de `parent`, pela ordem.
    pub fn children(&self, parent: u64) -> Vec<&BookmarkNode> {
        let mut children: Vec<&BookmarkNode> = self
            .nodes
            .iter()
            .filter(|node| node.parent == parent && node.id != ROOT_ID)
            .collect();
        children.sort_by_key(|node| (node.order, node.id));
        children
    }

    /// A arvore em pre-ordem a partir da raiz (exclusive): cada no com a
    /// sua profundidade (1 para os filhos da raiz).
    pub fn walk(&self) -> Vec<(usize, &BookmarkNode)> {
        let mut by_parent: HashMap<u64, Vec<&BookmarkNode>> = HashMap::new();
        for node in &self.nodes {
            if node.id != ROOT_ID {
                by_parent.entry(node.parent).or_default().push(node);
            }
        }
        for children in by_parent.values_mut() {
            children.sort_by_key(|node| (node.order, node.id));
        }
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<(usize, &BookmarkNode)> = by_parent
            .get(&ROOT_ID)
            .map(|children| children.iter().rev().map(|node| (1, *node)).collect())
            .unwrap_or_default();
        while let Some((depth, node)) = stack.pop() {
            out.push((depth, node));
            if let Some(children) = by_parent.get(&node.id) {
                stack.extend(children.iter().rev().map(|child| (depth + 1, *child)));
            }
        }
        out
    }

    /// As pastas, em pre-ordem (a raiz primeiro), com a profundidade.
    pub fn folders(&self) -> Vec<(usize, &BookmarkNode)> {
        let mut out = Vec::new();
        if let Some(root) = self.get(ROOT_ID) {
            out.push((0, root));
        }
        out.extend(
            self.walk()
                .into_iter()
                .filter(|(_, node)| node.kind == NodeKind::Folder),
        );
        out
    }

    /// O favorito com este endereco (pela chave canonica), se existir.
    pub fn find_url(&self, url: &Url) -> Option<u64> {
        let key = bookmark_key(url);
        self.nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Link)
            .find(|node| {
                Url::parse(&node.url)
                    .ok()
                    .is_some_and(|stored| bookmark_key(&stored) == key)
            })
            .map(|node| node.id)
    }

    /// As chaves de todos os favoritos (o que a estrela da barra consulta).
    pub fn url_keys(&self) -> HashSet<String> {
        self.nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Link)
            .filter_map(|node| Url::parse(&node.url).ok())
            .map(|url| bookmark_key(&url))
            .collect()
    }

    /// A regra inteira da arvore.
    pub fn validate(&self) -> Result<(), TreeError> {
        let count = self.nodes.len();
        if count == 0 {
            return Err(TreeError::NoRoot);
        }
        if count > MAX_NODES {
            return Err(TreeError::TooManyNodes(count));
        }
        let mut index: HashMap<u64, usize> = HashMap::with_capacity(count);
        for (position, node) in self.nodes.iter().enumerate() {
            if node.id == 0 {
                return Err(TreeError::ZeroId);
            }
            if index.insert(node.id, position).is_some() {
                return Err(TreeError::DuplicateId(node.id));
            }
            if node.id >= self.next_id {
                return Err(TreeError::IdAheadOfNext(node.id));
            }
            if !title_is_clean(&node.title) {
                return Err(TreeError::BadTitle(node.id));
            }
            let url_ok = match node.kind {
                NodeKind::Folder => node.url.is_empty(),
                NodeKind::Link => Url::parse(&node.url)
                    .ok()
                    .is_some_and(|url| is_bookmarkable(&url) && node.url.len() <= MAX_URL_BYTES),
            };
            if !url_ok {
                return Err(TreeError::BadUrl(node.id));
            }
        }
        let root = *index.get(&ROOT_ID).ok_or(TreeError::NoRoot)?;
        let root_node = &self.nodes[root];
        if root_node.parent != 0 || root_node.kind != NodeKind::Folder {
            return Err(TreeError::NoRoot);
        }
        for node in &self.nodes {
            if node.id == ROOT_ID {
                continue;
            }
            let parent_ok = index
                .get(&node.parent)
                .is_some_and(|&at| self.nodes[at].kind == NodeKind::Folder);
            if !parent_ok {
                return Err(TreeError::BadParent(node.id));
            }
        }
        // Profundidades, subindo de cada no ate um de profundidade ja
        // conhecida: O(nos x profundidade). Um caminho que volta a um no do
        // proprio caminho e um ciclo; um que passa de MAX_DEPTH sem chegar a
        // raiz e fundo demais.
        let mut depth: Vec<Option<usize>> = vec![None; count];
        depth[root] = Some(0);
        let mut path: Vec<usize> = Vec::with_capacity(MAX_DEPTH + 2);
        for start in 0..count {
            path.clear();
            let mut current = start;
            let base = loop {
                if let Some(known) = depth[current] {
                    break known;
                }
                if path.contains(&current) {
                    return Err(TreeError::Cycle(self.nodes[current].id));
                }
                path.push(current);
                if path.len() > MAX_DEPTH + 1 {
                    return Err(TreeError::TooDeep(self.nodes[start].id));
                }
                current = index[&self.nodes[current].parent];
            };
            for (step, &at) in path.iter().rev().enumerate() {
                let here = base + step + 1;
                if here > MAX_DEPTH {
                    return Err(TreeError::TooDeep(self.nodes[at].id));
                }
                depth[at] = Some(here);
            }
        }
        Ok(())
    }

    /// A profundidade de `id` (a raiz esta a 0), numa arvore valida.
    pub fn depth_of(&self, id: u64) -> Option<usize> {
        let mut depth = 0;
        let mut current = self.get(id)?;
        while current.id != ROOT_ID {
            current = self.get(current.parent)?;
            depth += 1;
            if depth > MAX_DEPTH + 1 {
                return None;
            }
        }
        Some(depth)
    }

    /// `id` e os descendentes todos.
    fn subtree(&self, id: u64) -> HashSet<u64> {
        let mut inside = HashSet::from([id]);
        let mut frontier = vec![id];
        while let Some(parent) = frontier.pop() {
            for node in &self.nodes {
                if node.parent == parent && node.id != ROOT_ID && inside.insert(node.id) {
                    frontier.push(node.id);
                }
            }
        }
        inside
    }

    /// Quantos niveis descem de `id` (0: sem filhos).
    fn height(&self, id: u64) -> usize {
        let mut deepest = 0;
        let mut stack = vec![(id, 0usize)];
        while let Some((parent, level)) = stack.pop() {
            deepest = deepest.max(level);
            for node in &self.nodes {
                if node.parent == parent && node.id != ROOT_ID {
                    stack.push((node.id, level + 1));
                }
            }
        }
        deepest
    }

    fn next_order(&self, parent: u64) -> u32 {
        self.nodes
            .iter()
            .filter(|node| node.parent == parent && node.id != ROOT_ID)
            .map(|node| node.order.saturating_add(1))
            .max()
            .unwrap_or(0)
    }

    /// Volta a numerar os filhos de `parent` como 0, 1, 2...
    fn renumber(&mut self, parent: u64) {
        let mut ids: Vec<(u32, u64)> = self
            .nodes
            .iter()
            .filter(|node| node.parent == parent && node.id != ROOT_ID)
            .map(|node| (node.order, node.id))
            .collect();
        ids.sort_unstable();
        let order: HashMap<u64, u32> = ids
            .into_iter()
            .enumerate()
            .map(|(position, (_, id))| (id, position as u32))
            .collect();
        for node in &mut self.nodes {
            if let Some(&position) = order.get(&node.id) {
                node.order = position;
            }
        }
    }

    /// Um no novo no fim de `parent`. A pasta pai existe e tem espaco.
    fn push(
        &mut self,
        parent: u64,
        kind: NodeKind,
        title: String,
        url: String,
        added_ms: u64,
    ) -> Result<u64, OpError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(OpError::Full);
        }
        let parent_depth = self.folder_depth(parent)?;
        if parent_depth >= MAX_DEPTH {
            return Err(OpError::TooDeep);
        }
        let id = self.next_id;
        self.next_id += 1;
        let order = self.next_order(parent);
        self.nodes.push(BookmarkNode {
            id,
            parent,
            kind,
            title,
            url,
            added_ms,
            order,
        });
        Ok(id)
    }

    fn folder_depth(&self, id: u64) -> Result<usize, OpError> {
        match self.get(id) {
            Some(node) if node.kind == NodeKind::Folder => {
                self.depth_of(id).ok_or(OpError::NotFound(id))
            }
            Some(_) => Err(OpError::NotAFolder(id)),
            None => Err(OpError::NotFound(id)),
        }
    }

    /// Aplica `op`. Numa copia: ou tudo vale e a arvore que sai passa na
    /// regra (`validate`), ou a arvore fica como estava.
    pub fn apply(&mut self, op: BookmarkOp) -> Result<OpOutcome, OpError> {
        let mut next = self.clone();
        let outcome = next.apply_in_place(op)?;
        next.validate().map_err(OpError::Invalid)?;
        *self = next;
        Ok(outcome)
    }

    fn apply_in_place(&mut self, op: BookmarkOp) -> Result<OpOutcome, OpError> {
        match op {
            BookmarkOp::AddLink {
                parent,
                title,
                url,
                added_ms,
            } => {
                let url = bookmarkable_url(&url).ok_or(OpError::NotBookmarkable)?;
                if let Some(existing) = self.find_url(&url) {
                    return Ok(OpOutcome::Existing(existing));
                }
                let mut title = clean_title(&title);
                if title.is_empty() {
                    title = clean_title(url.host_str().unwrap_or_default());
                }
                let id = self.push(parent, NodeKind::Link, title, url.to_string(), added_ms)?;
                Ok(OpOutcome::Added(id))
            }
            BookmarkOp::AddFolder {
                parent,
                title,
                added_ms,
            } => {
                let title = clean_title(&title);
                if title.is_empty() {
                    return Err(OpError::EmptyTitle);
                }
                let id = self.push(parent, NodeKind::Folder, title, String::new(), added_ms)?;
                Ok(OpOutcome::Added(id))
            }
            BookmarkOp::Rename { id, title } => {
                if id == ROOT_ID {
                    return Err(OpError::Root);
                }
                let title = clean_title(&title);
                let node = self
                    .nodes
                    .iter_mut()
                    .find(|node| node.id == id)
                    .ok_or(OpError::NotFound(id))?;
                if title.is_empty() && node.kind == NodeKind::Folder {
                    return Err(OpError::EmptyTitle);
                }
                node.title = title;
                Ok(OpOutcome::Renamed(id))
            }
            BookmarkOp::Move { id, parent } => {
                if id == ROOT_ID {
                    return Err(OpError::Root);
                }
                let old_parent = self.get(id).ok_or(OpError::NotFound(id))?.parent;
                let parent_depth = self.folder_depth(parent)?;
                if self.subtree(id).contains(&parent) {
                    return Err(OpError::IntoItself);
                }
                if parent_depth + 1 + self.height(id) > MAX_DEPTH {
                    return Err(OpError::TooDeep);
                }
                if old_parent == parent {
                    return Ok(OpOutcome::Moved(id));
                }
                let order = self.next_order(parent);
                if let Some(node) = self.nodes.iter_mut().find(|node| node.id == id) {
                    node.parent = parent;
                    node.order = order;
                }
                self.renumber(old_parent);
                Ok(OpOutcome::Moved(id))
            }
            BookmarkOp::Delete { id } => {
                if id == ROOT_ID {
                    return Err(OpError::Root);
                }
                let parent = self.get(id).ok_or(OpError::NotFound(id))?.parent;
                let gone = self.subtree(id);
                self.nodes.retain(|node| !gone.contains(&node.id));
                self.renumber(parent);
                Ok(OpOutcome::Deleted {
                    id,
                    removed: gone.len(),
                })
            }
            BookmarkOp::Import {
                folder_title,
                items,
                added_ms,
            } => self.import(&folder_title, items, added_ms),
        }
    }

    /// Uma importacao: uma pasta nova na raiz com o que chegou. Os
    /// enderecos que ja existem (na arvore ou mais acima na mesma
    /// importacao) contam como «ja existiam»; os que nao sao http(s)
    /// (`javascript:`, `file:`...) e os que ja nao cabem, como «ignorados».
    /// Sem nada novo, a pasta nao fica.
    fn import(
        &mut self,
        folder_title: &str,
        items: Vec<ImportedItem>,
        added_ms: u64,
    ) -> Result<OpOutcome, OpError> {
        let title = clean_title(folder_title);
        if title.is_empty() {
            return Err(OpError::EmptyTitle);
        }
        let folder = self.push(ROOT_ID, NodeKind::Folder, title, String::new(), added_ms)?;
        let mut keys = self.url_keys();
        let mut report = ImportReport::default();
        self.import_into(folder, 1, items, &mut keys, &mut report, added_ms);
        if report.imported == 0 {
            let gone = self.subtree(folder);
            self.nodes.retain(|node| !gone.contains(&node.id));
            self.renumber(ROOT_ID);
            return Ok(OpOutcome::Imported {
                folder: None,
                report,
            });
        }
        Ok(OpOutcome::Imported {
            folder: Some(folder),
            report,
        })
    }

    fn import_into(
        &mut self,
        parent: u64,
        depth: usize,
        items: Vec<ImportedItem>,
        keys: &mut HashSet<String>,
        report: &mut ImportReport,
        now_ms: u64,
    ) {
        for item in items {
            match item {
                ImportedItem::Link {
                    title,
                    url,
                    added_ms,
                } => {
                    let Some(url) = bookmarkable_url(&url) else {
                        report.ignored += 1;
                        continue;
                    };
                    if keys.contains(&bookmark_key(&url)) {
                        report.existing += 1;
                        continue;
                    }
                    let mut title = clean_title(&title);
                    if title.is_empty() {
                        title = clean_title(url.host_str().unwrap_or_default());
                    }
                    let key = bookmark_key(&url);
                    match self.push(
                        parent,
                        NodeKind::Link,
                        title,
                        url.to_string(),
                        added_ms.unwrap_or(now_ms),
                    ) {
                        Ok(_) => {
                            keys.insert(key);
                            report.imported += 1;
                        }
                        Err(_) => report.ignored += 1,
                    }
                }
                ImportedItem::Folder {
                    title,
                    added_ms,
                    children,
                } => {
                    let title = clean_title(&title);
                    // Fundo demais, sem nome ou sem espaco: o conteudo
                    // entra na pasta de cima.
                    let target = if depth < MAX_DEPTH && !title.is_empty() {
                        self.push(
                            parent,
                            NodeKind::Folder,
                            title,
                            String::new(),
                            added_ms.unwrap_or(now_ms),
                        )
                        .ok()
                    } else {
                        None
                    };
                    match target {
                        Some(folder) => {
                            self.import_into(folder, depth + 1, children, keys, report, now_ms)
                        }
                        None => self.import_into(parent, depth, children, keys, report, now_ms),
                    }
                }
            }
        }
    }
}

// ===================== as operacoes =====================

/// Um item lido de outro navegador, antes de entrar na arvore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportedItem {
    Folder {
        title: String,
        added_ms: Option<u64>,
        children: Vec<ImportedItem>,
    },
    Link {
        title: String,
        url: String,
        added_ms: Option<u64>,
    },
}

impl ImportedItem {
    /// Quantos favoritos (nao pastas) ha aqui dentro.
    pub fn link_count(items: &[ImportedItem]) -> usize {
        let mut count = 0;
        let mut stack: Vec<&ImportedItem> = items.iter().collect();
        while let Some(item) = stack.pop() {
            match item {
                ImportedItem::Link { .. } => count += 1,
                ImportedItem::Folder { children, .. } => stack.extend(children),
            }
        }
        count
    }
}

/// O que uma operacao pede.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookmarkOp {
    AddLink {
        parent: u64,
        title: String,
        url: String,
        added_ms: u64,
    },
    AddFolder {
        parent: u64,
        title: String,
        added_ms: u64,
    },
    Rename {
        id: u64,
        title: String,
    },
    /// Para o fim da pasta `parent`.
    Move {
        id: u64,
        parent: u64,
    },
    /// O no e tudo o que esta dentro dele.
    Delete {
        id: u64,
    },
    Import {
        folder_title: String,
        items: Vec<ImportedItem>,
        added_ms: u64,
    },
}

/// As contas de uma importacao: «N importados · M já existiam · K
/// ignorados (javascript:, file:)».
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    pub existing: usize,
    pub ignored: usize,
}

impl ImportReport {
    /// A frase em pt-BR.
    pub fn summary(&self) -> String {
        format!(
            "{} importados · {} já existiam · {} ignorados (javascript:, file:)",
            self.imported, self.existing, self.ignored
        )
    }
}

/// O que uma operacao fez.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpOutcome {
    Added(u64),
    /// O endereco ja era um favorito (este); nada mudou.
    Existing(u64),
    Renamed(u64),
    Moved(u64),
    Deleted {
        id: u64,
        removed: usize,
    },
    Imported {
        /// A pasta criada (`None`: nada de novo, nenhuma pasta ficou).
        folder: Option<u64>,
        report: ImportReport,
    },
}

/// Porque uma operacao foi recusada (a arvore ficou como estava).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpError {
    #[error("o no {0} nao existe")]
    NotFound(u64),
    #[error("o no {0} nao e uma pasta")]
    NotAFolder(u64),
    #[error("a raiz «Favoritos» nao se muda")]
    Root,
    #[error("so paginas http ou https podem ser favoritos")]
    NotBookmarkable,
    #[error("o titulo esta vazio")]
    EmptyTitle,
    #[error("os favoritos chegaram ao tecto de {MAX_NODES}")]
    Full,
    #[error("passa a profundidade de {MAX_DEPTH}")]
    TooDeep,
    #[error("uma pasta nao vai para dentro de si mesma")]
    IntoItself,
    #[error("a arvore ficava invalida: {0}")]
    Invalid(TreeError),
}

// ===================== a loja =====================

/// Porque a loja recusou.
#[derive(Debug, thiserror::Error)]
pub enum BookmarkStoreError {
    #[error("a loja dos favoritos pede um grant Explicit, veio {0:?}")]
    WrongKind(StoreKind),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Op(#[from] OpError),
}

/// O que `BookmarkStore::apply` fez: o resultado, a arvore que ficou no
/// disco e se foi escrita.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub outcome: OpOutcome,
    pub tree: BookmarkTree,
    pub saved: SaveOutcome,
}

/// O `bookmarks.json`, partilhado entre janelas.
///
/// So abre com um grant do registo das lojas, e so `Explicit`:
///
/// ```compile_fail
/// use neural_core::bookmarks::BookmarkStore;
/// let _store = BookmarkStore::open(std::path::PathBuf::from("bookmarks.json"));
/// ```
///
/// ```no_run
/// use neural_core::bookmarks::BookmarkStore;
/// use neural_core::json_store::{StoreKind, StoreRegistry, StoreShape, StoreSpec};
/// let registry = StoreRegistry::mint(std::env::temp_dir()).unwrap();
/// let spec = StoreSpec::new("bookmarks.json", StoreKind::Explicit, StoreShape::File);
/// let _store = BookmarkStore::open(registry.grant(spec).unwrap()).unwrap();
/// ```
#[derive(Debug)]
pub struct BookmarkStore {
    store: VersionedJsonStore<BookmarkTree>,
    /// A ultima arvore lida ou escrita por ESTA loja: o que ela mostra.
    /// Nunca e a base de uma escrita -- `apply` rele o disco debaixo do
    /// trinco.
    tree: BookmarkTree,
}

impl BookmarkStore {
    /// Abre a loja com o seu grant: `Explicit` (o utilizador pediu para
    /// guardar -- vale tambem no modo privado), partilhada entre janelas.
    /// Nao le nada ainda (`load`).
    pub fn open(grant: StoreGrant) -> Result<Self, BookmarkStoreError> {
        if grant.kind() != StoreKind::Explicit {
            return Err(BookmarkStoreError::WrongKind(grant.kind()));
        }
        let store = VersionedJsonStore::open(grant, STORE_VERSION, STORE_MAX_BYTES)?
            .shared_between_windows();
        Ok(Self {
            store,
            tree: BookmarkTree::default(),
        })
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    /// A ultima arvore que esta loja leu ou gravou.
    pub fn tree(&self) -> &BookmarkTree {
        &self.tree
    }

    /// Le o ficheiro. Estragado (um ciclo, um pai que falta, JSON partido)
    /// ou de uma versao futura: so a raiz, a loja fica so de leitura e os
    /// bytes lidos vao para `bookmarks.json.bak`.
    pub fn load(&mut self) -> LoadOutcome<BookmarkTree> {
        let outcome = self.store.load();
        self.tree = outcome.value().clone();
        outcome
    }

    pub fn read_only(&self) -> Option<&Degraded> {
        self.store.read_only()
    }

    /// Aplica `op` debaixo do trinco, ao que esta no disco AGORA (relido
    /// la dentro, nunca a arvore desta loja), e grava. Uma operacao
    /// recusada deixa os favoritos como estavam.
    pub fn apply(&mut self, op: BookmarkOp) -> Result<Applied, BookmarkStoreError> {
        let (result, saved) = self.store.update(|tree| {
            tree.apply(op)
                .map(|outcome| (outcome, tree.clone()))
                .map_err(|error| (error, tree.clone()))
        })?;
        match result {
            Ok((outcome, tree)) => {
                self.tree = tree.clone();
                Ok(Applied {
                    outcome,
                    tree,
                    saved,
                })
            }
            Err((error, unchanged)) => {
                self.tree = unchanged;
                Err(BookmarkStoreError::Op(error))
            }
        }
    }
}

// ===================== importar do Chrome / Edge =====================

/// Porque uma importacao nao leu nada.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("o ficheiro nao existe")]
    Missing,
    #[error("nao foi possivel ler o ficheiro")]
    Unreadable,
    #[error("o ficheiro passa de {IMPORT_MAX_BYTES} bytes")]
    TooLarge,
    #[error("nao e um ficheiro de favoritos do Chrome")]
    NotChromium,
    #[error("nao e um ficheiro de favoritos em HTML")]
    NotNetscape,
}

impl ImportError {
    /// A frase para o utilizador.
    pub fn pt_br(&self) -> &'static str {
        match self {
            ImportError::Missing => "Não foram encontrados favoritos para importar.",
            ImportError::Unreadable => "Não foi possível ler o arquivo de favoritos.",
            ImportError::TooLarge => "O arquivo de favoritos passa de 32 MB.",
            ImportError::NotChromium => "O arquivo não é um arquivo de favoritos do Chrome.",
            ImportError::NotNetscape => "O arquivo não é uma exportação de favoritos em HTML.",
        }
    }
}

/// Os UNICOS ficheiros de um perfil do Chrome/Edge que este modulo abre.
/// O historico, as senhas e os cookies nao tem variante: nao ha como os
/// pedir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromiumFile {
    /// `<User Data>/Local State`: a lista dos perfis.
    LocalState,
    /// `<User Data>/<perfil>/Bookmarks`: os favoritos do perfil.
    Bookmarks,
}

impl ChromiumFile {
    pub fn name(self) -> &'static str {
        match self {
            ChromiumFile::LocalState => "Local State",
            ChromiumFile::Bookmarks => "Bookmarks",
        }
    }
}

/// O navegador de onde se importa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromiumBrowser {
    Chrome,
    Edge,
}

impl ChromiumBrowser {
    pub const ALL: [ChromiumBrowser; 2] = [ChromiumBrowser::Chrome, ChromiumBrowser::Edge];

    pub fn label(self) -> &'static str {
        match self {
            ChromiumBrowser::Chrome => "Chrome",
            ChromiumBrowser::Edge => "Edge",
        }
    }

    /// A pasta `User Data` debaixo de `%LOCALAPPDATA%`.
    pub fn user_data_dir(self, local_app_data: &Path) -> PathBuf {
        let parts: [&str; 3] = match self {
            ChromiumBrowser::Chrome => ["Google", "Chrome", "User Data"],
            ChromiumBrowser::Edge => ["Microsoft", "Edge", "User Data"],
        };
        let mut dir = local_app_data.to_path_buf();
        for part in parts {
            dir.push(part);
        }
        dir
    }
}

/// Le `path` ate `IMPORT_MAX_BYTES`: o tecto confere-se antes de ler, e um
/// ficheiro que cresce a meio para no tecto.
fn read_capped(path: &Path) -> Result<Vec<u8>, ImportError> {
    let file = File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ImportError::Missing,
        _ => ImportError::Unreadable,
    })?;
    let meta = file.metadata().map_err(|_| ImportError::Unreadable)?;
    if meta.is_dir() {
        return Err(ImportError::Unreadable);
    }
    if meta.len() > IMPORT_MAX_BYTES {
        return Err(ImportError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(IMPORT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError::Unreadable)?;
    if bytes.len() as u64 > IMPORT_MAX_BYTES {
        return Err(ImportError::TooLarge);
    }
    Ok(bytes)
}

/// Le um dos dois ficheiros permitidos de `dir` (so leitura, com tecto).
pub fn read_chromium_file(dir: &Path, file: ChromiumFile) -> Result<Vec<u8>, ImportError> {
    read_capped(&dir.join(file.name()))
}

/// Um HTML de favoritos escolhido pelo utilizador (so leitura, com tecto).
pub fn read_bookmarks_html(path: &Path) -> Result<Vec<u8>, ImportError> {
    read_capped(path)
}

/// Os microssegundos desde 1601 do Chrome em ms Unix; `None` sem data ou
/// antes de 1970.
pub fn chrome_time_to_unix_ms(text: &str) -> Option<u64> {
    let micros: u64 = text.trim().parse().ok()?;
    micros
        .checked_sub(CHROME_EPOCH_OFFSET_US)
        .filter(|since_1970| *since_1970 > 0)
        .map(|since_1970| since_1970 / 1000)
}

/// Um perfil do Chrome/Edge: a pasta (`Default`, `Profile 1`) e o nome que
/// o navegador mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromiumProfile {
    pub dir: String,
    pub name: String,
}

/// Um nome de pasta de perfil: uma parte so, sem separadores nem `..`.
fn is_profile_dir_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
}

/// Os perfis do `Local State`, pela ordem do navegador (`profiles_order`,
/// senao alfabetica). Pastas com nomes estranhos ficam de fora.
pub fn chromium_profiles(local_state: &[u8]) -> Vec<ChromiumProfile> {
    if local_state.len() as u64 > IMPORT_MAX_BYTES {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(local_state) else {
        return Vec::new();
    };
    let Some(profile) = value.get("profile") else {
        return Vec::new();
    };
    let Some(cache) = profile
        .get("info_cache")
        .and_then(|cache| cache.as_object())
    else {
        return Vec::new();
    };
    let mut order: Vec<String> = profile
        .get("profiles_order")
        .and_then(|order| order.as_array())
        .map(|order| {
            order
                .iter()
                .filter_map(|dir| dir.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut rest: Vec<String> = cache
        .keys()
        .filter(|dir| !order.contains(dir))
        .cloned()
        .collect();
    rest.sort();
    order.extend(rest);
    let mut seen = HashSet::new();
    order
        .into_iter()
        .filter(|dir| cache.contains_key(dir) && is_profile_dir_name(dir))
        .filter(|dir| seen.insert(dir.clone()))
        .map(|dir| {
            let name = cache
                .get(&dir)
                .and_then(|entry| entry.get("name"))
                .and_then(|name| name.as_str())
                .map(clean_title)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| dir.clone());
            ChromiumProfile { dir, name }
        })
        .collect()
}

/// Os perfis de uma pasta `User Data`: os do `Local State`; sem ele (ou sem
/// perfis la), o `Default`, se tiver `Bookmarks`.
pub fn chromium_profiles_in(user_data: &Path) -> Vec<ChromiumProfile> {
    let listed = read_chromium_file(user_data, ChromiumFile::LocalState)
        .map(|bytes| chromium_profiles(&bytes))
        .unwrap_or_default();
    if !listed.is_empty() {
        return listed;
    }
    let default = user_data.join("Default");
    if default.join(ChromiumFile::Bookmarks.name()).is_file() {
        return vec![ChromiumProfile {
            dir: "Default".to_string(),
            name: "Default".to_string(),
        }];
    }
    Vec::new()
}

/// Os favoritos de um perfil: le `<User Data>/<perfil>/Bookmarks` e so ele.
pub fn import_chromium_profile(
    user_data: &Path,
    profile: &ChromiumProfile,
) -> Result<Vec<ImportedItem>, ImportError> {
    if !is_profile_dir_name(&profile.dir) {
        return Err(ImportError::Missing);
    }
    let bytes = read_chromium_file(&user_data.join(&profile.dir), ChromiumFile::Bookmarks)?;
    parse_chromium_bookmarks(&bytes)
}

/// O ficheiro `Bookmarks` do Chrome/Edge: as tres raizes (barra de
/// favoritos, outros favoritos, favoritos do telemovel), cada uma uma pasta
/// com o nome que o navegador lhe deu; as vazias ficam de fora.
pub fn parse_chromium_bookmarks(bytes: &[u8]) -> Result<Vec<ImportedItem>, ImportError> {
    if bytes.len() as u64 > IMPORT_MAX_BYTES {
        return Err(ImportError::TooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ImportError::NotChromium)?;
    let roots = value
        .get("roots")
        .and_then(|roots| roots.as_object())
        .ok_or(ImportError::NotChromium)?;
    let mut budget = IMPORT_MAX_ITEMS;
    let mut items = Vec::new();
    for key in ["bookmark_bar", "other", "synced"] {
        let Some(root) = roots.get(key) else {
            continue;
        };
        if let Some(ImportedItem::Folder {
            title,
            added_ms,
            children,
        }) = chromium_node(root, 0, &mut budget)
            && !children.is_empty()
        {
            items.push(ImportedItem::Folder {
                title,
                added_ms,
                children,
            });
        }
    }
    Ok(items)
}

fn chromium_node(
    value: &serde_json::Value,
    nesting: usize,
    budget: &mut usize,
) -> Option<ImportedItem> {
    if *budget == 0 {
        return None;
    }
    let object = value.as_object()?;
    let text = |key: &str| object.get(key).and_then(|value| value.as_str());
    let title = text("name").unwrap_or_default().to_string();
    let added_ms = text("date_added").and_then(chrome_time_to_unix_ms);
    match text("type")? {
        "url" => {
            *budget -= 1;
            Some(ImportedItem::Link {
                title,
                url: text("url")?.to_string(),
                added_ms,
            })
        }
        "folder" => {
            *budget -= 1;
            let mut children = Vec::new();
            if nesting < IMPORT_MAX_NESTING
                && let Some(list) = object.get("children").and_then(|list| list.as_array())
            {
                for child in list {
                    if let Some(item) = chromium_node(child, nesting + 1, budget) {
                        children.push(item);
                    }
                }
            }
            Some(ImportedItem::Folder {
                title,
                added_ms,
                children,
            })
        }
        _ => None,
    }
}

// ===================== HTML (Netscape) =====================

/// O HTML de favoritos que o Chrome, o Edge e o Firefox exportam
/// («NETSCAPE-Bookmark-file-1»): `<DL>` com `<DT><H3>pasta</H3><DL>...`
/// e `<DT><A HREF ADD_DATE>favorito</A>`. As datas vem em segundos Unix.
pub fn parse_netscape_html(bytes: &[u8]) -> Result<Vec<ImportedItem>, ImportError> {
    if bytes.len() as u64 > IMPORT_MAX_BYTES {
        return Err(ImportError::TooLarge);
    }
    let text = String::from_utf8_lossy(bytes);
    let document = Html::parse_document(&text);
    let list = Selector::parse("dl").map_err(|_| ImportError::NotNetscape)?;
    let Some(top) = document.select(&list).next() else {
        return Err(ImportError::NotNetscape);
    };
    let mut budget = IMPORT_MAX_ITEMS;
    Ok(netscape_list(top, 0, &mut budget))
}

fn netscape_date(element: ElementRef<'_>) -> Option<u64> {
    let seconds: u64 = element.value().attr("add_date")?.trim().parse().ok()?;
    (seconds > 0).then(|| seconds.saturating_mul(1000))
}

fn element_text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>()
}

/// Os itens de uma `<DL>` (ou de um embrulho dela, como o `<p>`).
fn netscape_list(list: ElementRef<'_>, nesting: usize, budget: &mut usize) -> Vec<ImportedItem> {
    let mut items: Vec<ImportedItem> = Vec::new();
    if nesting > IMPORT_MAX_NESTING {
        return items;
    }
    for child in list.children() {
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };
        match element.value().name() {
            "dt" => netscape_entry(element, nesting, budget, &mut items),
            // Uma sub-lista solta, logo a seguir a um `<DT><H3>`: e o
            // conteudo dessa pasta.
            "dl" => {
                let nested = netscape_list(element, nesting + 1, budget);
                match items.last_mut() {
                    Some(ImportedItem::Folder { children, .. }) if children.is_empty() => {
                        children.extend(nested)
                    }
                    _ => items.extend(nested),
                }
            }
            "p" | "div" => items.extend(netscape_list(element, nesting + 1, budget)),
            _ => {}
        }
    }
    items
}

/// Um `<DT>`: um favorito (`<A>`) ou uma pasta (`<H3>` e a `<DL>` dela).
fn netscape_entry(
    entry: ElementRef<'_>,
    nesting: usize,
    budget: &mut usize,
    items: &mut Vec<ImportedItem>,
) {
    let mut folder: Option<(String, Option<u64>)> = None;
    let mut children = Vec::new();
    for child in entry.children() {
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };
        if *budget == 0 {
            return;
        }
        match element.value().name() {
            "a" => {
                let Some(href) = element.value().attr("href") else {
                    continue;
                };
                *budget -= 1;
                items.push(ImportedItem::Link {
                    title: element_text(element),
                    url: href.trim().to_string(),
                    added_ms: netscape_date(element),
                });
            }
            "h3" => {
                *budget -= 1;
                folder = Some((element_text(element), netscape_date(element)));
            }
            "dl" if folder.is_some() => {
                children.extend(netscape_list(element, nesting + 1, budget));
            }
            _ => {}
        }
    }
    if let Some((title, added_ms)) = folder {
        items.push(ImportedItem::Folder {
            title,
            added_ms,
            children,
        });
    }
}

/// `& < > " '` escapados: o mesmo texto serve dentro de um elemento e de
/// um atributo entre aspas.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// A arvore inteira no HTML de favoritos que qualquer navegador importa. Os
/// filhos da raiz ficam no primeiro nivel.
pub fn export_netscape_html(tree: &BookmarkTree) -> String {
    let mut by_parent: HashMap<u64, Vec<&BookmarkNode>> = HashMap::new();
    for node in tree.nodes() {
        if node.id != ROOT_ID {
            by_parent.entry(node.parent).or_default().push(node);
        }
    }
    for children in by_parent.values_mut() {
        children.sort_by_key(|node| (node.order, node.id));
    }
    let mut out = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n\
         <!-- This is an automatically generated file.\n     \
         It will be read and overwritten.\n     DO NOT EDIT! -->\n\
         <META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n\
         <TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
    );
    // Pre-ordem com uma pilha: as pastas abrem e fecham a sua `<DL>`.
    enum Step<'a> {
        Node(&'a BookmarkNode, usize),
        Close(usize),
    }
    let mut stack: Vec<Step<'_>> = by_parent
        .get(&ROOT_ID)
        .map(|children| {
            children
                .iter()
                .rev()
                .map(|node| Step::Node(node, 1))
                .collect()
        })
        .unwrap_or_default();
    while let Some(step) = stack.pop() {
        match step {
            Step::Node(node, depth) => {
                let indent = "    ".repeat(depth);
                let seconds = node.added_ms / 1000;
                match node.kind {
                    NodeKind::Link => out.push_str(&format!(
                        "{indent}<DT><A HREF=\"{}\" ADD_DATE=\"{seconds}\">{}</A>\n",
                        escape_html(&node.url),
                        escape_html(&node.title)
                    )),
                    NodeKind::Folder => {
                        out.push_str(&format!(
                            "{indent}<DT><H3 ADD_DATE=\"{seconds}\">{}</H3>\n{indent}<DL><p>\n",
                            escape_html(&node.title)
                        ));
                        stack.push(Step::Close(depth));
                        if let Some(children) = by_parent.get(&node.id) {
                            stack.extend(
                                children
                                    .iter()
                                    .rev()
                                    .map(|child| Step::Node(child, depth + 1)),
                            );
                        }
                    }
                }
            }
            Step::Close(depth) => {
                out.push_str(&"    ".repeat(depth));
                out.push_str("</DL><p>\n");
            }
        }
    }
    out.push_str("</DL><p>\n");
    out
}

// ===================== datas =====================

/// Um dia do calendario (o app passa o dia local; os testes, um fixo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Day {
    /// O dia UTC de um instante em ms Unix.
    pub fn from_unix_ms(ms: u64) -> Day {
        // Howard Hinnant, `civil_from_days`.
        let days = (ms / 86_400_000) as i64;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
        Day { year, month, day }
    }

    /// `23/09/2026`.
    pub fn dmy(self) -> String {
        format!("{:02}/{:02}/{:04}", self.day, self.month, self.year)
    }

    /// `2026-09-23`.
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// «Importado do Chrome (23/09/2026)».
pub fn import_folder_title(source: &str, day: Day) -> String {
    format!("Importado do {source} ({})", day.dmy())
}

/// «favoritos-neuralia-2026-09-23.html».
pub fn export_file_name(day: Day) -> String {
    format!("favoritos-neuralia-{}.html", day.iso())
}

#[cfg(test)]
mod tests;
