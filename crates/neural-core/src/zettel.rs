//! Zettelkasten local: uma nota por ficheiro Markdown, `<dir>/<id>.md`, com um
//! front matter pequeno que o Obsidian lê como propriedades.
//!
//! O nome do ficheiro é a chave. O `id:` do front matter é escrito para quem
//! abre a pasta noutro editor, mas na leitura manda o nome do ficheiro: é por
//! ele que `get`, `save` e `delete` chegam ao disco, e é ele que se valida.
//!
//! O tempo entra sempre por argumento (`now_unix`): o módulo nunca lê o
//! relógio, por isso ids e ordenação são reprodutíveis.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
    str::Chars,
    sync::atomic::{AtomicU64, Ordering},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Subpasta para onde `delete` move as notas (o mesmo nome da lixeira do Obsidian).
pub const TRASH_DIR: &str = ".trash";

const NOTE_EXT: &str = "md";
/// Chega para `YYYYMMDDHHMMSS-N` com folga e trava nomes absurdos.
const MAX_ID_LEN: usize = 64;
/// Candidatos tentados num minuto antes de desistir.
const MAX_ID_ATTEMPTS: usize = 10_000;

// Pesos da busca: o teto do corpo fica abaixo de uma tag, e uma tag abaixo do
// título, para que repetir uma palavra no texto nunca passe à frente de quem a
// tem no nome.
const TITLE_WEIGHT: u64 = 100;
const TAG_WEIGHT: u64 = 30;
const BODY_HIT_WEIGHT: u64 = 1;
const BODY_HITS_CAP: u64 = 10;

static TEMP_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum ZettelError {
    /// Id com algo além de dígitos e `-`: nunca chega a virar caminho.
    #[error("id de nota inválido: {0:?}")]
    InvalidId(String),
    #[error("nota não encontrada: {0}")]
    NotFound(String),
    #[error("a nota {0} não está em UTF-8")]
    NotUtf8(String),
    #[error("sem id livre no minuto {0}")]
    IdsExhausted(String),
    #[error("falha de E/S: {0}")]
    Io(#[from] io::Error),
}

pub type ZettelResult<T> = std::result::Result<T, ZettelError>;

/// Uma nota. `body` é Markdown com fins de linha LF.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// `YYYYMMDDHHMM` (UTC); `YYYYMMDDHHMMSS` e depois `...SS-2`, `-3`...
    /// quando o minuto já tem nota.
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    /// URL de onde a nota veio.
    pub source: Option<String>,
    pub created_unix: u64,
    pub updated_unix: u64,
    /// Linhas cruas do front matter com chaves que este módulo não conhece
    /// (`aliases`, `cssclasses`...). Voltam ao ficheiro tal como vieram: sem
    /// isto um `save` apagava as propriedades que o Obsidian lá escreveu.
    #[serde(default)]
    pub extra_front_matter: Vec<String>,
}

impl Note {
    /// Texto do ficheiro: front matter + corpo.
    pub fn to_markdown(&self) -> String {
        let mut out = String::with_capacity(self.body.len() + 160);
        out.push_str("---\n");
        // Um id válido é só dígitos e `-`; um inválido sai entre aspas em vez
        // de partir o YAML.
        let id = if is_valid_note_id(&self.id) {
            self.id.clone()
        } else {
            yaml_quoted(&self.id)
        };
        push_field(&mut out, "id", &id);
        push_field(&mut out, "title", &yaml_scalar(&self.title, false));
        let tags: Vec<String> = self.tags.iter().map(|tag| yaml_scalar(tag, true)).collect();
        push_field(&mut out, "tags", &format!("[{}]", tags.join(", ")));
        if let Some(source) = &self.source {
            push_field(&mut out, "source", &yaml_scalar(source, false));
        }
        push_field(&mut out, "created", &self.created_unix.to_string());
        push_field(&mut out, "updated", &self.updated_unix.to_string());
        for line in clean_extra_lines(&self.extra_front_matter) {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("---\n");
        out.push_str(&normalize_newlines(&self.body));
        out
    }

    /// Lê o texto de um ficheiro; `id` é o nome dele sem `.md`. Nunca falha:
    /// front matter sem fecho conta como corpo e o que faltar recebe valores de
    /// recurso (título do primeiro `# `, senão o próprio id; `created` do id).
    pub fn from_markdown(id: &str, text: &str) -> Note {
        parse_note(id, text).0
    }
}

/// Ids de nota citados em `[[id]]` / `[[id|alias]]` no corpo, pela ordem e
/// sem repetidos. Links para nomes que não são ids e links dentro de código
/// (em linha ou em bloco) ficam de fora.
pub fn links(note: &Note) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in note.body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for target in wiki_targets(line) {
            if is_valid_note_id(target) && seen.insert(target) {
                out.push(target.to_string());
            }
        }
    }
    out
}

/// Notas que ligam para `id` (a própria nota não conta), na ordem de `list`.
pub fn backlinks(store: &ZettelStore, id: &str) -> ZettelResult<Vec<Note>> {
    if !is_valid_note_id(id) {
        return Err(ZettelError::InvalidId(id.to_string()));
    }
    Ok(store
        .list()?
        .into_iter()
        .filter(|note| note.id != id && links(note).iter().any(|target| target == id))
        .collect())
}

/// Id do minuto UTC de `unix`: `YYYYMMDDHHMM`.
pub fn note_id_for(unix: u64) -> String {
    // u64::MAX / 86_400 cabe em i64: o cast não perde nada.
    let (year, month, day) = civil_from_days((unix / 86_400) as i64);
    let second_of_day = unix % 86_400;
    format!(
        "{year:04}{month:02}{day:02}{:02}{:02}",
        second_of_day / 3600,
        second_of_day % 3600 / 60
    )
}

