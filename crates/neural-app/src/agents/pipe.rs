//! O canal entre as pontes e o hub: um named pipe local do Windows.
//!
//! - Nome: `\\.\pipe\neuralia-agents-<id>`, com um `id` de 128 bits novo a
//!   cada arranque do NeuralIA. Quem squattasse um nome antigo nao acerta no
//!   seguinte.
//! - Criado com `FILE_FLAG_FIRST_PIPE_INSTANCE` (se o nome ja existir, o hub
//!   nao arranca em vez de partilhar o canal) e `PIPE_REJECT_REMOTE_CLIENTS`.
//! - Descritor de seguranca: dono = o utilizador atual; DACL protegida com
//!   duas entradas so, o utilizador atual e o SYSTEM
//!   (`O:<sid>D:P(A;;FA;;;SY)(A;;FA;;;<sid>)`). Outros utilizadores da mesma
//!   maquina nem abrem o canal.
//! - A primeira linha de cada ligacao tem de trazer o token de 256 bits de
//!   `<data_dir>/agents/token`; sem ele a ligacao fecha
//!   ([`HubSession`]). O `token` e o `pipe` (o id) sao escritos de forma
//!   atomica com o mesmo descritor: so este utilizador os le.
//! - A ponte so entrega o token a um canal cujo DONO e o utilizador atual:
//!   um canal criado por outra conta (squatting do nome) nao recebe nada.
//! - Uma instancia do canal por ponte, uma thread por ligacao.
//! - `hub.lock`, aberto sem partilha enquanto o NeuralIA corre: uma segunda
//!   janela do NeuralIA nao abre outro hub sobre as mesmas conversas.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE, GetLastError,
    HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT, SE_OBJECT_TYPE,
};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FlushFileBuffers, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, READ_CONTROL, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::hub::{
    AgentHub, HUB_MAX_REPLY_BYTES, HUB_MAX_REQUEST_BYTES, HUB_PROTOCOL_VERSION, HubServerState,
    HubSession, TOKEN_HEX_LEN,
};
use super::mcp::{HubLink, LinkError};
use super::{Line, read_line_capped};

pub(crate) const PIPE_PREFIX: &str = r"\\.\pipe\neuralia-agents-";
pub(crate) const PIPE_ID_HEX_LEN: usize = 32;
pub(crate) const TOKEN_FILE: &str = "token";
pub(crate) const PIPE_FILE: &str = "pipe";
const LOCK_FILE: &str = "hub.lock";
const MAX_PIPE_INSTANCES: u32 = 16;
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const BUSY_WAIT_MS: u32 = 2_000;

/// Um HANDLE do Windows que se fecha sozinho.
pub(crate) struct OwnedHandle(HANDLE);

