//! O gestor de downloads (downloads-manager, plano 2.3): tudo o que decide o
//! que acontece a um download, sem WebView2 e sem relogio -- testavel em
//! qualquer plataforma. O COM (o `DownloadStarting`, o deferral, o
//! `BytesReceivedChanged`, o `StateChanged`) vive em
//! `neural-app/src/windows_app/downloads.rs` e so traduz: cada aviso do
//! WebView2 vira um [`DownloadEvent`], e o que [`DownloadManager::on_event`]
//! devolve ([`DownloadEffect`]) vira uma chamada COM ou uma escrita.
//!
//! - **Comecar** ([`decide_start`]): o nome que o WebView2 propos passa por
//!   `file_risk`. Um programa, um script, um atalho, uma imagem de disco ou
//!   uma base do Access e recusado (`SetCancel`) -- ou, com «Permitir baixar
//!   programas» ligado, fica preso no deferral a espera de uma confirmacao.
//!   Um disfarce (`fatura.pdf.exe`) ou um nome estragado e recusado sempre.
//! - **Acabar** ([`decide_finalize`], [`finalize_download`]): o ficheiro
//!   acabado e lido (os primeiros 4 KiB, `sniff_download`); um executavel,
//!   um atalho ou um gabinete que ninguem confirmou e apagado, o resto fica
//!   com a marca da Web (MOTW, `Zone.Identifier` com `ZoneId=3`, sem
//!   `HostUrl`) -- escrita so quando o ficheiro ainda nao a tem. Um `.zip`
//!   tem as entradas listadas pelo diretorio central (`file_risk::inspect_zip`,
//!   downloads-zip-inspect, sem ler os dados de nenhuma): com um programa,
//!   um script, um atalho, uma imagem de disco, uma base do Access ou outro
//!   arquivo compactado dentro e apagado, a menos que «Permitir baixar
//!   programas» esteja ligada; com um disfarce ou um nome inseguro dentro,
//!   sempre. Um que nao se deixa listar fica, como um 7z ou um RAR,
//!   [`Inspection::NotInspected`] -- nunca «seguro». O `neural-app` corre
//!   este fim fora da interface ([`finalize_download_with_commit`], que le
//!   «Permitir baixar programas» no commit); ate ao veredito o download fica
//!   [`DownloadState::Finalizing`], que nao se cancela.
//! - **Guardar** ([`DownloadLog`], `downloads.json`, no maximo
//!   [`MAX_LOG_ENTRIES`]): so os downloads acabados que nao vieram de uma
//!   pagina privada (o Split privado, um servico InPrivate) nem comecaram ou
//!   acabaram no Modo privado ([`DownloadManager::set_private_mode`]): um
//!   destes nunca entra no registo em memoria, e por isso nao chega ao
//!   ficheiro quando o modo volta ao normal. A loja e `StoreKind::Automatic`:
//!   no Modo privado tambem nao se escreve. Um fim a espera do veredito fica
//!   no registo como [`RecordOutcome::Pending`], com o caminho, e volta a
//!   correr no arranque seguinte ([`DownloadManager::resume_pending`]).
//! - **Apagar** ([`DownloadEvent::ClearLog`], Ctrl+Shift+Delete): o registo
//!   esvazia e o efeito [`DownloadEffect::EraseLog`] manda tirar o ficheiro
//!   do disco, em qualquer modo.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::file_risk::{
    self, BlockReason, RiskClass, SNIFF_HEAD_BYTES, SniffRisk, ZipEntryRisk, ZipVerdict,
    classify_download_name, inspect_zip_file, is_zip_name,
};

/// O intervalo minimo entre dois avisos de progresso de um download: o
/// `BytesReceivedChanged` chega dezenas de vezes por segundo.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// O `downloads.json` guarda no maximo isto, os mais novos primeiro.
pub const MAX_LOG_ENTRIES: usize = 200;
/// A versao do `downloads.json` que este codigo escreve.
pub const LOG_VERSION: u32 = 1;
/// O tecto do `downloads.json`: 200 registos com nomes e caminhos no maximo
/// (`MAX_RECORD_*`), com folga.
pub const LOG_MAX_BYTES: u64 = 1024 * 1024;
/// A versao do `downloads-settings.json`.
pub const SETTINGS_VERSION: u32 = 1;
/// O tecto do `downloads-settings.json`.
pub const SETTINGS_MAX_BYTES: u64 = 16 * 1024;

/// Um nome guardado tem no maximo isto (o limite do NTFS para um nome).
pub const MAX_RECORD_NAME_CHARS: usize = 255;
/// Um caminho maior do que isto nao se guarda (fica so o nome).
pub const MAX_RECORD_PATH_BYTES: usize = 1024;
/// O maior nome de anfitriao que o DNS admite.
pub const MAX_RECORD_HOST_BYTES: usize = 253;

/// O `Zone.Identifier` que o NeuralIA escreve: a zona Internet (3), sem
/// `HostUrl` nem `ReferrerUrl` (o endereco nao sai do download).
pub const MOTW_INTERNET: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";
/// O nome do fluxo alternativo (ADS) da marca da Web.
pub const MOTW_STREAM: &str = "Zone.Identifier";
/// O maior `Zone.Identifier` que [`read_motw`] le.
#[cfg(windows)]
const MOTW_MAX_BYTES: u64 = 64 * 1024;

/// O numero de um download nesta sessao (dado pelo lado do WebView2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DownloadId(pub u64);

/// A WebView de onde o download veio (um numero por WebView registada).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WebViewKey(pub u64);

// ===================== definicoes (downloads-settings.json) =====================

/// As escolhas do utilizador para os downloads: `StoreKind::Setting`, so
/// mudam numa definicao (a seccao Downloads do downloads-ui).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    /// A pasta onde os downloads caem (`None`: a pasta Transferencias do
    /// utilizador, que o `neural-app` repoe no perfil de cada WebView).
    pub folder: Option<PathBuf>,
    /// «Permitir baixar programas»: com ela, um programa pede confirmacao
    /// em vez de ser recusado. Um disfarce continua recusado.
    pub allow_programs: bool,
}

impl DownloadSettings {
    /// A pasta escolhida, so se for um caminho absoluto de uma pasta que
    /// existe (`is_dir` e a pergunta ao disco). Outra coisa volta a pasta do
    /// WebView2.
    pub fn usable_folder(&self, is_dir: impl Fn(&Path) -> bool) -> Option<&Path> {
        self.folder
            .as_deref()
            .filter(|folder| folder.is_absolute() && is_dir(folder))
    }
}

// ===================== o nome =====================

/// O risco de um nome de download, com a razao do bloqueio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRisk {
    Safe,
    /// Um documento com macros: baixa, com a marca da Web (o Office abre-o
    /// no Modo de Exibicao Protegido).
    Macro,
    Blocked(BlockReason),
}

/// O risco de `name` pelas regras de `file_risk`. Se a razao e a classe
/// alguma vez divergissem, um `Block` sem razao fica um nome estragado:
/// recusado sem exceção.
pub fn name_risk(name: &str) -> NameRisk {
    if let Some(reason) = file_risk::block_reason(name) {
        return NameRisk::Blocked(reason);
    }
    match classify_download_name(name) {
        RiskClass::Safe => NameRisk::Safe,
        RiskClass::Warn => NameRisk::Macro,
        RiskClass::Block => NameRisk::Blocked(BlockReason::BadName),
    }
}

/// O nome do ficheiro de um caminho, como texto; `""` sem nome ou sem UTF-8
/// (um nome que o `name_risk` recusa).
pub fn file_name_of(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
}

// ===================== a tabela do comeco =====================

/// O que o `DownloadStarting` faz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartDecision {
    /// Segue (o deferral completa-se com o caminho).
    Allow,
    /// Fica no deferral a espera do utilizador («Baixar programa?»).
    Ask(BlockReason),
    /// `SetCancel(true)`: o ficheiro nunca chega a existir.
    Refuse(BlockReason),
}

/// A tabela do comeco: o risco do nome e a definicao «Permitir baixar
/// programas».
pub fn decide_start(risk: NameRisk, allow_programs: bool) -> StartDecision {
    match risk {
        NameRisk::Safe | NameRisk::Macro => StartDecision::Allow,
        NameRisk::Blocked(reason) if allow_programs && reason.allows_confirmation() => {
            StartDecision::Ask(reason)
        }
        NameRisk::Blocked(reason) => StartDecision::Refuse(reason),
    }
}

/// Onde o download cai: o caminho que o WebView2 propos, ou, com uma pasta
/// escolhida onde ele nao esta (a definicao mudou depois de a WebView
/// nascer), o mesmo nome nessa pasta.
pub fn target_path(proposed: &Path, folder: Option<&Path>) -> PathBuf {
    match (folder, proposed.file_name()) {
        (Some(folder), Some(name)) if proposed.parent() != Some(folder) => folder.join(name),
        _ => proposed.to_path_buf(),
    }
}

/// `dir/name`, ou `dir/nome (N).ext` com o primeiro N que `exists` diz
/// livre, como o Chromium numera. Para ao fim de 999 tentativas com o
/// ultimo candidato.
pub fn unique_path(dir: &Path, name: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !exists(&first) {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    let mut candidate = first;
    for n in 1..=999 {
        candidate = dir.join(match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        });
        if !exists(&candidate) {
            break;
        }
    }
    candidate
}

// ===================== a tabela do fim =====================

/// Porque um download acabado foi apagado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeleteReason {
    /// O conteudo e um executavel (MZ/PE), um atalho (LNK) ou um gabinete
    /// (CAB) que ninguem pediu, com o nome que tiver.
    DangerousContent,
    /// O nome final bloqueia e nao foi confirmado (ou e um disfarce).
    BlockedName(BlockReason),
    /// O inicio do ficheiro nao se leu: sem sniff, nao fica.
    Unreadable,
    /// Um `.zip` com esta entrada dentro (downloads-zip-inspect): um
    /// programa, um script... ou outro arquivo compactado sem «Permitir
    /// baixar programas»; um disfarce ou um nome inseguro sempre.
    ArchiveEntry(ZipEntryRisk),
}

/// O que o fim diz de um ficheiro que ficou no disco (downloads-zip-inspect).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Inspection {
    /// Lido: o sniff e, num `.zip`, o diretorio central inteiro, sem nada
    /// que o apague.
    #[default]
    Checked,
    /// Um arquivo compactado cujo conteudo nao se viu: um 7z ou um RAR, um
    /// `.zip` que nao se deixou listar, ou um que leva outro arquivo
    /// compactado (com «Permitir baixar programas»). A lista mostra-o como
    /// tal, nunca como «seguro».
    NotInspected,
    /// Um `.zip` com programas ou scripts dentro, que ficou porque
    /// «Permitir baixar programas» esta ligada.
    HoldsPrograms,
}

impl Inspection {
    /// O valor por omissao, que o `downloads.json` nao escreve.
    pub fn is_checked(&self) -> bool {
        *self == Self::Checked
    }
}

/// O que se faz a um ficheiro acabado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeAction {
    /// Fica, com a marca da Web, e com o que a inspecao disse dele.
    Keep(Inspection),
    Delete(DeleteReason),
}

