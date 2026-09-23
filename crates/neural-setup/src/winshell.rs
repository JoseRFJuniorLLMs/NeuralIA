//! O que so existe no Windows: pastas conhecidas, atalhos `.lnk` e a chave de
//! desinstalacao.
//!
//! Os atalhos passam pelo `IShellLinkW`, que o `windows-sys` nao expoe -- so
//! traz funcoes, nao interfaces COM. A vtable esta declarada a mao aqui em
//! baixo. A ordem dos campos **e** a ABI: trocar dois deles chama o metodo
//! errado com os argumentos errados, e o teste de ida e volta existe por isso.

#![cfg(windows)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, S_OK};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegEnumValueW, RegGetValueW,
    RegOpenKeyExW, RegSetValueExW,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs, SHGetKnownFolderPath};
use windows_sys::core::GUID;

const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

/// A vtable do `IShellLinkW`. Os campos sao slots: a posicao e que manda, o
/// nome e so para nos. Os que nao chamamos ficam opacos de proposito -- um
/// ponteiro que nao se usa nao se pode usar mal.
#[repr(C)]
struct ShellLinkVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_path: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut c_void, u32) -> i32,
    get_id_list: *const c_void,
    set_id_list: *const c_void,
    get_description: unsafe extern "system" fn(*mut c_void, *mut u16, i32) -> i32,
    set_description: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    get_working_directory: unsafe extern "system" fn(*mut c_void, *mut u16, i32) -> i32,
    set_working_directory: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    get_arguments: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    set_arguments: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    get_hotkey: *const c_void,
    set_hotkey: *const c_void,
    get_show_cmd: *const c_void,
    set_show_cmd: *const c_void,
    get_icon_location: *const c_void,
    set_icon_location: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32,
    set_relative_path: *const c_void,
    resolve: *const c_void,
    set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
}

#[repr(C)]
struct PersistFileVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_class_id: *const c_void,
    is_dirty: *const c_void,
    load: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> i32,
    save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32,
    save_completed: *const c_void,
    get_cur_file: *const c_void,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    wide(&path.to_string_lossy())
}

/// COM por thread. Chamar duas vezes na mesma thread e inofensivo.
pub fn init_com() {
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let mut raw: *mut u16 = std::ptr::null_mut();
        if SHGetKnownFolderPath(id, 0, std::ptr::null_mut::<c_void>() as HANDLE, &mut raw) != S_OK
            || raw.is_null()
        {
            return None;
        }
        let mut len = 0usize;
        while *raw.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(raw, len));
        CoTaskMemFree(raw as *const c_void);
        Some(PathBuf::from(text))
    }
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.
pub fn start_menu_programs() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs)
}

pub fn desktop() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop)
}

/// Cria (ou substitui) um atalho `.lnk`.
pub fn create_shortcut(
    link: &Path,
    target: &Path,
    working_dir: &Path,
    description: &str,
    icon: &Path,
) -> Result<(), String> {
    unsafe {
        let mut raw: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_SHELL_LINK,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_ISHELL_LINK_W,
            &mut raw,
        );
        if hr != S_OK || raw.is_null() {
            return Err(format!("CoCreateInstance(ShellLink) falhou: 0x{hr:08x}"));
        }
        let vtbl = *(raw as *mut *mut ShellLinkVtbl);
        let guard = Released(raw, (*vtbl).release);

        let checks = [
            ((*vtbl).set_path)(raw, wide_path(target).as_ptr()),
            ((*vtbl).set_working_directory)(raw, wide_path(working_dir).as_ptr()),
            ((*vtbl).set_description)(raw, wide(description).as_ptr()),
            ((*vtbl).set_icon_location)(raw, wide_path(icon).as_ptr(), 0),
        ];
        if let Some(bad) = checks.iter().find(|hr| **hr != S_OK) {
            return Err(format!("IShellLinkW recusou: 0x{bad:08x}"));
        }

        let mut file: *mut c_void = std::ptr::null_mut();
        let hr = ((*vtbl).query_interface)(raw, &IID_IPERSIST_FILE, &mut file);
        if hr != S_OK || file.is_null() {
            return Err(format!("IPersistFile indisponível: 0x{hr:08x}"));
        }
        let file_vtbl = *(file as *mut *mut PersistFileVtbl);
        let file_guard = Released(file, (*file_vtbl).release);

        if let Some(parent) = link.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let hr = ((*file_vtbl).save)(file, wide_path(link).as_ptr(), 1);
        drop(file_guard);
        drop(guard);
        if hr != S_OK {
            return Err(format!("gravar {} falhou: 0x{hr:08x}", link.display()));
        }
        Ok(())
    }
}

