use super::*;
use std::collections::VecDeque;
use std::path::Path;

const AUDIO_LIBRARY_BYTES: usize = 64 * 1024 * 1024;
const AUDIO_LIBRARY_ITEMS: usize = 32;
static PENDING_AUDIO_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Bounds events queued while a native save dialog owns the message loop.
#[derive(Debug)]
pub(in crate::windows_app) struct PendingAudio {
    audio: Option<CapturedAudio>,
    bytes: usize,
}

impl PendingAudio {
    pub(in crate::windows_app) fn new(audio: CapturedAudio) -> Option<Self> {
        let bytes = audio.bytes.len();
        reserve_audio_bytes(&PENDING_AUDIO_BYTES, bytes, AUDIO_LIBRARY_BYTES).then(|| Self {
            audio: Some(audio),
            bytes,
        })
    }
}

impl Drop for PendingAudio {
    fn drop(&mut self) {
        PENDING_AUDIO_BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

fn reserve_audio_bytes(used: &AtomicUsize, bytes: usize, limit: usize) -> bool {
    bytes > 0
        && used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(bytes).filter(|next| *next <= limit)
            })
            .is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum AudioPanelRequest {
    List,
    Enabled(bool),
    Export(u64),
    Clear,
}

pub(in crate::windows_app) fn parse_audio_action(
    action: &str,
    args: Option<&serde_json::Value>,
) -> Option<PanelMessage> {
    let request = match action {
        "audio-list" => exact_keys(args, &[]).map(|_| AudioPanelRequest::List),
        "audio-clear" => exact_keys(args, &[]).map(|_| AudioPanelRequest::Clear),
        "audio-enabled" => exact_keys(args, &["on"])?
            .get("on")?
            .as_bool()
            .map(AudioPanelRequest::Enabled),
        "audio-export" => exact_keys(args, &["id"])?
            .get("id")?
            .as_u64()
            .filter(|id| (1..=(1u64 << 53) - 1).contains(id))
            .map(AudioPanelRequest::Export),
        _ => None,
    }?;
    Some(PanelMessage::Audio(request))
}

#[derive(Debug)]
pub(in crate::windows_app) enum AudioEvent {
    Captured(PendingAudio),
    Exported(Result<PathBuf, String>),
}

struct AudioEntry {
    id: u64,
    audio: Arc<CapturedAudio>,
}

pub(in crate::windows_app) struct AudioLibrary {
    pub(in crate::windows_app) enabled: Arc<AtomicBool>,
    entries: VecDeque<AudioEntry>,
    bytes: usize,
    next_id: u64,
    exporting: bool,
    status: String,
}

impl AudioLibrary {
    pub(in crate::windows_app) fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            entries: VecDeque::new(),
            bytes: 0,
            next_id: 1,
            exporting: false,
            status: String::new(),
        }
    }

    fn insert(&mut self, audio: CapturedAudio) -> bool {
        if !self.enabled.load(Ordering::Acquire)
            || audio.bytes.is_empty()
            || audio.bytes.len() > AUDIO_LIBRARY_BYTES
        {
            return false;
        }
        // A repetição da mesma resposta HTTP não ocupa outra linha.
        if self.entries.iter().any(|entry| {
            entry.audio.provider == audio.provider
                && entry.audio.mime == audio.mime
                && entry.audio.bytes == audio.bytes
        }) {
            return false;
        }
        while self.entries.len() >= AUDIO_LIBRARY_ITEMS
            || self.bytes + audio.bytes.len() > AUDIO_LIBRARY_BYTES
        {
            if let Some(old) = self.entries.pop_front() {
                self.bytes -= old.audio.bytes.len();
            }
        }
        self.bytes += audio.bytes.len();
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push_back(AudioEntry {
            id,
            audio: Arc::new(audio),
        });
        self.status =
            "Áudio detectado com a voz original. Escolha Exportar MP3 para salvar.".into();
        true
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.status = "Lista de áudios limpa.".into();
    }

    fn find(&self, id: u64) -> Option<Arc<CapturedAudio>> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| Arc::clone(&entry.audio))
    }

    fn render_script(&self) -> String {
        let items: Vec<_> = self.entries.iter().rev().map(|entry| serde_json::json!({
            "id": entry.id,
            "title": format!("{} · áudio {}", entry.audio.provider, entry.id),
            "detail": format!("{:.1} MB · {}", entry.audio.bytes.len() as f64 / 1048576.0, entry.audio.mime),
            "busy": self.exporting,
        })).collect();
        let data = serde_json::json!({
            "enabled": self.enabled.load(Ordering::Acquire),
            "status": self.status,
            "items": items,
        });
        format!("window.__neuraliaAudio && window.__neuraliaAudio.render({data});")
    }
}

