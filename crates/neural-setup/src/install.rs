//! O trabalho da instalacao: onde e que vai, o que se escreve, por que ordem,
//! e quanto e que ja foi feito.
//!
//! Tudo o que decide fica aqui e e portatil -- escrever ficheiros e
//! `std::fs`. Atalhos e registo, que so existem no Windows, vivem no
//! `winshell`. Assim a parte que se pode enganar em silencio (o caminho, a
//! ordem, a percentagem, o que se apaga) testa-se sem ecra e sem tocar na
//! instalacao verdadeira de quem corre os testes.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::archive::Entry;

/// O nome da pasta, do atalho e da chave de desinstalacao.
pub const PRODUCT: &str = "NeuralIA";
/// O executavel que o atalho aponta.
pub const EXECUTABLE: &str = "NeuralIA.exe";
/// O instalador copia-se para a pasta com este nome, para poder desinstalar.
pub const UNINSTALLER: &str = "Desinstalar NeuralIA.exe";
/// O `AppId` do instalador Inno Setup que publicou as 2.1.x. Uma atualizacao
/// por cima dessa instalacao tem de apagar o que ele deixou (o `unins000.*` e
/// a chave `<AppId>_is1`), senao "Aplicacoes" mostra duas NeuralIA.
pub const INNO_APP_ID: &str = "{8B2A98F4-7D55-4C43-ABF0-0D7D1A02C4B9}";
/// Onde o Windows le as entradas de "Aplicacoes", dentro do HKCU.
pub const UNINSTALL_BASE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
/// Pasta de ensaio. Com ela definida, o registo e os atalhos vao para dentro
/// dela em vez dos do utilizador: e o que deixa correr o instalador verdadeiro
/// numa maquina onde a NeuralIA ja esta instalada sem lhe tocar.
pub const SANDBOX_ENV: &str = "NEURALIA_SETUP_SANDBOX";
/// A raiz, dentro do HKCU, das chaves de uma pasta de ensaio.
pub const SANDBOX_REGISTRY_ROOT: &str = "Software\\NeuralIA-SetupSandbox";

#[derive(Debug)]
pub enum InstallError {
    /// Este instalador foi construido sem carga util. Acontece num
    /// `cargo build` normal; nunca devia sair assim para o mundo.
    NoPayload,
    /// A NeuralIA esta aberta: os ficheiros dela nao se deixam substituir.
    InUse(Vec<PathBuf>),
    Io(PathBuf, io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPayload => write!(f, "este instalador foi construido sem a NeuralIA la dentro"),
            Self::InUse(paths) => write!(f, "{}", in_use_message(paths)),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

/// O que se diz a quem tem a NeuralIA aberta. O nome do ficheiro vai junto:
/// "em uso" sem dizer qual nao ajuda ninguem a fecha-lo.
pub fn in_use_message(paths: &[PathBuf]) -> String {
    let first = paths
        .first()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| EXECUTABLE.to_string());
    format!("A NeuralIA esta aberta ({first} em uso). Feche-a e tente outra vez.")
}

/// Porque e que a instalacao nao aconteceu. O tipo decide o codigo de saida do
/// modo silencioso; a mensagem e o que o ecra mostra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Failed,
    BadArguments,
    InUse,
    NoPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<InstallError> for Failure {
    fn from(error: InstallError) -> Self {
        let kind = match &error {
            InstallError::NoPayload => FailureKind::NoPayload,
            InstallError::InUse(_) => FailureKind::InUse,
            InstallError::Io(..) => FailureKind::Failed,
        };
        Self::new(kind, error.to_string())
    }
}

/// Os codigos de saida do modo silencioso (`/S`). Quem o corre -- um script, o
/// `winget`, o CI -- so ve isto; cada falha que se resolve de maneira
/// diferente tem o seu numero.
pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_BAD_ARGUMENTS: i32 = 2;
pub const EXIT_IN_USE: i32 = 3;
pub const EXIT_NO_PAYLOAD: i32 = 4;

pub fn exit_code(outcome: &Result<(), Failure>) -> i32 {
    match outcome {
        Ok(()) => EXIT_OK,
        Err(failure) => match failure.kind {
            FailureKind::Failed => EXIT_FAILED,
            FailureKind::BadArguments => EXIT_BAD_ARGUMENTS,
            FailureKind::InUse => EXIT_IN_USE,
            FailureKind::NoPayload => EXIT_NO_PAYLOAD,
        },
    }
}

/// As fases, pela ordem em que acontecem. A percentagem no ecra sai daqui.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Preparing,
    Writing,
    Shortcuts,
    Registering,
    Done,
}

impl Stage {
    /// O que se le por baixo da barra.
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "A preparar a pasta",
            Self::Writing => "A instalar a NeuralIA",
            Self::Shortcuts => "A criar os atalhos",
            Self::Registering => "A registar no sistema",
            Self::Done => "Instalada",
        }
    }

    /// A fatia da barra que esta fase ocupa, `(inicio, fim)`. Escrever
    /// ficheiros e quase tudo o tempo real; o resto sao instantes, e uma barra
    /// que salta de 50% para 100% no fim parece avariada.
    fn span(self) -> (f64, f64) {
        match self {
            Self::Preparing => (0.00, 0.04),
            Self::Writing => (0.04, 0.88),
            Self::Shortcuts => (0.88, 0.95),
            Self::Registering => (0.95, 0.99),
            Self::Done => (1.00, 1.00),
        }
    }
}

/// A percentagem global, dada a fase e o quanto dela ja foi. Monotona: a barra
/// nunca anda para tras, que e a unica coisa que uma barra nao pode fazer.
pub fn overall_progress(stage: Stage, within: f64) -> f64 {
    let (start, end) = stage.span();
    start + (end - start) * within.clamp(0.0, 1.0)
}

/// Onde a NeuralIA vive. Por utilizador, dentro de `%LOCALAPPDATA%`: instalar
/// em `Program Files` pediria elevacao, e um navegador nao precisa de ser
/// administrador para existir.
pub fn install_root(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Programs").join(PRODUCT)
}

/// Um caminho do Windows na forma em que dois nomes da mesma pasta ficam
/// iguais: sem `\\?\`, sem `.`/`..`, sem barra no fim, em minusculas (o NTFS
/// nao distingue). So para comparar -- nunca para abrir.
pub fn path_key(path: &Path) -> String {
    let mut key = String::new();
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                let text = prefix.as_os_str().to_string_lossy();
                key = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
            }
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(name) => parts.push(name.to_string_lossy().into_owned()),
        }
    }
    for part in parts {
        key.push('\\');
        key.push_str(&part);
    }
    key.to_lowercase()
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// `inner` e `outer`, ou fica algures dentro dele.
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    let (inner, outer) = (path_key(inner), path_key(outer));
    inner == outer
        || (inner.len() > outer.len()
            && inner.starts_with(&outer)
            && inner[outer.len()..].starts_with('\\'))
}

