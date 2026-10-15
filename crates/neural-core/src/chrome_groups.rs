//! Grupos de abas guardados do Chrome e do Edge.
//!
//! O Chrome já não deixa a lista em `Preferences` (`specifics_to_data_migration`).
//! O que vale é o LevelDB de `Sync Data`: cada registo `saved_tab_group-dt-<uuid>`
//! traz um protobuf `SavedTabGroupSpecifics` (grupo ou aba). Tabelas `.ldb`
//! comprimidas com Snappy passam por `leveldb`; o log e os blocos sem
//! compressão continuam a ser lidos em texto.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

mod leveldb;

const MARKER: &[u8] = b"saved_tab_group-dt-";
const MAX_GROUPS: usize = 64;
const MAX_TABS: usize = 100;
const MAX_URL_CHARS: usize = 2048;
const MAX_TITLE_CHARS: usize = 120;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROFILE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PREFERENCES_BYTES: usize = 32 * 1024 * 1024;

/// Um grupo tal como o Chrome o guardou, já só com abas http(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeTabGroup {
    pub title: String,
    pub color: i32,
    pub position: i64,
    pub tabs: Vec<ChromeSavedTab>,
}

/// Uma aba dentro do grupo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeSavedTab {
    pub position: i64,
    pub url: String,
    pub title: String,
}

#[derive(Default)]
struct GroupBits {
    position: i64,
    title: String,
    color: i32,
    saw: bool,
}

struct TabBits {
    group_guid: String,
    position: i64,
    url: String,
    title: String,
}

enum Record {
    Group {
        guid: String,
        position: i64,
        title: String,
        color: i32,
    },
    Tab {
        guid: String,
        group_guid: String,
        position: i64,
        url: String,
        title: String,
    },
}

#[derive(Default)]
struct Index {
    groups: HashMap<String, (i64, String, i32)>,
    tabs: HashMap<String, TabBits>,
}

impl Index {
    fn absorb(&mut self, record: Record) {
        match record {
            Record::Group {
                guid,
                position,
                title,
                color,
            } => {
                self.groups.insert(guid, (position, title, color));
            }
            Record::Tab {
                guid,
                group_guid,
                position,
                url,
                title,
            } => {
                self.tabs.insert(
                    guid,
                    TabBits {
                        group_guid,
                        position,
                        url,
                        title,
                    },
                );
            }
        }
    }

    fn finish(self) -> Vec<ChromeTabGroup> {
        let mut by_group: HashMap<String, Vec<&TabBits>> = HashMap::new();
        for tab in self.tabs.values() {
            if !http_url(&tab.url) {
                continue;
            }
            by_group
                .entry(tab.group_guid.clone())
                .or_default()
                .push(tab);
        }
        let mut guids: Vec<String> = by_group.keys().cloned().collect();
        guids.sort_by(|a, b| {
            let pa = self.groups.get(a).map(|group| group.0).unwrap_or(i64::MAX);
            let pb = self.groups.get(b).map(|group| group.0).unwrap_or(i64::MAX);
            pa.cmp(&pb).then_with(|| a.cmp(b))
        });
        let mut out = Vec::new();
        for guid in guids {
            if out.len() >= MAX_GROUPS {
                break;
            }
            let Some(tabs) = by_group.get(&guid) else {
                continue;
            };
            let (position, title, color) =
                self.groups
                    .get(&guid)
                    .cloned()
                    .unwrap_or((0, String::new(), 0));
            let mut ordered: Vec<&TabBits> = tabs.clone();
            ordered.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.url.cmp(&b.url)));
            let mut seen = std::collections::HashSet::new();
            let mut kept = Vec::new();
            for tab in ordered {
                if kept.len() >= MAX_TABS {
                    break;
                }
                if !seen.insert(tab.url.clone()) {
                    continue;
                }
                kept.push(ChromeSavedTab {
                    position: tab.position,
                    url: tab.url.clone(),
                    title: clip(&tab.title, MAX_TITLE_CHARS),
                });
            }
            if kept.is_empty() {
                continue;
            }
            out.push(ChromeTabGroup {
                title: clip(&title, MAX_TITLE_CHARS),
                color,
                position,
                tabs: kept,
            });
        }
        out
    }
}

