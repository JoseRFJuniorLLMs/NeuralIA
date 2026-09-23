//! Abas e grupos do comparador guardados entre sessoes, em
//! `<data_dir>/tabs.json`.
//!
//! Portatil de proposito: nao ha uma unica chamada ao Windows aqui, so JSON,
//! validacao e ficheiros. Assim o formato, a leitura tolerante e a escrita
//! atomica sao compilados e testados tambem no runner Linux, e o
//! `windows_app.rs` fica so com a ponte entre o modelo da barra e isto.
//!
//! Regras que os testes prendem:
//! - o leitor nunca confia no ficheiro: tamanho com tecto, versao conhecida,
//!   cada aba e cada grupo validados um a um; um ficheiro estragado nao abre
//!   nada, fica guardado ao lado em `tabs.json.bak` e a sessao comeca limpa;
//! - o escritor nunca escreve o que o leitor recusaria (`sanitize` e o mesmo
//!   dos dois lados) e escreve por ficheiro temporario + `rename`, sem deixar
//!   o temporario para tras quando falha;
//! - uma sessao sem abas nao deixa ficheiro nenhum.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// Nome do ficheiro dentro de `data_dir`.
pub const FILE_NAME: &str = "tabs.json";
/// Unica versao que este leitor entende. Um ficheiro de uma versao mais nova
/// (depois de um downgrade) nao e adivinhado: vai para o `.bak`.
pub const VERSION: u64 = 1;
/// Colunas do comparador. O `windows_app.rs` prende isto ao seu
/// `COMPARATOR_COLUMNS` em tempo de compilacao.
pub const COLUMNS: usize = 3;
/// O mesmo limite da poda em `remember_context_tab`: um ficheiro nao traz de
/// volta mais abas do que a sessao viva deixaria existir.
pub const MAX_TABS_PER_COLUMN: usize = 32;
/// URLs maiores nao sao guardadas nem lidas.
pub const MAX_URL_BYTES: usize = 2048;
pub const MAX_TITLE_CHARS: usize = 120;
pub const MAX_GROUP_NAME_CHARS: usize = 64;
const MAX_COLOR_KEY_BYTES: usize = 16;
/// Tecto do ficheiro. O pior caso que o escritor consegue produzir (3 x 32
/// abas com URL e titulo no maximo, cada uma no seu grupo) cabe com folga --
/// ha um teste para isso.
pub const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTab {
    /// Identidade que a aba tinha na sessao que gravou. Na leitura e
    /// renumerada; so serve para o ficheiro ser legivel e consistente.
    pub id: u64,
    pub url: String,
    /// Rotulo mostrado na barra quando foi gravada.
    pub title: String,
    /// Id do grupo (de `SessionColumn::groups`), se a aba esta num.
    pub group: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGroup {
    pub id: u64,
    pub name: String,
    /// Chave da cor (`blue`, `green`, ...). Quem le decide o que fazer com uma
    /// chave que nao conhece; aqui so se garante que e curta e ASCII.
    pub color: String,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionColumn {
    pub tabs: Vec<SessionTab>,
    /// Grupos pela ordem da barra.
    pub groups: Vec<SessionGroup>,
    /// Indice (em `tabs`) da aba que estava aberta ao lado nesta coluna.
    pub active: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabSession {
    pub columns: [SessionColumn; COLUMNS],
}

impl TabSession {
    pub fn is_empty(&self) -> bool {
        self.columns.iter().all(|column| column.tabs.is_empty())
    }
}

/// Porque um ficheiro nao foi aceite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    Oversized(u64),
    NotUtf8,
    Corrupt(String),
    UnknownVersion(Option<u64>),
}

/// Texto para o aviso na interface (pt-BR). O pormenor do `serde_json`, em
/// ingles, fica so no `Debug`, para o log.
impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversized(_) => f.write_str("arquivo grande demais"),
            Self::NotUtf8 => f.write_str("arquivo que não é texto"),
            Self::Corrupt(_) => f.write_str("arquivo danificado"),
            Self::UnknownVersion(Some(version)) => write!(f, "versão {version} desconhecida"),
            Self::UnknownVersion(None) => f.write_str("arquivo sem versão"),
        }
    }
}

