//! Classificação portável de segurança de arquivos e nomes de download.
//!
//! Usado pelo subsistema de downloads e pelo explorador de arquivos para impedir
//! execução acidental de programas, scripts, imagens de disco, atalhos do Windows
//! e arquivos mascarados com extensões falsas, caracteres bidi ou invisíveis.
//!
//! Um ZIP baixado tem as entradas listadas pelo diretório central e pelos
//! cabeçalhos locais ([`inspect_zip`], downloads-zip-inspect), sem ler os
//! dados de nenhuma.

use std::{
    fs::File,
    io::{Read, Seek},
    path::{Path, PathBuf},
};

use crate::safezip::{self, ZipPolicy};

/// Categoria de risco para um nome ou arquivo baixado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskClass {
    /// Arquivo seguro para abertura ou manuseio comum.
    Safe,
    /// Arquivo que requer atenção do usuário (ex: documento com macros).
    Warn,
    /// Arquivo perigoso ou proibido (executáveis, scripts, imagens de disco,
    /// atalhos, arquivos mascarados ou nomes maliciosos com bidi/ADS).
    Block,
}

/// Diagnóstico de risco obtido por sniffing dos primeiros bytes (até 4 KiB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SniffRisk {
    /// Formato comum não executável identificado ou dados neutros.
    Safe,
    /// Formato compactado cujos conteúdos internos não foram inspecionados (ex.: 7z, RAR).
    NotInspected,
    /// Script ou lote detectado por cabeçalho em arquivo sem extensão de script.
    Warn,
    /// Binário executável nativo Windows (MZ/PE), atalho Shell (LNK) ou gabinete (CAB).
    Dangerous,
}

/// Extensões de executáveis e instaladores nativos do Windows.
///
/// `.msu` (pacote do Windows Update), `.mst` (transformação do Windows
/// Installer), `.xbap` (aplicação XAML do navegador) e `.xll` (suplemento
/// do Excel: uma DLL nativa que o Excel carrega e executa ao abrir).
pub const PROGRAM_EXTENSIONS: &[&str] = &[
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "msi",
    "msp",
    "msu",
    "mst",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "appinstaller",
    "xbap",
    "xll",
];

/// Extensões de scripts ou interpretadores executáveis.
///
/// Python (`.py`, `.pyw`, `.pyz`, `.pyzw` e os compilados `.pyc`/`.pyo`)
/// corre com um duplo clique pelo `py` launcher; `.ps1xml`/`.ps2xml` e
/// `.psc1`/`.psc2` são do PowerShell; `.sct` é um scriptlet COM; `.jnlp`
/// abre o Java Web Start.
pub const SCRIPT_EXTENSIONS: &[&str] = &[
    "bat", "cmd", "ps1", "psm1", "psd1", "ps1xml", "ps2xml", "psc1", "psc2", "vbs", "vbe", "js",
    "jse", "wsf", "wsh", "wsc", "sct", "hta", "jar", "jnlp", "py", "pyw", "pyz", "pyzw", "pyc",
    "pyo",
];

/// Extensões de atalhos e integrações shell do Windows.
pub const SHORTCUT_SHELL_EXTENSIONS: &[&str] = &[
    "lnk",
    "url",
    "website",
    "appref-ms",
    "application",
    "gadget",
    "msc",
    "inf",
    "reg",
    "scf",
    "chm",
    "hlp",
    "diagcab",
    "library-ms",
    "search-ms",
    "searchconnector-ms",
    "settingcontent-ms",
    // Objetos de recorte do shell (executam o que embrulham).
    "shb",
    "shs",
    // Ligação de Área de Trabalho Remota: pode mapear discos e a área de
    // transferência para um servidor de quem enviou o arquivo.
    "rdp",
    // Temas: apontam para recursos remotos (o Windows autentica-se neles).
    "theme",
    "themepack",
    // Configuração do Windows Sandbox: mapeia pastas e corre um comando.
    "wsb",
];

/// Extensões de imagens de disco e contêineres montáveis.
pub const DISKIMAGE_EXTENSIONS: &[&str] = &["iso", "img", "vhd", "vhdx"];

/// Bases e projetos do Access: abrem com macros de arranque e VBA, e o
/// Outlook bloqueia-os como anexos.
pub const DATABASE_APP_EXTENSIONS: &[&str] = &["mda", "mdb", "mde", "accde", "ade", "adp"];

/// Arquivos compactados. Dentro de um ZIP baixado, um destes esconde o que
/// tem: a inspeção nunca lê os dados de uma entrada, por isso um ZIP que leva
/// outro conta como um que leva programas ([`ZipEntryRisk::NestedArchive`]).
/// As imagens de disco (`.iso`, `.vhd`...) e o `.jar` já bloqueiam pelas
/// listas deles; os documentos que são ZIP por dentro (`.docx`, `.epub`)
/// não entram.
pub const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "zipx", "7z", "rar", "cab", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz",
    "zst", "lz", "lzma", "z", "arj", "lzh", "lha", "ace", "wim", "cpio",
];

/// Extensões de documentos com macros habilitadas (Office/Office-like). Sozinhas
/// são um aviso; depois de uma extensão de fachada (`fatura.pdf.docm`) são um
/// disfarce e bloqueiam.
///
/// Os 10 tipos OOXML com macros habilitadas (`.ppsm` abre direto no modo de
/// exibição) e os binários antigos que também as levam: `.xlsb` (pasta
/// binária), `.xlm` (folha de macros do Excel 4), `.xla` e `.ppa`
/// (suplementos: só existem para levar código) e `.pps` (apresentação antiga
/// que abre direto no modo de exibição).
pub const MACRO_EXTENSIONS: &[&str] = &[
    "docm", "dotm", "xlsm", "xltm", "xlam", "pptm", "potm", "ppam", "ppsm", "sldm", "xlsb", "xlm",
    "xla", "ppa", "pps",
];

/// Extensões conhecidas de documentos e texto, páginas, imagens, áudio e
/// vídeo e arquivos compactados comuns (nesta ordem) para detecção de
/// extensão dupla (masquerading). Contém cada tipo do
/// [`DEFAULT_APP_ALLOWLIST`] (gate `every_default_app_type_is_a_decoy`):
/// `livro.epub.docm` finge ser um livro que a própria NeuralIA abre.
pub const SAFE_DECOY_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "odt", "ods", "odp", "odg",
    "csv", "json", "xml", "md", "log", "epub", "eml", "msg", "ics", "vcf", "html", "htm", "xhtml",
    "mht", "mhtml", "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "heif", "avif",
    "tif", "tiff", "ico", "mp3", "m4a", "wav", "flac", "ogg", "opus", "aac", "mp4", "m4v", "webm",
    "mov", "avi", "mkv", "wmv", "zip", "7z", "rar", "gz", "tar",
];

/// Allowlist estrita de extensões permitidas no `DefaultAppTarget`.
///
/// Sem `.svg`: no Windows abre no navegador, que corre o `<script>` do
/// arquivo a partir de `file://` (SVG smuggling), como um `.html`. Esses
/// ficam com "Mostrar na pasta".
pub const DEFAULT_APP_ALLOWLIST: &[&str] = &[
    "txt", "md", "csv", "json", "log", "pdf", "epub", "docx", "xlsx", "pptx", "odt", "ods", "odp",
    "rtf", "png", "jpg", "jpeg", "gif", "webp", "bmp", "mp3", "m4a", "wav", "flac", "ogg", "opus",
    "webm", "mp4", "mov",
];

/// Lista de nomes reservados de dispositivos do DOS/Windows (ex.: CON, PRN, AUX, NUL, COM1..9, LPT1..9).
pub const WINDOWS_RESERVED_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Retorna se o caractere é de controle bidi ou C0/C1.
pub fn is_bidi_or_control(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}' // LRE, RLE, PDF, LRO, RLO
        | '\u{2066}'..='\u{2069}' // LRI, RLI, FSI, PDI
        | '\u{200E}' | '\u{200F}' // LRM, RLM
    ) || c.is_control()
}

/// Caractere de formato que não se vê: os Default_Ignorable_Code_Point do
/// Unicode (soft hyphen, ZWSP, word joiner, BOM, preenchimentos Hangul,
/// operadores invisíveis, caracteres tag, marcas bidi...) e o braille vazio
/// U+2800, que pinta um espaço sem ser espaço. Num nome de arquivo escondem
/// uma fachada (`fatura.pdf<ZWSP>.docm` lê-se `fatura.pdf.docm`) e bloqueiam,
/// como os bidi. Ficam de fora os [`is_text_joiner`], que o texto legítimo
/// usa.
fn is_hidden_format(c: char) -> bool {
    !is_text_joiner(c)
        && matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'..='\u{1160}'
                | '\u{17B4}'..='\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{2800}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF0}'..='\u{FFF8}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}

/// ZWNJ e ZWJ (persa, línguas índicas, sequências de emoji) e os seletores de
/// variação (o `❤️` é `U+2764 U+FE0F`): Default_Ignorable, mas parte de texto
/// legítimo. Não bloqueiam; a análise da extensão corre sem eles, para não
/// separarem uma fachada nem partirem uma extensão.
fn is_text_joiner(c: char) -> bool {
    matches!(
        c,
        '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}'
    )
}

/// Pontos que se leem como o `.` sem o ser (confusables do Unicode, os pontos
/// finais ideográficos e os pontos a meia altura). O Windows não os vê como
/// separador de extensão, mas quem lê `fatura．pdf.docm` vê uma fachada.
fn is_dot_lookalike(c: char) -> bool {
    matches!(
        c,
        '\u{00B7}'
            | '\u{0660}'
            | '\u{06D4}'
            | '\u{06F0}'
            | '\u{0701}'
            | '\u{0702}'
            | '\u{2024}'
            | '\u{2027}'
            | '\u{2E31}'
            | '\u{3002}'
            | '\u{A4F8}'
            | '\u{A60E}'
            | '\u{FE52}'
            | '\u{FF0E}'
            | '\u{FF61}'
            | '\u{10A50}'
            | '\u{1D16D}'
    )
}

/// O que vem antes da extensão (`stem`, sem o último `.`) termina numa
/// extensão de fachada: o último segmento não vazio depois do nome, com os
/// segmentos separados pelo `.` e pelos seus sósias. `fatura.pdf. ` e
/// `fatura．pdf` terminam em `pdf`; `pdf` sozinho é o nome, não uma fachada.
fn ends_in_decoy(stem: &str) -> bool {
    stem.split(|c| c == '.' || is_dot_lookalike(c))
        .skip(1)
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .last()
        .is_some_and(|segment| {
            SAFE_DECOY_EXTENSIONS.contains(&segment.to_ascii_lowercase().as_str())
        })
}

/// Normaliza um nome de arquivo segundo as regras fundamentais do subsistema Win32:
/// corta espaços e pontos finais à direita, e corta em ':' (Alternate Data Streams).
pub fn normalize_windows_name(name: &str) -> String {
    let before_ads = match name.split_once(':') {
        Some((stem, _)) => stem,
        None => name,
    };
    before_ads.trim_end_matches([' ', '.']).to_lowercase()
}

/// Substitui caracteres bidi, controles e caracteres de formato invisíveis
/// (ZWSP, word joiner, soft hyphen...) em rótulos visuais por marcas
/// explícitas legíveis. Os joiners dos emoji e das escritas que os usam
/// ficam como estão.
pub fn display_label(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '\u{202A}' => out.push_str("‹LRE›"),
            '\u{202B}' => out.push_str("‹RLE›"),
            '\u{202C}' => out.push_str("‹PDF›"),
            '\u{202D}' => out.push_str("‹LRO›"),
            '\u{202E}' => out.push_str("‹RLO›"),
            '\u{2066}' => out.push_str("‹LRI›"),
            '\u{2067}' => out.push_str("‹RLI›"),
            '\u{2068}' => out.push_str("‹FSI›"),
            '\u{2069}' => out.push_str("‹PDI›"),
            '\u{200E}' => out.push_str("‹LRM›"),
            '\u{200F}' => out.push_str("‹RLM›"),
            _ if c.is_control() || is_hidden_format(c) => {
                out.push_str(&format!("‹U+{:04X}›", c as u32));
            }
            _ => out.push(c),
        }
    }
    out
}