/// Só dígitos e `-`, a começar por dígito. É isto que impede um id como
/// `../../x` (ou `C:\x`) de sair da pasta quando vira nome de ficheiro.
pub fn is_valid_note_id(id: &str) -> bool {
    id.len() <= MAX_ID_LEN
        && id.starts_with(|c: char| c.is_ascii_digit())
        && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

/// Pasta de notas. Não guarda nada em memória: cada chamada lê o disco, por
/// isso duas instâncias (duas janelas) sobre a mesma pasta não divergem.
#[derive(Debug, Clone)]
pub struct ZettelStore {
    dir: PathBuf,
}

impl ZettelStore {
    /// Abre (e cria, se preciso) a pasta de notas.
    pub fn open(dir: impl Into<PathBuf>) -> ZettelResult<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `<dir>/<id>.md`, depois de validar o id.
    pub fn note_path(&self, id: &str) -> ZettelResult<PathBuf> {
        if !is_valid_note_id(id) {
            return Err(ZettelError::InvalidId(id.to_string()));
        }
        Ok(self.dir.join(format!("{id}.{NOTE_EXT}")))
    }

    /// Cria uma nota com id do minuto de `now_unix` (ver [`Note::id`]).
    pub fn create(
        &self,
        title: &str,
        body: &str,
        tags: Vec<String>,
        source: Option<String>,
        now_unix: u64,
    ) -> ZettelResult<Note> {
        let (id, path) = self.reserve_id(now_unix)?;
        let note = normalized(Note {
            id,
            title: title.to_string(),
            body: body.to_string(),
            tags,
            source,
            created_unix: now_unix,
            updated_unix: now_unix,
            extra_front_matter: Vec::new(),
        });
        if let Err(error) = self.write_atomically(&path, &note.to_markdown()) {
            // Devolve o id reservado: um ficheiro vazio ali seria uma nota fantasma.
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(note)
    }

    /// Grava `note` com `updated_unix = now_unix` e devolve a versão gravada
    /// (normalizada: título numa linha, corpo em LF, tags sem `#` nem repetidas).
    pub fn save(&self, note: &Note, now_unix: u64) -> ZettelResult<Note> {
        let path = self.note_path(&note.id)?;
        let mut saved = normalized(note.clone());
        saved.updated_unix = now_unix;
        if saved.created_unix == 0 {
            saved.created_unix = now_unix;
        }
        self.write_atomically(&path, &saved.to_markdown())?;
        Ok(saved)
    }

    /// `Ok(None)` quando a nota não existe; `NotUtf8` quando o ficheiro não é texto.
    pub fn get(&self, id: &str) -> ZettelResult<Option<Note>> {
        let path = self.note_path(id)?;
        self.read_note(id, &path)
    }

    /// Todas as notas, `updated_unix` mais recente primeiro. Ficheiros que não
    /// são notas (outro nome, outra extensão, bytes que não são UTF-8) ficam de
    /// fora em silêncio: um ficheiro estranho não pode esconder os outros.
    pub fn list(&self) -> ZettelResult<Vec<Note>> {
        let mut notes = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            let Some(id) = note_id_of(&path) else {
                continue;
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            if let Ok(Some(note)) = self.read_note(&id, &path) {
                notes.push(note);
            }
        }
        notes.sort_by(|a, b| {
            b.updated_unix
                .cmp(&a.updated_unix)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(notes)
    }

    /// Move a nota para `<dir>/.trash/` (recuperável, como no Obsidian) e
    /// devolve o caminho lá dentro.
    pub fn delete(&self, id: &str) -> ZettelResult<PathBuf> {
        let path = self.note_path(id)?;
        if !path.is_file() {
            return Err(ZettelError::NotFound(id.to_string()));
        }
        let trash = self.dir.join(TRASH_DIR);
        fs::create_dir_all(&trash)?;
        // `rename` substitui o destino em silêncio: uma nota apagada antes com
        // o mesmo id (os ids reciclam-se depois de um delete) não pode ser esmagada.
        let mut target = trash.join(format!("{id}.{NOTE_EXT}"));
        let mut copy = 2;
        while target.exists() {
            target = trash.join(format!("{id}~{copy}.{NOTE_EXT}"));
            copy += 1;
        }
        match fs::rename(&path, &target) {
            Ok(()) => Ok(target),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(ZettelError::NotFound(id.to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Notas em que todos os termos aparecem (título, tags ou corpo), sem
    /// distinguir maiúsculas nem acentos. Título pesa mais que tag, tag mais que
    /// corpo; empates ficam na ordem de `list`. Consulta vazia devolve nada.
    pub fn search(&self, query: &str) -> ZettelResult<Vec<Note>> {
        let terms = search_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut scored: Vec<(u64, Note)> = self
            .list()?
            .into_iter()
            .filter_map(|note| Some((score(&note, &terms)?, note)))
            .collect();
        // Ordenação estável: os empates mantêm a ordem de `list`.
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        Ok(scored.into_iter().map(|(_, note)| note).collect())
    }

    /// Escolhe o primeiro id livre do minuto e reserva-o com `create_new`, que
    /// é atómico: duas janelas (ou dois processos) a criar no mesmo segundo
    /// nunca ficam com o mesmo id nem se esmagam.
    fn reserve_id(&self, now_unix: u64) -> ZettelResult<(String, PathBuf)> {
        let minute = note_id_for(now_unix);
        let with_seconds = format!("{minute}{:02}", now_unix % 60);
        let candidates = [minute.clone(), with_seconds.clone()]
            .into_iter()
            .chain((2..).map(|n| format!("{with_seconds}-{n}")))
            .take(MAX_ID_ATTEMPTS);
        for id in candidates {
            let path = self.note_path(&id)?;
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok((id, path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ZettelError::IdsExhausted(minute))
    }

    /// Temporário na mesma pasta (mesmo volume, `rename` atómico) e só depois
    /// o `rename` por cima: um corte a meio deixa a versão anterior inteira.
    fn write_atomically(&self, target: &Path, content: &str) -> ZettelResult<()> {
        let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .dir
            .join(format!(".zettel-{}-{nonce}.tmp", std::process::id()));
        let result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        Ok(result?)
    }

    fn read_note(&self, id: &str, path: &Path) -> ZettelResult<Option<Note>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let text = String::from_utf8(bytes).map_err(|_| ZettelError::NotUtf8(id.to_string()))?;
        let (mut note, has_updated) = parse_note(id, &text);
        // Nota escrita fora daqui sem `updated`: a data do ficheiro é o melhor
        // que há para a ordenação.
        if !has_updated && let Some(modified) = modified_unix(path) {
            note.updated_unix = modified;
        }
        Ok(Some(note))
    }
}

/// `<dir>/<id>.md` -> `id`, só para ids válidos.
fn note_id_of(path: &Path) -> Option<String> {
    if path.extension()?.to_str()? != NOTE_EXT {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    is_valid_note_id(stem).then(|| stem.to_string())
}

fn modified_unix(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_secs())
}

// ---------------------------------------------------------------------------
// Normalização: a mesma para o que se grava e para o que se lê, e idempotente,
// para que ler o que se gravou devolva exatamente a mesma nota.
// ---------------------------------------------------------------------------

fn normalized(mut note: Note) -> Note {
    note.body = normalize_newlines(&note.body);
    note.title = Some(normalize_line(&note.title))
        .filter(|title| !title.is_empty())
        .or_else(|| first_heading(&note.body))
        .unwrap_or_else(|| note.id.clone());
    note.tags = normalize_tags(std::mem::take(&mut note.tags));
    note.source = note
        .source
        .as_deref()
        .map(normalize_line)
        .filter(|source| !source.is_empty());
    note.extra_front_matter = clean_extra_lines(&note.extra_front_matter);
    note
}

/// CRLF -> LF. Tira todos os `\r` colados a um `\n` (não só um), para que
/// normalizar duas vezes dê o mesmo que uma.
fn normalize_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut segments = text.split('\n').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_some() {
            out.push_str(segment.trim_end_matches('\r'));
            out.push('\n');
        } else {
            out.push_str(segment);
        }
    }
    out
}

fn normalize_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ").trim().to_string()
}

/// Sem `#` à frente (quem escreve `#rust` quer a tag `rust`), sem vazias, sem repetidas.
fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    tags.into_iter()
        .filter_map(|tag| {
            let tag =
                normalize_line(tag.trim_start_matches(|c: char| c == '#' || c.is_whitespace()));
            (!tag.is_empty() && seen.insert(tag.clone())).then_some(tag)
        })
        .collect()
}

/// Uma linha `---` fecharia o bloco a meio e o resto viraria corpo.
fn clean_extra_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|line| line.split('\n'))
        .map(|line| line.trim_end_matches('\r').to_string())
        .filter(|line| !is_front_matter_fence(line))
        .collect()
}

fn first_heading(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let heading = normalize_line(line.strip_prefix("# ")?.trim().trim_end_matches('#'));
        (!heading.is_empty()).then_some(heading)
    })
}

// ---------------------------------------------------------------------------
// Leitura
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FrontMatter {
    title: Option<String>,
    tags: Vec<String>,
    source: Option<String>,
    created: Option<u64>,
    updated: Option<u64>,
    extra: Vec<String>,
}

/// Dona das linhas indentadas (`  - item`) que vêm depois de uma chave.
#[derive(Clone, Copy)]
enum Owner {
    Known,
    Tags,
    Extra,
}

/// `(nota, o ficheiro trazia updated)`.
fn parse_note(id: &str, text: &str) -> (Note, bool) {
    let text = normalize_newlines(text);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let (front, body) = split_front_matter(text);
    let front = front.map(parse_front_matter).unwrap_or_default();
    let created_unix = front.created.or_else(|| unix_from_id(id)).unwrap_or(0);
    let note = normalized(Note {
        id: id.to_string(),
        title: front.title.unwrap_or_default(),
        body: body.to_string(),
        tags: front.tags,
        source: front.source,
        created_unix,
        updated_unix: front.updated.unwrap_or(created_unix),
        extra_front_matter: front.extra,
    });
    (note, front.updated.is_some())
}

/// `(front matter, corpo)`. Sem `---` a abrir e a fechar, é tudo corpo.
fn split_front_matter(text: &str) -> (Option<&str>, &str) {
    let Some((first, rest)) = text.split_once('\n') else {
        return (None, text);
    };
    if first.trim_end() != "---" {
        return (None, text);
    }
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if is_front_matter_fence(line.strip_suffix('\n').unwrap_or(line)) {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, text)
}

fn is_front_matter_fence(line: &str) -> bool {
    matches!(line.trim_end(), "---" | "...")
}

/// YAML suficiente para o que escrevemos e para o que o Obsidian escreve;
/// não é um parser de YAML e não finge ser.
fn parse_front_matter(front: &str) -> FrontMatter {
    let mut fm = FrontMatter::default();
    let mut owner = Owner::Known;
    for line in front.lines() {
        if line.trim().is_empty() || line.starts_with([' ', '\t', '-']) {
            match owner {
                Owner::Tags => {
                    if let Some(item) = line.trim_start().strip_prefix('-') {
                        fm.tags.push(parse_scalar(item));
                    }
                }
                Owner::Extra => fm.extra.push(line.to_string()),
                Owner::Known => {}
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            // Não é `chave: valor` (comentário, lixo): não é nosso, volta como veio.
            fm.extra.push(line.to_string());
            owner = Owner::Extra;
            continue;
        };
        owner = Owner::Known;
        match key.trim().to_ascii_lowercase().as_str() {
            "id" => {}
            "title" => fm.title = Some(parse_scalar(value)),
            "tags" | "tag" => {
                fm.tags = parse_inline_tags(value);
                owner = Owner::Tags;
            }
            "source" => fm.source = Some(parse_scalar(value)),
            "created" => fm.created = parse_timestamp(&parse_scalar(value)),
            "updated" => fm.updated = parse_timestamp(&parse_scalar(value)),
            _ => {
                fm.extra.push(line.to_string());
                owner = Owner::Extra;
            }
        }
    }
    fm
}

/// `[a, "b:c"]`, `a, b`, `"a"` — ou nada, quando a lista vem em linhas `- x`.
fn parse_inline_tags(value: &str) -> Vec<String> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('[') {
        split_flow_items(inner)
            .into_iter()
            .map(parse_scalar)
            .collect()
    } else if value.starts_with(['"', '\'']) {
        vec![parse_scalar(value)]
    } else {
        strip_comment(value)
            .split(',')
            .map(str::to_string)
            .collect()
    }
}

/// Itens de uma lista `[...]` (sem o `[`), respeitando aspas. Sem `]` a
/// fechar, vai até ao fim.
fn split_flow_items(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, ch) in inner.char_indices() {
        match quote {
            Some('"') => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    quote = None;
                }
            }
            Some(_) => {
                if ch == '\'' {
                    quote = None;
                }
            }
            None => match ch {
                // Aspa só abre citação no início do item; a meio é um caractere.
                '"' | '\'' if inner[start..index].trim().is_empty() => quote = Some(ch),
                ',' => {
                    items.push(&inner[start..index]);
                    start = index + 1;
                }
                ']' => {
                    items.push(&inner[start..index]);
                    return items;
                }
                _ => {}
            },
        }
    }
    items.push(&inner[start..]);
    items
}

