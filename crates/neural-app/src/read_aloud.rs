//! Leitura em voz alta (SPEC-0110, fase offline).
//!
//! O script vive em `assets/pdfjs/read-aloud.js` e embarca por dois caminhos:
//! o visualizador de PDF carrega-o de `http://neuralia-pdf.localhost/read-aloud.js`
//! (`serve_pdf_asset`) e o Modo Leitura recebe-o como initialization script.
//! Usa o `window.speechSynthesis` do WebView2: vozes do Windows
//! (`localService === true`) por omissao; vozes online so por escolha
//! explicita no seletor. Nao ha cliente de rede nenhum -- o CSP do
//! visualizador continua com `connect-src 'self'`.
//!
//! Os gates abaixo correm o texto QUE EMBARCA (esta constante) num `node:vm`
//! com um DOM, um `speechSynthesis` falso e armadilhas de rede
//! (`read_aloud_harness.js`).

/// O script da leitura em voz alta, byte a byte o ficheiro que embarca.
pub(crate) const READ_ALOUD_SCRIPT: &str = include_str!("../../../assets/pdfjs/read-aloud.js");

#[cfg(test)]
pub(crate) mod harness {
    use serde_json::Value;

    const HARNESS: &str = include_str!("read_aloud_harness.js");

    /// Corre `scripts` (nome, texto) pela ordem dada no mesmo contexto e
    /// depois `drive` (corpo de uma funcao async). Devolve
    /// `{result, errors, net, posted, nav}`. Sem Node nao ha gate: falha.
    pub(crate) fn run(scripts: &[(&str, &str)], href: &str, drive: &str) -> Value {
        run_with(scripts, href, None, drive)
    }

    /// Um modulo ES que embarca: o nome, o texto tal e qual, e o `prelude`
    /// que lhe monta o DOM e os `__modules` que os `import` dele recebem.
    pub(crate) struct Module<'a> {
        pub(crate) name: &'a str,
        pub(crate) text: &'a str,
        pub(crate) prelude: &'a str,
    }

    /// Como `run`, e depois dos scripts avalia `module` no mesmo contexto
    /// (node com `--experimental-vm-modules`), antes do `drive`.
    pub(crate) fn run_module(
        scripts: &[(&str, &str)],
        href: &str,
        module: Module<'_>,
        drive: &str,
    ) -> Value {
        run_with(scripts, href, Some(module), drive)
    }

    fn run_with(
        scripts: &[(&str, &str)],
        href: &str,
        module: Option<Module<'_>>,
        drive: &str,
    ) -> Value {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let input = serde_json::json!({
            "scripts": scripts
                .iter()
                .map(|(name, text)| serde_json::json!({ "name": name, "text": text }))
                .collect::<Vec<_>>(),
            "href": href,
            "module": module.as_ref().map(|module| serde_json::json!({
                "name": module.name,
                "text": module.text,
                "prelude": module.prelude,
            })),
            "drive": drive,
        });
        let program = format!("const INPUT = {input};\n{HARNESS}");
        let mut command = Command::new("node");
        if module.is_some() {
            command
                .arg("--experimental-vm-modules")
                .arg("--no-warnings");
        }
        let mut child = command
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("node is required to run the read-aloud gates");
        child
            .stdin
            .take()
            .expect("node stdin")
            .write_all(program.as_bytes())
            .expect("write program to node");
        let output = child.wait_with_output().expect("node output");
        assert!(
            output.status.success(),
            "node failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("harness json")
    }

    /// Exige um cenario sem erros de script e sem um unico acesso a rede, e
    /// devolve o `result` do drive.
    pub(crate) fn clean_result(outcome: &Value) -> &Value {
        assert_eq!(
            outcome["errors"],
            serde_json::json!([]),
            "erros no script: {}",
            outcome["errors"]
        );
        &outcome["result"]
    }
}

#[cfg(test)]
mod tests {
    use super::READ_ALOUD_SCRIPT;
    use super::harness::{clean_result, run};
    use serde_json::{Value, json};

    const PDF_HREF: &str = "http://neuralia-pdf.localhost/viewer.html";