fn clip(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

fn http_url(value: &str) -> bool {
    value.len() <= MAX_URL_CHARS
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        && crate::bookmarks::bookmarkable_url(value).is_some()
}

fn is_uuid(bytes: &[u8]) -> bool {
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() || *byte == b'-')
}

fn read_varint(data: &[u8], index: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0;
    while *index < data.len() && shift < 64 {
        let byte = data[*index];
        *index += 1;
        // O decimo byte de um varint u64 so pode guardar o bit 63.
        if shift == 63 && byte > 1 {
            return None;
        }
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
    }
    None
}

enum Field<'a> {
    Varint(u32, u64),
    Bytes(u32, &'a [u8]),
}

fn message(data: &[u8]) -> Option<Vec<Field<'_>>> {
    let mut index = 0;
    let mut fields = Vec::new();
    while index < data.len() {
        let tag = read_varint(data, &mut index)?;
        let field = u32::try_from(tag >> 3).ok()?;
        if field == 0 || field >= (1 << 29) {
            return None;
        }
        match tag & 7 {
            0 => fields.push(Field::Varint(field, read_varint(data, &mut index)?)),
            1 => {
                if index + 8 > data.len() {
                    return None;
                }
                index += 8;
            }
            2 => {
                let len = usize::try_from(read_varint(data, &mut index)?).ok()?;
                if len > data.len() - index {
                    return None;
                }
                fields.push(Field::Bytes(field, &data[index..index + len]));
                index += len;
            }
            5 => {
                if index + 4 > data.len() {
                    return None;
                }
                index += 4;
            }
            _ => return None,
        }
    }
    Some(fields)
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn as_group(body: &[u8]) -> Option<GroupBits> {
    let mut group = GroupBits::default();
    for field in message(body)? {
        match field {
            Field::Varint(1, value) => {
                group.position = value as i64;
                group.saw = true;
            }
            Field::Bytes(2, bytes) => {
                group.title = utf8(bytes);
                group.saw = true;
            }
            Field::Varint(3, value) if value <= 20 => {
                group.color = value as i32;
                group.saw = true;
            }
            _ => {}
        }
    }
    group.saw.then_some(group)
}

fn as_tab(body: &[u8]) -> Option<TabBits> {
    let mut group_guid = String::new();
    let mut position = 0i64;
    let mut url = String::new();
    let mut title = String::new();
    let mut saw_url = false;
    for field in message(body)? {
        match field {
            Field::Bytes(1, bytes) => group_guid = utf8(bytes),
            Field::Varint(2, value) => position = value as i64,
            Field::Bytes(3, bytes) => {
                url = utf8(bytes);
                saw_url = true;
            }
            Field::Bytes(4, bytes) => title = utf8(bytes),
            _ => {}
        }
    }
    (saw_url && http_url(&url)).then_some(TabBits {
        group_guid,
        position,
        url,
        title,
    })
}

fn from_specifics(body: &[u8], key_guid: &str, depth: u8) -> Option<Record> {
    let fields = message(body)?;
    let mut guid = String::new();
    let mut group = None;
    let mut tab = None;
    let mut nested = Vec::new();
    for field in fields {
        match field {
            Field::Bytes(1, bytes) => {
                let text = utf8(bytes);
                if is_uuid(text.as_bytes()) {
                    guid = text;
                } else if depth == 0 {
                    nested.push(bytes);
                }
            }
            Field::Bytes(4, bytes) => group = as_group(bytes),
            Field::Bytes(5, bytes) => tab = as_tab(bytes),
            Field::Bytes(_, bytes) if depth == 0 => nested.push(bytes),
            _ => {}
        }
    }
    let guid = if guid.is_empty() {
        key_guid.to_string()
    } else {
        guid
    };
    if let Some(tab) = tab {
        let group_guid = if is_uuid(tab.group_guid.as_bytes()) {
            tab.group_guid
        } else {
            String::new()
        };
        return Some(Record::Tab {
            guid,
            group_guid,
            position: tab.position,
            url: tab.url,
            title: tab.title,
        });
    }
    if let Some(group) = group {
        return Some(Record::Group {
            guid,
            position: group.position,
            title: group.title,
            color: group.color,
        });
    }
    if depth == 0 {
        for bytes in nested {
            if let Some(record) = from_specifics(bytes, key_guid, 1) {
                return Some(record);
            }
        }
    }
    None
}