fn parse_scalar(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        parse_double_quoted(rest)
    } else if let Some(rest) = raw.strip_prefix('\'') {
        parse_single_quoted(rest)
    } else {
        strip_comment(raw).trim_end().to_string()
    }
}

/// `#` só abre comentário depois de espaço: `https://x/#y` fica inteiro.
fn strip_comment(raw: &str) -> &str {
    match raw.find(" #").or_else(|| raw.find("\t#")) {
        Some(index) => &raw[..index],
        None => raw,
    }
}

/// Sem aspa de fecho, fica o que veio: tolerante, nunca pânico.
fn parse_double_quoted(rest: &str) -> String {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return out,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some('x') => push_escaped_code(&mut out, &mut chars, 2),
                Some('u') => push_escaped_code(&mut out, &mut chars, 4),
                Some('U') => push_escaped_code(&mut out, &mut chars, 8),
                Some(other) => out.push(other),
                None => out.push('\\'),
            },
            other => out.push(other),
        }
    }
    out
}

/// `\xHH`, `\uHHHH`, `\UHHHHHHHH`. Sem dígitos ou com um código que não é
/// char, fica U+FFFD.
fn push_escaped_code(out: &mut String, chars: &mut Chars<'_>, digits: usize) {
    let mut value: u32 = 0;
    let mut read = 0;
    while read < digits {
        let Some(digit) = chars.clone().next().and_then(|c| c.to_digit(16)) else {
            break;
        };
        chars.next();
        // Oito dígitos hex cabem exatamente em u32.
        value = value * 16 + digit;
        read += 1;
    }
    let decoded = (read > 0).then(|| char::from_u32(value)).flatten();
    out.push(decoded.unwrap_or(char::REPLACEMENT_CHARACTER));
}

