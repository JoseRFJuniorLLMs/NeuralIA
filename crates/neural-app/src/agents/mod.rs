//! Agentes externos — Claude Code, Codex CLI, Gemini CLI e qualquer outro
//! cliente MCP — a falar com o utilizador dentro do NeuralIA (pedido do dono,
//! 23/09/2026: «botão de agente ... para abrir o agente para interagir com o
//! usuário»).
//!
//! Tres pecas:
//!
//! 1. A **ponte**: `NeuralIA.exe --mcp --agent <nome>` corre SEM janela. E um
//!    servidor MCP em stdio (JSON-RPC 2.0, uma mensagem por linha) que o CLI
//!    do agente lanca como subprocesso ([`mcp`]). O `main` desvia para aqui
//!    antes de qualquer janela, WebView ou perfil ([`launch_mode`]).
//! 2. O **hub**, dentro do NeuralIA aberto ([`hub`]): estado, perguntas,
//!    limites, eventos para a interface. A ponte fala com ele por um named
//!    pipe local cujo DACL so deixa entrar o utilizador atual e o SYSTEM, e
//!    que exige um token aleatorio lido de `<data_dir>/agents/token`
//!    ([`pipe`], so Windows).
//! 3. As **conversas**, em `<data_dir>/agents/<agente>.jsonl` ([`store`]).
//!
//! Regra de privacidade: o agente nunca le o que o utilizador escreve nas
//! paginas. Le so o que ele escreve no painel «Agentes» para esse agente, e o
//! que partilha de proposito com os botoes «esta página» / «estas abas».
//!
//! API para a interface (o painel): ver [`AgentHub`] e [`AgentEvent`].

pub(crate) mod hub;
pub(crate) mod mcp;
#[cfg(target_os = "windows")]
pub(crate) mod pipe;
pub(crate) mod store;
pub(crate) mod tools;

use std::ffi::OsString;
use std::io::{self, BufRead};
use std::path::Path;

// A API do painel «Agentes» (interface). O app (`windows_app/agents_hub.rs`)
// so usa hoje o hub, os eventos e os registos; o resto (resumo, perguntas,
// respostas, contexto partilhado) e do painel, que chega no
// int-agents-finish -- ate la, so os testes o tocam.
#[allow(unused_imports)]
pub(crate) use hub::{
    AgentEvent, AgentHub, AgentStatusLine, AgentSummary, HubServerState, HubSnapshot,
    QuestionAnswer, QuestionOutcome, QuestionView,
};
#[allow(unused_imports)]
pub(crate) use store::{AgentRecord, AnswerVia, CloseReason, RecordBody, SharedContext, SharedTab};
pub(crate) use tools::{agent_display_name, sanitize_agent_name};

/// Nome do agente quando a linha de comando nao diz qual.
pub(crate) const DEFAULT_AGENT: &str = "agente";
/// O nome com que o NeuralIA se regista em cada CLI.
pub(crate) const MCP_SERVER_NAME: &str = "neuralia";

/// O que o executavel deve ser, conforme os argumentos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchMode {
    /// A janela de sempre.
    App,
    /// `--mcp`: a ponte, sem janela.
    Bridge { agent: String },
    /// `--mcp` com argumentos que nao se entendem.
    BridgeUsage(String),
}

/// Decide pelo primeiro olhar aos argumentos. Basta um `--mcp` em qualquer
/// posicao para nunca abrir a janela: um CLI mal configurado nao pode fazer
/// aparecer um navegador a roubar o foco.
pub(crate) fn launch_mode(args: &[OsString]) -> LaunchMode {
    if !args.iter().any(|arg| arg == "--mcp") {
        return LaunchMode::App;
    }
    let mut agent: Option<String> = None;
    let mut seen_mcp = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let Some(arg) = arg.to_str() else {
            return LaunchMode::BridgeUsage("argumento que não é texto".into());
        };
        let raw_agent = if arg == "--mcp" {
            if seen_mcp {
                return LaunchMode::BridgeUsage("--mcp repetido".into());
            }
            seen_mcp = true;
            continue;
        } else if arg == "--agent" {
            match iter.next().and_then(|value| value.to_str()) {
                Some(value) => value.to_string(),
                None => return LaunchMode::BridgeUsage("--agent sem nome".into()),
            }
        } else if let Some(value) = arg.strip_prefix("--agent=") {
            value.to_string()
        } else {
            let shown: String = arg.chars().take(40).collect();
            return LaunchMode::BridgeUsage(format!("argumento desconhecido: {shown}"));
        };
        if agent.is_some() {
            return LaunchMode::BridgeUsage("--agent repetido".into());
        }
        match sanitize_agent_name(&raw_agent) {
            Some(name) => agent = Some(name),
            None => {
                return LaunchMode::BridgeUsage(
                    "nome de agente inválido (use letras, números e -)".into(),
                );
            }
        }
    }
    LaunchMode::Bridge {
        agent: agent.unwrap_or_else(|| DEFAULT_AGENT.to_string()),
    }
}