/// Recusa uma pasta de instalacao que poria em risco o que nao e nosso.
///
/// A pasta dos dados (historico, memoria, notas, a chave do Gemini, o perfil
/// do WebView2) e do utilizador: instalar la dentro, ou numa pasta que a
/// contenha, poria ficheiros dele no caminho do que a desinstalacao apaga.
/// Uma raiz de unidade e um caminho relativo recusam-se pela mesma razao --
/// ninguem quer "desinstalar" `C:\`.
pub fn validate_install_root(root: &Path, data_dir: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err(format!(
            "A pasta de instalacao tem de ser um caminho completo, com a unidade: {}",
            root.display()
        ));
    }
    if root.parent().is_none() || !path_key(root).contains('\\') {
        return Err(format!(
            "A NeuralIA nao se instala na raiz de uma unidade: {}",
            root.display()
        ));
    }
    if is_within(root, data_dir) || is_within(data_dir, root) {
        return Err(format!(
            "{} mistura-se com a pasta dos seus dados ({}). Escolha outra pasta.",
            root.display(),
            data_dir.display()
        ));
    }
    Ok(())
}

/// Onde instalar: o que foi pedido com `/D=`; senao onde ja estava (a nossa
/// entrada primeiro, depois a do Inno 2.1.x); senao a pasta de sempre.
///
/// Um `/D=` invalido e erro, nao se troca em silencio por outra pasta. Uma
/// pasta antiga invalida (um registo mexido a mao) e so ignorada.
pub fn choose_install_root(
    explicit: Option<&Path>,
    previous: &[Option<PathBuf>],
    default_root: &Path,
    data_dir: &Path,
) -> Result<PathBuf, String> {
    if let Some(root) = explicit {
        validate_install_root(root, data_dir)?;
        return Ok(root.to_path_buf());
    }
    for root in previous.iter().flatten() {
        if validate_install_root(root, data_dir).is_ok() {
            return Ok(root.clone());
        }
    }
    validate_install_root(default_root, data_dir)?;
    Ok(default_root.to_path_buf())
}

/// De onde desinstalar. O desinstalador vive dentro da pasta que instalou, e
/// e essa -- e so essa -- que ele apaga: um desinstalador corrido de uma pasta
/// de ensaio nunca pode ir apagar a instalacao verdadeira em
/// `%LOCALAPPDATA%\Programs\NeuralIA`.
pub fn choose_uninstall_root(
    explicit: Option<&Path>,
    running_from: Option<&Path>,
    registered: Option<&Path>,
    default_root: &Path,
) -> PathBuf {
    if let Some(root) = explicit {
        return root.to_path_buf();
    }
    if let Some(me) = running_from
        && me
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(UNINSTALLER))
        && let Some(parent) = me.parent()
    {
        return parent.to_path_buf();
    }
    if let Some(root) = registered {
        return root.to_path_buf();
    }
    default_root.to_path_buf()
}

/// So se apaga uma entrada de "Aplicacoes" que aponte para esta pasta: senao,
/// desinstalar uma copia velha noutro sitio tirava a entrada da boa.
pub fn registration_points_here(registered: Option<&Path>, root: &Path) -> bool {
    registered.is_none_or(|location| same_path(location, root))
}

/// Onde ficam as coisas que nao sao ficheiros da aplicacao: a chave de
/// "Aplicacoes" e as pastas dos atalhos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// A chave, dentro do HKCU, onde vivem as entradas de desinstalacao.
    pub uninstall_base: String,
    pub start_menu: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
    /// A pasta quando ninguem diz outra.
    pub default_root: PathBuf,
}

impl Places {
    /// A nossa entrada de "Aplicacoes".
    pub fn key(&self) -> String {
        format!("{}\\{PRODUCT}", self.uninstall_base)
    }

    /// A entrada que o instalador Inno das 2.1.x deixou.
    pub fn inno_key(&self) -> String {
        format!("{}\\{INNO_APP_ID}_is1", self.uninstall_base)
    }

    pub fn shortcut_folders(&self) -> Vec<PathBuf> {
        [self.start_menu.clone(), self.desktop.clone()]
            .into_iter()
            .flatten()
            .collect()
    }
}

/// Os sitios de uma pasta de ensaio: tudo dentro dela, e a chave do registo
/// fora de `...\Uninstall`, onde o Windows nao a mostra a ninguem.
pub fn sandbox_places(sandbox: &Path) -> Result<Places, String> {
    if !sandbox.is_absolute() {
        return Err(format!(
            "{SANDBOX_ENV} tem de ser um caminho completo: {}",
            sandbox.display()
        ));
    }
    let leaf: String = sandbox
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if leaf.is_empty() {
        return Err(format!(
            "{SANDBOX_ENV} precisa de uma pasta com nome: {}",
            sandbox.display()
        ));
    }
    Ok(Places {
        uninstall_base: format!("{SANDBOX_REGISTRY_ROOT}\\{leaf}\\Uninstall"),
        start_menu: Some(sandbox.join("Start Menu").join("Programs")),
        desktop: Some(sandbox.join("Desktop")),
        default_root: install_root(&sandbox.join("LocalAppData")),
    })
}

/// O que ha para fazer.
#[derive(Debug, Clone)]
pub struct Plan {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub desktop_shortcut: bool,
}

impl Plan {
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.data.len() as u64).sum()
    }

    pub fn executable(&self) -> PathBuf {
        self.root.join(EXECUTABLE)
    }

    pub fn uninstaller(&self) -> PathBuf {
        self.root.join(UNINSTALLER)
    }
}

fn target_of(root: &Path, entry: &Entry) -> PathBuf {
    root.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// `NeuralIA.exe` -> `NeuralIA.exe.<suffix>`. Acrescenta em vez de trocar a
/// extensao: `a.exe` e `a.dll` nao podem partilhar o mesmo ficheiro de passagem.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

/// Um ficheiro que outro processo tem aberto sem partilha -- o executavel de
/// uma NeuralIA a correr, por exemplo. Abrir para escrita, sem partilhar e sem
/// truncar nada, e a pergunta que o Windows responde sem efeitos.
#[cfg(windows)]
fn is_locked(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
    {
        Ok(_) => false,
        Err(error) => matches!(
            error.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
        ),
    }
}

#[cfg(not(windows))]
fn is_locked(_path: &Path) -> bool {
    false
}

/// Os ficheiros que, agora, nao se deixam substituir. Vazio e o unico estado
/// em que vale a pena comecar.
pub fn files_in_use(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|path| path.is_file() && is_locked(path))
        .cloned()
        .collect()
}

/// Os ficheiros da carga util, onde ficam instalados.
pub fn payload_files(root: &Path, entries: &[Entry]) -> Vec<PathBuf> {
    entries.iter().map(|e| target_of(root, e)).collect()
}

