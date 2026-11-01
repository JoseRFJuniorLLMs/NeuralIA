//! Captures provider playback, retaining the original voice. Nothing is written
//! until the user explicitly exports a clip; private WebViews are excluded.
use super::{WebView, WebViewHost, remote_capability};

use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;

pub(in crate::windows_app) const MAX_AUDIO_BYTES: usize = 32 * 1024 * 1024;
const MAX_WIRE_BYTES: usize = 8192;
const MAX_CHUNK_BYTES: usize = 2048;
static NETWORK_READS: AtomicUsize = AtomicUsize::new(0);

struct NetworkPermit;
impl Drop for NetworkPermit {
    fn drop(&mut self) {
        NETWORK_READS.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Clone)]
pub(in crate::windows_app) struct CapturedAudio {
    pub provider: &'static str,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub(in crate::windows_app) fn capture_host(host: WebViewHost) -> bool {
    matches!(
        host,
        WebViewHost::Column(_) | WebViewHost::Split(_) | WebViewHost::External
    )
}

pub(in crate::windows_app) fn provider_for_url(value: &str) -> Option<&'static str> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return None;
    }
    match url.host_str()? {
        "chatgpt.com" | "www.chatgpt.com" | "chat.openai.com" => Some("ChatGPT"),
        "gemini.google.com" => Some("Gemini"),
        "claude.ai" | "www.claude.ai" => Some("Claude"),
        _ => None,
    }
}

