use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::json_store::{StoreRegistry, StoreShape, StoreSpec};

const CHROME_EXPORT: &str = include_str!("../../tests/fixtures/bookmarks/chrome-export.html");
const EDGE_EXPORT: &str = include_str!("../../tests/fixtures/bookmarks/edge-export.html");
const SPEC: StoreSpec = StoreSpec::new("bookmarks.json", StoreKind::Explicit, StoreShape::File);

/// A pasta das fixtures, montada por partes (portavel).
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("bookmarks")
}

fn user_data() -> PathBuf {
    fixtures().join("chrome-user-data")
}

/// Uma pasta temporaria so deste teste, apagada no fim.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "neuralia-bookmarks-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn link(parent: u64, title: &str, url: &str) -> BookmarkOp {
    BookmarkOp::AddLink {
        parent,
        title: title.to_string(),
        url: url.to_string(),
        added_ms: 1_000,
    }
}

fn folder(parent: u64, title: &str) -> BookmarkOp {
    BookmarkOp::AddFolder {
        parent,
        title: title.to_string(),
        added_ms: 1_000,
    }
}

fn added(outcome: Result<OpOutcome, OpError>) -> u64 {
    match outcome {
        Ok(OpOutcome::Added(id)) => id,
        other => panic!("esperava Added, veio {other:?}"),
    }
}

fn node(id: u64, parent: u64, kind: NodeKind, url: &str) -> BookmarkNode {
    BookmarkNode {
        id,
        parent,
        kind,
        title: format!("no {id}"),
        url: url.to_string(),
        added_ms: 0,
        order: 0,
    }
}

fn raw_tree(next_id: u64, nodes: Vec<BookmarkNode>) -> BookmarkTree {
    BookmarkTree { next_id, nodes }
}

fn root() -> BookmarkNode {
    BookmarkTree::default().nodes[0].clone()
}

fn counts(items: &[ImportedItem]) -> (usize, usize) {
    let mut links = 0;
    let mut folders = 0;
    let mut stack: Vec<&ImportedItem> = items.iter().collect();
    while let Some(item) = stack.pop() {
        match item {
            ImportedItem::Link { .. } => links += 1,
            ImportedItem::Folder { children, .. } => {
                folders += 1;
                stack.extend(children);
            }
        }
    }
    (links, folders)
}

