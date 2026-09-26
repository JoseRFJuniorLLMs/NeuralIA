//! Lojas em disco do NeuralIA: o token de capacidade que as abre e o ficheiro
//! JSON versionado (infra-settings-keys, plano 2.3).
//!
//! **O token.** Abrir uma loja pede um `StoreGrant`, e so o `StoreRegistry`
//! os passa. O registo cunha-se UMA vez por processo (`StoreRegistry::mint`,
//! chamado no `App::new` -- o unico no produto); uma segunda cunhagem devolve
//! `Err(AlreadyMinted)`. Nem o registo nem o grant tem campos publicos,
//! `Default`, `Clone` ou construtor publico, nem implementam trait nenhum
//! alem de `Debug` (um `From`, `FromStr` ou `Deserialize` seria um
//! construtor); o modulo nao tem `unsafe`: fora deste modulo a unica
//! maneira de ter um grant e pedi-lo ao registo, com uma `StoreSpec` que diz
//! sempre o tipo (`StoreKind`) da loja. Os testes cunham com
//! `StoreRegistry::mint_for_test`, que so existe em `cfg(test)` e com a
//! feature de CI `test-stores` (ligada apenas nas `[dev-dependencies]` do
//! neural-app; o exe publicado e compilado sem ela).
//!
//! A regra do tipo: `Setting` muda so por uma escolha explicita numa
//! definicao ou menu; `Explicit` e conteudo que o utilizador pediu para
//! guardar; `Automatic` e tudo o que se escreve como efeito lateral do uso,
//! ate geometria inofensiva (a largura do painel, a posicao do PiP). Um
//! ficheiro, um tipo -- e o NTFS nao distingue maiusculas nem guarda o ponto
//! final de um nome (`Panel-Width.json` e `panel-width.json.` sao o
//! `panel-width.json`): o registo compara os nomes sem maiusculas e recusa
//! partes que acabam em `.`. Enquanto o modo partilhado disser `Private`, as
//! escritas de um grant `Automatic` nao fazem nada (o `PrivacyGuard` da onda 2
//! e quem liga o modo; ate la e sempre `Normal`).
//!
//! **O ficheiro.** `VersionedJsonStore<T>` guarda `{"version":N,"data":T}`:
//!
//! - o tecto de bytes confere-se ANTES de ler e de fazer parse;
//! - sem ficheiro valem os valores por omissao e nada se escreve;
//! - estragado, de uma versao futura, grande demais ou ilegivel: estado
//!   degradado so de leitura, o ficheiro fica como esta e nunca e reescrito
//!   por cima; o estragado e o de uma versao futura (que cabem no tecto)
//!   ganham uma copia `.bak` com os bytes lidos; o grande demais nao (um
//!   ficheiro de tamanho qualquer nunca se duplica no disco);
//! - gravar e temporario + `sync_all` + `rename`: um corte a meio deixa o
//!   ficheiro antigo ou o novo, nunca meio;
//! - com `shared_between_windows`, um trinco (`<ficheiro>.lock`) e a releitura
//!   debaixo dele (`update`) impedem duas janelas de perder a escrita uma da
//!   outra.
//!
//! `TokenFile` (`read_token`/`write_token`) e o mesmo para os ficheiros de uma
//! palavra so (`theme`, `gmail`).

// Sem `unsafe`, um grant nao se copia nem se fabrica por baixo (`ptr::read`,
// `transmute`); o `a_store_cannot_open_without_a_grant` confere esta linha.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// O tipo de uma loja: quem a escreve e porque. Sem `Default`: cada loja diz
/// o seu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreKind {
    /// Efeito lateral do uso (historico, abas, largura do painel...). No modo
    /// privado as escritas nao fazem nada.
    Automatic,
    /// Conteudo que o utilizador pediu para guardar (notas, chaves...).
    Explicit,
    /// Uma escolha feita numa definicao ou num menu (tema, Pomodoro...).
    Setting,
}

/// Ficheiro ou pasta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreShape {
    File,
    Dir,
}

/// O que se pede ao registo: o nome da loja dentro da pasta de dados, o
/// tipo e a forma. O tipo e obrigatorio -- nao ha `Default`:
///
/// ```compile_fail
/// use neural_core::json_store::{StoreShape, StoreSpec};
/// let _sem_tipo = StoreSpec { name: "sem-tipo.json", shape: StoreShape::File, ..Default::default() };
/// ```
///
/// Com o tipo dito, compila:
///
/// ```
/// use neural_core::json_store::{StoreKind, StoreShape, StoreSpec};
/// let spec = StoreSpec { name: "com-tipo.json", kind: StoreKind::Setting, shape: StoreShape::File };
/// assert_eq!(spec, StoreSpec::new("com-tipo.json", StoreKind::Setting, StoreShape::File));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreSpec {
    /// Caminho relativo a pasta de dados, com `/` entre partes
    /// (`panel-width.json`, `keys`, `ai/settings.json`).
    pub name: &'static str,
    pub kind: StoreKind,
    pub shape: StoreShape,
}

impl StoreSpec {
    pub const fn new(name: &'static str, kind: StoreKind, shape: StoreShape) -> Self {
        Self { name, kind, shape }
    }
}

/// O modo partilhado entre o registo e os grants que ele passou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
    Normal,
    /// As escritas das lojas `Automatic` nao fazem nada.
    Private,
}

/// A segunda cunhagem do registo no mesmo processo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("o registo das lojas ja foi cunhado neste processo")]
pub struct AlreadyMinted;

/// Porque o registo recusou um grant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    #[error("nome de loja invalido: {0:?}")]
    BadName(&'static str),
    /// Um ficheiro, um tipo: o mesmo nome ja foi dado com outro tipo ou
    /// outra forma.
    #[error("a loja {name:?} ja foi pedida como {granted:?}/{granted_shape:?}")]
    Conflict {
        name: &'static str,
        granted: StoreKind,
        granted_shape: StoreShape,
    },
}

/// Sobe uma vez por processo, na primeira `mint`.
static MINTED: AtomicBool = AtomicBool::new(false);

/// O registo das lojas: a unica fonte de `StoreGrant`s. Sem campos publicos,
/// `Default`, `Clone` nem construtor publico alem de `mint`:
///
/// ```compile_fail
/// use neural_core::json_store::StoreRegistry;
/// let _forjado = StoreRegistry {
///     data_dir: std::env::temp_dir(),
///     private: Default::default(),
///     granted: Default::default(),
/// };
/// ```
///
/// ```compile_fail
/// use neural_core::json_store::StoreRegistry;
/// let registry = StoreRegistry::mint(std::env::temp_dir()).unwrap();
/// let _segundo: StoreRegistry = registry.clone();
/// ```
///
/// Nem por um trait de conversao:
///
/// ```compile_fail
/// use neural_core::json_store::StoreRegistry;
/// let _forjado: StoreRegistry = std::env::temp_dir().into();
/// ```
#[derive(Debug)]
pub struct StoreRegistry {
    data_dir: PathBuf,
    private: Arc<AtomicBool>,
    /// Os nomes ja dados, sem maiusculas (`store_key`), com o tipo e a forma.
    granted: Mutex<BTreeMap<String, (StoreKind, StoreShape)>>,
}