/// Classifica o risco de um nome de arquivo para download ou abertura.
///
/// Refusa nomes com controles bidi, caracteres de controle C0/C1, caracteres
/// de formato invisíveis (Default_Ignorable) ou dois-pontos (ADS).
/// Trata espaços e pontos ao final como Win32 trata.
/// Bloqueia executáveis, scripts, imagens de disco e mascaramentos com extensão dupla
/// (também um documento com macro depois de uma extensão de fachada, mesmo
/// separada por segmentos em branco ou por um sósia do ponto).
/// Emite aviso para documentos com macro.
pub fn classify_download_name(name: &str) -> RiskClass {
    if name.is_empty() {
        return RiskClass::Block;
    }

    // Refusa ':' (Alternate Data Streams), caracteres de controle ou bidi e
    // os de formato que não se veem.
    if name.contains(':')
        || name
            .chars()
            .any(|c| is_bidi_or_control(c) || is_hidden_format(c))
    {
        return RiskClass::Block;
    }

    // Os joiners não pintam nada: a análise corre sem eles, para não
    // separarem uma fachada (`fatura.pdf<ZWJ>.docm`) nem partirem uma
    // extensão. Tirá-los só junta o que o Windows vê separado: uma extensão
    // real perigosa continua perigosa aqui.
    let visible: String = name.chars().filter(|&c| !is_text_joiner(c)).collect();

    // Regra do Windows: remove espaços e pontos no final
    let clean = visible.trim_end_matches([' ', '.']);
    if clean.is_empty() {
        return RiskClass::Block;
    }

    // Verifica dispositivos reservados do Windows (ex.: CON, NUL.txt, etc.)
    let base_stem = clean
        .split('.')
        .next()
        .unwrap_or(clean)
        .trim()
        .to_ascii_lowercase();
    if WINDOWS_RESERVED_DEVICE_NAMES.contains(&base_stem.as_str()) {
        return RiskClass::Block;
    }

    // Decomposição de extensões: `stem` é tudo antes do último ponto.
    let Some((stem, last_raw)) = clean.rsplit_once('.') else {
        // Sem extensão
        return RiskClass::Safe;
    };
    let final_ext = last_raw.trim().to_ascii_lowercase();

    // Verificação de executáveis, scripts e imagens de disco -> Block
    let is_dangerous_ext = PROGRAM_EXTENSIONS.contains(&final_ext.as_str())
        || SCRIPT_EXTENSIONS.contains(&final_ext.as_str())
        || SHORTCUT_SHELL_EXTENSIONS.contains(&final_ext.as_str())
        || DISKIMAGE_EXTENSIONS.contains(&final_ext.as_str())
        || DATABASE_APP_EXTENSIONS.contains(&final_ext.as_str());
    let is_macro_ext = MACRO_EXTENSIONS.contains(&final_ext.as_str());

    // Detecção de Masquerade (disfarce com extensão dupla ou espaçamento):
    // Exemplo: `fatura.pdf.exe`, `foto.jpg     .scr`, `documento.docx.vbs`.
    // Um documento com macro disfarçado (`fatura.pdf.docm`, `fatura.pdf. .docm`,
    // `fatura．pdf.docm`, `livro.epub.docm`) também bloqueia: disfarce não
    // tem exceção.
    if (is_dangerous_ext || is_macro_ext) && ends_in_decoy(stem) {
        return RiskClass::Block;
    }

    // Espaçamento disfarçado antes da extensão perigosa (ex.: "arquivo.pdf   .exe")
    if last_raw.starts_with(' ') && (is_dangerous_ext || is_macro_ext) {
        return RiskClass::Block;
    }

    if is_dangerous_ext {
        return RiskClass::Block;
    }

    // Verificação de macros -> Warn
    if is_macro_ext {
        return RiskClass::Warn;
    }

    RiskClass::Safe
}

/// Porque um nome bloqueia: o que o gestor de downloads (downloads-manager)
/// guarda e decide. `Masquerade` e `BadName` nunca têm exceção; os outros
/// podem ser baixados só com «Permitir baixar programas» ligado e uma
/// confirmação por download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockReason {
    /// Vazio, só pontos e espaços, com `:` (ADS), bidi, controlo ou formato
    /// invisível, ou um nome reservado do DOS (`CON`, `NUL.txt`...).
    BadName,
    /// Uma extensão perigosa (ou de macro) depois de uma de fachada, ou
    /// separada por espaços: `fatura.pdf.exe`, `foto.jpg   .scr`.
    Masquerade,
    Program,
    Script,
    /// Atalhos e integrações do shell (`.lnk`, `.url`, `.reg`, `.chm`...).
    Shortcut,
    DiskImage,
    /// Bases e projetos do Access.
    DatabaseApp,
}

impl BlockReason {
    /// Só um programa, um script, um atalho, uma imagem de disco ou uma base
    /// do Access podem ser baixados, e só com a confirmação do utilizador:
    /// um disfarce ou um nome estragado nunca.
    pub fn allows_confirmation(self) -> bool {
        !matches!(self, Self::BadName | Self::Masquerade)
    }
}

/// A razão do `Block` de [`classify_download_name`], pelas mesmas regras e
/// na mesma ordem; `None` para um nome que não bloqueia (`Safe` ou `Warn`).
/// O gate `block_reason_agrees_with_classify_download_name` prende as duas
/// funções uma à outra.
pub fn block_reason(name: &str) -> Option<BlockReason> {
    if name.is_empty()
        || name.contains(':')
        || name
            .chars()
            .any(|c| is_bidi_or_control(c) || is_hidden_format(c))
    {
        return Some(BlockReason::BadName);
    }
    let visible: String = name.chars().filter(|&c| !is_text_joiner(c)).collect();
    let clean = visible.trim_end_matches([' ', '.']);
    if clean.is_empty() {
        return Some(BlockReason::BadName);
    }
    let base_stem = clean
        .split('.')
        .next()
        .unwrap_or(clean)
        .trim()
        .to_ascii_lowercase();
    if WINDOWS_RESERVED_DEVICE_NAMES.contains(&base_stem.as_str()) {
        return Some(BlockReason::BadName);
    }
    extension_reason(clean)
}

/// A parte de [`block_reason`] que olha para a extensão: `clean` já vem sem
/// os joiners e sem os pontos e os espaços do fim. Um programa, um script,
/// um atalho, uma imagem de disco ou uma base do Access, ou um disfarce
/// (uma extensão perigosa ou de macro depois de uma fachada ou de espaços).
fn extension_reason(clean: &str) -> Option<BlockReason> {
    let (stem, last_raw) = clean.rsplit_once('.')?;
    let ext = last_raw.trim().to_ascii_lowercase();
    let ext = ext.as_str();
    let kind = if PROGRAM_EXTENSIONS.contains(&ext) {
        Some(BlockReason::Program)
    } else if SCRIPT_EXTENSIONS.contains(&ext) {
        Some(BlockReason::Script)
    } else if SHORTCUT_SHELL_EXTENSIONS.contains(&ext) {
        Some(BlockReason::Shortcut)
    } else if DISKIMAGE_EXTENSIONS.contains(&ext) {
        Some(BlockReason::DiskImage)
    } else if DATABASE_APP_EXTENSIONS.contains(&ext) {
        Some(BlockReason::DatabaseApp)
    } else {
        None
    };
    let is_macro = MACRO_EXTENSIONS.contains(&ext);
    if (kind.is_some() || is_macro) && (ends_in_decoy(stem) || last_raw.starts_with(' ')) {
        return Some(BlockReason::Masquerade);
    }
    kind
}

// ===================== um ZIP baixado (downloads-zip-inspect) =====================

/// Porque uma entrada torna perigoso o ZIP baixado que a leva.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ZipEntryRisk {
    /// O nome da entrada bloqueia pelas regras de [`block_reason`]: um
    /// programa, um script, um atalho, uma imagem de disco, uma base do
    /// Access, um disfarce (`foto.jpg.exe`) ou um nome com bidi, controlos,
    /// invisíveis ou `:`.
    Blocked(BlockReason),
    /// Outro arquivo compactado ([`ARCHIVE_EXTENSIONS`]): o que ele tem
    /// dentro não se inspeciona.
    NestedArchive,
}

impl ZipEntryRisk {
    /// Como no download direto ([`BlockReason::allows_confirmation`]): com
    /// «Permitir baixar programas» ligado, o ZIP fica -- menos com um
    /// disfarce ou um nome inseguro dentro, que o apagam sempre.
    pub fn allows_confirmation(self) -> bool {
        match self {
            Self::Blocked(reason) => reason.allows_confirmation(),
            Self::NestedArchive => true,
        }
    }

    /// O que nunca se permite pesa mais; um programa pesa mais do que um
    /// arquivo por inspecionar.
    fn severity(self) -> u8 {
        match self {
            Self::Blocked(reason) if !reason.allows_confirmation() => 2,
            Self::Blocked(_) => 1,
            Self::NestedArchive => 0,
        }
    }
}

/// O nome como o Windows o grava: sem os joiners, sem os pontos e os espaços
/// do fim.
fn windows_clean_name(name: &str) -> String {
    let visible: String = name.chars().filter(|&c| !is_text_joiner(c)).collect();
    visible.trim_end_matches([' ', '.']).to_string()
}

/// A extensão final como o Windows a vê: sem os joiners, sem os pontos e os
/// espaços do fim, em minúsculas. `None` sem ponto.
fn final_extension(name: &str) -> Option<String> {
    let clean = windows_clean_name(name);
    let (_, ext) = clean.rsplit_once('.')?;
    Some(ext.trim().to_ascii_lowercase())
}

/// Bidi, controlos, formatos invisíveis ou `:` (um fluxo alternativo): o que
/// faz um nome mentir sobre o que é.
fn has_hostile_chars(name: &str) -> bool {
    name.contains(':')
        || name
            .chars()
            .any(|c| is_bidi_or_control(c) || is_hidden_format(c))
}

/// O risco de uma entrada de um ZIP por um nome dela, tal como o diretório
/// central, o campo Unicode Path ou o cabeçalho local o trazem. Conta o
/// último segmento do caminho (o ZIP separa com
/// `/`, e o Windows também aceita `\`): `pasta/sub/setup.exe` é um
/// programa a qualquer profundidade, `../../x.bat` um script. Um segmento
/// vazio é uma pasta. Com as regras de [`block_reason`] (o disfarce, os
/// pontos e os espaços do fim). Um nome com o radical de um dispositivo do
/// DOS (`aux`, `nul`, `com1`...) não é um nome estragado aqui: o Windows 11
/// cria `aux.exe` ou `nul.bat` como um arquivo qualquer, e um extrator
/// grava-os e corre-os -- por isso esse nome conta pela extensão, como
/// outro qualquer (`aux.exe` é um programa, `prn.pdf.exe` um disfarce,
/// `aux.c` e `con.txt` nada). Os feitos só de pontos e espaços não contam.
/// Depois, outro arquivo compactado ([`ARCHIVE_EXTENSIONS`]).
pub fn zip_entry_risk(entry: &str) -> Option<ZipEntryRisk> {
    let name = entry.rsplit(['/', '\\']).next().unwrap_or(entry);
    if name.is_empty() {
        return None;
    }
    match block_reason(name) {
        Some(BlockReason::BadName) if !has_hostile_chars(name) => {
            if let Some(reason) = extension_reason(&windows_clean_name(name)) {
                return Some(ZipEntryRisk::Blocked(reason));
            }
        }
        Some(reason) => return Some(ZipEntryRisk::Blocked(reason)),
        None => {}
    }
    final_extension(name)
        .filter(|ext| ARCHIVE_EXTENSIONS.contains(&ext.as_str()))
        .map(|_| ZipEntryRisk::NestedArchive)
}

/// O maior nome de entrada que um [`ZipVerdict::Holds`] guarda.
pub const MAX_VERDICT_ENTRY_CHARS: usize = 255;

/// O que a inspeção de um ZIP baixado encontrou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipVerdict {
    /// O diretório central (um só possível) e os cabeçalhos locais foram
    /// lidos inteiros e nenhum nome de nenhuma entrada é perigoso.
    Clean { entries: u64 },
    /// Uma entrada perigosa: o risco mais grave visto (o que nunca se
    /// permite primeiro) e o nome da primeira entrada com ele, cortado a
    /// [`MAX_VERDICT_ENTRY_CHARS`]. Vale mesmo que a listagem falhe
    /// depois: o que se viu, viu-se.
    Holds { risk: ZipEntryRisk, entry: String },
    /// A listagem falhou antes de mostrar uma entrada perigosa (não é um
    /// ZIP, está truncado ou corrompido, ZIP64 incompleto, entradas
    /// sobrepostas, mais de um diretório possível, um cabeçalho local que
    /// não bate com o diretório, acima dos tetos da
    /// [`ZipPolicy::BROWSE_LITE`], erro de leitura): o que o arquivo tem não
    /// se sabe. Nunca «seguro».
    NotInspected,
}

