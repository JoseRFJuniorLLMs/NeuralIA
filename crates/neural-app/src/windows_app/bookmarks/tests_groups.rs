use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

use neural_core::chrome_groups::{ChromeSavedTab, ChromeTabGroup};
use neural_core::json_store::StoreRegistry;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "neuralia-favorite-groups-{}-{}-{}",
            std::process::id(),
            now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("pasta temporaria exclusiva");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn group(title: &str, tabs: &[(&str, &str)]) -> ChromeTabGroup {
    ChromeTabGroup {
        title: title.to_string(),
        color: 5,
        position: 0,
        tabs: tabs
            .iter()
            .enumerate()
            .map(|(position, (title, url))| ChromeSavedTab {
                title: (*title).to_string(),
                url: (*url).to_string(),
                position: position as i64,
            })
            .collect(),
    }
}

#[test]
fn chrome_group_destination_works_from_home_and_preserves_tab_bar_choice() {
    assert_eq!(
        tab_group_destination(false, 1),
        Some(TabGroupDestination::Bookmarks)
    );
    assert_eq!(tab_group_destination(false, 2), None);
    assert_eq!(
        tab_group_destination(true, 1),
        Some(TabGroupDestination::Bookmarks)
    );
    assert_eq!(
        tab_group_destination(true, 2),
        Some(TabGroupDestination::TabBar)
    );
    assert_eq!(tab_group_destination(true, 0), None);
    assert_eq!(tab_group_destination(false, 0), None);
    assert_eq!(tab_group_destination(true, 3), None);
}

#[test]
fn chrome_groups_scan_default_without_bookmarks_or_local_state() {
    let dir = TempDir::new();
    let profile = ChromiumBrowser::Chrome
        .user_data_dir(&dir.0)
        .join("Default");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        profile.join("Preferences"),
        br#"{
        "saved_tab_groups": {"specifics": [{"title":"Trabalho","saved_tabs":[
            {"url":"https://example.com/artigo","title":"Artigo"}
        ]}]}
    }"#,
    )
    .unwrap();
    let found = scan_chromium_tab_groups(&dir.0);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].label, "Chrome · Default");
    assert_eq!(found[0].groups[0].title, "Trabalho");
    assert_eq!(found[0].groups[0].tabs[0].url, "https://example.com/artigo");
}

/// A tarefa que embarca guarda pastas/links, preserva ordem e titulos,
/// aplica a validacao normal dos favoritos e recarrega do disco.
#[test]
fn chrome_groups_bookmark_job_persists_folders_and_safe_links() {
    let dir = TempDir::new();
    let registry = StoreRegistry::mint_for_test(dir.0.join("data"));
    let open = || BookmarkStore::open(registry.grant(BOOKMARKS_STORE).unwrap()).unwrap();
    let mut store = open();
    let incoming = vec![
        group(
            "Trabalho\n novo",
            &[
                ("Primeiro", "https://example.com/primeiro"),
                ("Segundo", "https://example.com/segundo"),
                ("Repetido", "https://example.com/primeiro"),
                ("Script", "javascript:alert(1)"),
            ],
        ),
        group("\n\t", &[("Livro", "https://example.org/livro")]),
    ];
    let folder_title = "Importado do Chrome · grupos de abas (14/10/2026)";
    match run_bookmark_job(
        &mut store,
        BookmarkJob::ImportGroups {
            groups: incoming,
            folder_title: folder_title.to_string(),
            now_ms: 123,
        },
    ) {
        BookmarkReply::Imported { report, .. } => assert_eq!(
            report,
            ImportReport {
                imported: 3,
                existing: 1,
                ignored: 1,
            }
        ),
        other => panic!("{other:?}"),
    }
    drop(store);
    let mut store = open();
    let tree = match run_bookmark_job(&mut store, BookmarkJob::Load) {
        BookmarkReply::Loaded { tree, notice: None } => tree,
        other => panic!("{other:?}"),
    };
    let imported = tree
        .nodes()
        .iter()
        .find(|node| node.title == folder_title)
        .unwrap();
    let work = tree
        .nodes()
        .iter()
        .find(|node| node.title == "Trabalho novo")
        .unwrap();
    assert_eq!(work.parent, imported.id);
    let mut links: Vec<_> = tree
        .nodes()
        .iter()
        .filter(|node| node.parent == work.id)
        .collect();
    links.sort_by_key(|node| node.order);
    assert_eq!(
        links
            .iter()
            .map(|node| node.title.as_str())
            .collect::<Vec<_>>(),
        ["Primeiro", "Segundo"]
    );
    assert!(
        links
            .iter()
            .all(|node| node.kind == NodeKind::Link && node.added_ms == 123)
    );
    let unnamed = tree
        .nodes()
        .iter()
        .find(|node| node.title == "Grupo sem nome")
        .unwrap();
    assert_eq!(unnamed.parent, imported.id);
    assert!(
        tree.nodes()
            .iter()
            .any(|node| node.parent == unnamed.id && node.title == "Livro")
    );
    assert!(
        tree.nodes()
            .iter()
            .all(|node| !node.url.starts_with("javascript:"))
    );
    assert!(dir.0.join("data").join("bookmarks.json").is_file());
}

#[test]
fn chrome_groups_bookmark_job_preserves_a_corrupt_read_only_store() {
    let dir = TempDir::new();
    let data = dir.0.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let original = b"not a favorites database";
    std::fs::write(data.join("bookmarks.json"), original).unwrap();
    let registry = StoreRegistry::mint_for_test(&data);
    let mut store = BookmarkStore::open(registry.grant(BOOKMARKS_STORE).unwrap()).unwrap();
    assert!(matches!(
        run_bookmark_job(
            &mut store,
            BookmarkJob::ImportGroups {
                groups: vec![group("Novo", &[("Pagina", "https://example.com/")])],
                folder_title: "Importado".to_string(),
                now_ms: 123,
            }
        ),
        BookmarkReply::Failed(_)
    ));
    assert_eq!(
        std::fs::read(data.join("bookmarks.json")).unwrap(),
        original
    );
}