impl StoreRegistry {
    /// Cunha o registo. So a primeira chamada do processo tem sucesso; o
    /// produto chama-a uma vez, no `App::new`.
    pub fn mint(data_dir: impl Into<PathBuf>) -> Result<StoreRegistry, AlreadyMinted> {
        MINTED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AlreadyMinted)?;
        Ok(Self::build(data_dir.into()))
    }

    /// Um registo sobre uma pasta de teste, sem gastar a cunhagem do
    /// processo. So nos testes (e com a feature de CI `test-stores`).
    #[cfg(any(test, feature = "test-stores"))]
    pub fn mint_for_test(dir: impl Into<PathBuf>) -> StoreRegistry {
        Self::build(dir.into())
    }

    fn build(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            private: Arc::new(AtomicBool::new(false)),
            granted: Mutex::new(BTreeMap::new()),
        }
    }

    /// A unica fonte de grants. O mesmo nome pode ser pedido outra vez com o
    /// mesmo tipo e forma (duas lojas no mesmo ficheiro partilhado); com
    /// outro, e recusado. "O mesmo nome" e o mesmo ficheiro no NTFS: sem
    /// maiusculas (`Panel-Width.json` e o `panel-width.json`).
    pub fn grant(&self, spec: StoreSpec) -> Result<StoreGrant, GrantError> {
        if !valid_store_name(spec.name) {
            return Err(GrantError::BadName(spec.name));
        }
        let key = store_key(spec.name);
        let mut granted = self
            .granted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match granted.get(&key) {
            Some(&(kind, shape)) if (kind, shape) != (spec.kind, spec.shape) => {
                return Err(GrantError::Conflict {
                    name: spec.name,
                    granted: kind,
                    granted_shape: shape,
                });
            }
            Some(_) => {}
            None => {
                granted.insert(key, (spec.kind, spec.shape));
            }
        }
        let mut path = self.data_dir.clone();
        for part in spec.name.split('/') {
            path.push(part);
        }
        Ok(StoreGrant {
            path,
            name: spec.name,
            kind: spec.kind,
            shape: spec.shape,
            private: Arc::clone(&self.private),
        })
    }

    /// Muda o modo de todos os grants ja passados e dos que vierem. So quem
    /// tem o registo o pode fazer.
    pub fn set_mode(&self, mode: StoreMode) {
        self.private
            .store(mode == StoreMode::Private, Ordering::Release);
    }

    pub fn mode(&self) -> StoreMode {
        mode_of(&self.private)
    }
}

fn mode_of(flag: &AtomicBool) -> StoreMode {
    if flag.load(Ordering::Acquire) {
        StoreMode::Private
    } else {
        StoreMode::Normal
    }
}

/// Um nome relativo simples: partes de `[A-Za-z0-9._-]`, separadas por `/`,
/// sem `.`/`..`, sem raiz nem letra de unidade. Nunca sai da pasta de dados.
/// Nenhuma parte acaba em `.`: o Windows tira-o ao abrir, e
/// `panel-width.json.` seria o `panel-width.json` com outro nome.
fn valid_store_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name.split('/').all(|part| {
            !part.is_empty()
                && !part.ends_with('.')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

/// A chave de um nome valido no registo: o ficheiro que o NTFS abre, que nao
/// distingue maiusculas (os nomes sao ASCII, por isso basta o ASCII).
fn store_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// O token de capacidade de uma loja: o caminho, o tipo, a forma e o modo
/// partilhado. So `StoreRegistry::grant` o cria; nao se copia nem se forja:
///
/// ```compile_fail
/// use neural_core::json_store::{StoreGrant, StoreKind, StoreShape};
/// let _forjado = StoreGrant {
///     path: std::path::PathBuf::from("forjado.json"),
///     name: "forjado.json",
///     kind: StoreKind::Setting,
///     shape: StoreShape::File,
///     private: Default::default(),
/// };
/// ```
///
/// ```compile_fail
/// use neural_core::json_store::{StoreGrant, StoreKind, StoreRegistry, StoreShape, StoreSpec};
/// let registry = StoreRegistry::mint(std::env::temp_dir()).unwrap();
/// let spec = StoreSpec::new("a.json", StoreKind::Setting, StoreShape::File);
/// let grant = registry.grant(spec).unwrap();
/// let _copia: StoreGrant = grant.clone();
/// ```
///
/// ```compile_fail
/// use neural_core::json_store::StoreGrant;
/// let _forjado: StoreGrant = std::path::PathBuf::from("forjado.json").into();
/// ```
///
/// O mesmo caminho com o grant certo compila:
///
/// ```no_run
/// use neural_core::json_store::{StoreGrant, StoreKind, StoreRegistry, StoreShape, StoreSpec};
/// let registry = StoreRegistry::mint(std::env::temp_dir()).unwrap();
/// let spec = StoreSpec::new("a.json", StoreKind::Setting, StoreShape::File);
/// let grant: StoreGrant = registry.grant(spec).unwrap();
/// assert_eq!(grant.kind(), StoreKind::Setting);
/// ```
#[derive(Debug)]
pub struct StoreGrant {
    path: PathBuf,
    name: &'static str,
    kind: StoreKind,
    shape: StoreShape,
    private: Arc<AtomicBool>,
}

impl StoreGrant {
    /// Onde a loja vive (dentro da pasta de dados do registo).
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn kind(&self) -> StoreKind {
        self.kind
    }

    pub fn shape(&self) -> StoreShape {
        self.shape
    }

    pub fn mode(&self) -> StoreMode {
        mode_of(&self.private)
    }

    /// Falso so para uma loja `Automatic` com o modo em `Private`.
    pub fn writes_allowed(&self) -> bool {
        !(self.kind == StoreKind::Automatic && self.mode() == StoreMode::Private)
    }
}

/// Porque o ficheiro ficou so de leitura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Degraded {
    /// Existe mas nao se consegue ler (permissoes, e uma pasta...).
    Unreadable(io::ErrorKind),
    /// Maior do que o tecto; nao chegou a ser lido.
    TooLarge { bytes: u64, max: u64 },
    /// Nao e o envelope `{"version":N,"data":...}` de um `T`.
    Corrupt,
    /// Escrito por um NeuralIA mais novo.
    FutureVersion { found: u32, known: u32 },
}

/// O que `load` encontrou.
#[derive(Debug, PartialEq)]
pub enum LoadOutcome<T> {
    Loaded(T),
    /// Sem ficheiro: os valores por omissao; nada foi escrito.
    Missing(T),
    /// Os valores por omissao; o ficheiro ficou como estava e a loja so le.
    Degraded {
        value: T,
        why: Degraded,
        /// A copia `<ficheiro>.bak` dos bytes lidos, se foi possivel fazer.
        /// Nunca de um ficheiro `TooLarge` ou `Unreadable` (nao foi lido).
        backup: Option<PathBuf>,
    },
}

impl<T> LoadOutcome<T> {
    pub fn value(&self) -> &T {
        match self {
            Self::Loaded(value) | Self::Missing(value) | Self::Degraded { value, .. } => value,
        }
    }

    pub fn into_value(self) -> T {
        match self {
            Self::Loaded(value) | Self::Missing(value) | Self::Degraded { value, .. } => value,
        }
    }
}

/// O que uma escrita fez.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    Written,
    /// Loja `Automatic` com o modo em `Private`: nada foi escrito.
    SkippedPrivate,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("a loja {name:?} nao e um {expected:?}")]
    WrongShape {
        name: &'static str,
        expected: StoreShape,
    },
    #[error("a loja esta so de leitura: {0:?}")]
    ReadOnly(Degraded),
    #[error("{bytes} bytes passam o tecto de {max}")]
    TooLarge { bytes: u64, max: u64 },
    #[error("nao e uma palavra valida para um ficheiro de uma palavra")]
    NotAToken,
    #[error("nao foi possivel codificar: {0}")]
    Encode(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Serialize)]
