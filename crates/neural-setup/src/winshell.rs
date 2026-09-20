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

#[cfg(test)]
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Foundation::{HANDLE, S_OK};
#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
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
            return Err(format!("IPersistFile indisponivel: 0x{hr:08x}"));
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

// So os testes chamam isto -- e e essa a razao de existir.
#[allow(dead_code)]
/// Le de volta tudo o que o `create_shortcut` grava. Existe para o teste de
/// ida e volta cobrir **todos** os slots que usamos: um slot trocado nao da
/// erro nenhum, da lixo, e so se ve comparando o que saiu com o que entrou.
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
            return Err("IPersistFile indisponivel".into());
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

const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\NeuralIA";

/// Escreve a entrada de "Aplicacoes e funcionalidades" no ramo do utilizador.
/// HKCU, e nao HKLM: a instalacao e do utilizador e nao pediu elevacao.
pub fn register_uninstall(
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
            wide(UNINSTALL_KEY).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if status != 0 {
            return Err(format!("nao consegui criar a chave: {status}"));
        }

        let quoted = format!("\"{}\"", uninstaller.display());
        let text_values: [(&str, String); 7] = [
            ("DisplayName", "NeuralIA".to_string()),
            ("DisplayVersion", version.to_string()),
            ("Publisher", "Jose Ribamar Ferreira Junior".to_string()),
            ("DisplayIcon", icon.display().to_string()),
            ("InstallLocation", root.display().to_string()),
            ("UninstallString", quoted.clone()),
            ("QuietUninstallString", format!("{quoted} /S")),
        ];
        for (name, value) in &text_values {
            let data = wide(value);
            RegSetValueExW(
                key,
                wide(name).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            );
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
        Ok(())
    }
}

pub fn unregister_uninstall() {
    unsafe {
        RegDeleteTreeW(HKEY_CURRENT_USER, wide(UNINSTALL_KEY).as_ptr());
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume_serial: u32,
    file_index: u64,
}

#[cfg(test)]
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