fn try_record(data: &[u8], key_guid: &str) -> Option<(usize, Record)> {
    let mut index = 0;
    let len = usize::try_from(read_varint(data, &mut index)?).ok()?;
    if !(8..=8192).contains(&len) || index + len > data.len() {
        return None;
    }
    let body = &data[index..index + len];
    let record = from_specifics(body, key_guid, 0).or_else(|| {
        as_tab(body).map(|tab| Record::Tab {
            guid: key_guid.to_string(),
            group_guid: tab.group_guid,
            position: tab.position,
            url: tab.url,
            title: tab.title,
        })
    })?;
    Some((index + len, record))
}

fn absorb_bytes(index: &mut Index, bytes: &[u8]) {
    let mut cursor = 0;
    let mut found = 0usize;
    while found < 400 {
        let Some(at) = bytes[cursor..]
            .windows(MARKER.len())
            .position(|window| window == MARKER)
        else {
            break;
        };
        let marker = cursor + at;
        let uuid_at = marker + MARKER.len();
        if uuid_at + 36 > bytes.len() {
            break;
        }
        if !is_uuid(&bytes[uuid_at..uuid_at + 36]) {
            cursor = marker + 1;
            continue;
        }
        let key_guid = String::from_utf8_lossy(&bytes[uuid_at..uuid_at + 36]).into_owned();
        let search_from = uuid_at + 36;
        let search_to = (search_from + 96).min(bytes.len());
        let mut advanced = false;
        for start in search_from..search_to {
            if let Some((end, record)) = try_record(&bytes[start..], &key_guid) {
                index.absorb(record);
                cursor = start + end;
                found += 1;
                advanced = true;
                break;
            }
        }
        if !advanced {
            cursor = uuid_at + 36;
        }
    }
}

/// Uma tabela LevelDB entrega as chaves e os valores já descomprimidos.
/// Um extrato que não é tabela, ou que não passa na conferência, continua
/// a ser varrido em texto.
fn absorb_profile_file(index: &mut Index, bytes: &[u8]) {
    if leveldb::is_table(bytes) {
        let mut budget = leveldb::Budget {
            bytes: MAX_FILE_BYTES,
            entries: 100_000,
        };
        if let Some(entries) = leveldb::read_table(bytes, &mut budget) {
            for (mut key, value) in entries {
                key.extend_from_slice(&value);
                absorb_bytes(index, &key);
            }
            return;
        }
    }
    absorb_bytes(index, bytes);
}

/// Os grupos num extrato de LevelDB (`.ldb` ou `.log`), em texto.
pub fn parse_saved_tab_groups(bytes: &[u8]) -> Vec<ChromeTabGroup> {
    if bytes.len() > MAX_FILE_BYTES {
        return Vec::new();
    }
    let mut index = Index::default();
    absorb_bytes(&mut index, bytes);
    index.finish()
}

fn json_i64(value: &serde_json::Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or(0)
}

fn json_color(value: &serde_json::Value) -> i32 {
    if let Some(number) = value.as_i64() {
        return number as i32;
    }
    match value.as_str().unwrap_or("").to_ascii_uppercase().as_str() {
        "GREY" | "GRAY" => 1,
        "BLUE" => 2,
        "RED" => 3,
        "YELLOW" => 4,
        "GREEN" => 5,
        "PINK" => 6,
        "PURPLE" => 7,
        "CYAN" => 8,
        "ORANGE" => 9,
        _ => 0,
    }
}