fn parse_single_quoted(rest: &str) -> String {
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\'' {
            out.push(ch);
        } else if chars.peek() == Some(&'\'') {
            chars.next();
            out.push('\'');
        } else {
            return out;
        }
    }
    out
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let value = value.trim();
    value
        .parse::<u64>()
        .ok()
        .or_else(|| parse_iso_datetime(value))
}

/// `2026-09-23`, `2026-09-23T04:12` ou `2026-09-23 04:12:05`, lidos como UTC
/// (é o que os modelos do Obsidian costumam escrever); o resto é ignorado.
fn parse_iso_datetime(value: &str) -> Option<u64> {
    let at = |index: usize, expected: u8| value.as_bytes().get(index) == Some(&expected);
    if !(at(4, b'-') && at(7, b'-')) {
        return None;
    }
    let (year, month, day) = (
        digits(value, 0..4)?,
        digits(value, 5..7)?,
        digits(value, 8..10)?,
    );
    let (mut hour, mut minute, mut second) = (0, 0, 0);
    if (at(10, b'T') || at(10, b' ')) && at(13, b':') {
        hour = digits(value, 11..13)?;
        minute = digits(value, 14..16)?;
        if at(16, b':') {
            second = digits(value, 17..19)?;
        }
    }
    unix_from_parts(year, month, day, hour, minute, second)
}

/// `created` de recurso para notas sem front matter: o próprio id é a data.
fn unix_from_id(id: &str) -> Option<u64> {
    let stamp = id.split('-').next()?;
    let second = match stamp.len() {
        12 => 0,
        14 => digits(stamp, 12..14)?,
        _ => return None,
    };
    unix_from_parts(
        digits(stamp, 0..4)?,
        digits(stamp, 4..6)?,
        digits(stamp, 6..8)?,
        digits(stamp, 8..10)?,
        digits(stamp, 10..12)?,
        second,
    )
}

fn digits(text: &str, range: Range<usize>) -> Option<u32> {
    let part = text.get(range)?;
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

fn unix_from_parts(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<u64> {
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    // 31/02 passa nos limites acima; a volta pelo calendário apanha-o.
    if civil_from_days(days) != (i64::from(year), month, day) {
        return None;
    }
    let days = u64::try_from(days).ok()?;
    Some(days * 86_400 + u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second))
}

/// Dias desde 1970-01-01, calendário gregoriano proléptico (Howard Hinnant).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = (if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    }) as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

// ---------------------------------------------------------------------------
// Escrita
// ---------------------------------------------------------------------------

fn push_field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

/// Escalar sem aspas quando o YAML o leria como o mesmo texto; senão, entre aspas.
fn yaml_scalar(value: &str, in_flow: bool) -> String {
    if is_plain_safe(value, in_flow) {
        value.to_string()
    } else {
        yaml_quoted(value)
    }
}

/// Conservador de propósito: na dúvida, aspas. Começar por dígito também leva
/// aspas, senão o Obsidian mostrava `2026-09-23` como data e `123` como número.
fn is_plain_safe(value: &str, in_flow: bool) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    if !first.is_alphanumeric() || first.is_ascii_digit() {
        return false;
    }
    if value.ends_with(|c: char| c.is_whitespace() || c == ':')
        || value.contains(": ")
        || value.contains(" #")
    {
        return false;
    }
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '"' | '\'' | '\\'))
    {
        return false;
    }
    if in_flow && value.contains([',', '[', ']', '{', '}']) {
        return false;
    }
    !matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "y" | "n" | "null"
    )
}