/// O alvo, a descricao e a pasta de trabalho gravados num `.lnk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutDetails {
    pub target: PathBuf,
    pub description: String,
    pub working_dir: PathBuf,
}

/// Le de volta tudo o que o `create_shortcut` grava. A desinstalacao usa-o
/// para so apagar um atalho que aponte para a pasta que esta a remover; e o
/// teste de ida e volta usa-o para cobrir **todos** os slots que usamos: um
/// slot trocado nao da erro nenhum, da lixo, e so se ve comparando o que saiu
/// com o que entrou.
pub fn shortcut_details(link: &Path) -> Result<ShortcutDetails, String> {
    unsafe {
        let mut raw: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_SHELL_LINK,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_ISHELL_LINK_W,
            &mut raw,
        );
        if hr != S_OK || raw.is_null() {
            return Err(format!("CoCreateInstance falhou: 0x{hr:08x}"));
        }
        let vtbl = *(raw as *mut *mut ShellLinkVtbl);
        let guard = Released(raw, (*vtbl).release);

        let mut file: *mut c_void = std::ptr::null_mut();
        if ((*vtbl).query_interface)(raw, &IID_IPERSIST_FILE, &mut file) != S_OK || file.is_null() {
            return Err("IPersistFile indisponível".into());
        }
        let file_vtbl = *(file as *mut *mut PersistFileVtbl);
        let file_guard = Released(file, (*file_vtbl).release);

        // STGM_READ = 0
        let hr = ((*file_vtbl).load)(file, wide_path(link).as_ptr(), 0);
        if hr != S_OK {
            return Err(format!("ler {} falhou: 0x{hr:08x}", link.display()));
        }

        let mut buffer = [0u16; 1024];
        // SLGP_RAWPATH = 4: o caminho tal como foi gravado, sem o Windows o
        // tentar resolver noutro sitio.
        let hr = ((*vtbl).get_path)(
            raw,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
            std::ptr::null_mut(),
            4,
        );
        if hr < 0 {
            drop(file_guard);
            drop(guard);
            return Err(format!("GetPath falhou: 0x{hr:08x}"));
        }
        let target = PathBuf::from(read_wide(&buffer));

        let mut buffer = [0u16; 1024];
        ((*vtbl).get_description)(raw, buffer.as_mut_ptr(), buffer.len() as i32);
        let description = read_wide(&buffer);

        let mut buffer = [0u16; 1024];
        ((*vtbl).get_working_directory)(raw, buffer.as_mut_ptr(), buffer.len() as i32);
        let working_dir = PathBuf::from(read_wide(&buffer));

        drop(file_guard);
        drop(guard);
        Ok(ShortcutDetails {
            target,
            description,
            working_dir,
        })
    }
}