/// A tabela do fim: o nome final, o sniff dos primeiros 4 KiB (`None`: nao
/// se leu), a inspecao do `.zip` (`None`: o nome nao e um `.zip`), se o
/// utilizador confirmou este programa e «Permitir baixar programas».
pub fn decide_finalize(
    name: NameRisk,
    sniff: Option<SniffRisk>,
    archive: Option<&ZipVerdict>,
    confirmed_program: bool,
    allow_programs: bool,
) -> FinalizeAction {
    let Some(sniff) = sniff else {
        return FinalizeAction::Delete(DeleteReason::Unreadable);
    };
    if let NameRisk::Blocked(reason) = name
        && !(confirmed_program && reason.allows_confirmation())
    {
        return FinalizeAction::Delete(DeleteReason::BlockedName(reason));
    }
    if sniff == SniffRisk::Dangerous && !confirmed_program {
        return FinalizeAction::Delete(DeleteReason::DangerousContent);
    }
    match archive {
        // O que se permite so com a definicao (como o «Baixar programa?» do
        // comeco, que um `.zip` nunca pergunta); o disfarce e o nome
        // inseguro nunca.
        Some(ZipVerdict::Holds { risk, .. }) if !(allow_programs && risk.allows_confirmation()) => {
            FinalizeAction::Delete(DeleteReason::ArchiveEntry(*risk))
        }
        Some(ZipVerdict::Holds {
            risk: ZipEntryRisk::NestedArchive,
            ..
        })
        | Some(ZipVerdict::NotInspected) => FinalizeAction::Keep(Inspection::NotInspected),
        Some(ZipVerdict::Holds { .. }) => FinalizeAction::Keep(Inspection::HoldsPrograms),
        Some(ZipVerdict::Clean { .. }) | None if sniff == SniffRisk::NotInspected => {
            FinalizeAction::Keep(Inspection::NotInspected)
        }
        Some(ZipVerdict::Clean { .. }) | None => FinalizeAction::Keep(Inspection::Checked),
    }
}

/// O que [`write_motw_if_absent`] encontrou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotwOutcome {
    Written,
    /// O ficheiro ja tinha um `Zone.Identifier` (o do WebView2, por
    /// exemplo): ficou como estava.
    AlreadyPresent,
    /// Fora do Windows nao ha fluxos alternativos.
    Unsupported,
}

/// O que aconteceu a um ficheiro acabado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeOutcome {
    Kept(MotwOutcome, Inspection),
    /// Ficou, mas a marca nao se escreveu (um disco sem fluxos alternativos,
    /// FAT32 ou exFAT).
    KeptWithoutMotw(io::ErrorKind, Inspection),
    Deleted(DeleteReason),
    /// Devia ter sido apagado e o `remove_file` falhou: continua no disco.
    DeleteFailed(DeleteReason, io::ErrorKind),
}

/// Os primeiros [`SNIFF_HEAD_BYTES`] de um ficheiro regular.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut head = Vec::with_capacity(SNIFF_HEAD_BYTES as usize);
    file.take(SNIFF_HEAD_BYTES).read_to_end(&mut head).ok()?;
    Some(head)
}

/// Acaba um download: le o inicio do ficheiro (e, num `.zip`, o diretorio
/// central, nunca os dados das entradas), decide pela tabela e escreve a
/// marca da Web ou apaga-o. Um ficheiro que ja nao existe conta como
/// apagado.
pub fn finalize_download(
    path: &Path,
    confirmed_program: bool,
    allow_programs: bool,
) -> FinalizeOutcome {
    finalize_download_with_commit(
        path,
        confirmed_program,
        || allow_programs,
        |action| apply_finalize_download(path, action),
    )
}

/// O fim em dois tempos, como a thread `neural-download-finalize` o corre:
/// primeiro le o ficheiro (o sniff e, num `.zip`, o diretorio central); so
/// depois pergunta «Permitir baixar programas» (`allow_programs`, lida no
/// commit e nao quando o download acabou: desligada durante a verificacao,
/// um ZIP com um programa dentro e apagado) e entrega a acao a `commit`, que
/// a aplica ao disco ([`apply_finalize_download`]). Nao ha commit
/// condicional: uma verificacao nunca se cancela, e o veredito chega sempre.
pub fn finalize_download_with_commit(
    path: &Path,
    confirmed_program: bool,
    allow_programs: impl FnOnce() -> bool,
    commit: impl FnOnce(FinalizeAction) -> FinalizeOutcome,
) -> FinalizeOutcome {
    let name = file_name_of(path);
    let risk = name_risk(name);
    let sniff = read_head(path).map(|head| file_risk::sniff_download(&head));
    let archive = (sniff.is_some() && is_zip_name(name)).then(|| inspect_zip_file(path));
    commit(decide_finalize(
        risk,
        sniff,
        archive.as_ref(),
        confirmed_program,
        allow_programs(),
    ))
}

/// Aplica o veredito ja calculado: a marca da Web num que fica, apagar um
/// que nao fica.
pub fn apply_finalize_download(path: &Path, action: FinalizeAction) -> FinalizeOutcome {
    match action {
        FinalizeAction::Keep(inspection) => match write_motw_if_absent(path) {
            Ok(motw) => FinalizeOutcome::Kept(motw, inspection),
            Err(error) => FinalizeOutcome::KeptWithoutMotw(error.kind(), inspection),
        },
        FinalizeAction::Delete(reason) => match fs::remove_file(path) {
            Ok(()) => FinalizeOutcome::Deleted(reason),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                FinalizeOutcome::Deleted(reason)
            }
            Err(error) => FinalizeOutcome::DeleteFailed(reason, error.kind()),
        },
    }
}

// ===================== a marca da Web (MOTW) =====================

/// `<ficheiro>:Zone.Identifier`.
pub fn motw_stream_path(path: &Path) -> PathBuf {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":");
    stream.push(MOTW_STREAM);
    PathBuf::from(stream)
}

/// Escreve [`MOTW_INTERNET`] no fluxo `Zone.Identifier` de um ficheiro que
/// ainda nao o tem. `create_new` torna o "so quando falta" atomico: um
/// fluxo que ja existe da `AlreadyPresent` e nunca e reescrito.
#[cfg(windows)]
pub fn write_motw_if_absent(path: &Path) -> io::Result<MotwOutcome> {
    use std::io::Write;
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a marca da Web so vai para ficheiros",
        ));
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(motw_stream_path(path))
    {
        Ok(mut stream) => {
            stream.write_all(MOTW_INTERNET.as_bytes())?;
            stream.sync_all()?;
            Ok(MotwOutcome::Written)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Ok(MotwOutcome::AlreadyPresent)
        }
        Err(error) => Err(error),
    }
}

/// Fora do Windows nao ha `Zone.Identifier`.
#[cfg(not(windows))]
pub fn write_motw_if_absent(path: &Path) -> io::Result<MotwOutcome> {
    fs::metadata(path)?;
    Ok(MotwOutcome::Unsupported)
}

/// O `Zone.Identifier` de um ficheiro (ate 64 KiB), `None` se nao o tem.
#[cfg(windows)]
pub fn read_motw(path: &Path) -> io::Result<Option<String>> {
    match File::open(motw_stream_path(path)) {
        Ok(stream) => {
            let mut bytes = Vec::new();
            stream.take(MOTW_MAX_BYTES).read_to_end(&mut bytes)?;
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Fora do Windows nao ha `Zone.Identifier`.
#[cfg(not(windows))]
pub fn read_motw(path: &Path) -> io::Result<Option<String>> {
    fs::metadata(path)?;
    Ok(None)
}

/// O `ZoneId` da seccao `[ZoneTransfer]` de um `Zone.Identifier`.
pub fn motw_zone_id(text: &str) -> Option<u32> {
    let mut in_section = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case("[ZoneTransfer]");
            continue;
        }
        if in_section
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("ZoneId")
        {
            return value.trim().parse().ok();
        }
    }
    None
}

// ===================== o progresso =====================

/// O filtro do `BytesReceivedChanged` de UM download: deixa passar o
/// primeiro aviso e depois um a cada [`PROGRESS_INTERVAL`].
#[derive(Debug, Clone, Default)]
pub struct ProgressThrottle {
    last: Option<Instant>,
}

impl ProgressThrottle {
    pub fn admit(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < PROGRESS_INTERVAL => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

// ===================== o registo (downloads.json) =====================

/// Como um download acabou, no registo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum RecordOutcome {
    /// No disco, com a marca da Web. `warn`: um documento com macros ou um
    /// ficheiro sem extensao que comeca como um script. `inspection`: o que
    /// o fim disse dele (um arquivo compactado nao inspecionado, um `.zip`
    /// com programas que ficou pela definicao); omitido quando nada ha a
    /// dizer, e assim lido de um registo de antes da 2.4.
    Completed {
        warn: bool,
        #[serde(default, skip_serializing_if = "Inspection::is_checked")]
        inspection: Inspection,
    },
    /// Recusado antes de comecar.
    Blocked {
        reason: BlockReason,
    },
    /// Apagado depois de acabar.
    Deleted {
        reason: DeleteReason,
    },
    /// Devia ter sido apagado e nao foi: continua no disco.
    NotDeleted {
        reason: DeleteReason,
    },
    Cancelled,
    Interrupted,
    /// Acabou e ainda nao tem veredito: gravado quando o download acaba (com
    /// o caminho), para que um fim abrupto a meio da verificacao deixe rasto,
    /// e trocado pelo desfecho quando o veredito chega. No arranque seguinte
    /// o fim corre de novo ([`DownloadManager::resume_pending`]); `resumed`
    /// diz que ja voltou uma vez -- se volta outra, fica «não verificado»
    /// sem terceira tentativa: uma verificacao que derruba a NeuralIA nao a
    /// derruba em cada arranque. `confirmed_program`: o «Baixar programa?»
    /// deste download teve sim (o fim de novo precisa de o saber).
    Pending {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        confirmed_program: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        resumed: bool,
    },
}

/// Uma linha do `downloads.json`. Sem o endereco completo: so o anfitriao.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    pub outcome: RecordOutcome,
    /// Quando comecou, em segundos Unix.
    pub at: u64,
}

impl DownloadRecord {
    /// O registo dentro dos tectos: o nome cortado, um caminho ou um
    /// anfitriao grandes demais largados.
    fn bounded(mut self) -> Self {
        if self.name.chars().count() > MAX_RECORD_NAME_CHARS {
            self.name = self.name.chars().take(MAX_RECORD_NAME_CHARS).collect();
        }
        if self
            .path
            .as_ref()
            .is_some_and(|path| path.as_os_str().len() > MAX_RECORD_PATH_BYTES)
        {
            self.path = None;
        }
        if self
            .host
            .as_ref()
            .is_some_and(|host| host.len() > MAX_RECORD_HOST_BYTES)
        {
            self.host = None;
        }
        self
    }
}

/// O `downloads.json`: os mais novos primeiro, no maximo
/// [`MAX_LOG_ENTRIES`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadLog {
    pub entries: Vec<DownloadRecord>,
}

// ===================== o gestor =====================

/// O que o `DownloadStarting` sabe de um download novo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadStart {
    pub id: DownloadId,
    pub webview: WebViewKey,
    /// Veio do Split privado ou de um servico InPrivate: nunca vai para o
    /// `downloads.json`.
    pub private: bool,
    /// O `ResultFilePath` que o WebView2 propos: o nome e o dele.
    pub proposed: PathBuf,
    /// O anfitriao do endereco do download.
    pub host: Option<String>,
    pub total: Option<u64>,
    /// Segundos Unix.
    pub at: u64,
}

/// Como o WebView2 disse que um download acabou (`StateChanged`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEnd {
    /// No disco, neste caminho (`ResultFilePath` da operacao).
    Completed {
        path: PathBuf,
    },
    Cancelled,
    Interrupted,
}