fn yaml_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Os controlos (Cc) acabam em U+009F: quatro dígitos chegam.
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Links e busca
// ---------------------------------------------------------------------------

/// Alvos de `[[...]]` numa linha, saltando código em linha (`` `[[x]]` `` é texto).
fn wiki_targets(line: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let mut rest = line;
    while let Some(link) = rest.find("[[") {
        if let Some(code) = rest.find('`').filter(|&code| code < link) {
            let run = rest[code..].bytes().take_while(|&b| b == b'`').count();
            let after = &rest[code + run..];
            // Crase sem par não abre código: continua logo a seguir a ela.
            rest = match after.find(&rest[code..code + run]) {
                Some(close) => &after[close + run..],
                None => after,
            };
            continue;
        }
        let after = &rest[link + 2..];
        let Some(close) = after.find("]]") else { break };
        let inner = &after[..close];
        if inner.contains('[') {
            // `[[a [[b]]`: o primeiro `[[` não fecha; recomeça no seguinte.
            rest = after;
            continue;
        }
        targets.push(link_target(inner));
        rest = &after[close + 2..];
    }
    targets
}

/// `id|alias`, `id#secção`, `id^bloco` -> `id`. O `\|` é como o Obsidian
/// escapa a barra dentro de tabelas.
fn link_target(inner: &str) -> &str {
    let target = inner.split('|').next().unwrap_or(inner);
    let target = target.split(['#', '^']).next().unwrap_or(target);
    target.trim().trim_end_matches('\\').trim_end()
}

fn search_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    fold(query)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty() && seen.insert(term.to_string()))
        .map(str::to_string)
        .collect()
}

fn score(note: &Note, terms: &[String]) -> Option<u64> {
    let title = fold(&note.title);
    let tags: Vec<String> = note.tags.iter().map(|tag| fold(tag)).collect();
    let body = fold(&note.body);
    let mut total = 0;
    for term in terms {
        let mut term_score = 0;
        if title.contains(term.as_str()) {
            term_score += TITLE_WEIGHT;
        }
        if tags.iter().any(|tag| tag.contains(term.as_str())) {
            term_score += TAG_WEIGHT;
        }
        let hits = body.matches(term.as_str()).count() as u64;
        term_score += hits.min(BODY_HITS_CAP) * BODY_HIT_WEIGHT;
        if term_score == 0 {
            return None;
        }
        total += term_score;
    }
    Some(total)
}

/// Minúsculas sem acentos: "Ação" -> "acao". Tabela para o Latin-1 e o Latin
/// Extended-A, e as marcas combinantes caem (texto em NFD, como o que o macOS
/// grava), sem depender de uma crate de normalização Unicode.
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ('\u{300}'..='\u{36f}').contains(&ch) {
            continue;
        }
        match base_letter(ch) {
            Some(base) => out.push_str(base),
            None => out.push(ch),
        }
    }
    out
}