fn read_wide(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

/// Larga a referencia COM mesmo que se saia a meio por erro.
struct Released(*mut c_void, unsafe extern "system" fn(*mut c_void) -> u32);

impl Drop for Released {
    fn drop(&mut self) {
        unsafe {
            (self.1)(self.0);
        }
    }
}

/// As duas linhas de comandos que o Windows corre para desinstalar: a normal
/// (Definicoes > Aplicacoes) e a silenciosa (`winget uninstall --silent`,
/// scripts). O desinstalador e o mesmo executavel que o instalador, por isso
/// sem `--uninstall` abria o ecra de instalar.
pub fn uninstall_commands(uninstaller: &Path) -> (String, String) {
    let command = format!("\"{}\" --uninstall", uninstaller.display());
    (command.clone(), format!("{command} /S"))
}

/// Escreve a entrada de "Aplicacoes e funcionalidades" no ramo do utilizador,
/// na chave `key_path` (dentro do HKCU). HKCU, e nao HKLM: a instalacao e do
/// utilizador e nao pediu elevacao.
pub fn register_uninstall(
    key_path: &str,
    root: &Path,
    uninstaller: &Path,
    icon: &Path,
    version: &str,
    size_kb: u32,
) -> Result<(), String> {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(key_path).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if status != 0 {
            return Err(format!("não foi possível criar a chave: {status}"));
        }

        let (uninstall, quiet_uninstall) = uninstall_commands(uninstaller);
        let text_values: [(&str, String); 7] = [
            ("DisplayName", "NeuralIA".to_string()),
            ("DisplayVersion", version.to_string()),
            ("Publisher", "Jose Ribamar Ferreira Junior".to_string()),
            ("DisplayIcon", icon.display().to_string()),
            ("InstallLocation", root.display().to_string()),
            ("UninstallString", uninstall),
            ("QuietUninstallString", quiet_uninstall),
        ];
        // Uma entrada sem versao ou sem `UninstallString` e uma entrada que
        // mente em "Aplicacoes": a falha de um valor e a falha do registo.
        let mut failed = None;
        for (name, value) in &text_values {
            let data = wide(value);
            let status = RegSetValueExW(
                key,
                wide(name).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            );
            if status != 0 {
                failed.get_or_insert(format!("não foi possível gravar {name}: {status}"));
            }
        }
        for (name, value) in [
            ("EstimatedSize", size_kb),
            ("NoModify", 1u32),
            ("NoRepair", 1u32),
        ] {
            RegSetValueExW(
                key,
                wide(name).as_ptr(),
                0,
                REG_DWORD,
                &value as *const u32 as *const u8,
                4,
            );
        }
        RegCloseKey(key);
        failed.map_or(Ok(()), Err)
    }
}

/// Apaga a chave `key_path` (dentro do HKCU), ela e tudo o que tem dentro.
pub fn delete_key(key_path: &str) {
    unsafe {
        RegDeleteTreeW(HKEY_CURRENT_USER, wide(key_path).as_ptr());
    }
}

/// Os valores de uma chave, tal como estavam: nome, tipo e bytes. E o que a
/// instalacao guarda antes de escrever a sua entrada, para a poder repor se
/// um passo depois falhar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySnapshot {
    values: Vec<(Vec<u16>, u32, Vec<u8>)>,
}

/// Le todos os valores de `key_path` (dentro do HKCU). `None` se a chave nao
/// existe. As entradas de "Aplicativos" nao tem subchaves; so os valores
/// contam.
pub fn snapshot_key(key_path: &str) -> Option<KeySnapshot> {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(key_path).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        ) != 0
        {
            return None;
        }
        let mut values = Vec::new();
        let mut name = vec![0u16; 16_384];
        let mut data = vec![0u8; 64 * 1024];
        for index in 0.. {
            let mut name_len = name.len() as u32;
            let mut data_len = data.len() as u32;
            let mut kind = 0u32;
            let status = RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &mut name_len,
                std::ptr::null(),
                &mut kind,
                data.as_mut_ptr(),
                &mut data_len,
            );
            if status != 0 {
                // ERROR_NO_MORE_ITEMS, ou um valor maior do que 64 KiB, que
                // uma entrada de desinstalacao nao tem.
                break;
            }
            values.push((
                name[..name_len as usize].to_vec(),
                kind,
                data[..data_len as usize].to_vec(),
            ));
        }
        RegCloseKey(key);
        Some(KeySnapshot { values })
    }
}

