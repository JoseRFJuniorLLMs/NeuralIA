//! Classificação portável de segurança de arquivos e nomes de download.
//!
//! Usado pelo subsistema de downloads e pelo explorador de arquivos para impedir
//! execução acidental de programas, scripts, imagens de disco, atalhos do Windows
//! e arquivos mascarados com extensões falsas ou caracteres bidi.

use std::path::{Path, PathBuf};

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
pub const PROGRAM_EXTENSIONS: &[&str] = &[
    "exe",
    "com",
    "scr",
    "pif",
    "cpl",
    "msi",
    "msp",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "appinstaller",
];

/// Extensões de scripts ou interpretadores executáveis.
pub const SCRIPT_EXTENSIONS: &[&str] = &[
    "bat", "cmd", "ps1", "psm1", "psd1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "wsc", "hta",
    "jar",
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
];

/// Extensões de imagens de disco e contêineres montáveis.
pub const DISKIMAGE_EXTENSIONS: &[&str] = &["iso", "img", "vhd", "vhdx"];

/// Extensões de documentos com macros habilitadas (Office/Office-like).
pub const MACRO_EXTENSIONS: &[&str] = &[
    "docm", "dotm", "xlsm", "xltm", "xlam", "pptm", "potm", "ppam", "sldm", "xll",
];

/// Extensões conhecidas de documentos, imagens e mídias seguros comuns
/// para detecção de extensão dupla (masquerading).
pub const SAFE_DECOY_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "odt", "ods", "odp", "png",
    "jpg", "jpeg", "gif", "webp", "bmp", "svg", "mp3", "mp4", "wav", "zip", "csv", "json",
];

/// Allowlist estrita de extensões permitidas no `DefaultAppTarget`.
pub const DEFAULT_APP_ALLOWLIST: &[&str] = &[
    "txt", "md", "csv", "json", "log", "pdf", "epub", "docx", "xlsx", "pptx", "odt", "ods", "odp",
    "rtf", "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "mp3", "m4a", "wav", "flac", "ogg",
    "opus", "webm", "mp4", "mov",
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

/// Normaliza um nome de arquivo segundo as regras fundamentais do subsistema Win32:
/// corta espaços e pontos finais à direita, e corta em ':' (Alternate Data Streams).
pub fn normalize_windows_name(name: &str) -> String {
    let before_ads = match name.split_once(':') {
        Some((stem, _)) => stem,
        None => name,
    };
    before_ads.trim_end_matches([' ', '.']).to_lowercase()
}

/// Substitui caracteres bidi ou controles em rótulos visuais por marcas explícitas legíveis.
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
            _ if c.is_control() => {
                out.push_str(&format!("‹U+{:04X}›", c as u32));
            }
            _ => out.push(c),
        }
    }
    out
}

