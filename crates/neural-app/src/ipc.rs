use neural_core::{is_local_network_target, validate_web_url};
use serde_json::{Map, Value};

pub const IPC_MAX_BYTES: usize = 8 * 1024;
/// Tecto de uma pergunta replicada as outras colunas (`ask`).
pub const ASK_MAX_CHARS: usize = 2_000;
/// Tecto do texto selecionado mandado para pesquisa (`search`).
pub const SEARCH_MAX_CHARS: usize = 2_000;

/// A dica centrada que uma coluna pede ao passar o rato pelos controlos
/// injetados (o "−" e o "⛶ <IA>"). Lista fechada: a pagina so escolhe QUAL
/// dica; o texto e o nome da IA vem do nativo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnHint {
    Minimize,
    Expand,
    /// O rato saiu do controlo: a dica some.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcAction {
    Home,
    Back,
    Restore,
    AutoScroll,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Reload,
    Print,
    Omnibox,
    History,
    ClearHistory,
    Fullscreen,
    DevTools,
    ViewSource,
    NewTab {
        col: Option<usize>,
    },
    Expand {
        col: usize,
    },
    /// Atalho 1/2/3: ao contrario do botao de uma pagina, e global ao
    /// comparador e pode escolher outra coluna. Continua autenticado pela
    /// capability e separado de Expand para nao alargar a autoridade do DOM.
    ShortcutExpand {
        col: usize,
    },
    Minimize {
        col: usize,
    },
    Split {
        col: usize,
        url: String,
    },
    /// Um clique num link dentro de uma coluna. `aside` distingue as duas
    /// intencoes do Chrome: clique simples abre onde se esta, Ctrl+clique (ou
    /// clique do meio) abre "noutro separador" -- aqui, o painel lateral.
    Link {
        col: usize,
        url: String,
        aside: bool,
    },
    /// Pergunta escrita e ENVIADA na caixa de uma IA (Enter ou botao de
    /// enviar). As outras colunas recebem o mesmo texto, cada uma no seu
    /// fornecedor; a coluna de origem segue a conversa dela.
    Ask {
        col: usize,
        text: String,
    },
    /// "Pesquisar" da barra de selecao: o texto selecionado vai as tres IAs
    /// como PERGUNTA. Nunca passa pelo interpretador de comandos da omnibox.
    Search {
        text: String,
    },
    SplitClose,
    SplitExpand,
    Palette {
        col: usize,
    },
    GmailState {
        unread: u32,
        sender: String,
        subject: String,
        key: String,
    },
    ResearchAnswer {
        col: usize,
        text: String,
    },
    AgentObservation {
        data: String,
    },
    /// O rato entrou (ou saiu) de um controlo injetado numa coluna.
    Hint {
        col: usize,
        hint: ColumnHint,
    },
    /// Ctrl+Shift+Z: "cria uma nota com o que selecionei". Sem argumentos de
    /// proposito -- a pagina so PEDE; o texto selecionado, o endereco e o
    /// titulo sao lidos pelo lado nativo, da WebView que mandou o pedido, e
    /// nunca de uma WebView privada.
    Note,
}

pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