/// Poe `key_path` como estava: sem chave se nao existia, ou exatamente com os
/// valores guardados.
pub fn restore_key(key_path: &str, snapshot: Option<&KeySnapshot>) {
    delete_key(key_path);
    let Some(snapshot) = snapshot else {
        return;
    };
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(key_path).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        ) != 0
        {
            return;
        }
        for (name, kind, data) in &snapshot.values {
            let name: Vec<u16> = name.iter().copied().chain([0]).collect();
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                *kind,
                data.as_ptr(),
                data.len() as u32,
            );
        }
        RegCloseKey(key);
    }
}

/// Apaga a chave `key_path` so se ja nao tiver subchaves. So os testes o usam,
/// para nao deixar a raiz das pastas de ensaio vazia no registo de quem os
/// corre.
#[cfg(test)]
pub fn delete_empty_key(key_path: &str) {
    unsafe {
        windows_sys::Win32::System::Registry::RegDeleteKeyW(
            HKEY_CURRENT_USER,
            wide(key_path).as_ptr(),
        );
    }
}

/// A chave existe no HKCU?
pub fn key_exists(key_path: &str) -> bool {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(key_path).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        );
        if status == 0 {
            RegCloseKey(key);
            true
        } else {
            false
        }
    }
}

/// Um valor de texto de uma chave do HKCU, se existir e nao estiver vazio.
pub fn read_string(key_path: &str, name: &str) -> Option<String> {
    unsafe {
        let key = wide(key_path);
        let value = wide(name);
        let mut bytes = 0u32;
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        );
        if status != 0 || bytes < 2 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as *mut c_void,
            &mut bytes,
        );
        if status != 0 {
            return None;
        }
        let text = read_wide(&buffer);
        (!text.is_empty()).then_some(text)
    }
}

/// A pasta que uma entrada de "Aplicacoes" diz ser a da instalacao. O Inno
/// escreve-a com `\` no fim; tira-se, para a pasta se mostrar como as outras.
pub fn registered_location(key_path: &str) -> Option<PathBuf> {
    let text = read_string(key_path, "InstallLocation")?;
    let trimmed = text.trim_end_matches(['\\', '/']);
    // `C:\` sem a barra seria `C:`, que e outra coisa: fica como veio.
    let text = if trimmed.ends_with(':') || trimmed.is_empty() {
        text.as_str()
    } else {
        trimmed
    };
    Some(PathBuf::from(text))
}

/// A entrada que o Inno das 2.1.x deixou, se ainda existir.
pub fn inno_registration(key_path: &str) -> Option<crate::install::InnoRegistration> {
    key_exists(key_path).then(|| crate::install::InnoRegistration {
        install_location: registered_location(key_path),
        uninstall_string: read_string(key_path, "UninstallString"),
    })
}

/// Escreve um valor de texto numa chave do HKCU, criando-a. So os testes o
/// usam: e como semeiam uma entrada do Inno numa pasta de ensaio.
#[cfg(test)]
pub fn write_string(key_path: &str, name: &str, value: &str) {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(key_path).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        assert_eq!(status, 0, "criar {key_path}");
        let data = wide(value);
        RegSetValueExW(
            key,
            wide(name).as_ptr(),
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        );
        RegCloseKey(key);
    }
}

/// O que identifica um ficheiro ou pasta no disco, seja qual for o nome por
/// que se chega la: o volume e o indice do NTFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume_serial: u32,
    file_index: u64,
}

fn file_identity(path: &Path) -> Result<FileIdentity, String> {
    let path = wide_path(path);
    unsafe {
        let handle = CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "abrir caminho para identidade falhou: {}",
                std::io::Error::last_os_error()
            ));
        }

        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        let ok = GetFileInformationByHandle(handle, info.as_mut_ptr());
        let close = CloseHandle(handle);
        if ok == 0 {
            return Err(format!(
                "GetFileInformationByHandle falhou: {}",
                std::io::Error::last_os_error()
            ));
        }
        if close == 0 {
            return Err(format!(
                "CloseHandle falhou: {}",
                std::io::Error::last_os_error()
            ));
        }

        let info = info.assume_init();
        Ok(FileIdentity {
            volume_serial: info.dwVolumeSerialNumber,
            file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        })
    }
}