    fn pdf(drive: &str) -> Value {
        run(&[("read-aloud.js", READ_ALOUD_SCRIPT)], PDF_HREF, drive)
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap_or_else(|| panic!("esperava uma lista: {value}"))
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// Vozes tipicas de um Windows em pt-BR com o runtime a expor uma voz
    /// online.
    const VOICES: &str = r#"
const LOCAL_BR = __voice('Microsoft Maria - Portuguese (Brazil)', 'pt-BR', true);
const LOCAL_EN = __voice('Microsoft Zira - English (United States)', 'en-US', true, { default: true });
const ONLINE_BR = __voice('Microsoft Francisca Online (Natural) - Portuguese (Brazil)', 'pt-BR', false);
"#;

    #[test]
    fn read_aloud_sentences_keep_abbreviations_and_numbers_whole() {
        let outcome = pdf(r#"
const cases = [
  'O Sr. Silva chegou. A Dra. Ana saiu.',
  'Pi vale 3.14 aproximadamente. O total foi 1.000,50 reais. Fim.',
  'Frutas, legumes etc. são saudáveis. Coma frutas etc. Depois durma.',
  'Veja a pág. 12 e a fig. 3. Isso basta! Será? Sim… Talvez.',
  'J. R. R. Tolkien escreveu. Ele olhou o mar. Depois saiu.',
  'Acesse www.exemplo.com.br hoje. O Prof. Carlos e a Sra. Lima vieram.',
];
return cases.map((text) =>
  NeuralIAReadAloud.segmentSentences(text).map((s) => text.slice(s.start, s.end)));
"#);
        let result = clean_result(&outcome);
        assert_eq!(
            result,
            &json!([
                ["O Sr. Silva chegou.", "A Dra. Ana saiu."],
                [
                    "Pi vale 3.14 aproximadamente.",
                    "O total foi 1.000,50 reais.",
                    "Fim."
                ],
                [
                    "Frutas, legumes etc. são saudáveis.",
                    "Coma frutas etc.",
                    "Depois durma."
                ],
                [
                    "Veja a pág. 12 e a fig. 3.",
                    "Isso basta!",
                    "Será?",
                    "Sim…",
                    "Talvez."
                ],
                [
                    "J. R. R. Tolkien escreveu.",
                    "Ele olhou o mar.",
                    "Depois saiu."
                ],
                [
                    "Acesse www.exemplo.com.br hoje.",
                    "O Prof. Carlos e a Sra. Lima vieram."
                ]
            ])
        );
    }

    #[test]
    fn read_aloud_long_sentences_become_short_utterances_without_losing_words() {
        // O corte do Chromium: uma utterance longa para a meio sem 'end'. A
        // frase de ~1300 caracteres tem de chegar ao motor em pedacos curtos,
        // um de cada vez, e com todas as palavras pela ordem.
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
const words = [];
for (let i = 0; i < 180; i++) words.push('palavra' + i + (i % 25 === 24 ? ',' : ''));
const long = words.join(' ') + '.';
__pdf([[{{ str: long, eol: false }}]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
for (let i = 0; i < 40 && __speech.queue.length; i++) {{ __speech.finish(); await __settle(); }}
return {{
  max: NeuralIAReadAloud.MAX_UTTERANCE,
  lengths: __said().map((t) => t.length),
  joined: __said().join(' ') === long,
  maxQueue: __speech.maxQueue,
}};
"#
        ));
        let result = clean_result(&outcome);
        let max = result["max"].as_u64().expect("max");
        let lengths: Vec<u64> = result["lengths"]
            .as_array()
            .expect("lengths")
            .iter()
            .map(|n| n.as_u64().expect("length"))
            .collect();
        assert!(lengths.len() >= 7, "partes: {lengths:?}");
        assert!(
            lengths.iter().all(|&n| n <= max),
            "utterance acima de {max}: {lengths:?}"
        );
        assert_eq!(result["joined"], json!(true), "perdeu ou trocou palavras");
        assert_eq!(result["maxQueue"], json!(1), "so uma utterance de cada vez");
    }

    #[test]
    fn read_aloud_queue_advances_on_end_and_turns_the_page() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
__pdf([
  [{{ str: 'Primeira frase. Segunda', eol: true }}, {{ str: 'frase aqui.', eol: false }}],
  [],
  [{{ str: 'Terceira página começa.', eol: true }}, {{ str: 'E acaba.', eol: false }}],
]);
__renderPage(0);
__click(__byId('neuralia-ra-toggle'));
await __settle();
const out = {{ first: __said().slice(), queue: [__speech.queue.length] }};
__speech.finish(); await __settle();
out.second = __said().slice();
__speech.finish(); await __settle();
out.third = __said().slice();
out.shown = __pdfState.shown.slice();
out.beforeRender = __highlight();
__renderPage(2);
out.afterRender = __highlight().map((r) => r.text);
__speech.finish(); await __settle();
__speech.finish(); await __settle();
out.all = __said();
out.state = __byId('neuralia-ra-bar').getAttribute('data-state');
out.hint = __byId('neuralia-ra-hint').textContent;
out.maxQueue = __speech.maxQueue;
return out;
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(strings(&result["first"]), ["Primeira frase."]);
        assert_eq!(
            strings(&result["second"]),
            ["Primeira frase.", "Segunda frase aqui."]
        );
        // A pagina 2 esta vazia (digitalizada): salta-se para a 3.
        assert_eq!(
            strings(&result["third"]),
            [
                "Primeira frase.",
                "Segunda frase aqui.",
                "Terceira página começa."
            ]
        );
        assert_eq!(result["shown"], json!([2]), "leva a pagina lida ao ecra");
        assert_eq!(
            result["beforeRender"],
            json!([]),
            "sem camada de texto nao ha realce"
        );
        assert_eq!(
            strings(&result["afterRender"]),
            ["Terceira página começa."],
            "o realce chega quando a camada de texto e desenhada"
        );
        assert_eq!(
            strings(&result["all"]),
            [
                "Primeira frase.",
                "Segunda frase aqui.",
                "Terceira página começa.",
                "E acaba."
            ]
        );
        assert_eq!(result["state"], json!("idle"));
        assert_eq!(result["hint"], json!("Leitura concluída."));
        assert_eq!(result["maxQueue"], json!(1));
    }

    #[test]
    fn read_aloud_pause_resume_and_stop_ignore_stale_speech_events() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
__pdf([[{{ str: 'Um. Dois. Três. Quatro.', eol: false }}]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
__speech.finish(); await __settle();
const state = () => __byId('neuralia-ra-bar').getAttribute('data-state');
const out = {{ speaking: state(), said: __said().slice() }};
const cancelsBefore = __speech.cancels;
__click(__byId('neuralia-ra-play'));
out.paused = state();
out.pauseCancels = __speech.cancels - cancelsBefore;
out.engineSpeakingWhilePaused = __speech.speaking;
// O Chromium entrega o 'end' da utterance cancelada depois do cancel().
__speech.deliverStale('end');
await __settle();
out.afterStaleEnd = __said().slice();
out.stillPaused = state();
out.pausedHighlight = __highlight().map((r) => r.text);
__click(__byId('neuralia-ra-play'));
await __settle();
out.resumed = __said().slice();
__click(__byId('neuralia-ra-next'));
await __settle();
out.next = __said().slice();
// O 'end' atrasado da frase saltada chega com a seguinte ja a falar: so a
// utterance viva faz a fila andar.
__speech.deliverStale('end');
await __settle();
out.afterStaleWhileSpeaking = __said().slice();
__click(__byId('neuralia-ra-prev'));
await __settle();
out.prev = __said().slice();
__click(__byId('neuralia-ra-stop'));
out.stopped = state();
out.stoppedHighlight = __highlight();
__speech.deliverStale('error');
__speech.deliverStale('end');
await __settle();
out.afterStop = __said().length;
out.finalState = state();
return out;
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(result["speaking"], json!("speaking"));
        assert_eq!(strings(&result["said"]), ["Um.", "Dois."]);
        assert_eq!(result["paused"], json!("paused"));
        assert_eq!(result["pauseCancels"], json!(1), "a pausa cala o motor");
        assert_eq!(
            result["engineSpeakingWhilePaused"],
            json!(false),
            "em pausa o motor nao continua a falar"
        );
        assert_eq!(
            strings(&result["afterStaleEnd"]),
            ["Um.", "Dois."],
            "o 'end' atrasado da utterance cancelada nao pode avancar a fila"
        );
        assert_eq!(result["stillPaused"], json!("paused"));
        assert_eq!(strings(&result["pausedHighlight"]), ["Dois."]);
        assert_eq!(
            strings(&result["resumed"]),
            ["Um.", "Dois.", "Dois."],
            "continuar repete a frase interrompida"
        );
        assert_eq!(strings(&result["next"]), ["Um.", "Dois.", "Dois.", "Três."]);
        assert_eq!(
            strings(&result["afterStaleWhileSpeaking"]),
            ["Um.", "Dois.", "Dois.", "Três."],
            "o 'end' da utterance saltada nao salta outra frase"
        );
        assert_eq!(
            strings(&result["prev"]),
            ["Um.", "Dois.", "Dois.", "Três.", "Dois."]
        );
        assert_eq!(result["stopped"], json!("idle"));
        assert_eq!(result["stoppedHighlight"], json!([]));
        assert_eq!(result["afterStop"], json!(5), "parado nao volta a falar");
        assert_eq!(result["finalState"], json!("idle"));
    }

    #[test]
    fn read_aloud_recovers_when_chromium_drops_the_end_event() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
__pdf([[{{ str: 'Primeira. Segunda.', eol: false }}]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
// A fala para e o 'end' nunca chega.
__speech.lose();
__tick(1000); await __settle();
const early = __said().slice();
__tick(1000); await __settle();
return {{ early, late: __said() }};
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            strings(&result["early"]),
            ["Primeira."],
            "um periodo calado ainda e silencio normal"
        );
        assert_eq!(strings(&result["late"]), ["Primeira.", "Segunda."]);
    }

    #[test]
    fn read_aloud_default_voice_is_local_pt_br_and_never_online() {
        let outcome = pdf(&format!(
            r#"{VOICES}
const pick = (list, stored) => {{
  const v = NeuralIAReadAloud.chooseVoice(list, stored, 'pt');
  return v ? v.voiceURI : null;
}};
const LOCAL_PT = __voice('Microsoft Helia - Portuguese (Portugal)', 'pt-PT', true);
return {{
  full: pick([ONLINE_BR, LOCAL_EN, LOCAL_PT, LOCAL_BR]),
  ptOnly: pick([ONLINE_BR, LOCAL_EN, LOCAL_PT]),
  englishOnly: pick([ONLINE_BR, LOCAL_EN]),
  onlineOnly: pick([ONLINE_BR]),
  storedOnline: pick([ONLINE_BR, LOCAL_BR], ONLINE_BR.voiceURI),
  storedGone: pick([ONLINE_BR, LOCAL_BR], 'voz-que-ja-nao-existe'),
  underscore: pick([ONLINE_BR, __voice('br-underscore', 'pt_BR', true)]),
  names: [LOCAL_BR.voiceURI, LOCAL_EN.voiceURI, LOCAL_PT.voiceURI],
}};
"#
        ));
        let result = clean_result(&outcome);
        let names = strings(&result["names"]);
        let (local_br, local_en, local_pt) = (&names[0], &names[1], &names[2]);
        assert_eq!(result["full"].as_str(), Some(local_br.as_str()));
        assert_eq!(result["ptOnly"].as_str(), Some(local_pt.as_str()));
        assert_eq!(result["englishOnly"].as_str(), Some(local_en.as_str()));
        assert_eq!(
            result["onlineOnly"],
            Value::Null,
            "sem voz local nao se escolhe a online sozinho"
        );
        assert_eq!(
            result["storedOnline"].as_str(),
            Some("Microsoft Francisca Online (Natural) - Portuguese (Brazil)"),
            "a escolha explicita do utilizador vale"
        );
        assert_eq!(result["storedGone"].as_str(), Some(local_br.as_str()));
        assert_eq!(result["underscore"].as_str(), Some("br-underscore"));
    }

    #[test]
    fn read_aloud_with_only_online_voices_asks_instead_of_sending_text() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([ONLINE_BR]);
__pdf([[{{ str: 'Texto sensível do documento.', eol: false }}]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
// Espera um pouco pelas vozes do Windows (podem chegar numa segunda leva)
// e depois diz que nao ha nenhuma.
__tick(1500);
await __settle();
const out = {{
  said: __said().slice(),
  hint: __byId('neuralia-ra-hint').textContent,
  picker: __byId('neuralia-ra-voice').value,
}};
// O utilizador escolhe a voz online no seletor: agora sim.
__change(__byId('neuralia-ra-voice'), ONLINE_BR.voiceURI);
__click(__byId('neuralia-ra-play'));
await __settle();
out.after = __speech.log.map((x) => x.voice);
out.status = __byId('neuralia-ra-status').textContent;
out.stored = __stored();
return out;
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            result["said"],
            json!([]),
            "nada vai para a voz online sem escolha"
        );
        assert!(
            result["hint"]
                .as_str()
                .unwrap_or_default()
                .contains("Não há voz offline"),
            "{}",
            result["hint"]
        );
        assert_eq!(
            result["picker"],
            json!(""),
            "o seletor nao finge uma escolha"
        );
        assert_eq!(
            result["after"],
            json!(["Microsoft Francisca Online (Natural) - Portuguese (Brazil)"])
        );
        assert!(
            result["status"]
                .as_str()
                .unwrap_or_default()
                .contains("voz online"),
            "a barra avisa que a voz e online: {}",
            result["status"]
        );
        assert!(
            result["stored"]["neuralia.readAloud.v1"]
                .as_str()
                .unwrap_or_default()
                .contains("Francisca Online"),
            "{}",
            result["stored"]
        );
    }

    #[test]
    fn read_aloud_waits_for_the_windows_voices_that_arrive_after_the_online_ones() {
        // Medido no Edge (o motor do WebView2): o primeiro 'voiceschanged'
        // traz so as vozes online e as do Windows chegam ~50 ms depois. Um
        // Ctrl+Shift+U nesse intervalo nao pode desistir nem ler com a online.
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__pdf([[{{ str: 'Olá mundo.', eol: false }}]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
__speech.setVoices([ONLINE_BR]);
await __settle();
const out = {{
  early: __said().slice(),
  hintEarly: __byId('neuralia-ra-hint').hidden,
  stateEarly: __byId('neuralia-ra-bar').getAttribute('data-state'),
}};
__tick(50);
__speech.setVoices([ONLINE_BR, LOCAL_EN, LOCAL_BR]);
await __settle();
out.voices = __speech.log.map((x) => [x.text, x.voice]);
return out;
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(result["early"], json!([]));
        assert_eq!(
            result["hintEarly"],
            json!(true),
            "nao diz que falta a voz offline"
        );
        assert_eq!(result["stateEarly"], json!("loading"));
        assert_eq!(
            result["voices"],
            json!([["Olá mundo.", "Microsoft Maria - Portuguese (Brazil)"]])
        );
    }

    #[test]
    fn read_aloud_picker_separates_offline_and_online_voices() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([ONLINE_BR, LOCAL_EN, LOCAL_BR]);
__pdf([[{{ str: 'Olá.', eol: false }}]]);
const select = __byId('neuralia-ra-voice');
return {{
  value: select.value,
  groups: select.children.map((g) => ({{ label: g.getAttribute('label'), voices: g.children.map((o) => o.value) }})),
  rates: __byId('neuralia-ra-rate').children.map((o) => o.value),
}};
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            result["value"],
            json!("Microsoft Maria - Portuguese (Brazil)")
        );
        assert_eq!(
            result["groups"],
            json!([
                {
                    "label": "Offline (Windows) — o texto fica no computador",
                    "voices": [
                        "Microsoft Maria - Portuguese (Brazil)",
                        "Microsoft Zira - English (United States)"
                    ]
                },
                {
                    "label": "Online — precisa de internet; o texto vai para o provedor da voz",
                    "voices": ["Microsoft Francisca Online (Natural) - Portuguese (Brazil)"]
                }
            ])
        );
        assert_eq!(
            result["rates"],
            json!(["0.75", "1", "1.25", "1.5", "1.75", "2"])
        );
    }

    #[test]
    fn read_aloud_highlights_the_spoken_sentence_in_the_text_layer_spans() {
        // O PDF.js parte o texto em spans por troco de fonte e por linha; a
        // frase lida tem de voltar aos spans certos, ao caracter, com a
        // palavra hifenizada no fim da linha colada na fala.
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
__pdf([[
  {{ str: 'O Sr. Silva che', eol: false }},
  {{ str: 'gou. A Dra. Ana', eol: false }},
  {{ str: '', eol: false }},
  {{ str: ' saiu com o exem-', eol: true }},
  {{ str: 'plo na mão.', eol: false }},
]]);
const spans = __renderPage(0);
// O span onde a segunda frase comeca esta abaixo do ecra.
spans[1].__rect = {{ top: 900, bottom: 920, left: 0, right: 100, width: 100, height: 20 }};
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
const out = {{ first: __highlight(), scrollsFirst: __scrolls.length }};
__speech.finish(); await __settle();
out.second = __highlight();
out.said = __said();
out.scrolls = __scrolls.map((s) => s.unit);
// Sem a CSS Custom Highlight API marca-se o span inteiro.
__click(__byId('neuralia-ra-stop'));
CSS.highlights = undefined;
__click(__byId('neuralia-ra-play'));
await __settle();
out.fallback = __marked();
return out;
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            result["first"],
            json!([
                {"unit": "0:0", "endUnit": "0:0", "from": 0, "to": 15, "text": "O Sr. Silva che"},
                {"unit": "0:1", "endUnit": "0:1", "from": 0, "to": 4, "text": "gou."}
            ])
        );
        assert_eq!(
            result["second"],
            json!([
                {"unit": "0:1", "endUnit": "0:1", "from": 5, "to": 15, "text": "A Dra. Ana"},
                {"unit": "0:3", "endUnit": "0:3", "from": 0, "to": 17, "text": " saiu com o exem-"},
                {"unit": "0:4", "endUnit": "0:4", "from": 0, "to": 11, "text": "plo na mão."}
            ])
        );
        assert_eq!(
            strings(&result["said"]),
            [
                "O Sr. Silva chegou.",
                "A Dra. Ana saiu com o exemplo na mão."
            ]
        );
        assert_eq!(
            result["scrollsFirst"],
            json!(0),
            "frase visivel: sem scroll"
        );
        assert_eq!(
            result["scrolls"],
            json!(["0:1"]),
            "frase fora do ecra: centra o primeiro span dela"
        );
        assert_eq!(result["fallback"], json!(["0:0", "0:1"]));
    }

    #[test]
    fn read_aloud_highlight_skips_marked_content_markers_like_the_text_layer() {
        // PDFs marcados (Word, LaTeX com tagpdf) trazem marcadores de conteudo
        // marcado sem `str`. A TextLayer nao lhes da textDiv; se a leitura os
        // contasse como unidades, o realce caia no span errado.
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
__pdf([[
  {{ mark: 'begin' }},
  {{ str: 'Primeira frase.', eol: false }},
  {{ mark: 'end' }},
  {{ mark: 'begin' }},
  {{ str: ' Segunda frase.', eol: false }},
  {{ mark: 'end' }},
]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
const out = {{ first: __highlight().map((r) => [r.unit, r.text]) }};
__speech.finish(); await __settle();
out.second = __highlight().map((r) => [r.unit, r.text]);
out.said = __said();
return out;
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(result["first"], json!([["0:1", "Primeira frase."]]));
        assert_eq!(result["second"], json!([["0:4", "Segunda frase."]]));
        assert_eq!(
            strings(&result["said"]),
            ["Primeira frase.", "Segunda frase."]
        );
    }

    #[test]
    fn read_aloud_remembers_the_chosen_voice_and_speed() {
        // A escolha guardada (origem local do visualizador) vale na leitura
        // seguinte; uma velocidade fora das opcoes vai para a mais proxima e o
        // seletor mostra-a.
        let outcome = pdf(&format!(
            r#"{VOICES}
const LOCAL_PT = __voice('Microsoft Helia - Portuguese (Portugal)', 'pt-PT', true);
localStorage.setItem('neuralia.readAloud.v1', JSON.stringify({{ voice: LOCAL_PT.voiceURI, rate: 1.1 }}));
__contentLoaded();
__speech.setVoices([LOCAL_BR, LOCAL_PT]);
__pdf([[{{ str: 'Olá mundo.', eol: false }}]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
return {{
  log: __speech.log.map((x) => [x.voice, x.rate]),
  picker: __byId('neuralia-ra-voice').value,
  rate: __byId('neuralia-ra-rate').value,
}};
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            result["log"],
            json!([["Microsoft Helia - Portuguese (Portugal)", 1]])
        );
        assert_eq!(
            result["picker"],
            json!("Microsoft Helia - Portuguese (Portugal)")
        );
        assert_eq!(result["rate"], json!("1"));
    }

    #[test]
    fn read_aloud_starts_from_the_selection() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
__pdf([
  [{{ str: 'Página um. Não lida.', eol: false }}],
  [{{ str: 'Antes da seleção. Aqui come', eol: false }}, {{ str: 'ça a leitura. Depois.', eol: false }}],
]);
__renderPage(0);
const spans = __renderPage(1);
// Selecao a partir de "leitura" (dentro do segundo span da pagina 2).
__select(spans[1].firstChild, 5);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
__speech.finish(); await __settle();
return __said();
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            strings(result),
            ["Aqui começa a leitura.", "Depois."],
            "comeca na frase que contem a selecao"
        );
    }

    #[test]
    fn read_aloud_survives_storage_that_throws() {
        // Modo Leitura: origem opaca, o localStorage lanca.
        let outcome = pdf(&format!(
            r#"{VOICES}
__storageThrows = true;
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
__pdf([[{{ str: 'Um. Dois.', eol: false }}]]);
__renderPage(0);
__change(__byId('neuralia-ra-rate'), '1.5');
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
return __speech.log.map((x) => x.rate);
"#
        ));
        assert_eq!(clean_result(&outcome), &json!([1.5]));
    }

    /// Gate: num PDF com tags (Word, InDesign, PDF/UA) o PDF.js fecha o texto
    /// em cada conteudo marcado e o fim de linha chega num item VAZIO
    /// `{str: '', hasEOL: true}`. A palavra partida por hifen tem de se colar
    /// na fala na mesma -- antes a voz dizia "exem" ... "plo".
    #[test]
    fn read_aloud_joins_a_hyphenated_word_across_an_empty_end_of_line_item() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR]);
// A forma exata que o getTextContent() do PDF.js vendorizado da num PDF com
// BDC/EMC por linha (medido com o pdf.mjs 6.3.289): o hifen num item, o fim
// de linha num item vazio, o resto da palavra no seguinte.
__pdf([[
  {{ str: 'Isto e um exem-', eol: false }},
  {{ str: '', eol: true }},
  {{ str: 'plo de texto. Outra frase com hí-', eol: false }},
  {{ str: '', eol: true }},
  {{ str: '', eol: false }},
  {{ str: 'fen.', eol: false }},
]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
__speech.finish(); await __settle();
const plain = NeuralIAReadAloud.pageModel([
  {{ text: 'Isto e um exem-', eol: true }},
  {{ text: 'plo de texto.', eol: false }},
]);
return {{
  said: __said(),
  plain: plain.sentences.map((s) => NeuralIAReadAloud.spokenText(plain, s)),
}};
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            strings(&result["said"]),
            ["Isto e um exemplo de texto.", "Outra frase com hífen."]
        );
        assert_eq!(strings(&result["plain"]), ["Isto e um exemplo de texto."]);
    }

    /// Gate: uma frase que acaba num numero com pontos ("12.000", "R$
    /// 1.500", "3.14") ou num endereco ("www.exemplo.pt") fecha quando a
    /// seguinte comeca por maiuscula. As siglas so com letras ("e.g.", "S.A.",
    /// "Ph.D.") continuam a nao fechar.
    #[test]
    fn read_aloud_a_sentence_ending_in_a_number_or_an_address_still_ends() {
        let outcome = pdf(r#"
const cases = [
  'A população é de 12.000. Depois cresceu.',
  'O valor é 3.14. Depois mudou.',
  'Visite www.exemplo.pt. Depois volte.',
  'Custou R$ 1.500. Em seguida caiu.',
  'Ele nasceu em 1990. Depois estudou.',
  'A empresa X S.A. Comprou tudo.',
  'O Ph.D. Silva chegou.',
];
return cases.map((text) =>
  NeuralIAReadAloud.segmentSentences(text).map((s) => text.slice(s.start, s.end)));
"#);
        assert_eq!(
            clean_result(&outcome),
            &json!([
                ["A população é de 12.000.", "Depois cresceu."],
                ["O valor é 3.14.", "Depois mudou."],
                ["Visite www.exemplo.pt.", "Depois volte."],
                ["Custou R$ 1.500.", "Em seguida caiu."],
                ["Ele nasceu em 1990.", "Depois estudou."],
                ["A empresa X S.A. Comprou tudo."],
                ["O Ph.D. Silva chegou."]
            ])
        );
    }

    /// Gate: a voz escolhida fica guardada POR IDIOMA. A inglesa escolhida
    /// para um documento em ingles nao passa a ler os em portugues, e a
    /// escolha antiga (uma so para tudo) vale so para o idioma da propria voz.
    #[test]
    fn read_aloud_remembers_the_chosen_voice_per_language() {
        let voice_for = |stored: &str, text: &str| {
            let outcome = pdf(&format!(
                r#"{VOICES}
localStorage.setItem('neuralia.readAloud.v1', JSON.stringify({stored}));
__contentLoaded();
__speech.setVoices([LOCAL_BR, LOCAL_EN]);
// Sem idioma declarado (como um PDF sem /Lang): ouve-se o texto.
const state = {{ current: 0 }};
__ctl = NeuralIAReadAloud.attachPdf({{
  window,
  lang: '',
  pageCount: () => 1,
  currentPage: () => 0,
  textContent: () => Promise.resolve([{{ str: {text}, hasEOL: false }}]),
  textDivs: () => null,
  pageOf: () => null,
  showPage: () => {{}},
}});
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
return __speech.log.map((x) => x.voice)[0] || null;
"#,
                text = json!(text)
            ));
            clean_result(&outcome)
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        let english = "The quick brown fox jumps over the lazy dog. It was sunny and the children were playing in the park with their friends.";
        let portuguese = "A raposa pula por cima do cão. Não é uma história com muito sentido, mas é da tradição dos testes.";
        let zira = "Microsoft Zira - English (United States)";
        let maria = "Microsoft Maria - Portuguese (Brazil)";
        let per_language = r#"{ "voices": { "en": "Microsoft Zira - English (United States)" } }"#;
        assert_eq!(voice_for(per_language, english), zira);
        assert_eq!(voice_for(per_language, portuguese), maria);
        let legacy = r#"{ "voice": "Microsoft Zira - English (United States)" }"#;
        assert_eq!(voice_for(legacy, portuguese), maria, "a escolha antiga");
        assert_eq!(voice_for(legacy, english), zira);
        // E escolher no seletor guarda para o idioma do documento aberto.
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_BR, LOCAL_EN]);
__pdf([[{{ str: {english}, eol: false }}]]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
__change(__byId('neuralia-ra-voice'), LOCAL_EN.voiceURI);
await __settle();
return JSON.parse(__stored()['neuralia.readAloud.v1']).voices;
"#,
            english = json!(english)
        ));
        // O __pdf do harness declara 'pt' (como o viewer antigo): a escolha
        // fica para 'pt'.
        assert_eq!(clean_result(&outcome), &json!({ "pt": zira }));
    }

    /// Gate: sem voz online nenhuma no runtime, o seletor diz-o por extenso
    /// no grupo Online, em vez de o grupo simplesmente nao existir.
    #[test]
    fn read_aloud_picker_says_when_there_is_no_online_voice() {
        let outcome = pdf(&format!(
            r#"{VOICES}
__contentLoaded();
__speech.setVoices([LOCAL_EN, LOCAL_BR]);
__pdf([[{{ str: 'Olá.', eol: false }}]]);
const select = __byId('neuralia-ra-voice');
return select.children.map((g) => ({{
  label: g.getAttribute('label'),
  options: g.children.map((o) => [o.value, o.textContent, !!o.disabled]),
}}));
"#
        ));
        let result = clean_result(&outcome);
        assert_eq!(
            result[1],
            json!({
                "label": "Online — precisa de internet; o texto vai para o provedor da voz",
                "options": [["", "Nenhuma voz online neste Windows — lê com as vozes instaladas", true]]
            }),
            "{result}"
        );
    }

    #[test]
    fn read_aloud_offline_session_touches_no_network_api() {
        // Sessao completa com vozes locais: abrir, ler, pausar, continuar,
        // saltar, mudar velocidade e voz, parar. Nenhuma API de rede pode ser
        // tocada -- e o que permite o CSP do visualizador ficar como esta.
        let outcome = pdf(&format!(
            r#"{VOICES}
const LOCAL_PT = __voice('Microsoft Helia - Portuguese (Portugal)', 'pt-PT', true);
__contentLoaded();
__speech.setVoices([LOCAL_BR, LOCAL_PT, LOCAL_EN]);
__pdf([
  [{{ str: 'Um. Dois. Três.', eol: true }}],
  [{{ str: 'Quatro. Cinco.', eol: false }}],
]);
__renderPage(0);
__key({{ key: 'U', ctrlKey: true, shiftKey: true }});
await __settle();
__speech.finish(); await __settle();
__click(__byId('neuralia-ra-play'));
__click(__byId('neuralia-ra-play')); await __settle();
__click(__byId('neuralia-ra-next')); await __settle();
__change(__byId('neuralia-ra-rate'), '2'); await __settle();
__change(__byId('neuralia-ra-voice'), LOCAL_PT.voiceURI); await __settle();
__speech.finish(); await __settle();
__renderPage(1);
__speech.finish(); await __settle();
__tick(5000); await __settle();
__key({{ key: 'Escape' }});
await __settle();
return {{ said: __said().length, voices: [...new Set(__speech.log.map((x) => x.voice))] }};
"#
        ));
        assert_eq!(outcome["net"], json!([]), "a leitura offline tocou na rede");
        let result = clean_result(&outcome);
        assert!(result["said"].as_u64().unwrap_or(0) >= 5, "{result}");
        assert_eq!(
            result["voices"],
            json!([
                "Microsoft Maria - Portuguese (Brazil)",
                "Microsoft Helia - Portuguese (Portugal)"
            ])
        );
        // Proibicao de presenca (AGENTS.md 4.3): o script nem sequer nomeia
        // uma API de rede.
        for api in [
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "EventSource",
            "sendBeacon",
            "importScripts",
            "new Worker",
        ] {
            assert!(
                !READ_ALOUD_SCRIPT.contains(api),
                "read-aloud.js nao pode usar {api}"
            );
        }
    }
}