/// Escreve a carga util. `progress` recebe a fracao ja escrita, entre 0 e 1.
///
/// Tudo ou nada. Primeiro confirma que nenhum ficheiro a substituir esta em
/// uso (a NeuralIA aberta); depois escreve cada ficheiro com um nome de
/// passagem; so quando estao todos no disco e que troca, guardando os antigos
/// ao lado. Se uma troca falhar, os antigos voltam ao sitio. Um disco cheio a
/// meio, ou a NeuralIA aberta, deixam a versao anterior inteira -- nunca meia
/// de cada.
pub fn write_payload(plan: &Plan, mut progress: impl FnMut(f64)) -> Result<(), InstallError> {
    if plan.entries.is_empty() {
        return Err(InstallError::NoPayload);
    }
    let targets = payload_files(&plan.root, &plan.entries);
    let busy = files_in_use(&targets);
    if !busy.is_empty() {
        return Err(InstallError::InUse(busy));
    }

    let total = plan.total_bytes().max(1);
    let mut written = 0u64;
    fs::create_dir_all(&plan.root).map_err(|e| InstallError::Io(plan.root.clone(), e))?;

    let mut staged: Vec<PathBuf> = Vec::with_capacity(targets.len());
    for (entry, target) in plan.entries.iter().zip(&targets) {
        let part = sibling(target, "neuralia-part");
        let result = target
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&part, &entry.data));
        if let Err(error) = result {
            let _ = fs::remove_file(&part);
            discard(&staged);
            return Err(InstallError::Io(part, error));
        }
        staged.push(part);
        written += entry.data.len() as u64;
        // A troca que falta e instantanea; a barra so chega ao fim com ela.
        progress(written as f64 / total as f64 * 0.99);
    }

    // A troca. `swapped` guarda o que ja trocou, para se poder desfazer.
    let mut swapped: Vec<(PathBuf, Option<PathBuf>)> = Vec::with_capacity(targets.len());
    for (part, target) in staged.iter().zip(&targets) {
        let old = if target.exists() {
            let old = sibling(target, "neuralia-old");
            let _ = fs::remove_file(&old);
            if let Err(error) = fs::rename(target, &old) {
                roll_back(&swapped);
                discard(&staged);
                return Err(InstallError::Io(target.clone(), error));
            }
            Some(old)
        } else {
            None
        };
        if let Err(error) = fs::rename(part, target) {
            if let Some(old) = &old {
                let _ = fs::rename(old, target);
            }
            roll_back(&swapped);
            discard(&staged);
            return Err(InstallError::Io(target.clone(), error));
        }
        swapped.push((target.clone(), old));
    }

    // Tudo no sitio: as versoes antigas ja nao servem para nada.
    for old in swapped.iter().filter_map(|(_, old)| old.as_ref()) {
        let _ = fs::remove_file(old);
    }
    progress(1.0);
    Ok(())
}

fn discard(staged: &[PathBuf]) {
    for part in staged {
        let _ = fs::remove_file(part);
    }
}

fn roll_back(swapped: &[(PathBuf, Option<PathBuf>)]) {
    for (target, old) in swapped.iter().rev() {
        let _ = fs::remove_file(target);
        if let Some(old) = old {
            let _ = fs::rename(old, target);
        }
    }
}

/// Copia o proprio instalador para dentro da pasta, onde passa a ser o
/// desinstalador. Por um nome de passagem, como a carga util. Se o que esta a
/// correr ja e o desinstalador da pasta, nao ha nada a copiar.
pub fn place_uninstaller(me: &Path, target: &Path) -> Result<(), InstallError> {
    if same_path(me, target) {
        return Ok(());
    }
    let part = sibling(target, "neuralia-part");
    let _ = fs::remove_file(&part);
    fs::copy(me, &part).map_err(|e| InstallError::Io(part.clone(), e))?;
    fs::rename(&part, target).map_err(|e| {
        let _ = fs::remove_file(&part);
        InstallError::Io(target.to_path_buf(), e)
    })
}

/// Tudo o que a desinstalacao apaga dentro da pasta. Nao apaga a pasta do
/// perfil do utilizador -- historico, memoria e sessoes sao dele, nao nossas.
pub fn installed_files(root: &Path, entries: &[Entry]) -> Vec<PathBuf> {
    let mut paths = payload_files(root, entries);
    paths.push(root.join(UNINSTALLER));
    paths
}

/// A ultima guarda antes de apagar ou substituir o que quer que seja: a pasta
/// e aceitavel e nada do que se vai tocar fica dentro da pasta dos dados do
/// utilizador. Corre no caminho que embarca, antes de instalar e antes de
/// desinstalar -- e e esta mesma funcao que o teste dos dados exercita.
pub fn guard_user_data(
    root: &Path,
    entries: &[Entry],
    legacy: &LegacyCleanup,
    data_dir: &Path,
) -> Result<(), Failure> {
    validate_install_root(root, data_dir)
        .map_err(|why| Failure::new(FailureKind::BadArguments, why))?;
    if let Some(path) = delete_set(root, entries, legacy)
        .into_iter()
        .find(|path| is_within(path, data_dir))
    {
        return Err(Failure::new(
            FailureKind::BadArguments,
            format!(
                "Recusado: {} fica dentro da pasta dos seus dados.",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Tudo o que, em algum momento, o instalador ou o desinstalador apagam ou
/// substituem: a carga util e os seus ficheiros de passagem, o desinstalador,
/// e o que o Inno deixou. Existe para se poder provar que a pasta dos dados
/// nunca esta aqui.
pub fn delete_set(root: &Path, entries: &[Entry], legacy: &LegacyCleanup) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for file in installed_files(root, entries) {
        paths.push(sibling(&file, "neuralia-part"));
        paths.push(sibling(&file, "neuralia-old"));
        paths.push(file);
    }
    paths.extend(legacy.files.iter().cloned());
    paths
}

/// Apaga o que a instalacao escreveu e devolve o que ficou no disco.
///
/// Um ficheiro que ja nao existe conta como apagado. O desinstalador que o
/// Windows corre e o que esta dentro da pasta, e um executavel a correr nao se
/// deixa apagar -- mas deixa-se mover no mesmo disco: vai para `parking` (a
/// pasta temporaria do utilizador) ou, se essa estiver noutro disco, para a
/// pasta de cima da instalacao; e a pasta da instalacao fica vazia. As
/// subpastas da carga util e a raiz so desaparecem se ficaram vazias: o que o
/// utilizador la tiver posto e dele.
pub fn remove_installed(
    root: &Path,
    entries: &[Entry],
    parking: &Path,
    mut progress: impl FnMut(f64),
) -> Result<(), Vec<PathBuf>> {
    let files = installed_files(root, entries);
    let total = files.len().max(1);
    let mut left = Vec::new();
    for (done, path) in files.iter().enumerate() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) if path.ends_with(UNINSTALLER) && park(path, parking, root) => {}
            Err(_) => left.push(path.clone()),
        }
        progress((done + 1) as f64 / total as f64);
    }
    remove_empty_folders(root, &files);
    if left.is_empty() { Ok(()) } else { Err(left) }
}

/// O prefixo de um desinstalador estacionado: `neuralia-uninstaller-<pid>.exe`
/// na pasta temporaria, `.neuralia-uninstaller-<pid>.exe` na pasta de cima.
pub const PARKED_PREFIX: &str = "neuralia-uninstaller-";

/// Os sitios onde o desinstalador a correr se pode mudar, por ordem.
///
/// Mover so funciona dentro do mesmo disco: com a pasta temporaria em `C:` e
/// a instalacao em `D:` (um `/D=`, ou o CI, onde a pasta de trabalho esta
/// noutro disco), o primeiro falha e fica o segundo, ao lado da pasta da
/// instalacao. Sem ele a desinstalacao inteira acabava em "feche a NeuralIA"
/// -- por causa do proprio desinstalador.
pub fn parking_spots(parking: &Path, root: &Path) -> Vec<PathBuf> {
    let name = format!("{PARKED_PREFIX}{}.exe", std::process::id());
    let mut spots = vec![parking.join(&name)];
    if let Some(parent) = root.parent() {
        spots.push(parent.join(format!(".{name}")));
    }
    spots
}

fn park(path: &Path, parking: &Path, root: &Path) -> bool {
    parking_spots(parking, root).into_iter().any(|spot| {
        if let Some(folder) = spot.parent() {
            let _ = fs::create_dir_all(folder);
        }
        let _ = fs::remove_file(&spot);
        fs::rename(path, &spot).is_ok()
    })
}

/// `neuralia-uninstaller-1234.exe` ou `.neuralia-uninstaller-1234.exe`: um
/// desinstalador que uma desinstalacao anterior estacionou.
pub fn is_parked_name(name: &str) -> bool {
    let name = name.strip_prefix('.').unwrap_or(name);
    name.strip_prefix(PARKED_PREFIX)
        .and_then(|rest| rest.strip_suffix(".exe"))
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
}

/// Apaga os desinstaladores estacionados por desinstalacoes anteriores. Os
/// que ainda estiverem a correr nao se deixam apagar e ficam para a proxima.
pub fn sweep_parked(folders: &[PathBuf]) {
    for folder in folders {
        let Ok(read) = fs::read_dir(folder) else {
            continue;
        };
        for item in read.flatten() {
            if is_parked_name(&item.file_name().to_string_lossy())
                && item.file_type().is_ok_and(|kind| kind.is_file())
            {
                let _ = fs::remove_file(item.path());
            }
        }
    }
}

/// As subpastas que a carga util criou, e a raiz, se ficaram vazias. As mais
/// fundas primeiro: uma pasta so sai depois das que tem dentro.
pub fn remove_empty_folders(root: &Path, files: &[PathBuf]) {
    let mut folders: Vec<PathBuf> = files
        .iter()
        .filter_map(|file| file.parent())
        .flat_map(|parent| parent.ancestors().take_while(move |dir| *dir != root))
        .filter(|dir| dir.starts_with(root))
        .map(Path::to_path_buf)
        .collect();
    folders.sort();
    folders.dedup();
    folders.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for folder in folders.iter().map(PathBuf::as_path).chain([root]) {
        let _ = fs::remove_dir(folder);
    }
}

/// O que a chave `<AppId>_is1` do Inno diz.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InnoRegistration {
    pub install_location: Option<PathBuf>,
    pub uninstall_string: Option<String>,
}