fn folder_named<'a>(items: &'a [ImportedItem], name: &str) -> &'a [ImportedItem] {
    items
        .iter()
        .find_map(|item| match item {
            ImportedItem::Folder {
                title, children, ..
            } if title == name => Some(children.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("sem a pasta {name:?} em {items:?}"))
}

fn link_of<'a>(items: &'a [ImportedItem], name: &str) -> (&'a str, Option<u64>) {
    items
        .iter()
        .find_map(|item| match item {
            ImportedItem::Link {
                title,
                url,
                added_ms,
            } if title == name => Some((url.as_str(), *added_ms)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("sem o favorito {name:?} em {items:?}"))
}

// ===================== a arvore =====================

#[test]
fn the_default_tree_is_the_root_alone_and_valid() {
    let tree = BookmarkTree::default();
    assert_eq!(tree.len(), 1);
    assert!(tree.is_empty());
    let root = tree.get(ROOT_ID).expect("raiz");
    assert_eq!(
        (root.parent, root.kind, root.title.as_str()),
        (0, NodeKind::Folder, ROOT_TITLE)
    );
    assert_eq!(tree.validate(), Ok(()));
    // O JSON da arvore volta a ser a mesma arvore.
    let json = serde_json::to_string(&tree).expect("json");
    assert_eq!(
        serde_json::from_str::<BookmarkTree>(&json).expect("volta"),
        tree
    );
}

/// Gate (critico: dados do utilizador, entrada nao confiavel): a regra da
/// arvore recusa cada forma de arvore invalida -- e o `Deserialize` passa
/// por ela, por isso um ficheiro assim nunca vira arvore.
#[test]
fn validate_refuses_every_broken_tree() {
    let web = "https://example.com/";
    let cases: Vec<(&str, BookmarkTree, TreeError)> = vec![
        ("sem nos", raw_tree(2, vec![]), TreeError::NoRoot),
        (
            "sem raiz",
            raw_tree(3, vec![node(2, 0, NodeKind::Folder, "")]),
            TreeError::NoRoot,
        ),
        (
            "raiz com pai",
            raw_tree(3, vec![node(1, 2, NodeKind::Folder, "")]),
            TreeError::NoRoot,
        ),
        (
            "id 0",
            raw_tree(3, vec![root(), node(0, 1, NodeKind::Link, web)]),
            TreeError::ZeroId,
        ),
        (
            "id repetido",
            raw_tree(
                4,
                vec![
                    root(),
                    node(2, 1, NodeKind::Link, web),
                    node(2, 1, NodeKind::Link, web),
                ],
            ),
            TreeError::DuplicateId(2),
        ),
        (
            "id a frente do proximo",
            raw_tree(2, vec![root(), node(2, 1, NodeKind::Link, web)]),
            TreeError::IdAheadOfNext(2),
        ),
        (
            "pai que falta",
            raw_tree(4, vec![root(), node(2, 3, NodeKind::Link, web)]),
            TreeError::BadParent(2),
        ),
        (
            "pai que e um favorito",
            raw_tree(
                4,
                vec![
                    root(),
                    node(2, 1, NodeKind::Link, web),
                    node(3, 2, NodeKind::Link, "https://b.example/"),
                ],
            ),
            TreeError::BadParent(3),
        ),
        (
            "ciclo de dois",
            raw_tree(
                4,
                vec![
                    root(),
                    node(2, 3, NodeKind::Folder, ""),
                    node(3, 2, NodeKind::Folder, ""),
                ],
            ),
            TreeError::Cycle(2),
        ),
        (
            "no pai de si mesmo",
            raw_tree(3, vec![root(), node(2, 2, NodeKind::Folder, "")]),
            TreeError::Cycle(2),
        ),
        (
            "javascript:",
            raw_tree(
                3,
                vec![root(), node(2, 1, NodeKind::Link, "javascript:alert(1)")],
            ),
            TreeError::BadUrl(2),
        ),
        (
            "file:",
            raw_tree(
                3,
                vec![root(), node(2, 1, NodeKind::Link, "file:///C:/x.pdf")],
            ),
            TreeError::BadUrl(2),
        ),
        (
            "pasta com endereco",
            raw_tree(3, vec![root(), node(2, 1, NodeKind::Folder, web)]),
            TreeError::BadUrl(2),
        ),
    ];
    for (name, tree, error) in cases {
        assert_eq!(tree.validate(), Err(error), "{name}");
        let json = serde_json::to_string(&tree).expect("json");
        assert!(
            serde_json::from_str::<BookmarkTree>(&json).is_err(),
            "{name}: o Deserialize aceitou"
        );
    }

    // Titulos: ate 300 caracteres, sem controlo.
    let mut long = raw_tree(3, vec![root(), node(2, 1, NodeKind::Link, web)]);
    long.nodes[1].title = "a".repeat(MAX_TITLE_CHARS);
    assert_eq!(long.validate(), Ok(()));
    long.nodes[1].title.push('a');
    assert_eq!(long.validate(), Err(TreeError::BadTitle(2)));
    long.nodes[1].title = "linha\nquebrada".to_string();
    assert_eq!(long.validate(), Err(TreeError::BadTitle(2)));

    // Profundidade: uma cadeia de 16 pastas passa, 17 nao.
    let chain = |levels: u64| {
        let mut nodes = vec![root()];
        for id in 2..levels + 2 {
            nodes.push(node(id, id - 1, NodeKind::Folder, ""));
        }
        raw_tree(levels + 2, nodes)
    };
    assert_eq!(chain(MAX_DEPTH as u64).validate(), Ok(()));
    assert_eq!(
        chain(MAX_DEPTH as u64 + 1).validate(),
        Err(TreeError::TooDeep(MAX_DEPTH as u64 + 2))
    );

    // Nos: 20 000 passam, 20 001 nao.
    let mut full = vec![root()];
    for id in 2..=MAX_NODES as u64 {
        full.push(node(id, 1, NodeKind::Folder, ""));
    }
    let tree = raw_tree(MAX_NODES as u64 + 1, full.clone());
    assert_eq!(tree.validate(), Ok(()));
    full.push(node(MAX_NODES as u64 + 1, 1, NodeKind::Folder, ""));
    assert_eq!(
        raw_tree(MAX_NODES as u64 + 2, full).validate(),
        Err(TreeError::TooManyNodes(MAX_NODES + 1))
    );
}

#[test]
fn only_web_pages_can_be_bookmarks() {
    for good in [
        "https://example.com/",
        "http://example.com/a?b=c#d",
        "  https://pt.wikipedia.org/wiki/Brasil  ",
        "http://localhost:3000/",
    ] {
        assert!(bookmarkable_url(good).is_some(), "{good}");
    }
    let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
    for bad in [
        "about:blank",
        "data:text/html,<h1>x</h1>",
        "javascript:alert(1)",
        "file:///C:/Users/x/a.pdf",
        "neuralia-pdf://viewer/viewer.html",
        "http://neuralia-pdf.localhost/viewer.html",
        "https://neuralia-epub.localhost/library",
        "ftp://example.com/",
        "chrome://settings",
        "edge://favorites",
        "",
        "não é um endereço",
        long.as_str(),
    ] {
        assert!(bookmarkable_url(bad).is_none(), "{bad}");
    }
}

#[test]
fn clean_titles_lose_control_characters_and_stop_at_300() {
    assert_eq!(
        clean_title("  Título\u{0}com\tcontrolo\n "),
        "Título com controlo"
    );
    assert_eq!(
        clean_title(&"é".repeat(400)).chars().count(),
        MAX_TITLE_CHARS
    );
    assert_eq!(clean_title("\u{7}\u{1b}"), "");
}

// ===================== as operacoes =====================

/// Gate (critico: dados do utilizador): cada operacao e toda ou nada, o
/// mesmo endereco (pela chave canonica dos dominios) e o mesmo favorito, a
/// raiz nao se mexe, uma pasta nao vai para dentro de si, e apagar leva o
/// que esta dentro.
#[test]
fn operations_keep_the_tree_valid_and_dedupe_by_the_canonical_key() {
    let mut tree = BookmarkTree::default();
    let work = added(tree.apply(folder(ROOT_ID, "Trabalho")));
    let docs = added(tree.apply(folder(work, "Docs")));
    let page = added(tree.apply(link(
        docs,
        "Artigo",
        "https://www.example.com/artigo/?utm_source=x&id=7",
    )));
    // A mesma pagina com outro rastreio, sem www e sem a barra: o mesmo.
    assert_eq!(
        tree.apply(link(
            ROOT_ID,
            "Outro",
            "https://example.com/artigo?id=7&fbclid=1"
        )),
        Ok(OpOutcome::Existing(page))
    );
    assert_eq!(
        tree.find_url(&Url::parse("http://example.com/artigo?id=7").expect("url")),
        None,
        "http e outro esquema: outra chave"
    );
    assert_eq!(
        tree.find_url(&Url::parse("https://example.com/artigo/?id=7").expect("url")),
        Some(page)
    );
    // Sem titulo: o host.
    let bare = added(tree.apply(link(ROOT_ID, " \u{1} ", "https://rust-lang.org/")));
    assert_eq!(tree.get(bare).expect("no").title, "rust-lang.org");

    // Recusas: a arvore fica exactamente como estava.
    let before = tree.clone();
    for (op, error) in [
        (
            link(ROOT_ID, "js", "javascript:alert(1)"),
            OpError::NotBookmarkable,
        ),
        (
            link(page, "dentro de um favorito", "https://b.example/"),
            OpError::NotAFolder(page),
        ),
        (
            link(999, "pai que falta", "https://b.example/"),
            OpError::NotFound(999),
        ),
        (folder(ROOT_ID, "   "), OpError::EmptyTitle),
        (BookmarkOp::Delete { id: ROOT_ID }, OpError::Root),
        (
            BookmarkOp::Rename {
                id: ROOT_ID,
                title: "x".into(),
            },
            OpError::Root,
        ),
        (
            BookmarkOp::Move {
                id: work,
                parent: docs,
            },
            OpError::IntoItself,
        ),
        (
            BookmarkOp::Move {
                id: work,
                parent: work,
            },
            OpError::IntoItself,
        ),
        (BookmarkOp::Delete { id: 12345 }, OpError::NotFound(12345)),
    ] {
        assert_eq!(tree.apply(op.clone()), Err(error), "{op:?}");
        assert_eq!(tree, before, "{op:?} mexeu na arvore");
    }

    // Mover para o fim de outra pasta renumera as duas.
    assert_eq!(
        tree.apply(BookmarkOp::Move {
            id: page,
            parent: ROOT_ID
        }),
        Ok(OpOutcome::Moved(page))
    );
    let top: Vec<u64> = tree.children(ROOT_ID).iter().map(|node| node.id).collect();
    assert_eq!(top, vec![work, bare, page]);
    assert!(tree.children(docs).is_empty());
    let orders: Vec<u32> = tree
        .children(ROOT_ID)
        .iter()
        .map(|node| node.order)
        .collect();
    assert_eq!(orders, vec![0, 1, 2]);

    assert_eq!(
        tree.apply(BookmarkOp::Rename {
            id: page,
            title: "Artigo\nrenomeado".into()
        }),
        Ok(OpOutcome::Renamed(page))
    );
    assert_eq!(tree.get(page).expect("no").title, "Artigo renomeado");

    // Apagar a pasta leva a subpasta.
    assert_eq!(
        tree.apply(BookmarkOp::Delete { id: work }),
        Ok(OpOutcome::Deleted {
            id: work,
            removed: 2
        })
    );
    assert!(tree.get(docs).is_none());
    assert_eq!(tree.validate(), Ok(()));
    // Um id apagado nunca volta a ser dado.
    let fresh = added(tree.apply(folder(ROOT_ID, "Nova")));
    assert!(fresh > page && fresh > bare);
}

/// Gate (critico: tectos): 300 caracteres de titulo, 8 KiB de endereco,
/// profundidade 16, 20 000 nos.
#[test]
fn caps_hold_on_every_way_in() {
    let mut tree = BookmarkTree::default();
    let long_title = added(tree.apply(link(ROOT_ID, &"t".repeat(900), "https://a.example/")));
    assert_eq!(
        tree.get(long_title).expect("no").title.chars().count(),
        MAX_TITLE_CHARS
    );
    let long_url = format!("https://a.example/{}", "p".repeat(MAX_URL_BYTES));
    assert_eq!(
        tree.apply(link(ROOT_ID, "longo", &long_url)),
        Err(OpError::NotBookmarkable)
    );

    let mut parent = ROOT_ID;
    for level in 1..=MAX_DEPTH {
        parent = added(tree.apply(folder(parent, &format!("nivel {level}"))));
    }
    assert_eq!(tree.depth_of(parent), Some(MAX_DEPTH));
    assert_eq!(
        tree.apply(folder(parent, "fundo demais")),
        Err(OpError::TooDeep)
    );
    assert_eq!(
        tree.apply(link(parent, "fundo demais", "https://b.example/")),
        Err(OpError::TooDeep)
    );
    // Mover uma pasta com filhos para onde passava do fundo tambem nao.
    let shallow = added(tree.apply(folder(ROOT_ID, "raso")));
    added(tree.apply(folder(shallow, "filho")));
    let deep_parent = tree.get(parent).expect("no").parent;
    assert_eq!(
        tree.apply(BookmarkOp::Move {
            id: shallow,
            parent: deep_parent
        }),
        Err(OpError::TooDeep)
    );

    let mut full = BookmarkTree::default();
    for index in 0..MAX_NODES - 1 {
        full.nodes.push(BookmarkNode {
            id: full.next_id,
            parent: ROOT_ID,
            kind: NodeKind::Link,
            title: String::new(),
            url: format!("https://example.com/{index}"),
            added_ms: 0,
            order: index as u32,
        });
        full.next_id += 1;
    }
    assert_eq!(full.validate(), Ok(()));
    assert_eq!(
        full.apply(link(ROOT_ID, "mais um", "https://mais.example/")),
        Err(OpError::Full)
    );
    // Um repetido continua a ser encontrado com a arvore cheia.
    assert!(matches!(
        full.apply(link(ROOT_ID, "repetido", "https://example.com/7/")),
        Ok(OpOutcome::Existing(_))
    ));
}

// ===================== a loja =====================

#[test]
fn the_store_opens_only_with_an_explicit_grant() {
    let dir = TempDir::new("kind");
    let registry = StoreRegistry::mint_for_test(&dir.0);
    for kind in [StoreKind::Automatic, StoreKind::Setting] {
        let name = match kind {
            StoreKind::Automatic => "auto.json",
            _ => "setting.json",
        };
        let grant = registry
            .grant(StoreSpec::new(name, kind, StoreShape::File))
            .expect("grant");
        assert!(matches!(
            BookmarkStore::open(grant),
            Err(BookmarkStoreError::WrongKind(found)) if found == kind
        ));
    }
    let mut store = BookmarkStore::open(registry.grant(SPEC).expect("grant")).expect("abre");
    // Sem ficheiro: so a raiz, e nada escrito.
    assert!(matches!(store.load(), LoadOutcome::Missing(_)));
    assert!(!dir.0.join("bookmarks.json").exists());
    let applied = store
        .apply(link(ROOT_ID, "Exemplo", "https://example.com/"))
        .expect("grava");
    assert_eq!(applied.saved, SaveOutcome::Written);
    assert!(matches!(applied.outcome, OpOutcome::Added(_)));
    assert!(dir.0.join("bookmarks.json").is_file());
    assert_eq!(store.tree(), &applied.tree);
}

/// Gate (critico: dados do utilizador): um `bookmarks.json` com um ciclo
/// (ou qualquer arvore invalida) nao e lido como arvore: a loja fica so de
/// leitura, os bytes vao para `bookmarks.json.bak` e o ficheiro nunca e
/// reescrito -- nem por uma operacao a seguir.
#[test]
fn a_store_with_a_cycle_goes_to_bak_and_is_never_overwritten() {
    let dir = TempDir::new("cycle");
    let registry = StoreRegistry::mint_for_test(&dir.0);
    let path = dir.0.join("bookmarks.json");
    let cyclic = serde_json::json!({
        "version": STORE_VERSION,
        "data": {
            "next_id": 5,
            "nodes": [
                {"id": 1, "parent": 0, "kind": "folder", "title": "Favoritos"},
                {"id": 2, "parent": 3, "kind": "folder", "title": "A"},
                {"id": 3, "parent": 2, "kind": "folder", "title": "B"},
                {"id": 4, "parent": 2, "kind": "link", "title": "x", "url": "https://example.com/"}
            ]
        }
    })
    .to_string();
    std::fs::write(&path, &cyclic).expect("escreve");
    let mut store = BookmarkStore::open(registry.grant(SPEC).expect("grant")).expect("abre");
    match store.load() {
        LoadOutcome::Degraded { value, why, backup } => {
            assert_eq!(value, BookmarkTree::default());
            assert_eq!(why, Degraded::Corrupt);
            let backup = backup.expect("copia .bak");
            assert_eq!(backup, dir.0.join("bookmarks.json.bak"));
            assert_eq!(std::fs::read_to_string(backup).expect("bak"), cyclic);
        }
        other => panic!("um ciclo foi lido como arvore: {other:?}"),
    }
    assert_eq!(store.read_only(), Some(&Degraded::Corrupt));
    assert!(matches!(
        store.apply(link(ROOT_ID, "novo", "https://novo.example/")),
        Err(BookmarkStoreError::Store(StoreError::ReadOnly(
            Degraded::Corrupt
        )))
    ));
    // Uma segunda loja, que nunca leu, tambem nao escreve por cima.
    let mut other = BookmarkStore::open(registry.grant(SPEC).expect("grant")).expect("abre");
    assert!(
        other
            .apply(link(ROOT_ID, "novo", "https://novo.example/"))
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).expect("ficheiro"), cyclic);
}

/// Gate (critico: dados do utilizador): duas lojas no mesmo ficheiro (duas
/// janelas), intercaladas, cada uma com a sua arvore em memoria ja velha:
/// cada operacao rele o disco debaixo do trinco, e nenhuma perde a escrita
/// da outra.
#[test]
fn two_stores_interleaved_keep_both_writes() {
    let dir = TempDir::new("two");
    let registry = StoreRegistry::mint_for_test(&dir.0);
    let mut first = BookmarkStore::open(registry.grant(SPEC).expect("grant")).expect("abre");
    let mut second = BookmarkStore::open(registry.grant(SPEC).expect("grant")).expect("abre");
    first.load();
    second.load();
    first
        .apply(link(ROOT_ID, "Primeira", "https://primeira.example/"))
        .expect("primeira");
    // A segunda ainda so viu a raiz.
    assert!(second.tree().is_empty());
    let applied = second
        .apply(link(ROOT_ID, "Segunda", "https://segunda.example/"))
        .expect("segunda");
    let titles = |tree: &BookmarkTree| -> Vec<String> {
        tree.children(ROOT_ID)
            .iter()
            .map(|node| node.title.clone())
            .collect()
    };
    assert_eq!(titles(&applied.tree), ["Primeira", "Segunda"]);
    first.apply(folder(ROOT_ID, "Terceira")).expect("terceira");
    let mut fresh = BookmarkStore::open(registry.grant(SPEC).expect("grant")).expect("abre");
    let on_disk = fresh.load().into_value();
    assert_eq!(titles(&on_disk), ["Primeira", "Segunda", "Terceira"]);

    // E em paralelo, de verdade: duas threads, 40 favoritos cada.
    let threads: Vec<_> = (0..2)
        .map(|writer| {
            let grant = registry.grant(SPEC).expect("grant");
            std::thread::spawn(move || {
                let mut store = BookmarkStore::open(grant).expect("abre");
                for index in 0..40 {
                    store
                        .apply(link(
                            ROOT_ID,
                            &format!("{writer}-{index}"),
                            &format!("https://w{writer}.example/{index}"),
                        ))
                        .expect("grava");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("thread");
    }
    let on_disk = fresh.load().into_value();
    assert_eq!(on_disk.children(ROOT_ID).len(), 3 + 80);
}

// ===================== Chrome / Edge =====================

/// Gate (critico: entrada nao confiavel): a fixture do Chrome -- as
/// contas, a arvore, as datas (microssegundos desde 1601 -> ms Unix) -- e
/// o que entra na arvore: os repetidos e os `javascript:`/`file:` contados.
#[test]
fn the_chromium_fixture_gives_the_tree_the_counts_and_the_dates() {
    let bytes =
        read_chromium_file(&user_data().join("Default"), ChromiumFile::Bookmarks).expect("fixture");
    let items = parse_chromium_bookmarks(&bytes).expect("chrome");
    // O "synced" vazio fica de fora.
    let roots: Vec<&str> = items
        .iter()
        .map(|item| match item {
            ImportedItem::Folder { title, .. } => title.as_str(),
            ImportedItem::Link { title, .. } => panic!("favorito solto na raiz: {title}"),
        })
        .collect();
    assert_eq!(roots, ["Barra de favoritos", "Outros favoritos"]);
    assert_eq!(counts(&items), (7, 4));
    assert_eq!(ImportedItem::link_count(&items), 7);

    let bar = folder_named(&items, "Barra de favoritos");
    assert_eq!(
        link_of(bar, "NeuralIA no GitHub"),
        (
            "https://github.com/JoseRFJuniorLLMs/NeuralIA",
            // 2026-09-23T12:30:00Z
            Some(1_790_166_600_000)
        )
    );
    let reading = folder_named(bar, "Leituras");
    assert_eq!(
        link_of(reading, "The Rust Programming Language").1,
        Some(1_748_736_000_000) // 2025-06-01T00:00:00Z
    );
    let articles = folder_named(reading, "Artigos");
    assert_eq!(articles.len(), 2);
    let other = folder_named(&items, "Outros favoritos");
    assert_eq!(link_of(other, "Brasil – Wikipédia").1, None, "date 0");

    assert_eq!(
        chrome_time_to_unix_ms("13434640200000000"),
        Some(1_790_166_600_000)
    );
    assert_eq!(chrome_time_to_unix_ms("11644473600000000"), None);
    assert_eq!(chrome_time_to_unix_ms("0"), None);
    assert_eq!(chrome_time_to_unix_ms("-1"), None);
    assert_eq!(chrome_time_to_unix_ms("x"), None);

    // Na arvore: uma pasta «Importado do Chrome (23/09/2026)» com o que e
    // novo; o artigo repetido (a mesma chave) e o favorito ja existente
    // contam como «ja existiam», o javascript: e o file: como ignorados.
    let mut tree = BookmarkTree::default();
    added(tree.apply(link(
        ROOT_ID,
        "ja la",
        "https://pt.wikipedia.org/wiki/Brasil",
    )));
    let day = Day {
        year: 2026,
        month: 9,
        day: 23,
    };
    let outcome = tree
        .apply(BookmarkOp::Import {
            folder_title: import_folder_title("Chrome", day),
            items,
            added_ms: 5,
        })
        .expect("importa");
    let OpOutcome::Imported {
        folder: Some(folder),
        report,
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        report,
        ImportReport {
            imported: 3,
            existing: 2,
            ignored: 2
        }
    );
    assert_eq!(
        report.summary(),
        "3 importados · 2 já existiam · 2 ignorados (javascript:, file:)"
    );
    assert_eq!(
        tree.get(folder).expect("pasta").title,
        "Importado do Chrome (23/09/2026)"
    );
    // A estrutura veio: Importado > Barra > Leituras > Artigos > artigo.
    let path: Vec<String> = tree
        .walk()
        .into_iter()
        .filter(|(_, node)| node.kind == NodeKind::Folder)
        .map(|(depth, node)| format!("{depth}:{}", node.title))
        .collect();
    assert_eq!(
        path,
        [
            "1:Importado do Chrome (23/09/2026)",
            "2:Barra de favoritos",
            "3:Leituras",
            "4:Artigos",
            "2:Outros favoritos",
        ]
    );
    let github = tree
        .find_url(&Url::parse("https://github.com/JoseRFJuniorLLMs/NeuralIA").expect("url"))
        .expect("github");
    assert_eq!(tree.get(github).expect("no").added_ms, 1_790_166_600_000);
    assert_eq!(tree.validate(), Ok(()));

    // A mesma importacao outra vez: nada novo, nenhuma pasta vazia fica.
    let bytes =
        read_chromium_file(&user_data().join("Default"), ChromiumFile::Bookmarks).expect("fixture");
    let len = tree.len();
    let again = tree
        .apply(BookmarkOp::Import {
            folder_title: import_folder_title("Chrome", day),
            items: parse_chromium_bookmarks(&bytes).expect("chrome"),
            added_ms: 6,
        })
        .expect("importa");
    assert_eq!(
        again,
        OpOutcome::Imported {
            folder: None,
            report: ImportReport {
                imported: 0,
                existing: 5,
                ignored: 2
            }
        }
    );
    assert_eq!(tree.len(), len);
}

#[test]
fn chromium_profiles_come_from_local_state_in_the_browser_order() {
    let profiles = chromium_profiles_in(&user_data());
    assert_eq!(
        profiles,
        [
            ChromiumProfile {
                dir: "Profile 1".into(),
                name: "Trabalho".into()
            },
            ChromiumProfile {
                dir: "Default".into(),
                name: "Pessoa 1".into()
            },
        ],
        "as pastas com / ou \\ ficam de fora"
    );
    let work = import_chromium_profile(&user_data(), &profiles[0]).expect("perfil");
    assert_eq!(counts(&work), (1, 1));
    // Um perfil forjado nunca sai da pasta User Data.
    for dir in ["..", "../Default", "Default/..", "a\\b", ""] {
        let forged = ChromiumProfile {
            dir: dir.into(),
            name: "x".into(),
        };
        assert_eq!(
            import_chromium_profile(&user_data(), &forged),
            Err(ImportError::Missing),
            "{dir:?}"
        );
    }
    assert!(chromium_profiles(b"not json").is_empty());
    assert!(chromium_profiles(br#"{"profile":{}}"#).is_empty());
    // As pastas de cada navegador, por partes.
    let base = Path::new("base");
    assert_eq!(
        ChromiumBrowser::Chrome.user_data_dir(base),
        base.join("Google").join("Chrome").join("User Data")
    );
    assert_eq!(
        ChromiumBrowser::Edge.user_data_dir(base),
        base.join("Microsoft").join("Edge").join("User Data")
    );
}

/// Gate (critico: dados do utilizador de outro programa): a importacao so
/// abre `Local State` e `Bookmarks`. Num perfil com historico, senhas e
/// cookies ao lado, le os favoritos; e o codigo que embarca nao nomeia
/// esses ficheiros (a `ChromiumFile` so tem as duas variantes).
#[test]
fn the_import_reads_only_local_state_and_bookmarks() {
    let dir = TempDir::new("profile");
    let profile = dir.0.join("Default");
    std::fs::create_dir_all(&profile).expect("perfil");
    std::fs::copy(
        user_data().join("Default").join("Bookmarks"),
        profile.join("Bookmarks"),
    )
    .expect("copia");
    for decoy in ["History", "Login Data", "Cookies"] {
        std::fs::write(profile.join(decoy), b"SQLite format 3\0nao abrir").expect("isca");
    }
    // Sem Local State: o Default.
    let profiles = chromium_profiles_in(&dir.0);
    assert_eq!(profiles.len(), 1);
    let items = import_chromium_profile(&dir.0, &profiles[0]).expect("importa");
    assert_eq!(counts(&items), (7, 4));
    assert_eq!(
        [
            ChromiumFile::LocalState.name(),
            ChromiumFile::Bookmarks.name()
        ],
        ["Local State", "Bookmarks"]
    );

    let source = include_str!("../bookmarks.rs").replace("\r\n", "\n");
    for forbidden in [
        "\"History\"",
        "\"Login Data\"",
        "\"Cookies\"",
        "\"Web Data\"",
    ] {
        assert!(
            !source.contains(forbidden),
            "bookmarks.rs nomeia {forbidden}"
        );
    }
}

/// Gate (critico: tectos da entrada): acima de 32 MiB nada se le nem se
/// interpreta, e um encaixe fundo nao rebenta a pilha.
#[test]
fn imports_are_capped_before_parsing() {
    let dir = TempDir::new("cap");
    let big = dir.0.join("Bookmarks");
    let file = std::fs::File::create(&big).expect("cria");
    file.set_len(IMPORT_MAX_BYTES + 1).expect("tamanho");
    drop(file);
    assert_eq!(
        read_chromium_file(&dir.0, ChromiumFile::Bookmarks),
        Err(ImportError::TooLarge)
    );
    assert_eq!(read_bookmarks_html(&big), Err(ImportError::TooLarge));
    assert_eq!(
        read_chromium_file(&dir.0.join("nao-existe"), ChromiumFile::Bookmarks),
        Err(ImportError::Missing)
    );
    let oversize = vec![b' '; IMPORT_MAX_BYTES as usize + 1];
    assert_eq!(
        parse_chromium_bookmarks(&oversize),
        Err(ImportError::TooLarge)
    );
    assert_eq!(parse_netscape_html(&oversize), Err(ImportError::TooLarge));
    assert_eq!(
        parse_chromium_bookmarks(b"{\"roots\": 3}"),
        Err(ImportError::NotChromium)
    );
    assert_eq!(
        parse_netscape_html(b"<html><body>nada</body></html>"),
        Err(ImportError::NotNetscape)
    );

    // 400 pastas encaixadas em HTML: sem estouro, e na arvore o que passa
    // da profundidade sobe para a ultima pasta que cabe.
    let mut html = String::from("<DL><p>");
    for level in 0..400 {
        html.push_str(&format!("<DT><H3>p{level}</H3><DL><p>"));
    }
    html.push_str("<DT><A HREF=\"https://fundo.example/\">fundo</A>");
    let items = parse_netscape_html(html.as_bytes()).expect("html");
    let mut tree = BookmarkTree::default();
    let outcome = tree
        .apply(BookmarkOp::Import {
            folder_title: "Importado".into(),
            items,
            added_ms: 1,
        })
        .expect("importa");
    assert!(matches!(outcome, OpOutcome::Imported { .. }));
    assert_eq!(tree.validate(), Ok(()));
    assert!(tree.walk().iter().all(|(depth, _)| *depth <= MAX_DEPTH));
}

// ===================== HTML (Netscape) =====================

/// Gate (critico: entrada nao confiavel): as exportacoes do Chrome e do
/// Edge (maiusculas e minusculas, `<p>` e `<DD>` pelo meio) dao a mesma
/// arvore que o navegador mostrava -- sub-pastas dentro de sub-pastas --,
/// com as entidades decodificadas e as datas em ms.
#[test]
fn netscape_exports_of_chrome_and_edge_give_the_tree() {
    let chrome = parse_netscape_html(CHROME_EXPORT.as_bytes()).expect("chrome");
    assert_eq!(counts(&chrome), (6, 3));
    let bar = folder_named(&chrome, "Barra de favoritos");
    assert_eq!(
        link_of(bar, "NeuralIA no GitHub"),
        (
            "https://github.com/JoseRFJuniorLLMs/NeuralIA",
            Some(1_790_166_600_000)
        )
    );
    let reading = folder_named(bar, "Leituras");
    let articles = folder_named(reading, "Artigos & ensaios");
    assert_eq!(
        link_of(articles, "Artigo <7>").0,
        "https://www.example.com/artigo/?utm_source=news&id=7"
    );
    assert_eq!(
        link_of(reading, "Bookmarklet").0,
        "javascript:alert(document.cookie)"
    );
    assert_eq!(
        link_of(&chrome, "Brasil – Wikipédia").1,
        Some(1_748_736_000_000)
    );

    let edge = parse_netscape_html(EDGE_EXPORT.as_bytes()).expect("edge");
    assert_eq!(counts(&edge), (4, 4));
    let work = folder_named(folder_named(&edge, "Barra de favoritos"), "Trabalho");
    assert_eq!(
        link_of(work, "Outlook").0,
        "https://outlook.office.com/mail/"
    );
    assert_eq!(
        link_of(folder_named(work, "Projetos"), "GitHub"),
        ("https://github.com/", Some(1_705_305_600_000))
    );
    assert_eq!(
        link_of(folder_named(&edge, "Outros favoritos"), "Brasil").0,
        "https://pt.wikipedia.org/wiki/Brasil"
    );

    // Na arvore: o artigo com rastreio e o da fixture do Chrome sao o mesmo.
    let mut tree = BookmarkTree::default();
    let first = tree
        .apply(BookmarkOp::Import {
            folder_title: "Importado do Edge (01/06/2025)".into(),
            items: edge,
            added_ms: 1,
        })
        .expect("edge");
    assert!(matches!(
        first,
        OpOutcome::Imported {
            report: ImportReport {
                imported: 4,
                existing: 0,
                ignored: 0
            },
            ..
        }
    ));
    let second = tree
        .apply(BookmarkOp::Import {
            folder_title: "Importado".into(),
            items: chrome,
            added_ms: 1,
        })
        .expect("chrome");
    assert!(matches!(
        second,
        OpOutcome::Imported {
            report: ImportReport {
                imported: 3,
                existing: 1,
                ignored: 2
            },
            ..
        }
    ));
}

/// Gate (critico: saida que outro navegador interpreta): exportar escapa
/// `& < > " '` no texto e nos atributos. Um titulo `</A><script>` volta
/// igual depois de exportar e importar, e o HTML nunca tem um `<script>`.
#[test]
fn export_escapes_markup_and_round_trips() {
    let mut tree = BookmarkTree::default();
    let hostile = "</A><script>alert('x')</script> & \"aspas\"";
    let folder_id = added(tree.apply(folder(ROOT_ID, "Pasta <b>&</b>")));
    added(tree.apply(link(folder_id, hostile, "https://example.com/a?b=1&c=2")));
    added(tree.apply(link(
        ROOT_ID,
        "Com aspas",
        "https://example.com/\"x\"><img src=y>",
    )));
    let nested = added(tree.apply(folder(folder_id, "Dentro")));
    added(tree.apply(link(nested, "Fundo", "https://fundo.example/")));

    let html = export_netscape_html(&tree);
    assert!(html.starts_with("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n"));
    assert!(!html.to_ascii_lowercase().contains("<script"), "{html}");
    assert!(!html.contains("<b>"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert_eq!(html.matches("<DT><A ").count(), 3);
    assert_eq!(html.matches("<DL><p>").count(), 3);
    assert_eq!(html.matches("</DL><p>").count(), 3);

    let back = parse_netscape_html(html.as_bytes()).expect("volta");
    let mut again = BookmarkTree::default();
    again
        .apply(BookmarkOp::Import {
            folder_title: "Volta".into(),
            items: back,
            added_ms: 1,
        })
        .expect("importa");
    let shape = |tree: &BookmarkTree, skip: usize| -> Vec<(usize, NodeKind, String, String)> {
        tree.walk()
            .into_iter()
            .filter(|(depth, _)| *depth > skip)
            .map(|(depth, node)| {
                (
                    depth - skip,
                    node.kind,
                    node.title.clone(),
                    node.url.clone(),
                )
            })
            .collect()
    };
    assert_eq!(shape(&again, 1), shape(&tree, 0));
    assert_eq!(
        escape_html("<a href=\"x\">'&'</a>"),
        "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
    );
}

#[test]
fn days_format_for_the_import_folder_and_the_export_file() {
    assert_eq!(
        Day::from_unix_ms(1_790_166_600_000),
        Day {
            year: 2026,
            month: 9,
            day: 23
        }
    );
    assert_eq!(Day::from_unix_ms(0).iso(), "1970-01-01");
    assert_eq!(Day::from_unix_ms(951_782_400_000).iso(), "2000-02-29");
    assert_eq!(Day::from_unix_ms(4_107_542_400_000).iso(), "2100-03-01");
    let day = Day::from_unix_ms(1_790_166_600_000);
    assert_eq!(
        import_folder_title("Chrome", day),
        "Importado do Chrome (23/09/2026)"
    );
    assert_eq!(export_file_name(day), "favoritos-neuralia-2026-09-23.html");
}
