//! O cofre das chaves (infra-settings-keys, plano 2.3): a UNICA extracao da
//! DPAPI do NeuralIA. Antes vivia em `gemini_live.rs`, presa a entropia do
//! Live; agora cada uso traz a sua entropia (`SecretFile`), e quem precisar
//! de cifrar para o utilizador atual (o Registo de experiencia, as
//! permissoes do agente, os tokens dos conectores) reusa estes helpers com a
//! entropia dele -- nunca um segundo `dpapi.rs`.
//!
//! - `SecretFile{path, magic, entropy}`: um ficheiro `<magic><blob da DPAPI>`,
//!   escrito de forma atomica. Estragado, de outro formato ou cifrado com
//!   outra entropia: vale "sem segredo", nunca um panic.
//! - `KeySlot`: onde vive cada chave. O Gemini delega no ficheiro do Live
//!   (`gemini-live.key`, `NLK1`, `NeuralIA/gemini-live/v1`, byte a byte os
//!   de antes); os outros vivem em `<data_dir>/keys/<slot>.key`, com `NBK1` e
//!   a entropia `NeuralIA/key/<slot>/v1` -- a de um slot nao abre o de outro.
//! - `ApiKey`: nunca se imprime (`Debug` diz so que existe) e e zerada ao
//!   sair de cena. `validate_api_key` decide a forma por slot.
//! - `redact_debug_secrets`: o unico redator do log de depuracao.
//!
//! As chaves sao credenciais: o Ctrl+Shift+Delete nunca as apaga (nao estao
//! em `clear_history.rs`); so o "Esquecer chave" o faz.

use std::borrow::Cow;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use neural_core::json_store::StoreGrant;

use crate::gemini_live::{LiveKeyStore, validate_live_key};
use crate::stores::{KEYS_STORE, LIVE_KEY_STORE};

// ------------------------------------------------------------ memoria

/// Zera um buffer do proprio NeuralIA antes de o devolver ao alocador. Zeros
/// sao UTF-8 valido, por isso servem tambem para o `String` de uma chave.
///
/// E so higiene, nao uma garantia: uma chave passa por copias que ninguem
/// zera (o corpo de uma mensagem do WebView2, um `serde_json::Value`, o
/// HSTRING de um script). O que a protege de verdade e a DPAPI no disco e
/// ela nunca ir para o log (`redact_debug_secrets`).
pub(crate) fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Texto decifrado, apagado da memoria quando sai de cena.
pub(crate) struct Plain(Vec<u8>);

impl Plain {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Plain {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

// ------------------------------------------------------------ DPAPI

pub(crate) fn blob(
    bytes: &[u8],
) -> std::io::Result<windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB> {
    Ok(
        windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(bytes.len())
                .map_err(|_| std::io::Error::other("dados demasiado grandes para a DPAPI"))?,
            // A DPAPI so le a entrada; o `*mut` e a assinatura do Win32.
            pbData: bytes.as_ptr() as *mut u8,
        },
    )
}

/// Copia a saida da DPAPI (LocalAlloc) e liberta-a, apagando-a antes.
pub(crate) unsafe fn take_dpapi_output(
    output: &windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
) -> Vec<u8> {
    if output.pbData.is_null() {
        return Vec::new();
    }
    let len = output.cbData as usize;
    let copy = unsafe { std::slice::from_raw_parts(output.pbData, len) }.to_vec();
    wipe(unsafe { std::slice::from_raw_parts_mut(output.pbData, len) });
    unsafe {
        windows_sys::Win32::Foundation::LocalFree(output.pbData as _);
    }
    copy
}

/// Cifra para o utilizador atual do Windows, com `entropy` como parte da
/// fechadura: sem ela (outro programa do mesmo utilizador, outro slot) o
/// `CryptUnprotectData` nao abre.
pub(crate) fn dpapi_protect(plain: &[u8], entropy: &[u8]) -> std::io::Result<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };
    let input = blob(plain)?;
    let entropy = blob(entropy)?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { take_dpapi_output(&output) })
}

/// Decifra com a mesma `entropy`; qualquer falha e `None`.
pub(crate) fn dpapi_unprotect(protected: &[u8], entropy: &[u8]) -> Option<Plain> {
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };
    if protected.is_empty() {
        return None;
    }
    let input = blob(protected).ok()?;
    let entropy = blob(entropy).ok()?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    (ok != 0).then(|| Plain(unsafe { take_dpapi_output(&output) }))
}

