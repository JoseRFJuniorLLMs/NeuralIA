use neural_core::{is_local_network_target, validate_web_url};
use serde_json::{Map, Value};

pub const IPC_MAX_BYTES: usize = 8 * 1024;

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
        assert!(
            parse_ipc_message(&message("shortcut-expand", json!({"col":3})), CAP, 3).is_none()
        );
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
    fn protocol_covers_twenty_five_real_actions() {
        let messages = [
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
        ];
        assert_eq!(messages.len(), 26);
        assert!(
            messages
                .iter()
                .all(|body| parse_ipc_message(body, CAP, 3).is_some())
        );
    }
}