fn audio_format(mime: &str) -> Option<&'static str> {
    match mime.split(';').next()?.trim().to_ascii_lowercase().as_str() {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" | "audio/wave" | "audio/x-wav" => Some("wav"),
        "audio/ogg" | "application/ogg" | "audio/opus" => Some("ogg"),
        "audio/webm" => Some("matroska"),
        "audio/mp4" | "audio/x-m4a" => Some("mov"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/aac" | "audio/aacp" => Some("aac"),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePacket {
    v: u8,
    cap: String,
    kind: String,
    id: u32,
    seq: u32,
    total: usize,
    data: String,
}

struct PartialClip {
    id: u32,
    next: u32,
    total: usize,
    bytes: Vec<u8>,
    since: Instant,
}

#[derive(Default)]
struct AudioAssembler(Option<PartialClip>);

impl AudioAssembler {
    fn accept(&mut self, json: &str, cap: &str) -> Option<Vec<u8>> {
        if json.len() > MAX_WIRE_BYTES {
            return None;
        }
        let packet: WirePacket = serde_json::from_str(json).ok()?;
        if packet.v != 1
            || cap.len() != 32
            || !crate::ipc::constant_time_eq(packet.cap.as_bytes(), cap.as_bytes())
            || packet.total < 44
            || packet.total > MAX_AUDIO_BYTES
        {
            return None;
        }
        if self
            .0
            .as_ref()
            .is_some_and(|clip| clip.since.elapsed() > Duration::from_secs(30))
        {
            self.0 = None;
        }
        match packet.kind.as_str() {
            "start" if packet.seq == 0 && packet.data.is_empty() => {
                self.0 = Some(PartialClip {
                    id: packet.id,
                    next: 0,
                    total: packet.total,
                    bytes: Vec::new(),
                    since: Instant::now(),
                });
            }
            "chunk" => {
                let clip = self.0.as_mut()?;
                if packet.id != clip.id
                    || packet.seq != clip.next
                    || packet.total != clip.total
                    || packet.data.is_empty()
                    || packet.data.len() > MAX_CHUNK_BYTES * 2
                    || !packet.data.len().is_multiple_of(2)
                    || clip.bytes.len() + packet.data.len() / 2 > clip.total
                {
                    self.0 = None;
                    return None;
                }
                let decoded = packet
                    .data
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| {
                        let hi = (pair[0] as char).to_digit(16)?;
                        let lo = (pair[1] as char).to_digit(16)?;
                        Some(((hi << 4) | lo) as u8)
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(decoded) = decoded else {
                    self.0 = None;
                    return None;
                };
                clip.bytes.extend_from_slice(&decoded);
                clip.next += 1;
            }
            "end" => {
                let clip = self.0.take()?;
                if packet.id == clip.id
                    && packet.seq == clip.next
                    && packet.total == clip.total
                    && packet.data.is_empty()
                    && clip.bytes.len() == clip.total
                    && valid_pcm_wav(&clip.bytes)
                {
                    return Some(clip.bytes);
                }
            }
            _ => {}
        }
        None
    }
}

fn valid_pcm_wav(bytes: &[u8]) -> bool {
    if bytes.len() < 44
        || &bytes[..4] != b"RIFF"
        || &bytes[8..16] != b"WAVEfmt "
        || &bytes[36..40] != b"data"
    {
        return false;
    }
    let u16_at = |at| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let rate = u32_at(24);
    u32_at(4) as usize == bytes.len() - 8
        && u32_at(16) == 16
        && u16_at(20) == 1
        && u16_at(22) == 1
        && (8_000..=192_000).contains(&rate)
        && u32_at(28) == rate * 2
        && u16_at(32) == 2
        && u16_at(34) == 16
        && u32_at(40) as usize == bytes.len() - 44
        && (bytes.len() - 44).is_multiple_of(2)
}

/// Register once after building an internet WebView. `enabled` must become false
/// in global private mode. `send` hands bounded bytes to the UI's session list.
pub(in crate::windows_app) fn register_audio_capture<F>(
    webview: &WebView,
    host: WebViewHost,
    cap: &str,
    enabled: Arc<AtomicBool>,
    send: F,
) -> Result<(), String>
where
    F: Fn(CapturedAudio) + Clone + 'static,
{
    if !capture_host(host) {
        return Ok(());
    }
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::ICoreWebView2_2, WebResourceResponseReceivedEventHandler,
        WebResourceResponseViewGetContentCompletedHandler, take_pwstr,
    };
    use windows_core::{HSTRING, Interface, PWSTR};
    use wry::WebViewExtWindows;

    install_buffer_capture(webview, cap, enabled.clone(), send.clone())?;
    let handler = WebResourceResponseReceivedEventHandler::create(Box::new(move |sender, args| {
        if !enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        let (Some(sender), Some(args)) = (sender, args) else {
            return Ok(());
        };
        let mut source = PWSTR::null();
        unsafe {
            sender.Source(&mut source)?;
        }
        let Some(provider) = provider_for_url(&take_pwstr(source)) else {
            return Ok(());
        };
        let response = unsafe { args.Response()? };
        let mut status = 0;
        unsafe {
            response.StatusCode(&mut status)?;
        }
        // Partial responses are not complete files and must never be advertised
        // as complete recordings. WebAudio can still capture their decoded data.
        if status != 200 {
            return Ok(());
        }
        let headers = unsafe { response.Headers()? };
        let mut mime = PWSTR::null();
        if unsafe { headers.GetHeader(&HSTRING::from("Content-Type"), &mut mime) }.is_err() {
            return Ok(());
        }
        let mime = take_pwstr(mime);
        if audio_format(&mime).is_none() {
            return Ok(());
        }
        let mut length = PWSTR::null();
        if unsafe { headers.GetHeader(&HSTRING::from("Content-Length"), &mut length) }.is_ok()
            && take_pwstr(length)
                .parse::<usize>()
                .is_ok_and(|n| n > MAX_AUDIO_BYTES)
        {
            return Ok(());
        }
        if NETWORK_READS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .is_err()
        {
            return Ok(());
        }
        // The permit belongs to the completion closure. COM drops it even if a
        // WebView is destroyed before GetContent completes.
        let permit = Rc::new(NetworkPermit);
        let completion_permit = permit.clone();
        let enabled = enabled.clone();
        let send = send.clone();
        let completed = WebResourceResponseViewGetContentCompletedHandler::create(Box::new(
            move |status, stream| {
                let _keep_permit = &completion_permit;
                if status.is_err() || !enabled.load(Ordering::Acquire) {
                    return Ok(());
                }
                let Some(stream) = stream else {
                    return Ok(());
                };
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 64 * 1024];
                loop {
                    let mut read = 0;
                    let status = unsafe {
                        stream.Read(
                            chunk.as_mut_ptr().cast(),
                            chunk.len() as u32,
                            Some(&mut read),
                        )
                    };
                    if status.is_err() || read as usize > chunk.len() {
                        return Ok(());
                    }
                    if read == 0 {
                        break;
                    }
                    if bytes.len() + read as usize > MAX_AUDIO_BYTES {
                        return Ok(());
                    }
                    bytes.extend_from_slice(&chunk[..read as usize]);
                }
                if !bytes.is_empty() && enabled.load(Ordering::Acquire) {
                    send(CapturedAudio {
                        provider,
                        mime: mime.clone(),
                        bytes,
                    });
                }
                Ok(())
            },
        ));
        unsafe {
            response.GetContent(&completed)?;
        }
        Ok(())
    }));
    let core = webview
        .webview()
        .cast::<ICoreWebView2_2>()
        .map_err(|e| e.to_string())?;
    let mut token = 0;
    unsafe { core.add_WebResourceResponseReceived(&handler, &mut token) }.map_err(|e| e.to_string())
}

