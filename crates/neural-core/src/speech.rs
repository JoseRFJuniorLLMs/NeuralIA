//! Leitura em Voz Alta Neural via protocolo Edge TTS (SPEC-0116).
//!
//! Módulo determinístico do neural-core para geração de SSML, configuração
//! de vozes neurais e parsing de envelopes do protocolo WebSocket do Edge TTS.

use serde::{Deserialize, Serialize};

use crate::Result;

/// Token de cliente confiável do protocolo Edge TTS do Microsoft Edge.
pub const EDGE_TTS_TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";

/// Endpoint oficial do WebSocket de síntese do Edge.
pub const EDGE_TTS_WSS_URL: &str =
    "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";

/// Codec e taxa de amostragem padrão recomendada para baixa latência.
pub const DEFAULT_AUDIO_OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// Vozes neurais com suporte nativo e entonação natural.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeVoice {
    FranciscaNeuralPtBr,
    AntonioNeuralPtBr,
    ThalitaNeuralPtBr,
    AriaNeuralEnUs,
    GuyNeuralEnUs,
}

impl EdgeVoice {
    pub fn identifier(self) -> &'static str {
        match self {
            EdgeVoice::FranciscaNeuralPtBr => "pt-BR-FranciscaNeural",
            EdgeVoice::AntonioNeuralPtBr => "pt-BR-AntonioNeural",
            EdgeVoice::ThalitaNeuralPtBr => "pt-BR-ThalitaNeural",
            EdgeVoice::AriaNeuralEnUs => "en-US-AriaNeural",
            EdgeVoice::GuyNeuralEnUs => "en-US-GuyNeural",
        }
    }

    pub fn lang(self) -> &'static str {
        match self {
            EdgeVoice::FranciscaNeuralPtBr
            | EdgeVoice::AntonioNeuralPtBr
            | EdgeVoice::ThalitaNeuralPtBr => "pt-BR",
            EdgeVoice::AriaNeuralEnUs | EdgeVoice::GuyNeuralEnUs => "en-US",
        }
    }
}

/// Configurações de síntese de voz (velocidade, tom e volume).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechConfig {
    pub voice: EdgeVoice,
    /// Ajuste de taxa (ex: "+0%", "+15%", "-10%").
    pub rate: String,
    /// Ajuste de pitch (ex: "+0Hz", "+5Hz").
    pub pitch: String,
    /// Ajuste de volume (ex: "+0%", "-20%").
    pub volume: String,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            voice: EdgeVoice::FranciscaNeuralPtBr,
            rate: "+0%".to_string(),
            pitch: "+0Hz".to_string(),
            volume: "+0%".to_string(),
        }
    }
}

/// Escapa caracteres especiais de XML para inclusão segura no SSML.
pub fn escape_xml_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Constrói o payload SSML XML estrito para a síntese neural.
pub fn build_ssml(text: &str, config: &SpeechConfig) -> String {
    let escaped = escape_xml_text(text.trim());
    let voice = config.voice.identifier();
    let lang = config.voice.lang();
    let rate = &config.rate;
    let pitch = &config.pitch;
    let volume = &config.volume;

    format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{lang}'>\
           <voice name='{voice}'>\
             <prosody pitch='{pitch}' rate='{rate}' volume='{volume}'>\
               {escaped}\
             </prosody>\
           </voice>\
         </speak>"
    )
}

/// Constrói o envelope de texto inicial `speech.config` para o handshake do protocolo.
pub fn build_speech_config_message() -> String {
    format!(
        "Content-Type: application/json; charset=utf-8\r\n\
         Path: speech.config\r\n\r\n\
         {{\"context\":{{\"synthesis\":{{\"audio\":{{\"outputFormat\":\"{DEFAULT_AUDIO_OUTPUT_FORMAT}\"}}}}}}}}"
    )
}

/// Constrói o envelope de texto `ssml` com o Request-ID e SSML.
pub fn build_ssml_message(request_id: &str, ssml: &str) -> String {
    format!(
        "X-RequestId: {request_id}\r\n\
         Content-Type: application/ssml+xml\r\n\
         Path: ssml\r\n\r\n\
         {ssml}"
    )
}