pub fn parse_ipc_message(body: &str, expected_cap: &str, max_columns: usize) -> Option<IpcAction> {
    if body.len() > IPC_MAX_BYTES || expected_cap.len() != 32 {
        return None;
    }

    let value: Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    if object.len() != 4
        || !["v", "cap", "action", "args"]
            .iter()
            .all(|key| object.contains_key(*key))
    {
        return None;
    }
    if object.get("v")?.as_u64()? != 1 {
        return None;
    }

    let cap = object.get("cap")?.as_str()?;
    if cap.len() != 32
        || !cap.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !constant_time_eq(cap.as_bytes(), expected_cap.as_bytes())
    {
        return None;
    }

    let action = object.get("action")?.as_str()?;
    let args = object.get("args")?.as_object()?;

    match action {
        "home" => no_args(args, IpcAction::Home),
        "back" => no_args(args, IpcAction::Back),
        "restore" => no_args(args, IpcAction::Restore),
        "autoscroll" => no_args(args, IpcAction::AutoScroll),
        "zoomin" => no_args(args, IpcAction::ZoomIn),
        "zoomout" => no_args(args, IpcAction::ZoomOut),
        "zoomreset" => no_args(args, IpcAction::ZoomReset),
        "reload" => no_args(args, IpcAction::Reload),
        "print" => no_args(args, IpcAction::Print),
        "omnibox" => no_args(args, IpcAction::Omnibox),
        "history" => no_args(args, IpcAction::History),
        "clearhistory" => no_args(args, IpcAction::ClearHistory),
        "fullscreen" => no_args(args, IpcAction::Fullscreen),
        "devtools" => no_args(args, IpcAction::DevTools),
        "viewsource" => no_args(args, IpcAction::ViewSource),
        "note" => no_args(args, IpcAction::Note),
        "newtab" => {
            if args.is_empty() {
                Some(IpcAction::NewTab { col: None })
            } else {
                let col = bounded_col(args, max_columns)?;
                exact_keys(args, &["col"])?;
                Some(IpcAction::NewTab { col: Some(col) })
            }
        }
        "expand" => {
            let col = bounded_col(args, max_columns)?;
            exact_keys(args, &["col"])?;
            Some(IpcAction::Expand { col })
        }
        "shortcut-expand" => {
            let col = bounded_col(args, max_columns)?;
            exact_keys(args, &["col"])?;
            Some(IpcAction::ShortcutExpand { col })
        }
        "minimize" => {
            let col = bounded_col(args, max_columns)?;
            exact_keys(args, &["col"])?;
            Some(IpcAction::Minimize { col })
        }
        "palette" => {
            let col = bounded_col(args, max_columns)?;
            exact_keys(args, &["col"])?;
            Some(IpcAction::Palette { col })
        }
        "split" => {
            let col = bounded_col(args, max_columns)?;
            let raw = bounded_string(args, "url", 2_048, false)?;
            exact_keys(args, &["col", "url"])?;
            let url = validate_web_url(&raw).ok()?;
            if is_local_network_target(&url) {
                return None;
            }
            Some(IpcAction::Split {
                col,
                url: url.to_string(),
            })
        }
        "link" => {
            let col = bounded_col(args, max_columns)?;
            let raw = bounded_string(args, "url", 2_048, false)?;
            let aside = args.get("aside")?.as_bool()?;
            exact_keys(args, &["col", "url", "aside"])?;
            let url = validate_web_url(&raw).ok()?;
            // A mesma politica do `split`: uma pagina nao usa um clique para
            // mandar o navegador a rede local de quem a esta a ler.
            if is_local_network_target(&url) {
                return None;
            }
            Some(IpcAction::Link {
                col,
                url: url.to_string(),
                aside,
            })
        }
        "split-close" => no_args(args, IpcAction::SplitClose),
        "split-expand" => no_args(args, IpcAction::SplitExpand),
        "gmail-state" => {
            exact_keys(args, &["count", "sender", "subject", "key"])?;
            let unread = u32::try_from(args.get("count")?.as_u64()?).ok()?;
            let sender = bounded_string(args, "sender", 180, true)?;
            let subject = bounded_string(args, "subject", 180, true)?;
            let key = bounded_string(args, "key", 180, true)?;
            Some(IpcAction::GmailState {
                unread,
                sender,
                subject,
                key,
            })
        }
        "ask" => {
            exact_keys(args, &["col", "text"])?;
            let col = bounded_col(args, max_columns)?;
            let text = bounded_string(args, "text", ASK_MAX_CHARS, false)?;
            // Caracteres de controlo nao sao algo que alguem escreveu numa
            // caixa de pergunta; so a quebra de linha e o tab passam.
            if text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            {
                return None;
            }
            Some(IpcAction::Ask {
                col,
                text: text.trim().to_string(),
            })
        }
        "search" => {
            exact_keys(args, &["text"])?;
            let raw = args.get("text")?.as_str()?;
            // Os mesmos caracteres que uma pergunta escrita: dos de controlo
            // so a quebra de linha e o tab passam.
            if raw
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            {
                return None;
            }
            let text = raw.trim();
            if text.is_empty() || text.chars().count() > SEARCH_MAX_CHARS {
                return None;
            }
            Some(IpcAction::Search {
                text: text.to_string(),
            })
        }
        "research-answer" => {
            exact_keys(args, &["col", "text"])?;
            let col = bounded_col(args, max_columns)?;
            let text = bounded_string(args, "text", 2_048, false)?;
            Some(IpcAction::ResearchAnswer { col, text })
        }
        "agent-observation" => {
            exact_keys(args, &["data"])?;
            let data = bounded_string(args, "data", 7_500, false)?;
            Some(IpcAction::AgentObservation { data })
        }
        "hint" => {
            exact_keys(args, &["col", "id"])?;
            let col = bounded_col(args, max_columns)?;
            let hint = match args.get("id")?.as_str()? {
                "minimize" => ColumnHint::Minimize,
                "expand" => ColumnHint::Expand,
                "none" => ColumnHint::None,
                _ => return None,
            };
            Some(IpcAction::Hint { col, hint })
        }
        _ => None,
    }
}

