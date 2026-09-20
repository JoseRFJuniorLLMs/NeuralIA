// O formato da carga util que o instalador traz dentro de si.
//
// Um instalador e um programa que escreve ficheiros onde lhe mandam. O que
// aqui interessa nao e o formato -- e simples de proposito -- mas as duas
// recusas: um caminho que saia da pasta de instalacao, e um pacote que nao
// bata certo com o seu proprio resumo. Ambas sao testadas.
// (Comentario normal e nao `//!` porque o `build.rs` inclui este ficheiro
// dentro de um modulo, e um doc-comment interno so vale no inicio do ficheiro.)

use std::fmt;

/// `NeuralIA Payload`. Muda se o formato mudar.
const MAGIC: &[u8; 4] = b"NIAP";
const VERSION: u32 = 1;
/// Tecto por entrada: 512 MiB. Um comprimento absurdo num pacote truncado
/// levaria a uma reserva de memoria absurda antes de se dar pelo erro.
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: u32 = 4096;

#[derive(Debug, PartialEq, Eq)]
pub enum ArchiveError {
    /// Nao comeca por `NIAP`: isto nao e um pacote nosso.
    NotAPackage,
    /// Versao de formato que este instalador nao sabe ler.
    UnknownVersion(u32),
    /// Acabou a meio. Descarregamento cortado, quase sempre.
    Truncated,
    /// O resumo nao bate certo: os bytes mudaram entre empacotar e instalar.
    Corrupt,
    /// Um caminho que nao fica debaixo da pasta de instalacao.
    UnsafePath(String),
    /// Numeros que nao fazem sentido nenhum (entradas a mais, entrada enorme).
    Absurd,
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAPackage => write!(f, "este ficheiro nao e um pacote NeuralIA"),
            Self::UnknownVersion(v) => {
                write!(f, "pacote da versao {v}, que este instalador nao le")
            }
            Self::Truncated => write!(f, "o pacote acabou a meio"),
            Self::Corrupt => write!(f, "o pacote nao bate certo com o seu resumo"),
            Self::UnsafePath(path) => write!(f, "caminho recusado: {path}"),
            Self::Absurd => write!(f, "o pacote declara tamanhos impossiveis"),
        }
    }
}

/// Um ficheiro dentro do pacote. `path` e sempre relativo, com `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub data: Vec<u8>,
}