fn base_letter(ch: char) -> Option<&'static str> {
    Some(match ch {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'þ' => "th",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-23T04:12:05Z, calculado à mão (20719 dias * 86400 + 15125 s).
    const T0: u64 = 1_790_136_725;

    static DIR_NONCE: AtomicU64 = AtomicU64::new(1);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neuralia-zettel-{name}-{}-{}",
                std::process::id(),
                DIR_NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn store(&self) -> ZettelStore {
            ZettelStore::open(self.0.join("notas")).unwrap()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn ids(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|note| note.id.as_str()).collect()
    }

    fn note_with_body(id: &str, body: &str) -> Note {
        Note {
            id: id.to_string(),
            title: id.to_string(),
            body: body.to_string(),
            ..Note::default()
        }
    }

    #[test]
    fn id_is_the_utc_minute_of_the_injected_time() {
        assert_eq!(note_id_for(T0), "202609230412");
        // 2024-02-29T23:59:59Z: dia bissexto, último segundo do dia.
        assert_eq!(note_id_for(1_709_251_199), "202402292359");
        assert_eq!(note_id_for(0), "197001010000");
        assert!(is_valid_note_id(&note_id_for(u64::MAX)));
        assert_eq!(unix_from_id("202609230412"), Some(T0 - 5));
        assert_eq!(unix_from_id("20260923041205-7"), Some(T0));
        assert_eq!(unix_from_id("202602300000"), None, "30 de fevereiro");
    }

    #[test]
    fn create_save_get_round_trip() {
        let dir = TempDir::new("round-trip");
        let store = ZettelStore::open(dir.path().join("cofre").join("sub")).unwrap();
        assert!(store.dir().is_dir(), "open cria a pasta");

        let note = store
            .create(
                "Rust: ownership \"move\"",
                "Corpo com [[202601010000]].\n",
                vec!["rust".into(), "#linguagens".into(), "rust".into()],
                Some("https://exemplo.pt/a?b=1#sec".into()),
                T0,
            )
            .unwrap();
        assert_eq!(note.id, "202609230412");
        assert_eq!((note.created_unix, note.updated_unix), (T0, T0));
        assert_eq!(note.tags, ["rust", "linguagens"]);

        let text = fs::read_to_string(store.dir().join("202609230412.md")).unwrap();
        assert!(text.starts_with("---\nid: 202609230412\n"), "{text}");
        assert!(text.contains("\ntags: [rust, linguagens]\n"), "{text}");
        assert!(
            text.ends_with("---\nCorpo com [[202601010000]].\n"),
            "{text}"
        );
        assert_eq!(store.get(&note.id).unwrap(), Some(note.clone()));

        let mut edited = note.clone();
        edited.title = "  Novo título  ".into();
        edited.body = "linha 1\r\nlinha 2\r\n".into();
        let saved = store.save(&edited, T0 + 3600).unwrap();
        assert_eq!(saved.updated_unix, T0 + 3600);
        assert_eq!(saved.created_unix, T0);
        assert_eq!(saved.title, "Novo título");
        assert_eq!(saved.body, "linha 1\nlinha 2\n");
        assert_eq!(store.get(&note.id).unwrap(), Some(saved));
        assert_eq!(store.get("209901010000").unwrap(), None);
    }

    #[test]
    fn titles_tags_and_sources_with_colons_and_quotes_round_trip() {
        let dir = TempDir::new("quoting");
        let store = dir.store();
        let tricky = [
            "Rust: ownership",
            "a:b",
            "termina em dois pontos:",
            "Ele disse \"oi\"",
            "'simples'",
            "it's",
            "barra \\ invertida",
            "a # não é comentário",
            "#hashtag",
            "- lista",
            "[x] feito",
            "{chaves}",
            "true",
            "No",
            "null",
            "~",
            "2026-09-23",
            "123",
            "a, b",
            "ação: reação",
            "tab\taqui",
            "* estrela",
            "&âncora",
            "!tag",
            "%pct",
            "@arroba",
            "`código`",
            "|barra",
            ">maior",
            "?q",
            "---",
            "...",
        ];
        for (index, text) in tricky.iter().enumerate() {
            let note = store
                .create(
                    text,
                    "",
                    vec![
                        text.to_string(),
                        format!("t{index}:x"),
                        format!("x{index}, y"),
                    ],
                    Some(format!("https://x.pt/?q={text}")),
                    T0 + index as u64 * 60,
                )
                .unwrap();
            let back = store.get(&note.id).unwrap().unwrap();
            assert_eq!(back.title, *text);
            assert_eq!(
                back.tags[1..],
                [format!("t{index}:x"), format!("x{index}, y")]
            );
            assert_eq!(back, note, "{}", note.to_markdown());
        }
    }

    #[test]
    fn crlf_file_reads_like_lf() {
        let dir = TempDir::new("crlf");
        let store = dir.store();
        let raw = "---\r\nid: 202601010000\r\ntitle: \"Título: com CRLF\"\r\n\
                   tags: [a, \"b:c\"]\r\nsource: https://ex.pt/x\r\n\
                   created: 100\r\nupdated: 200\r\n---\r\n# Cabeçalho\r\nlinha\r\n";
        fs::write(store.dir().join("202601010000.md"), raw).unwrap();

        let note = store.get("202601010000").unwrap().unwrap();
        assert_eq!(note.title, "Título: com CRLF");
        assert_eq!(note.tags, ["a", "b:c"]);
        assert_eq!(note.source.as_deref(), Some("https://ex.pt/x"));
        assert_eq!((note.created_unix, note.updated_unix), (100, 200));
        assert_eq!(note.body, "# Cabeçalho\nlinha\n");
        assert!(note.extra_front_matter.is_empty());
    }

    #[test]
    fn obsidian_files_without_front_matter_or_with_its_properties() {
        let dir = TempDir::new("obsidian");
        let store = dir.store();
        let body = "Intro solta\n\n# Título do Obsidian #\n\ntexto [[202601010001]]\n";
        fs::write(
            store.dir().join("202601010000.md"),
            format!("\u{feff}{body}"),
        )
        .unwrap();
        fs::write(store.dir().join("202601010002.md"), "só texto\n").unwrap();
        fs::write(
            store.dir().join("202601010003.md"),
            "---\ntags:\n  - leitura\n  - \"a:b\"\naliases:\n  - Outro nome\n\
             cssclasses: wide\ntitle: 'It''s'\ncreated: 2026-01-01T10:30\n---\ncorpo",
        )
        .unwrap();

        let plain = store.get("202601010000").unwrap().unwrap();
        assert_eq!(plain.title, "Título do Obsidian");
        assert_eq!(plain.body, body, "o BOM sai, o resto fica");
        assert!(plain.tags.is_empty() && plain.source.is_none());
        // 2026-01-01T00:00Z, tirado do próprio id.
        assert_eq!(plain.created_unix, 1_767_225_600);

        let untitled = store.get("202601010002").unwrap().unwrap();
        assert_eq!(untitled.title, "202601010002");

        let props = store.get("202601010003").unwrap().unwrap();
        assert_eq!(props.tags, ["leitura", "a:b"]);
        assert_eq!(props.title, "It's");
        assert_eq!(props.created_unix, 1_767_225_600 + 10 * 3600 + 30 * 60);
        assert_eq!(
            props.extra_front_matter,
            ["aliases:", "  - Outro nome", "cssclasses: wide"]
        );
        let saved = store.save(&props, T0).unwrap();
        let text = fs::read_to_string(store.dir().join("202601010003.md")).unwrap();
        assert!(
            text.contains("\naliases:\n  - Outro nome\ncssclasses: wide\n---\ncorpo"),
            "as propriedades do Obsidian sobrevivem ao save: {text}"
        );
        assert_eq!(store.get("202601010003").unwrap(), Some(saved));
    }

    #[test]
    fn front_matter_edge_cases_are_tolerated() {
        let unclosed = Note::from_markdown("202601010000", "---\ntitle: x\nsem fecho");
        assert_eq!(unclosed.title, "202601010000");
        assert_eq!(unclosed.body, "---\ntitle: x\nsem fecho");

        let minimal = Note::from_markdown("202601010000", "---\ntitle: Só título\n---\n");
        assert_eq!(minimal.title, "Só título");
        assert!(minimal.tags.is_empty() && minimal.source.is_none() && minimal.body.is_empty());
        assert_eq!(minimal.created_unix, minimal.updated_unix);

        let loose = Note::from_markdown(
            "1",
            "---\ntitle: Nota # comentário\ntags: a,b , a # comentário\nsource:\ncreated: nunca\n---\n",
        );
        assert_eq!(loose.title, "Nota");
        assert_eq!(loose.tags, ["a", "b"]);
        assert_eq!(loose.source, None);
        assert_eq!(loose.created_unix, 0, "id sem data e created ilegível");

        let broken = Note::from_markdown("1", "---\ntitle: \"sem fecho\\\ntags: [a, \"b\n---\n");
        assert_eq!(broken.title, "sem fecho\\");
        assert_eq!(broken.tags, ["a", "b"]);
    }

    #[test]
    fn ids_never_collide_within_a_minute() {
        let dir = TempDir::new("collision");
        let store = dir.store();
        let a = store.create("a", "", vec![], None, T0).unwrap();
        let b = store.create("b", "", vec![], None, T0).unwrap();
        let c = store.create("c", "", vec![], None, T0).unwrap();
        let d = store.create("d", "", vec![], None, T0 + 10).unwrap();
        // Outra janela sobre o mesmo cofre.
        let other = ZettelStore::open(store.dir()).unwrap();
        let e = other.create("e", "", vec![], None, T0).unwrap();

        assert_eq!(
            ids(&[a, b, c, d, e]),
            [
                "202609230412",
                "20260923041205",
                "20260923041205-2",
                "20260923041215",
                "20260923041205-3",
            ]
        );
        let mut titles: Vec<String> = store.list().unwrap().into_iter().map(|n| n.title).collect();
        titles.sort();
        assert_eq!(titles, ["a", "b", "c", "d", "e"], "nenhuma esmagou outra");
    }

    #[test]
    fn links_are_ids_in_order_deduplicated_with_alias() {
        let note = note_with_body(
            "202601019999",
            "Ver [[202601010000]] e [[202601010001|um alias]].\n\
             De novo [[202601010000]], [[Nome do Obsidian]], [[202601010002#Secção]].\n\
             Em código `[[202601010008]]` não conta; ![[202601010003]] conta.\n\
             ```\n[[202601010009]]\n```\n\
             | tabela | [[202601010004\\|alias]] |\n\
             [[quebrado [[202601010005]] [[]] [[ ]]",
        );
        assert_eq!(
            links(&note),
            [
                "202601010000",
                "202601010001",
                "202601010002",
                "202601010003",
                "202601010004",
                "202601010005",
            ]
        );
    }

    #[test]
    fn backlinks_find_every_note_that_links_by_id_or_alias() {
        let dir = TempDir::new("backlinks");
        let store = dir.store();
        let target = store
            .create("alvo", "eu mesma: [[202609230412]]", vec![], None, T0)
            .unwrap();
        assert_eq!(target.id, "202609230412");
        let by_id = store
            .create("por id", "[[202609230412]]", vec![], None, T0 + 60)
            .unwrap();
        let by_alias = store
            .create(
                "por alias",
                "ver [[202609230412|o alvo]]",
                vec![],
                None,
                T0 + 120,
            )
            .unwrap();
        let _unrelated = store
            .create("nada", "[[209901010000]]", vec![], None, T0 + 180)
            .unwrap();

        let found = backlinks(&store, &target.id).unwrap();
        assert_eq!(ids(&found), [by_alias.id.as_str(), by_id.id.as_str()]);
        assert!(backlinks(&store, &by_id.id).unwrap().is_empty());
    }

    #[test]
    fn search_is_case_and_accent_insensitive() {
        let dir = TempDir::new("accents");
        let store = dir.store();
        let acao = store
            .create(
                "Ação e reação",
                "Plano de ação",
                vec!["Política".into()],
                None,
                T0,
            )
            .unwrap();
        // "reação" em NFD, como o macOS grava nomes e às vezes texto.
        let nfd = store
            .create(
                "Decomposto",
                "reac\u{0327}a\u{0303}o em NFD",
                vec![],
                None,
                T0 + 60,
            )
            .unwrap();
        let _other = store
            .create("Outra coisa", "nada", vec![], None, T0 + 120)
            .unwrap();

        let both = [acao.id.as_str(), nfd.id.as_str()];
        assert_eq!(ids(&store.search("acao").unwrap()), both);
        assert_eq!(ids(&store.search("AÇÃO").unwrap()), both);
        assert_eq!(ids(&store.search("reacao").unwrap()), both);
        assert_eq!(ids(&store.search("politica").unwrap()), [acao.id.as_str()]);
        assert!(store.search("").unwrap().is_empty());
        assert!(store.search("  ,;  ").unwrap().is_empty());
        assert!(store.search("inexistente").unwrap().is_empty());
    }

    #[test]
    fn search_ranks_title_above_tag_above_body_and_needs_every_term() {
        let dir = TempDir::new("ranking");
        let store = dir.store();
        let body = store
            .create("Diário", &"zettel ".repeat(50), vec![], None, T0)
            .unwrap();
        let tagged = store
            .create("Método", "texto", vec!["zettel".into()], None, T0 + 60)
            .unwrap();
        let titled = store
            .create("Zettelkasten na prática", "texto", vec![], None, T0 + 120)
            .unwrap();
        let _other = store
            .create("Outra", "nada", vec![], None, T0 + 180)
            .unwrap();

        assert_eq!(
            ids(&store.search("zettel").unwrap()),
            [titled.id.as_str(), tagged.id.as_str(), body.id.as_str()]
        );
        assert_eq!(
            ids(&store.search("zettel pratica").unwrap()),
            [titled.id.as_str()]
        );
    }

    #[test]
    fn list_is_sorted_by_updated_desc() {
        let dir = TempDir::new("order");
        let store = dir.store();
        let old = store.create("velha", "", vec![], None, T0).unwrap();
        let mid = store.create("meio", "", vec![], None, T0 + 60).unwrap();
        let new = store.create("nova", "", vec![], None, T0 + 120).unwrap();
        assert_eq!(
            ids(&store.list().unwrap()),
            [new.id.as_str(), mid.id.as_str(), old.id.as_str()]
        );

        store.save(&old, T0 + 600).unwrap();
        assert_eq!(
            ids(&store.list().unwrap()),
            [old.id.as_str(), new.id.as_str(), mid.id.as_str()]
        );
    }

    #[test]
    fn delete_moves_the_file_into_trash() {
        let dir = TempDir::new("trash");
        let store = dir.store();
        let note = store.create("Apagar", "x", vec![], None, T0).unwrap();
        let trashed = store.delete(&note.id).unwrap();

        assert_eq!(trashed, store.dir().join(TRASH_DIR).join("202609230412.md"));
        assert!(!store.dir().join("202609230412.md").exists());
        assert_eq!(fs::read_to_string(&trashed).unwrap(), note.to_markdown());
        assert_eq!(store.get(&note.id).unwrap(), None);
        assert!(store.list().unwrap().is_empty());

        // O id recicla-se; o segundo delete não pode esmagar o primeiro na lixeira.
        let again = store.create("De novo", "y", vec![], None, T0).unwrap();
        assert_eq!(again.id, note.id);
        let second = store.delete(&again.id).unwrap();
        assert_ne!(second, trashed);
        assert_eq!(fs::read_to_string(&trashed).unwrap(), note.to_markdown());
        assert_eq!(fs::read_to_string(&second).unwrap(), again.to_markdown());
        assert!(matches!(
            store.delete(&note.id),
            Err(ZettelError::NotFound(_))
        ));
    }

    #[test]
    fn path_traversal_ids_are_rejected() {
        let dir = TempDir::new("traversal");
        let store = dir.store();
        let victim = dir.path().join("x.md");
        fs::write(&victim, "não tocar").unwrap();

        for id in [
            "../x",
            "..\\x",
            "../../x",
            "/x",
            "C:\\x",
            "202609230412/../../x",
            "202609230412.md",
            "202609230412\0",
            "",
            ".",
            "..",
            "-1",
            "abc",
            "2026 09",
        ] {
            assert!(
                matches!(store.get(id), Err(ZettelError::InvalidId(_))),
                "get {id:?}"
            );
            let note = Note {
                id: id.to_string(),
                title: "t".into(),
                ..Note::default()
            };
            assert!(
                matches!(store.save(&note, T0), Err(ZettelError::InvalidId(_))),
                "save {id:?}"
            );
            assert!(
                matches!(store.delete(id), Err(ZettelError::InvalidId(_))),
                "delete {id:?}"
            );
            assert!(matches!(
                backlinks(&store, id),
                Err(ZettelError::InvalidId(_))
            ));
        }
        assert_eq!(fs::read_to_string(&victim).unwrap(), "não tocar");
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            2,
            "notas/ + x.md"
        );
        assert_eq!(fs::read_dir(store.dir()).unwrap().count(), 0);
    }

    #[test]
    fn list_skips_non_utf8_and_foreign_files() {
        let dir = TempDir::new("foreign");
        let store = dir.store();
        let good = store.create("Boa", "ok", vec![], None, T0).unwrap();
        fs::write(
            store.dir().join("202601010000.md"),
            [0xFF, 0xFE, b'-', 0x80, 0xC3],
        )
        .unwrap();
        fs::write(store.dir().join("Minha nota.md"), "# fora do esquema").unwrap();
        fs::write(store.dir().join("202601010001.txt"), "não é nota").unwrap();
        fs::create_dir(store.dir().join("202601010002.md")).unwrap();

        assert_eq!(store.list().unwrap(), std::slice::from_ref(&good));
        assert_eq!(store.search("boa").unwrap(), std::slice::from_ref(&good));
        assert!(backlinks(&store, &good.id).unwrap().is_empty());
        assert!(matches!(
            store.get("202601010000"),
            Err(ZettelError::NotUtf8(_))
        ));
    }

    #[test]
    fn failed_save_leaves_no_temp_file_behind() {
        let dir = TempDir::new("failed-save");
        let store = dir.store();
        // Uma pasta com o nome da nota: o `rename` final tem de falhar.
        fs::create_dir(store.dir().join("202609230412.md")).unwrap();
        let note = Note {
            id: "202609230412".into(),
            title: "x".into(),
            ..Note::default()
        };
        assert!(store.save(&note, T0).is_err());
        let names: Vec<String> = fs::read_dir(store.dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["202609230412.md"]);
    }

    /// Bytes arbitrários nunca fazem pânico, e ler o que se escreveu devolve a
    /// mesma nota (parse ∘ serialize ∘ parse = parse).
    #[test]
    fn parser_never_panics_and_round_trips_its_own_output() {
        const PIECES: &[&[u8]] = &[
            b"---",
            b"\n",
            b"\r\n",
            b"\r",
            b"...",
            b"title:",
            b"tags:",
            b"source: ",
            b"created: ",
            b"updated: ",
            b"aliases:",
            b"id: ",
            b" [",
            b"]",
            b",",
            b"\"",
            b"'",
            b"\\",
            b"\\u12",
            b"\\x",
            b"\\U0010FFFF",
            b"#",
            b" #",
            b"# ",
            b"[[",
            b"]]",
            b"|",
            b"`",
            b"```",
            b"  - ",
            b"- ",
            b"99999999999999999999",
            b"2026-02-30T10:30",
            b"1700000000",
            b"a",
            b"z9",
            b" ",
            b"\t",
            b"\0",
            b"\xC3\xA7",
            b"\xFF",
            b"\xE2\x82",
            b"\xEF\xBB\xBF",
            b"202601010000",
            b":",
            b"true",
        ];
        let dir = TempDir::new("fuzz");
        let store = dir.store();
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for case in 0..3000 {
            let mut bytes = Vec::new();
            for _ in 0..next() % 40 {
                bytes.extend_from_slice(PIECES[(next() % PIECES.len() as u64) as usize]);
            }
            let text = String::from_utf8_lossy(&bytes);
            let first = Note::from_markdown("202601010000", &text);
            let second = Note::from_markdown("202601010000", &first.to_markdown());
            assert_eq!(first, second, "entrada {text:?}");
            let _ = links(&first);
            if case % 30 == 0 {
                fs::write(
                    store.dir().join(format!("{}.md", 202601010000u64 + case)),
                    &bytes,
                )
                .unwrap();
            }
        }
        let listed = store.list().unwrap();
        assert!(!listed.is_empty() && listed.len() <= 100);
        let _ = store.search("a title tags");
    }
}