// SAFETY: um HANDLE de ficheiro/pipe pode ser usado de qualquer thread; o
// I/O sincrono do Windows serializa as operacoes sobre o mesmo objeto.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: o handle e nosso e so se fecha aqui.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// As duas pontas de uma ligacao pelo canal (leitura e escrita partilham o
/// mesmo handle; cada ponta so e usada por uma thread de cada vez).
#[derive(Clone)]
struct Pipe(Arc<OwnedHandle>);

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0u32;
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: buffer valido com o tamanho declarado; I/O sincrono.
        let ok = unsafe {
            ReadFile(
                self.0.raw(),
                buf.as_mut_ptr(),
                len,
                &mut read,
                core::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return Ok(read as usize);
        }
        // SAFETY: le o erro da thread atual.
        let error = unsafe { GetLastError() };
        if error == ERROR_BROKEN_PIPE {
            return Ok(0);
        }
        Err(io::Error::from_raw_os_error(error as i32))
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut written = 0u32;
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: buffer valido com o tamanho declarado; I/O sincrono.
        let ok = unsafe {
            WriteFile(
                self.0.raw(),
                buf.as_ptr(),
                len,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// SAFETY: `text` aponta para uma string UTF-16 terminada em zero.
unsafe fn from_wide(text: *const u16) -> String {
    let mut len = 0usize;
    // SAFETY: percorre ate ao zero terminal, que o chamador garante.
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
    }
}

fn last_error(what: &str) -> String {
    format!("{what}: {}", io::Error::last_os_error())
}

/// `bytes` aleatorios do CSPRNG do Windows, em hexadecimal. Falha fechado.
fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    // SAFETY: buffer valido; sem handle de algoritmo, a flag usa o RNG do
    // sistema.
    let status = unsafe {
        BCryptGenRandom(
            core::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(format!("BCryptGenRandom falhou ({status:#x})"));
    }
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(bytes * 2);
    for byte in buf {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// SID do utilizador deste processo (`S-1-5-21-...`).
pub(crate) fn current_user_sid() -> Result<String, String> {
    // SAFETY: token aberto e fechado aqui; o buffer do TOKEN_USER vive ate a
    // conversao do SID em texto.
    unsafe {
        let mut token: HANDLE = core::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        let token = OwnedHandle(token);
        let mut needed = 0u32;
        GetTokenInformation(
            token.raw(),
            TokenUser,
            core::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 {
            return Err(last_error("GetTokenInformation"));
        }
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        if GetTokenInformation(
            token.raw(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            return Err(last_error("GetTokenInformation"));
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        sid_to_string(user.User.Sid)
    }
}

/// SAFETY: `sid` e um SID valido.
unsafe fn sid_to_string(sid: PSID) -> Result<String, String> {
    let mut text: *mut u16 = core::ptr::null_mut();
    // SAFETY: o chamador garante o SID; o texto e libertado com LocalFree.
    unsafe {
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(last_error("ConvertSidToStringSidW"));
        }
        let value = from_wide(text);
        LocalFree(text as HLOCAL);
        Ok(value)
    }
}

/// Um descritor de seguranca alocado pelo Windows (`LocalFree` no fim).
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: memoria so lida depois de criada; nunca alterada.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: alocado pelo Windows com LocalAlloc.
        unsafe {
            LocalFree(self.0 as HLOCAL);
        }
    }
}

/// O SDDL do canal e dos ficheiros: dono e DACL so para `sid` e o SYSTEM.
pub(crate) fn user_only_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;;FA;;;SY)(A;;FA;;;{sid})")
}

fn user_only_descriptor(sid: &str) -> Result<SecurityDescriptor, String> {
    let sddl = wide_null(&user_only_sddl(sid));
    let mut descriptor: PSECURITY_DESCRIPTOR = core::ptr::null_mut();
    // SAFETY: SDDL terminado em zero; o descritor e libertado no Drop.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW",
        ));
    }
    Ok(SecurityDescriptor(descriptor))
}

fn attributes(descriptor: &SecurityDescriptor) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

/// Le o descritor de seguranca de um objeto aberto, em SDDL (dono + DACL).
#[cfg(test)]
pub(crate) fn handle_sddl(handle: HANDLE, object: SE_OBJECT_TYPE) -> Result<String, String> {
    use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut descriptor: PSECURITY_DESCRIPTOR = core::ptr::null_mut();
    // SAFETY: handle valido com READ_CONTROL; descritor libertado no Drop.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            object,
            info,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(format!(
            "GetSecurityInfo: {}",
            io::Error::from_raw_os_error(status as i32)
        ));
    }
    let descriptor = SecurityDescriptor(descriptor);
    let mut text: *mut u16 = core::ptr::null_mut();
    // SAFETY: descritor valido; texto libertado com LocalFree.
    unsafe {
        if ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            info,
            &mut text,
            core::ptr::null_mut(),
        ) == 0
        {
            return Err(last_error(
                "ConvertSecurityDescriptorToStringSecurityDescriptorW",
            ));
        }
        let value = from_wide(text);
        LocalFree(text as HLOCAL);
        Ok(value)
    }
}

/// SID do dono de um objeto aberto.
fn handle_owner_sid(handle: HANDLE, object: SE_OBJECT_TYPE) -> Result<String, String> {
    let mut owner: PSID = core::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = core::ptr::null_mut();
    // SAFETY: handle valido com READ_CONTROL; o SID do dono aponta para
    // dentro do descritor, que so e libertado depois da conversao.
    unsafe {
        let status = GetSecurityInfo(
            handle,
            object,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            &mut descriptor,
        );
        if status != 0 {
            return Err(format!(
                "GetSecurityInfo: {}",
                io::Error::from_raw_os_error(status as i32)
            ));
        }
        let descriptor = SecurityDescriptor(descriptor);
        let result = sid_to_string(owner);
        drop(descriptor);
        result
    }
}