fn no_args(args: &Map<String, Value>, action: IpcAction) -> Option<IpcAction> {
    args.is_empty().then_some(action)
}

fn exact_keys(args: &Map<String, Value>, expected: &[&str]) -> Option<()> {
    if args.len() != expected.len() || !expected.iter().all(|key| args.contains_key(*key)) {
        return None;
    }
    Some(())
}

fn bounded_col(args: &Map<String, Value>, max_columns: usize) -> Option<usize> {
    let raw = args.get("col")?.as_u64()?;
    let col = usize::try_from(raw).ok()?;
    (col < max_columns).then_some(col)
}

fn bounded_string(
    args: &Map<String, Value>,
    key: &str,
    max_chars: usize,
    allow_empty: bool,
) -> Option<String> {
    let value = args.get(key)?.as_str()?;
    if value.chars().count() > max_chars || (!allow_empty && value.trim().is_empty()) {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CAP: &str = "0123456789abcdef0123456789abcdef";

    fn message(action: &str, args: Value) -> String {
        json!({"v":1,"cap":CAP,"action":action,"args":args}).to_string()
    }

    #[test]
    fn rejects_invalid_envelope_and_oversized_body() {
        assert!(parse_ipc_message("", CAP, 3).is_none());
        assert!(parse_ipc_message("{}", CAP, 3).is_none());
        assert!(
            parse_ipc_message(
                &json!({"v":2,"cap":CAP,"action":"home","args":{}}).to_string(),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &json!({"v":1,"action":"home","args":{}}).to_string(),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &json!({"v":1,"cap":"bad","action":"home","args":{}}).to_string(),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &json!({"v":1,"cap":CAP,"action":"inventado","args":{}}).to_string(),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &json!({"v":1,"cap":CAP,"action":"home","args":[]}).to_string(),
                CAP,
                3
            )
            .is_none()
        );
        assert!(parse_ipc_message(&"x".repeat(IPC_MAX_BYTES + 1), CAP, 3).is_none());
        assert!(
            parse_ipc_message(
                &json!({"v":1,"cap":CAP,"action":"home","args":{},"extra":true}).to_string(),
                CAP,
                3
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_wrong_capability_at_first_middle_and_last_byte() {
        for idx in [0usize, 15, 31] {
            let mut wrong = CAP.as_bytes().to_vec();
            wrong[idx] = if wrong[idx] == b'0' { b'1' } else { b'0' };
            let wrong = String::from_utf8(wrong).unwrap();
            assert!(parse_ipc_message(&message("home", json!({})), &wrong, 3).is_none());
        }
        assert!(constant_time_eq(CAP.as_bytes(), CAP.as_bytes()));
        assert!(!constant_time_eq(CAP.as_bytes(), &CAP.as_bytes()[..31]));
    }

    #[test]
    fn accepts_all_simple_actions() {
        let cases = [
            ("home", IpcAction::Home),
            ("back", IpcAction::Back),
            ("restore", IpcAction::Restore),
            ("autoscroll", IpcAction::AutoScroll),
            ("zoomin", IpcAction::ZoomIn),
            ("zoomout", IpcAction::ZoomOut),
            ("zoomreset", IpcAction::ZoomReset),
            ("reload", IpcAction::Reload),
            ("print", IpcAction::Print),
            ("omnibox", IpcAction::Omnibox),
            ("history", IpcAction::History),
            ("clearhistory", IpcAction::ClearHistory),
            ("fullscreen", IpcAction::Fullscreen),
            ("devtools", IpcAction::DevTools),
            ("viewsource", IpcAction::ViewSource),
            ("note", IpcAction::Note),
        ];
        for (name, expected) in cases {
            assert_eq!(
                parse_ipc_message(&message(name, json!({})), CAP, 3),
                Some(expected),
                "{name}"
            );
        }
    }

    #[test]
    fn accepts_parameterized_actions_and_rejects_bad_types() {
        assert_eq!(
            parse_ipc_message(&message("newtab", json!({})), CAP, 3),
            Some(IpcAction::NewTab { col: None })
        );
        assert_eq!(
            parse_ipc_message(&message("newtab", json!({"col":2})), CAP, 3),
            Some(IpcAction::NewTab { col: Some(2) })
        );
        assert_eq!(
            parse_ipc_message(&message("expand", json!({"col":1})), CAP, 3),
            Some(IpcAction::Expand { col: 1 })
        );
        assert_eq!(
            parse_ipc_message(&message("shortcut-expand", json!({"col":2})), CAP, 3),
            Some(IpcAction::ShortcutExpand { col: 2 })
        );
        assert!(parse_ipc_message(&message("shortcut-expand", json!({"col":3})), CAP, 3).is_none());
        assert_eq!(
            parse_ipc_message(&message("minimize", json!({"col":0})), CAP, 3),
            Some(IpcAction::Minimize { col: 0 })
        );
        assert_eq!(
            parse_ipc_message(&message("palette", json!({"col":2})), CAP, 3),
            Some(IpcAction::Palette { col: 2 })
        );
        assert!(parse_ipc_message(&message("expand", json!({"col":3})), CAP, 3).is_none());
        assert!(parse_ipc_message(&message("expand", json!({"col":"1"})), CAP, 3).is_none());
        assert!(parse_ipc_message(&message("home", json!({"col":0})), CAP, 3).is_none());
    }

    #[test]
    fn a_link_click_carries_which_panel_it_opens_in() {
        // O `aside` e a diferenca entre "abre aqui" e "abre ao lado". Se
        // chegasse por omissao, um clique simples abria sempre no mesmo sitio.
        assert_eq!(
            parse_ipc_message(
                &message(
                    "link",
                    json!({"col":1,"url":"https://example.com/a","aside":false})
                ),
                CAP,
                3
            ),
            Some(IpcAction::Link {
                col: 1,
                url: "https://example.com/a".into(),
                aside: false
            })
        );
        assert_eq!(
            parse_ipc_message(
                &message(
                    "link",
                    json!({"col":2,"url":"https://example.com/b","aside":true})
                ),
                CAP,
                3
            ),
            Some(IpcAction::Link {
                col: 2,
                url: "https://example.com/b".into(),
                aside: true
            })
        );
    }

    #[test]
    fn a_link_click_obeys_the_same_policy_as_the_split() {
        // Uma pagina nao usa um clique para mandar o navegador de quem a le a
        // um endereco da rede local dele.
        for url in [
            "http://127.0.0.1:8080/",
            "http://192.168.1.10/",
            "http://localhost/",
            "file:///C:/Windows/System32/drivers/etc/hosts",
            "javascript:alert(1)",
            "neuralia:home",
        ] {
            assert!(
                parse_ipc_message(
                    &message("link", json!({"col":0,"url":url,"aside":true})),
                    CAP,
                    3
                )
                .is_none(),
                "{url} devia ser recusado"
            );
        }
        // Coluna fora do alcance, tipo errado e chaves a mais ou a menos.
        assert!(
            parse_ipc_message(
                &message(
                    "link",
                    json!({"col":9,"url":"https://example.com/","aside":true})
                ),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &message(
                    "link",
                    json!({"col":0,"url":"https://example.com/","aside":"sim"})
                ),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &message("link", json!({"col":0,"url":"https://example.com/"})),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &message(
                    "link",
                    json!({"col":0,"url":"https://example.com/","aside":true,"extra":1})
                ),
                CAP,
                3
            )
            .is_none()
        );
    }

    #[test]
    fn split_rejects_local_network_and_invalid_urls() {
        assert!(matches!(
            parse_ipc_message(
                &message("split", json!({"col":1,"url":"https://example.com/a"})),
                CAP,
                3
            ),
            Some(IpcAction::Split { col: 1, .. })
        ));
        for url in [
            "http://127.0.0.1/",
            "http://localhost/",
            "file:///tmp/a",
            "javascript:1",
        ] {
            assert!(
                parse_ipc_message(&message("split", json!({"col":1,"url":url})), CAP, 3).is_none(),
                "{url}"
            );
        }
    }

    #[test]
    fn accepts_data_actions_with_bounds() {
        assert_eq!(
            parse_ipc_message(
                &message(
                    "gmail-state",
                    json!({"count":4,"sender":"A","subject":"B","key":"K"})
                ),
                CAP,
                3
            ),
            Some(IpcAction::GmailState {
                unread: 4,
                sender: "A".into(),
                subject: "B".into(),
                key: "K".into(),
            })
        );
        assert_eq!(
            parse_ipc_message(
                &message("research-answer", json!({"col":1,"text":"resposta"})),
                CAP,
                3
            ),
            Some(IpcAction::ResearchAnswer {
                col: 1,
                text: "resposta".into(),
            })
        );
        assert_eq!(
            parse_ipc_message(
                &message(
                    "agent-observation",
                    json!({"data":"1\nhttps://example.com"})
                ),
                CAP,
                3
            ),
            Some(IpcAction::AgentObservation {
                data: "1\nhttps://example.com".into(),
            })
        );

        assert!(
            parse_ipc_message(
                &message(
                    "gmail-state",
                    json!({"count":"4","sender":"A","subject":"B","key":"K"})
                ),
                CAP,
                3
            )
            .is_none()
        );
        assert!(
            parse_ipc_message(
                &message("research-answer", json!({"col":1,"text":"x".repeat(2_049)})),
                CAP,
                3
            )
            .is_none()
        );
    }

    #[test]
    fn hint_names_one_of_three_closed_hints_for_its_own_column() {
        let hint = |args: Value| parse_ipc_message(&message("hint", args), CAP, 3);
        for (id, expected) in [
            ("minimize", ColumnHint::Minimize),
            ("expand", ColumnHint::Expand),
            ("none", ColumnHint::None),
        ] {
            assert_eq!(
                hint(json!({"col":1,"id":id})),
                Some(IpcAction::Hint {
                    col: 1,
                    hint: expected
                }),
                "{id}"
            );
        }
        for (args, why) in [
            (json!({"col":1,"id":"Minimizar ChatGPT"}), "texto livre"),
            (json!({"col":1,"id":"close"}), "dica inventada"),
            (json!({"col":1,"id":7}), "tipo errado"),
            (json!({"col":3,"id":"expand"}), "coluna fora"),
            (json!({"col":1}), "sem id"),
            (json!({"col":1,"id":"expand","text":"x"}), "campo a mais"),
        ] {
            assert_eq!(hint(args), None, "aceitou uma dica com {why}");
        }
    }

    #[test]
    fn ask_carries_the_typed_question_within_bounds() {
        let ask = |args: Value| parse_ipc_message(&message("ask", args), CAP, 3);
        assert_eq!(
            ask(json!({"col":2,"text":"  capital da França\nem 1900  "})),
            Some(IpcAction::Ask {
                col: 2,
                text: "capital da França\nem 1900".to_string()
            })
        );
        let too_long = "a".repeat(ASK_MAX_CHARS + 1);
        for (args, why) in [
            (json!({"col":0,"text":""}), "vazia"),
            (json!({"col":0,"text":"   \n "}), "so espacos"),
            (json!({"col":0,"text":too_long}), "longa demais"),
            (json!({"col":0,"text":"a\u{7}b"}), "caracter de controlo"),
            (json!({"col":3,"text":"x"}), "coluna fora"),
            (
                json!({"col":0,"text":"x","url":"https://e.com"}),
                "campo a mais",
            ),
            (json!({"col":0,"text":7}), "tipo errado"),
        ] {
            assert_eq!(ask(args), None, "aceitou uma pergunta {why}");
        }
    }

    #[test]
    fn note_is_a_bare_request_and_carries_no_page_data() {
        // A pagina so pede a nota. Texto, endereco e titulo sao lidos pelo
        // lado nativo da WebView que pediu: um `note` com dados e recusado,
        // para ninguem passar a confiar no que a pagina diz de si propria.
        assert_eq!(
            parse_ipc_message(&message("note", json!({})), CAP, 3),
            Some(IpcAction::Note)
        );
        for args in [
            json!({"text":"texto escolhido pela pagina"}),
            json!({"url":"https://example.com/"}),
            json!({"col":0}),
            json!({"title":"x","text":"y","url":"https://example.com/"}),
        ] {
            assert_eq!(
                parse_ipc_message(&message("note", args.clone()), CAP, 3),
                None,
                "note aceitou argumentos: {args}"
            );
        }
    }

    #[test]
    fn search_carries_the_selected_text_within_bounds() {
        let search = |args: Value| parse_ipc_message(&message("search", args), CAP, 3);
        // O texto chega tal como foi selecionado, aparado nas pontas: um
        // "agent:" selecionado e so texto, nao um comando.
        assert_eq!(
            search(json!({"text":"  agent:https://example.com | click=Comprar\n\tlinha 2  "})),
            Some(IpcAction::Search {
                text: "agent:https://example.com | click=Comprar\n\tlinha 2".to_string()
            })
        );
        // O tecto conta caracteres, nao bytes, e so depois de aparar.
        let at_limit = "ç".repeat(SEARCH_MAX_CHARS);
        assert_eq!(
            search(json!({"text": format!("  {at_limit}  ")})),
            Some(IpcAction::Search { text: at_limit })
        );
        let too_long = "a".repeat(SEARCH_MAX_CHARS + 1);
        for (args, why) in [
            (json!({"text":""}), "vazia"),
            (json!({"text":" \n\t "}), "so com espacos"),
            (json!({"text":too_long}), "longa demais"),
            (json!({"text":"a\u{7}b"}), "com caracter de controlo"),
            (json!({"text":"a\rb"}), "com retorno de carro"),
            (json!({"text":"x","col":0}), "com campo a mais"),
            (json!({}), "sem texto"),
            (json!({"text":7}), "de tipo errado"),
        ] {
            assert_eq!(search(args), None, "aceitou uma pesquisa {why}");
        }
    }

    /// SPEC-0005 publica a lista fechada de acoes pagina->nativo. O texto
    /// publicado e lido tal como embarca no repositorio.
    const SPEC_0005: &str = include_str!("../../../docs/specs/SPEC-0005-webview.md");

    /// Quantas acoes o canal publica. E o UNICO numero a mudar quando entra
    /// ou sai uma acao: o gate abaixo exige um exemplo aceite por acao e que
    /// a SPEC-0005 (lista e contagem), a SPEC-0108 (lista e contagem) e a
    /// SPEC-0015 (contagem) digam exatamente isto.
    const PUBLISHED_ACTION_COUNT: usize = 31;

    /// Nome de fio de cada variante. O `match` e exaustivo de proposito: uma
    /// variante nova nao compila sem passar por aqui, e o teste abaixo exige
    /// entao um exemplo aceite pelo parser e o nome nas specs publicadas.
    fn wire_name(action: &IpcAction) -> &'static str {
        match action {
            IpcAction::Home => "home",
            IpcAction::Back => "back",
            IpcAction::Restore => "restore",
            IpcAction::AutoScroll => "autoscroll",
            IpcAction::ZoomIn => "zoomin",
            IpcAction::ZoomOut => "zoomout",
            IpcAction::ZoomReset => "zoomreset",
            IpcAction::Reload => "reload",
            IpcAction::Print => "print",
            IpcAction::Omnibox => "omnibox",
            IpcAction::History => "history",
            IpcAction::ClearHistory => "clearhistory",
            IpcAction::Fullscreen => "fullscreen",
            IpcAction::DevTools => "devtools",
            IpcAction::ViewSource => "viewsource",
            IpcAction::NewTab { .. } => "newtab",
            IpcAction::Expand { .. } => "expand",
            IpcAction::ShortcutExpand { .. } => "shortcut-expand",
            IpcAction::Minimize { .. } => "minimize",
            IpcAction::Split { .. } => "split",
            IpcAction::Link { .. } => "link",
            IpcAction::Ask { .. } => "ask",
            IpcAction::Search { .. } => "search",
            IpcAction::SplitClose => "split-close",
            IpcAction::SplitExpand => "split-expand",
            IpcAction::Palette { .. } => "palette",
            IpcAction::GmailState { .. } => "gmail-state",
            IpcAction::ResearchAnswer { .. } => "research-answer",
            IpcAction::AgentObservation { .. } => "agent-observation",
            IpcAction::Hint { .. } => "hint",
            IpcAction::Note => "note",
        }
    }

    /// SPEC-0108 repete a lista (secao do envelope) e a contagem (criterio de
    /// aceitacao 1); SPEC-0015 repete a contagem no modelo de ameacas.
    const SPEC_0108: &str = include_str!("../../../md/SPEC-0108-secure-webview-ipc-channel.md");
    const SPEC_0015: &str = include_str!("../../../docs/specs/SPEC-0015-threat-model.md");

    /// O texto entre `start` e o primeiro `end` que se lhe segue.
    fn between<'a>(text: &'a str, start: &str, end: &str, what: &str) -> &'a str {
        let from = text
            .find(start)
            .unwrap_or_else(|| panic!("{what}: marcador {start:?} ausente"))
            + start.len();
        let rest = &text[from..];
        &rest[..rest
            .find(end)
            .unwrap_or_else(|| panic!("{what}: fim {end:?} ausente"))]
    }

    /// Os nomes entre crases de um trecho publicado, pela ordem.
    fn backticked(list: &str) -> Vec<String> {
        list.split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }

    /// A contagem em algarismos logo apos `marker` ("29 names", "29-action").
    fn published_count(text: &str, marker: &str, what: &str) -> usize {
        let from = text
            .find(marker)
            .unwrap_or_else(|| panic!("{what}: marcador {marker:?} ausente"))
            + marker.len();
        text[from..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or_else(|_| panic!("{what}: contagem em algarismos apos {marker:?}"))
    }

    #[test]
    fn protocol_accepts_exactly_the_published_actions() {
        let examples = [
            message("home", json!({})),
            message("back", json!({})),
            message("restore", json!({})),
            message("autoscroll", json!({})),
            message("zoomin", json!({})),
            message("zoomout", json!({})),
            message("zoomreset", json!({})),
            message("reload", json!({})),
            message("print", json!({})),
            message("omnibox", json!({})),
            message("history", json!({})),
            message("clearhistory", json!({})),
            message("fullscreen", json!({})),
            message("devtools", json!({})),
            message("viewsource", json!({})),
            message("newtab", json!({})),
            message("expand", json!({"col":0})),
            message("minimize", json!({"col":0})),
            message("split", json!({"col":0,"url":"https://example.com"})),
            message(
                "link",
                json!({"col":0,"url":"https://example.com/","aside":false}),
            ),
            message("ask", json!({"col":1,"text":"capital da França"})),
            message("search", json!({"text":"texto selecionado"})),
            message("shortcut-expand", json!({"col":1})),
            message("split-close", json!({})),
            message("split-expand", json!({})),
            message("palette", json!({"col":0})),
            message(
                "gmail-state",
                json!({"count":0,"sender":"","subject":"","key":""}),
            ),
            message(
                "research-answer",
                json!({"col":0,"text":"texto suficiente"}),
            ),
            message(
                "agent-observation",
                json!({"data":"1\nhttps://example.com"}),
            ),
            message("hint", json!({"col":2,"id":"expand"})),
            message("note", json!({})),
        ];

        // O que o parser que embarca aceita, pelo nome da variante devolvida.
        let accepted: std::collections::BTreeSet<&str> = examples
            .iter()
            .map(|body| {
                let action = parse_ipc_message(body, CAP, 3)
                    .unwrap_or_else(|| panic!("o parser recusou um exemplo valido: {body}"));
                let name = wire_name(&action);
                let sent: Value = serde_json::from_str(body).expect("json");
                assert_eq!(sent["action"], name, "a acao chegou como outra variante");
                name
            })
            .collect();

        // Um exemplo por acao, nem mais nem menos.
        assert_eq!(
            examples.len(),
            PUBLISHED_ACTION_COUNT,
            "exemplos a mais ou a menos para PUBLISHED_ACTION_COUNT"
        );
        assert_eq!(
            accepted.len(),
            PUBLISHED_ACTION_COUNT,
            "o parser aceita um numero de acoes diferente de PUBLISHED_ACTION_COUNT"
        );

        // Cada lista publicada e o conjunto aceite, sem repetidos.
        for (what, names) in [
            (
                "SPEC-0005",
                backticked(between(
                    SPEC_0005,
                    "The closed action set has ",
                    "Unknown actions",
                    "SPEC-0005",
                )),
            ),
            (
                "SPEC-0108",
                backticked(between(
                    SPEC_0108,
                    "um nome da lista fechada de SPEC-0005 (",
                    "Nome fora da lista",
                    "SPEC-0108",
                )),
            ),
        ] {
            let published: std::collections::BTreeSet<&str> =
                names.iter().map(String::as_str).collect();
            assert_eq!(
                published.len(),
                names.len(),
                "{what} repete um nome na lista publicada"
            );
            assert_eq!(
                accepted, published,
                "{what} publica um conjunto diferente do que o parser aceita"
            );
        }

        // Cada contagem publicada e PUBLISHED_ACTION_COUNT.
        for (what, text, marker) in [
            ("SPEC-0005", SPEC_0005, "The closed action set has "),
            ("SPEC-0108", SPEC_0108, "aceita cada uma das "),
            ("SPEC-0015", SPEC_0015, "a closed "),
        ] {
            assert_eq!(
                published_count(text, marker, what),
                PUBLISHED_ACTION_COUNT,
                "{what} publica outra contagem de acoes"
            );
        }
    }
}