/// O formato antigo, quando `Preferences` ainda traz `saved_tab_groups.specifics`.
pub fn groups_from_preferences(bytes: &[u8]) -> Vec<ChromeTabGroup> {
    if bytes.len() > MAX_PREFERENCES_BYTES {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    let Some(specifics) = value
        .get("saved_tab_groups")
        .and_then(|node| node.get("specifics"))
        .and_then(|node| node.as_array())
    else {
        return Vec::new();
    };
    let mut index = Index::default();
    for (ordinal, item) in specifics.iter().enumerate() {
        let guid = item
            .get("guid")
            .and_then(|node| node.as_str())
            .unwrap_or("")
            .to_string();
        let guid = if is_uuid(guid.as_bytes()) {
            guid
        } else {
            format!("pref-{ordinal:04}")
        };
        if let Some(tabs) = item
            .get("saved_tabs")
            .or_else(|| item.get("tabs"))
            .and_then(|node| node.as_array())
        {
            index.absorb(Record::Group {
                guid: guid.clone(),
                position: json_i64(item.get("position").unwrap_or(&serde_json::Value::Null)),
                title: item
                    .get("title")
                    .and_then(|node| node.as_str())
                    .unwrap_or("")
                    .to_string(),
                color: item.get("color").map(json_color).unwrap_or(0),
            });
            for (tab_index, tab) in tabs.iter().enumerate() {
                let url = tab.get("url").and_then(|node| node.as_str()).unwrap_or("");
                if !http_url(url) {
                    continue;
                }
                index.absorb(Record::Tab {
                    guid: format!("{guid}:{tab_index}"),
                    group_guid: guid.clone(),
                    position: json_i64(tab.get("position").unwrap_or(&serde_json::Value::Null)),
                    url: url.to_string(),
                    title: tab
                        .get("title")
                        .and_then(|node| node.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
            continue;
        }
        if let Some(group) = item.get("group") {
            index.absorb(Record::Group {
                guid,
                position: json_i64(group.get("position").unwrap_or(&serde_json::Value::Null)),
                title: group
                    .get("title")
                    .and_then(|node| node.as_str())
                    .unwrap_or("")
                    .to_string(),
                color: group.get("color").map(json_color).unwrap_or(0),
            });
        } else if let Some(tab) = item.get("tab") {
            let url = tab.get("url").and_then(|node| node.as_str()).unwrap_or("");
            if !http_url(url) {
                continue;
            }
            let group_guid = tab
                .get("group_guid")
                .and_then(|node| node.as_str())
                .unwrap_or("")
                .to_string();
            index.absorb(Record::Tab {
                guid,
                group_guid,
                position: json_i64(tab.get("position").unwrap_or(&serde_json::Value::Null)),
                url: url.to_string(),
                title: tab
                    .get("title")
                    .and_then(|node| node.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    index.finish()
}

/// Os grupos de um perfil (`Default`, `Profile 1`, …).
pub fn read_profile_tab_groups(profile_dir: &Path) -> Vec<ChromeTabGroup> {
    let mut index = Index::default();
    let mut total = 0usize;
    let leveldb = profile_dir.join("Sync Data").join("LevelDB");
    if let Ok(entries) = std::fs::read_dir(&leveldb) {
        let mut files: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| is_leveldb_payload(path))
            .collect();
        files.sort_by_key(|path| file_rank(path));
        for path in files {
            if total >= MAX_PROFILE_BYTES {
                break;
            }
            let limit = MAX_FILE_BYTES.min(MAX_PROFILE_BYTES - total);
            let Some(bytes) = read_file_capped(&path, limit) else {
                continue;
            };
            total += bytes.len();
            absorb_profile_file(&mut index, &bytes);
        }
    }
    let parsed = index.finish();
    if !parsed.is_empty() {
        return parsed;
    }
    read_file_capped(&profile_dir.join("Preferences"), MAX_PREFERENCES_BYTES)
        .map(|bytes| groups_from_preferences(&bytes))
        .unwrap_or_default()
}

/// Confere o tamanho antes de alocar e limita a leitura se o ficheiro
/// crescer enquanto o Chrome o grava. Um extrato parcial nao e importado.
fn read_file_capped(path: &Path, limit: usize) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= limit).then_some(bytes)
}

fn is_leveldb_payload(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name == "log"
        || name == "log.old"
        || matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some(ext) if ext.eq_ignore_ascii_case("ldb") || ext.eq_ignore_ascii_case("log")
        )
}

fn file_rank(path: &Path) -> (u8, String) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_log = name == "log"
        || name == "log.old"
        || path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("log"));
    (u8::from(is_log), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
        out
    }

    fn delimited(field: u32, body: &[u8]) -> Vec<u8> {
        let mut out = vec![((field << 3) | 2) as u8];
        out.extend(varint(body.len() as u64));
        out.extend(body);
        out
    }

    fn proto_str(field: u32, text: &str) -> Vec<u8> {
        delimited(field, text.as_bytes())
    }

    fn proto_var(field: u32, value: u64) -> Vec<u8> {
        let mut out = vec![(field << 3) as u8];
        out.extend(varint(value));
        out
    }

    fn wrap(guid: &str, specifics: &[u8]) -> Vec<u8> {
        let mut blob = b"xxsaved_tab_group-dt-".to_vec();
        blob.extend(guid.as_bytes());
        blob.extend(b"pad");
        blob.extend(varint(specifics.len() as u64));
        blob.extend(specifics);
        blob
    }

    #[test]
    fn leveldb_text_becomes_groups_and_the_last_record_wins() {
        let group_guid = "11111111-1111-1111-1111-111111111111";
        let tab_guid = "22222222-2222-2222-2222-222222222222";
        let group = [proto_var(1, 3), proto_str(2, "Trabalho"), proto_var(3, 2)].concat();
        let specifics = [proto_str(1, group_guid), delimited(4, &group)].concat();
        let tab = [
            proto_str(1, group_guid),
            proto_var(2, 1),
            proto_str(3, "https://example.com/a"),
            proto_str(4, "Artigo"),
        ]
        .concat();
        let tab_specifics = [proto_str(1, tab_guid), delimited(5, &tab)].concat();
        let renamed = [proto_str(2, "Trabalho novo"), proto_var(3, 5)].concat();
        let renamed_specifics = [proto_str(1, group_guid), delimited(4, &renamed)].concat();
        let mut bytes = wrap(group_guid, &specifics);
        bytes.extend(wrap(tab_guid, &tab_specifics));
        bytes.extend(wrap(group_guid, &renamed_specifics));
        let groups = parse_saved_tab_groups(&bytes);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Trabalho novo");
        assert_eq!(groups[0].color, 5);
        assert_eq!(groups[0].tabs.len(), 1);
        assert_eq!(groups[0].tabs[0].url, "https://example.com/a");
        assert_eq!(groups[0].tabs[0].title, "Artigo");
    }

    #[test]
    fn compressed_leveldb_table_yields_the_same_saved_group() {
        let group_guid = "11111111-1111-1111-1111-111111111111";
        let tab_guid = "22222222-2222-2222-2222-222222222222";
        let group = [proto_var(1, 3), proto_str(2, "Comprimido"), proto_var(3, 2)].concat();
        let specifics = [proto_str(1, group_guid), delimited(4, &group)].concat();
        let tab = [
            proto_str(1, group_guid),
            proto_var(2, 1),
            proto_str(3, "https://example.com/comprimido"),
            proto_str(4, "Aba"),
        ]
        .concat();
        let tab_specifics = [proto_str(1, tab_guid), delimited(5, &tab)].concat();
        let file = leveldb::fixture_table(
            &[
                (b"group".to_vec(), wrap(group_guid, &specifics)),
                (b"tab".to_vec(), wrap(tab_guid, &tab_specifics)),
            ],
            true,
        );
        let mut index = Index::default();
        absorb_profile_file(&mut index, &file);
        let groups = index.finish();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Comprimido");
        assert_eq!(groups[0].tabs[0].url, "https://example.com/comprimido");
        let mut corrupt = file.clone();
        corrupt[0] ^= 1;
        let mut broken = Index::default();
        absorb_profile_file(&mut broken, &corrupt);
        assert!(broken.finish().is_empty());
    }

    #[test]
    fn preferences_specifics_keep_only_web_tabs() {
        let json = br#"{
            "saved_tab_groups": { "specifics": [
                { "guid": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "title": "Leitura", "color": "GREEN",
                  "saved_tabs": [
                    { "url": "https://example.com/ok", "title": "Ok", "position": 0 },
                    { "url": "chrome://settings", "title": "nao", "position": 1 }
                  ] }
            ] }
        }"#;
        let groups = groups_from_preferences(json);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Leitura");
        assert_eq!(groups[0].color, 5);
        assert_eq!(groups[0].tabs.len(), 1);
    }

    #[test]
    fn chrome_group_urls_require_real_web_hosts_and_reject_local_app_origins() {
        let urls = [
            "https://example.com/ok",
            "HTTP://example.org/path",
            "http://",
            "https://#fragment",
            "https://neuralia-pdf.localhost/viewer.html",
            "http://neuralia-epub.localhost/library",
            "javascript:alert(1)",
            "file:///C:/secret.txt",
            "chrome://settings",
            "https://example.com/with space",
            "https://example.com/with\nnewline",
        ];
        let tabs: Vec<_> = urls
            .iter()
            .map(|url| serde_json::json!({"url":url}))
            .collect();
        let preferences = serde_json::json!({"saved_tab_groups":{"specifics":[{
            "title":"Seguro", "saved_tabs":tabs
        }]}});
        let parsed = groups_from_preferences(&serde_json::to_vec(&preferences).unwrap());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].tabs.len(), 2);
        assert_eq!(parsed[0].tabs[0].url, "HTTP://example.org/path");
        assert_eq!(parsed[0].tabs[1].url, "https://example.com/ok");
    }

    #[test]
    fn chrome_group_protobuf_rejects_overflowing_varints_and_field_numbers() {
        let mut cursor = 0;
        let overflow = [&[0xff; 9][..], &[0x02]].concat();
        assert_eq!(read_varint(&overflow, &mut cursor), None);
        let mut cursor = 0;
        assert_eq!(read_varint(&varint(u64::MAX), &mut cursor), Some(u64::MAX));
        // Antes, o cast para u32 transformava este campo gigante no campo 1.
        let oversized_tag = [varint(((1u64 << 32) | 1) << 3), varint(7)].concat();
        assert!(message(&oversized_tag).is_none());
        assert!(message(&[0x0a, 0xff, 0x7f]).is_none());
    }

    #[test]
    fn chrome_group_reads_and_public_parser_enforce_byte_limits() {
        let root = std::env::temp_dir().join(format!(
            "neuralia-groups-caps-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("large.ldb");
        let mut bytes = wrap(
            "11111111-1111-1111-1111-111111111111",
            &[
                proto_str(1, "11111111-1111-1111-1111-111111111111"),
                delimited(4, &proto_str(2, "Grupo")),
            ]
            .concat(),
        );
        bytes.extend(wrap(
            "22222222-2222-2222-2222-222222222222",
            &[
                proto_str(1, "22222222-2222-2222-2222-222222222222"),
                delimited(
                    5,
                    &[
                        proto_str(1, "11111111-1111-1111-1111-111111111111"),
                        proto_str(3, "https://example.com/ok"),
                    ]
                    .concat(),
                ),
            ]
            .concat(),
        ));
        assert_eq!(parse_saved_tab_groups(&bytes).len(), 1);
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(read_file_capped(&file, bytes.len()), Some(bytes.clone()));
        assert!(read_file_capped(&file, bytes.len() - 1).is_none());
        bytes.resize(MAX_FILE_BYTES + 1, 0);
        assert!(parse_saved_tab_groups(&bytes).is_empty());
        let oversized = File::options().write(true).open(&file).unwrap();
        oversized.set_len((MAX_FILE_BYTES + 1) as u64).unwrap();
        drop(oversized);
        assert!(read_file_capped(&file, MAX_FILE_BYTES).is_none());
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn a_profile_directory_reads_the_log_after_the_ldb() {
        let root = std::env::temp_dir().join(format!("neuralia-groups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let level = root.join("Sync Data").join("LevelDB");
        std::fs::create_dir_all(&level).unwrap();
        let guid = "33333333-3333-3333-3333-333333333333";
        let tab_guid = "44444444-4444-4444-4444-444444444444";
        let group = [proto_str(2, "Do ldb"), proto_var(3, 2)].concat();
        std::fs::write(
            level.join("000001.ldb"),
            wrap(guid, &[proto_str(1, guid), delimited(4, &group)].concat()),
        )
        .unwrap();
        let tab = [
            proto_str(1, guid),
            proto_str(3, "https://example.com/log"),
            proto_str(4, "Log"),
        ]
        .concat();
        std::fs::write(
            level.join("LOG"),
            wrap(
                tab_guid,
                &[proto_str(1, tab_guid), delimited(5, &tab)].concat(),
            ),
        )
        .unwrap();
        let groups = read_profile_tab_groups(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "Do ldb");
        assert_eq!(groups[0].tabs[0].url, "https://example.com/log");
    }
}