pub(crate) const BRIDGE_USAGE: &str = "Uso: NeuralIA.exe --mcp [--agent <nome>]\n\
     Liga um agente (Claude Code, Codex, Gemini CLI...) ao NeuralIA aberto, por MCP em stdio.\n\
     Usage: NeuralIA.exe --mcp [--agent <name>]";

/// Um comando para registar o NeuralIA num CLI (secção «Ligar agentes»).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct SetupCommand {
    /// Nome do agente (`claude`, `codex`, `gemini`).
    pub(crate) agent: &'static str,
    /// O que o painel mostra: «Claude Code», «Codex CLI», «Gemini CLI».
    pub(crate) label: &'static str,
    /// Para o Prompt de Comando (cmd.exe).
    pub(crate) cmd: String,
    /// Para o PowerShell: o `--` vai entre aspas, senao o PowerShell come-o
    /// antes de o CLI o ver.
    pub(crate) powershell: String,
    /// Nota extra (pt-BR), quando ha.
    pub(crate) note: Option<&'static str>,
}

/// Os comandos que ligam cada CLI a este executavel.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn setup_commands(exe: &Path) -> Vec<SetupCommand> {
    let exe = exe.display().to_string();
    // `before` vem antes do `--`, `after` depois dele. O Claude Code e o
    // Codex recebem o executavel depois do `--`; o Gemini CLI recebe-o antes
    // e so os argumentos depois.
    let entry = |agent: &'static str,
                 label: &'static str,
                 before: String,
                 after: String,
                 note: Option<&'static str>| SetupCommand {
        agent,
        label,
        cmd: format!("{before} -- {after}"),
        powershell: format!("{before} '--' {after}"),
        note,
    };
    vec![
        entry(
            "claude",
            "Claude Code",
            format!("claude mcp add --scope user {MCP_SERVER_NAME}"),
            format!("\"{exe}\" --mcp --agent claude"),
            None,
        ),
        entry(
            "codex",
            "Codex CLI",
            format!("codex mcp add {MCP_SERVER_NAME}"),
            format!("\"{exe}\" --mcp --agent codex"),
            Some(
                "O Codex desiste de uma ferramenta ao fim de 60 s. Para o ask_user poder esperar \
                 pela sua resposta, acrescente tool_timeout_sec = 3600 em [mcp_servers.neuralia] \
                 no ficheiro ~/.codex/config.toml.",
            ),
        ),
        entry(
            "gemini",
            "Gemini CLI",
            format!("gemini mcp add --scope user {MCP_SERVER_NAME} \"{exe}\""),
            "--mcp --agent gemini".to_string(),
            None,
        ),
    ]
}

/// Uma linha lida com tecto.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Text(Vec<u8>),
    /// Passou do tecto; o resto dela foi descartado ate a quebra de linha.
    TooLong,
}