impl InnoRegistration {
    /// O executavel da `UninstallString`: `"C:\...\unins000.exe"`, com ou sem
    /// argumentos depois.
    pub fn uninstaller(&self) -> Option<PathBuf> {
        let command = self.uninstall_string.as_deref()?.trim();
        if let Some(rest) = command.strip_prefix('"') {
            let end = rest.find('"')?;
            return Some(PathBuf::from(&rest[..end]));
        }
        let end = command.to_ascii_lowercase().find(".exe")? + 4;
        Some(PathBuf::from(&command[..end]))
    }
}

/// `unins000.exe`, `unins000.dat`, `unins001.msg` -> `unins000`/`unins001`.
/// O Inno numera o desinstalador quando a pasta ja tem outro; o `.dat` e o
/// registo do que ele instalou e o `.msg` as mensagens traduzidas.
pub fn inno_stem(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let (stem, extension) = lower.rsplit_once('.')?;
    if !matches!(extension, "exe" | "dat" | "msg") {
        return None;
    }
    let digits = stem.strip_prefix("unins")?;
    (digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit())).then(|| stem.to_string())
}

/// O inicio de um `unins???.dat` e o cabecalho do registo de desinstalacao do
/// Inno: `Inno Setup Uninstall Log`, e o `AppId` em ASCII logo a seguir. E a
/// prova de que um `unins000.exe` e da NeuralIA e nao de outro programa que
/// partilhe a pasta -- o nome `unins000` e o mesmo em todos.
pub fn is_neuralia_inno_log(head: &[u8]) -> bool {
    let needle = INNO_APP_ID.as_bytes();
    head.starts_with(b"Inno Setup Uninstall Log")
        && head
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

/// O que ha dentro de uma pasta de instalacao, para decidir a limpeza sem
/// tocar em nada: os nomes dos ficheiros, e quais dos `unins???.dat` sao
/// registos da NeuralIA.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folder {
    pub names: Vec<String>,
    pub neuralia_logs: Vec<String>,
}

/// Le a pasta: so o nome de cada ficheiro e o cabecalho dos `unins???.dat`.
pub fn read_folder(root: &Path) -> Folder {
    let mut folder = Folder::default();
    let Ok(read) = fs::read_dir(root) else {
        return folder;
    };
    for item in read.flatten() {
        if !item.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        if name.to_ascii_lowercase().ends_with(".dat") && inno_stem(&name).is_some() {
            let mut head = vec![0u8; 512];
            let read = fs::File::open(item.path())
                .and_then(|mut file| io::Read::read(&mut file, &mut head))
                .unwrap_or(0);
            if is_neuralia_inno_log(&head[..read]) {
                folder.neuralia_logs.push(name.clone());
            }
        }
        folder.names.push(name);
    }
    folder.names.sort();
    folder.neuralia_logs.sort();
    folder
}

/// O que sobrou do instalador Inno das 2.1.x e tem de sair.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyCleanup {
    /// Ficheiros `unins???.*` desta pasta que sao da NeuralIA.
    pub files: Vec<PathBuf>,
    /// Se a chave `<AppId>_is1` sai de "Aplicacoes".
    pub unregister: bool,
}

/// Decide o que se apaga do Inno, sem apagar nada.
///
/// Um `unins???` so e nosso se o `.dat` dele tiver o nosso `AppId`, ou se a
/// chave do Inno apontar para esta pasta e para ele. A chave sai quando aponta
/// para esta pasta -- a NeuralIA que ela descrevia e a que acabamos de
/// substituir (ou remover) -- ou quando aponta para uma pasta onde ja nao ha
/// NeuralIA nenhuma. Uma chave que aponta para outra instalacao que ainda
/// existe fica: essa NeuralIA ainda funciona e ainda se desinstala por la.
pub fn plan_legacy_cleanup(
    root: &Path,
    folder: &Folder,
    registration: Option<&InnoRegistration>,
    registered_app_present: bool,
) -> LegacyCleanup {
    let registered_here = registration
        .and_then(|r| r.install_location.as_deref())
        .is_some_and(|location| same_path(location, root));

    let mut ours: Vec<String> = folder
        .neuralia_logs
        .iter()
        .filter_map(|name| inno_stem(name))
        .collect();
    if registered_here
        && let Some(exe) = registration.and_then(InnoRegistration::uninstaller)
        && exe.parent().is_some_and(|parent| same_path(parent, root))
        && let Some(stem) = exe
            .file_name()
            .and_then(|name| inno_stem(&name.to_string_lossy()))
    {
        ours.push(stem);
    }

    let files = folder
        .names
        .iter()
        .filter(|name| inno_stem(name).is_some_and(|stem| ours.contains(&stem)))
        .map(|name| root.join(name))
        .collect();
    let unregister = registration.is_some() && (registered_here || !registered_app_present);
    LegacyCleanup { files, unregister }
}

