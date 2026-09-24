//! Le o icone que um executavel ja compilado traz nos recursos, para os testes
//! do NeuralIA.exe e do instalador provarem que o `build.rs` meteu la o
//! `assets/logo.ico` -- e nao um icone antigo, nem nenhum.
//!
//! Precisa do `brand` (o parser do GRPICONDIR) declarado na raiz do crate.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{
    FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
    LoadResource, LockResource, SizeofResource,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{RT_GROUP_ICON, RT_ICON};

use crate::brand::{GroupEntry, parse_group_icon};

/// O grupo de icone `group_id` do executavel, com os bytes de cada RT_ICON.
pub fn group_icon(exe: &Path, group_id: u16) -> Result<Vec<(GroupEntry, Vec<u8>)>, String> {
    let wide: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    // Como ficheiro de dados: nada do executavel corre, so se leem recursos.
    let module = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    };
    if module.is_null() {
        return Err(format!("nao abriu {}", exe.display()));
    }
    let resource = |id: u16, kind: *const u16| -> Option<Vec<u8>> {
        unsafe {
            let found = FindResourceW(module, id as usize as *const u16, kind);
            if found.is_null() {
                return None;
            }
            let loaded = LoadResource(module, found);
            let data = LockResource(loaded) as *const u8;
            let size = SizeofResource(module, found) as usize;
            (!data.is_null() && size > 0).then(|| std::slice::from_raw_parts(data, size).to_vec())
        }
    };
    let result = (|| {
        let group = resource(group_id, RT_GROUP_ICON)
            .ok_or_else(|| format!("{} nao tem o RT_GROUP_ICON {group_id}", exe.display()))?;
        parse_group_icon(&group)?
            .into_iter()
            .map(|entry| {
                let data = resource(entry.id, RT_ICON)
                    .ok_or_else(|| format!("RT_ICON {} em falta", entry.id))?;
                Ok((entry, data))
            })
            .collect()
    })();
    unsafe { FreeLibrary(module) };
    result
}