/// Aceita so o que fica garantidamente debaixo da pasta de instalacao.
///
/// Nao e uma lista de coisas proibidas -- e o contrario: cada componente tem
/// de ser um nome simples. `..`, raizes, letras de unidade e nomes vazios caem
/// todos por nao serem isso, sem ser preciso lembrar-se de cada um.
pub fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 1024 {
        return false;
    }
    if path.contains('\\') || path.contains(':') || path.contains('\0') {
        return false;
    }
    if path.starts_with('/') {
        return false;
    }
    path.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.ends_with(' ')
            && !part.ends_with('.')
            && !part.chars().any(|c| c.is_control())
    })
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Empacota. Recusa-se a produzir um pacote com um caminho que o `unpack` iria
/// recusar -- o erro aparece a quem constroi, nao a quem instala.
// Quem chama e o `build.rs`, que compila este ficheiro a parte; do lado do
// binario so o usam os testes.
#[allow(dead_code)]
pub fn pack(entries: &[Entry]) -> Result<Vec<u8>, ArchiveError> {
    if entries.len() as u64 > MAX_ENTRIES as u64 {
        return Err(ArchiveError::Absurd);
    }
    let mut body = Vec::new();
    body.extend_from_slice(MAGIC);
    body.extend_from_slice(&VERSION.to_le_bytes());
    body.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        if !is_safe_relative_path(&entry.path) {
            return Err(ArchiveError::UnsafePath(entry.path.clone()));
        }
        if entry.data.len() as u64 > MAX_ENTRY_BYTES {
            return Err(ArchiveError::Absurd);
        }
        let name = entry.path.as_bytes();
        body.extend_from_slice(&(name.len() as u32).to_le_bytes());
        body.extend_from_slice(name);
        body.extend_from_slice(&(entry.data.len() as u64).to_le_bytes());
        body.extend_from_slice(&entry.data);
    }
    let sum = digest(&body);
    body.extend_from_slice(&sum);
    Ok(body)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], ArchiveError> {
        let end = self.at.checked_add(count).ok_or(ArchiveError::Absurd)?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(ArchiveError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, ArchiveError> {
        let raw = self.take(4)?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    }

    fn u64(&mut self) -> Result<u64, ArchiveError> {
        let raw = self.take(8)?;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(raw);
        Ok(u64::from_le_bytes(buf))
    }
}

/// Desempacota, verificando o resumo antes de devolver seja o que for.
pub fn unpack(bytes: &[u8]) -> Result<Vec<Entry>, ArchiveError> {
    if bytes.len() < 44 {
        return Err(ArchiveError::Truncated);
    }
    let split = bytes.len() - 32;
    let (body, sum) = bytes.split_at(split);
    if body.get(..4) != Some(MAGIC.as_slice()) {
        return Err(ArchiveError::NotAPackage);
    }
    // Primeiro o resumo. So depois de os bytes serem os certos e que se
    // acredita nos comprimentos que eles declaram.
    if digest(body) != sum {
        return Err(ArchiveError::Corrupt);
    }

    let mut cursor = Cursor { bytes: body, at: 4 };
    let version = cursor.u32()?;
    if version != VERSION {
        return Err(ArchiveError::UnknownVersion(version));
    }
    let count = cursor.u32()?;
    if count > MAX_ENTRIES {
        return Err(ArchiveError::Absurd);
    }

    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name_len = cursor.u32()? as usize;
        let name = cursor.take(name_len)?;
        let path = std::str::from_utf8(name)
            .map_err(|_| ArchiveError::Corrupt)?
            .to_string();
        if !is_safe_relative_path(&path) {
            return Err(ArchiveError::UnsafePath(path));
        }
        let data_len = cursor.u64()?;
        if data_len > MAX_ENTRY_BYTES {
            return Err(ArchiveError::Absurd);
        }
        let data = cursor.take(data_len as usize)?.to_vec();
        entries.push(Entry { path, data });
    }
    if cursor.at != body.len() {
        return Err(ArchiveError::Corrupt);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, data: &[u8]) -> Entry {
        Entry {
            path: path.to_string(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn what_goes_in_comes_out() {
        let entries = vec![
            entry("NeuralIA.exe", b"MZ\x00\x90binario"),
            entry("assets/logo.ico", &[0u8, 1, 2, 3, 250, 251]),
            entry("vazio.txt", b""),
        ];
        let packed = pack(&entries).expect("empacotar");
        assert_eq!(unpack(&packed).expect("desempacotar"), entries);
    }

    #[test]
    fn a_path_that_escapes_the_install_folder_is_refused() {
        // O caso que interessa: um pacote trocado nao pode escrever no
        // arranque do Windows nem por cima de um ficheiro do sistema.
        for bad in [
            "../fora.exe",
            "a/../../fora.exe",
            "/etc/passwd",
            "C:/Windows/System32/evil.dll",
            "pasta\\ficheiro.exe",
            "",
            "a//b",
            "a/./b",
            "fim.",
            "espaco ",
        ] {
            assert!(!is_safe_relative_path(bad), "{bad:?} devia ser recusado");
            assert_eq!(
                pack(&[entry(bad, b"x")]),
                Err(ArchiveError::UnsafePath(bad.to_string())),
                "empacotar {bad:?} devia falhar"
            );
        }
        for good in ["NeuralIA.exe", "assets/logo.ico", "a/b/c/d.dll"] {
            assert!(is_safe_relative_path(good), "{good:?} devia passar");
        }
    }

    #[test]
    fn a_crafted_package_cannot_smuggle_an_escaping_path_past_unpack() {
        // Quem empacota e nosso; quem entrega o ficheiro pode nao ser. O
        // `unpack` volta a verificar em vez de confiar no `pack`.
        let honest = pack(&[entry("aaaaaaaaaaaa", b"x")]).expect("empacotar");
        let mut forged = honest[..honest.len() - 32].to_vec();
        let at = forged
            .windows(12)
            .position(|w| w == b"aaaaaaaaaaaa")
            .expect("nome no corpo");
        forged[at..at + 12].copy_from_slice(b"../fora.exe\0");
        let sum = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(&forged);
            h.finalize()
        };
        forged.extend_from_slice(&sum);

        // O resumo bate certo -- e mesmo assim tem de ser recusado.
        assert!(matches!(
            unpack(&forged),
            Err(ArchiveError::UnsafePath(_)) | Err(ArchiveError::Corrupt)
        ));
    }

    #[test]
    fn a_truncated_download_is_caught_instead_of_half_installed() {
        let packed = pack(&[entry("NeuralIA.exe", &vec![7u8; 4096])]).expect("empacotar");
        for cut in [0, 10, 43, 100, packed.len() - 1] {
            assert!(
                unpack(&packed[..cut]).is_err(),
                "cortado em {cut} devia falhar"
            );
        }
    }

    #[test]
    fn a_single_flipped_byte_is_caught_before_anything_is_written() {
        let packed = pack(&[entry("NeuralIA.exe", &vec![7u8; 512])]).expect("empacotar");
        for at in [4, 40, packed.len() / 2, packed.len() - 40] {
            let mut damaged = packed.clone();
            damaged[at] ^= 0xff;
            assert_eq!(
                unpack(&damaged),
                Err(ArchiveError::Corrupt),
                "byte {at} alterado devia ser apanhado"
            );
        }
    }

    #[test]
    fn something_that_is_not_a_package_says_so() {
        let mut junk = vec![0u8; 128];
        junk[0..4].copy_from_slice(b"ZIP\0");
        assert_eq!(unpack(&junk), Err(ArchiveError::NotAPackage));
    }

    #[test]
    fn an_empty_package_is_valid_and_installs_nothing() {
        // E o que sai de um `cargo build` sem carga util. Tem de ler-se sem
        // explodir, para o instalador poder dizer porque nao instala.
        let packed = pack(&[]).expect("empacotar");
        assert_eq!(unpack(&packed).expect("desempacotar"), Vec::new());
    }
}
