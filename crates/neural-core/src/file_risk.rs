//! Classificação portável de segurança de arquivos e nomes de download.
//!
//! Usado pelo subsistema de downloads e pelo explorador de arquivos para impedir
//! execução acidental de programas, scripts, imagens de disco, atalhos do Windows
//! e arquivos mascarados com extensões falsas, caracteres bidi ou invisíveis.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

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

    // Verifica classificação geral do nome
    match classify_download_name(filename) {
        RiskClass::Block | RiskClass::Warn => return None,
        RiskClass::Safe => {}
    }

    // Recusa explicitamente extensões de controle e atalhos de shell
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "url" | "lnk" | "library-ms" | "search-ms" | "chm" | "hta"
    ) {
        return None;
    }

    // Exige correspondência estrita com a allowlist
    if !DEFAULT_APP_ALLOWLIST.contains(&ext.as_str()) {
        return None;
    }

    // Sniff de magic bytes no próprio arquivo: sem cabeçalho lido, sem alvo.
    let head = read_head(p)?;
    if head.starts_with(b"MZ") || matches!(sniff_download(&head), SniffRisk::Dangerous) {
        return None;
    }

    Some(DefaultAppTarget(p.to_path_buf()))
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
}