/// Apaga os ficheiros do Inno e devolve os que ficaram.
pub fn remove_legacy_files(cleanup: &LegacyCleanup) -> Vec<PathBuf> {
    cleanup
        .files
        .iter()
        .filter(|path| match fs::remove_file(path) {
            Ok(()) => false,
            Err(e) => e.kind() != io::ErrorKind::NotFound,
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: usize) -> Entry {
        Entry {
            path: path.to_string(),
            data: vec![9u8; size],
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neuralia-setup-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_bar_only_ever_moves_forward() {
        // Uma barra que anda para tras e a forma mais rapida de a instalacao
        // parecer avariada.
        let mut last = -1.0;
        for stage in [
            Stage::Preparing,
            Stage::Writing,
            Stage::Shortcuts,
            Stage::Registering,
            Stage::Done,
        ] {
            for step in 0..=10 {
                let value = overall_progress(stage, step as f64 / 10.0);
                assert!(
                    value >= last,
                    "{stage:?} em {step}/10 recuou de {last} para {value}"
                );
                assert!((0.0..=1.0).contains(&value));
                last = value;
            }
        }
        assert_eq!(overall_progress(Stage::Done, 1.0), 1.0);
    }

    #[test]
    fn progress_outside_the_range_does_not_escape_the_bar() {
        assert_eq!(
            overall_progress(Stage::Writing, -5.0),
            overall_progress(Stage::Writing, 0.0)
        );
        assert_eq!(
            overall_progress(Stage::Writing, 9.0),
            overall_progress(Stage::Writing, 1.0)
        );
    }

    #[test]
    fn writing_files_is_most_of_the_bar() {
        // Se escrever 40 MB ocupasse um quinto da barra, ela ficaria parada
        // quase todo o tempo e depois saltava. E o tempo real que manda.
        let (start, end) = Stage::Writing.span();
        assert!(
            end - start > 0.7,
            "escrever ocupa so {:.0}% da barra",
            (end - start) * 100.0
        );
    }

    #[test]
    fn the_install_folder_is_per_user_and_needs_no_administrator() {
        let root = install_root(Path::new("C:/Users/alguem/AppData/Local"));
        assert!(root.ends_with("Programs/NeuralIA") || root.ends_with("Programs\\NeuralIA"));
        let shown = root.to_string_lossy().to_lowercase();
        assert!(
            !shown.contains("program files"),
            "instalar aqui pedia elevacao: {shown}"
        );
    }

    #[test]
    fn an_installer_built_without_a_payload_refuses_instead_of_making_an_empty_folder() {
        let dir = std::env::temp_dir().join("neuralia-setup-empty");
        let plan = Plan {
            root: dir,
            entries: Vec::new(),
            desktop_shortcut: false,
        };
        assert!(matches!(
            write_payload(&plan, |_| {}),
            Err(InstallError::NoPayload)
        ));
    }

    #[test]
    fn every_file_lands_where_the_package_said_and_the_bar_reaches_the_end() {
        let dir = std::env::temp_dir().join(format!("neuralia-setup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plan = Plan {
            root: dir.clone(),
            entries: vec![
                entry("NeuralIA.exe", 2048),
                entry("assets/logo.ico", 256),
                entry("a/b/c.dll", 64),
            ],
            desktop_shortcut: true,
        };

        let mut seen: Vec<f64> = Vec::new();
        write_payload(&plan, |fraction| seen.push(fraction)).expect("instalar");

        for path in installed_files(&dir, &plan.entries) {
            if path.ends_with(UNINSTALLER) {
                continue;
            }
            assert!(path.is_file(), "faltou {}", path.display());
        }
        // Nenhum `.neuralia-part` sobrevive a uma instalacao que correu bem.
        let leftovers: Vec<_> = walk(&dir)
            .into_iter()
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e == "neuralia-part" || e == "neuralia-old")
            })
            .collect();
        assert!(leftovers.is_empty(), "ficheiros a meio: {leftovers:?}");

        assert_eq!(seen.last().copied(), Some(1.0), "a barra nao chegou ao fim");
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "a barra recuou");
        let _ = fs::remove_dir_all(&dir);
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(read) = fs::read_dir(dir) else {
            return out;
        };
        for item in read.flatten() {
            let path = item.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    #[test]
    fn reinstalling_over_a_previous_copy_replaces_it() {
        let dir = std::env::temp_dir().join(format!("neuralia-setup-again-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let old = Plan {
            root: dir.clone(),
            entries: vec![entry("NeuralIA.exe", 4096)],
            desktop_shortcut: false,
        };
        write_payload(&old, |_| {}).expect("primeira");
        let new = Plan {
            root: dir.clone(),
            entries: vec![Entry {
                path: "NeuralIA.exe".into(),
                data: b"nova versao".to_vec(),
            }],
            desktop_shortcut: false,
        };
        write_payload(&new, |_| {}).expect("segunda");
        assert_eq!(
            fs::read(dir.join("NeuralIA.exe")).expect("ler"),
            b"nova versao",
            "a segunda instalacao devia substituir a primeira"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_payload_that_cannot_be_written_whole_leaves_the_previous_version_whole() {
        // Nunca meia instalacao: se o segundo ficheiro nao se escreve, o
        // primeiro nao pode ja ter sido trocado pela versao nova.
        let dir = temp("whole");
        fs::create_dir_all(&dir).expect("pasta");
        fs::write(dir.join(EXECUTABLE), b"versao antiga").expect("antiga");
        // `bloqueio` e um ficheiro: a subpasta de que o segundo precisa nao
        // se pode criar.
        fs::write(dir.join("bloqueio"), b"x").expect("bloqueio");
        let plan = Plan {
            root: dir.clone(),
            entries: vec![
                Entry {
                    path: EXECUTABLE.into(),
                    data: b"versao nova".to_vec(),
                },
                entry("bloqueio/extra.dll", 16),
            ],
            desktop_shortcut: false,
        };
        let result = write_payload(&plan, |_| {});
        assert!(matches!(result, Err(InstallError::Io(..))), "{result:?}");
        assert_eq!(
            fs::read(dir.join(EXECUTABLE)).expect("ler"),
            b"versao antiga",
            "a falha a meio deixou uma instalacao meio nova, meio velha"
        );
        let leftovers: Vec<_> = walk(&dir)
            .into_iter()
            .filter(|p| p.to_string_lossy().contains(".neuralia-"))
            .collect();
        assert!(leftovers.is_empty(), "ficaram restos: {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_open_neuralia_stops_the_install_before_anything_is_written() {
        // O dono com a NeuralIA aberta carrega em "Instalar": nada muda, e o
        // erro diz que e isso -- o modo silencioso sai com o codigo proprio.
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp("inuse");
        fs::create_dir_all(&dir).expect("pasta");
        let exe = dir.join(EXECUTABLE);
        fs::write(&exe, b"versao antiga").expect("antiga");
        let hold = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&exe)
            .expect("segurar");
        let plan = Plan {
            root: dir.clone(),
            entries: vec![Entry {
                path: EXECUTABLE.into(),
                data: b"versao nova".to_vec(),
            }],
            desktop_shortcut: false,
        };
        let result = write_payload(&plan, |_| {});
        drop(hold);
        let Err(InstallError::InUse(busy)) = &result else {
            panic!("esperava InUse, veio {result:?}");
        };
        assert_eq!(busy, &vec![exe.clone()]);
        assert_eq!(
            exit_code(&result.map_err(Failure::from)),
            EXIT_IN_USE,
            "o modo silencioso tem de dizer que a NeuralIA esta aberta"
        );
        assert_eq!(fs::read(&exe).expect("ler"), b"versao antiga");
        assert!(!dir.join("NeuralIA.exe.neuralia-part").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_running_neuralia_executable_is_seen_as_in_use() {
        // Um executavel a correr deixa-se RENOMEAR no Windows: sem a
        // verificacao previa, a instalacao trocava-o por baixo da NeuralIA
        // aberta. Aqui corre de verdade (o `sort.exe` do sistema, a esperar
        // pelo stdin, com o nome da NeuralIA).
        let dir = temp("running");
        fs::create_dir_all(&dir).expect("pasta");
        let exe = dir.join(EXECUTABLE);
        let system = std::env::var_os("SystemRoot").expect("SystemRoot");
        fs::copy(
            PathBuf::from(system).join("System32").join("sort.exe"),
            &exe,
        )
        .expect("copiar");
        let before = fs::read(&exe).expect("ler");
        let mut running = std::process::Command::new(&exe)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("correr");
        let plan = Plan {
            root: dir.clone(),
            entries: vec![Entry {
                path: EXECUTABLE.into(),
                data: b"versao nova".to_vec(),
            }],
            desktop_shortcut: false,
        };
        let result = write_payload(&plan, |_| {});
        drop(running.stdin.take());
        let _ = running.kill();
        let _ = running.wait();
        assert!(
            matches!(&result, Err(InstallError::InUse(busy)) if busy == &vec![exe.clone()]),
            "com a NeuralIA a correr a instalacao disse {result:?}"
        );
        assert_eq!(fs::read(&exe).expect("ler"), before);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_failure_has_its_own_exit_code_and_success_is_zero() {
        let codes = [
            exit_code(&Ok(())),
            exit_code(&Err(Failure::new(FailureKind::Failed, ""))),
            exit_code(&Err(Failure::new(FailureKind::BadArguments, ""))),
            exit_code(&Err(Failure::new(FailureKind::InUse, ""))),
            exit_code(&Err(Failure::new(FailureKind::NoPayload, ""))),
        ];
        assert_eq!(codes, [0, 1, 2, 3, 4]);
        assert_eq!(
            Failure::from(InstallError::NoPayload).kind,
            FailureKind::NoPayload
        );
        assert_eq!(
            Failure::from(InstallError::InUse(vec![])).kind,
            FailureKind::InUse
        );
    }

    #[test]
    fn paths_compare_like_windows_compares_them() {
        assert!(same_path(
            Path::new(r"C:\Users\Eu\AppData\Local\Programs\NeuralIA\"),
            Path::new(r"c:\users\eu\appdata\local\programs\neuralia")
        ));
        assert!(same_path(
            Path::new(r"\\?\C:\a\b"),
            Path::new(r"C:\a\x\..\b\.")
        ));
        assert!(!same_path(Path::new(r"C:\a\b"), Path::new(r"C:\a\bc")));
        assert!(is_within(Path::new(r"C:\a\b\c"), Path::new(r"C:\a\b")));
        assert!(is_within(Path::new(r"C:\a\b"), Path::new(r"C:\A\B\")));
        assert!(!is_within(Path::new(r"C:\a\bc"), Path::new(r"C:\a\b")));
        assert!(!is_within(Path::new(r"C:\a"), Path::new(r"C:\a\b")));
    }

    /// A pasta de dados como a NeuralIA a calcula de verdade, sem mexer no
    /// ambiente dos outros testes: `%LOCALAPPDATA%\NeuralIA`.
    fn real_layout() -> (PathBuf, PathBuf) {
        let local = PathBuf::from(r"C:\Users\alguem\AppData\Local");
        (install_root(&local), local.join("NeuralIA"))
    }

    #[test]
    fn the_data_folder_is_never_in_what_setup_deletes() {
        // O historico, a memoria, as notas, a chave do Gemini e o perfil do
        // WebView2 vivem em `%LOCALAPPDATA%\NeuralIA`. Para cada pasta de
        // instalacao que o instalador ACEITA, nada do que ele apaga ou
        // substitui (a carga util, os ficheiros de passagem, o desinstalador,
        // o `unins000.*`) pode cair la dentro.
        let (default_root, data) = real_layout();
        let entries = vec![entry(EXECUTABLE, 1), entry("assets/a.bin", 1)];
        let candidates = [
            default_root.clone(),
            data.clone(),
            data.join("WebView2"),
            data.join("Programs").join(PRODUCT),
            data.parent().expect("pai").to_path_buf(),
            PathBuf::from(r"C:\Users\alguem\AppData"),
            PathBuf::from(r"C:\Users\alguem\AppData\Local\neuralia"),
            PathBuf::from(r"C:\Users\alguem\AppData\Local\Programs\NeuralIA\..\..\NeuralIA"),
            PathBuf::from(r"D:\Apps\NeuralIA"),
        ];
        let mut accepted = 0;
        for root in &candidates {
            let folder = Folder {
                names: vec!["unins000.exe".into(), "unins000.dat".into()],
                neuralia_logs: vec!["unins000.dat".into()],
            };
            let legacy = plan_legacy_cleanup(root, &folder, None, false);
            if let Err(failure) = guard_user_data(root, &entries, &legacy, &data) {
                assert_eq!(failure.kind, FailureKind::BadArguments);
                continue;
            }
            accepted += 1;
            for path in delete_set(root, &entries, &legacy) {
                assert!(
                    !is_within(&path, &data),
                    "instalar em {} apagaria {} -- dentro dos dados do dono",
                    root.display(),
                    path.display()
                );
            }
        }
        assert_eq!(accepted, 2, "so a pasta de sempre e D:\\Apps\\NeuralIA");
        // E a pasta de sempre fica fora dos dados, por construcao.
        assert!(validate_install_root(&default_root, &data).is_ok());
    }

    #[test]
    fn an_install_folder_that_mixes_with_the_data_folder_is_refused() {
        let (_, data) = real_layout();
        for root in [
            data.clone(),
            data.join("WebView2"),
            data.parent().expect("pai").to_path_buf(),
        ] {
            assert!(
                validate_install_root(&root, &data).is_err(),
                "{} devia ser recusada",
                root.display()
            );
            assert!(
                choose_install_root(Some(&root), &[], &root, &data).is_err(),
                "um /D= para {} devia falhar em vez de instalar",
                root.display()
            );
        }
        for root in [r"C:\", r"relativa\NeuralIA", r"\sem\unidade"] {
            assert!(
                validate_install_root(Path::new(root), &data).is_err(),
                "{root} devia ser recusada"
            );
        }
    }

    #[test]
    fn the_install_goes_where_it_was_asked_else_where_it_already_was() {
        let (default_root, data) = real_layout();
        let asked = PathBuf::from(r"D:\Apps\Neural IA");
        let ours = PathBuf::from(r"E:\NeuralIA");
        let inno = PathBuf::from(r"F:\Programas\NeuralIA");
        assert_eq!(
            choose_install_root(
                Some(&asked),
                &[Some(ours.clone()), Some(inno.clone())],
                &default_root,
                &data
            ),
            Ok(asked)
        );
        assert_eq!(
            choose_install_root(
                None,
                &[Some(ours.clone()), Some(inno.clone())],
                &default_root,
                &data
            ),
            Ok(ours)
        );
        assert_eq!(
            choose_install_root(None, &[None, Some(inno.clone())], &default_root, &data),
            Ok(inno)
        );
        // Um registo que aponta para os dados nao arrasta a instalacao para la.
        assert_eq!(
            choose_install_root(None, &[Some(data.clone())], &default_root, &data),
            Ok(default_root.clone())
        );
        assert_eq!(
            choose_install_root(None, &[], &default_root, &data),
            Ok(default_root)
        );
    }

    #[test]
    fn the_uninstaller_removes_the_folder_it_lives_in_and_no_other() {
        // O CI instala numa pasta temporaria e corre o desinstalador de la.
        // Se ele fosse apagar `%LOCALAPPDATA%\Programs\NeuralIA`, cada corrida
        // do CI numa maquina de programador desinstalava a NeuralIA dele.
        let (default_root, _) = real_layout();
        let trial = PathBuf::from(r"C:\Temp\smoke\Neural IA");
        let me = trial.join(UNINSTALLER);
        assert_eq!(
            choose_uninstall_root(None, Some(&me), Some(&default_root), &default_root),
            trial
        );
        // Corrido de outro sitio (o instalador descarregado com --uninstall),
        // segue o registo; sem registo, a pasta de sempre.
        let downloads = PathBuf::from(r"C:\Users\eu\Downloads\NeuralIA-Setup-2.1.6-x64.exe");
        assert_eq!(
            choose_uninstall_root(None, Some(&downloads), Some(&trial), &default_root),
            trial
        );
        assert_eq!(
            choose_uninstall_root(None, Some(&downloads), None, &default_root),
            default_root
        );
        let explicit = PathBuf::from(r"D:\outra");
        assert_eq!(
            choose_uninstall_root(Some(&explicit), Some(&me), None, &default_root),
            explicit
        );
    }

    #[test]
    fn only_the_registration_of_this_folder_is_removed() {
        let root = Path::new(r"C:\Temp\smoke\NeuralIA");
        assert!(registration_points_here(Some(root), root));
        assert!(registration_points_here(
            Some(Path::new(r"c:\temp\smoke\neuralia\")),
            root
        ));
        assert!(registration_points_here(None, root));
        assert!(!registration_points_here(
            Some(Path::new(r"C:\Users\eu\AppData\Local\Programs\NeuralIA")),
            root
        ));
    }

    #[test]
    fn a_sandbox_keeps_the_registry_and_shortcuts_away_from_the_users() {
        let sandbox = Path::new(r"C:\Temp\neuralia-setup-sandbox-1234");
        let places = sandbox_places(sandbox).expect("pasta de ensaio");
        assert!(
            !places.key().starts_with(UNINSTALL_BASE),
            "a chave de ensaio apareceria em Aplicacoes: {}",
            places.key()
        );
        assert!(places.key().starts_with(SANDBOX_REGISTRY_ROOT));
        assert!(places.inno_key().starts_with(SANDBOX_REGISTRY_ROOT));
        assert!(places.key().contains("neuralia-setup-sandbox-1234"));
        for folder in places.shortcut_folders() {
            assert!(is_within(&folder, sandbox), "{}", folder.display());
        }
        assert!(is_within(&places.default_root, sandbox));
        assert!(sandbox_places(Path::new("relativa")).is_err());
    }

    fn inno_log() -> Vec<u8> {
        // O cabecalho real: 64 bytes de assinatura e o AppId em ASCII.
        let mut head = b"Inno Setup Uninstall Log (b) 64-bit".to_vec();
        head.resize(64, 0);
        head.extend_from_slice(INNO_APP_ID.as_bytes());
        head.resize(448, 0);
        head
    }

    fn inno_registration(root: &Path) -> InnoRegistration {
        InnoRegistration {
            install_location: Some(PathBuf::from(format!("{}\\", root.display()))),
            uninstall_string: Some(format!("\"{}\"", root.join("unins000.exe").display())),
        }
    }

    #[test]
    fn the_inno_uninstaller_of_neuralia_is_recognised_by_its_log() {
        assert!(is_neuralia_inno_log(&inno_log()));
        let mut other = inno_log();
        other[64] = b'[';
        assert!(
            !is_neuralia_inno_log(&other),
            "o desinstalador de outro programa nao e nosso"
        );
        assert!(!is_neuralia_inno_log(b"MZ qualquer coisa"));
        assert_eq!(inno_stem("unins000.exe").as_deref(), Some("unins000"));
        assert_eq!(inno_stem("UNINS001.DAT").as_deref(), Some("unins001"));
        assert_eq!(inno_stem("unins000.msg").as_deref(), Some("unins000"));
        for name in ["unins00.exe", "unins0000.exe", "unins000.dll", EXECUTABLE] {
            assert_eq!(inno_stem(name), None, "{name}");
        }
        let with_args = InnoRegistration {
            install_location: None,
            uninstall_string: Some(r#""C:\a b\unins000.exe" /SILENT"#.into()),
        };
        assert_eq!(
            with_args.uninstaller(),
            Some(PathBuf::from(r"C:\a b\unins000.exe"))
        );
        let bare = InnoRegistration {
            install_location: None,
            uninstall_string: Some(r"C:\a b\unins000.exe /SILENT".into()),
        };
        assert_eq!(
            bare.uninstaller(),
            Some(PathBuf::from(r"C:\a b\unins000.exe"))
        );
    }

    #[test]
    fn upgrading_over_the_inno_install_removes_its_uninstaller_and_its_entry() {
        // O estado da maquina do dono: a 2.1.5 do Inno, com `unins000.*` e a
        // chave `{AppId}_is1` a apontar para esta pasta.
        let root = PathBuf::from(r"C:\Users\eu\AppData\Local\Programs\NeuralIA");
        let folder = Folder {
            names: vec![
                UNINSTALLER.into(),
                EXECUTABLE.into(),
                "unins000.dat".into(),
                "unins000.exe".into(),
                "notas do dono.txt".into(),
            ],
            neuralia_logs: vec!["unins000.dat".into()],
        };
        let registration = inno_registration(&root);
        let plan = plan_legacy_cleanup(&root, &folder, Some(&registration), true);
        assert_eq!(
            plan.files,
            vec![root.join("unins000.dat"), root.join("unins000.exe")]
        );
        assert!(
            plan.unregister,
            "a chave do Inno ficava: Aplicacoes mostrava duas NeuralIA"
        );

        // Sem o `.dat` (apagado a mao), a chave ainda prova que e nosso.
        let no_log = Folder {
            neuralia_logs: vec![],
            ..folder.clone()
        };
        let plan = plan_legacy_cleanup(&root, &no_log, Some(&registration), true);
        assert_eq!(
            plan.files,
            vec![root.join("unins000.dat"), root.join("unins000.exe")]
        );

        // Sem a chave, o `.dat` prova-o sozinho, e nao ha entrada a tirar.
        let plan = plan_legacy_cleanup(&root, &folder, None, true);
        assert_eq!(plan.files.len(), 2);
        assert!(!plan.unregister);
    }

    #[test]
    fn another_programs_inno_uninstaller_is_left_alone() {
        // `unins000` e o nome de todos os desinstaladores Inno. Numa pasta
        // escolhida com /D=, um que nao tenha o nosso AppId nao e nosso.
        let root = PathBuf::from(r"D:\Apps");
        let folder = Folder {
            names: vec!["unins000.dat".into(), "unins000.exe".into()],
            neuralia_logs: vec![],
        };
        let plan = plan_legacy_cleanup(&root, &folder, None, false);
        assert!(plan.files.is_empty(), "{:?}", plan.files);
        // Nem quando a chave da NeuralIA existe mas aponta para outra pasta,
        // onde a NeuralIA do Inno ainda vive: essa fica, com a sua entrada.
        let elsewhere = inno_registration(Path::new(r"C:\Outra\NeuralIA"));
        let plan = plan_legacy_cleanup(&root, &folder, Some(&elsewhere), true);
        assert!(plan.files.is_empty());
        assert!(!plan.unregister);
        // Se essa outra pasta ja nao tem NeuralIA, a entrada e lixo e sai.
        let plan = plan_legacy_cleanup(&root, &folder, Some(&elsewhere), false);
        assert!(plan.files.is_empty());
        assert!(plan.unregister);
    }

    #[test]
    fn the_inno_leftovers_are_found_on_a_real_folder_and_removed() {
        let dir = temp("inno");
        fs::create_dir_all(&dir).expect("pasta");
        fs::write(dir.join(EXECUTABLE), b"2.1.5").expect("exe");
        fs::write(dir.join("unins000.exe"), b"MZ inno").expect("unins");
        fs::write(dir.join("unins000.dat"), inno_log()).expect("dat");
        fs::write(
            dir.join("unins001.dat"),
            b"Inno Setup Uninstall Log (b) outro",
        )
        .expect("outro");
        fs::write(dir.join("unins001.exe"), b"MZ outro").expect("outro exe");

        let folder = read_folder(&dir);
        assert_eq!(folder.neuralia_logs, vec!["unins000.dat".to_string()]);
        let plan = plan_legacy_cleanup(&dir, &folder, None, true);
        assert!(remove_legacy_files(&plan).is_empty());
        assert!(!dir.join("unins000.exe").exists());
        assert!(!dir.join("unins000.dat").exists());
        assert!(
            dir.join("unins001.exe").exists(),
            "o de outro programa ficou"
        );
        assert!(dir.join(EXECUTABLE).exists(), "a NeuralIA nao e do Inno");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_running_uninstaller_that_cannot_reach_temp_still_empties_its_folder() {
        // A pasta temporaria noutro disco (o CI: RUNNER_TEMP em D:, TEMP em
        // C:; ou um /D= noutra unidade): mover para la falha. Sem o segundo
        // sitio, o proprio desinstalador ficava na pasta e a desinstalacao
        // inteira acabava em "feche a NeuralIA".
        let base = temp("park");
        let apps = base.join("Apps");
        let root = apps.join(PRODUCT);
        fs::create_dir_all(&root).expect("pasta");
        fs::write(root.join(EXECUTABLE), b"app").expect("exe");
        let uninstaller = root.join(UNINSTALLER);
        let system = std::env::var_os("SystemRoot").expect("SystemRoot");
        fs::copy(
            PathBuf::from(system).join("System32").join("sort.exe"),
            &uninstaller,
        )
        .expect("copiar");
        let mut running = std::process::Command::new(&uninstaller)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("correr o desinstalador");
        // Uma "pasta temporaria" inalcancavel: debaixo de um ficheiro.
        let blocked = base.join("bloqueio");
        fs::write(&blocked, b"x").expect("bloqueio");
        let unreachable = blocked.join("Temp");

        let result = remove_installed(&root, &[entry(EXECUTABLE, 3)], &unreachable, |_| {});
        let parked = apps.join(format!(".{PARKED_PREFIX}{}.exe", std::process::id()));
        let parked_there = parked.exists();
        let root_left = root.exists();
        drop(running.stdin.take());
        let _ = running.kill();
        let _ = running.wait();

        assert_eq!(result, Ok(()));
        assert!(!root_left, "a pasta ficou com o desinstalador dentro");
        assert!(
            parked_there,
            "o desinstalador nao foi para ao lado da pasta"
        );
        // A proxima corrida varre-o.
        sweep_parked(&[unreachable, apps.clone()]);
        assert!(
            !parked.exists(),
            "o desinstalador estacionado ficou para sempre"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn only_parked_uninstallers_are_swept() {
        for name in [
            "neuralia-uninstaller-1234.exe",
            ".neuralia-uninstaller-7.exe",
        ] {
            assert!(is_parked_name(name), "{name}");
        }
        for name in [
            "neuralia-uninstaller-.exe",
            "neuralia-uninstaller-12a.exe",
            "neuralia-uninstaller-12.exe.bak",
            "outro-uninstaller-12.exe",
            EXECUTABLE,
            UNINSTALLER,
            "history.jsonl",
        ] {
            assert!(!is_parked_name(name), "{name}");
        }
        let dir = temp("sweep");
        fs::create_dir_all(&dir).expect("pasta");
        for name in ["neuralia-uninstaller-99.exe", "notas.txt", EXECUTABLE] {
            fs::write(dir.join(name), b"x").expect("ficheiro");
        }
        sweep_parked(std::slice::from_ref(&dir));
        assert!(!dir.join("neuralia-uninstaller-99.exe").exists());
        assert!(dir.join("notas.txt").exists());
        assert!(dir.join(EXECUTABLE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn placing_the_uninstaller_over_itself_is_a_no_op() {
        let dir = temp("self-copy");
        fs::create_dir_all(&dir).expect("pasta");
        let target = dir.join(UNINSTALLER);
        fs::write(&target, b"eu").expect("eu");
        place_uninstaller(&target, &target).expect("nada a fazer");
        assert_eq!(fs::read(&target).expect("ler"), b"eu");
        let other = dir.join("novo.exe");
        fs::write(&other, b"novo").expect("novo");
        place_uninstaller(&other, &target).expect("copiar");
        assert_eq!(fs::read(&target).expect("ler"), b"novo");
        assert!(!dir.join("Desinstalar NeuralIA.exe.neuralia-part").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