struct EnvelopeOut<'a, T> {
    version: u32,
    data: &'a T,
}

#[derive(Deserialize)]
struct Head {
    version: u32,
}

#[derive(Deserialize)]
struct Body<T> {
    data: T,
}

/// O que esta no disco agora.
enum OnDisk<T> {
    Missing,
    Valid(T),
    /// Com os bytes lidos (sempre dentro do tecto) quando chegou a ler:
    /// `Corrupt` e `FutureVersion`; `TooLarge` e `Unreadable` nao os tem.
    Degraded(Degraded, Option<Vec<u8>>),
}

/// Um JSON `{"version":N,"data":T}` numa loja de ficheiro, aberto com o seu
/// grant. Sem grant nao abre:
///
/// ```compile_fail
/// use neural_core::json_store::VersionedJsonStore;
/// let _store = VersionedJsonStore::<Vec<u32>>::open(std::path::PathBuf::from("x.json"), 1, 4096);
/// ```
///
/// Com o grant, sim:
///
/// ```no_run
/// use neural_core::json_store::{StoreKind, StoreRegistry, StoreShape, StoreSpec, VersionedJsonStore};
/// let registry = StoreRegistry::mint(std::env::temp_dir()).unwrap();
/// let grant = registry.grant(StoreSpec::new("x.json", StoreKind::Setting, StoreShape::File)).unwrap();
/// let _store = VersionedJsonStore::<Vec<u32>>::open(grant, 1, 4096).unwrap();
/// ```
#[derive(Debug)]
pub struct VersionedJsonStore<T> {
    grant: StoreGrant,
    version: u32,
    max_bytes: u64,
    shared: bool,
    read_only: Option<Degraded>,
    _data: PhantomData<fn() -> T>,
}

impl<T: Serialize + DeserializeOwned + Default> VersionedJsonStore<T> {
    /// `version` e a versao que este codigo escreve (a partir de 1); um
    /// ficheiro de versao maior fica so de leitura. `max_bytes` e o tecto do
    /// ficheiro, conferido antes de o ler.
    pub fn open(grant: StoreGrant, version: u32, max_bytes: u64) -> Result<Self, StoreError> {
        if grant.shape != StoreShape::File {
            return Err(StoreError::WrongShape {
                name: grant.name,
                expected: StoreShape::File,
            });
        }
        Ok(Self {
            grant,
            version: version.max(1),
            max_bytes,
            shared: false,
            read_only: None,
            _data: PhantomData,
        })
    }

    /// Ficheiro partilhado entre janelas: gravar toma o trinco
    /// `<ficheiro>.lock`, e `update` rele o ficheiro debaixo dele.
    pub fn shared_between_windows(mut self) -> Self {
        self.shared = true;
        self
    }

    pub fn path(&self) -> &Path {
        &self.grant.path
    }

    pub fn kind(&self) -> StoreKind {
        self.grant.kind
    }

    /// `Some` depois de um `load` (ou de uma escrita) que encontrou o ficheiro
    /// estragado, de uma versao futura ou grande demais.
    pub fn read_only(&self) -> Option<&Degraded> {
        self.read_only.as_ref()
    }

    /// Le o ficheiro. Nunca escreve o ficheiro: sem ele valem os valores por
    /// omissao; degradado, a loja passa a so de leitura e, se o que leu cabe
    /// no tecto (estragado ou de uma versao futura) e as escritas estao
    /// permitidas, guarda esses bytes numa copia `.bak`.
    pub fn load(&mut self) -> LoadOutcome<T> {
        match self.on_disk() {
            OnDisk::Missing => {
                self.read_only = None;
                LoadOutcome::Missing(T::default())
            }
            OnDisk::Valid(value) => {
                self.read_only = None;
                LoadOutcome::Loaded(value)
            }
            OnDisk::Degraded(why, read) => {
                let backup = self.degrade(why.clone(), read);
                LoadOutcome::Degraded {
                    value: T::default(),
                    why,
                    backup,
                }
            }
        }
    }

    /// Grava `value` por cima. Recusa se a loja esta so de leitura ou se o
    /// ficheiro no disco (relido agora) esta degradado: um ficheiro de uma
    /// versao futura escrito por outra janela nunca e esmagado.
    pub fn save(&mut self, value: &T) -> Result<SaveOutcome, StoreError> {
        if !self.grant.writes_allowed() {
            return Ok(SaveOutcome::SkippedPrivate);
        }
        if let Some(why) = &self.read_only {
            return Err(StoreError::ReadOnly(why.clone()));
        }
        let bytes = self.encode(value)?;
        self.ensure_parent()?;
        let _lock = self.lock()?;
        if let OnDisk::Degraded(why, read) = self.on_disk() {
            self.degrade(why.clone(), read);
            return Err(StoreError::ReadOnly(why));
        }
        write_atomically(&self.grant.path, &bytes)?;
        Ok(SaveOutcome::Written)
    }

    /// Le, muda e grava debaixo do trinco: a mudanca aplica-se ao que esta
    /// no disco AGORA, nao ao que esta janela leu antes. Duas janelas que
    /// fazem `update` ao mesmo ficheiro nunca perdem a escrita da outra.
    pub fn update<R>(
        &mut self,
        change: impl FnOnce(&mut T) -> R,
    ) -> Result<(R, SaveOutcome), StoreError> {
        if !self.grant.writes_allowed() {
            let mut value = match self.on_disk() {
                OnDisk::Valid(value) => value,
                OnDisk::Missing | OnDisk::Degraded(..) => T::default(),
            };
            return Ok((change(&mut value), SaveOutcome::SkippedPrivate));
        }
        if let Some(why) = &self.read_only {
            return Err(StoreError::ReadOnly(why.clone()));
        }
        self.ensure_parent()?;
        let _lock = self.lock()?;
        let mut value = match self.on_disk() {
            OnDisk::Missing => T::default(),
            OnDisk::Valid(value) => value,
            OnDisk::Degraded(why, read) => {
                self.degrade(why.clone(), read);
                return Err(StoreError::ReadOnly(why));
            }
        };
        let result = change(&mut value);
        let bytes = self.encode(&value)?;
        write_atomically(&self.grant.path, &bytes)?;
        Ok((result, SaveOutcome::Written))
    }

    fn encode(&self, value: &T) -> Result<Vec<u8>, StoreError> {
        let mut bytes = serde_json::to_vec_pretty(&EnvelopeOut {
            version: self.version,
            data: value,
        })
        .map_err(|error| StoreError::Encode(error.to_string()))?;
        bytes.push(b'\n');
        // Nunca se escreve um ficheiro que a propria loja depois recusasse.
        let len = bytes.len() as u64;
        if len > self.max_bytes {
            return Err(StoreError::TooLarge {
                bytes: len,
                max: self.max_bytes,
            });
        }
        Ok(bytes)
    }