/// A ponte so fala com um canal cujo dono e `expected_sid` (o utilizador
/// dela). Um canal criado por outra conta com o mesmo nome nao recebe o
/// token.
fn verify_pipe_owner(handle: HANDLE, expected_sid: &str) -> Result<(), LinkError> {
    let owner = handle_owner_sid(handle, SE_KERNEL_OBJECT).map_err(LinkError::Rejected)?;
    if owner == expected_sid {
        Ok(())
    } else {
        Err(LinkError::Rejected(
            "o canal do NeuralIA não pertence a este usuário do Windows".into(),
        ))
    }
}

/// Escreve um ficheiro pequeno que so este utilizador pode ler, de uma vez:
/// temporario criado com o descritor, `MoveFileEx` por cima do antigo.
fn write_private_file(
    path: &Path,
    contents: &str,
    descriptor: &SecurityDescriptor,
) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "nome de ficheiro inválido".to_string())?;
    let tmp = path.with_file_name(format!("{name}.{}.tmp", random_hex(4)?));
    let wide_tmp = wide_path(&tmp);
    let security = attributes(descriptor);
    // SAFETY: caminho terminado em zero; o handle fecha-se no Drop.
    let handle = unsafe {
        CreateFileW(
            wide_tmp.as_ptr(),
            GENERIC_WRITE,
            0,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            core::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW"));
    }
    let file = OwnedHandle(handle);
    let mut pipe = Pipe(Arc::new(file));
    let written = pipe
        .write_all(contents.as_bytes())
        .map_err(|error| error.to_string())
        // SAFETY: handle de escrita valido.
        .and_then(|()| {
            if unsafe { FlushFileBuffers(pipe.0.raw()) } == 0 {
                Err(last_error("FlushFileBuffers"))
            } else {
                Ok(())
            }
        });
    drop(pipe);
    if let Err(error) = written {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    let wide_path = wide_path(path);
    // SAFETY: dois caminhos terminados em zero.
    let moved = unsafe {
        MoveFileExW(
            wide_tmp.as_ptr(),
            wide_path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        let error = last_error("MoveFileExW");
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

fn create_instance(
    name: &str,
    descriptor: &SecurityDescriptor,
    first: bool,
) -> Result<OwnedHandle, String> {
    let wide = wide_null(name);
    let security = attributes(descriptor);
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: nome terminado em zero; descritor vivo durante a chamada.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            MAX_PIPE_INSTANCES,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            0,
            &security,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateNamedPipeW"));
    }
    Ok(OwnedHandle(handle))
}

/// Abre o canal como cliente (a ponte, e os testes).
fn open_client(name: &str) -> Result<OwnedHandle, LinkError> {
    let wide = wide_null(name);
    for attempt in 0..2 {
        // SAFETY: nome terminado em zero. SECURITY_IDENTIFICATION: o hub pode
        // saber quem somos, mas nunca agir como nos.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
                0,
                core::ptr::null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                core::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(OwnedHandle(handle));
        }
        // SAFETY: le o erro da thread atual.
        let error = unsafe { GetLastError() };
        match error {
            ERROR_PIPE_BUSY if attempt == 0 => {
                // SAFETY: nome terminado em zero.
                unsafe {
                    WaitNamedPipeW(wide.as_ptr(), BUSY_WAIT_MS);
                }
            }
            ERROR_FILE_NOT_FOUND => {
                return Err(LinkError::NotRunning(
                    "o canal do NeuralIA não existe".into(),
                ));
            }
            ERROR_ACCESS_DENIED => {
                return Err(LinkError::Rejected("acesso negado ao canal".into()));
            }
            other => {
                return Err(LinkError::NotRunning(format!(
                    "não foi possível abrir o canal: {}",
                    io::Error::from_raw_os_error(other as i32)
                )));
            }
        }
    }
    Err(LinkError::NotRunning(
        "o canal do NeuralIA está ocupado".into(),
    ))
}

/// O canal do hub, pronto a aceitar pontes.
pub(crate) struct HubServer {
    pipe_name: String,
    token: String,
    descriptor: SecurityDescriptor,
    first: Option<OwnedHandle>,
    _lock: File,
}

/// A thread que aceita pontes. Largar isto nao a para (corre ate o processo
/// sair); `stop` para (usado pelos testes).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ServerHandle {
    stop: Arc<AtomicBool>,
    pipe_name: String,
    join: Option<JoinHandle<()>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl ServerHandle {
    pub(crate) fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Acorda o ConnectNamedPipe com uma ligacao que nao diz nada.
        let _ = open_client(&self.pipe_name);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl HubServer {
    /// Cria o canal e escreve `token` e `pipe` em `dir`.
    pub(crate) fn bind(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(dir.join(LOCK_FILE))
            .map_err(|error| {
                if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) {
                    "Outra janela do NeuralIA já recebe os agentes.".to_string()
                } else {
                    format!("hub.lock: {error}")
                }
            })?;
        let sid = current_user_sid()?;
        let descriptor = user_only_descriptor(&sid)?;
        let id = random_hex(PIPE_ID_HEX_LEN / 2)?;
        let token = random_hex(TOKEN_HEX_LEN / 2)?;
        let pipe_name = format!("{PIPE_PREFIX}{id}");
        let first = create_instance(&pipe_name, &descriptor, true)?;
        // O canal ja existe quando os ficheiros aparecem.
        write_private_file(&dir.join(TOKEN_FILE), &token, &descriptor)?;
        write_private_file(&dir.join(PIPE_FILE), &id, &descriptor)?;
        Ok(Self {
            pipe_name,
            token,
            descriptor,
            first: Some(first),
            _lock: lock,
        })
    }

    #[cfg(test)]
    pub(crate) fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    /// Aceita pontes numa thread propria; cada ligacao tem a sua thread.
    pub(crate) fn spawn(self, hub: AgentHub) -> Result<ServerHandle, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let pipe_name = self.pipe_name.clone();
        let thread_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("neural-agents-accept".into())
            .spawn(move || self.accept_loop(&hub, &thread_stop))
            .map_err(|error| error.to_string())?;
        Ok(ServerHandle {
            stop,
            pipe_name,
            join: Some(join),
        })
    }

    fn accept_loop(mut self, hub: &AgentHub, stop: &AtomicBool) {
        let mut next = self.first.take();
        loop {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let instance = match next.take() {
                Some(instance) => instance,
                None => match create_instance(&self.pipe_name, &self.descriptor, false) {
                    Ok(instance) => instance,
                    Err(error) => {
                        // Todas as instancias ocupadas: espera que uma saia.
                        eprintln!("[agents] {error}");
                        thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                },
            };
            // SAFETY: instancia valida; I/O sincrono.
            let connected = unsafe {
                ConnectNamedPipe(instance.raw(), core::ptr::null_mut()) != 0
                    || GetLastError() == ERROR_PIPE_CONNECTED
            };
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if !connected {
                continue;
            }
            let pipe = Pipe(Arc::new(instance));
            let hub = hub.clone();
            let token = self.token.clone();
            let spawned = thread::Builder::new()
                .name("neural-agents-bridge".into())
                .spawn(move || serve_connection(pipe, hub, token));
            if let Err(error) = spawned {
                eprintln!("[agents] ligação recusada: {error}");
            }
        }
    }
}

fn serve_connection(pipe: Pipe, hub: AgentHub, token: String) {
    let mut session = HubSession::new(hub, token);
    let mut reader = BufReader::new(pipe.clone());
    let mut writer = pipe;
    loop {
        let reply = match read_line_capped(&mut reader, HUB_MAX_REQUEST_BYTES) {
            Ok(Some(Line::Text(bytes))) => {
                let line = String::from_utf8_lossy(&bytes);
                session.handle_line(line.trim_end_matches('\r'))
            }
            Ok(Some(Line::TooLong)) => super::hub::SessionReply {
                line: json!({"ok": false, "error": "Pedido grande demais."}).to_string(),
                close: true,
            },
            Ok(None) | Err(_) => break,
        };
        let mut line = reply.line.into_bytes();
        line.push(b'\n');
        if writer.write_all(&line).is_err() || reply.close {
            break;
        }
    }
    // A sessao fecha as perguntas desta ponte; o handle fecha-se sem
    // DisconnectNamedPipe, para a ponte ainda ler a ultima resposta.
    drop(session);
}

/// Arranca o hub do NeuralIA aberto: le as conversas e abre o canal, fora da
/// thread da janela.
pub(crate) fn start(hub: AgentHub) {
    let spawned = thread::Builder::new()
        .name("neural-agents-hub".into())
        .spawn(move || {
            hub.load();
            match HubServer::bind(&hub.dir()).and_then(|server| server.spawn(hub.clone())) {
                Ok(_running) => hub.set_server_state(HubServerState::Running),
                Err(error) => {
                    eprintln!("[agents] canal indisponível: {error}");
                    hub.set_server_state(HubServerState::Unavailable(error));
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("[agents] hub não arrancou: {error}");
    }
}

/// A ligacao da ponte ao hub, pelo canal.
pub(crate) struct PipeLink {
    dir: PathBuf,
    agent: String,
    conn: Option<ClientConn>,
}

struct ClientConn {
    reader: BufReader<Pipe>,
    writer: Pipe,
}

impl ClientConn {
    fn send(&mut self, request: &Value) -> io::Result<()> {
        let mut line = request.to_string().into_bytes();
        line.push(b'\n');
        self.writer.write_all(&line)
    }

    fn receive(&mut self) -> io::Result<Value> {
        match read_line_capped(&mut self.reader, HUB_MAX_REPLY_BYTES)? {
            Some(Line::Text(bytes)) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Some(Line::TooLong) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "resposta grande demais",
            )),
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "o NeuralIA fechou a ligação",
            )),
        }
    }
}

fn read_small_file(path: &Path, len: usize) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit())).then(|| text.to_string())
}