/// Classifica o risco de um nome de arquivo para download ou abertura.
///
/// Refusa nomes com controles bidi, caracteres de controle C0/C1 ou dois-pontos (ADS).
/// Trata espaços e pontos ao final como Win32 trata.
/// Bloqueia executáveis, scripts, imagens de disco e mascaramentos com extensão dupla.
/// Emite aviso para documentos com macro.
pub fn classify_download_name(name: &str) -> RiskClass {
    if name.is_empty() {
        return RiskClass::Block;
    }

    // Refusa ':' (Alternate Data Streams) e caracteres de controle ou bidi
    if name.contains(':') || name.chars().any(is_bidi_or_control) {
        return RiskClass::Block;
    }

    // Regra do Windows: remove espaços e pontos no final
    let clean = name.trim_end_matches([' ', '.']);
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

    // Decomposição de extensões
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() < 2 {
        // Sem extensão
        return RiskClass::Safe;
    }

    let last_raw = parts.last().unwrap();
    let final_ext = last_raw.trim().to_ascii_lowercase();

    // Verificação de executáveis, scripts e imagens de disco -> Block
    let is_dangerous_ext = PROGRAM_EXTENSIONS.contains(&final_ext.as_str())
        || SCRIPT_EXTENSIONS.contains(&final_ext.as_str())
        || SHORTCUT_SHELL_EXTENSIONS.contains(&final_ext.as_str())
        || DISKIMAGE_EXTENSIONS.contains(&final_ext.as_str());

    // Detecção de Masquerade (disfarce com extensão dupla ou espaçamento):
    // Exemplo: `fatura.pdf.exe`, `foto.jpg     .scr`, `documento.docx.vbs`
    if parts.len() >= 3 {
        let penult_ext = parts[parts.len() - 2].trim().to_ascii_lowercase();
        if SAFE_DECOY_EXTENSIONS.contains(&penult_ext.as_str()) && is_dangerous_ext {
            return RiskClass::Block;
        }
    }

    // Espaçamento disfarçado antes da extensão perigosa (ex.: "arquivo.pdf   .exe")
    if last_raw.starts_with(' ') && is_dangerous_ext {
        return RiskClass::Block;
    }

    if is_dangerous_ext {
        return RiskClass::Block;
    }

    // Verificação de macros -> Warn
    if MACRO_EXTENSIONS.contains(&final_ext.as_str()) {
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
    let trimmed = slice.trim_ascii_start();
    if trimmed.starts_with(b"@echo off") || trimmed.starts_with(b"@ECHO OFF") {
        return SniffRisk::Warn;
    }

    SniffRisk::Safe
}

/// Alvo validado para abertura no aplicativo padrão do sistema.
///
/// Construtor privado: NENHUM caminho bruto pode alcançar `ShellExecuteW('open')`.
/// Somente instâncias de `DefaultAppTarget` construídas através de `default_app_target()`
/// após rigorosa verificação de allowlist, bloqueio de executáveis/scripts/macros/masquerades
/// e sniffing de magic bytes (MZ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultAppTarget(PathBuf);

impl DefaultAppTarget {
    /// O caminho absoluto ou relativo seguro validado para o alvo.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Valida se um arquivo e seus bytes iniciais podem ser abertos pelo aplicativo padrão do sistema.
///
/// Recusa programas, scripts, documentos com macro, atalhos do Windows (.url, .lnk, .library-ms, etc.)
/// e arquivos que não pertençam à allowlist de tipos seguros de documentos/mídias.
pub fn default_app_target(path: impl AsRef<Path>, head: Option<&[u8]>) -> Option<DefaultAppTarget> {
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

    // Sniff de magic bytes: se o arquivo tiver assinatura executável MZ, recusa imediatamente
    if let Some(bytes) = head
        && (bytes.starts_with(b"MZ") || matches!(sniff_download(bytes), SniffRisk::Dangerous))
    {
        return None;
    }

    Some(DefaultAppTarget(p.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            // Scripts (Block)
            ("run.bat", RiskClass::Block),
            ("run.cmd", RiskClass::Block),
            ("deploy.ps1", RiskClass::Block),
            ("module.psm1", RiskClass::Block),
            ("data.psd1", RiskClass::Block),
            ("auto.vbs", RiskClass::Block),
            ("encode.vbe", RiskClass::Block),
            ("payload.js", RiskClass::Block),
            ("payload.jse", RiskClass::Block),
            ("script.wsf", RiskClass::Block),
            ("script.wsh", RiskClass::Block),
            ("component.wsc", RiskClass::Block),
            ("app.hta", RiskClass::Block),
            ("archive.jar", RiskClass::Block),
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
            ("excel.xll", RiskClass::Warn),
            // Masquerade (Block)
            ("invoice.pdf.exe", RiskClass::Block),
            ("invoice.pdf    .exe", RiskClass::Block),
            ("relatorio.docx.bat", RiskClass::Block),
            ("foto.png.vbs", RiskClass::Block),
            ("dados.xlsx.scr", RiskClass::Block),
            ("tabela.csv.ps1", RiskClass::Block),
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
    fn sniff_download_identifies_binary_and_script_signatures() {
        // MZ + PE
        let mut pe_bytes = vec![0u8; 1024];
        pe_bytes[0] = b'M';
        pe_bytes[1] = b'Z';
        pe_bytes[0x3C] = 0x80; // offset do PE em 128
        pe_bytes[128..132].copy_from_slice(b"PE\0\0");
        assert_eq!(sniff_download(&pe_bytes), SniffRisk::Dangerous);

        // LNK (Windows Shell Link)
        let mut lnk_bytes = vec![0u8; 100];
        lnk_bytes[..4].copy_from_slice(&[0x4C, 0, 0, 0]);
        lnk_bytes[4..20].copy_from_slice(&[
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ]);
        assert_eq!(sniff_download(&lnk_bytes), SniffRisk::Dangerous);

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

        // Batch @echo off
        assert_eq!(sniff_download(b"@echo off\ndir"), SniffRisk::Warn);
        assert_eq!(sniff_download(b"  @ECHO OFF\r\ncls"), SniffRisk::Warn);

        // Texto plano normal
        assert_eq!(
            sniff_download(b"Hello world! This is plain text."),
            SniffRisk::Safe
        );
    }

    #[test]
    fn default_app_target_table_enforces_strict_allowlist_and_mz_sniffing() {
        // Permitidos
        assert!(default_app_target("livro.epub", None).is_some());
        assert!(default_app_target("manual.pdf", Some(b"%PDF-1.7...")).is_some());
        assert!(default_app_target("imagem.png", None).is_some());
        assert!(default_app_target("dados.csv", None).is_some());
        assert!(default_app_target("texto.txt", None).is_some());
        assert!(default_app_target("documento.docx", None).is_some());

        // Bloqueados por extensão perigosa
        assert!(default_app_target("programa.exe", None).is_none());
        assert!(default_app_target("script.ps1", None).is_none());
        assert!(default_app_target("lote.bat", None).is_none());
        assert!(default_app_target("app.hta", None).is_none());
        assert!(default_app_target("link.url", None).is_none());
        assert!(default_app_target("atalho.lnk", None).is_none());
        assert!(default_app_target("ajuda.chm", None).is_none());
        assert!(default_app_target("macro.docm", None).is_none());

        // Masquerade bloqueado
        assert!(default_app_target("relatorio.pdf.exe", None).is_none());

        // Falso PDF contendo binário MZ (sniffing bloqueia)
        assert!(default_app_target("falso.pdf", Some(b"MZ\x90\0\x03\0\0\0")).is_none());
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
        assert_eq!(classify_download_name("script.ps1"), RiskClass::Block);
        assert!(default_app_target("script.ps1", None).is_none());
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
        assert!(default_app_target("app.hta", None).is_none());
    }
}