/// Inspeciona um ZIP pelo diretório central e pelos cabeçalhos locais, pela
/// [`safezip::list_central_directory`] com a [`ZipPolicy::BROWSE_LITE`]:
/// cada nome que um extrator pode dar a uma entrada (o do diretório, o do
/// Unicode Path, o do cabeçalho local) é classificado, e o pior vale.
/// Nenhum byte dos dados das entradas é lido, por isso um ZIP de 2 GiB custa
/// o mesmo que um pequeno com as mesmas entradas.
pub fn inspect_zip(reader: impl Read + Seek + Send + 'static) -> ZipVerdict {
    let mut worst: Option<(ZipEntryRisk, String)> = None;
    let listed = safezip::list_central_directory(reader, &ZipPolicy::BROWSE_LITE, |entry| {
        let Some(risk) = zip_entry_risk(entry) else {
            return;
        };
        if worst
            .as_ref()
            .is_none_or(|(seen, _)| risk.severity() > seen.severity())
        {
            worst = Some((risk, entry.chars().take(MAX_VERDICT_ENTRY_CHARS).collect()));
        }
    });
    match (worst, listed) {
        (Some((risk, entry)), _) => ZipVerdict::Holds { risk, entry },
        (None, Ok(entries)) => ZipVerdict::Clean { entries },
        (None, Err(_)) => ZipVerdict::NotInspected,
    }
}

/// [`inspect_zip`] sobre o arquivo no disco; um que não abre, ou que não é
/// um arquivo regular, fica [`ZipVerdict::NotInspected`].
pub fn inspect_zip_file(path: &Path) -> ZipVerdict {
    match File::open(path) {
        Ok(file) if file.metadata().is_ok_and(|meta| meta.is_file()) => inspect_zip(file),
        _ => ZipVerdict::NotInspected,
    }
}

/// Um nome que se inspeciona como ZIP: a extensão final, como o Windows a
/// vê, é `.zip` -- a que o Explorador abre como uma pasta, onde um
/// duplo-clique corre o que estiver dentro.
pub fn is_zip_name(name: &str) -> bool {
    final_extension(name).as_deref() == Some("zip")
}

/// Inspeciona o cabeçalho dos primeiros bytes (até 4 KiB) para identificar perigos.
pub fn sniff_download(bytes: &[u8]) -> SniffRisk {
    let len = bytes.len().min(4096);
    let slice = &bytes[..len];

    // MZ (DOS/Windows Executable)
    if slice.len() >= 2 && slice.starts_with(b"MZ") {
        // Verifica se há o cabeçalho PE em offset 0x3C ou nos primeiros 4 KiB
        if slice.len() >= 0x40 {
            let pe_offset =
                u32::from_le_bytes([slice[0x3C], slice[0x3D], slice[0x3E], slice[0x3F]]) as usize;
            if pe_offset + 4 <= slice.len() && &slice[pe_offset..pe_offset + 4] == b"PE\0\0" {
                return SniffRisk::Dangerous;
            }
        }
        // Mesmo sem PE completo se for pequeno, MZ já é perigoso em download
        return SniffRisk::Dangerous;
    }

    // LNK (Windows Shell Link: tamanho do cabeçalho 0x0000004C e GUID de Shell Link)
    const LNK_GUID_PREFIX: [u8; 16] = [
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    if slice.len() >= 20
        && slice.starts_with(&[0x4C, 0x00, 0x00, 0x00])
        && slice[4..20] == LNK_GUID_PREFIX
    {
        return SniffRisk::Dangerous;
    }

    // CAB (Microsoft Cabinet: magic 'MSCF')
    if slice.len() >= 4 && slice.starts_with(b"MSCF") {
        return SniffRisk::Dangerous;
    }

    // 7-Zip: 37 7A BC AF 27 1C
    if slice.len() >= 6 && slice.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return SniffRisk::NotInspected;
    }

    // RAR: 52 61 72 21 1A 07
    if slice.len() >= 6 && slice.starts_with(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07]) {
        return SniffRisk::NotInspected;
    }

    // Shebang de shell script
    if slice.starts_with(b"#!") {
        return SniffRisk::Warn;
    }

    // Batch script @echo off
    if starts_like_batch(slice) {
        return SniffRisk::Warn;
    }

    SniffRisk::Safe
}

/// `@echo off` no início, como o `cmd.exe` o lê: sem distinguir maiúsculas
/// (`@Echo Off`), depois de um BOM UTF-8 e de espaços, com um ou mais `@`
/// seguidos ou não de espaços (`@ echo off`, `@@echo off`: o `cmd /c` corre-os
/// como `@echo off`) e com um ou mais espaços ou tabs entre `echo` e `off`.
fn starts_like_batch(bytes: &[u8]) -> bool {
    let text = bytes
        .strip_prefix(b"\xEF\xBB\xBF")
        .unwrap_or(bytes)
        .trim_ascii_start();
    // Espaços e tabs da mesma linha.
    fn blanks(line: &[u8]) -> &[u8] {
        let gap = line
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        &line[gap..]
    }
    let Some(mut command) = text.strip_prefix(b"@") else {
        return false;
    };
    while let Some(next) = blanks(command).strip_prefix(b"@") {
        command = next;
    }
    let command = blanks(command);
    let Some(rest) = command
        .get(..4)
        .filter(|head| head.eq_ignore_ascii_case(b"echo"))
        .map(|_| &command[4..])
    else {
        return false;
    };
    let after = rest.trim_ascii_start();
    after.len() < rest.len()
        && after
            .get(..3)
            .is_some_and(|word| word.eq_ignore_ascii_case(b"off"))
}

/// Alvo validado para abertura no aplicativo padrão do sistema.
///
/// Construtor privado: NENHUM caminho bruto pode alcançar `ShellExecuteW('open')`.
/// Só existe através de `default_app_target()`, depois da allowlist, do bloqueio
/// de executáveis/scripts/macros/masquerades e do sniffing dos primeiros 4 KiB
/// do PRÓPRIO arquivo (MZ, LNK, CAB), lidos pelo construtor: quem chama não
/// escolhe os bytes nem pode saltar a leitura. O arquivo pode ainda ser trocado
/// entre o sniffing e o `ShellExecuteW`; quem abre deve fazê-lo logo a seguir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultAppTarget(PathBuf);

impl DefaultAppTarget {
    /// O caminho absoluto ou relativo seguro validado para o alvo.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Tamanho do cabeçalho que `default_app_target` lê do arquivo para o sniffing.
pub const SNIFF_HEAD_BYTES: u64 = 4096;

/// Valida se um arquivo pode ser aberto pelo aplicativo padrão do sistema.
///
/// Recusa programas, scripts, documentos com macro, atalhos do Windows (.url, .lnk, .library-ms, etc.)
/// e arquivos que não pertençam à allowlist de tipos seguros de documentos/mídias. Depois do
/// nome, lê os primeiros [`SNIFF_HEAD_BYTES`] do arquivo e recusa um executável (MZ), um
/// atalho (LNK) ou um gabinete (CAB) com nome de documento. Sem conseguir ler o arquivo (não
/// existe, é uma pasta, sem permissão), recusa: o sniffing é obrigatório.
pub fn default_app_target(path: impl AsRef<Path>) -> Option<DefaultAppTarget> {
    let p = path.as_ref();
    let filename = p.file_name()?.to_str()?;

    // O nome primeiro: a mesma regra que a lista de downloads usa para
    // decidir se mostra «Abrir» (`default_app_name_allowed`).
    if !default_app_name_allowed(filename) {
        return None;
    }

    // Sniff de magic bytes no próprio arquivo: sem cabeçalho lido, sem alvo.
    let head = read_head(p)?;
    if head.starts_with(b"MZ") || matches!(sniff_download(&head), SniffRisk::Dangerous) {
        return None;
    }

    Some(DefaultAppTarget(p.to_path_buf()))
}

/// A metade do nome de [`default_app_target`], sem ler o disco: um nome
/// `Safe` para [`classify_download_name`], que não é um atalho nem um
/// controlo do shell (`.url`, `.lnk`, `.library-ms`, `.search-ms`, `.chm`,
/// `.hta`) e cuja extensão está no [`DEFAULT_APP_ALLOWLIST`]. É o que a
/// lista de downloads (downloads-ui) usa para decidir se oferece «Abrir» ou
/// só «Mostrar na pasta»; abrir continua a exigir o [`DefaultAppTarget`],
/// que relê o arquivo. `name` é só o nome, sem pasta.
pub fn default_app_name_allowed(name: &str) -> bool {
    if classify_download_name(name) != RiskClass::Safe {
        return false;
    }
    // Recusa explicitamente extensões de controle e atalhos de shell
    let Some(ext) = Path::new(name).extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "url" | "lnk" | "library-ms" | "search-ms" | "chm" | "hta"
    ) {
        return false;
    }
    // Exige correspondência estrita com a allowlist
    DEFAULT_APP_ALLOWLIST.contains(&ext.as_str())
}