impl PipeLink {
    pub(crate) fn new(dir: PathBuf, agent: String) -> Self {
        Self {
            dir,
            agent,
            conn: None,
        }
    }

    fn connect(&mut self) -> Result<(), LinkError> {
        let Some(id) = read_small_file(&self.dir.join(PIPE_FILE), PIPE_ID_HEX_LEN) else {
            return Err(LinkError::NotRunning("sem registo do canal".into()));
        };
        let Some(token) = read_small_file(&self.dir.join(TOKEN_FILE), TOKEN_HEX_LEN) else {
            return Err(LinkError::NotRunning("sem token".into()));
        };
        let handle = open_client(&format!("{PIPE_PREFIX}{id}"))?;
        let me = current_user_sid().map_err(LinkError::Rejected)?;
        verify_pipe_owner(handle.raw(), &me)?;
        let pipe = Pipe(Arc::new(handle));
        let mut conn = ClientConn {
            reader: BufReader::new(pipe.clone()),
            writer: pipe,
        };
        let hello = json!({
            "t": "hello",
            "v": HUB_PROTOCOL_VERSION,
            "token": token,
            "agent": self.agent,
        });
        let reply = conn
            .send(&hello)
            .and_then(|()| conn.receive())
            .map_err(|error| LinkError::Broken(error.to_string()))?;
        if reply.get("ok").and_then(Value::as_bool) != Some(true) {
            let reason = reply
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("recusado");
            return Err(LinkError::Rejected(reason.to_string()));
        }
        self.conn = Some(conn);
        Ok(())
    }
}