/// Tudo o que chega ao gestor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    Starting(DownloadStart),
    /// Ja filtrado a [`PROGRESS_INTERVAL`] pelo lado do WebView2.
    Progress {
        id: DownloadId,
        received: u64,
        total: Option<u64>,
    },
    Ended {
        id: DownloadId,
        end: DownloadEnd,
    },
    /// O que `finalize_download` fez ao ficheiro.
    Finalized {
        id: DownloadId,
        outcome: FinalizeOutcome,
    },
    /// O fim deste download perdeu-se sem veredito (a thread que o corria
    /// rebentou a meio): o gestor pede-o outra vez (`Finalize`), e quem o
    /// corre, sem a thread, corre-o ja. Uma linha nunca fica presa em
    /// «Verificando o arquivo…».
    FinalizeLost {
        id: DownloadId,
    },
    /// A resposta ao «Baixar programa?».
    Answered {
        id: DownloadId,
        allow: bool,
    },
    /// O utilizador cancelou.
    CancelRequested {
        id: DownloadId,
    },
    /// A WebView de onde estes downloads vieram foi destruida.
    WebViewGone {
        webview: WebViewKey,
    },
    SettingsChanged(DownloadSettings),
    /// Ctrl+Shift+Delete: o registo e os downloads acabados da lista. Da
    /// [`DownloadEffect::EraseLog`], nunca um `Persist`.
    ClearLog,
}

/// Um aviso para quem usa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadNotice {
    Blocked {
        id: DownloadId,
        name: String,
        reason: BlockReason,
    },
    Deleted {
        id: DownloadId,
        name: String,
        reason: DeleteReason,
    },
    NotDeleted {
        id: DownloadId,
        name: String,
        reason: DeleteReason,
    },
}

/// O que o lado do WebView2 (e o `App`) faz com uma decisao.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEffect {
    /// Recusado no `DownloadStarting`: `SetCancel(true)`, o deferral
    /// completo e a operacao largada.
    Refuse(DownloadId),
    /// Segue: o deferral completo com este `ResultFilePath`.
    Proceed {
        id: DownloadId,
        path: PathBuf,
    },
    /// O deferral fica preso: perguntar ao utilizador.
    Ask {
        id: DownloadId,
        reason: BlockReason,
    },
    /// `Cancel()` da operacao de um download a correr.
    CancelRunning(DownloadId),
    /// Largar a operacao do WebView2 (acabou, ou a WebView dela foi-se).
    ForgetOp(DownloadId),
    /// Correr [`finalize_download`] sobre o ficheiro acabado.
    Finalize {
        id: DownloadId,
        path: PathBuf,
        confirmed_program: bool,
        /// «Permitir baixar programas» agora: decide um `.zip` com programas
        /// dentro quando o fim corre ja (sincrono). A thread do fim nao usa
        /// este valor: relê a definicao no commit
        /// ([`finalize_download_with_commit`]).
        allow_programs: bool,
    },
    /// O registo mudou: gravar [`DownloadManager::log`].
    Persist,
    /// O registo foi esvaziado (Ctrl+Shift+Delete): tirar o `downloads.json`
    /// e as copias dele do disco, em qualquer modo e mesmo com a loja so de
    /// leitura -- uma gravacao nao serve (no Modo privado ou com um ficheiro
    /// de uma versao futura nao escreve nada).
    EraseLog,
    /// A linha deste download mudou.
    Changed(DownloadId),
    Notice(DownloadNotice),
}

/// Onde um download esta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    /// Preso no deferral, a espera do «Baixar programa?».
    Asking(BlockReason),
    Running,
    /// Acabou; `finalize_download` a correr («Verificando o arquivo…»). Nao
    /// se cancela: o veredito chega sempre, e e o que a linha mostra.
    Finalizing,
    Done(RecordOutcome),
}

/// Um download desta sessao (privados incluidos: a lista mostra-os, o
/// registo nunca).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadEntry {
    pub id: DownloadId,
    pub webview: WebViewKey,
    pub private: bool,
    pub name: String,
    pub path: PathBuf,
    pub host: Option<String>,
    pub received: u64,
    pub total: Option<u64>,
    pub state: DownloadState,
    pub confirmed_program: bool,
    /// Um documento com macros.
    pub warn: bool,
    pub at: u64,
}

impl DownloadEntry {
    /// A correr ou a espera: o que o downloads-ui conta ao sair.
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            DownloadState::Asking(_) | DownloadState::Running | DownloadState::Finalizing
        )
    }
}

/// O gestor: a lista da sessao, o registo e as definicoes. Cada evento
/// devolve os efeitos, pela ordem em que se aplicam.
#[derive(Debug, Clone, Default)]
pub struct DownloadManager {
    settings: DownloadSettings,
    entries: BTreeMap<DownloadId, DownloadEntry>,
    log: VecDeque<DownloadRecord>,
    /// O Modo privado do registo das lojas, posto por quem conduz o gestor
    /// antes de cada evento.
    private_mode: bool,
}

impl DownloadManager {
    /// Com as definicoes e o registo lidos do disco (cortado a
    /// [`MAX_LOG_ENTRIES`] e aos tectos de cada campo).
    pub fn new(settings: DownloadSettings, log: DownloadLog) -> Self {
        Self {
            settings,
            entries: BTreeMap::new(),
            log: log
                .entries
                .into_iter()
                .take(MAX_LOG_ENTRIES)
                .map(DownloadRecord::bounded)
                .collect(),
            private_mode: false,
        }
    }

    /// O Modo privado (o do registo das lojas, `StoreMode::Private`). Com
    /// ele ligado, nada entra no registo: um download que comeca fica
    /// privado ate ao fim (como um do Split privado), e um que acaba nao e
    /// registado. O registo em memoria e o que a proxima gravacao escreve
    /// quando o modo volta ao normal: nao escrever no Modo privado nao
    /// chegava.
    pub fn set_private_mode(&mut self, private: bool) {
        self.private_mode = private;
    }

    pub fn private_mode(&self) -> bool {
        self.private_mode
    }

    pub fn settings(&self) -> &DownloadSettings {
        &self.settings
    }

    pub fn entry(&self, id: DownloadId) -> Option<&DownloadEntry> {
        self.entries.get(&id)
    }

    pub fn entries(&self) -> impl Iterator<Item = &DownloadEntry> {
        self.entries.values()
    }

    /// Os downloads a correr ou a espera.
    pub fn active(&self) -> usize {
        self.entries.values().filter(|e| e.is_active()).count()
    }

    /// O que vai para o `downloads.json`.
    pub fn log(&self) -> DownloadLog {
        DownloadLog {
            entries: self.log.iter().cloned().collect(),
        }
    }

    pub fn on_event(&mut self, event: DownloadEvent) -> Vec<DownloadEffect> {
        match event {
            DownloadEvent::Starting(start) => self.starting(start),
            DownloadEvent::Progress {
                id,
                received,
                total,
            } => match self.entries.get_mut(&id) {
                Some(entry) if entry.state == DownloadState::Running => {
                    entry.received = received;
                    if total.is_some() {
                        entry.total = total;
                    }
                    vec![DownloadEffect::Changed(id)]
                }
                _ => Vec::new(),
            },
            DownloadEvent::Ended { id, end } => self.ended(id, end),
            DownloadEvent::Finalized { id, outcome } => self.finalized(id, outcome),
            DownloadEvent::FinalizeLost { id } => match self.entries.get(&id) {
                Some(entry) if entry.state == DownloadState::Finalizing => {
                    vec![DownloadEffect::Finalize {
                        id,
                        path: entry.path.clone(),
                        confirmed_program: entry.confirmed_program,
                        allow_programs: self.settings.allow_programs,
                    }]
                }
                _ => Vec::new(),
            },
            DownloadEvent::Answered { id, allow } => self.answered(id, allow),
            // A verificacao do fim (`Finalizing`) nao se cancela: o ficheiro
            // ja esta no disco, e so o veredito o apaga ou lhe poe a marca da
            // Web. Um Cancel que la chega (a linha, a saida, um clique velho
            // de quando ainda corria) nao faz nada; o veredito chega depois.
            DownloadEvent::CancelRequested { id } => match self.entries.get(&id).map(|e| e.state) {
                Some(DownloadState::Asking(_)) => self.answered(id, false),
                Some(DownloadState::Running) => vec![DownloadEffect::CancelRunning(id)],
                _ => Vec::new(),
            },
            DownloadEvent::WebViewGone { webview } => self.webview_gone(webview),
            DownloadEvent::SettingsChanged(settings) => {
                self.settings = settings;
                Vec::new()
            }
            DownloadEvent::ClearLog => {
                self.log.clear();
                self.entries.retain(|_, entry| entry.is_active());
                vec![DownloadEffect::EraseLog]
            }
        }
    }

    fn starting(&mut self, start: DownloadStart) -> Vec<DownloadEffect> {
        let id = start.id;
        // Um numero repetido e um erro do lado do WebView2: o segundo
        // nunca comeca.
        if self.entries.contains_key(&id) {
            return vec![DownloadEffect::Refuse(id)];
        }
        let name = file_name_of(&start.proposed).to_string();
        let risk = name_risk(&name);
        let decision = decide_start(risk, self.settings.allow_programs);
        // A pasta ja chega conferida no disco (quem le as definicoes larga
        // uma que nao existe); aqui so se exige que seja absoluta.
        let folder = self.settings.usable_folder(|_| true);
        let path = target_path(&start.proposed, folder);
        let mut entry = DownloadEntry {
            id,
            webview: start.webview,
            // Comecado no Modo privado: privado ate ao fim, mesmo que o modo
            // volte ao normal antes de ele acabar.
            private: start.private || self.private_mode,
            name: if name.is_empty() {
                start.proposed.to_string_lossy().into_owned()
            } else {
                name
            },
            path: path.clone(),
            host: start.host,
            received: 0,
            total: start.total,
            state: DownloadState::Running,
            confirmed_program: false,
            warn: risk == NameRisk::Macro,
            at: start.at,
        };
        let mut effects = Vec::new();
        match decision {
            StartDecision::Allow => effects.push(DownloadEffect::Proceed { id, path }),
            StartDecision::Ask(reason) => {
                entry.state = DownloadState::Asking(reason);
                effects.push(DownloadEffect::Ask { id, reason });
            }
            StartDecision::Refuse(reason) => {
                entry.state = DownloadState::Done(RecordOutcome::Blocked { reason });
                effects.push(DownloadEffect::Refuse(id));
                effects.push(DownloadEffect::Notice(DownloadNotice::Blocked {
                    id,
                    name: entry.name.clone(),
                    reason,
                }));
            }
        }
        effects.push(DownloadEffect::Changed(id));
        let done = matches!(entry.state, DownloadState::Done(_));
        self.entries.insert(id, entry);
        if done {
            effects.extend(self.record(id));
        }
        effects
    }

    fn answered(&mut self, id: DownloadId, allow: bool) -> Vec<DownloadEffect> {
        let Some(entry) = self.entries.get_mut(&id) else {
            return Vec::new();
        };
        let DownloadState::Asking(reason) = entry.state else {
            // Uma resposta atrasada (o download ja acabou ou foi recusado).
            return Vec::new();
        };
        if allow {
            entry.state = DownloadState::Running;
            entry.confirmed_program = true;
            return vec![
                DownloadEffect::Proceed {
                    id,
                    path: entry.path.clone(),
                },
                DownloadEffect::Changed(id),
            ];
        }
        entry.state = DownloadState::Done(RecordOutcome::Blocked { reason });
        let mut effects = vec![DownloadEffect::Refuse(id), DownloadEffect::Changed(id)];
        effects.extend(self.record(id));
        effects
    }