/// O que a leitura encontrou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// Nao ha ficheiro: primeira execucao, ou o historico foi apagado.
    Missing,
    Restored(TabSession),
    /// O ficheiro era invalido; foi posto de lado em `tabs.json.bak`.
    Quarantined(LoadError),
    /// Nao se conseguiu ler (permissao, bloqueio). O ficheiro nao e tocado.
    Unreadable(String),
}

pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// Onde fica guardada a copia de um ficheiro recusado.
pub fn backup_path(path: &Path) -> PathBuf {
    sibling(path, ".bak")
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| FILE_NAME.into());
    name.push(suffix);
    path.with_file_name(name)
}

/// Temporario da escrita atomica: no mesmo diretorio (o `rename` nao cruza
/// volumes) e com o pid, para duas instancias nao partilharem o mesmo.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(FILE_NAME);
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

fn is_temp_of(path: &Path, candidate: &str) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(FILE_NAME);
    candidate.starts_with(&format!(".{name}.")) && candidate.ends_with(".tmp")
}

/// Texto de uma pessoa: sem caracteres de controlo (um CR/LF num nome de
/// grupo partia a barra), sem espacos nas pontas, cortado na fronteira de
/// char.
fn clean_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(max_chars)
        .collect()
}

fn clean_url(value: &str) -> Option<String> {
    if value.len() > MAX_URL_BYTES {
        return None;
    }
    // As mesmas regras de quando a aba abre ao lado: http(s), com host, sem
    // credenciais embutidas.
    let url = neural_core::validate_web_url(value.trim()).ok()?;
    let url = url.to_string();
    (url.len() <= MAX_URL_BYTES).then_some(url)
}

fn clean_color(value: &str) -> String {
    let key: String = value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_COLOR_KEY_BYTES)
        .collect::<String>()
        .to_ascii_lowercase();
    if key.is_empty() { "blue".into() } else { key }
}

/// Uma coluna tal como pode ser escrita e lida. E o mesmo filtro na escrita e
/// na leitura: por isso `decode(encode(s)) == sanitize(s)`.
fn sanitize_column(column: &SessionColumn) -> SessionColumn {
    // Abas validas, com o indice original para reencontrar a ativa.
    let mut kept: Vec<(usize, SessionTab)> = column
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(index, tab)| {
            Some((
                index,
                SessionTab {
                    id: tab.id,
                    url: clean_url(&tab.url)?,
                    title: clean_text(&tab.title, MAX_TITLE_CHARS),
                    group: tab.group,
                },
            ))
        })
        .collect();
    // O limite da sessao viva: saem as mais antigas.
    if kept.len() > MAX_TABS_PER_COLUMN {
        kept.drain(..kept.len() - MAX_TABS_PER_COLUMN);
    }

    // Grupos: o primeiro com cada id, e so os que ainda tem abas. Um grupo
    // vazio seria uma pilula fantasma na barra.
    let mut groups: Vec<SessionGroup> = Vec::new();
    for group in &column.groups {
        if groups.iter().any(|seen| seen.id == group.id) {
            continue;
        }
        if !kept.iter().any(|(_, tab)| tab.group == Some(group.id)) {
            continue;
        }
        let name = clean_text(&group.name, MAX_GROUP_NAME_CHARS);
        groups.push(SessionGroup {
            id: group.id,
            name: if name.is_empty() {
                "Grupo".into()
            } else {
                name
            },
            color: clean_color(&group.color),
            collapsed: group.collapsed,
        });
    }
    // Aba num grupo que nao existe volta a ser solta, em vez de desaparecer.
    for (_, tab) in &mut kept {
        if let Some(id) = tab.group
            && !groups.iter().any(|group| group.id == id)
        {
            tab.group = None;
        }
    }

    let active = column
        .active
        .and_then(|original| kept.iter().position(|(index, _)| *index == original));
    SessionColumn {
        tabs: kept.into_iter().map(|(_, tab)| tab).collect(),
        groups,
        active,
    }
}