impl HubLink for PipeLink {
    fn exchange(&mut self, request: &Value) -> Result<Value, LinkError> {
        self.ensure_connected()?;
        let sent = self.conn.as_mut().expect("connected").send(request);
        if sent.is_err() {
            // A escrita falhou: o pedido nao saiu (o NeuralIA reiniciou, por
            // exemplo). Repete-se uma vez numa ligacao nova.
            self.conn = None;
            self.ensure_connected()?;
            if let Err(error) = self.conn.as_mut().expect("connected").send(request) {
                self.conn = None;
                return Err(LinkError::Broken(error.to_string()));
            }
        }
        match self.conn.as_mut().expect("connected").receive() {
            Ok(reply) => Ok(reply),
            Err(error) => {
                self.conn = None;
                Err(LinkError::Broken(error.to_string()))
            }
        }
    }

    fn ensure_connected(&mut self) -> Result<(), LinkError> {
        if self.conn.is_some() {
            return Ok(());
        }
        self.connect()
    }

    fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    fn close(&mut self) {
        self.conn = None;
    }
}

/// O processo tem stdin e stdout com quem falar. Um executavel do subsistema
/// Windows lancado sem redirecionamento recebe handles nulos.
pub(crate) fn stdio_present() -> bool {
    [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE]
        .into_iter()
        .all(|which| {
            // SAFETY: consulta sem efeitos.
            let handle = unsafe { GetStdHandle(which) };
            !handle.is_null() && handle != INVALID_HANDLE_VALUE
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::hub::tests::{ManualClock, fixture};
    use crate::agents::hub::{AgentEvent, QuestionAnswer};
    use crate::agents::store::tests::{TempDir, open_store};
    use crate::agents::{RecordBody, SharedContext};
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::Instant;
    use windows_sys::Win32::Security::Authorization::SE_FILE_OBJECT;

    /// `O:<dono>D:<flags>(ace)(ace)...` -> (dono, flags, [(tipo, direitos, sid)]).
    fn parse_sddl(sddl: &str) -> (String, String, Vec<(String, String, String)>) {
        let rest = sddl.strip_prefix("O:").expect("owner first");
        let (owner, dacl) = rest.split_once("D:").expect("dacl present");
        let flags_end = dacl.find('(').unwrap_or(dacl.len());
        let flags = dacl[..flags_end].to_string();
        let aces = dacl[flags_end..]
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(")(")
            .filter(|ace| !ace.is_empty())
            .map(|ace| {
                let fields: Vec<&str> = ace.split(';').collect();
                assert_eq!(fields.len(), 6, "{ace}");
                (
                    fields[0].to_string(),
                    fields[2].to_string(),
                    fields[5].to_string(),
                )
            })
            .collect();
        (owner.to_string(), flags, aces)
    }

    fn assert_user_only(sddl: &str, me: &str) {
        let (owner, flags, aces) = parse_sddl(sddl);
        assert_eq!(owner, me, "{sddl}");
        assert!(flags.contains('P'), "DACL must be protected: {sddl}");
        let mut trustees: Vec<&str> = aces.iter().map(|(_, _, sid)| sid.as_str()).collect();
        trustees.sort_unstable();
        let mut expected = vec!["SY", me];
        expected.sort_unstable();
        assert_eq!(trustees, expected, "{sddl}");
        for (kind, rights, _) in &aces {
            assert_eq!(kind, "A", "{sddl}");
            assert!(rights == "FA" || rights == "0x1f01ff", "{sddl}");
        }
    }

    fn hello(token: &str) -> Value {
        json!({"t":"hello","v":1,"token":token,"agent":"claude"})
    }

    fn raw_client(name: &str) -> ClientConn {
        let pipe = Pipe(Arc::new(
            open_client(name).unwrap_or_else(|e| panic!("{e:?}")),
        ));
        ClientConn {
            reader: BufReader::new(pipe.clone()),
            writer: pipe,
        }
    }

    #[test]
    fn hub_pipe_and_token_files_are_owned_by_and_open_only_to_this_user() {
        let dir = TempDir::new("pipe-dacl");
        let server = HubServer::bind(&dir.0).unwrap();
        let me = current_user_sid().unwrap();
        assert!(me.starts_with("S-1-"), "{me}");
        assert_eq!(
            user_only_sddl(&me),
            format!("O:{me}D:P(A;;FA;;;SY)(A;;FA;;;{me})")
        );
        // O que um cliente ve ao abrir o canal.
        let client = open_client(server.pipe_name()).unwrap_or_else(|e| panic!("{e:?}"));
        let sddl = handle_sddl(client.raw(), SE_KERNEL_OBJECT).unwrap();
        assert_user_only(&sddl, &me);
        drop(client);
        // Os ficheiros do token e do id.
        let token = fs::read_to_string(dir.0.join(TOKEN_FILE)).unwrap();
        let id = fs::read_to_string(dir.0.join(PIPE_FILE)).unwrap();
        assert_eq!(token.len(), TOKEN_HEX_LEN);
        assert_eq!(id.len(), PIPE_ID_HEX_LEN);
        assert!(
            token
                .bytes()
                .chain(id.bytes())
                .all(|b| b.is_ascii_hexdigit())
        );
        assert_eq!(server.pipe_name(), format!("{PIPE_PREFIX}{id}"));
        for name in [TOKEN_FILE, PIPE_FILE] {
            let file = File::open(dir.0.join(name)).unwrap();
            let sddl = handle_sddl(file.as_raw_handle(), SE_FILE_OBJECT).unwrap();
            assert_user_only(&sddl, &me);
        }
        // Nada de temporarios deixados para tras.
        let leftovers: Vec<String> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn bridge_refuses_a_pipe_owned_by_another_account() {
        let dir = TempDir::new("pipe-owner");
        let server = HubServer::bind(&dir.0).unwrap();
        let client = open_client(server.pipe_name()).unwrap_or_else(|e| panic!("{e:?}"));
        let me = current_user_sid().unwrap();
        assert!(verify_pipe_owner(client.raw(), &me).is_ok());
        // O mesmo canal visto por outra conta (SYSTEM, LocalService).
        for other in ["S-1-5-18", "S-1-5-19"] {
            assert!(
                matches!(
                    verify_pipe_owner(client.raw(), other),
                    Err(LinkError::Rejected(_))
                ),
                "{other}"
            );
        }
    }

    #[test]
    fn hub_pipe_rejects_a_wrong_or_missing_token_and_serves_the_right_one() {
        let f = fixture("pipe-token");
        let dir = f.hub.dir();
        let server = HubServer::bind(&dir).unwrap();
        let running = server.spawn(f.hub.clone()).unwrap();
        let token = fs::read_to_string(dir.join(TOKEN_FILE)).unwrap();
        let mut wrong = token.clone().into_bytes();
        wrong[10] = if wrong[10] == b'a' { b'b' } else { b'a' };
        let wrong = String::from_utf8(wrong).unwrap();

        for bad in [
            hello(&wrong),
            json!({"t":"hello","v":1,"agent":"claude"}),
            json!({"t":"call","tool":"send_message","args":{"text":"sem hello"}}),
        ] {
            let mut conn = raw_client(running.pipe_name());
            conn.send(&bad).unwrap();
            let reply = conn.receive().unwrap();
            assert_eq!(reply["ok"], false, "{bad} -> {reply}");
            // E o hub fechou a ligacao.
            assert!(conn.receive().is_err());
        }
        assert!(f.hub.conversation("claude").is_empty());
        assert!(f.events.lock().unwrap().is_empty());

        let mut conn = raw_client(running.pipe_name());
        conn.send(&hello(&token)).unwrap();
        assert_eq!(conn.receive().unwrap()["ok"], true);
        conn.send(&json!({"t":"call","tool":"send_message","args":{"text":"pelo canal"}}))
            .unwrap();
        assert_eq!(conn.receive().unwrap()["value"]["delivered"], true);
        drop(conn);
        // Desligar a ponte chega ao hub.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, AgentEvent::Disconnected { .. }))
        {
            assert!(
                Instant::now() < deadline,
                "disconnect never reached the hub"
            );
            thread::sleep(Duration::from_millis(10));
        }
        running.stop();
    }

    #[test]
    fn pipe_link_reaches_the_hub_and_says_not_running_without_it() {
        let f = fixture("pipe-link");
        let dir = f.hub.dir();
        // Sem nada na pasta: NeuralIA fechado, e depressa.
        let started = Instant::now();
        let mut link = PipeLink::new(dir.clone(), "codex".into());
        assert!(matches!(
            link.exchange(&json!({"t":"ping"})),
            Err(LinkError::NotRunning(_))
        ));
        // Ficheiros de um arranque antigo, canal ja inexistente.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(PIPE_FILE), "0".repeat(PIPE_ID_HEX_LEN)).unwrap();
        fs::write(dir.join(TOKEN_FILE), "0".repeat(TOKEN_HEX_LEN)).unwrap();
        assert!(matches!(
            link.exchange(&json!({"t":"ping"})),
            Err(LinkError::NotRunning(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(3));

        let server = HubServer::bind(&dir).unwrap();
        let running = server.spawn(f.hub.clone()).unwrap();
        let reply = link
            .exchange(
                &json!({"t":"call","tool":"set_status","args":{"text":"a ler","progress":10}}),
            )
            .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(link.is_connected());
        assert!(
            f.events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, AgentEvent::Connected { agent } if agent == "codex"))
        );
        // Uma segunda janela nao abre outro hub na mesma pasta.
        let second = HubServer::bind(&dir);
        assert!(second.is_err_and(|e| e.contains("Outra janela")));
        running.stop();
    }

    /// Ponta a ponta: o `NeuralIA.exe --mcp` que embarca, conduzido por um
    /// cliente MCP em Node (`scripts/test-agents-mcp.mjs --with-hub`), fala
    /// com este hub pelo canal real. O teste faz de utilizador: responde as
    /// perguntas pelo `answer_question` do painel. Lanca o executavel (sem
    /// janela): so no CI, como os gates de foco.
    #[test]
    #[ignore = "launches NeuralIA.exe --mcp: runs in CI"]
    fn shipped_bridge_exe_speaks_mcp_to_the_hub_over_the_pipe() {
        let exe = std::env::var_os("NEURALIA_TEST_EXE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("NeuralIA.exe")
            });
        assert!(
            exe.exists(),
            "{} nao existe: corra `cargo test -p neural-app` (os testes de integracao obrigam o cargo a construir o binario)",
            exe.display()
        );
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test-agents-mcp.mjs");
        let root = TempDir::new("e2e");
        let data_dir = root.0.join("data");
        let (_registry, store) = open_store(&data_dir);
        let clock = ManualClock::new();
        let hub_slot: Arc<Mutex<Option<AgentHub>>> = Arc::new(Mutex::new(None));
        let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let hub = {
            let hub_slot = Arc::clone(&hub_slot);
            let events = Arc::clone(&events);
            let clock_for_user = clock.clone();
            AgentHub::with_clock(
                store,
                Arc::new(move |event: AgentEvent| {
                    // O «utilizador» deste teste.
                    if let AgentEvent::Question(card) = &event {
                        let hub = hub_slot.lock().unwrap().clone().unwrap();
                        if card.text.contains("responda Sim") {
                            hub.answer_question(card.id, QuestionAnswer::Button(0))
                                .unwrap();
                        } else if card.text.contains("responda por texto") {
                            hub.answer_question(
                                card.id,
                                QuestionAnswer::Text("olá do teste".into()),
                            )
                            .unwrap();
                        } else if card.text.contains("deixe expirar") {
                            clock_for_user.advance(Duration::from_secs(3_600));
                        }
                    }
                    events.lock().unwrap().push(event);
                }),
                clock.clock(),
            )
        };
        *hub_slot.lock().unwrap() = Some(hub.clone());
        hub.load();
        hub.user_message("claude", "mensagem do usuário").unwrap();
        hub.share_context(
            "claude",
            SharedContext::Page {
                title: "Página partilhada".into(),
                url: "https://example.com/partilhada".into(),
                excerpt: None,
            },
        )
        .unwrap();
        let running = HubServer::bind(&data_dir.join("agents"))
            .unwrap()
            .spawn(hub.clone())
            .unwrap();

        let output = Command::new("node")
            .arg(&script)
            .arg("--with-hub")
            .arg("--exe")
            .arg(&exe)
            .env("NEURALIA_DATA_DIR", &data_dir)
            .stdin(Stdio::null())
            .output()
            .expect("node precisa de estar no PATH para este gate");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "node falhou ({:?}):\n{stdout}\n{stderr}",
            output.status
        );
        assert!(stdout.contains("agents-mcp: ok"), "{stdout}\n{stderr}");

        // O que o hub viu, pela ordem.
        let conversation = hub.conversation("claude");
        let has = |pick: &dyn Fn(&RecordBody) -> bool| conversation.iter().any(|r| pick(&r.body));
        assert!(has(
            &|b| matches!(b, RecordBody::AgentMessage { text, .. } if text == "Olá do Node")
        ));
        assert!(has(
            &|b| matches!(b, RecordBody::UserAnswer { text, .. } if text == "Sim")
        ));
        assert!(has(
            &|b| matches!(b, RecordBody::UserAnswer { text, .. } if text == "olá do teste")
        ));
        assert!(has(&|b| matches!(
            b,
            RecordBody::QuestionClosed {
                reason: crate::agents::CloseReason::TimedOut,
                ..
            }
        )));
        let events = events.lock().unwrap().clone();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Connected { agent } if agent == "claude"))
        );
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::Status { status: Some(s), .. } if s.progress == Some(42))
        ));
        // A ponte saiu com o stdin: o hub viu a ligacao cair.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !hub.snapshot().agents.iter().all(|a| !a.connected) {
            assert!(Instant::now() < deadline, "bridge never disconnected");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(hub.pending_questions().is_empty());
        running.stop();
    }
}