/// Metadado de contorno e sincronização palavra a palavra (*WordBoundary*).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordBoundary {
    /// Deslocamento temporal em nanossegundos/ticks.
    pub audio_offset: u64,
    /// Duração do fonema/palavra.
    pub duration: u64,
    /// Palavra sintetizada.
    pub text: String,
}

/// Tipo de mensagem recebida no canal de síntese.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeTtsResponse {
    TurnStart,
    TurnEnd,
    WordBoundary(WordBoundary),
    AudioChunk(Vec<u8>),
    Other(String),
}

/// Parseia um frame de texto retornado pelo WebSocket do Edge TTS.
pub fn parse_edge_text_frame(frame: &str) -> Result<EdgeTtsResponse> {
    let Some(pos) = frame.find("Path:") else {
        return Ok(EdgeTtsResponse::Other("unknown".to_string()));
    };
    let after_path = pos + "Path:".len();
    let path_line = if frame.is_char_boundary(after_path) {
        &frame[after_path..]
    } else {
        ""
    };
    let path = path_line.lines().next().unwrap_or("").trim();
    match path {
        "turn.start" => Ok(EdgeTtsResponse::TurnStart),
        "turn.end" => Ok(EdgeTtsResponse::TurnEnd),
        "audio.metadata" => {
            let Some(body_start) = frame.find("\r\n\r\n").or_else(|| frame.find("\n\n")) else {
                return Ok(EdgeTtsResponse::Other(path.to_string()));
            };
            let body = frame[body_start..].trim();
            let Ok(val) = serde_json::from_str::<serde_json::Value>(body) else {
                return Ok(EdgeTtsResponse::Other(path.to_string()));
            };
            let Some(meta) = val.get("Metadata").and_then(|m| m.as_array()) else {
                return Ok(EdgeTtsResponse::Other(path.to_string()));
            };
            for item in meta {
                let is_wb = item.get("Type").and_then(|t| t.as_str()) == Some("WordBoundary");
                let data_opt = item.get("Data");
                if let (true, Some(data)) = (is_wb, data_opt) {
                    let offset = data.get("Offset").and_then(|v| v.as_u64()).unwrap_or(0);
                    let duration = data.get("Duration").and_then(|v| v.as_u64()).unwrap_or(0);
                    let text = data
                        .get("text")
                        .and_then(|t| t.get("Text"))
                        .and_then(|txt| txt.as_str())
                        .unwrap_or("")
                        .to_string();
                    return Ok(EdgeTtsResponse::WordBoundary(WordBoundary {
                        audio_offset: offset,
                        duration,
                        text,
                    }));
                }
            }
            Ok(EdgeTtsResponse::Other(path.to_string()))
        }
        _ => Ok(EdgeTtsResponse::Other(path.to_string())),
    }
}

/// Versão zero-alloc que retorna uma fatia dos bytes brutos de áudio sem alocar `Vec<u8>`.
pub fn parse_edge_binary_frame_slice(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 2 {
        return None;
    }
    let header_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    if bytes.len() < 2 + header_len {
        return None;
    }
    let header_str = std::str::from_utf8(&bytes[2..2 + header_len]).unwrap_or("");
    if header_str.contains("Path:audio") {
        Some(&bytes[2 + header_len..])
    } else {
        None
    }
}