    fn ended(&mut self, id: DownloadId, end: DownloadEnd) -> Vec<DownloadEffect> {
        let allow_programs = self.settings.allow_programs;
        let Some(entry) = self.entries.get_mut(&id) else {
            return vec![DownloadEffect::ForgetOp(id)];
        };
        let mut effects = vec![DownloadEffect::ForgetOp(id)];
        match end {
            DownloadEnd::Completed { path } => {
                // Um ficheiro acabado passa SEMPRE pelo fim, mesmo depois de
                // a WebView dele ter ido (o `Cancel` pode ter chegado tarde):
                // so um ja acabado (repetido) nao volta a passar.
                if matches!(
                    entry.state,
                    DownloadState::Finalizing
                        | DownloadState::Done(
                            RecordOutcome::Completed { .. }
                                | RecordOutcome::Deleted { .. }
                                | RecordOutcome::NotDeleted { .. }
                        )
                ) {
                    return effects;
                }
                if let Some(total) = entry.total {
                    entry.received = total;
                }
                // Acabou no Modo privado: privado ate ao fim, mesmo que o
                // modo volte ao normal antes do veredito (quem vai para o
                // registo decide-se aqui, nao quando a verificacao acaba).
                // So neste passo: um `Completed` repetido nao o muda.
                entry.private |= self.private_mode;
                entry.path = path.clone();
                entry.state = DownloadState::Finalizing;
                effects.push(DownloadEffect::Finalize {
                    id,
                    path,
                    confirmed_program: entry.confirmed_program,
                    allow_programs,
                });
                effects.push(DownloadEffect::Changed(id));
                // O rasto de um fim sem veredito, antes de ele correr.
                effects.extend(self.record(id));
            }
            DownloadEnd::Cancelled | DownloadEnd::Interrupted => {
                if !matches!(
                    entry.state,
                    DownloadState::Running | DownloadState::Asking(_)
                ) {
                    return effects;
                }
                entry.state = DownloadState::Done(if end == DownloadEnd::Cancelled {
                    RecordOutcome::Cancelled
                } else {
                    RecordOutcome::Interrupted
                });
                effects.push(DownloadEffect::Changed(id));
                effects.extend(self.record(id));
            }
        }
        effects
    }

    fn finalized(&mut self, id: DownloadId, outcome: FinalizeOutcome) -> Vec<DownloadEffect> {
        let Some(entry) = self.entries.get_mut(&id) else {
            return Vec::new();
        };
        if entry.state != DownloadState::Finalizing {
            return Vec::new();
        }
        let mut effects = Vec::new();
        let recorded = match outcome {
            FinalizeOutcome::Kept(_, inspection)
            | FinalizeOutcome::KeptWithoutMotw(_, inspection) => RecordOutcome::Completed {
                warn: entry.warn,
                inspection,
            },
            FinalizeOutcome::Deleted(reason) => {
                effects.push(DownloadEffect::Notice(DownloadNotice::Deleted {
                    id,
                    name: entry.name.clone(),
                    reason,
                }));
                RecordOutcome::Deleted { reason }
            }
            FinalizeOutcome::DeleteFailed(reason, _) => {
                effects.push(DownloadEffect::Notice(DownloadNotice::NotDeleted {
                    id,
                    name: entry.name.clone(),
                    reason,
                }));
                RecordOutcome::NotDeleted { reason }
            }
        };
        entry.state = DownloadState::Done(recorded);
        effects.push(DownloadEffect::Changed(id));
        // O registo pendente (o do fim) da lugar ao desfecho. Quem vai para o
        // registo decidiu-se quando o download acabou (`ended`): o modo de
        // agora nao conta, e um que acabou no normal nao some do registo por
        // o veredito cair no Modo privado (a loja nao escreve nesse modo; a
        // gravacao seguinte no normal leva-o).
        // O pendente como foi gravado (dentro dos tectos: um nome cortado,
        // um caminho grande demais largado).
        let wanted = record_of(entry, RecordOutcome::Cancelled);
        let wanted_path = Some(entry.path.as_path())
            .filter(|path| path.as_os_str().len() <= MAX_RECORD_PATH_BYTES);
        let pending = self.log.iter().position(|record| {
            matches!(record.outcome, RecordOutcome::Pending { .. })
                && record.name == wanted.name
                && record.host == wanted.host
                && record.at == wanted.at
                && record.path.as_deref() == wanted_path
        });
        let removed = pending.and_then(|index| self.log.remove(index)).is_some();
        match self.push_record(id) {
            Some(persist) => effects.push(persist),
            None if removed => effects.push(DownloadEffect::Persist),
            None => {}
        }
        effects
    }

    /// Os downloads que ficaram por verificar no `downloads.json` (a
    /// NeuralIA fechou ou caiu com o veredito a meio): cada um volta como
    /// uma linha desta sessao em «Verificando o arquivo…», com o numero que
    /// `next_id` da, e o fim pedido de novo (`Finalize`, com «Permitir
    /// baixar programas» de agora). O registo fica pendente, marcado
    /// `resumed`, ate ao veredito; e o `Persist` vem PRIMEIRO, para essa
    /// marca estar no disco antes de o fim correr. Um que ja tinha voltado
    /// uma vez, ou cujo caminho nao ficou no registo, nao corre outra vez:
    /// fica «não verificado». Um ficheiro que ja nao existe conta como
    /// apagado (`Unreadable`), como no [`finalize_download`].
    pub fn resume_pending(
        &mut self,
        mut next_id: impl FnMut() -> DownloadId,
    ) -> Vec<DownloadEffect> {
        let allow_programs = self.settings.allow_programs;
        let mut marked = false;
        let mut effects = Vec::new();
        let mut resumed = Vec::new();
        for record in self.log.iter_mut() {
            let RecordOutcome::Pending {
                confirmed_program,
                resumed: false,
            } = record.outcome
            else {
                continue;
            };
            record.outcome = RecordOutcome::Pending {
                confirmed_program,
                resumed: true,
            };
            marked = true;
            if let Some(path) = record.path.clone() {
                resumed.push((record.clone(), path, confirmed_program));
            }
        }
        if marked {
            effects.push(DownloadEffect::Persist);
        }
        for (record, path, confirmed_program) in resumed {
            let id = next_id();
            if self.entries.contains_key(&id) {
                continue;
            }
            self.entries.insert(
                id,
                DownloadEntry {
                    id,
                    // Nenhuma WebView: o `WebViewGone` nunca o apanha.
                    webview: WebViewKey(0),
                    private: false,
                    warn: name_risk(&record.name) == NameRisk::Macro,
                    name: record.name,
                    path: path.clone(),
                    host: record.host,
                    received: record.bytes.unwrap_or(0),
                    total: record.bytes,
                    state: DownloadState::Finalizing,
                    confirmed_program,
                    at: record.at,
                },
            );
            effects.push(DownloadEffect::Finalize {
                id,
                path,
                confirmed_program,
                allow_programs,
            });
            effects.push(DownloadEffect::Changed(id));
        }
        effects
    }

    /// A WebView destes downloads foi destruida: o que esperava a resposta
    /// e recusado, o que corria e cancelado, e as operacoes largadas. Um
    /// ficheiro ja acabado continua o fim dele.
    fn webview_gone(&mut self, webview: WebViewKey) -> Vec<DownloadEffect> {
        let ids: Vec<DownloadId> = self
            .entries
            .values()
            .filter(|entry| entry.webview == webview)
            .map(|entry| entry.id)
            .collect();
        let mut effects = Vec::new();
        for id in ids {
            let Some(entry) = self.entries.get_mut(&id) else {
                continue;
            };
            match entry.state {
                DownloadState::Asking(_) => {
                    entry.state = DownloadState::Done(RecordOutcome::Cancelled);
                    effects.push(DownloadEffect::Refuse(id));
                }
                DownloadState::Running => {
                    entry.state = DownloadState::Done(RecordOutcome::Interrupted);
                    effects.push(DownloadEffect::CancelRunning(id));
                    effects.push(DownloadEffect::ForgetOp(id));
                }
                DownloadState::Finalizing | DownloadState::Done(_) => continue,
            }
            effects.push(DownloadEffect::Changed(id));
            effects.extend(self.record(id));
        }
        effects
    }

    /// Um download acabado entra no registo -- so se nao veio de uma pagina
    /// privada nem comecou ou acabou no Modo privado.
    fn record(&mut self, id: DownloadId) -> Option<DownloadEffect> {
        if self.private_mode {
            return None;
        }
        self.push_record(id)
    }

    /// A unica porta do registo: um download que nao e privado (`private`
    /// junta a pagina privada e o Modo privado do comeco e do fim), com o
    /// desfecho, ou `Pending` com o caminho enquanto a verificacao corre.
    fn push_record(&mut self, id: DownloadId) -> Option<DownloadEffect> {
        let entry = self.entries.get(&id)?;
        if entry.private {
            return None;
        }
        let outcome = match entry.state {
            DownloadState::Done(outcome) => outcome,
            DownloadState::Finalizing => RecordOutcome::Pending {
                confirmed_program: entry.confirmed_program,
                resumed: false,
            },
            DownloadState::Asking(_) | DownloadState::Running => return None,
        };
        let record = record_of(entry, outcome);
        self.log.push_front(record);
        self.log.truncate(MAX_LOG_ENTRIES);
        Some(DownloadEffect::Persist)
    }
}