impl App {
    pub(in crate::windows_app) fn audio_event(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Captured(mut pending) => {
                let Some(audio) = pending.audio.take() else {
                    return;
                };
                if self.privacy.mode() == PrivacyMode::Private || !self.audio_library.insert(audio)
                {
                    return;
                }
            }
            AudioEvent::Exported(result) => {
                self.audio_library.exporting = false;
                self.audio_library.status = match result {
                    Ok(path) => format!("MP3 salvo: {}", path.display()),
                    Err(error) => format!("Não foi possível exportar o MP3: {error}"),
                };
            }
        }
        self.render_audio_panel();
    }

    fn render_audio_panel(&mut self) {
        let script = self.audio_library.render_script();
        self.panel_run(script);
    }

    pub(in crate::windows_app) fn sync_audio_capture_state(&self) {
        let on = self.audio_library.enabled.load(Ordering::Acquire)
            && self.privacy.mode() != PrivacyMode::Private;
        if let Some(view) = &self.webview {
            set_audio_capture_enabled(view, on);
        }
        if let Some(comparator) = &self.comparator {
            for view in &comparator.views {
                set_audio_capture_enabled(&view.webview, on);
            }
            if let Some(split) = &comparator.split {
                set_audio_capture_enabled(&split.webview, on && !split.private);
            }
        }
    }

    pub(in crate::windows_app) fn audio_panel_request(&mut self, request: AudioPanelRequest) {
        match request {
            AudioPanelRequest::List => {}
            AudioPanelRequest::Enabled(on) => {
                self.audio_library.enabled.store(on, Ordering::Release);
                self.sync_audio_capture_state();
                self.audio_library.status = if on {
                    "Detecção ativada. Reproduza a leitura em voz alta na página da IA."
                } else {
                    "Detecção pausada. Os áudios já detectados continuam disponíveis."
                }
                .into();
            }
            AudioPanelRequest::Clear => self.audio_library.clear(),
            AudioPanelRequest::Export(id) => {
                if self.audio_library.exporting {
                    return;
                }
                let Some(audio) = self.audio_library.find(id) else {
                    return;
                };
                let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
                    return;
                };
                let folder = self
                    .downloads
                    .manager
                    .settings()
                    .usable_folder(Path::is_dir)
                    .map(Path::to_path_buf)
                    .or_else(user_downloads_folder);
                let name = format!("NeuralIA-{}-{id}.mp3", audio.provider);
                let Some(path) = pick_audio_file(owner, &name, folder.as_deref()) else {
                    return;
                };
                if !path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
                {
                    self.audio_library.status = "Escolha um arquivo com a extensão .mp3.".into();
                    self.render_audio_panel();
                    return;
                }
                self.audio_library.exporting = true;
                self.audio_library.status = "Exportando MP3 com a voz original…".into();
                let proxy = self.proxy.clone();
                if let Err(error) = thread::Builder::new()
                    .name("neural-audio-export".into())
                    .spawn(move || {
                        let result = export_mp3(&audio, &path).map(|_| path);
                        let _ = proxy.send_event(UserEvent::Audio(AudioEvent::Exported(result)));
                    })
                {
                    self.audio_library.exporting = false;
                    self.audio_library.status =
                        format!("Não foi possível iniciar a exportação: {error}");
                }
            }
        }
        self.render_audio_panel();
    }
}