/// Parseia um frame binário do Edge TTS contendo chunks de áudio MP3 (SPEC-0116 §4.2).
/// O envelope oficial da Microsoft traz:
/// - 2 bytes big-endian: tamanho do cabeçalho de metadados em texto (`H`).
/// - `H` bytes do cabeçalho de texto (`Path:audio\r\n...`).
/// - Bytes restantes: dados brutos de áudio (MP3).
pub fn parse_edge_binary_frame(bytes: &[u8]) -> Option<Vec<u8>> {
    parse_edge_binary_frame_slice(bytes).map(Vec::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_xml_text() {
        let input = "Texto com <tags> & 'aspas' ou \"aspas duplas\"";
        let escaped = escape_xml_text(input);
        assert_eq!(
            escaped,
            "Texto com &lt;tags&gt; &amp; &apos;aspas&apos; ou &quot;aspas duplas&quot;"
        );
    }

    #[test]
    fn test_build_ssml() {
        let config = SpeechConfig {
            voice: EdgeVoice::FranciscaNeuralPtBr,
            rate: "+10%".to_string(),
            pitch: "+2Hz".to_string(),
            volume: "+0%".to_string(),
        };
        let ssml = build_ssml("Olá mundo & bem-vindos!", &config);
        assert!(ssml.contains("xml:lang='pt-BR'"));
        assert!(ssml.contains("voice name='pt-BR-FranciscaNeural'"));
        assert!(ssml.contains("pitch='+2Hz' rate='+10%' volume='+0%'"));
        assert!(ssml.contains("Olá mundo &amp; bem-vindos!"));
    }

    #[test]
    fn test_protocol_messages() {
        let conf_msg = build_speech_config_message();
        assert!(conf_msg.contains("Path: speech.config"));
        assert!(conf_msg.contains(DEFAULT_AUDIO_OUTPUT_FORMAT));

        let ssml_msg = build_ssml_message("req-1234", "<speak></speak>");
        assert!(ssml_msg.contains("X-RequestId: req-1234"));
        assert!(ssml_msg.contains("Path: ssml"));
        assert!(ssml_msg.contains("<speak></speak>"));
    }

    #[test]
    fn test_parse_edge_text_frame_turn() {
        let frame_start = "X-RequestId: abc\r\nPath: turn.start\r\n\r\n";
        assert_eq!(
            parse_edge_text_frame(frame_start).unwrap(),
            EdgeTtsResponse::TurnStart
        );

        let frame_end = "X-RequestId: abc\r\nPath: turn.end\r\n\r\n";
        assert_eq!(
            parse_edge_text_frame(frame_end).unwrap(),
            EdgeTtsResponse::TurnEnd
        );
    }

    #[test]
    fn test_parse_edge_text_frame_word_boundary() {
        let metadata_frame = "X-RequestId: 123\r\nPath: audio.metadata\r\n\r\n\
        {\"Metadata\":[{\"Type\":\"WordBoundary\",\"Data\":{\"Offset\":50000,\"Duration\":250000,\"text\":{\"Text\":\"Palavra\"}}}]}";

        let resp = parse_edge_text_frame(metadata_frame).unwrap();
        match resp {
            EdgeTtsResponse::WordBoundary(wb) => {
                assert_eq!(wb.audio_offset, 50000);
                assert_eq!(wb.duration, 250000);
                assert_eq!(wb.text, "Palavra");
            }
            other => panic!("esperava WordBoundary, veio {other:?}"),
        }
    }

    #[test]
    fn test_parse_edge_binary_frame() {
        let header = "X-RequestId: req-1\r\nPath:audio\r\nContent-Type:audio/mpeg\r\n\r\n";
        let header_bytes = header.as_bytes();
        let header_len = header_bytes.len() as u16;
        let dummy_mp3 = [0xFF, 0xFB, 0x90, 0x64, 0x00, 0x00];

        let mut packet = Vec::new();
        packet.extend_from_slice(&header_len.to_be_bytes());
        packet.extend_from_slice(header_bytes);
        packet.extend_from_slice(&dummy_mp3);

        let audio = parse_edge_binary_frame(&packet).expect("parse audio chunk");
        assert_eq!(audio, dummy_mp3);

        let invalid_header = "Path:other\r\n\r\n";
        let mut invalid_packet = Vec::new();
        invalid_packet.extend_from_slice(&(invalid_header.len() as u16).to_be_bytes());
        invalid_packet.extend_from_slice(invalid_header.as_bytes());
        invalid_packet.extend_from_slice(&dummy_mp3);
        assert!(parse_edge_binary_frame(&invalid_packet).is_none());
    }
}
