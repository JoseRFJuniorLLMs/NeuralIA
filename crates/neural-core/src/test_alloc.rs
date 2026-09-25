//! Alocador dos testes do `neural-core`: o `System` com um contador por
//! thread, para os gates medirem o PICO de memória de uma chamada (um livro
//! hostil que multiplica o que aloca). Só existe em `cfg(test)`; o binário que
//! embarca usa o alocador padrão.
//!
//! O contador é por thread (os testes correm em paralelo) e só conta dentro
//! de [`peak_during`]. Memória libertada que foi alocada antes da medição
//! desce abaixo de zero em vez de esconder alocações seguintes.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

pub(crate) struct CountingAlloc;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static CURRENT: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn note(delta: isize) {
    // `try_with`: no fim de uma thread o TLS já pode ter ido.
    let _ = TRACKING.try_with(|tracking| {
        if !tracking.get() {
            return;
        }
        let _ = CURRENT.try_with(|current| {
            let now = current.get().saturating_add(delta);
            current.set(now);
            let _ = PEAK.try_with(|peak| {
                if now > peak.get() {
                    peak.set(now);
                }
            });
        });
    });
}

// SAFETY: delega tudo ao `System`; só conta os tamanhos.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: o mesmo contrato do chamador.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            note(layout.size() as isize);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: o mesmo contrato do chamador.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            note(layout.size() as isize);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: o mesmo contrato do chamador.
        unsafe { System.dealloc(ptr, layout) };
        note(-(layout.size() as isize));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: o mesmo contrato do chamador.
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            note(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

/// Corre `work` nesta thread e devolve o que ele devolveu e o pico de bytes
/// vivos (acima do ponto de partida) durante a chamada.
pub(crate) fn peak_during<T>(work: impl FnOnce() -> T) -> (T, usize) {
    CURRENT.with(|current| current.set(0));
    PEAK.with(|peak| peak.set(0));
    TRACKING.with(|tracking| tracking.set(true));
    let out = work();
    TRACKING.with(|tracking| tracking.set(false));
    let peak = PEAK.with(Cell::get);
    (out, peak.max(0) as usize)
}