/// A linha do registo de `entry` com `outcome`, dentro dos tectos: o
/// caminho e o tamanho so de um que ficou no disco (ou que ainda o vai
/// verificar).
fn record_of(entry: &DownloadEntry, outcome: RecordOutcome) -> DownloadRecord {
    let kept = matches!(
        outcome,
        RecordOutcome::Completed { .. }
            | RecordOutcome::NotDeleted { .. }
            | RecordOutcome::Pending { .. }
    );
    DownloadRecord {
        name: entry.name.clone(),
        path: kept.then(|| entry.path.clone()),
        host: entry.host.clone(),
        bytes: kept.then_some(entry.received),
        outcome,
        at: entry.at,
    }
    .bounded()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(1);

    /// Uma pasta temporaria com ficheiros reais (o fim le o disco).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neuralia-downloads-{name}-{}-{}",
                std::process::id(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("pasta temporaria");
            Self(path)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).expect("ficheiro");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pe_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x100];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes
    }

    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";

    fn start(id: u64, name: &str, private: bool) -> DownloadEvent {
        DownloadEvent::Starting(DownloadStart {
            id: DownloadId(id),
            webview: WebViewKey(7),
            private,
            // Sem nome, o caminho vazio (um `ResultFilePath` sem ficheiro).
            proposed: if name.is_empty() {
                PathBuf::new()
            } else {
                PathBuf::from(r"C:\Users\x\Downloads").join(name)
            },
            host: Some("example.com".to_string()),
            total: Some(1000),
            at: 1_790_000_000,
        })
    }

    fn manager(allow_programs: bool) -> DownloadManager {
        DownloadManager::new(
            DownloadSettings {
                folder: None,
                allow_programs,
            },
            DownloadLog::default(),
        )
    }

    /// Gate critico: a tabela do comeco, pelo gestor que embarca. Cada
    /// classe de nome, com e sem «Permitir baixar programas».
    #[test]
    fn the_start_decision_table() {
        use BlockReason::*;
        use StartDecision::*;
        let rows: &[(&str, StartDecision, StartDecision)] = &[
            // nome, sem a definicao, com a definicao
            ("relatorio.pdf", Allow, Allow),
            ("foto.JPG", Allow, Allow),
            ("LEIAME", Allow, Allow),
            ("dados.tar.gz", Allow, Allow),
            ("planilha.xlsm", Allow, Allow),
            ("setup.exe", Refuse(Program), Ask(Program)),
            ("SETUP.EXE.", Refuse(Program), Ask(Program)),
            ("pacote.msi", Refuse(Program), Ask(Program)),
            ("run.bat", Refuse(Script), Ask(Script)),
            ("deploy.ps1", Refuse(Script), Ask(Script)),
            ("atalho.lnk", Refuse(Shortcut), Ask(Shortcut)),
            ("ajuda.chm", Refuse(Shortcut), Ask(Shortcut)),
            ("disco.iso", Refuse(DiskImage), Ask(DiskImage)),
            ("base.mdb", Refuse(DatabaseApp), Ask(DatabaseApp)),
            ("fatura.pdf.exe", Refuse(Masquerade), Refuse(Masquerade)),
            ("foto.jpg   .scr", Refuse(Masquerade), Refuse(Masquerade)),
            ("fatura.pdf.docm", Refuse(Masquerade), Refuse(Masquerade)),
            ("safe\u{202e}exe.pdf", Refuse(BadName), Refuse(BadName)),
            ("nul.txt", Refuse(BadName), Refuse(BadName)),
            ("", Refuse(BadName), Refuse(BadName)),
        ];
        for &(name, without, with) in rows {
            for (allow_programs, expected) in [(false, without), (true, with)] {
                assert_eq!(
                    decide_start(name_risk(name), allow_programs),
                    expected,
                    "{name:?} allow_programs={allow_programs}"
                );
                // O mesmo pelo gestor: o efeito que o COM aplica.
                let mut manager = manager(allow_programs);
                let effects = manager.on_event(start(1, name, false));
                let id = DownloadId(1);
                let first = effects.first().cloned();
                match expected {
                    Allow => assert!(
                        matches!(first, Some(DownloadEffect::Proceed { id: got, .. }) if got == id),
                        "{name:?}: {effects:?}"
                    ),
                    Ask(reason) => {
                        assert_eq!(first, Some(DownloadEffect::Ask { id, reason }), "{name:?}");
                        assert_eq!(manager.active(), 1);
                    }
                    Refuse(reason) => {
                        assert_eq!(first, Some(DownloadEffect::Refuse(id)), "{name:?}");
                        assert!(effects.contains(&DownloadEffect::Notice(
                            DownloadNotice::Blocked {
                                id,
                                name: manager.entry(id).expect("entrada").name.clone(),
                                reason,
                            }
                        )));
                        assert_eq!(manager.active(), 0, "{name:?}");
                    }
                }
            }
        }
        // A resposta ao «Baixar programa?»: sim segue, nao recusa; uma
        // resposta atrasada nao faz nada.
        let mut m = manager(true);
        m.on_event(start(1, "setup.exe", false));
        let yes = m.on_event(DownloadEvent::Answered {
            id: DownloadId(1),
            allow: true,
        });
        assert!(matches!(yes[0], DownloadEffect::Proceed { .. }), "{yes:?}");
        assert!(m.entry(DownloadId(1)).expect("entrada").confirmed_program);
        assert!(
            m.on_event(DownloadEvent::Answered {
                id: DownloadId(1),
                allow: false,
            })
            .is_empty()
        );
        m.on_event(start(2, "setup.exe", false));
        let no = m.on_event(DownloadEvent::Answered {
            id: DownloadId(2),
            allow: false,
        });
        assert_eq!(no[0], DownloadEffect::Refuse(DownloadId(2)));
        // Um numero repetido nunca comeca.
        assert_eq!(
            m.on_event(start(2, "relatorio.pdf", false)),
            vec![DownloadEffect::Refuse(DownloadId(2))]
        );
    }

    /// Gate critico: a tabela do fim -- o nome, o sniff, a inspecao de um
    /// `.zip` (downloads-zip-inspect), a confirmacao e «Permitir baixar
    /// programas». Sabotado: um arquivo nao inspecionado dado como lido
    /// (`Keep(Checked)`) fica vermelho aqui e no gate do ZIP no disco.
    #[test]
    fn the_finalize_decision_table() {
        use BlockReason::*;
        use DeleteReason::*;
        use FinalizeAction::*;
        use Inspection::{Checked, HoldsPrograms, NotInspected};
        let safe = NameRisk::Safe;
        let macro_doc = NameRisk::Macro;
        let program = NameRisk::Blocked(Program);
        let masquerade = NameRisk::Blocked(Masquerade);
        let bad = NameRisk::Blocked(BadName);
        let holds = |risk| {
            Some(ZipVerdict::Holds {
                risk,
                entry: "x".to_string(),
            })
        };
        let clean = Some(ZipVerdict::Clean { entries: 2 });
        let unread = Some(ZipVerdict::NotInspected);
        let zip_program = holds(ZipEntryRisk::Blocked(Program));
        let zip_script = holds(ZipEntryRisk::Blocked(Script));
        let zip_masquerade = holds(ZipEntryRisk::Blocked(Masquerade));
        let zip_bad = holds(ZipEntryRisk::Blocked(BadName));
        let zip_nested = holds(ZipEntryRisk::NestedArchive);
        let safe_sniff = Some(SniffRisk::Safe);
        // nome, sniff, zip, confirmado, definicao, esperado
        type Row = (
            NameRisk,
            Option<SniffRisk>,
            Option<ZipVerdict>,
            bool,
            bool,
            FinalizeAction,
        );
        let rows: Vec<Row> = vec![
            (safe, safe_sniff, None, false, false, Keep(Checked)),
            (
                safe,
                Some(SniffRisk::NotInspected),
                None,
                false,
                false,
                Keep(NotInspected),
            ),
            (
                safe,
                Some(SniffRisk::Warn),
                None,
                false,
                false,
                Keep(Checked),
            ),
            (macro_doc, safe_sniff, None, false, false, Keep(Checked)),
            (
                safe,
                Some(SniffRisk::Dangerous),
                None,
                false,
                false,
                Delete(DangerousContent),
            ),
            (
                macro_doc,
                Some(SniffRisk::Dangerous),
                None,
                false,
                false,
                Delete(DangerousContent),
            ),
            (safe, None, None, false, false, Delete(Unreadable)),
            (safe, None, None, true, true, Delete(Unreadable)),
            (
                program,
                Some(SniffRisk::Dangerous),
                None,
                false,
                true,
                Delete(BlockedName(Program)),
            ),
            (
                program,
                safe_sniff,
                None,
                false,
                false,
                Delete(BlockedName(Program)),
            ),
            (
                program,
                Some(SniffRisk::Dangerous),
                None,
                true,
                true,
                Keep(Checked),
            ),
            (
                safe,
                Some(SniffRisk::Dangerous),
                None,
                true,
                false,
                Keep(Checked),
            ),
            (
                masquerade,
                Some(SniffRisk::Dangerous),
                None,
                true,
                true,
                Delete(BlockedName(Masquerade)),
            ),
            (
                masquerade,
                safe_sniff,
                None,
                false,
                false,
                Delete(BlockedName(Masquerade)),
            ),
            (
                bad,
                safe_sniff,
                None,
                true,
                true,
                Delete(BlockedName(BadName)),
            ),
            // Um `.zip` lido inteiro sem nada perigoso; um que nao se leu
            // fica, mas nunca como lido -- com ou sem a definicao.
            (safe, safe_sniff, clean.clone(), false, false, Keep(Checked)),
            (safe, safe_sniff, clean.clone(), false, true, Keep(Checked)),
            (
                safe,
                safe_sniff,
                unread.clone(),
                false,
                false,
                Keep(NotInspected),
            ),
            (
                safe,
                safe_sniff,
                unread.clone(),
                false,
                true,
                Keep(NotInspected),
            ),
            // Programas ou scripts dentro: apagado sem a definicao, fica
            // com ela (e a lista diz o que leva).
            (
                safe,
                safe_sniff,
                zip_program.clone(),
                false,
                false,
                Delete(ArchiveEntry(ZipEntryRisk::Blocked(Program))),
            ),
            (
                safe,
                safe_sniff,
                zip_program.clone(),
                false,
                true,
                Keep(HoldsPrograms),
            ),
            (
                safe,
                safe_sniff,
                zip_script.clone(),
                false,
                false,
                Delete(ArchiveEntry(ZipEntryRisk::Blocked(Script))),
            ),
            (
                safe,
                safe_sniff,
                zip_script.clone(),
                false,
                true,
                Keep(HoldsPrograms),
            ),
            // Outro arquivo compactado dentro: apagado sem a definicao; com
            // ela fica, e fica nao inspecionado.
            (
                safe,
                safe_sniff,
                zip_nested.clone(),
                false,
                false,
                Delete(ArchiveEntry(ZipEntryRisk::NestedArchive)),
            ),
            (
                safe,
                safe_sniff,
                zip_nested.clone(),
                false,
                true,
                Keep(NotInspected),
            ),
            // Um disfarce ou um nome inseguro dentro: nunca fica.
            (
                safe,
                safe_sniff,
                zip_masquerade.clone(),
                false,
                true,
                Delete(ArchiveEntry(ZipEntryRisk::Blocked(Masquerade))),
            ),
            (
                safe,
                safe_sniff,
                zip_bad.clone(),
                true,
                true,
                Delete(ArchiveEntry(ZipEntryRisk::Blocked(BadName))),
            ),
            // O sniff decide antes do ZIP: um executavel com nome de `.zip`
            // e apagado pelo conteudo.
            (
                safe,
                Some(SniffRisk::Dangerous),
                unread.clone(),
                false,
                true,
                Delete(DangerousContent),
            ),
        ];
        for (name, sniff, archive, confirmed, allow, expected) in &rows {
            assert_eq!(
                decide_finalize(*name, *sniff, archive.as_ref(), *confirmed, *allow),
                *expected,
                "{name:?} {sniff:?} {archive:?} confirmado={confirmed} definicao={allow}"
            );
        }

        // No disco: um PDF fica (com a marca, no Windows); um executavel com
        // nome de PDF e apagado; um programa confirmado fica; um que ja nao
        // existe conta como apagado.
        let dir = TempDir::new("finalize");
        let pdf = dir.file("relatorio.pdf", PDF);
        let kept = finalize_download(&pdf, false, false);
        assert!(
            matches!(kept, FinalizeOutcome::Kept(_, Inspection::Checked)),
            "{kept:?}"
        );
        assert!(pdf.exists());
        let disguised = dir.file("relatorio2.pdf", &pe_bytes());
        assert_eq!(
            finalize_download(&disguised, false, false),
            FinalizeOutcome::Deleted(DangerousContent)
        );
        assert!(!disguised.exists());
        let setup = dir.file("setup.exe", &pe_bytes());
        assert!(matches!(
            finalize_download(&setup, true, false),
            FinalizeOutcome::Kept(_, Inspection::Checked)
        ));
        assert!(setup.exists());
        let unconfirmed = dir.file("outro.exe", &pe_bytes());
        assert_eq!(
            finalize_download(&unconfirmed, false, false),
            FinalizeOutcome::Deleted(BlockedName(Program))
        );
        assert!(!unconfirmed.exists());
        assert_eq!(
            finalize_download(&dir.0.join("sumiu.pdf"), false, false),
            FinalizeOutcome::Deleted(Unreadable)
        );
    }

    /// Gate critico (downloads-zip-inspect), no disco, pelo
    /// `finalize_download` que o `neural-app` corre: o `pacote.zip` do E2E
    /// (setup.exe e run.bat) e apagado sem «Permitir baixar programas» e
    /// fica com ela; um `aux.exe`, um `setup.exe` no Unicode Path (0x7075)
    /// ou no cabecalho local apagam-no; um `a/../setup.exe` ou um segmento
    /// de 62 000 bytes apagam-no mesmo com ela; `setup.exe/.`,
    /// `setup.exe\.` e `setup.exe/ .` contam como o programa; um ZIP do
    /// bsdtar (`./LEIAME.txt`, `./docs/`) e um ZIP limpo ficam lidos; um
    /// sobreposto, um com dois diretorios possiveis, um estragado e um 7z
    /// ficam nao inspecionados; so um `.zip` e inspecionado.
    #[test]
    fn a_downloaded_zip_is_inspected_on_disk() {
        use crate::epub::test_support::{
            RawEntry, ZipBuilder, directory_gap_zip, two_end_records_zip, unicode_path_extra,
        };
        let dir = TempDir::new("zip");
        let pacote = ZipBuilder::new()
            .stored("setup.exe", &pe_bytes())
            .stored("run.bat", b"@echo off\r\necho oi\r\n")
            .build();
        let deleted = dir.file("pacote.zip", &pacote);
        assert_eq!(
            finalize_download(&deleted, false, false),
            FinalizeOutcome::Deleted(DeleteReason::ArchiveEntry(ZipEntryRisk::Blocked(
                BlockReason::Program
            )))
        );
        assert!(!deleted.exists(), "o pacote.zip ficou no disco");
        let allowed = dir.file("pacote.zip", &pacote);
        assert!(
            matches!(
                finalize_download(&allowed, false, true),
                FinalizeOutcome::Kept(_, Inspection::HoldsPrograms)
            ),
            "com a definicao, o pacote.zip fica e diz o que leva"
        );
        assert!(allowed.exists());

        // O que outro extrator tira do mesmo ZIP: um `aux.exe` (o Windows 11
        // grava-o), o nome do Unicode Path (0x7075) e o do cabecalho local.
        let mut unicode = RawEntry::stored("foto.jpg", &pe_bytes());
        unicode.extra = unicode_path_extra(b"foto.jpg", "setup.exe");
        let mut local = RawEntry::stored("foto.jpg", &pe_bytes());
        local.local_name = Some(b"setup.exe".to_vec());
        for (name, zip) in [
            (
                "dispositivo.zip",
                ZipBuilder::new()
                    .stored("LEIAME.txt", b"ola")
                    .stored("aux.exe", &pe_bytes())
                    .build(),
            ),
            (
                "unicode.zip",
                ZipBuilder::new()
                    .stored("LEIAME.txt", b"ola")
                    .entry(unicode)
                    .build(),
            ),
            (
                "local.zip",
                ZipBuilder::new()
                    .stored("LEIAME.txt", b"ola")
                    .entry(local)
                    .build(),
            ),
        ] {
            let path = dir.file(name, &zip);
            assert_eq!(
                finalize_download(&path, false, false),
                FinalizeOutcome::Deleted(DeleteReason::ArchiveEntry(ZipEntryRisk::Blocked(
                    BlockReason::Program
                ))),
                "{name}"
            );
            assert!(!path.exists(), "{name} ficou no disco");
        }

        // Um `..` no caminho (sobe de pasta) e um segmento de 62 000 bytes
        // (nenhum disco o grava) sao nomes estragados: apagam o ZIP mesmo
        // com a definicao ligada (ZI-5, ZI-6).
        let huge = format!("{}.txt", "a".repeat(61_996));
        for (name, entry) in [
            ("subida.zip", "a/../setup.exe"),
            ("enorme.zip", huge.as_str()),
        ] {
            let zip = ZipBuilder::new()
                .stored("LEIAME.txt", b"ola")
                .stored(entry, &pe_bytes())
                .build();
            for allow_programs in [false, true] {
                let path = dir.file(name, &zip);
                assert_eq!(
                    finalize_download(&path, false, allow_programs),
                    FinalizeOutcome::Deleted(DeleteReason::ArchiveEntry(ZipEntryRisk::Blocked(
                        BlockReason::BadName
                    ))),
                    "{name}, allow_programs={allow_programs}"
                );
                assert!(!path.exists(), "{name} ficou no disco");
            }
        }
        // Os segmentos do fim so de pontos e espacos caem: `setup.exe/.`
        // (o tar.exe grava o programa `setup.exe`), `setup.exe\.` e
        // `setup.exe/ .` sao o programa `setup.exe` (ZI-5).
        for (name, entry) in [
            ("ponto.zip", "setup.exe/."),
            ("contrabarra.zip", "setup.exe\\."),
            ("espaco.zip", "setup.exe/ ."),
        ] {
            let trailing = ZipBuilder::new()
                .stored("LEIAME.txt", b"ola")
                .stored(entry, &pe_bytes())
                .build();
            let path = dir.file(name, &trailing);
            assert_eq!(
                finalize_download(&path, false, false),
                FinalizeOutcome::Deleted(DeleteReason::ArchiveEntry(ZipEntryRisk::Blocked(
                    BlockReason::Program
                ))),
                "{name}"
            );
            assert!(!path.exists(), "o {name} ficou no disco");
            let path = dir.file(name, &trailing);
            assert!(
                matches!(
                    finalize_download(&path, false, true),
                    FinalizeOutcome::Kept(_, Inspection::HoldsPrograms)
                ),
                "{name}"
            );
        }
        // Um ZIP do bsdtar (`tar -a -cf x.zip .`): cada entrada comeca por
        // `./`, que nao muda o caminho. E um ZIP limpo, lido e guardado.
        let bsdtar = ZipBuilder::new()
            .stored("./docs/", b"")
            .stored("./docs/LEIAME.txt", b"ola")
            .stored("./fotos/praia.jpg", b"\xFF\xD8\xFF\xE0")
            .build();
        let path = dir.file("bsdtar.zip", &bsdtar);
        assert!(matches!(
            finalize_download(&path, false, false),
            FinalizeOutcome::Kept(_, Inspection::Checked)
        ));

        let fotos = ZipBuilder::new()
            .stored("fotos/praia.jpg", b"\xFF\xD8\xFF\xE0")
            .deflated("LEIAME.txt", b"ferias")
            .build();
        let clean = dir.file("fotos.zip", &fotos);
        assert!(matches!(
            finalize_download(&clean, false, false),
            FinalizeOutcome::Kept(_, Inspection::Checked)
        ));
        // As duas entradas no mesmo cabecalho local: a bomba de sobreposicao.
        let mut second = RawEntry::stored("fotos/b.jpg", b"\xFF\xD8\xFF\xE0");
        second.offset = Some(0);
        second.central_only = true;
        let overlap = ZipBuilder::new()
            .stored("fotos/a.jpg", b"\xFF\xD8\xFF\xE0")
            .entry(second)
            .build();
        for (name, bytes) in [
            ("sobreposto.zip", overlap),
            ("dois-fins.zip", two_end_records_zip()),
            ("folga.zip", directory_gap_zip()),
            ("estragado.zip", pacote[..pacote.len() - 30].to_vec()),
            ("pagina.zip", b"<html>nao e um zip</html>".to_vec()),
            ("arquivo.7z", vec![0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0, 4]),
        ] {
            let path = dir.file(name, &bytes);
            assert!(
                matches!(
                    finalize_download(&path, false, false),
                    FinalizeOutcome::Kept(_, Inspection::NotInspected)
                ),
                "{name}"
            );
            assert!(path.exists(), "{name}");
        }
        // So o `.zip` e inspecionado: o mesmo pacote com outro nome e um
        // arquivo que o Explorador nao abre como pasta.
        let other = dir.file("pacote.bin", &pacote);
        assert!(matches!(
            finalize_download(&other, false, false),
            FinalizeOutcome::Kept(_, Inspection::Checked)
        ));
        // O Windows tira o ponto do fim: `pacote.ZIP.` e um `.zip`.
        assert!(is_zip_name("pacote.ZIP."));
        assert!(!is_zip_name("pacote.zipx"));
    }

    /// O `inspection` de um `Completed` vai para o `downloads.json` so
    /// quando diz alguma coisa, e um registo de antes (sem ele) le-se como
    /// lido.
    #[test]
    fn the_inspection_round_trips_through_downloads_json() {
        let old: RecordOutcome =
            serde_json::from_str(r#"{"kind":"completed","warn":false}"#).expect("json");
        assert_eq!(
            old,
            RecordOutcome::Completed {
                warn: false,
                inspection: Inspection::Checked
            }
        );
        let checked = serde_json::to_string(&old).expect("json");
        assert!(!checked.contains("inspection"), "{checked}");
        for inspection in [Inspection::NotInspected, Inspection::HoldsPrograms] {
            let outcome = RecordOutcome::Completed {
                warn: true,
                inspection,
            };
            let text = serde_json::to_string(&outcome).expect("json");
            assert!(text.contains("inspection"), "{text}");
            assert_eq!(
                serde_json::from_str::<RecordOutcome>(&text).expect("json"),
                outcome
            );
        }
        let deleted = RecordOutcome::Deleted {
            reason: DeleteReason::ArchiveEntry(ZipEntryRisk::Blocked(BlockReason::Program)),
        };
        let text = serde_json::to_string(&deleted).expect("json");
        assert_eq!(
            text,
            r#"{"kind":"deleted","reason":{"archive-entry":{"blocked":"program"}}}"#
        );
        assert_eq!(
            serde_json::from_str::<RecordOutcome>(&text).expect("json"),
            deleted
        );
    }

    /// Gate critico: a marca da Web escrita e relida pelo fluxo alternativo
    /// do NTFS, so quando falta.
    #[cfg(windows)]
    #[test]
    fn motw_is_written_and_read_back_through_the_ads() {
        let dir = TempDir::new("motw");
        let pdf = dir.file("relatorio.pdf", PDF);
        assert_eq!(read_motw(&pdf).expect("ler"), None);
        assert_eq!(
            write_motw_if_absent(&pdf).expect("escrever"),
            MotwOutcome::Written
        );
        let written = read_motw(&pdf).expect("ler").expect("a marca");
        assert_eq!(written, MOTW_INTERNET);
        assert_eq!(motw_zone_id(&written), Some(3));
        assert!(!written.contains("HostUrl"), "{written}");
        // O conteudo do ficheiro nao mudou: a marca vive ao lado.
        assert_eq!(fs::read(&pdf).expect("ficheiro"), PDF);
        // Uma marca que ja existe (a do WebView2) fica como estava.
        let other = dir.file("de-fora.pdf", PDF);
        let theirs = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/x.pdf\r\n";
        fs::write(motw_stream_path(&other), theirs).expect("marca de fora");
        assert_eq!(
            write_motw_if_absent(&other).expect("escrever"),
            MotwOutcome::AlreadyPresent
        );
        assert_eq!(read_motw(&other).expect("ler").as_deref(), Some(theirs));
        // O fim de um download escreve-a.
        let done = dir.file("acabado.pdf", PDF);
        assert_eq!(
            finalize_download(&done, false, false),
            FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked)
        );
        assert_eq!(
            read_motw(&done)
                .expect("ler")
                .as_deref()
                .and_then(motw_zone_id),
            Some(3)
        );
        // Uma pasta nunca recebe a marca.
        assert!(write_motw_if_absent(&dir.0).is_err());
    }

    #[test]
    fn motw_zone_id_reads_the_zone_transfer_section() {
        assert_eq!(motw_zone_id(MOTW_INTERNET), Some(3));
        assert_eq!(
            motw_zone_id("[ZoneTransfer]\nReferrerUrl=x\nZoneId = 2\n"),
            Some(2)
        );
        assert_eq!(motw_zone_id("[Outra]\nZoneId=3\n"), None);
        assert_eq!(motw_zone_id(""), None);
    }

    /// Gate critico: nada de uma pagina privada chega ao registo -- nem
    /// recusado, nem acabado, nem cancelado, nem com a WebView destruida --,
    /// e o registo continua a guardar os outros.
    #[test]
    fn private_downloads_are_never_recorded() {
        let mut m = manager(false);
        let mut effects = Vec::new();
        effects.extend(m.on_event(start(1, "relatorio.pdf", true)));
        effects.extend(m.on_event(DownloadEvent::Ended {
            id: DownloadId(1),
            end: DownloadEnd::Completed {
                path: PathBuf::from(r"C:\d").join("relatorio.pdf"),
            },
        }));
        effects.extend(m.on_event(DownloadEvent::Finalized {
            id: DownloadId(1),
            outcome: FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked),
        }));
        effects.extend(m.on_event(start(2, "setup.exe", true)));
        effects.extend(m.on_event(start(3, "video.mp4", true)));
        effects.extend(m.on_event(DownloadEvent::Ended {
            id: DownloadId(3),
            end: DownloadEnd::Cancelled,
        }));
        effects.extend(m.on_event(start(4, "grande.zip", true)));
        effects.extend(m.on_event(DownloadEvent::WebViewGone {
            webview: WebViewKey(7),
        }));
        effects.extend(m.on_event(start(5, "mascara.pdf", true)));
        effects.extend(m.on_event(DownloadEvent::Ended {
            id: DownloadId(5),
            end: DownloadEnd::Completed {
                path: PathBuf::from(r"C:\d").join("mascara.pdf"),
            },
        }));
        effects.extend(m.on_event(DownloadEvent::Finalized {
            id: DownloadId(5),
            outcome: FinalizeOutcome::Deleted(DeleteReason::DangerousContent),
        }));
        assert!(
            !effects.contains(&DownloadEffect::Persist),
            "um download privado pediu gravacao: {effects:?}"
        );
        assert!(m.log().entries.is_empty(), "{:?}", m.log());
        // A lista da sessao mostra-os (o downloads-ui), o registo nao.
        assert_eq!(m.entries().count(), 5);

        // O mesmo percurso fora do privado grava cada fim.
        let mut m = manager(false);
        m.on_event(start(1, "relatorio.pdf", false));
        m.on_event(DownloadEvent::Ended {
            id: DownloadId(1),
            end: DownloadEnd::Completed {
                path: PathBuf::from(r"C:\d").join("relatorio.pdf"),
            },
        });
        let done = m.on_event(DownloadEvent::Finalized {
            id: DownloadId(1),
            outcome: FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked),
        });
        assert!(done.contains(&DownloadEffect::Persist), "{done:?}");
        let blocked = m.on_event(start(2, "setup.exe", false));
        assert!(blocked.contains(&DownloadEffect::Persist), "{blocked:?}");
        let log = m.log();
        assert_eq!(log.entries.len(), 2);
        assert_eq!(log.entries[0].name, "setup.exe");
        assert_eq!(
            log.entries[0].outcome,
            RecordOutcome::Blocked {
                reason: BlockReason::Program
            }
        );
        assert_eq!(log.entries[0].path, None, "um recusado nao tem ficheiro");
        assert_eq!(log.entries[1].name, "relatorio.pdf");
        assert_eq!(
            log.entries[1].path.as_deref(),
            Some(PathBuf::from(r"C:\d").join("relatorio.pdf").as_path())
        );
        assert_eq!(log.entries[1].host.as_deref(), Some("example.com"));
    }

    /// Gate critico (DM-1 da revisao): nada feito no Modo privado entra no
    /// registo -- nem o que comeca nele e acaba depois, nem o que comeca
    /// antes e acaba nele --, e por isso nada disso chega ao ficheiro quando
    /// o modo volta ao normal e a gravacao seguinte escreve o registo todo.
    #[test]
    fn nothing_made_in_private_mode_is_ever_recorded() {
        let finish = |m: &mut DownloadManager, id: u64, name: &str| {
            let mut effects = m.on_event(DownloadEvent::Ended {
                id: DownloadId(id),
                end: DownloadEnd::Completed {
                    path: PathBuf::from(r"C:\d").join(name),
                },
            });
            effects.extend(m.on_event(DownloadEvent::Finalized {
                id: DownloadId(id),
                outcome: FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked),
            }));
            effects
        };
        let mut m = manager(false);
        // Comeca no normal e acaba no privado.
        m.on_event(start(1, "antes.pdf", false));
        m.set_private_mode(true);
        assert!(m.private_mode());
        let mut effects = finish(&mut m, 1, "antes.pdf");
        // Comeca e acaba no privado; recusado no privado; comeca no privado
        // e acaba ja no normal.
        effects.extend(m.on_event(start(2, "segredo-modo-privado.pdf", false)));
        effects.extend(finish(&mut m, 2, "segredo-modo-privado.pdf"));
        effects.extend(m.on_event(start(3, "setup.exe", false)));
        effects.extend(m.on_event(start(4, "depois.zip", false)));
        assert!(m.entry(DownloadId(4)).expect("entrada").private);
        m.set_private_mode(false);
        effects.extend(finish(&mut m, 4, "depois.zip"));
        assert!(
            !effects.contains(&DownloadEffect::Persist),
            "o Modo privado pediu gravacao: {effects:?}"
        );
        assert!(m.log().entries.is_empty(), "{:?}", m.log());
        // A lista da sessao mostra-os; o registo nao.
        assert_eq!(m.entries().count(), 4);

        // De volta ao normal, um download novo grava -- e o registo que vai
        // para o ficheiro so tem esse.
        m.on_event(start(5, "normal.pdf", false));
        let done = finish(&mut m, 5, "normal.pdf");
        assert!(done.contains(&DownloadEffect::Persist), "{done:?}");
        let names: Vec<String> = m.log().entries.into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["normal.pdf".to_string()]);
    }

    /// Gate critico (revisao do PR #176): a verificacao do fim nao se
    /// cancela e deixa rasto. O fim grava um registo `Pending` com o caminho
    /// (e so fora do privado: o modo do FIM conta, mesmo que o veredito caia
    /// ja no normal); um Cancel durante a verificacao nao faz nada; o
    /// veredito troca o pendente pelo desfecho, com o aviso; um fim perdido
    /// pede-se outra vez; e o arranque seguinte retoma os pendentes uma vez
    /// so, com a marca `resumed` gravada antes de o fim correr.
    #[test]
    fn a_verification_is_never_cancelled_and_leaves_a_pending_record() {
        let path = |name: &str| PathBuf::from(r"C:\d").join(name);
        let ended = |m: &mut DownloadManager, id: u64, name: &str| {
            m.on_event(DownloadEvent::Ended {
                id: DownloadId(id),
                end: DownloadEnd::Completed { path: path(name) },
            })
        };
        let deleted = FinalizeOutcome::Deleted(DeleteReason::ArchiveEntry(ZipEntryRisk::Blocked(
            BlockReason::Program,
        )));

        // O fim grava o pendente (com o caminho) antes do veredito.
        let mut m = manager(false);
        m.on_event(start(1, "pacote.zip", false));
        let effects = ended(&mut m, 1, "pacote.zip");
        assert!(effects.contains(&DownloadEffect::Persist), "{effects:?}");
        let log = m.log();
        assert_eq!(log.entries.len(), 1);
        assert_eq!(
            log.entries[0].outcome,
            RecordOutcome::Pending {
                confirmed_program: false,
                resumed: false
            }
        );
        assert_eq!(
            log.entries[0].path.as_deref(),
            Some(path("pacote.zip").as_path())
        );
        // Cancelar a verificacao nao faz nada: nem estado, nem registo.
        assert!(
            m.on_event(DownloadEvent::CancelRequested { id: DownloadId(1) })
                .is_empty()
        );
        assert_eq!(
            m.entry(DownloadId(1)).expect("entrada").state,
            DownloadState::Finalizing
        );
        assert_eq!(m.active(), 1);
        // O veredito chega e e aplicado: o aviso, a linha e o registo (o
        // pendente sai, fica o desfecho).
        let verdict = m.on_event(DownloadEvent::Finalized {
            id: DownloadId(1),
            outcome: deleted,
        });
        assert!(
            verdict.contains(&DownloadEffect::Notice(DownloadNotice::Deleted {
                id: DownloadId(1),
                name: "pacote.zip".to_string(),
                reason: DeleteReason::ArchiveEntry(ZipEntryRisk::Blocked(BlockReason::Program)),
            })),
            "{verdict:?}"
        );
        assert!(verdict.contains(&DownloadEffect::Persist), "{verdict:?}");
        let log = m.log();
        assert_eq!(log.entries.len(), 1, "{log:?}");
        assert!(matches!(
            log.entries[0].outcome,
            RecordOutcome::Deleted { .. }
        ));
        // Um fim perdido pede-se outra vez; so a quem ainda verifica.
        m.on_event(start(2, "fotos.zip", false));
        ended(&mut m, 2, "fotos.zip");
        assert_eq!(
            m.on_event(DownloadEvent::FinalizeLost { id: DownloadId(2) }),
            vec![DownloadEffect::Finalize {
                id: DownloadId(2),
                path: path("fotos.zip"),
                confirmed_program: false,
                allow_programs: false,
            }]
        );
        assert!(
            m.on_event(DownloadEvent::FinalizeLost { id: DownloadId(1) })
                .is_empty()
        );

        // Acabou no Modo privado e o veredito chega ja no normal: nada no
        // registo, nem o pendente.
        let mut m = manager(false);
        m.on_event(start(3, "antes.pdf", false));
        m.set_private_mode(true);
        let effects = ended(&mut m, 3, "antes.pdf");
        assert!(!effects.contains(&DownloadEffect::Persist), "{effects:?}");
        m.set_private_mode(false);
        let effects = m.on_event(DownloadEvent::Finalized {
            id: DownloadId(3),
            outcome: FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked),
        });
        assert!(!effects.contains(&DownloadEffect::Persist), "{effects:?}");
        assert!(m.log().entries.is_empty(), "{:?}", m.log());
        // Acabou no normal e o veredito cai no Modo privado: o desfecho
        // troca o pendente (que ja estava no disco); nada fica pendente.
        m.on_event(start(4, "normal.pdf", false));
        ended(&mut m, 4, "normal.pdf");
        m.set_private_mode(true);
        // Um `Completed` repetido ja no Modo privado nao o torna privado.
        assert_eq!(
            ended(&mut m, 4, "normal.pdf"),
            vec![DownloadEffect::ForgetOp(DownloadId(4))]
        );
        assert!(!m.entry(DownloadId(4)).expect("entrada").private);
        m.on_event(DownloadEvent::Finalized {
            id: DownloadId(4),
            outcome: FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked),
        });
        let outcomes: Vec<RecordOutcome> = m.log().entries.into_iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            vec![RecordOutcome::Completed {
                warn: false,
                inspection: Inspection::Checked
            }]
        );

        // O arranque seguinte: um pendente com caminho volta a verificar, com
        // o `Persist` antes do `Finalize`; um sem caminho e um ja retomado
        // ficam como estao (nao correm).
        let pending =
            |name: &str, path: Option<PathBuf>, confirmed_program, resumed| DownloadRecord {
                name: name.to_string(),
                path,
                host: Some("example.com".to_string()),
                bytes: Some(64),
                outcome: RecordOutcome::Pending {
                    confirmed_program,
                    resumed,
                },
                at: 7,
            };
        let mut m = DownloadManager::new(
            DownloadSettings {
                folder: None,
                allow_programs: true,
            },
            DownloadLog {
                entries: vec![
                    pending("setup.exe", Some(path("setup.exe")), true, false),
                    pending("longe.zip", None, false, false),
                    pending("de-novo.zip", Some(path("de-novo.zip")), false, true),
                ],
            },
        );
        let mut ids = 40..;
        let effects = m.resume_pending(|| DownloadId(ids.next().expect("numero")));
        assert_eq!(
            effects,
            vec![
                DownloadEffect::Persist,
                DownloadEffect::Finalize {
                    id: DownloadId(40),
                    path: path("setup.exe"),
                    confirmed_program: true,
                    allow_programs: true,
                },
                DownloadEffect::Changed(DownloadId(40)),
            ]
        );
        let entry = m.entry(DownloadId(40)).expect("retomado");
        assert_eq!(entry.state, DownloadState::Finalizing);
        assert!(!entry.private && entry.confirmed_program);
        assert!(
            m.log().entries.iter().all(|record| matches!(
                record.outcome,
                RecordOutcome::Pending { resumed: true, .. }
            ))
        );
        // Retomar outra vez (outro arranque sem veredito) nao corre nada.
        let mut again = DownloadManager::new(DownloadSettings::default(), m.log());
        assert!(again.resume_pending(|| DownloadId(90)).is_empty());
        assert_eq!(again.entries().count(), 0);
        // O veredito do retomado troca o pendente dele.
        m.on_event(DownloadEvent::Finalized {
            id: DownloadId(40),
            outcome: FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked),
        });
        let names: Vec<(String, bool)> = m
            .log()
            .entries
            .into_iter()
            .map(|r| (r.name, matches!(r.outcome, RecordOutcome::Pending { .. })))
            .collect();
        assert_eq!(
            names,
            vec![
                ("setup.exe".to_string(), false),
                ("longe.zip".to_string(), true),
                ("de-novo.zip".to_string(), true),
            ]
        );

        // O pendente no `downloads.json`: so as marcas que dizem algo.
        let text = serde_json::to_string(&RecordOutcome::Pending {
            confirmed_program: false,
            resumed: false,
        })
        .expect("json");
        assert_eq!(text, r#"{"kind":"pending"}"#);
        let back: RecordOutcome =
            serde_json::from_str(r#"{"kind":"pending","confirmed_program":true,"resumed":true}"#)
                .expect("json");
        assert_eq!(
            back,
            RecordOutcome::Pending {
                confirmed_program: true,
                resumed: true
            }
        );
    }

    /// O ciclo de um download e as operacoes do WebView2: largadas quando
    /// acaba e quando a WebView dele e destruida.
    #[test]
    fn ops_are_forgotten_on_finish_and_on_webview_destroy() {
        let mut m = manager(true);
        m.on_event(start(1, "relatorio.pdf", false));
        let progress = m.on_event(DownloadEvent::Progress {
            id: DownloadId(1),
            received: 400,
            total: Some(1000),
        });
        assert_eq!(progress, vec![DownloadEffect::Changed(DownloadId(1))]);
        assert_eq!(m.entry(DownloadId(1)).expect("entrada").received, 400);
        let ended = m.on_event(DownloadEvent::Ended {
            id: DownloadId(1),
            end: DownloadEnd::Completed {
                path: PathBuf::from(r"C:\d").join("relatorio (1).pdf"),
            },
        });
        assert_eq!(ended[0], DownloadEffect::ForgetOp(DownloadId(1)));
        assert_eq!(
            ended[1],
            DownloadEffect::Finalize {
                id: DownloadId(1),
                path: PathBuf::from(r"C:\d").join("relatorio (1).pdf"),
                confirmed_program: false,
                // O gestor deste teste tem «Permitir baixar programas».
                allow_programs: true,
            }
        );
        // Um StateChanged repetido nao finaliza duas vezes.
        assert_eq!(
            m.on_event(DownloadEvent::Ended {
                id: DownloadId(1),
                end: DownloadEnd::Completed {
                    path: PathBuf::from(r"C:\d").join("relatorio (1).pdf"),
                },
            }),
            vec![DownloadEffect::ForgetOp(DownloadId(1))]
        );

        // A WebView 7 vai-se: o que corria e cancelado e largado, o que
        // esperava a resposta e recusado; um de outra WebView fica.
        m.on_event(start(2, "grande.zip", false));
        m.on_event(start(3, "setup.exe", false));
        m.on_event(DownloadEvent::Starting(DownloadStart {
            id: DownloadId(4),
            webview: WebViewKey(8),
            private: false,
            proposed: PathBuf::from(r"C:\d").join("outro.zip"),
            host: None,
            total: None,
            at: 0,
        }));
        let gone = m.on_event(DownloadEvent::WebViewGone {
            webview: WebViewKey(7),
        });
        assert!(gone.contains(&DownloadEffect::CancelRunning(DownloadId(2))));
        assert!(gone.contains(&DownloadEffect::ForgetOp(DownloadId(2))));
        assert!(gone.contains(&DownloadEffect::Refuse(DownloadId(3))));
        assert!(
            !gone.iter().any(|effect| matches!(
                effect,
                DownloadEffect::CancelRunning(DownloadId(4))
                    | DownloadEffect::ForgetOp(DownloadId(4))
                    | DownloadEffect::Refuse(DownloadId(4))
            )),
            "{gone:?}"
        );
        assert_eq!(m.active(), 2, "o 1 a finalizar e o 4 da outra WebView");
        // Um Completed que chega depois do Cancel ainda passa pelo fim.
        let late = m.on_event(DownloadEvent::Ended {
            id: DownloadId(2),
            end: DownloadEnd::Completed {
                path: PathBuf::from(r"C:\d").join("grande.zip"),
            },
        });
        assert!(
            late.iter()
                .any(|effect| matches!(effect, DownloadEffect::Finalize { .. })),
            "{late:?}"
        );
        // Cancelar um a correr pede o Cancel da operacao.
        assert_eq!(
            m.on_event(DownloadEvent::CancelRequested { id: DownloadId(4) }),
            vec![DownloadEffect::CancelRunning(DownloadId(4))]
        );
        // Um evento de um numero desconhecido so larga a operacao.
        assert_eq!(
            m.on_event(DownloadEvent::Ended {
                id: DownloadId(99),
                end: DownloadEnd::Interrupted,
            }),
            vec![DownloadEffect::ForgetOp(DownloadId(99))]
        );
    }

    #[test]
    fn the_log_keeps_the_newest_200_within_the_field_caps() {
        let old: Vec<DownloadRecord> = (0..250)
            .map(|n| DownloadRecord {
                name: format!("velho-{n}.pdf"),
                path: None,
                host: None,
                bytes: None,
                outcome: RecordOutcome::Cancelled,
                at: n,
            })
            .collect();
        let mut m = DownloadManager::new(DownloadSettings::default(), DownloadLog { entries: old });
        assert_eq!(m.log().entries.len(), MAX_LOG_ENTRIES);
        assert_eq!(m.log().entries[0].name, "velho-0.pdf");
        let long = format!("{}.pdf", "n".repeat(400));
        m.on_event(start(1, &long, false));
        m.on_event(DownloadEvent::Ended {
            id: DownloadId(1),
            end: DownloadEnd::Completed {
                path: PathBuf::from(format!(r"C:\{}\x.pdf", "p".repeat(2000))),
            },
        });
        m.on_event(DownloadEvent::Finalized {
            id: DownloadId(1),
            outcome: FinalizeOutcome::Kept(MotwOutcome::Written, Inspection::Checked),
        });
        let log = m.log();
        assert_eq!(log.entries.len(), MAX_LOG_ENTRIES);
        assert_eq!(log.entries[0].name.chars().count(), MAX_RECORD_NAME_CHARS);
        assert_eq!(log.entries[0].path, None, "caminho grande demais");
        assert_eq!(log.entries[MAX_LOG_ENTRIES - 1].name, "velho-198.pdf");
        // O pendente do fim (gravado cortado: o nome, sem o caminho) saiu
        // quando o veredito chegou.
        assert!(
            !log.entries
                .iter()
                .any(|record| matches!(record.outcome, RecordOutcome::Pending { .. })),
            "{:?}",
            log.entries[0]
        );
        // 200 registos no tamanho maximo cabem no tecto do ficheiro.
        let worst = DownloadRecord {
            name: "\u{1F600}".repeat(MAX_RECORD_NAME_CHARS),
            path: Some(PathBuf::from("\\".repeat(MAX_RECORD_PATH_BYTES))),
            host: Some("h".repeat(MAX_RECORD_HOST_BYTES)),
            bytes: Some(u64::MAX),
            outcome: RecordOutcome::NotDeleted {
                reason: DeleteReason::BlockedName(BlockReason::DatabaseApp),
            },
            at: u64::MAX,
        };
        let full = DownloadLog {
            entries: vec![worst; MAX_LOG_ENTRIES],
        };
        let bytes =
            serde_json::to_vec_pretty(&serde_json::json!({"version": LOG_VERSION, "data": full}))
                .expect("json");
        assert!(
            (bytes.len() as u64) < LOG_MAX_BYTES,
            "{} bytes passam o tecto",
            bytes.len()
        );
        // Ctrl+Shift+Delete: o registo e os acabados saem, o que corre fica;
        // o ficheiro sai do disco (`EraseLog`), nunca e regravado.
        m.on_event(start(2, "a-correr.zip", false));
        assert_eq!(
            m.on_event(DownloadEvent::ClearLog),
            vec![DownloadEffect::EraseLog]
        );
        assert!(m.log().entries.is_empty());
        assert_eq!(m.entries().count(), 1);
        assert!(m.entry(DownloadId(2)).is_some());
    }

    #[test]
    fn progress_is_throttled_to_250_ms() {
        let mut throttle = ProgressThrottle::default();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let admitted: Vec<u64> = [0, 10, 100, 249, 250, 251, 400, 499, 500, 1000, 1001]
            .into_iter()
            .filter(|&ms| throttle.admit(at(ms)))
            .collect();
        assert_eq!(admitted, vec![0, 250, 500, 1000]);
    }

    /// Um caminho absoluto na plataforma que corre o teste.
    fn abs(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(if cfg!(windows) { r"D:\" } else { "/" });
        path.extend(parts);
        path
    }

    #[test]
    fn the_folder_and_the_target_path() {
        let folder = abs(&["baixados"]);
        let folder = folder.as_path();
        let proposed = abs(&["Users", "x", "Downloads", "relatorio.pdf"]);
        assert_eq!(target_path(&proposed, None), proposed);
        assert_eq!(
            target_path(&proposed, Some(folder)),
            folder.join("relatorio.pdf")
        );
        let already = folder.join("relatorio (2).pdf");
        assert_eq!(target_path(&already, Some(folder)), already);
        let taken = [folder.join("a.pdf"), folder.join("a (1).pdf")];
        assert_eq!(
            unique_path(folder, "a.pdf", |p| taken.iter().any(|t| t == p)),
            folder.join("a (2).pdf")
        );
        assert_eq!(
            unique_path(folder, "b.pdf", |_| false),
            folder.join("b.pdf")
        );
        assert_eq!(
            unique_path(folder, "LEIAME", |p| p == folder.join("LEIAME")),
            folder.join("LEIAME (1)")
        );
        let relative = DownloadSettings {
            folder: Some(PathBuf::from("relativa")),
            allow_programs: false,
        };
        assert_eq!(relative.usable_folder(|_| true), None);
        let missing = DownloadSettings {
            folder: Some(abs(&["x"])),
            allow_programs: false,
        };
        assert_eq!(missing.usable_folder(|_| false), None);
        assert!(missing.usable_folder(|_| true).is_some());
        // O gestor usa a pasta escolhida no Proceed; uma relativa nao conta.
        let mut m = DownloadManager::new(
            DownloadSettings {
                folder: Some(folder.to_path_buf()),
                allow_programs: false,
            },
            DownloadLog::default(),
        );
        let effects = m.on_event(start(1, "relatorio.pdf", true));
        assert_eq!(
            effects[0],
            DownloadEffect::Proceed {
                id: DownloadId(1),
                path: folder.join("relatorio.pdf"),
            }
        );
        let mut m = DownloadManager::new(relative, DownloadLog::default());
        let effects = m.on_event(start(1, "relatorio.pdf", true));
        assert_eq!(
            effects[0],
            DownloadEffect::Proceed {
                id: DownloadId(1),
                path: PathBuf::from(r"C:\Users\x\Downloads").join("relatorio.pdf"),
            }
        );
    }
}