fn pick_audio_file(owner: HWND, name: &str, folder: Option<&Path>) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_EXPLORER, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let filter: Vec<u16> = "Áudio MP3 (*.mp3)\0*.mp3\0\0".encode_utf16().collect();
    let title = wide_null("Exportar áudio original para MP3");
    let extension = wide_null("mp3");
    let directory = folder.map(|path| wide_null(&path.to_string_lossy()));
    let mut buffer = vec![0u16; 32768];
    for (slot, unit) in buffer.iter_mut().zip(name.encode_utf16()) {
        *slot = unit;
    }
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: extension.as_ptr(),
        lpstrInitialDir: directory
            .as_ref()
            .map_or(std::ptr::null(), |dir| dir.as_ptr()),
        Flags: OFN_EXPLORER
            | OFN_OVERWRITEPROMPT
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_HIDEREADONLY,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut dialog) } == 0 {
        return None;
    }
    let len = buffer.iter().position(|unit| *unit == 0)?;
    (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(byte: u8) -> CapturedAudio {
        CapturedAudio {
            provider: "ChatGPT",
            mime: "audio/mpeg".into(),
            bytes: vec![byte; 16],
        }
    }

    #[test]
    fn audio_library_bounds_deduplicates_and_never_reuses_ids() {
        let mut library = AudioLibrary::new();
        assert!(library.insert(clip(1)));
        assert!(!library.insert(clip(1)));
        for n in 2..=40 {
            assert!(library.insert(clip(n)));
        }
        assert_eq!(library.entries.len(), AUDIO_LIBRARY_ITEMS);
        assert_eq!(library.bytes, AUDIO_LIBRARY_ITEMS * 16);
        assert!(library.find(1).is_none());
        assert!(library.find(40).is_some());
        library.clear();
        assert_eq!(library.bytes, 0);
        assert!(library.find(40).is_none());
        assert!(library.insert(clip(1)));
        assert!(library.find(41).is_some());
    }

    #[test]
    fn audio_paused_and_oversized_input_is_not_retained() {
        let mut library = AudioLibrary::new();
        library.enabled.store(false, Ordering::Release);
        assert!(!library.insert(clip(1)));
        library.enabled.store(true, Ordering::Release);
        let mut huge = clip(2);
        huge.bytes.resize(AUDIO_LIBRARY_BYTES + 1, 0);
        assert!(!library.insert(huge));
        assert!(library.entries.is_empty());
    }

    #[test]
    fn audio_panel_accepts_only_typed_ids_and_closed_actions() {
        assert_eq!(
            parse_panel_message(r#"{"action":"audio-export","args":{"id":1}}"#),
            Some(PanelMessage::Audio(AudioPanelRequest::Export(1)))
        );
        for body in [
            r#"{"action":"audio-export","args":{"id":"D:/secret.mp3"}}"#,
            r#"{"action":"audio-export","args":{"id":0}}"#,
            r#"{"action":"audio-export","args":{"id":9007199254740992}}"#,
            r#"{"action":"audio-export","args":{"id":1,"path":"D:/secret.mp3"}}"#,
            r#"{"action":"audio-enabled","args":{"on":1}}"#,
            r#"{"action":"audio-clear","args":{"path":"D:/secret.mp3"}}"#,
            r#"{"action":"audio-delete"}"#,
        ] {
            assert!(parse_panel_message(body).is_none(), "{body}");
        }
    }

    #[test]
    fn audio_event_queue_has_a_byte_budget_before_ui_consumes_it() {
        let used = AtomicUsize::new(0);
        assert!(reserve_audio_bytes(&used, 6, 10));
        assert!(!reserve_audio_bytes(&used, 5, 10));
        assert_eq!(used.load(Ordering::Acquire), 6);
        assert!(reserve_audio_bytes(&used, 4, 10));
        assert!(!reserve_audio_bytes(&used, usize::MAX, 10));
        assert!(!reserve_audio_bytes(&used, 0, 10));
        let before = PENDING_AUDIO_BYTES.load(Ordering::Acquire);
        let mut empty = clip(1);
        empty.bytes.clear();
        assert!(PendingAudio::new(empty).is_none());
        assert_eq!(PENDING_AUDIO_BYTES.load(Ordering::Acquire), before);
        let pending = PendingAudio::new(clip(1)).unwrap();
        assert_eq!(PENDING_AUDIO_BYTES.load(Ordering::Acquire), before + 16);
        drop(pending);
        assert_eq!(PENDING_AUDIO_BYTES.load(Ordering::Acquire), before);
    }
}