pub fn sanitize(session: &TabSession) -> TabSession {
    TabSession {
        columns: std::array::from_fn(|index| sanitize_column(&session.columns[index])),
    }
}

pub fn encode(session: &TabSession) -> Vec<u8> {
    let clean = sanitize(session);
    let columns: Vec<Value> = clean
        .columns
        .iter()
        .map(|column| {
            json!({
                "active": column.active,
                "groups": column.groups.iter().map(|group| json!({
                    "id": group.id,
                    "name": group.name,
                    "color": group.color,
                    "collapsed": group.collapsed,
                })).collect::<Vec<_>>(),
                "tabs": column.tabs.iter().map(|tab| json!({
                    "id": tab.id,
                    "title": tab.title,
                    "url": tab.url,
                    "group": tab.group,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let document = json!({ "version": VERSION, "columns": columns });
    // Um Value construido aqui serializa sempre; o vazio so por seguranca.
    serde_json::to_vec_pretty(&document).unwrap_or_default()
}

fn field_u64(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key).and_then(Value::as_u64)
}

fn field_str<'a>(object: &'a Map<String, Value>, key: &str) -> &'a str {
    object.get(key).and_then(Value::as_str).unwrap_or("")
}

fn decode_column(value: &Value) -> SessionColumn {
    // Uma coluna que nao e objeto perde-se sozinha; as outras continuam.
    let Some(object) = value.as_object() else {
        return SessionColumn::default();
    };
    let tabs = object
        .get("tabs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_object().map(|tab| SessionTab {
                        id: field_u64(tab, "id").unwrap_or(0),
                        url: field_str(tab, "url").to_string(),
                        title: field_str(tab, "title").to_string(),
                        group: field_u64(tab, "group"),
                    })
                })
                // Uma entrada estragada ocupa o seu indice como aba invalida,
                // para o `active` continuar a apontar para a aba certa.
                .map(|tab| {
                    tab.unwrap_or(SessionTab {
                        id: 0,
                        url: String::new(),
                        title: String::new(),
                        group: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let groups = object
        .get("groups")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_object)
                .filter_map(|group| {
                    Some(SessionGroup {
                        id: field_u64(group, "id")?,
                        name: field_str(group, "name").to_string(),
                        color: field_str(group, "color").to_string(),
                        collapsed: group
                            .get("collapsed")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let active = field_u64(object, "active").and_then(|index| usize::try_from(index).ok());
    SessionColumn {
        tabs,
        groups,
        active,
    }
}

/// Le o conteudo de um `tabs.json`. Tolerante com o que e da pessoa (fins de
/// linha CRLF de um editor, BOM, campos a mais, uma aba estragada no meio) e
/// intransigente com o que nao se pode adivinhar (tamanho, versao, JSON
/// partido).
pub fn decode(bytes: &[u8]) -> Result<TabSession, LoadError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(LoadError::Oversized(bytes.len() as u64));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| LoadError::NotUtf8)?;
    let document: Value =
        serde_json::from_str(text).map_err(|error| LoadError::Corrupt(error.to_string()))?;
    let Some(root) = document.as_object() else {
        return Err(LoadError::Corrupt("a raiz nao e um objeto".into()));
    };
    let version = field_u64(root, "version");
    if version != Some(VERSION) {
        return Err(LoadError::UnknownVersion(version));
    }
    let Some(columns) = root.get("columns").and_then(Value::as_array) else {
        return Err(LoadError::Corrupt("sem colunas".into()));
    };
    let raw = TabSession {
        columns: std::array::from_fn(|index| {
            columns.get(index).map(decode_column).unwrap_or_default()
        }),
    };
    Ok(sanitize(&raw))
}

/// Le no maximo `MAX_FILE_BYTES + 1` bytes: um ficheiro gigante nunca entra
/// inteiro em memoria so para ser recusado.
fn read_capped(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Poe um ficheiro recusado de lado. Se nem isso der, apaga-o: a proxima
/// gravacao escrevia por cima de qualquer maneira, e um ficheiro que se
/// recusa a cada arranque so atrasava.
fn quarantine(path: &Path) {
    if fs::rename(path, backup_path(path)).is_err() {
        let _ = fs::remove_file(path);
    }
}

pub fn load(path: &Path) -> Loaded {
    let bytes = match read_capped(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Loaded::Missing,
        Err(error) => return Loaded::Unreadable(error.to_string()),
    };
    match decode(&bytes) {
        Ok(session) => Loaded::Restored(session),
        Err(error) => {
            quarantine(path);
            Loaded::Quarantined(error)
        }
    }
}

/// Escreve `bytes` em `path` por um temporario no mesmo diretorio, com
/// `sync` antes do `rename`: quem le ve o ficheiro antigo ou o novo, nunca um
/// meio. `rename` e injetavel so para os testes provarem o caminho de erro.
fn write_atomically(
    path: &Path,
    bytes: &[u8],
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    let result = (|| -> io::Result<()> {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Grava a sessao. Sem abas nao fica ficheiro nenhum -- e assim que "Apagar
/// historico" seguido do fecho do comparador nao volta a criar o que acabou
/// de apagar.
pub fn save(path: &Path, session: &TabSession) -> io::Result<()> {
    let clean = sanitize(session);
    if clean.is_empty() {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
    }
    let bytes = encode(&clean);
    if bytes.len() as u64 > MAX_FILE_BYTES {
        // Nunca escrever o que o leitor recusaria no proximo arranque.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sessão de abas acima do tamanho máximo",
        ));
    }
    write_atomically(path, &bytes, |from, to| fs::rename(from, to))
}

/// Apaga o ficheiro, a copia de um ficheiro recusado e qualquer temporario
/// que uma escrita interrompida tenha deixado. Tudo isto guarda URLs.
pub fn clear(path: &Path) -> io::Result<()> {
    let mut first_error = None;
    let mut remove = |target: &Path| {
        if let Err(error) = fs::remove_file(target)
            && error.kind() != io::ErrorKind::NotFound
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    };
    remove(path);
    remove(&backup_path(path));
    if let Some(parent) = path.parent()
        && let Ok(entries) = fs::read_dir(if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        })
    {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| is_temp_of(path, name))
            {
                remove(&entry.path());
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "neuralia-tabs-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn tab(id: u64, url: &str, group: Option<u64>) -> SessionTab {
        SessionTab {
            id,
            url: url.into(),
            title: format!("aba {id}"),
            group,
        }
    }

    fn group(id: u64, name: &str, color: &str, collapsed: bool) -> SessionGroup {
        SessionGroup {
            id,
            name: name.into(),
            color: color.into(),
            collapsed,
        }
    }

    fn sample() -> TabSession {
        TabSession {
            columns: [
                SessionColumn {
                    tabs: vec![
                        tab(1, "https://example.com/a", Some(7)),
                        tab(2, "https://example.com/b", Some(7)),
                        tab(3, "https://rust-lang.org/", None),
                    ],
                    groups: vec![group(7, "Leitura ç", "green", true)],
                    active: Some(1),
                },
                SessionColumn::default(),
                SessionColumn {
                    tabs: vec![tab(9, "https://docs.rs/serde", Some(8))],
                    groups: vec![group(8, "Docs", "purple", false)],
                    active: None,
                },
            ],
        }
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn tab_session_round_trips_through_disk_with_crlf_and_bom() {
        let dir = temp_dir("roundtrip");
        let path = path_in(&dir);
        let session = sample();

        save(&path, &session).expect("save");
        assert_eq!(load(&path), Loaded::Restored(session.clone()));
        // Nada fica para tras alem do proprio ficheiro.
        assert_eq!(files_in(&dir), vec![FILE_NAME.to_string()]);

        // Um editor no Windows grava CRLF e as vezes um BOM: continua a ler.
        let text = fs::read_to_string(&path).expect("read");
        assert!(!text.contains('\r'));
        let crlf = format!("\u{FEFF}{}", text.replace('\n', "\r\n"));
        fs::write(&path, crlf).expect("write crlf");
        assert_eq!(load(&path), Loaded::Restored(session));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_tab_session_files_start_clean_and_keep_a_backup() {
        let oversized = {
            let mut bytes = br#"{"version":1,"columns":[],"pad":""#.to_vec();
            bytes.resize(MAX_FILE_BYTES as usize + 16, b'x');
            bytes.extend_from_slice(b"\"}");
            bytes
        };
        type Expected = fn(&LoadError) -> bool;
        let cases: [(&str, Vec<u8>, Expected); 5] = [
            (
                "corrupt",
                b"{\"version\":1,\"columns\":[".to_vec(),
                |error| matches!(error, LoadError::Corrupt(_)),
            ),
            (
                "future",
                br#"{"version":2,"columns":[]}"#.to_vec(),
                |error| *error == LoadError::UnknownVersion(Some(2)),
            ),
            ("unversioned", br#"{"columns":[]}"#.to_vec(), |error| {
                *error == LoadError::UnknownVersion(None)
            }),
            ("binary", vec![0xFF, 0xFE, 0x00, 0x7B], |error| {
                *error == LoadError::NotUtf8
            }),
            ("oversized", oversized, |error| {
                matches!(error, LoadError::Oversized(_))
            }),
        ];
        for (label, bytes, expected) in cases {
            let dir = temp_dir(label);
            let path = path_in(&dir);
            fs::write(&path, &bytes).expect("write bad file");
            match load(&path) {
                Loaded::Quarantined(error) => assert!(expected(&error), "{label}: {error:?}"),
                other => panic!("{label}: a bad file must be quarantined, got {other:?}"),
            }
            assert!(!path.exists(), "{label}: the bad file is still in place");
            assert_eq!(
                fs::read(backup_path(&path)).expect("backup"),
                bytes,
                "{label}: the backup must be the bad file, byte for byte"
            );
            // Comecar limpo: a proxima leitura ja nao encontra nada.
            assert_eq!(load(&path), Loaded::Missing, "{label}");
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_tab_session_file_is_trusted_entry_by_entry() {
        let text = r#"{
            "version": 1,
            "extra": "campo de uma versao futura",
            "columns": [
                {
                    "active": 3,
                    "groups": [
                        {"id": 5, "name": "Linha\r\nquebrada", "color": "Blue!!", "collapsed": true},
                        {"id": 5, "name": "duplicado", "color": "pink"},
                        {"id": 6, "name": "sem abas", "color": "green"}
                    ],
                    "tabs": [
                        {"id": 1, "url": "javascript:alert(1)", "group": 5},
                        "nao e objeto",
                        {"id": 2, "url": "https://user:secret@example.com/", "group": 5},
                        {"id": 3, "url": "https://example.com/ok", "group": 5},
                        {"id": 4, "url": "https://example.com/solta", "group": 99}
                    ]
                },
                "coluna estragada"
            ]
        }"#;
        let session = decode(text.as_bytes()).expect("tolerant decode");
        let first = &session.columns[0];
        assert_eq!(
            first
                .tabs
                .iter()
                .map(|tab| tab.url.as_str())
                .collect::<Vec<_>>(),
            vec!["https://example.com/ok", "https://example.com/solta"],
            "script, credentials and non-objects never come back"
        );
        assert_eq!(
            first.active,
            Some(0),
            "active follows its tab, not its index"
        );
        assert_eq!(
            first.groups.len(),
            1,
            "duplicate and empty groups are dropped"
        );
        assert_eq!(first.groups[0].name, "Linhaquebrada");
        assert_eq!(first.groups[0].color, "blue");
        assert_eq!(
            first.tabs[1].group, None,
            "a missing group leaves the tab loose"
        );
        assert!(session.columns[1].tabs.is_empty());
        assert!(session.columns[2].tabs.is_empty());
    }

    #[test]
    fn a_column_never_restores_more_tabs_than_the_live_limit() {
        let mut column = SessionColumn::default();
        for id in 0..40u64 {
            column
                .tabs
                .push(tab(id, &format!("https://example.com/{id}"), None));
        }
        column.active = Some(2);
        let session = TabSession {
            columns: [column, SessionColumn::default(), SessionColumn::default()],
        };
        let back = decode(&encode(&session)).expect("decode");
        let tabs = &back.columns[0].tabs;
        assert_eq!(tabs.len(), MAX_TABS_PER_COLUMN);
        // As mais antigas saem, como em `remember_context_tab`.
        assert_eq!(tabs[0].id, 8);
        assert_eq!(tabs.last().map(|tab| tab.id), Some(39));
        // A ativa era uma das podadas: nao aponta para outra aba qualquer.
        assert_eq!(back.columns[0].active, None);
    }

    #[test]
    fn the_largest_session_the_writer_can_produce_is_still_readable() {
        let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES - 20));
        assert_eq!(long_url.len(), MAX_URL_BYTES);
        let column = SessionColumn {
            tabs: (0..MAX_TABS_PER_COLUMN as u64)
                .map(|id| SessionTab {
                    id: u64::MAX - id,
                    url: long_url.clone(),
                    title: "\u{1F600}".repeat(MAX_TITLE_CHARS),
                    group: Some(u64::MAX - id),
                })
                .collect(),
            groups: (0..MAX_TABS_PER_COLUMN as u64)
                .map(|id| {
                    group(
                        u64::MAX - id,
                        &"é".repeat(MAX_GROUP_NAME_CHARS),
                        "slate",
                        true,
                    )
                })
                .collect(),
            active: Some(0),
        };
        let session = TabSession {
            columns: [column.clone(), column.clone(), column],
        };
        let bytes = encode(&session);
        assert!(
            (bytes.len() as u64) <= MAX_FILE_BYTES,
            "writer produced {} bytes, above the reader cap",
            bytes.len()
        );
        assert_eq!(decode(&bytes).expect("decode"), sanitize(&session));

        let dir = temp_dir("largest");
        let path = path_in(&dir);
        save(&path, &session).expect("save");
        assert_eq!(load(&path), Loaded::Restored(sanitize(&session)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_when_it_fails() {
        // 1. O rename falha de verdade: o destino e um diretorio.
        let dir = temp_dir("atomic-dir");
        let path = path_in(&dir);
        fs::create_dir_all(&path).expect("directory in the way");
        assert!(save(&path, &sample()).is_err());
        assert!(
            path.is_dir(),
            "the failed write must not replace what was there"
        );
        assert_eq!(
            files_in(&dir),
            vec![FILE_NAME.to_string()],
            "temp left behind"
        );
        let _ = fs::remove_dir_all(&dir);

        // 2. O rename falha a meio de uma substituicao: o ficheiro antigo
        //    fica intacto e o temporario desaparece.
        let dir = temp_dir("atomic-old");
        let path = path_in(&dir);
        save(&path, &sample()).expect("first save");
        let before = fs::read(&path).expect("read");
        let mut changed = sample();
        changed.columns[1]
            .tabs
            .push(tab(50, "https://example.org/", None));
        let result = write_atomically(&path, &encode(&changed), |_, _| {
            Err(io::Error::other("disco cheio"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).expect("read"), before, "old file damaged");
        assert_eq!(
            files_in(&dir),
            vec![FILE_NAME.to_string()],
            "temp left behind"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_session_leaves_no_file_and_clear_removes_every_copy() {
        let dir = temp_dir("clear");
        let path = path_in(&dir);
        save(&path, &sample()).expect("save");
        fs::write(backup_path(&path), b"old bad file").expect("bak");
        fs::write(temp_path(&path), b"interrupted").expect("tmp");
        fs::write(dir.join(".tabs.json.1.tmp"), b"other pid").expect("tmp 2");
        fs::write(dir.join("history.jsonl"), b"not ours").expect("other");

        clear(&path).expect("clear");
        assert_eq!(files_in(&dir), vec!["history.jsonl".to_string()]);
        assert_eq!(load(&path), Loaded::Missing);

        save(&path, &sample()).expect("save");
        save(&path, &TabSession::default()).expect("save empty");
        assert!(!path.exists(), "an empty session must not leave a file");
        let _ = fs::remove_dir_all(&dir);
    }
}