pub(in crate::windows_app) fn audio_capture_initialization(cap: &str, enabled: bool) -> String {
    include_str!("audio_capture.js")
        .replace("__AUDIO_CAP__", cap)
        .replace("__AUDIO_ENABLED__", if enabled { "true" } else { "false" })
}

fn install_buffer_capture<F>(
    webview: &WebView,
    cap: &str,
    enabled: Arc<AtomicBool>,
    send: F,
) -> Result<(), String>
where
    F: Fn(CapturedAudio) + 'static,
{
    use webview2_com::{WebMessageReceivedEventHandler, take_pwstr};
    use windows_core::PWSTR;
    use wry::WebViewExtWindows;
    let cap = cap.to_string();
    let assembler = RefCell::new(AudioAssembler::default());
    let handler = WebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
        if !enabled.load(Ordering::Acquire) {
            assembler.borrow_mut().0 = None;
            return Ok(());
        }
        let (Some(sender), Some(args)) = (sender, args) else {
            return Ok(());
        };
        let mut source = PWSTR::null();
        unsafe {
            args.Source(&mut source)?;
        }
        let source = take_pwstr(source);
        let Some(provider) = provider_for_url(&source) else {
            return Ok(());
        };
        let mut current = PWSTR::null();
        unsafe {
            sender.Source(&mut current)?;
        }
        if source != take_pwstr(current) {
            assembler.borrow_mut().0 = None;
            return Ok(());
        }
        let mut message = PWSTR::null();
        if unsafe { args.TryGetWebMessageAsString(&mut message) }.is_err() {
            return Ok(());
        }
        if let Some(bytes) = assembler.borrow_mut().accept(&take_pwstr(message), &cap) {
            send(CapturedAudio {
                provider,
                mime: "audio/wav".to_string(),
                bytes,
            });
        }
        Ok(())
    }));
    let core = webview.webview();
    let mut token = 0;
    unsafe { core.add_WebMessageReceived(&handler, &mut token) }.map_err(|e| e.to_string())
}

/// Also call after page load: initialization scripts keep their original
/// snapshot, whereas this message carries the current private/pause state.
pub(in crate::windows_app) fn set_audio_capture_enabled(webview: &WebView, enabled: bool) {
    use windows_core::HSTRING;
    use wry::WebViewExtWindows;
    let json = if enabled {
        r#"{"neuraliaAudioEnabled":true}"#
    } else {
        r#"{"neuraliaAudioEnabled":false}"#
    };
    let _ = unsafe { webview.webview().PostWebMessageAsJson(&HSTRING::from(json)) };
}

fn encoder_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join("ffmpeg.exe");
    if sibling.is_file() {
        return Some(sibling);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("ffmpeg.exe"))
        .find(|path| path.is_file())
}