#[cfg(test)]
fn same_filesystem_object(left: &Path, right: &Path) -> Result<bool, String> {
    Ok(file_identity(left)? == file_identity(right)?)
}

/// `a` e `b` sao o mesmo ficheiro? Pela identidade no disco quando os dois
/// existem: `C:\Users\RUNNER~1\x` e `C:\Users\runneradmin\x` sao o mesmo, e o
/// `IShellLink` devolve sempre o nome longo, seja qual for o que se gravou.
/// Se um deles ja nao existe, a pasta de cada um decide (com o mesmo nome de
/// ficheiro); sem nenhuma das duas, o texto.
pub fn same_file(a: &Path, b: &Path) -> bool {
    if let (Ok(left), Ok(right)) = (file_identity(a), file_identity(b)) {
        return left == right;
    }
    if let (Some(a_dir), Some(b_dir), Some(a_name), Some(b_name)) =
        (a.parent(), b.parent(), a.file_name(), b.file_name())
        && let (Ok(left), Ok(right)) = (file_identity(a_dir), file_identity(b_dir))
    {
        return left == right
            && a_name.to_string_lossy().to_lowercase() == b_name.to_string_lossy().to_lowercase();
    }
    crate::install::same_path(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prova que a vtable esta alinhada: gravamos um alvo e lemo-lo de volta
    /// pelo slot `GetPath`. Com um campo trocado, isto devolve lixo ou estoira
    /// -- que e exatamente o modo de falha que uma ABI escrita a mao tem.
    #[test]
    fn a_shortcut_round_trips_through_the_hand_written_vtable() {
        init_com();
        let dir = std::env::temp_dir().join(format!("neuralia-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("pasta");
        let target = dir.join("NeuralIA.exe");
        std::fs::write(&target, b"MZ").expect("alvo");
        let link = dir.join("NeuralIA.lnk");

        create_shortcut(&link, &target, &dir, "Navegador NeuralIA", &target).expect("criar atalho");
        assert!(link.is_file(), "o .lnk nao foi gravado");

        let read = shortcut_details(&link).expect("ler atalho");
        assert!(
            same_filesystem_object(&read.target, &target).expect("identidade do alvo"),
            "o atalho aponta para outro sitio: {} != {}",
            read.target.display(),
            target.display()
        );
        assert_eq!(
            read.description, "Navegador NeuralIA",
            "a descricao saiu por outro slot"
        );
        assert!(
            same_filesystem_object(&read.working_dir, &dir).expect("identidade da pasta"),
            "a pasta de trabalho saiu por outro slot: {} != {}",
            read.working_dir.display(),
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shortcut_to_another_executable_is_not_the_same_destination() {
        init_com();
        let dir =
            std::env::temp_dir().join(format!("neuralia-lnk-negative-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("pasta");

        let expected = dir.join("NeuralIA.exe");
        let other = dir.join("Outro.exe");
        std::fs::write(&expected, b"MZ expected").expect("alvo esperado");
        std::fs::write(&other, b"MZ other").expect("outro alvo");

        let link = dir.join("Outro.lnk");
        create_shortcut(&link, &other, &dir, "Outro executavel", &other)
            .expect("criar atalho para outro executavel");

        let read = shortcut_details(&link).expect("ler atalho");
        assert!(
            same_filesystem_object(&read.target, &other).expect("identidade do outro alvo"),
            "o atalho de controle deve apontar para Outro.exe"
        );
        assert!(
            !same_filesystem_object(&read.target, &expected).expect("comparar alvos distintos"),
            "um atalho para Outro.exe nunca pode ser aceito como NeuralIA.exe"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_folders_the_shortcuts_go_to_exist() {
        init_com();
        let programs = start_menu_programs().expect("menu iniciar");
        assert!(programs.is_dir(), "{} nao existe", programs.display());
        assert!(desktop().is_some_and(|d| d.is_dir()));
    }
}