    fn on_disk(&self) -> OnDisk<T> {
        let unread = |why| OnDisk::Degraded(why, None);
        let path = &self.grant.path;
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return OnDisk::Missing,
            Err(error) => return unread(Degraded::Unreadable(error.kind())),
        };
        // O tecto antes de ler um byte: um ficheiro enorme nunca chega ao
        // parser (nem a memoria).
        let len = match file.metadata() {
            Ok(meta) if meta.is_dir() => {
                return unread(Degraded::Unreadable(io::ErrorKind::IsADirectory));
            }
            Ok(meta) => meta.len(),
            Err(error) => return unread(Degraded::Unreadable(error.kind())),
        };
        if len > self.max_bytes {
            return unread(Degraded::TooLarge {
                bytes: len,
                max: self.max_bytes,
            });
        }
        let mut bytes = Vec::new();
        if let Err(error) = file.take(self.max_bytes + 1).read_to_end(&mut bytes) {
            return unread(Degraded::Unreadable(error.kind()));
        }
        // Cresceu entre o metadata e a leitura.
        if bytes.len() as u64 > self.max_bytes {
            return unread(Degraded::TooLarge {
                bytes: bytes.len() as u64,
                max: self.max_bytes,
            });
        }
        let Ok(head) = serde_json::from_slice::<Head>(&bytes) else {
            return OnDisk::Degraded(Degraded::Corrupt, Some(bytes));
        };
        if head.version == 0 {
            return OnDisk::Degraded(Degraded::Corrupt, Some(bytes));
        }
        if head.version > self.version {
            let why = Degraded::FutureVersion {
                found: head.version,
                known: self.version,
            };
            return OnDisk::Degraded(why, Some(bytes));
        }
        match serde_json::from_slice::<Body<T>>(&bytes) {
            Ok(body) => OnDisk::Valid(body.data),
            Err(_) => OnDisk::Degraded(Degraded::Corrupt, Some(bytes)),
        }
    }

    /// Passa a so de leitura e guarda na copia `.bak` os bytes que leu. So o
    /// que foi lido dentro do tecto vai para la: um ficheiro grande demais
    /// (ou que nao se le) fica como esta e sem copia, e a copia nunca passa
    /// do tecto por mais que o ficheiro cresca.
    fn degrade(&mut self, why: Degraded, read: Option<Vec<u8>>) -> Option<PathBuf> {
        self.read_only = Some(why);
        if !self.grant.writes_allowed() {
            return None;
        }
        let bytes = read?;
        let backup = sibling(&self.grant.path, ".bak");
        write_atomically(&backup, &bytes).ok().map(|()| backup)
    }

    fn ensure_parent(&self) -> io::Result<()> {
        match self.grant.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => fs::create_dir_all(dir),
            _ => Ok(()),
        }
    }

    /// O trinco entre janelas, so nas lojas partilhadas. Solta-se quando o
    /// `File` sai de cena.
    fn lock(&self) -> io::Result<Option<File>> {
        if !self.shared {
            return Ok(None);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(sibling(&self.grant.path, ".lock"))?;
        file.lock()?;
        Ok(Some(file))
    }
}

/// `<pasta>/<nome><sufixo>`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