/// Explicit export only. FFmpeg receives bytes through pipes, cannot fetch
/// remote resources, and never receives page-controlled filenames/options.
/// Conversion failures preserve existing destination files.
pub(in crate::windows_app) fn export_mp3(
    audio: &CapturedAudio,
    output: &Path,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if audio.bytes.is_empty() || audio.bytes.len() > MAX_AUDIO_BYTES {
        return Err("Áudio vazio ou acima do limite de 32 MiB.".to_string());
    }
    let format = audio_format(&audio.mime).ok_or("Formato de áudio não suportado.")?;
    let encoder = encoder_path().ok_or("FFmpeg não encontrado. Instale-o ou coloque ffmpeg.exe ao lado do NeuralIA.exe para exportar MP3.")?;
    let mut child = Command::new(encoder)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-max_alloc",
            "67108864",
            "-protocol_whitelist",
            "pipe",
            "-threads",
            "1",
            "-f",
            format,
            "-i",
            "pipe:0",
            "-map",
            "0:a:0",
            "-vn",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "128k",
            "-f",
            "mp3",
            "pipe:1",
        ])
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Não foi possível iniciar o FFmpeg: {e}"))?;
    let mut input = child
        .stdin
        .take()
        .ok_or("Entrada do FFmpeg indisponível.")?;
    let mut stdout = child.stdout.take().ok_or("Saída do FFmpeg indisponível.")?;
    let source = audio.bytes.clone();
    let writer = std::thread::spawn(move || input.write_all(&source));
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .by_ref()
            .take(MAX_AUDIO_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("A conversão excedeu o prazo ou falhou.");
            }
        }
    };
    let _ = writer.join();
    let bytes = reader
        .join()
        .map_err(|_| "Falha ao receber o MP3.")?
        .map_err(|e| format!("Falha ao receber o MP3: {e}"))?;
    if !status?.success() || bytes.is_empty() || bytes.len() > MAX_AUDIO_BYTES {
        return Err("O áudio não pôde ser convertido em MP3.".to_string());
    }
    let staging = output.with_file_name(format!(".neuralia-audio-{}.tmp", remote_capability()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .map_err(|e| format!("Não foi possível criar o MP3: {e}"))?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&staging);
        return Err(format!("Falha ao salvar o MP3: {error}"));
    }
    drop(file);
    if let Err(error) = std::fs::rename(&staging, output) {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("Não foi possível concluir o MP3: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const CAP: &str = "0123456789abcdef0123456789abcdef";
    fn wav() -> Vec<u8> {
        let mut bytes = b"RIFF\x26\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\x02\0\0\0\0\0".to_vec();
        bytes[4..8].copy_from_slice(&38u32.to_le_bytes());
        bytes
    }
    fn wire(kind: &str, seq: u32, data: String) -> String {
        serde_json::json!({"v":1,"cap":CAP,"kind":kind,"id":7,"seq":seq,"total":46,"data":data})
            .to_string()
    }
    #[test]
    fn audio_origin_and_private_hosts_fail_closed() {
        assert!(capture_host(WebViewHost::Column(0)));
        assert!(!capture_host(WebViewHost::PrivateSplit(0)));
        assert!(!capture_host(WebViewHost::SidePanel));
        for origin in [
            "http://chatgpt.com",
            "https://chatgpt.com.attacker.test",
            "https://chatgpt.com:444",
            "https://user@claude.ai",
            "file:///gemini.google.com",
        ] {
            assert_eq!(provider_for_url(origin), None, "{origin}");
        }
        assert_eq!(
            provider_for_url("https://gemini.google.com/app"),
            Some("Gemini")
        );
    }
    #[test]
    fn audio_transport_requires_cap_sequence_complete_valid_wav() {
        let bytes = wav();
        assert!(valid_pcm_wav(&bytes));
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut state = AudioAssembler::default();
        state.accept(&wire("start", 0, String::new()), CAP);
        assert!(
            state
                .accept(
                    &wire("chunk", 0, hex.clone()),
                    "ffffffffffffffffffffffffffffffff"
                )
                .is_none()
        );
        state.accept(&wire("chunk", 0, hex.clone()), CAP);
        assert_eq!(
            state.accept(&wire("end", 1, String::new()), CAP),
            Some(bytes)
        );
        state.accept(&wire("start", 0, String::new()), CAP);
        state.accept(&wire("chunk", 1, hex), CAP);
        assert!(state.0.is_none());
        assert!(state.accept(&wire("end", 2, String::new()), CAP).is_none());
        let oversized = serde_json::json!({"v":1,"cap":CAP,"kind":"start","id":8,"seq":0,"total":MAX_AUDIO_BYTES+1,"data":""}).to_string();
        state.accept(&oversized, CAP);
        assert!(state.0.is_none());
    }
    #[test]
    fn audio_playback_script_uses_bounded_session_capture() {
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test-audio-capture.mjs");
        let output = Command::new("node")
            .arg(script)
            .output()
            .expect("Node.js for audio playback gate");
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "requires local FFmpeg and FFprobe; run explicitly for export validation"]
    fn audio_export_encodes_real_mp3_and_preserves_destination_on_failure() {
        let dir =
            std::env::temp_dir().join(format!("neuralia-audio-export-{}", remote_capability()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("original.mp3");
        let rate = 16000u32;
        let mut pcm = Vec::new();
        for sample in 0..rate {
            let value = ((sample as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin()
                * 12000.0) as i16;
            pcm.extend_from_slice(&value.to_le_bytes());
        }
        let mut bytes = wav();
        bytes.truncate(44);
        bytes[4..8].copy_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        bytes[40..44].copy_from_slice(&(pcm.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&pcm);
        let clip = CapturedAudio {
            provider: "Gemini",
            mime: "audio/wav".into(),
            bytes,
        };
        std::fs::write(&path, b"old file").unwrap();
        export_mp3(&clip, &path)
            .expect("encode and atomically replace the user-selected destination");
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=codec_name,sample_rate:format=duration",
                "-of",
                "json",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(probe.status.success());
        let metadata: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(metadata["streams"][0]["codec_name"], "mp3");
        assert!(
            metadata["format"]["duration"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
                >= 1.0
        );
        let saved = std::fs::read(&path).unwrap();
        let bad = CapturedAudio {
            provider: "Gemini",
            mime: "audio/wav".into(),
            bytes: b"invalid media".to_vec(),
        };
        assert!(export_mp3(&bad, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