/// Le ate `\n` (exclusive) sem nunca guardar mais de `cap` bytes. `None` no
/// fim do fluxo. Uma ultima linha sem `\n` conta como linha.
pub(crate) fn read_line_capped(input: &mut impl BufRead, cap: usize) -> io::Result<Option<Line>> {
    let mut line = Vec::new();
    let mut too_long = false;
    let mut any = false;
    loop {
        let available = match input.fill_buf() {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(if !any {
                None
            } else if too_long {
                Some(Line::TooLong)
            } else {
                Some(Line::Text(line))
            });
        }
        any = true;
        let newline = available.iter().position(|&b| b == b'\n');
        let chunk = &available[..newline.unwrap_or(available.len())];
        if !too_long {
            if line.len() + chunk.len() > cap {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = newline.map_or(available.len(), |at| at + 1);
        input.consume(used);
        if newline.is_some() {
            return Ok(Some(if too_long {
                Line::TooLong
            } else {
                Line::Text(line)
            }));
        }
    }
}

/// `NeuralIA.exe --mcp`: corre a ponte ate o stdin fechar. Devolve o codigo
/// de saida. Nada disto toca em janelas, WebView2 ou no perfil.
#[cfg(target_os = "windows")]
pub(crate) fn run_bridge(agent: String) -> i32 {
    // Um executavel do subsistema Windows lancado sem stdio redirecionado
    // (duplo clique num atalho com --mcp) nao tem com quem falar.
    if !pipe::stdio_present() {
        return 3;
    }
    let dir = neural_core::config::default_data_dir().join("agents");
    let link = pipe::PipeLink::new(dir, agent.clone());
    let stdin = io::stdin();
    let input = stdin.lock();
    let output: Box<dyn io::Write + Send> = Box::new(io::stdout());
    match mcp::serve(
        &agent,
        input,
        output,
        Box::new(link),
        mcp::ServeOptions::default(),
    ) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("[neuralia-mcp] {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn mcp_flag_anywhere_selects_the_windowless_bridge() {
        assert_eq!(launch_mode(&args(&[])), LaunchMode::App);
        assert_eq!(launch_mode(&args(&["https://x.test"])), LaunchMode::App);
        assert_eq!(
            launch_mode(&args(&["--mcp"])),
            LaunchMode::Bridge {
                agent: DEFAULT_AGENT.into()
            }
        );
        assert_eq!(
            launch_mode(&args(&["--mcp", "--agent", "Claude"])),
            LaunchMode::Bridge {
                agent: "claude".into()
            }
        );
        assert_eq!(
            launch_mode(&args(&["--agent=codex", "--mcp"])),
            LaunchMode::Bridge {
                agent: "codex".into()
            }
        );
        for bad in [
            &["--mcp", "--agent"][..],
            &["--mcp", "--agent", "é"],
            &["--mcp", "--agent", "nul"],
            &["--mcp", "--mcp"],
            &["--mcp", "--agent", "a", "--agent", "b"],
            &["--mcp", "--verbose"],
            &["https://x.test", "--mcp"],
        ] {
            assert!(
                matches!(launch_mode(&args(bad)), LaunchMode::BridgeUsage(_)),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn setup_commands_point_every_cli_at_this_executable() {
        let exe = Path::new(r"C:\Program Files\NeuralIA\NeuralIA.exe");
        let commands = setup_commands(exe);
        let agents: Vec<&str> = commands.iter().map(|c| c.agent).collect();
        assert_eq!(agents, ["claude", "codex", "gemini"]);
        assert_eq!(
            commands[0].cmd,
            r#"claude mcp add --scope user neuralia -- "C:\Program Files\NeuralIA\NeuralIA.exe" --mcp --agent claude"#
        );
        assert_eq!(
            commands[1].powershell,
            r#"codex mcp add neuralia '--' "C:\Program Files\NeuralIA\NeuralIA.exe" --mcp --agent codex"#
        );
        assert_eq!(
            commands[2].cmd,
            r#"gemini mcp add --scope user neuralia "C:\Program Files\NeuralIA\NeuralIA.exe" -- --mcp --agent gemini"#
        );
        assert!(commands[1].note.unwrap().contains("tool_timeout_sec"));
        // Cada comando, lido como o CLI o le, lanca a ponte deste agente.
        for command in &commands {
            let tail = command.cmd.split(" -- ").nth(1).unwrap();
            assert!(tail.ends_with(&format!("--mcp --agent {}", command.agent)));
        }
    }

    #[test]
    fn capped_reader_never_buffers_past_the_cap() {
        let mut input = io::Cursor::new(b"abc\n0123456789\nxy\r\n\nlast".to_vec());
        let mut reader = io::BufReader::with_capacity(4, &mut input);
        assert_eq!(
            read_line_capped(&mut reader, 5).unwrap(),
            Some(Line::Text(b"abc".to_vec()))
        );
        assert_eq!(
            read_line_capped(&mut reader, 5).unwrap(),
            Some(Line::TooLong)
        );
        assert_eq!(
            read_line_capped(&mut reader, 5).unwrap(),
            Some(Line::Text(b"xy\r".to_vec()))
        );
        assert_eq!(
            read_line_capped(&mut reader, 5).unwrap(),
            Some(Line::Text(Vec::new()))
        );
        assert_eq!(
            read_line_capped(&mut reader, 5).unwrap(),
            Some(Line::Text(b"last".to_vec()))
        );
        assert_eq!(read_line_capped(&mut reader, 5).unwrap(), None);
    }
}