// ------------------------------------------------------------ ficheiro

/// Um segredo em disco: `<magic><blob da DPAPI com a entropia>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SecretFile {
    path: PathBuf,
    magic: [u8; 4],
    entropy: Vec<u8>,
}

/// Um segredo cifrado e pequeno: um ficheiro maior nem se le.
const SECRET_FILE_MAX_BYTES: u64 = 16 * 1024;

impl SecretFile {
    pub(crate) fn new(path: PathBuf, magic: [u8; 4], entropy: impl Into<Vec<u8>>) -> Self {
        Self {
            path,
            magic,
            entropy: entropy.into(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// O segredo, ou `None` sem ficheiro, com outro cabecalho, estragado ou
    /// cifrado com outra entropia -- nunca derruba o app.
    pub(crate) fn load(&self) -> Option<Plain> {
        let file = std::fs::File::open(&self.path).ok()?;
        let mut bytes = Vec::new();
        file.take(SECRET_FILE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > SECRET_FILE_MAX_BYTES {
            return None;
        }
        let blob = bytes.strip_prefix(self.magic.as_slice())?;
        dpapi_unprotect(blob, &self.entropy)
    }

    /// Cifra e escreve num temporario ao lado, e troca: um corte a meio
    /// deixa o segredo antigo ou o novo, nunca meio ficheiro.
    pub(crate) fn save(&self, plain: &[u8]) -> std::io::Result<()> {
        let blob = dpapi_protect(plain, &self.entropy)?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".tmp");
        let temp = self.path.with_file_name(name);
        let written = (|| {
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(&self.magic)?;
            file.write_all(&blob)?;
            file.sync_all()
        })()
        .and_then(|()| std::fs::rename(&temp, &self.path));
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written
    }

    /// Apaga o ficheiro. Sem ficheiro ja esta esquecido.
    pub(crate) fn forget(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

// ------------------------------------------------------------ chaves

/// Cabecalho dos ficheiros de `<data_dir>/keys/`.
pub(crate) const KEY_MAGIC: [u8; 4] = *b"NBK1";

/// O identificador de um servidor BYOM ou de um conector: 1 a 32 de
/// `[a-z0-9-]`, sem `-` nas pontas. Vira nome de ficheiro e parte da
/// entropia, por isso nunca leva `/`, `.` ou maiusculas.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SlotId(String);

impl SlotId {
    pub(crate) fn new(raw: &str) -> Option<Self> {
        let valid = (1..=32).contains(&raw.len())
            && !raw.starts_with('-')
            && !raw.ends_with('-')
            && raw
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        valid.then(|| Self(raw.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Onde vive cada chave. Uma chave da OpenAI serve o juiz e o Copiloto.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum KeySlot {
    /// Delega no ficheiro do Gemini Live (`gemini-live.key`).
    Gemini,
    OpenAi,
    Anthropic,
    GoogleFactCheck,
    Byom(SlotId),
    Connector(SlotId),
}

impl KeySlot {
    /// O nome do slot no disco e na entropia (`openai`, `byom-<id>`...).
    pub(crate) fn stem(&self) -> String {
        match self {
            Self::Gemini => "gemini".to_string(),
            Self::OpenAi => "openai".to_string(),
            Self::Anthropic => "anthropic".to_string(),
            Self::GoogleFactCheck => "google-factcheck".to_string(),
            Self::Byom(id) => format!("byom-{}", id.as_str()),
            Self::Connector(id) => format!("connector-{}", id.as_str()),
        }
    }

    /// A entropia da DPAPI deste slot, `NeuralIA/key/<slot>/v1`: a de um
    /// slot nao abre o ficheiro de outro. O Gemini nao a usa -- delega no
    /// ficheiro do Live, com a entropia do Live.
    pub(crate) fn entropy(&self) -> Vec<u8> {
        format!("NeuralIA/key/{}/v1", self.stem()).into_bytes()
    }

    /// O nome que o utilizador ve.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Gemini => "Gemini".to_string(),
            Self::OpenAi => "OpenAI".to_string(),
            Self::Anthropic => "Anthropic".to_string(),
            Self::GoogleFactCheck => "Google Fact Check".to_string(),
            Self::Byom(id) => format!("servidor {}", id.as_str()),
            Self::Connector(id) => format!("conector {}", id.as_str()),
        }
    }
}

/// Uma chave de API. Nunca se imprime: o `Debug` diz so que existe, para um
/// `{:?}` de um evento (o `UserEvent` deriva Debug) nao a deixar num log. E
/// zerada quando sai de cena. So `validate_api_key` (e o cofre, que valida
/// o que decifra) a cria.
pub(crate) struct ApiKey(String);

impl ApiKey {
    /// O texto da chave, para o cofre e para o pedido ao fornecedor.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ApiKey(<omitida>)")
    }
}

impl Drop for ApiKey {
    fn drop(&mut self) {
        wipe(unsafe { self.0.as_mut_vec() });
    }
}

/// A chave que o transporte de IA (`neural_core::llm`, infra-llm-transport)
/// aceita: ele so a le para o cabecalho de autenticacao do fornecedor, nunca
/// para o URL nem para o corpo.
impl neural_core::llm::ApiCredential for ApiKey {
    fn secret(&self) -> &str {
        self.expose()
    }
}

pub(crate) const API_KEY_MIN_CHARS: usize = 20;
pub(crate) const API_KEY_MAX_CHARS: usize = 300;

/// A forma de uma chave, por slot. Sempre sem espacos a volta (a colagem
/// traz muitas vezes uma quebra de linha); depois, 20 a 300 caracteres ASCII
/// visiveis (nada de espacos, controlo ou acentos). A OpenAI comeca por
/// `sk-` (e nao `sk-ant-`, que e da Anthropic); a Anthropic por `sk-ant-`; o
/// Gemini segue a regra do Live (`validate_live_key`).
pub(crate) fn validate_api_key(slot: &KeySlot, raw: &str) -> Option<ApiKey> {
    let key = raw.trim();
    if *slot == KeySlot::Gemini {
        return validate_live_key(key).map(|live| ApiKey(live.expose().to_string()));
    }
    let shaped = (API_KEY_MIN_CHARS..=API_KEY_MAX_CHARS).contains(&key.len())
        && key.bytes().all(|byte| byte.is_ascii_graphic());
    let prefixed = match slot {
        KeySlot::OpenAi => key.starts_with("sk-") && !key.starts_with("sk-ant-"),
        KeySlot::Anthropic => key.starts_with("sk-ant-"),
        _ => true,
    };
    (shaped && prefixed).then(|| ApiKey(key.to_string()))
}

/// O grant errado para o cofre.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WrongVaultGrant(pub(crate) &'static str);

/// As chaves de todos os slots, abertas com os dois grants da pasta de
/// dados: `keys/` e `gemini-live.key` (ambos `Explicit`).
pub(crate) struct KeyVault {
    keys_dir: PathBuf,
    live: LiveKeyStore,
}

impl KeyVault {
    pub(crate) fn open(keys: StoreGrant, live: StoreGrant) -> Result<Self, WrongVaultGrant> {
        for (grant, spec) in [(&keys, KEYS_STORE), (&live, LIVE_KEY_STORE)] {
            if (grant.name(), grant.kind(), grant.shape()) != (spec.name, spec.kind, spec.shape) {
                return Err(WrongVaultGrant(grant.name()));
            }
        }
        Ok(Self {
            keys_dir: keys.path().to_path_buf(),
            live: LiveKeyStore::at(live.path().to_path_buf()),
        })
    }

    /// O ficheiro de um slot. O Gemini e o do Live.
    pub(crate) fn secret_file(&self, slot: &KeySlot) -> SecretFile {
        match slot {
            KeySlot::Gemini => self.live.secret_file().clone(),
            _ => SecretFile::new(
                self.keys_dir.join(format!("{}.key", slot.stem())),
                KEY_MAGIC,
                slot.entropy(),
            ),
        }
    }

    /// A chave guardada, ou `None` (sem ficheiro, estragado, de outro slot,
    /// ou sem a forma do slot).
    pub(crate) fn load(&self, slot: &KeySlot) -> Option<ApiKey> {
        if *slot == KeySlot::Gemini {
            return self
                .live
                .load()
                .map(|live| ApiKey(live.expose().to_string()));
        }
        let plain = self.secret_file(slot).load()?;
        validate_api_key(slot, std::str::from_utf8(plain.bytes()).ok()?)
    }

    /// Guarda a chave no slot. Uma chave sem a forma DESTE slot (valida
    /// para outro) e recusada antes de tocar no disco.
    pub(crate) fn save(&self, slot: &KeySlot, key: &ApiKey) -> std::io::Result<()> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("não é uma chave de {}", slot.label()),
            )
        };
        if *slot == KeySlot::Gemini {
            let live = validate_live_key(key.expose()).ok_or_else(invalid)?;
            return self.live.save(&live);
        }
        let checked = validate_api_key(slot, key.expose()).ok_or_else(invalid)?;
        self.secret_file(slot).save(checked.expose().as_bytes())
    }

    /// "Esquecer chave": apaga o ficheiro do slot.
    pub(crate) fn forget(&self, slot: &KeySlot) -> std::io::Result<()> {
        if *slot == KeySlot::Gemini {
            return self.live.forget();
        }
        self.secret_file(slot).forget()
    }
}

// ------------------------------------------------------------ log

const REDACTION_MARK: &str = "[chave omitida]";

/// Os bytes de uma chave (`AIza...`, `sk-...`, o valor de `key=`).
fn key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'%')
}

/// Os de um token de cabecalho (`Bearer`, `x-api-key`): tambem base64.
fn header_byte(byte: u8) -> bool {
    key_byte(byte) || matches!(byte, b'+' | b'/' | b'=' | b'~')
}

fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn starts_with_ignore_case(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Onde comeca o que se tapa, o minimo de bytes para contar e o alfabeto.
type SecretSpan = (usize, usize, fn(u8) -> bool);

/// Um segredo que comeca em `index`.
fn secret_at(bytes: &[u8], index: usize) -> Option<SecretSpan> {
    let rest = &bytes[index..];
    let boundary = index == 0 || !word_byte(bytes[index - 1]);
    if starts_with_ignore_case(rest, b"key=") {
        return Some((index + 4, 8, key_byte));
    }
    if rest.starts_with(b"\"key\":\"") {
        return Some((index + 7, 8, key_byte));
    }
    // A chave inteira, com o prefixo: `AIza...` (Google), `sk-...`,
    // `sk-proj-...` (OpenAI), `sk-ant-...` (Anthropic).
    if rest.starts_with(b"AIza") {
        return Some((index, 20, key_byte));
    }
    if boundary && rest.starts_with(b"sk-") {
        return Some((index, 20, key_byte));
    }
    if boundary && starts_with_ignore_case(rest, b"bearer") {
        let spaces = rest[6..]
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        return (spaces > 0).then_some((index + 6 + spaces, 8, header_byte));
    }
    // `x-api-key: v`, `x-api-key=v`, `"x-api-key": "v"`, e o mesmo com
    // `x-goog-api-key`.
    let name = [b"x-goog-api-key".as_slice(), b"x-api-key".as_slice()]
        .into_iter()
        .find(|name| boundary && starts_with_ignore_case(rest, name))?;
    let mut at = index + name.len();
    let skip = |at: &mut usize, wanted: &[u8]| {
        while *at < bytes.len() && wanted.contains(&bytes[*at]) {
            *at += 1;
        }
    };
    skip(&mut at, b"\"");
    skip(&mut at, b" \t");
    if at >= bytes.len() || !matches!(bytes[at], b':' | b'=') {
        return None;
    }
    at += 1;
    skip(&mut at, b" \t");
    skip(&mut at, b"\"");
    Some((at, 8, header_byte))
}

/// Rede de seguranca do log de depuracao: nenhum codigo escreve uma chave
/// la, mas uma linha que a leve sai com ela trocada por um marcador. Apanha
/// `key=<valor>`, `"key":"<valor>"`, as chaves soltas `AIza...`, `sk-...`,
/// `sk-proj-...` e `sk-ant-...`, `Bearer <token>`, e os cabecalhos
/// `x-api-key` e `x-goog-api-key`.
pub(crate) fn redact_debug_secrets(line: &str) -> Cow<'_, str> {
    let bytes = line.as_bytes();
    let mut out = String::new();
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        let Some((start, minimum, token)) = secret_at(bytes, index) else {
            index += 1;
            continue;
        };
        let end = start
            + bytes[start..]
                .iter()
                .take_while(|byte| token(**byte))
                .count();
        if end - start >= minimum {
            // Os limites caem sempre em bytes ASCII: fronteiras de char.
            out.push_str(&line[copied..start]);
            out.push_str(REDACTION_MARK);
            copied = end;
            index = end;
        } else {
            index += 1;
        }
    }
    if copied == 0 {
        Cow::Borrowed(line)
    } else {
        out.push_str(&line[copied..]);
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini_live::{LIVE_KEY_ENTROPY, LIVE_KEY_FILE, LIVE_KEY_MAGIC};
    use neural_core::json_store::StoreRegistry;

    // Chaves com a forma das verdadeiras, montadas com `concat!` para o
    // texto do codigo nao ter a forma de uma chave (o scanner de segredos
    // do PR acusava-as). Nenhuma e real.
    const GEMINI_KEY: &str = concat!("AIza", "SyTESTONLY-not-a-real-key_0123456789");
    const OPENAI_KEY: &str = concat!("sk-", "proj-", "TESTONLY_not_a_real_key_0123456789");
    const ANTHROPIC_KEY: &str = concat!("sk-", "ant-", "api03-TESTONLY-not-a-real-key-0123");
    const OTHER_KEY: &str = concat!("TESTONLY", "-plain-provider-key-0123456789");

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("neuralia-secrets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("pasta temporaria");
        dir
    }

    fn vault(dir: &Path) -> KeyVault {
        let registry = StoreRegistry::mint_for_test(dir);
        KeyVault::open(
            registry.grant(KEYS_STORE).expect("grant keys"),
            registry.grant(LIVE_KEY_STORE).expect("grant live"),
        )
        .expect("cofre")
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn slots() -> Vec<(KeySlot, &'static str)> {
        vec![
            (KeySlot::Gemini, GEMINI_KEY),
            (KeySlot::OpenAi, OPENAI_KEY),
            (KeySlot::Anthropic, ANTHROPIC_KEY),
            (KeySlot::GoogleFactCheck, OTHER_KEY),
            (KeySlot::Byom(SlotId::new("ollama").expect("id")), OTHER_KEY),
            (
                KeySlot::Byom(SlotId::new("lm-studio").expect("id")),
                OTHER_KEY,
            ),
            (
                KeySlot::Connector(SlotId::new("ollama").expect("id")),
                OTHER_KEY,
            ),
        ]
    }

    #[test]
    fn every_slot_round_trips_encrypted_and_forgets() {
        let dir = temp_dir("round-trip");
        let vault = vault(&dir);
        for (slot, text) in slots() {
            assert!(vault.load(&slot).is_none(), "{slot:?} sem ficheiro");
            let key = validate_api_key(&slot, text).expect("chave de teste valida");
            vault.save(&slot, &key).expect("guardar");
            let file = vault.secret_file(&slot);
            let expected_path = match &slot {
                KeySlot::Gemini => dir.join("gemini-live.key"),
                other => dir.join("keys").join(format!("{}.key", other.stem())),
            };
            assert_eq!(file.path(), expected_path, "{slot:?}");
            let bytes = std::fs::read(file.path()).expect("o ficheiro existe");
            let magic: &[u8] = if slot == KeySlot::Gemini {
                b"NLK1"
            } else {
                b"NBK1"
            };
            assert!(bytes.starts_with(magic), "{slot:?}");
            // Nem inteira, nem um pedaco, nem em UTF-16: nada em claro.
            assert!(!contains(&bytes, text.as_bytes()), "{slot:?} em claro");
            assert!(
                !contains(&bytes, &text.as_bytes()[4..20]),
                "{slot:?} pedaco"
            );
            let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            assert!(!contains(&bytes, &utf16), "{slot:?} em UTF-16");
            assert_eq!(
                vault.load(&slot).as_ref().map(ApiKey::expose),
                Some(text),
                "{slot:?}"
            );
            let mut temp = file.path().file_name().expect("nome").to_os_string();
            temp.push(".tmp");
            assert!(!file.path().with_file_name(temp).exists());
        }
        // Cada slot tem o seu ficheiro: os sete estao la ao mesmo tempo.
        for (slot, text) in slots() {
            assert_eq!(vault.load(&slot).as_ref().map(ApiKey::expose), Some(text));
        }
        for (slot, _) in slots() {
            vault.forget(&slot).expect("esquecer");
            assert!(!vault.secret_file(&slot).path().exists(), "{slot:?}");
            assert!(vault.load(&slot).is_none());
            vault.forget(&slot).expect("esquecer sem ficheiro");
        }
        // Uma chave com a forma de outro slot nunca chega ao disco.
        let anthropic = validate_api_key(&KeySlot::Anthropic, ANTHROPIC_KEY).expect("chave");
        assert!(vault.save(&KeySlot::OpenAi, &anthropic).is_err());
        assert!(!vault.secret_file(&KeySlot::OpenAi).path().exists());
        // Um ficheiro que abre mas nao tem a forma do slot vale "sem chave".
        vault
            .secret_file(&KeySlot::OpenAi)
            .save(b"tem espacos e <html>")
            .expect("cifrar");
        assert!(vault.load(&KeySlot::OpenAi).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// O Live de antes deste refactor, copiado tal como estava em
    /// `gemini_live.rs` (45a59c0) e com as constantes ESCRITAS AQUI, nao
    /// importadas: um `gemini-live.key` gravado por um NeuralIA antigo tem de
    /// continuar a abrir, e o gravado agora tem de abrir num antigo.
    fn legacy_live_save(data_dir: &Path, key: &str) {
        use windows_sys::Win32::Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
        };
        let entropy: &[u8] = b"NeuralIA/gemini-live/v1";
        let input = blob(key.as_bytes()).expect("blob");
        let entropy = blob(entropy).expect("blob");
        let mut output = CRYPT_INTEGER_BLOB::default();
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        assert_ne!(ok, 0, "cifrar como o Live antigo");
        let protected = unsafe { take_dpapi_output(&output) };
        let mut file = b"NLK1".to_vec();
        file.extend(protected);
        std::fs::write(data_dir.join("gemini-live.key"), file).expect("gravar como o antigo");
    }

    fn legacy_live_load(data_dir: &Path) -> Option<String> {
        use windows_sys::Win32::Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        };
        let bytes = std::fs::read(data_dir.join("gemini-live.key")).ok()?;
        let protected = bytes.strip_prefix(b"NLK1".as_slice())?;
        let input = blob(protected).ok()?;
        let entropy = blob(b"NeuralIA/gemini-live/v1").ok()?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        (ok != 0).then(|| String::from_utf8(unsafe { take_dpapi_output(&output) }).ok())?
    }

    #[test]
    fn a_live_key_file_from_before_the_refactor_still_loads() {
        // Os tres valores do formato, byte a byte.
        assert_eq!(LIVE_KEY_FILE, "gemini-live.key");
        assert_eq!(LIVE_KEY_MAGIC, b"NLK1");
        assert_eq!(LIVE_KEY_ENTROPY, b"NeuralIA/gemini-live/v1");
        assert_eq!(LIVE_KEY_STORE.name, "gemini-live.key");

        let dir = temp_dir("legacy-live");
        legacy_live_save(&dir, GEMINI_KEY);
        // O painel do Live (o caminho que embarca) e o slot Gemini do cofre.
        assert_eq!(
            LiveKeyStore::in_dir(&dir)
                .load()
                .as_ref()
                .map(|key| key.expose()),
            Some(GEMINI_KEY)
        );
        let vault = vault(&dir);
        assert_eq!(
            vault.load(&KeySlot::Gemini).as_ref().map(ApiKey::expose),
            Some(GEMINI_KEY)
        );

        // E ao contrario: o que o cofre grava, um NeuralIA antigo abre.
        let other = concat!("AIza", "SyOTHERTEST-still-not-a-real-key-42");
        let key = validate_api_key(&KeySlot::Gemini, other).expect("chave");
        vault.save(&KeySlot::Gemini, &key).expect("guardar");
        assert_eq!(legacy_live_load(&dir).as_deref(), Some(other));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_slots_entropy_cannot_open_another_slots_file() {
        let dir = temp_dir("cross");
        let vault = vault(&dir);
        let slots: Vec<KeySlot> = slots().into_iter().map(|(slot, _)| slot).collect();
        // As entropias sao todas diferentes, e nenhuma e a do Live.
        for (index, slot) in slots.iter().enumerate().skip(1) {
            assert_ne!(
                slot.entropy(),
                LIVE_KEY_ENTROPY,
                "{slot:?} usa a entropia do Live"
            );
            for other in &slots[index + 1..] {
                assert_ne!(slot.entropy(), other.entropy(), "{slot:?} e {other:?}");
            }
        }
        for from in &slots {
            let source = vault.secret_file(from);
            source.save(b"segredo-de-teste").expect("cifrar");
            let bytes = std::fs::read(source.path()).expect("ler");
            let protected = &bytes[4..];
            assert_eq!(
                source.load().map(|plain| plain.bytes().to_vec()),
                Some(b"segredo-de-teste".to_vec()),
                "{from:?} abre o seu"
            );
            for to in slots.iter().filter(|to| *to != from) {
                let target = vault.secret_file(to);
                // O blob de `from` com o cabecalho de `to`, no sitio de `to`.
                let magic: &[u8] = if *to == KeySlot::Gemini {
                    b"NLK1"
                } else {
                    b"NBK1"
                };
                let mut moved = magic.to_vec();
                moved.extend_from_slice(protected);
                std::fs::create_dir_all(target.path().parent().expect("pasta")).expect("pasta");
                std::fs::write(target.path(), &moved).expect("copiar");
                assert!(
                    target.load().is_none(),
                    "a entropia de {to:?} abriu o ficheiro de {from:?}"
                );
                target.forget().expect("limpar");
            }
            source.forget().expect("limpar");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_keys_have_the_shape_of_their_slot() {
        let long = format!("sk-{}", "a".repeat(API_KEY_MAX_CHARS - 3));
        let too_long = format!("{long}a");
        let open_ai = KeySlot::OpenAi;
        let anthropic = KeySlot::Anthropic;
        let fact = KeySlot::GoogleFactCheck;
        let rows: [(&KeySlot, &str, bool); 20] = [
            (&open_ai, OPENAI_KEY, true),
            (
                &open_ai,
                concat!("sk-", "TESTONLYabcdefghij0123456789"),
                true,
            ),
            (&open_ai, &long, true),
            (&open_ai, &too_long, false),
            (&open_ai, ANTHROPIC_KEY, false),
            (&open_ai, OTHER_KEY, false),
            (&open_ai, "sk-curta", false),
            (&anthropic, ANTHROPIC_KEY, true),
            (&anthropic, OPENAI_KEY, false),
            (
                &anthropic,
                concat!("sk-", "TESTONLYabcdefghij0123456789"),
                false,
            ),
            (&fact, OTHER_KEY, true),
            (&fact, "TESTONLY com espacos no meio da chave", false),
            (&fact, "TESTONLY-chave-com-acento-ção-0123", false),
            (&fact, "TESTONLY-chave\u{7}-com-controlo-0123", false),
            (&fact, "curta-demais", false),
            (&KeySlot::Gemini, GEMINI_KEY, true),
            (&KeySlot::Gemini, OPENAI_KEY, true),
            (&KeySlot::Gemini, "AIza com <html> dentro da chave", false),
            (&open_ai, &format!("  {OPENAI_KEY}\r\n"), true),
            (&anthropic, "", false),
        ];
        for (slot, raw, valid) in rows {
            let key = validate_api_key(slot, raw);
            assert_eq!(key.is_some(), valid, "{slot:?} {raw:?}");
            if let Some(key) = key {
                assert_eq!(key.expose(), raw.trim());
            }
        }
    }

    #[test]
    fn an_api_key_never_prints_and_is_wiped() {
        let key = validate_api_key(&KeySlot::OpenAi, OPENAI_KEY).expect("chave");
        let printed = format!("{key:?} {:?}", Some(&key));
        assert!(!printed.contains(OPENAI_KEY), "{printed}");
        assert!(!printed.contains(&OPENAI_KEY[4..20]), "{printed}");
        assert!(printed.contains("<omitida>"));
        let mut buffer = OPENAI_KEY.as_bytes().to_vec();
        wipe(&mut buffer);
        assert!(buffer.iter().all(|byte| *byte == 0));
        // O Drop da ApiKey passa pelo mesmo wipe.
        let mut owned = ApiKey(OPENAI_KEY.to_string());
        unsafe { wipe(owned.0.as_mut_vec()) };
        assert!(owned.expose().bytes().all(|byte| byte == 0));
    }

    #[test]
    fn slot_ids_are_safe_file_names() {
        for good in ["ollama", "lm-studio", "a", "x9", &"a".repeat(32)] {
            assert!(SlotId::new(good).is_some(), "{good}");
        }
        for bad in [
            "",
            "-a",
            "a-",
            "A",
            "a/b",
            "..",
            "a.b",
            "a b",
            "ção",
            &"a".repeat(33),
        ] {
            assert!(SlotId::new(bad).is_none(), "{bad}");
        }
        assert_eq!(
            KeySlot::Byom(SlotId::new("ollama").expect("id")).entropy(),
            b"NeuralIA/key/byom-ollama/v1"
        );
        assert_eq!(KeySlot::OpenAi.entropy(), b"NeuralIA/key/openai/v1");
    }

    #[test]
    fn the_vault_opens_only_with_its_own_grants() {
        use neural_core::json_store::{StoreKind, StoreShape, StoreSpec};
        let dir = temp_dir("grants");
        let registry = StoreRegistry::mint_for_test(&dir);
        let other = registry
            .grant(StoreSpec::new(
                "outra",
                StoreKind::Explicit,
                StoreShape::Dir,
            ))
            .expect("grant");
        assert!(KeyVault::open(other, registry.grant(LIVE_KEY_STORE).expect("live")).is_err());
        let keys = registry.grant(KEYS_STORE).expect("keys");
        assert!(KeyVault::open(registry.grant(KEYS_STORE).expect("keys"), keys).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redaction_hides_every_key_shape() {
        const MARK: &str = "[chave omitida]";
        let secrets = [GEMINI_KEY, OPENAI_KEY, ANTHROPIC_KEY];
        let table: Vec<(String, String)> = vec![
            // O que o Live ja cobria.
            (
                format!("wss://x/ws?key={GEMINI_KEY}&alt=json"),
                format!("wss://x/ws?key={MARK}&alt=json"),
            ),
            (
                format!("{{\"key\":\"{GEMINI_KEY}\"}}"),
                format!("{{\"key\":\"{MARK}\"}}"),
            ),
            (
                format!("solta {GEMINI_KEY} no meio"),
                format!("solta {MARK} no meio"),
            ),
            // sk-, sk-proj-, sk-ant- soltas.
            (
                format!("erro 401 com {OPENAI_KEY} fim"),
                format!("erro 401 com {MARK} fim"),
            ),
            (
                concat!("chave sk-", "TESTONLYabcdefghij0123456789 fim").to_string(),
                format!("chave {MARK} fim"),
            ),
            (format!("({ANTHROPIC_KEY})"), format!("({MARK})")),
            // Bearer.
            (
                format!("Authorization: Bearer {OPENAI_KEY}"),
                format!("Authorization: {}", format_args!("Bearer {MARK}")),
            ),
            (
                // Fake JWT ({"alg":"HS256"} . {} . "signature"), split so
                // secret scanners do not read the test literal as a token.
                concat!(
                    "authorization: bearer ",
                    "eyJhbGci",
                    "OiJIUzI1NiJ9.e30.c2lnbmF0dXJl+/="
                )
                .to_string(),
                format!("authorization: bearer {MARK}"),
            ),
            // x-api-key e x-goog-api-key, como cabecalho, em JSON e com =.
            (
                format!("x-api-key: {ANTHROPIC_KEY}"),
                format!("x-api-key: {MARK}"),
            ),
            (
                format!("X-Goog-Api-Key:{GEMINI_KEY}"),
                format!("X-Goog-Api-Key:{MARK}"),
            ),
            (
                "{\"x-api-key\": \"TESTONLY-abcdefgh\"}".to_string(),
                format!("{{\"x-api-key\": \"{MARK}\"}}"),
            ),
            (
                "headers x-goog-api-key=TESTONLY_abcdefgh ok".to_string(),
                format!("headers x-goog-api-key={MARK} ok"),
            ),
        ];
        for (line, expected) in &table {
            let clean = redact_debug_secrets(line);
            assert_eq!(&clean, expected, "{line}");
            for secret in secrets {
                assert!(!clean.contains(&secret[4..20]), "pedaco de chave: {clean}");
            }
        }
        // O resto do log fica igual, e com acentos no meio.
        for plain in [
            "live panel: ligado",
            "resized 1440x900 surface=Comparator",
            "monkey=5 ção AIza curto",
            "task-list-with-many-words-and-more-of-them",
            "risk-free sk-curta Bearer ok",
            "the x-api-key header is missing",
            "x-api-key: curta",
        ] {
            assert!(
                matches!(redact_debug_secrets(plain), Cow::Borrowed(text) if text == plain),
                "{plain}"
            );
        }
    }
}