/// Os primeiros [`SNIFF_HEAD_BYTES`] de um arquivo regular; `None` se não
/// abre, não é um arquivo ou a leitura falha.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut head = Vec::with_capacity(SNIFF_HEAD_BYTES as usize);
    file.take(SNIFF_HEAD_BYTES).read_to_end(&mut head).ok()?;
    Some(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DIR_NONCE: AtomicU64 = AtomicU64::new(1);

    /// Pasta temporária com os arquivos reais que o `default_app_target` lê.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neuralia-file-risk-{name}-{}-{}",
                std::process::id(),
                DIR_NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pe_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 1024];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        bytes[0x3C] = 0x80;
        bytes[128..132].copy_from_slice(b"PE\0\0");
        bytes
    }

    fn lnk_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 100];
        bytes[..4].copy_from_slice(&[0x4C, 0, 0, 0]);
        bytes[4..20].copy_from_slice(&[
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ]);
        bytes
    }

    #[test]
    fn download_name_classification_table_rejects_and_allows_accurately() {
        // Tabela com mais de 60 casos críticos cobrindo todas as categorias exigidas pelo gate:
        // programas, scripts, imagens de disco, atalhos, macros, bidi, trailing dots/spaces,
        // ADS, masquerades e arquivos seguros comuns.
        let cases = [
            // Programas normais (Block)
            ("app.exe", RiskClass::Block),
            ("setup.msi", RiskClass::Block),
            ("patch.msp", RiskClass::Block),
            ("tool.com", RiskClass::Block),
            ("screen.scr", RiskClass::Block),
            ("bundle.msix", RiskClass::Block),
            ("bundle.msixbundle", RiskClass::Block),
            ("app.appx", RiskClass::Block),
            ("app.appxbundle", RiskClass::Block),
            ("installer.appinstaller", RiskClass::Block),
            ("control.cpl", RiskClass::Block),
            ("legacy.pif", RiskClass::Block),
            ("update.msu", RiskClass::Block),
            ("patch.mst", RiskClass::Block),
            ("browser.xbap", RiskClass::Block),
            // Suplemento do Excel: uma DLL nativa, não um documento com macro.
            ("excel.xll", RiskClass::Block),
            ("a.xll", RiskClass::Block),
            // Scripts (Block)
            ("run.bat", RiskClass::Block),
            ("run.cmd", RiskClass::Block),
            ("deploy.ps1", RiskClass::Block),
            ("module.psm1", RiskClass::Block),
            ("data.psd1", RiskClass::Block),
            ("profile.ps1xml", RiskClass::Block),
            ("types.ps2xml", RiskClass::Block),
            ("console.psc1", RiskClass::Block),
            ("console.psc2", RiskClass::Block),
            ("auto.vbs", RiskClass::Block),
            ("encode.vbe", RiskClass::Block),
            ("payload.js", RiskClass::Block),
            ("payload.jse", RiskClass::Block),
            ("script.wsf", RiskClass::Block),
            ("script.wsh", RiskClass::Block),
            ("component.wsc", RiskClass::Block),
            ("scriptlet.sct", RiskClass::Block),
            ("app.hta", RiskClass::Block),
            ("archive.jar", RiskClass::Block),
            ("launch.jnlp", RiskClass::Block),
            ("report.py", RiskClass::Block),
            ("tool.pyw", RiskClass::Block),
            ("bundle.pyz", RiskClass::Block),
            ("bundle.pyzw", RiskClass::Block),
            ("compiled.pyc", RiskClass::Block),
            ("compiled.pyo", RiskClass::Block),
            // Imagens de disco (Block)
            ("system.iso", RiskClass::Block),
            ("disk.img", RiskClass::Block),
            ("virtual.vhd", RiskClass::Block),
            ("virtual.vhdx", RiskClass::Block),
            // Atalhos e integrações shell (Block)
            ("shortcut.lnk", RiskClass::Block),
            ("web.url", RiskClass::Block),
            ("site.website", RiskClass::Block),
            ("ref.appref-ms", RiskClass::Block),
            ("deploy.application", RiskClass::Block),
            ("mini.gadget", RiskClass::Block),
            ("console.msc", RiskClass::Block),
            ("driver.inf", RiskClass::Block),
            ("settings.reg", RiskClass::Block),
            ("command.scf", RiskClass::Block),
            ("help.chm", RiskClass::Block),
            ("legacy.hlp", RiskClass::Block),
            ("diag.diagcab", RiskClass::Block),
            ("folder.library-ms", RiskClass::Block),
            ("query.search-ms", RiskClass::Block),
            ("connector.searchconnector-ms", RiskClass::Block),
            ("settings.settingcontent-ms", RiskClass::Block),
            ("scrap.shb", RiskClass::Block),
            ("scrap.shs", RiskClass::Block),
            ("remote.rdp", RiskClass::Block),
            ("aero.theme", RiskClass::Block),
            ("aero.themepack", RiskClass::Block),
            ("sandbox.wsb", RiskClass::Block),
            // Access (Block)
            ("db.accde", RiskClass::Block),
            ("db.mdb", RiskClass::Block),
            ("db.mde", RiskClass::Block),
            ("db.mda", RiskClass::Block),
            ("project.ade", RiskClass::Block),
            ("project.adp", RiskClass::Block),
            // Macros do Office (Warn)
            ("document.docm", RiskClass::Warn),
            ("template.dotm", RiskClass::Warn),
            ("sheet.xlsm", RiskClass::Warn),
            ("sheet_tpl.xltm", RiskClass::Warn),
            ("sheet_add.xlam", RiskClass::Warn),
            ("presentation.pptm", RiskClass::Warn),
            ("slides.potm", RiskClass::Warn),
            ("addon.ppam", RiskClass::Warn),
            ("slide.sldm", RiskClass::Warn),
            ("orcamento.2024.xlsm", RiskClass::Warn),
            // Masquerade (Block)
            ("invoice.pdf.exe", RiskClass::Block),
            ("invoice.pdf    .exe", RiskClass::Block),
            ("relatorio.docx.bat", RiskClass::Block),
            ("foto.png.vbs", RiskClass::Block),
            ("dados.xlsx.scr", RiskClass::Block),
            ("tabela.csv.ps1", RiskClass::Block),
            ("invoice.pdf.pyw", RiskClass::Block),
            ("invoice.pdf.py", RiskClass::Block),
            ("foto.jpg.rdp", RiskClass::Block),
            ("contrato.pdf.accde", RiskClass::Block),
            // Macro depois de uma extensão de fachada: disfarce, sem exceção.
            ("invoice.pdf.docm", RiskClass::Block),
            ("invoice.pdf    .docm", RiskClass::Block),
            ("planilha.xlsx.xlsm", RiskClass::Block),
            ("foto.jpg.pptm", RiskClass::Block),
            // Bidi injection (Block)
            ("fatura\u{202E}fdp.exe", RiskClass::Block),
            ("document\u{202A}pdf.exe", RiskClass::Block),
            ("test\u{2066}run.cmd", RiskClass::Block),
            ("book\u{200E}.pdf", RiskClass::Block),
            // ADS - Alternate Data Streams (Block)
            ("file.txt:stream", RiskClass::Block),
            ("doc.pdf:malware.exe", RiskClass::Block),
            // Trailing dots e espaços (Block para executáveis com trim Win32)
            ("malware.exe.", RiskClass::Block),
            ("malware.exe...", RiskClass::Block),
            ("malware.exe ", RiskClass::Block),
            ("malware.exe . .", RiskClass::Block),
            ("report.py.", RiskClass::Block),
            // Nomes reservados do Windows (Block)
            ("con.txt", RiskClass::Block),
            ("prn.pdf", RiskClass::Block),
            ("aux", RiskClass::Block),
            ("nul.doc", RiskClass::Block),
            ("com1.png", RiskClass::Block),
            ("lpt1.exe", RiskClass::Block),
            // Variação de maiúsculas/minúsculas (Block)
            ("SETUP.EXE", RiskClass::Block),
            ("InVoIcE.PdF.eXe", RiskClass::Block),
            ("SCRIPT.BAT", RiskClass::Block),
            ("REPORT.PY", RiskClass::Block),
            ("INVOICE.PDF.DOCM", RiskClass::Block),
            ("MACRO.DOCM", RiskClass::Warn),
            // Arquivos seguros permitidos (Safe)
            ("relatorio.pdf", RiskClass::Safe),
            ("livro.epub", RiskClass::Safe),
            ("documento.docx", RiskClass::Safe),
            ("planilha.xlsx", RiskClass::Safe),
            ("imagem.png", RiskClass::Safe),
            ("foto.jpeg", RiskClass::Safe),
            ("musica.mp3", RiskClass::Safe),
            ("video.mp4", RiskClass::Safe),
            ("dados.csv", RiskClass::Safe),
            ("config.json", RiskClass::Safe),
            ("notas.txt", RiskClass::Safe),
            ("README.md", RiskClass::Safe),
        ];

        assert!(
            cases.len() >= 60,
            "A tabela de classificação deve ter pelo menos 60 casos para cobertura completa (tem {})",
            cases.len()
        );

        for (name, expected) in cases {
            let actual = classify_download_name(name);
            assert_eq!(
                actual, expected,
                "Nome '{name}' esperado {:?}, obteve {:?}",
                expected, actual
            );
        }
    }

    #[test]
    fn every_listed_dangerous_type_blocks_alone_and_behind_a_decoy() {
        let lists: [&[&str]; 5] = [
            PROGRAM_EXTENSIONS,
            SCRIPT_EXTENSIONS,
            SHORTCUT_SHELL_EXTENSIONS,
            DISKIMAGE_EXTENSIONS,
            DATABASE_APP_EXTENSIONS,
        ];
        for ext in lists.into_iter().flatten() {
            assert!(!MACRO_EXTENSIONS.contains(ext), "{ext} em duas listas");
            for name in [format!("arquivo.{ext}"), format!("fatura.pdf.{ext}")] {
                assert_eq!(classify_download_name(&name), RiskClass::Block, "{name}");
            }
        }
        for ext in MACRO_EXTENSIONS {
            assert_eq!(
                classify_download_name(&format!("arquivo.{ext}")),
                RiskClass::Warn
            );
            for decoy in SAFE_DECOY_EXTENSIONS {
                let name = format!("arquivo.{decoy}.{ext}");
                assert_eq!(classify_download_name(&name), RiskClass::Block, "{name}");
            }
        }
    }

    /// Os 10 tipos OOXML com macros habilitadas (a lista do Office, escrita
    /// aqui e não lida de `MACRO_EXTENSIONS`: um tipo que falte na lista do
    /// produto fica vermelho) e os binários antigos que também levam macros.
    #[test]
    fn every_macro_enabled_office_type_warns_alone_and_blocks_behind_a_decoy() {
        let ooxml_macro_enabled = [
            "docm", "dotm", "xlsm", "xltm", "xlam", "pptm", "potm", "ppam", "ppsm", "sldm",
        ];
        // `.xlsb` (pasta binária, com VBA), `.xlm` (folha de macros do Excel
        // 4), `.xla`/`.ppa` (suplementos antigos: só existem para levar
        // código) e `.pps` (apresentação antiga que abre direto no modo de
        // exibição).
        let legacy_macro_carriers = ["xlsb", "xlm", "xla", "ppa", "pps"];
        for ext in ooxml_macro_enabled.iter().chain(&legacy_macro_carriers) {
            for (name, expected) in [
                (format!("arquivo.{ext}"), RiskClass::Warn),
                (format!("ARQUIVO.{}", ext.to_uppercase()), RiskClass::Warn),
                (format!("fatura.pdf.{ext}"), RiskClass::Block),
                (format!("foto.jpg   .{ext}"), RiskClass::Block),
            ] {
                assert_eq!(classify_download_name(&name), expected, "{name}");
            }
        }
        assert_eq!(classify_download_name("apresentacao.ppsm"), RiskClass::Warn);
        assert_eq!(classify_download_name("fatura.pdf.ppsm"), RiskClass::Block);
    }

    /// Um documento com macro atrás de uma fachada bloqueia também quando a
    /// fachada está separada por um segmento em branco, por caracteres que
    /// não se veem, por um ponto que não é o ASCII, ou é um tipo que a
    /// própria NeuralIA abre (`.epub`, `.md`) ou uma página (`.html`).
    #[test]
    fn a_macro_behind_a_decoy_blocks_through_blanks_invisibles_and_lookalike_dots() {
        for name in [
            "fatura.pdf. .docm",
            "fatura.pdf..docm",
            "fatura.pdf . . .docm",
            "fatura.pdf\u{200B}.docm",
            "fatura.pdf\u{2060}.docm",
            "fatura.pdf\u{00AD}.docm",
            "fatura.pdf\u{FEFF}.docm",
            "fatura.pdf\u{3164}\u{3164}.docm",
            "fatura.pdf\u{2800}\u{2800}.docm",
            "fatura.pdf\u{200D}.docm",
            "fatura.p\u{200C}df.docm",
            "fatura.pdf\u{FE0F}.docm",
            "fatura\u{FF0E}pdf.docm",
            "fatura\u{2024}pdf.xlsm",
            "fatura\u{FE52}pdf.pptm",
            "fatura\u{0660}pdf.docm",
            "fatura.pdf\u{3000}\u{3000}.docm",
            "livro.epub.docm",
            "nota.md.xlsm",
            "foto.heic.docm",
            "fatura.html.docm",
            "pagina.htm.xlsm",
            "digitalizacao.tif.docm",
            ".pdf.docm",
        ] {
            assert_eq!(classify_download_name(name), RiskClass::Block, "{name:?}");
        }
        // Sem fachada, um documento com macro continua a ser só um aviso.
        for name in [
            "orcamento.2024.xlsm",
            "relatorio.final.docm",
            "ata..docm",
            "pdf.docm",
            "ata\u{200D}.docm",
        ] {
            assert_eq!(classify_download_name(name), RiskClass::Warn, "{name:?}");
        }
    }

    /// Caracteres de formato que não se veem (Default_Ignorable_Code_Point,
    /// como os bidi) bloqueiam o nome; os que o texto legítimo usa (ZWJ e
    /// ZWNJ, seletores de variação dos emoji) não, mas também não separam
    /// nada: a análise corre sem eles.
    #[test]
    fn invisible_format_characters_block_and_text_joiners_do_not() {
        for name in [
            "foto\u{200B}.png",
            "foto.png\u{2060}",
            "rel\u{00AD}atorio.pdf",
            "\u{FEFF}notas.txt",
            "a\u{061C}b.pdf",
            "tag\u{E0041}.pdf",
            "x\u{180E}.pdf",
            "x\u{1BCA0}.pdf",
            "x\u{2800}.pdf",
        ] {
            assert_eq!(classify_download_name(name), RiskClass::Block, "{name:?}");
        }
        for name in [
            "familia \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}.jpg",
            "coracao \u{2764}\u{FE0F}.png",
            "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}.pdf",
        ] {
            assert_eq!(classify_download_name(name), RiskClass::Safe, "{name:?}");
        }
        // Um joiner não disfarça um executável nem quebra a extensão.
        assert_eq!(
            classify_download_name("fatura.pdf.e\u{200D}xe"),
            RiskClass::Block
        );
        assert_eq!(classify_download_name("con\u{200D}.txt"), RiskClass::Block);
    }

    /// Cada tipo que o `DefaultAppTarget` abre é também uma fachada: um
    /// `livro.epub.docm` finge ser um livro que a NeuralIA abriria.
    #[test]
    fn every_default_app_type_is_a_decoy() {
        for ext in DEFAULT_APP_ALLOWLIST {
            assert!(SAFE_DECOY_EXTENSIONS.contains(ext), "{ext}");
            let name = format!("arquivo.{ext}.docm");
            assert_eq!(classify_download_name(&name), RiskClass::Block, "{name}");
        }
    }

    /// No Windows o `.svg` abre no navegador, que corre o `<script>` dele a
    /// partir de `file://` (SVG smuggling): só "Mostrar na pasta".
    #[test]
    fn an_svg_never_reaches_the_default_app() {
        let temp = TempDir::new("svg");
        let svg = temp.file(
            "imagem.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(document.domain)</script></svg>"#,
        );
        assert!(default_app_target(&svg).is_none());
        let plain = temp.file(
            "desenho.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#,
        );
        assert!(default_app_target(&plain).is_none());
        assert!(!DEFAULT_APP_ALLOWLIST.contains(&"svg"));
    }

    #[test]
    fn display_label_marks_invisible_format_characters() {
        assert_eq!(
            display_label("fatura.pdf\u{200B}.docm"),
            "fatura.pdf‹U+200B›.docm"
        );
        assert_eq!(display_label("a\u{2060}b\u{00AD}c"), "a‹U+2060›b‹U+00AD›c");
        // Os joiners dos emoji ficam como estão.
        assert_eq!(
            display_label("\u{2764}\u{FE0F}.png"),
            "\u{2764}\u{FE0F}.png"
        );
    }

    #[test]
    fn sniff_download_identifies_binary_and_script_signatures() {
        // MZ + PE
        assert_eq!(sniff_download(&pe_bytes()), SniffRisk::Dangerous);

        // LNK (Windows Shell Link)
        assert_eq!(sniff_download(&lnk_bytes()), SniffRisk::Dangerous);

        // CAB (MSCF)
        assert_eq!(sniff_download(b"MSCF\0\0\0\0"), SniffRisk::Dangerous);

        // 7z
        assert_eq!(
            sniff_download(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0, 0]),
            SniffRisk::NotInspected
        );

        // RAR
        assert_eq!(
            sniff_download(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0, 0]),
            SniffRisk::NotInspected
        );

        // Shebang
        assert_eq!(sniff_download(b"#!/bin/bash\necho oi"), SniffRisk::Warn);

        // Batch @echo off: o cmd.exe não distingue maiúsculas, e o Bloco de
        // Notas grava um BOM UTF-8 antes.
        for batch in [
            &b"@echo off\ndir"[..],
            b"  @ECHO OFF\r\ncls",
            b"@Echo Off\r\ndir",
            b"@eChO oFf",
            b"\xEF\xBB\xBF@echo off\r\ndir",
            b"\xEF\xBB\xBF  @Echo Off\r\ndir",
            b"\r\n@echo\toff\r\n",
            b"@echo   off",
            // O `cmd.exe` aceita espaços depois do `@` e mais de um `@`
            // (conferido com `cmd /c`: nenhum destes ecoa a linha seguinte).
            b"@ echo off\r\ndir",
            b"@\techo off",
            b" @ Echo  Off\r\n",
            b"@@echo off",
            b"@ @ echo off",
            b"\xEF\xBB\xBF@ echo off",
        ] {
            assert_eq!(sniff_download(batch), SniffRisk::Warn, "{batch:?}");
        }
        for plain in [
            &b"@echoes of a text"[..],
            b"@echo",
            b"@echooff",
            b"echo off",
            b"\xEF\xBB\xBFHello",
            b"@ echoes",
            b"@ ",
            b"@@",
            b"@ e-mail: echo off",
        ] {
            assert_eq!(sniff_download(plain), SniffRisk::Safe, "{plain:?}");
        }

        // Texto plano normal
        assert_eq!(
            sniff_download(b"Hello world! This is plain text."),
            SniffRisk::Safe
        );
    }

    #[test]
    fn default_app_target_table_enforces_strict_allowlist_and_mz_sniffing() {
        let temp = TempDir::new("default-app");

        // Permitidos: o arquivo existe e o cabeçalho é o do tipo.
        for (name, bytes) in [
            ("livro.epub", &b"PK\x03\x04mimetypeapplication/epub+zip"[..]),
            ("manual.pdf", b"%PDF-1.7\n..."),
            ("imagem.png", b"\x89PNG\r\n\x1a\n"),
            ("dados.csv", b"a,b\n1,2\n"),
            ("texto.txt", b"ola"),
            ("documento.docx", b"PK\x03\x04"),
            ("vazio.txt", b""),
        ] {
            let path = temp.file(name, bytes);
            let target = default_app_target(&path);
            assert_eq!(
                target.as_ref().map(DefaultAppTarget::path),
                Some(path.as_path()),
                "{name}"
            );
        }

        // Bloqueados pelo nome (o arquivo existe e é inofensivo).
        for name in [
            "programa.exe",
            "script.ps1",
            "lote.bat",
            "app.hta",
            "link.url",
            "atalho.lnk",
            "ajuda.chm",
            "macro.docm",
            "relatorio.pdf.exe",
            "report.py",
            "sandbox.wsb",
            "remote.rdp",
            "a.xll",
            "invoice.pdf.docm",
            "db.accde",
            "pagina.html",
        ] {
            let path = temp.file(name, b"texto inofensivo");
            assert!(default_app_target(&path).is_none(), "{name}");
        }

        // Nome de documento, conteúdo perigoso: o sniffing do PRÓPRIO arquivo recusa.
        for (name, bytes) in [
            ("falso.pdf", pe_bytes()),
            ("falso_curto.pdf", b"MZ\x90\0\x03\0\0\0".to_vec()),
            ("falso.png", lnk_bytes()),
            ("falso.txt", b"MSCF\0\0\0\0".to_vec()),
        ] {
            let path = temp.file(name, &bytes);
            assert!(default_app_target(&path).is_none(), "{name}");
        }

        // Sem ler o arquivo não há alvo: inexistente, pasta, nome só.
        assert!(default_app_target(temp.0.join("ausente.pdf")).is_none());
        let folder = temp.0.join("pasta.pdf");
        std::fs::create_dir_all(&folder).unwrap();
        assert!(default_app_target(&folder).is_none());
        assert!(default_app_target("falso-inexistente-neuralia.pdf").is_none());
    }

    #[test]
    fn display_label_neutralizes_bidi_and_controls() {
        let label = display_label("fatura\u{202E}fdp.exe");
        assert_eq!(label, "fatura‹RLO›fdp.exe");

        let label_ctrl = display_label("teste\x07aviso");
        assert_eq!(label_ctrl, "teste‹U+0007›aviso");
    }

    // Sabotagens do gate comprovadas como testes de regressão:
    #[test]
    fn sabotage_ps1_must_be_blocked() {
        let temp = TempDir::new("ps1");
        assert_eq!(classify_download_name("script.ps1"), RiskClass::Block);
        assert!(default_app_target(temp.file("script.ps1", b"Write-Host oi")).is_none());
    }

    #[test]
    fn sabotage_trailing_dot_trim_must_block_executable() {
        assert_eq!(classify_download_name("virus.exe."), RiskClass::Block);
    }

    #[test]
    fn sabotage_bidi_must_be_blocked() {
        assert_eq!(
            classify_download_name("safe\u{202e}exe.pdf"),
            RiskClass::Block
        );
    }

    #[test]
    fn sabotage_hta_never_passes_default_app_target() {
        let temp = TempDir::new("hta");
        assert!(default_app_target(temp.file("app.hta", b"<script></script>")).is_none());
    }

    /// Gate (downloads-manager): a razão do bloqueio e a classificação nunca
    /// divergem -- um nome tem razão se e só se `classify_download_name` o
    /// bloqueia --, e cada razão é a certa. O gestor de downloads decide pela
    /// razão (um programa pode ser confirmado, um disfarce nunca).
    #[test]
    fn block_reason_agrees_with_classify_download_name() {
        use BlockReason::*;
        let mut corpus: Vec<(String, Option<BlockReason>)> = Vec::new();
        let lists: [(&[&str], BlockReason); 5] = [
            (PROGRAM_EXTENSIONS, Program),
            (SCRIPT_EXTENSIONS, Script),
            (SHORTCUT_SHELL_EXTENSIONS, Shortcut),
            (DISKIMAGE_EXTENSIONS, DiskImage),
            (DATABASE_APP_EXTENSIONS, DatabaseApp),
        ];
        for (list, reason) in lists {
            for ext in list {
                corpus.push((format!("arquivo.{ext}"), Some(reason)));
                corpus.push((format!("ARQUIVO.{}", ext.to_uppercase()), Some(reason)));
                corpus.push((format!("arquivo.{ext}. . "), Some(reason)));
                corpus.push((format!("fatura.pdf.{ext}"), Some(Masquerade)));
                corpus.push((format!("foto.jpg   .{ext}"), Some(Masquerade)));
                corpus.push((format!("fatura\u{FF0E}pdf.{ext}"), Some(Masquerade)));
            }
        }
        for ext in MACRO_EXTENSIONS {
            corpus.push((format!("planilha.{ext}"), None));
            corpus.push((format!("fatura.pdf.{ext}"), Some(Masquerade)));
            corpus.push((format!("fatura.pdf   .{ext}"), Some(Masquerade)));
        }
        for ext in SAFE_DECOY_EXTENSIONS {
            corpus.push((format!("relatorio.{ext}"), None));
        }
        for (name, reason) in [
            ("", Some(BadName)),
            ("...", Some(BadName)),
            ("   ", Some(BadName)),
            ("a:b.pdf", Some(BadName)),
            ("fatura.pdf:Zone.Identifier", Some(BadName)),
            ("safe\u{202e}exe.pdf", Some(BadName)),
            ("nota\u{200B}.pdf", Some(BadName)),
            ("linha\n.pdf", Some(BadName)),
            ("CON", Some(BadName)),
            ("nul.txt", Some(BadName)),
            ("Com1.pdf", Some(BadName)),
            ("LEIAME", None),
            ("arquivo.tar.gz", None),
            ("setup.exe", Some(Program)),
            ("setup.exe.", Some(Program)),
            ("install.bat", Some(Script)),
            ("atalho.lnk", Some(Shortcut)),
            ("disco.iso", Some(DiskImage)),
            ("base.accde", Some(DatabaseApp)),
            ("fatura.pdf.exe", Some(Masquerade)),
            ("livro.epub.docm", Some(Masquerade)),
            ("rel\u{200D}atorio.pdf", None),
        ] {
            corpus.push((name.to_string(), reason));
        }
        assert!(corpus.len() > 500, "corpus curto: {}", corpus.len());
        for (name, reason) in &corpus {
            assert_eq!(block_reason(name), *reason, "{name:?}");
            assert_eq!(
                block_reason(name).is_some(),
                classify_download_name(name) == RiskClass::Block,
                "{name:?}: razão {:?} e classe {:?} divergem",
                block_reason(name),
                classify_download_name(name)
            );
        }
        for reason in [Program, Script, Shortcut, DiskImage, DatabaseApp] {
            assert!(reason.allows_confirmation(), "{reason:?}");
        }
        assert!(!Masquerade.allows_confirmation());
        assert!(!BadName.allows_confirmation());
    }

    /// Gate (downloads-ui): a metade do nome que a lista de downloads usa
    /// para oferecer «Abrir» e a de `default_app_target` nunca divergem --
    /// com um conteúdo inofensivo no disco, um nome tem alvo se e só se
    /// `default_app_name_allowed` o aceita. Um tipo recusado nunca mostra
    /// «Abrir» (crítica C15), e um que o mostra abre.
    #[test]
    fn default_app_name_allowed_agrees_with_default_app_target() {
        let temp = TempDir::new("name-allowed");
        let mut corpus: Vec<String> = Vec::new();
        let lists: [&[&str]; 8] = [
            PROGRAM_EXTENSIONS,
            SCRIPT_EXTENSIONS,
            SHORTCUT_SHELL_EXTENSIONS,
            DISKIMAGE_EXTENSIONS,
            DATABASE_APP_EXTENSIONS,
            MACRO_EXTENSIONS,
            SAFE_DECOY_EXTENSIONS,
            DEFAULT_APP_ALLOWLIST,
        ];
        for list in lists {
            for ext in list {
                corpus.push(format!("arquivo.{ext}"));
                corpus.push(format!("ARQUIVO.{}", ext.to_uppercase()));
                corpus.push(format!("fatura.pdf.{ext}"));
            }
        }
        corpus.extend(
            [
                "LEIAME",
                "arquivo.tar.gz",
                "imagem.svg",
                "pagina.html",
                "rel\u{200D}atorio.pdf",
                "setup.exe",
                "livro.epub.docm",
            ]
            .map(str::to_string),
        );
        let mut allowed = 0usize;
        for name in &corpus {
            let path = temp.file(name, b"texto inofensivo");
            let target = default_app_target(&path).is_some();
            assert_eq!(
                default_app_name_allowed(name),
                target,
                "{name:?}: o nome e o alvo divergem"
            );
            allowed += usize::from(target);
        }
        assert!(corpus.len() > 400, "corpus curto: {}", corpus.len());
        assert!(allowed >= 2 * DEFAULT_APP_ALLOWLIST.len(), "{allowed}");
        for refused in [
            "macro.docm",
            "app.hta",
            "link.url",
            "atalho.lnk",
            "busca.search-ms",
            "biblioteca.library-ms",
            "ajuda.chm",
            "setup.exe",
            "imagem.svg",
            "pagina.html",
            "arquivo.zip",
            "LEIAME",
        ] {
            assert!(!default_app_name_allowed(refused), "{refused}");
        }
        for offered in ["relatorio.pdf", "foto.PNG", "notas.txt", "musica.mp3"] {
            assert!(default_app_name_allowed(offered), "{offered}");
        }
    }

    // ===================== downloads-zip-inspect =====================

    use crate::epub::test_support::{RawEntry, ZipBuilder, unicode_path_extra};
    use std::io::{Cursor, Seek, SeekFrom};
    use std::sync::{Arc, Mutex};

    fn verdict_of(bytes: Vec<u8>) -> ZipVerdict {
        inspect_zip(Cursor::new(bytes))
    }

    /// O `pacote.zip` do E2E: um programa e um script.
    fn pacote() -> Vec<u8> {
        ZipBuilder::new()
            .stored("setup.exe", &pe_bytes())
            .stored("run.bat", b"@echo off\r\necho oi\r\n")
            .build()
    }

    fn holds(risk: ZipEntryRisk, entry: &str) -> ZipVerdict {
        ZipVerdict::Holds {
            risk,
            entry: entry.to_string(),
        }
    }

    /// Gate critico (entrada nao confiavel; sabotado: saltar os nomes
    /// dentro de pastas; saltar os arquivos compactados dentro; isentar o
    /// radical de um dispositivo do DOS; ignorar o 0x7075 do diretorio; nao
    /// ler o cabecalho local; ignorar o 0x7075 local): a tabela de
    /// classificacao de cada entrada de um ZIP -- o ultimo segmento do
    /// caminho a qualquer profundidade (`/` e `\`), os disfarces, os pontos
    /// e espacos do fim, os nomes inseguros, os radicais do DOS (`aux.exe`)
    /// e os arquivos compactados --, e a mesma tabela por ZIPs reais, com
    /// cada nome que um extrator pode dar a uma entrada: o do diretorio
    /// central, o do Unicode Path (0x7075) e o do cabecalho local.
    #[test]
    fn zip_entry_classification_table() {
        use BlockReason::*;
        use ZipEntryRisk::{Blocked, NestedArchive};
        let rows: &[(&str, Option<ZipEntryRisk>)] = &[
            // Programas e scripts, na raiz e dentro de pastas.
            ("setup.exe", Some(Blocked(Program))),
            ("run.bat", Some(Blocked(Script))),
            ("SETUP.EXE", Some(Blocked(Program))),
            ("pasta/setup.exe", Some(Blocked(Program))),
            ("a/b/c/d/instalar.msi", Some(Blocked(Program))),
            ("scripts/deploy.ps1", Some(Blocked(Script))),
            ("src\\tools\\run.cmd", Some(Blocked(Script))),
            ("mixed/dir\\payload.vbs", Some(Blocked(Script))),
            ("../../evil.exe", Some(Blocked(Program))),
            ("/abs/evil.js", Some(Blocked(Script))),
            ("C:/Windows/evil.bat", Some(Blocked(Script))),
            ("lib/tool.pyw", Some(Blocked(Script))),
            // Atalhos, imagens de disco e bases do Access.
            ("atalho.lnk", Some(Blocked(Shortcut))),
            ("docs/ajuda.url", Some(Blocked(Shortcut))),
            ("imagens/disco.iso", Some(Blocked(DiskImage))),
            ("dados/base.accde", Some(Blocked(DatabaseApp))),
            // Disfarces: uma fachada antes, ou espacos antes da extensao.
            ("foto.jpg.exe", Some(Blocked(Masquerade))),
            ("fotos/ferias/foto.jpg.exe", Some(Blocked(Masquerade))),
            ("fatura.pdf   .scr", Some(Blocked(Masquerade))),
            ("contrato.pdf.docm", Some(Blocked(Masquerade))),
            ("LEIA.txt.vbs", Some(Blocked(Masquerade))),
            // Pontos e espacos no fim (o Windows corta-os).
            ("setup.exe.", Some(Blocked(Program))),
            ("setup.exe...", Some(Blocked(Program))),
            ("setup.exe ", Some(Blocked(Program))),
            ("setup.exe . .", Some(Blocked(Program))),
            ("pasta/run.bat. ", Some(Blocked(Script))),
            // Nomes que mentem: bidi, invisiveis, controlos, fluxo alternativo.
            ("fatura\u{202E}fdp.exe", Some(Blocked(BadName))),
            ("pasta/nota\u{200B}.pdf", Some(Blocked(BadName))),
            ("setup.exe\u{0}.txt", Some(Blocked(BadName))),
            ("leia.txt:evil.exe", Some(Blocked(BadName))),
            // O radical de um dispositivo do DOS nao isenta: o Windows 11
            // cria `aux.exe` e `nul.bat` como arquivos, e um extrator
            // corre-os. Contam pela extensao, como outro nome qualquer.
            ("aux.exe", Some(Blocked(Program))),
            ("nul.bat", Some(Blocked(Script))),
            ("bin/com1.scr", Some(Blocked(Program))),
            ("prn.pdf.exe", Some(Blocked(Masquerade))),
            ("lpt1.lnk", Some(Blocked(Shortcut))),
            ("con.iso", Some(Blocked(DiskImage))),
            ("CON.EXE. ", Some(Blocked(Program))),
            ("pasta\\Aux.Ps1", Some(Blocked(Script))),
            ("nul.zip", Some(NestedArchive)),
            // Outros arquivos compactados, a qualquer profundidade.
            ("inner.zip", Some(NestedArchive)),
            ("inner.7z", Some(NestedArchive)),
            ("inner.rar", Some(NestedArchive)),
            ("backup/inner.ZIP", Some(NestedArchive)),
            ("dados\\pacote.cab", Some(NestedArchive)),
            ("fontes.tar.gz", Some(NestedArchive)),
            ("inner.zip.", Some(NestedArchive)),
            ("inner.zip  ", Some(NestedArchive)),
            ("velho.ace", Some(NestedArchive)),
            ("foto.jpg.zip", Some(NestedArchive)),
            ("inner.z\u{200D}ip", Some(NestedArchive)),
            // O que nao pesa: documentos, imagens, pastas, radicais do DOS
            // com uma extensao inofensiva ou sem nenhuma.
            ("LEIAME.txt", None),
            ("fotos/praia.jpg", None),
            ("docs/manual.pdf", None),
            ("planilhas/orcamento.xlsx", None),
            ("livro.epub", None),
            ("docs/", None),
            ("setup.exe/", None),
            ("inner.zip/", None),
            ("pasta\\", None),
            ("src/aux.c", None),
            ("con.txt", None),
            ("docs/prn.pdf", None),
            ("COM1", None),
            ("nul.", None),
            ("a/..", None),
            ("setup", None),
            ("relatorio.docm", None),
        ];
        for (entry, expected) in rows {
            assert_eq!(zip_entry_risk(entry), *expected, "{entry:?}");
        }

        // Pelos ZIPs reais: o pior risco visto, com o nome da primeira
        // entrada que o tem.
        assert_eq!(verdict_of(pacote()), holds(Blocked(Program), "setup.exe"));
        let nested_only = ZipBuilder::new()
            .stored("LEIAME.txt", b"ola")
            .stored("backup/inner.7z", &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C])
            .build();
        assert_eq!(
            verdict_of(nested_only),
            holds(NestedArchive, "backup/inner.7z")
        );
        let deep = ZipBuilder::new()
            .stored("projeto/", b"")
            .stored("projeto/LEIAME.md", b"# ola")
            .deflated("projeto/bin/Release/app.exe", &pe_bytes())
            .build();
        assert_eq!(
            verdict_of(deep),
            holds(Blocked(Program), "projeto/bin/Release/app.exe")
        );
        // Um disfarce pesa mais do que um programa, e um programa mais do que
        // outro arquivo compactado, venha antes ou depois.
        let mixed = ZipBuilder::new()
            .stored("inner.zip", b"PK\x05\x06")
            .stored("setup.exe", &pe_bytes())
            .stored("fotos/foto.jpg.exe", &pe_bytes())
            .stored("run.bat", b"@echo off")
            .build();
        assert_eq!(
            verdict_of(mixed),
            holds(Blocked(Masquerade), "fotos/foto.jpg.exe")
        );
        // Uma entrada cifrada ou com um metodo que o EPUB recusa lista-se
        // na mesma: o nome esta no diretorio central.
        let mut cipher = RawEntry::stored("secreto/setup.exe", b"xx");
        cipher.flags = 1;
        let mut lzma = RawEntry::stored("run.bat", b"xx");
        lzma.method = 14;
        lzma.local_method = Some(14);
        assert_eq!(
            verdict_of(ZipBuilder::new().entry(cipher).build()),
            holds(Blocked(Program), "secreto/setup.exe")
        );
        assert_eq!(
            verdict_of(ZipBuilder::new().entry(lzma).build()),
            holds(Blocked(Script), "run.bat")
        );
        // O radical de um dispositivo do DOS, por um ZIP real (o que o
        // Windows 11 grava e o tar.exe corre).
        for (device, risk) in [
            ("aux.exe", Blocked(Program)),
            ("nul.bat", Blocked(Script)),
            ("bin/com1.scr", Blocked(Program)),
            ("prn.pdf.exe", Blocked(Masquerade)),
            ("lpt1.lnk", Blocked(Shortcut)),
            ("con.iso", Blocked(DiskImage)),
        ] {
            let zip = ZipBuilder::new()
                .stored("LEIAME.txt", b"ola")
                .stored(device, b"MZ")
                .build();
            assert_eq!(verdict_of(zip), holds(risk, device), "{device}");
        }
        // Os outros nomes de uma entrada: o do campo Unicode Path (0x7075)
        // do diretorio (o 7-Zip e o bsdtar extraem `setup.exe` de um
        // `foto.jpg`), o do cabecalho local (o tar.exe extrai por ele) e o
        // 0x7075 do extra local.
        let mut unicode = RawEntry::stored("fotos/foto.jpg", b"MZ");
        unicode.extra = unicode_path_extra(b"fotos/foto.jpg", "fotos/setup.exe");
        let mut local = RawEntry::stored("foto.jpg", b"MZ");
        local.local_name = Some(b"setup.exe".to_vec());
        let mut local_unicode = RawEntry::stored("foto.jpg", b"MZ");
        local_unicode.local_extra = unicode_path_extra(b"foto.jpg", "run.bat");
        for (entry, expected) in [
            (unicode, holds(Blocked(Program), "fotos/setup.exe")),
            (local, holds(Blocked(Program), "setup.exe")),
            (local_unicode, holds(Blocked(Script), "run.bat")),
        ] {
            let what = String::from_utf8_lossy(&entry.name).into_owned();
            let classic = ZipBuilder::new()
                .stored("LEIAME.txt", b"ola")
                .entry(entry.clone())
                .build();
            assert_eq!(verdict_of(classic), expected, "{what}");
            let zip64 = ZipBuilder::new()
                .stored("LEIAME.txt", b"ola")
                .entry(entry)
                .zip64()
                .build();
            assert_eq!(verdict_of(zip64), expected, "{what} (ZIP64)");
        }
        // Um 0x7075 legitimo (o nome em CP437 no cru, em UTF-8 no extra)
        // nao pesa: fica limpo.
        let mut accented = RawEntry::stored("ferias.txt", b"ola");
        accented.name = b"f\x82rias.txt".to_vec();
        accented.extra = unicode_path_extra(b"f\x82rias.txt", "f\u{e9}rias.txt");
        accented.local_extra = accented.extra.clone();
        assert_eq!(
            verdict_of(ZipBuilder::new().entry(accented).build()),
            ZipVerdict::Clean { entries: 1 }
        );
        // Um nome que o EPUB recusa (`..`, `\`) tambem se classifica.
        let slip = ZipBuilder::new()
            .stored("../../Startup/evil.bat", b"@echo off")
            .build();
        assert_eq!(
            verdict_of(slip),
            holds(Blocked(Script), "../../Startup/evil.bat")
        );
        // Limpo: fotos, documentos e pastas.
        let clean = ZipBuilder::new()
            .stored("fotos/", b"")
            .stored("fotos/praia.jpg", b"\xFF\xD8\xFF\xE0")
            .deflated("fotos/LEIAME.txt", b"ferias de 2026")
            .stored("src/aux.c", b"int main(){}")
            .build();
        assert_eq!(verdict_of(clean), ZipVerdict::Clean { entries: 4 });
        assert_eq!(
            verdict_of(ZipBuilder::new().build()),
            ZipVerdict::Clean { entries: 0 }
        );
        // ZIP64 valido e inspecionado como o classico.
        assert_eq!(
            verdict_of(
                ZipBuilder::new()
                    .stored("LEIAME.txt", b"ola")
                    .stored("bin/setup.exe", &pe_bytes())
                    .zip64()
                    .build()
            ),
            holds(Blocked(Program), "bin/setup.exe")
        );
        // Um nome enorme guarda-se cortado.
        let long = format!("{}/setup.exe", "p".repeat(600));
        match verdict_of(ZipBuilder::new().stored(&long, b"x").build()) {
            ZipVerdict::Holds { entry, .. } => {
                assert_eq!(entry.chars().count(), MAX_VERDICT_ENTRY_CHARS)
            }
            other => panic!("{other:?}"),
        }
    }

    /// Um ZIP enorme que so existe em memoria nos cabecalhos locais e no
    /// fim: os dados das entradas sao zeros virtuais (nunca materializados),
    /// e cada leitura fica registada (offset, bytes).
    struct SparseZip {
        len: u64,
        /// Os bytes que existem, por ordem e sem sobreposicao: cada cabecalho
        /// local e o fim (diretorio central e registros de fim).
        segments: Vec<(u64, Vec<u8>)>,
        pos: u64,
        reads: Arc<Mutex<Vec<(u64, u64)>>>,
    }

    impl Read for SparseZip {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let left = self.len.saturating_sub(self.pos);
            let n = (buf.len() as u64).min(left) as usize;
            if n == 0 {
                return Ok(0);
            }
            let (from, to) = (self.pos, self.pos + n as u64);
            buf[..n].fill(0);
            for (at, bytes) in &self.segments {
                let end = at + bytes.len() as u64;
                if end <= from || *at >= to {
                    continue;
                }
                let lo = from.max(*at);
                let hi = to.min(end);
                buf[(lo - from) as usize..(hi - from) as usize]
                    .copy_from_slice(&bytes[(lo - at) as usize..(hi - at) as usize]);
            }
            self.reads.lock().unwrap().push((self.pos, n as u64));
            self.pos = to;
            Ok(n)
        }
    }

    impl Seek for SparseZip {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            let next = match to {
                SeekFrom::Start(at) => Some(at),
                SeekFrom::End(delta) => self.len.checked_add_signed(delta),
                SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
            };
            self.pos = next.ok_or_else(|| std::io::Error::other("seek negativo"))?;
            Ok(self.pos)
        }
    }

    fn push16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn push32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn push64(out: &mut Vec<u8>, value: u64) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    /// Onde esta cada parte de um [`SparseZip`].
    struct SparseLayout {
        directory_offset: u64,
        directory_size: u64,
        /// `[inicio, fim)` de cada cabecalho local (30 bytes, nome e extra).
        local_headers: Vec<(u64, u64)>,
    }

    /// Um ZIP de entradas *stored* com `sizes` bytes cada, com os cabecalhos
    /// locais e sem os dados: devolve o leitor e onde esta cada parte.
    fn sparse_zip(entries: &[(&str, u64)], zip64: bool) -> (SparseZip, SparseLayout) {
        let mut offset = 0u64;
        let mut central = Vec::new();
        let mut segments = Vec::new();
        let mut local_headers = Vec::new();
        let sized = |value: u64| {
            if zip64 {
                u32::MAX
            } else {
                u32::try_from(value).expect("cabe em 32 bits")
            }
        };
        for (name, size) in entries {
            let mut local_extra = Vec::new();
            let mut extra = Vec::new();
            if zip64 {
                push16(&mut local_extra, 1);
                push16(&mut local_extra, 16);
                push64(&mut local_extra, *size);
                push64(&mut local_extra, *size);
                push16(&mut extra, 1);
                push16(&mut extra, 24);
                push64(&mut extra, *size);
                push64(&mut extra, *size);
                push64(&mut extra, offset);
            }
            let mut local = Vec::new();
            push32(&mut local, 0x0403_4b50);
            push16(&mut local, if zip64 { 45 } else { 20 });
            push16(&mut local, 0);
            push16(&mut local, 0);
            push32(&mut local, 0);
            push32(&mut local, 0);
            push32(&mut local, sized(*size));
            push32(&mut local, sized(*size));
            push16(&mut local, name.len() as u16);
            push16(&mut local, local_extra.len() as u16);
            local.extend_from_slice(name.as_bytes());
            local.extend_from_slice(&local_extra);
            let local_len = local.len() as u64;
            local_headers.push((offset, offset + local_len));
            segments.push((offset, local));

            push32(&mut central, 0x0201_4b50);
            push16(&mut central, 0x031E);
            push16(&mut central, if zip64 { 45 } else { 20 });
            push16(&mut central, 0);
            push16(&mut central, 0);
            push32(&mut central, 0);
            push32(&mut central, 0);
            push32(&mut central, sized(*size));
            push32(&mut central, sized(*size));
            push16(&mut central, name.len() as u16);
            push16(&mut central, extra.len() as u16);
            push16(&mut central, 0);
            push16(&mut central, 0);
            push16(&mut central, 0);
            push32(&mut central, 0);
            push32(&mut central, sized(offset));
            central.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&extra);
            offset += local_len + size;
        }
        let directory_offset = offset;
        let directory_size = central.len() as u64;
        let mut tail = central;
        let count = entries.len() as u64;
        if zip64 {
            let record = directory_offset + tail.len() as u64;
            push32(&mut tail, 0x0606_4b50);
            push64(&mut tail, 44);
            push16(&mut tail, 45);
            push16(&mut tail, 45);
            push32(&mut tail, 0);
            push32(&mut tail, 0);
            push64(&mut tail, count);
            push64(&mut tail, count);
            push64(&mut tail, directory_size);
            push64(&mut tail, directory_offset);
            push32(&mut tail, 0x0706_4b50);
            push32(&mut tail, 0);
            push64(&mut tail, record);
            push32(&mut tail, 1);
        }
        push32(&mut tail, 0x0605_4b50);
        push16(&mut tail, 0);
        push16(&mut tail, 0);
        let short = if zip64 { u16::MAX } else { count as u16 };
        push16(&mut tail, short);
        push16(&mut tail, short);
        push32(
            &mut tail,
            if zip64 {
                u32::MAX
            } else {
                directory_size as u32
            },
        );
        push32(
            &mut tail,
            if zip64 {
                u32::MAX
            } else {
                directory_offset as u32
            },
        );
        push16(&mut tail, 0);
        let len = directory_offset + tail.len() as u64;
        segments.push((directory_offset, tail));
        (
            SparseZip {
                len,
                segments,
                pos: 0,
                reads: Arc::default(),
            },
            SparseLayout {
                directory_offset,
                directory_size,
                local_headers,
            },
        )
    }

    /// O que uma inspecao leu: o veredito, as leituras, o fim do arquivo e
    /// onde esta cada parte.
    fn inspect_sparse(
        entries: &[(&str, u64)],
        zip64: bool,
    ) -> (ZipVerdict, Vec<(u64, u64)>, u64, SparseLayout) {
        let (zip, layout) = sparse_zip(entries, zip64);
        let reads = Arc::clone(&zip.reads);
        let len = zip.len;
        let verdict = inspect_zip(zip);
        let reads = reads.lock().unwrap().clone();
        assert!(len > layout.directory_offset + layout.directory_size);
        (verdict, reads, len, layout)
    }

    /// Gate critico (entrada nao confiavel; sabotado: ler os dados das
    /// entradas): um ZIP legitimo de mais de 2 GiB (e um ZIP64 de 6 GiB),
    /// que o EPUB recusaria pelos tetos de tamanho, e inspecionado sem ler
    /// os dados de nenhuma entrada -- sem arquivo nenhum no disco, com um
    /// leitor que conta. A relacao: de 1 MiB a GiB por entrada, as leituras
    /// sao exatamente as mesmas; cada leitura cai no fim do arquivo (a busca do
    /// registro de fim, 64 KiB + 22), no diretorio central ou dentro de um
    /// cabecalho local (30 bytes, o nome e o extra: o `tar.exe` extrai pelo
    /// nome dele); e o total nunca passa do fim, do diretorio e dos
    /// cabecalhos locais.
    #[test]
    fn a_big_zip_is_inspected_without_reading_entry_bodies() {
        const MIB: u64 = 1024 * 1024;
        const GIB: u64 = 1024 * MIB;
        const TAIL_WINDOW: u64 = 22 + 0xFFFF;
        // O classico cabe em 32 bits: de 1 MiB a mais de 1 GiB por entrada
        // (o arquivo passa de 2 GiB). O ZIP64 vai de 1 MiB a 6 GiB por
        // entrada (mais de 12 GiB).
        for (zip64, bodies) in [
            (false, vec![MIB, 64 * MIB, GIB + 4 * MIB]),
            (true, vec![MIB, 3 * GIB, 6 * GIB]),
        ] {
            let mut measured = Vec::new();
            let mut biggest = 0;
            for body in bodies {
                for (entries, expected) in [
                    (
                        [
                            ("videos/ferias.mp4", body),
                            ("backup/dados.bin", body),
                            ("instalar/LEIAME.md", 4096),
                        ],
                        ZipVerdict::Clean { entries: 3 },
                    ),
                    (
                        [
                            ("videos/ferias.mp4", body),
                            ("backup/dados.bin", body),
                            ("instalar/setup.exe", 4096),
                        ],
                        holds(
                            ZipEntryRisk::Blocked(BlockReason::Program),
                            "instalar/setup.exe",
                        ),
                    ),
                ] {
                    let (verdict, reads, len, layout) = inspect_sparse(&entries, zip64);
                    assert_eq!(verdict, expected, "zip64={zip64}, {body} bytes por entrada");
                    biggest = biggest.max(len);
                    // Cada leitura cai na janela do fim, no diretorio ou
                    // dentro de um cabecalho local.
                    let floor = layout.directory_offset.min(len.saturating_sub(TAIL_WINDOW));
                    for &(at, n) in &reads {
                        let in_tail = at >= floor && at + n <= len;
                        let in_local = layout
                            .local_headers
                            .iter()
                            .any(|&(start, end)| at >= start && at + n <= end);
                        assert!(
                            in_tail || in_local,
                            "zip64={zip64}, {body} bytes por entrada: leitura em {at}..{} fora \
                             do fim ({floor}..{len}) e dos cabecalhos locais {:?} -- leu os \
                             dados de uma entrada",
                            at + n,
                            layout.local_headers
                        );
                    }
                    // Cada cabecalho local e lido (o nome dele conta).
                    for &(start, _) in &layout.local_headers {
                        assert!(
                            reads.iter().any(|&(at, _)| at == start),
                            "zip64={zip64}: o cabecalho local em {start} nao foi lido"
                        );
                    }
                    let locals: u64 = layout
                        .local_headers
                        .iter()
                        .map(|&(start, end)| end - start)
                        .sum();
                    let total: u64 = reads.iter().map(|&(_, n)| n).sum();
                    assert!(
                        total <= TAIL_WINDOW + 20 + (len - layout.directory_offset) + locals,
                        "zip64={zip64}: {total} bytes lidos"
                    );
                    measured.push(total);
                }
            }
            assert!(biggest > 2 * GIB, "zip64={zip64}: so {biggest} bytes");
            // A relacao: de 1 MiB a GiB por entrada, as mesmas leituras.
            assert!(
                measured.windows(2).all(|pair| pair[0] == pair[1]),
                "zip64={zip64}: as leituras mudaram com o tamanho dos dados: {measured:?}"
            );
        }
    }

    /// Um leitor que falha a meio (um disco que desaparece).
    struct FailingReader {
        inner: Cursor<Vec<u8>>,
        fail_after: u64,
    }

    impl Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.inner.position() >= self.fail_after {
                return Err(std::io::Error::other("o disco foi-se"));
            }
            self.inner.read(buf)
        }
    }

    impl Seek for FailingReader {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    /// `(entradas, offset, tamanho)` do diretorio central de um ZIP classico
    /// do `ZipBuilder` (sem comentario no fim), e onde esta o registro de fim.
    fn classic_directory(zip: &[u8]) -> (usize, usize, usize, usize) {
        let eocd = zip.len() - 22;
        let le16 = |at: usize| u16::from_le_bytes([zip[at], zip[at + 1]]) as usize;
        let le32 = |at: usize| {
            u32::from_le_bytes([zip[at], zip[at + 1], zip[at + 2], zip[at + 3]]) as usize
        };
        assert_eq!(le32(eocd), 0x0605_4b50);
        (le16(eocd + 10), le32(eocd + 16), le32(eocd + 12), eocd)
    }

    /// Gate critico (entrada nao confiavel; sabotado: um ZIP nao inspecionado
    /// dado como limpo; e, cada uma sozinha, tirar as regras do diretorio
    /// unico -- a ultima assinatura, o diretorio colado ao fim, os campos
    /// classicos iguais aos do ZIP64, o registro ZIP64 colado ao localizador
    /// -- e a conferencia do cabecalho local): cada ZIP estragado, ZIP64
    /// incompleto, com entradas sobrepostas, com mais de um diretorio
    /// possivel ou com um cabecalho local que nao bate -- todos so com nomes
    /// inofensivos no que se le -- fica `NotInspected`, nunca `Clean`, e
    /// nunca panica.
    #[test]
    fn corrupt_zip64_and_overlap_zips_are_not_inspected() {
        use crate::epub::test_support::{
            classic_disagrees_with_zip64_zip, directory_gap_zip, two_end_records_zip,
            zip64_record_gap_zip,
        };
        let benign = || {
            ZipBuilder::new()
                .stored("fotos/praia.jpg", b"\xFF\xD8\xFF\xE0")
                .deflated("LEIAME.txt", b"ferias de 2026")
        };
        let good = benign().build();
        assert_eq!(verdict_of(good.clone()), ZipVerdict::Clean { entries: 2 });
        let good64 = benign().zip64().build();
        assert_eq!(verdict_of(good64.clone()), ZipVerdict::Clean { entries: 2 });
        let (entries, cd_offset, cd_size, eocd) = classic_directory(&good);
        assert_eq!(entries, 2);

        let mut fixtures: Vec<(&str, Vec<u8>)> = vec![
            ("vazio", Vec::new()),
            ("so PK", b"PK\x03\x04".to_vec()),
            ("HTML", b"<!doctype html><title>404</title>".to_vec()),
            ("sem o fim", good[..good.len() - 1].to_vec()),
            ("metade", good[..good.len() / 2].to_vec()),
        ];
        let mut bad_sig = good.clone();
        bad_sig[cd_offset] ^= 0xFF;
        fixtures.push(("assinatura do diretorio", bad_sig));
        let mut more = good.clone();
        more[eocd + 8] = 3;
        more[eocd + 10] = 3;
        fixtures.push(("mais entradas do que o diretorio tem", more));
        let mut less = good.clone();
        less[eocd + 8] = 1;
        less[eocd + 10] = 1;
        fixtures.push(("bytes a mais no fim do diretorio", less));
        let mut outside = good.clone();
        outside[eocd + 12..eocd + 16].copy_from_slice(&((cd_size + 64) as u32).to_le_bytes());
        fixtures.push(("diretorio fora do arquivo", outside));
        let mut volumes = good.clone();
        volumes[eocd + 4] = 1;
        fixtures.push(("volume 1", volumes));
        let mut start_disk = good.clone();
        start_disk[cd_offset + 34] = 1;
        fixtures.push(("entrada noutro volume", start_disk));

        // ZIP64: campos saturados sem o registro de fim ZIP64.
        let mut saturated = good.clone();
        saturated[eocd + 8..eocd + 12].copy_from_slice(&[0xFF; 4]);
        saturated[eocd + 12..eocd + 20].copy_from_slice(&[0xFF; 8]);
        fixtures.push(("ZIP64 sem o registro de fim", saturated));
        // ZIP64: entrada saturada sem o extra ZIP64.
        let mut no_extra = good.clone();
        no_extra[cd_offset + 24..cd_offset + 28].copy_from_slice(&[0xFF; 4]);
        fixtures.push(("ZIP64 sem o extra da entrada", no_extra));
        // ZIP64: o localizador diz dois discos; o registro sem assinatura;
        // o registro fora do lugar; contagens que nao batem.
        let eocd64 = good64.len() - 22;
        let locator = eocd64 - 20;
        let record = locator - 56;
        let mut disks = good64.clone();
        disks[locator + 16] = 2;
        fixtures.push(("ZIP64 em dois discos", disks));
        let mut record_sig = good64.clone();
        record_sig[record] ^= 0xFF;
        fixtures.push(("ZIP64 sem assinatura", record_sig));
        let mut misplaced = good64.clone();
        misplaced[locator + 8..locator + 16].copy_from_slice(&(locator as u64).to_le_bytes());
        fixtures.push(("ZIP64 fora do lugar", misplaced));
        let mut counts = good64.clone();
        counts[record + 24] = 7;
        fixtures.push(("ZIP64 com contagens diferentes", counts));
        // ZIP64: mais entradas do que o teto da BROWSE_LITE.
        let mut too_many = good64.clone();
        let over = ZipPolicy::BROWSE_LITE.max_entries + 1;
        too_many[record + 24..record + 32].copy_from_slice(&over.to_le_bytes());
        too_many[record + 32..record + 40].copy_from_slice(&over.to_le_bytes());
        fixtures.push(("acima do teto de entradas", too_many));
        let mut too_big = good64.clone();
        let size = ZipPolicy::BROWSE_LITE.max_directory_size + 1;
        too_big[record + 40..record + 48].copy_from_slice(&size.to_le_bytes());
        fixtures.push(("acima do teto do diretorio", too_big));

        // Sobreposicao: duas entradas no mesmo cabecalho local; uma que
        // comeca dentro dos dados da outra; uma que invade o diretorio.
        let mut same = RawEntry::stored("fotos/b.jpg", b"\xFF\xD8\xFF\xE0");
        same.offset = Some(0);
        same.central_only = true;
        fixtures.push((
            "sobreposicao no mesmo cabecalho",
            ZipBuilder::new()
                .stored("fotos/a.jpg", b"\xFF\xD8\xFF\xE0")
                .entry(same.clone())
                .build(),
        ));
        fixtures.push((
            "sobreposicao ZIP64",
            ZipBuilder::new()
                .stored("fotos/a.jpg", b"\xFF\xD8\xFF\xE0")
                .entry(same)
                .zip64()
                .build(),
        ));
        let mut inside = RawEntry::stored("fotos/b.jpg", b"\xFF\xD8\xFF\xE0");
        inside.offset = Some(40);
        inside.central_only = true;
        fixtures.push((
            "entrada dentro dos dados de outra",
            ZipBuilder::new()
                .stored("fotos/a.jpg", &[0x55; 64])
                .entry(inside)
                .build(),
        ));
        let mut invades = RawEntry::stored("LEIAME.txt", b"ola");
        invades.compressed = 4096;
        fixtures.push((
            "dados que invadem o diretorio",
            ZipBuilder::new().entry(invades).build(),
        ));

        // Mais de um diretorio possivel: o que se le so tem o LEIAME.txt, e
        // outro leitor lista o setup.exe do outro (7-Zip, .NET, bsdtar).
        fixtures.push(("dois registros de fim", two_end_records_zip()));
        fixtures.push((
            "bytes entre o diretorio e o registro de fim",
            directory_gap_zip(),
        ));
        fixtures.push((
            "registro classico diferente do ZIP64",
            classic_disagrees_with_zip64_zip(),
        ));
        fixtures.push((
            "outro registro ZIP64 colado ao localizador",
            zip64_record_gap_zip(),
        ));
        // O cabecalho local nao bate com o diretorio (o tar.exe extrai pelo
        // nome local): sem assinatura, ou com outro nome inofensivo.
        let mut no_local = good.clone();
        no_local[0] ^= 0xFF;
        fixtures.push(("cabecalho local sem assinatura", no_local));
        let mut renamed = RawEntry::stored("fotos/a.jpg", b"\xFF\xD8\xFF\xE0");
        renamed.local_name = Some(b"fotos/b.jpg".to_vec());
        fixtures.push((
            "nome local diferente",
            ZipBuilder::new().entry(renamed).build(),
        ));
        let mut longer = RawEntry::stored("a.jpg", b"\xFF\xD8\xFF\xE0");
        longer.local_name = Some(b"fotos/outra.jpg".to_vec());
        fixtures.push((
            "nome local de outro tamanho",
            ZipBuilder::new().entry(longer).build(),
        ));

        for (what, bytes) in fixtures {
            let verdict = std::panic::catch_unwind(|| verdict_of(bytes))
                .unwrap_or_else(|_| panic!("{what}: a inspecao panicou"));
            assert_eq!(verdict, ZipVerdict::NotInspected, "{what}");
        }

        // Um erro de leitura (o disco que desaparece) tambem.
        let failing = FailingReader {
            inner: Cursor::new(good.clone()),
            fail_after: cd_offset as u64 + 10,
        };
        assert_eq!(inspect_zip(failing), ZipVerdict::NotInspected);
        // E um arquivo que nao existe ou que e uma pasta.
        let temp = TempDir::new("zip-missing");
        assert_eq!(
            inspect_zip_file(&temp.0.join("sumiu.zip")),
            ZipVerdict::NotInspected
        );
        assert_eq!(inspect_zip_file(&temp.0), ZipVerdict::NotInspected);
        assert_eq!(
            inspect_zip_file(&temp.file("fotos.zip", &good)),
            ZipVerdict::Clean { entries: 2 }
        );
    }

    /// O `seed` classico com o diretorio central cortado em `keep` bytes e
    /// um registro de fim novo que o declara desse tamanho: o corte chega ao
    /// parser do diretorio.
    fn with_directory_cut(seed: &[u8], keep: usize) -> Vec<u8> {
        let (entries, offset, _, _) = classic_directory(seed);
        let mut out = seed[..offset + keep].to_vec();
        push32(&mut out, 0x0605_4b50);
        push16(&mut out, 0);
        push16(&mut out, 0);
        push16(&mut out, entries as u16);
        push16(&mut out, entries as u16);
        push32(&mut out, keep as u32);
        push32(&mut out, offset as u32);
        push16(&mut out, 0);
        out
    }

    /// Gate critico (entrada nao confiavel; o harness de mutacao): um ZIP
    /// com um programa e um script (e um documento antes), classico e ZIP64,
    /// cortado em cada comprimento, com o diretorio central cortado em cada
    /// byte (com um fim que o declara), e com cada bit de cada byte trocado
    /// e cada byte posto a 0x00 e a 0xFF: nunca panica, e nunca sai
    /// `Clean` -- ou viu uma entrada perigosa, ou fica nao inspecionado.
    /// Uma mutacao so mexe num nome, por isso sobra sempre outro perigoso.
    #[test]
    fn zip_mutation_harness_never_panics_and_never_comes_out_clean() {
        let seed = |zip64: bool| {
            let mut noted = RawEntry::deflated("docs/LEIAME.txt", b"leia antes de instalar");
            noted.extra = vec![0xCA, 0xFE, 4, 0, 1, 2, 3, 4];
            noted.comment = b"comentario".to_vec();
            let builder = ZipBuilder::new()
                .entry(noted)
                .stored("setup.exe", &pe_bytes()[..64])
                .stored("scripts/run.bat", b"@echo off\r\n");
            if zip64 { builder.zip64() } else { builder }.build()
        };
        let check = |bytes: Vec<u8>, what: &str| {
            let verdict = std::panic::catch_unwind(|| verdict_of(bytes))
                .unwrap_or_else(|_| panic!("{what}: a inspecao panicou"));
            assert!(
                !matches!(verdict, ZipVerdict::Clean { .. }),
                "{what}: saiu limpo ({verdict:?})"
            );
            verdict
        };
        let mut cases = 0usize;
        let mut not_inspected = 0usize;
        for zip64 in [false, true] {
            let seed = seed(zip64);
            assert!(matches!(verdict_of(seed.clone()), ZipVerdict::Holds { .. }));
            for len in 0..seed.len() {
                let verdict = check(
                    seed[..len].to_vec(),
                    &format!("zip64={zip64}, corte em {len}"),
                );
                not_inspected += usize::from(verdict == ZipVerdict::NotInspected);
                cases += 1;
            }
            for at in 0..seed.len() {
                for bit in 0..8 {
                    let mut bytes = seed.clone();
                    bytes[at] ^= 1 << bit;
                    check(bytes, &format!("zip64={zip64}, bit {bit} do byte {at}"));
                    cases += 1;
                }
                for value in [0x00, 0xFF] {
                    let mut bytes = seed.clone();
                    bytes[at] = value;
                    check(bytes, &format!("zip64={zip64}, byte {at} = {value:#04x}"));
                    cases += 1;
                }
            }
        }
        // Os cortes do diretorio central chegam ao parser dele.
        let classic = seed(false);
        let (_, _, size, _) = classic_directory(&classic);
        assert!(matches!(
            verdict_of(with_directory_cut(&classic, size)),
            ZipVerdict::Holds { .. }
        ));
        for keep in 0..size {
            check(
                with_directory_cut(&classic, keep),
                &format!("diretorio cortado em {keep} de {size}"),
            );
            cases += 1;
        }
        assert!(cases > 5_000, "{cases} casos");
        assert!(not_inspected > 0);
    }
}