/// Temporario unico na mesma pasta + `sync_all` + `rename`. Um corte a meio
/// deixa o ficheiro antigo ou o novo; o temporario de uma escrita falhada
/// nao fica para tras.
fn write_atomically(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let mut name = std::ffi::OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(format!(".{}-{nonce}.tmp", std::process::id()));
    let temp = target.with_file_name(name);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Uma palavra valida num ficheiro de uma palavra: 1 a 64 bytes de
/// `[A-Za-z0-9._-]`.
pub fn is_token(text: &str) -> bool {
    (1..=TokenFile::MAX_BYTES).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Um ficheiro com uma palavra so (`theme` = `escuro`), aberto com o seu
/// grant.
#[derive(Debug)]
pub struct TokenFile {
    grant: StoreGrant,
}

impl TokenFile {
    /// O tecto do ficheiro e da palavra.
    pub const MAX_BYTES: usize = 64;

    pub fn open(grant: StoreGrant) -> Result<Self, StoreError> {
        if grant.shape != StoreShape::File {
            return Err(StoreError::WrongShape {
                name: grant.name,
                expected: StoreShape::File,
            });
        }
        Ok(Self { grant })
    }

    pub fn path(&self) -> &Path {
        &self.grant.path
    }

    /// A palavra, sem espacos a volta. Sem ficheiro, grande demais ou com
    /// outra coisa dentro: `None` (e nada e escrito).
    pub fn read_token(&self) -> Option<String> {
        let file = File::open(&self.grant.path).ok()?;
        let mut bytes = Vec::new();
        file.take(Self::MAX_BYTES as u64 + 8)
            .read_to_end(&mut bytes)
            .ok()?;
        let text = std::str::from_utf8(&bytes).ok()?.trim();
        is_token(text).then(|| text.to_string())
    }

    /// Grava a palavra (temporario + `rename`). Uma palavra invalida nunca
    /// chega ao disco.
    pub fn write_token(&self, token: &str) -> Result<SaveOutcome, StoreError> {
        if !is_token(token) {
            return Err(StoreError::NotAToken);
        }
        if !self.grant.writes_allowed() {
            return Ok(SaveOutcome::SkippedPrivate);
        }
        if let Some(dir) = self.grant.path.parent()
            && !dir.as_os_str().is_empty()
        {
            fs::create_dir_all(dir)?;
        }
        write_atomically(&self.grant.path, token.as_bytes())?;
        Ok(SaveOutcome::Written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
    struct Prefs {
        count: u32,
        #[serde(default)]
        label: String,
    }

    const PREFS: StoreSpec = StoreSpec::new("prefs.json", StoreKind::Setting, StoreShape::File);

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("neuralia-json-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("pasta temporaria");
        dir
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|read| {
                read.filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn open(registry: &StoreRegistry, max: u64) -> VersionedJsonStore<Prefs> {
        VersionedJsonStore::open(registry.grant(PREFS).expect("grant"), 1, max).expect("abrir")
    }

    #[test]
    fn a_missing_file_gives_defaults_and_writes_nothing() {
        let dir = temp_dir("missing");
        // A pasta de dados nem existe: ler nao a cria.
        let data = dir.join("dados");
        let registry = StoreRegistry::mint_for_test(&data);
        let mut store = open(&registry, 4096);
        assert_eq!(store.load(), LoadOutcome::Missing(Prefs::default()));
        assert_eq!(store.load(), LoadOutcome::Missing(Prefs::default()));
        assert!(store.read_only().is_none());
        assert!(
            !data.exists(),
            "ler uma loja sem ficheiro escreveu no disco"
        );
        assert!(entries(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_value_round_trips_atomically() {
        let dir = temp_dir("round-trip");
        let registry = StoreRegistry::mint_for_test(&dir);
        let mut store = open(&registry, 4096);
        let prefs = Prefs {
            count: 7,
            label: "ção".to_string(),
        };
        assert_eq!(store.save(&prefs).expect("gravar"), SaveOutcome::Written);
        assert_eq!(store.load(), LoadOutcome::Loaded(prefs.clone()));
        // Outra loja no mesmo ficheiro le o mesmo, e so ha o ficheiro.
        assert_eq!(open(&registry, 4096).load().into_value(), prefs);
        assert_eq!(entries(&dir), vec!["prefs.json".to_string()]);
        let text = fs::read_to_string(dir.join("prefs.json")).expect("ler");
        let json: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(json["version"], 1);
        assert_eq!(json["data"]["count"], 7);

        // Um ficheiro de uma versao anterior le-se; o campo novo vem por
        // omissao.
        fs::write(
            dir.join("prefs.json"),
            br#"{"version":1,"data":{"count":3}}"#,
        )
        .expect("v1");
        let mut newer: VersionedJsonStore<Prefs> =
            VersionedJsonStore::open(registry.grant(PREFS).expect("grant"), 2, 4096)
                .expect("abrir");
        assert_eq!(
            newer.load(),
            LoadOutcome::Loaded(Prefs {
                count: 3,
                label: String::new()
            })
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_kept_backed_up_and_never_overwritten() {
        let dir = temp_dir("corrupt");
        let registry = StoreRegistry::mint_for_test(&dir);
        let path = dir.join("prefs.json");
        for (index, bad) in [
            b"{ isto nao e json".to_vec(),
            br#"{"data":{"count":1}}"#.to_vec(),
            br#"{"version":0,"data":{"count":1}}"#.to_vec(),
            br#"{"version":1,"data":{"count":"um"}}"#.to_vec(),
            br#"{"version":1}"#.to_vec(),
            Vec::new(),
        ]
        .iter()
        .enumerate()
        {
            fs::write(&path, bad).expect("escrever o estragado");
            let _ = fs::remove_file(dir.join("prefs.json.bak"));
            let mut store = open(&registry, 4096);
            let outcome = store.load();
            assert_eq!(
                outcome,
                LoadOutcome::Degraded {
                    value: Prefs::default(),
                    why: Degraded::Corrupt,
                    backup: Some(dir.join("prefs.json.bak")),
                },
                "estragado {index}"
            );
            assert_eq!(&fs::read(dir.join("prefs.json.bak")).expect("bak"), bad);
            assert!(matches!(
                store.save(&Prefs::default()),
                Err(StoreError::ReadOnly(Degraded::Corrupt))
            ));
            assert!(matches!(
                store.update(|prefs| prefs.count += 1),
                Err(StoreError::ReadOnly(Degraded::Corrupt))
            ));
            assert_eq!(
                &fs::read(&path).expect("ficheiro"),
                bad,
                "estragado {index}"
            );
            // Uma loja nova que grava sem ler tambem nao o esmaga.
            let mut blind = open(&registry, 4096);
            assert!(matches!(
                blind.save(&Prefs::default()),
                Err(StoreError::ReadOnly(Degraded::Corrupt))
            ));
            assert_eq!(
                &fs::read(&path).expect("ficheiro"),
                bad,
                "estragado {index}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_a_future_version_is_read_only() {
        let dir = temp_dir("future");
        let registry = StoreRegistry::mint_for_test(&dir);
        let path = dir.join("prefs.json");
        let future = br#"{"version":9,"data":{"count":1,"novo":true}}"#;
        fs::write(&path, future).expect("futuro");
        let mut store = open(&registry, 4096);
        let LoadOutcome::Degraded { value, why, backup } = store.load() else {
            panic!("uma versao futura tem de degradar");
        };
        assert_eq!(value, Prefs::default());
        assert_eq!(why, Degraded::FutureVersion { found: 9, known: 1 });
        assert_eq!(backup, Some(dir.join("prefs.json.bak")));
        assert!(store.save(&Prefs::default()).is_err());
        assert_eq!(fs::read(&path).expect("ficheiro"), future);

        // Carregada boa, e outra janela (um NeuralIA mais novo) poe uma
        // versao futura: a gravacao seguinte relê e recusa.
        fs::remove_file(&path).expect("apagar");
        let mut store = open(&registry, 4096);
        assert!(matches!(store.load(), LoadOutcome::Missing(_)));
        fs::write(&path, future).expect("futuro");
        assert!(matches!(
            store.save(&Prefs {
                count: 5,
                label: String::new()
            }),
            Err(StoreError::ReadOnly(Degraded::FutureVersion { .. }))
        ));
        assert_eq!(fs::read(&path).expect("ficheiro"), future);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversize_file_is_never_parsed_and_never_written() {
        let dir = temp_dir("oversize");
        let registry = StoreRegistry::mint_for_test(&dir);
        let path = dir.join("prefs.json");
        // JSON valido e completo, um byte acima do tecto: se chegasse ao
        // parser, carregava.
        let mut valid = br#"{"version":1,"data":{"count":1,"label":""#.to_vec();
        valid.extend(std::iter::repeat_n(b'a', 100));
        valid.extend(br#""}}"#);
        let max = valid.len() as u64 - 1;
        fs::write(&path, &valid).expect("grande");
        let mut store = open(&registry, max);
        // Grande demais nao ganha `.bak`: nunca foi lido, e copia-lo
        // duplicava no disco um ficheiro de tamanho qualquer a cada abertura.
        assert_eq!(
            store.load(),
            LoadOutcome::Degraded {
                value: Prefs::default(),
                why: Degraded::TooLarge {
                    bytes: max + 1,
                    max
                },
                backup: None,
            }
        );
        assert!(matches!(
            store.save(&Prefs::default()),
            Err(StoreError::ReadOnly(Degraded::TooLarge { .. }))
        ));
        // Nem uma loja que grava sem ler antes.
        let mut blind = open(&registry, max);
        assert!(matches!(
            blind.save(&Prefs::default()),
            Err(StoreError::ReadOnly(Degraded::TooLarge { .. }))
        ));
        assert!(matches!(
            blind.update(|prefs| prefs.count += 1),
            Err(StoreError::ReadOnly(Degraded::TooLarge { .. }))
        ));
        assert_eq!(fs::read(&path).expect("ficheiro"), valid);
        assert_eq!(
            entries(&dir),
            vec!["prefs.json".to_string()],
            "um ficheiro grande demais ganhou uma copia"
        );
        // Com o tecto certo, o mesmo ficheiro le-se: foi so o tamanho.
        assert!(matches!(
            open(&registry, max + 1).load(),
            LoadOutcome::Loaded(_)
        ));

        // Um valor que nao cabe no tecto nao e gravado.
        fs::remove_file(&path).expect("apagar");
        let mut small = open(&registry, 32);
        let big = Prefs {
            count: 1,
            label: "x".repeat(64),
        };
        assert!(matches!(
            small.save(&big),
            Err(StoreError::TooLarge { max: 32, .. })
        ));
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_writers_never_lose_an_update() {
        let dir = temp_dir("two-writers");
        let registry = StoreRegistry::mint_for_test(&dir);
        let shared = |registry: &StoreRegistry| open(registry, 4096).shared_between_windows();
        let mut first = shared(&registry);
        let mut second = shared(&registry);

        // As duas leem 0; cada uma soma 1. A segunda soma ao que a primeira
        // gravou, nao ao 0 que tinha lido.
        assert_eq!(first.load().into_value().count, 0);
        assert_eq!(second.load().into_value().count, 0);
        first.update(|prefs| prefs.count += 1).expect("primeira");
        let ((), outcome) = second.update(|prefs| prefs.count += 1).expect("segunda");
        assert_eq!(outcome, SaveOutcome::Written);
        assert_eq!(first.load().into_value().count, 2);

        // Enquanto uma janela esta a meio de um update (trinco na mao), a
        // outra espera; nenhuma perde a escrita da outra.
        let (holding_tx, holding_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            first
                .update(|prefs| {
                    holding_tx.send(()).expect("avisar");
                    release_rx.recv().expect("esperar");
                    prefs.count += 10;
                })
                .expect("update com o trinco");
        });
        holding_rx.recv().expect("o primeiro tem o trinco");
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            second.update(|prefs| prefs.count += 100).expect("update");
            done_tx.send(()).expect("avisar");
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "a segunda janela gravou com o trinco na mao da primeira"
        );
        release_tx.send(()).expect("soltar");
        holder.join().expect("primeira");
        done_rx.recv().expect("a segunda acaba depois");
        waiter.join().expect("segunda");
        assert_eq!(shared(&registry).load().into_value().count, 112);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_kind_is_mandatory() {
        let dir = temp_dir("kind");
        let registry = StoreRegistry::mint_for_test(&dir);
        const WIDTH: StoreSpec =
            StoreSpec::new("panel-width.json", StoreKind::Automatic, StoreShape::File);
        const NOTES: StoreSpec = StoreSpec::new("notas", StoreKind::Explicit, StoreShape::Dir);

        // O grant leva o tipo e a forma que a spec disse.
        let width = registry.grant(WIDTH).expect("grant");
        assert_eq!(
            (width.kind(), width.shape(), width.name()),
            (StoreKind::Automatic, StoreShape::File, "panel-width.json")
        );
        assert_eq!(width.path(), dir.join("panel-width.json"));
        let notes = registry.grant(NOTES).expect("grant");
        assert_eq!(
            (notes.kind(), notes.shape()),
            (StoreKind::Explicit, StoreShape::Dir)
        );

        // Um ficheiro, um tipo: o mesmo nome com outro tipo ou forma e
        // recusado; com o mesmo, dado outra vez.
        assert_eq!(
            registry
                .grant(StoreSpec::new(
                    "panel-width.json",
                    StoreKind::Setting,
                    StoreShape::File
                ))
                .err(),
            Some(GrantError::Conflict {
                name: "panel-width.json",
                granted: StoreKind::Automatic,
                granted_shape: StoreShape::File,
            })
        );
        assert!(
            registry
                .grant(StoreSpec::new(
                    "notas",
                    StoreKind::Explicit,
                    StoreShape::File
                ))
                .is_err()
        );
        assert!(registry.grant(WIDTH).is_ok());
        // O NTFS nao distingue maiusculas: `Panel-Width.json` e o mesmo
        // ficheiro, e um `Setting` nele escrevia o `Automatic` no modo
        // privado. O mesmo tipo com outras maiusculas e o mesmo grant.
        assert_eq!(
            registry
                .grant(StoreSpec::new(
                    "Panel-Width.json",
                    StoreKind::Setting,
                    StoreShape::File
                ))
                .err(),
            Some(GrantError::Conflict {
                name: "Panel-Width.json",
                granted: StoreKind::Automatic,
                granted_shape: StoreShape::File,
            })
        );
        assert_eq!(
            registry
                .grant(StoreSpec::new(
                    "PANEL-WIDTH.JSON",
                    StoreKind::Automatic,
                    StoreShape::File
                ))
                .map(|grant| grant.kind()),
            Ok(StoreKind::Automatic)
        );
        assert!(
            registry
                .grant(StoreSpec::new(
                    "NOTAS",
                    StoreKind::Explicit,
                    StoreShape::File
                ))
                .is_err()
        );

        // Uma loja de pasta nao abre como ficheiro JSON.
        assert!(matches!(
            VersionedJsonStore::<Prefs>::open(notes, 1, 64),
            Err(StoreError::WrongShape { .. })
        ));

        // O tipo decide o modo privado: Automatic nao escreve nada, Setting
        // e Explicit escrevem.
        let mut automatic: VersionedJsonStore<Prefs> =
            VersionedJsonStore::open(width, 1, 4096).expect("abrir");
        let mut setting = open(&registry, 4096);
        let explicit = TokenFile::open(
            registry
                .grant(StoreSpec::new(
                    "explicita",
                    StoreKind::Explicit,
                    StoreShape::File,
                ))
                .expect("grant"),
        )
        .expect("abrir");
        assert_eq!(registry.mode(), StoreMode::Normal);
        registry.set_mode(StoreMode::Private);
        let one = Prefs {
            count: 1,
            label: String::new(),
        };
        assert_eq!(
            automatic.save(&one).expect("privado"),
            SaveOutcome::SkippedPrivate
        );
        assert_eq!(
            automatic.update(|prefs| prefs.count).expect("privado").1,
            SaveOutcome::SkippedPrivate
        );
        assert!(!dir.join("panel-width.json").exists());
        assert_eq!(setting.save(&one).expect("setting"), SaveOutcome::Written);
        assert_eq!(
            explicit.write_token("sim").expect("explicit"),
            SaveOutcome::Written
        );
        registry.set_mode(StoreMode::Normal);
        assert_eq!(automatic.save(&one).expect("normal"), SaveOutcome::Written);
        assert!(dir.join("panel-width.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn grant_names_never_leave_the_data_dir() {
        let registry = StoreRegistry::mint_for_test(std::env::temp_dir().join("neuralia-names"));
        for bad in [
            "",
            "..",
            "../fora.json",
            "a/../../fora",
            "/raiz.json",
            "C:/Windows/x",
            "c:x",
            "a\\b",
            "a//b",
            "./a",
            "espaco .json",
            "ç.json",
            // O Windows tira o ponto final: seria o `panel-width.json`.
            "panel-width.json.",
            "ai./settings.json",
            "...",
        ] {
            assert_eq!(
                registry
                    .grant(StoreSpec::new(bad, StoreKind::Setting, StoreShape::File))
                    .err(),
                Some(GrantError::BadName(bad)),
                "{bad:?}"
            );
        }
        let nested = registry
            .grant(StoreSpec::new(
                "ai/settings.json",
                StoreKind::Setting,
                StoreShape::File,
            ))
            .expect("aninhado");
        assert!(
            nested
                .path()
                .ends_with(Path::new("ai").join("settings.json"))
        );
    }

    #[test]
    fn one_word_files_hold_one_word() {
        let dir = temp_dir("token");
        let registry = StoreRegistry::mint_for_test(&dir);
        let theme = TokenFile::open(
            registry
                .grant(StoreSpec::new(
                    "theme",
                    StoreKind::Setting,
                    StoreShape::File,
                ))
                .expect("grant"),
        )
        .expect("abrir");
        assert_eq!(theme.read_token(), None, "sem ficheiro");
        assert!(!dir.join("theme").exists());
        theme.write_token("escuro").expect("gravar");
        assert_eq!(theme.read_token().as_deref(), Some("escuro"));
        fs::write(dir.join("theme"), "  claro\r\n").expect("com espacos");
        assert_eq!(theme.read_token().as_deref(), Some("claro"));
        for bad in ["duas palavras", "", "<html>", "ção"] {
            fs::write(dir.join("theme"), bad).expect("lixo");
            assert_eq!(theme.read_token(), None, "{bad:?}");
            assert!(matches!(theme.write_token(bad), Err(StoreError::NotAToken)));
            assert_eq!(fs::read_to_string(dir.join("theme")).expect("ler"), bad);
        }
        fs::write(dir.join("theme"), "a".repeat(TokenFile::MAX_BYTES + 1)).expect("grande");
        assert_eq!(theme.read_token(), None);
        assert!(
            TokenFile::open(
                registry
                    .grant(StoreSpec::new("pasta", StoreKind::Setting, StoreShape::Dir))
                    .expect("grant")
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A metade em tempo de execucao do `a_store_cannot_open_without_a_grant`:
    /// o registo cunha-se uma vez por processo. E o UNICO teste do neural-core
    /// que chama `StoreRegistry::mint`; os outros usam `mint_for_test`.
    #[test]
    fn mint_once() {
        let first = StoreRegistry::mint(std::env::temp_dir().join("neuralia-mint-a"));
        let second = StoreRegistry::mint(std::env::temp_dir().join("neuralia-mint-b"));
        assert!(first.is_ok(), "a primeira cunhagem do processo");
        assert_eq!(second.err(), Some(AlreadyMinted), "uma segunda cunhagem");
        assert_eq!(
            StoreRegistry::mint(std::env::temp_dir()).err(),
            Some(AlreadyMinted)
        );
        // O registo de teste nao gasta nem depende da cunhagem.
        let test = StoreRegistry::mint_for_test(std::env::temp_dir().join("neuralia-mint-c"));
        assert!(test.grant(PREFS).is_ok());
    }

    const CAPABILITIES: [&str; 2] = ["StoreGrant", "StoreRegistry"];

    /// O codigo Rust em tokens, sem comentarios: identificadores, cada
    /// literal de texto, byte ou caracter vira `""`, e o resto e um caracter
    /// de pontuacao cada. Chega para achar cabecalhos de `impl`, literais de
    /// struct e chamadas sem se perder em chavetas dentro de um texto.
    fn rust_tokens(code: &str) -> Vec<&str> {
        let mut tokens = Vec::new();
        let mut rest = code;
        while let Some(first) = rest.chars().next() {
            let (len, token) = if first.is_whitespace() {
                (first.len_utf8(), None)
            } else if rest.starts_with("//") {
                (rest.find('\n').unwrap_or(rest.len()), None)
            } else if rest.starts_with("/*") {
                (rest.find("*/").map_or(rest.len(), |end| end + 2), None)
            } else if let Some(len) = literal_len(rest) {
                (len, Some("\"\""))
            } else if first.is_alphanumeric() || first == '_' {
                let len = rest
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                (len, Some(&rest[..len]))
            } else {
                (first.len_utf8(), Some(&rest[..first.len_utf8()]))
            };
            tokens.extend(token);
            rest = &rest[len..];
        }
        tokens
    }

    /// O comprimento do literal no inicio de `text` (`"..."`, `b"..."`,
    /// `r#"..."#`, `'x'`, `b'\n'`), ou `None` (um tempo de vida `'a` nao e
    /// literal; um identificador que comeca por `b`/`r` tambem nao).
    fn literal_len(text: &str) -> Option<usize> {
        let prefix = if text.starts_with("br") {
            2
        } else if text.starts_with(['b', 'r']) {
            1
        } else {
            0
        };
        let body = &text[prefix..];
        if text[..prefix].ends_with('r') {
            let hashes = body.len() - body.trim_start_matches('#').len();
            let open = body[hashes..].strip_prefix('"')?;
            let close = format!("\"{}", "#".repeat(hashes));
            return open
                .find(&close)
                .map(|end| prefix + hashes + 1 + end + close.len());
        }
        if let Some(inner) = body.strip_prefix('"') {
            let mut escaped = false;
            for (at, c) in inner.char_indices() {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => return Some(prefix + 1 + at + 1),
                    _ => {}
                }
            }
            return None;
        }
        let inner = body.strip_prefix('\'')?;
        let len = if inner.starts_with('\\') {
            inner.get(2..)?.find('\'')? + 3
        } else {
            let c = inner.chars().next()?;
            if !inner[c.len_utf8()..].starts_with('\'') {
                return None;
            }
            c.len_utf8() + 1
        };
        Some(prefix + 1 + len)
    }

    enum Scope<'a> {
        Impl(Vec<&'a str>),
        Fn(&'a str),
        Block,
    }

    fn innermost_fn<'a>(stack: &[Scope<'a>]) -> &'a str {
        stack
            .iter()
            .rev()
            .find_map(|scope| match scope {
                Scope::Fn(name) => Some(*name),
                _ => None,
            })
            .unwrap_or("-")
    }

    /// Onde o codigo do modulo nomeia um grant ou o registo.
    #[derive(Debug, Default)]
    struct CapabilitySites {
        /// Cada cabecalho de `impl`, a qualquer profundidade, que os nomeia.
        impls: Vec<String>,
        /// Cada literal `StoreGrant { .. }`/`StoreRegistry { .. }` (ou
        /// `Self { .. }` num impl deles) e a funcao onde esta.
        literals: Vec<String>,
        /// As funcoes que chamam o `build` do registo.
        build_calls: Vec<String>,
        /// `type`, `use`, `const` ou `static` que os nomeiam.
        aliases: Vec<String>,
    }

    fn capability_sites(tokens: &[&str]) -> CapabilitySites {
        let names = |text: &[&str]| CAPABILITIES.into_iter().find(|cap| text.contains(cap));
        let mut sites = CapabilitySites::default();
        let mut stack: Vec<Scope> = Vec::new();
        let mut pending: Option<Scope> = None;
        for (index, &token) in tokens.iter().enumerate() {
            let before = |back: usize| index.checked_sub(back).map_or("", |at| tokens[at]);
            let next = tokens.get(index + 1).copied().unwrap_or("");
            let until = |end: &str| -> Vec<&str> {
                tokens[index..]
                    .iter()
                    .copied()
                    .take_while(|token| *token != end && *token != "{")
                    .collect()
            };
            match token {
                // Um `impl` de item (nao o `impl Trait` de um argumento).
                "impl" if matches!(before(1), "" | "}" | ";" | "]" | "{" | "unsafe") => {
                    let header = until(";");
                    if names(header.as_slice()).is_some() {
                        sites.impls.push(header.join(" "));
                    }
                    pending = Some(Scope::Impl(header));
                }
                "fn" if next.starts_with(|c: char| c.is_alphabetic() || c == '_') => {
                    pending = Some(Scope::Fn(next));
                }
                "{" => stack.push(pending.take().unwrap_or(Scope::Block)),
                "}" => {
                    stack.pop();
                }
                ";" if matches!(pending, Some(Scope::Fn(_))) => pending = None,
                // `&'static str` e um tempo de vida, nao um item.
                "type" | "use" | "const" | "static" if next != "fn" && before(1) != "'" => {
                    let item = tokens[index..]
                        .iter()
                        .copied()
                        .take_while(|token| *token != ";")
                        .collect::<Vec<_>>();
                    if names(item.as_slice()).is_some() {
                        sites.aliases.push(item.join(" "));
                    }
                }
                "build" if next == "(" && matches!(before(1), ":" | ".") => {
                    sites.build_calls.push(innermost_fn(&stack).to_string());
                }
                _ => {}
            }
            // Um literal de struct: `Nome {` que nao e a declaracao, o
            // cabecalho de um impl nem o tipo devolvido antes do corpo.
            let return_type = before(1) == ">" && before(2) == "-";
            if next == "{" && !return_type && !matches!(before(1), "struct" | "impl" | "for") {
                let built = if CAPABILITIES.contains(&token) {
                    Some(token)
                } else if token == "Self" {
                    stack
                        .iter()
                        .rev()
                        .find_map(|scope| match scope {
                            Scope::Impl(header) => Some(names(header.as_slice())),
                            _ => None,
                        })
                        .flatten()
                } else {
                    None
                };
                if let Some(cap) = built {
                    sites
                        .literals
                        .push(format!("{cap} em {}", innermost_fn(&stack)));
                }
            }
        }
        assert!(
            stack.is_empty() && pending.is_none(),
            "chavetas desalinhadas: o leitor de tokens perdeu-se"
        );
        sites.impls.sort();
        sites.literals.sort();
        sites.build_calls.sort();
        sites
    }

    /// Os doctests `compile_fail` deste modulo provam que fora dele nao ha
    /// grant nem registo sem a cunhagem (literal, `clone`, `Default`, `From`)
    /// e que `VersionedJsonStore::open` nao aceita um caminho. Isto prova o
    /// que um doctest nao consegue: nenhuma funcao publica (com outro nome
    /// qualquer) devolve um `StoreGrant` ou um `StoreRegistry`, alem das tres
    /// portas; nenhum trait (`From`, `TryFrom`, `FromStr`, `Deserialize`...)
    /// os implementa -- so os `impl` inerentes e o `#[derive(Debug)]`; um
    /// grant so se constroi no `grant` e um registo so no `build`, que so as
    /// duas cunhagens chamam; nao ha `unsafe`, `macro_rules!`, `include!`,
    /// submodulos nem apelidos que escondam isto; e os doctests continuam
    /// aqui.
    #[test]
    fn a_store_cannot_open_without_a_grant() {
        let source = include_str!("json_store.rs").replace("\r\n", "\n");
        let code = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("codigo");
        let mut impl_of = "";
        let mut depth = 0usize;
        let mut doors = Vec::new();
        let lines: Vec<&str> = code.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if depth == 0 && trimmed.starts_with("impl") {
                impl_of = trimmed;
            }
            if trimmed.starts_with("pub") && trimmed.contains(" fn ") {
                // A assinatura inteira, ate a chaveta.
                let mut signature = String::new();
                for next in &lines[index..] {
                    signature.push_str(next.trim());
                    signature.push(' ');
                    if next.contains('{') || next.trim_end().ends_with(';') {
                        break;
                    }
                }
                let returns = signature
                    .split_once("->")
                    .map(|(_, rest)| rest.split('{').next().unwrap_or(rest))
                    .unwrap_or("");
                let in_capability =
                    impl_of.contains("StoreGrant") || impl_of.contains("StoreRegistry");
                if returns.contains("StoreGrant")
                    || returns.contains("StoreRegistry")
                    || (in_capability && returns.contains("Self"))
                {
                    let name = signature
                        .split(" fn ")
                        .nth(1)
                        .and_then(|rest| rest.split(['(', '<']).next())
                        .unwrap_or("?")
                        .to_string();
                    doors.push(name);
                }
            }
            depth += line.matches('{').count();
            depth = depth.saturating_sub(line.matches('}').count());
        }
        doors.sort();
        assert_eq!(
            doors,
            vec!["grant", "mint", "mint_for_test"],
            "so estas tres funcoes publicas dao um grant ou um registo"
        );
        // Nenhum derive alem de `Debug`: `Clone`, `Default`, `Deserialize`...
        // seriam uma porta com outro nome. So `#[derive(Debug)]`, sem outros
        // atributos.
        for capability in CAPABILITIES {
            let declared = code
                .split(&format!("pub struct {capability} {{"))
                .next()
                .and_then(|before| before.rsplit("\n\n").next())
                .expect("a struct e a sua doc");
            let attributes: Vec<&str> = declared
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with("#["))
                .collect();
            assert_eq!(
                attributes,
                vec!["#[derive(Debug)]"],
                "{capability}: so #[derive(Debug)]"
            );
            assert!(
                !code.contains(&format!("pub struct {capability}(")),
                "{capability} sem campos de tupla"
            );
        }
        // Nenhum trait escrito a mao (`impl From<PathBuf> for StoreGrant`,
        // `FromStr`, `TryFrom`, `Default`...): so os dois impl inerentes. E um
        // grant ou um registo so se constroem nos seus dois sitios.
        let tokens = rust_tokens(code);
        let sites = capability_sites(&tokens);
        assert_eq!(
            sites.impls,
            vec!["impl StoreGrant", "impl StoreRegistry"],
            "so os impl inerentes nomeiam um grant ou o registo: nenhum `impl <Trait> for`"
        );
        assert_eq!(
            sites.literals,
            vec!["StoreGrant em grant", "StoreRegistry em build"],
            "um grant so se constroi no `grant` e um registo so no `build`"
        );
        assert_eq!(
            sites.build_calls,
            vec!["mint", "mint_for_test"],
            "so as duas cunhagens chamam o `build` do registo"
        );
        assert_eq!(
            sites.aliases,
            Vec::<String>::new(),
            "sem `type`, `use`, `const` ou `static` com um grant ou o registo"
        );
        // Nada que gere codigo fora da vista deste gate, nem `unsafe` (que
        // copiava um grant com `ptr::read`).
        for hidden in ["macro_rules", "include", "mod", "unsafe"] {
            assert!(!tokens.contains(&hidden), "`{hidden}` no codigo das lojas");
        }
        assert!(code.contains("\n#![forbid(unsafe_code)]\n"));
        // Os campos das duas sao privados.
        for capability in CAPABILITIES {
            let body = code
                .split(&format!("pub struct {capability} {{"))
                .nth(1)
                .and_then(|rest| rest.split("\n}").next())
                .expect("corpo da struct");
            assert!(
                !body.contains("pub "),
                "{capability} nao pode ter campos publicos"
            );
        }
        // O `mint_for_test` so existe nos testes e na feature de CI.
        assert!(
            code.contains(
                "#[cfg(any(test, feature = \"test-stores\"))]\n    pub fn mint_for_test("
            )
        );
        // Os doctests compile_fail continuam aqui, um por porta fechada.
        for door in [
            "let _forjado = StoreGrant {",
            "let _copia: StoreGrant = grant.clone();",
            "let _forjado: StoreGrant = std::path::PathBuf::from(\"forjado.json\").into();",
            "let _forjado = StoreRegistry {",
            "let _segundo: StoreRegistry = registry.clone();",
            "let _forjado: StoreRegistry = std::env::temp_dir().into();",
            "VersionedJsonStore::<Vec<u32>>::open(std::path::PathBuf::from(\"x.json\"), 1, 4096);",
            "let _sem_tipo = StoreSpec {",
        ] {
            let at = code
                .find(door)
                .unwrap_or_else(|| panic!("doctest em falta: {door}"));
            let fence = code[..at].rfind("```").expect("dentro de um bloco");
            assert!(
                code[fence..at].starts_with("```compile_fail\n"),
                "o doctest de {door} tem de ser compile_fail"
            );
        }
    }
}
