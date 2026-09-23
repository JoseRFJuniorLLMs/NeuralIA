//! O Pomodoro da interface: o que o clique, o menu do botao direito, o
//! comando `pomodoro:` e o tique de cada segundo fazem ao
//! `neural_core::Pomodoro` -- sem Win32 e sem relogio.
//!
//! Cada decisao recebe o `now` de quem chama: a app passa o `Instant::now()`
//! do evento, os testes andam horas num instante sem dormir. O `App` so faz o
//! que estas funcoes devolvem (agendar o tique, mostrar o aviso, tocar o som,
//! gravar as opcoes, repintar a barra).

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use neural_core::{Phase, Pomodoro, PomodoroEvent, PomodoroSettings};

/// Marca a frente do tempo enquanto o Pomodoro esta pausado. O GDI desenha-a
/// com a "Segoe UI Symbol", que o Windows liga a "Segoe UI" (FontLink).
pub(crate) const PAUSE_MARKER: &str = "⏸";

/// Tomate do botao durante o foco, e verde das pausas. A barra acerta o tom
/// ao fundo do tema (`readable`) antes de o usar.
pub(crate) const FOCUS_COLOR: (u8, u8, u8) = (255, 99, 71);
pub(crate) const BREAK_COLOR: (u8, u8, u8) = (46, 160, 67);

/// Limite das duracoes lidas do ficheiro, em minutos. Ate 99 o tempo na
/// barra tem sempre cinco caracteres ("99:59"), e com a marca de pausa sete:
/// e essa a etiqueta mais larga que o gate da barra mede.
const MAX_MINUTES: u64 = 99;
const MAX_LONG_BREAK_EVERY: u32 = 12;
/// O ficheiro das opcoes tem meia duzia de linhas; mais do que isto e lixo.
const SETTINGS_MAX_BYTES: u64 = 4096;

/// Duracoes prontas do menu do botao direito e do `pomodoro:25|50|15`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroPreset {
    /// 25 / 5 min, pausa longa de 15 a cada 4 focos: o padrao.
    Classic,
    /// 50 / 10 min, pausa longa de 30.
    Long,
    /// 15 / 3 min, pausa longa de 10.
    Short,
}

impl PomodoroPreset {
    pub(crate) const ALL: [Self; 3] = [Self::Classic, Self::Long, Self::Short];

    pub(crate) fn settings(self) -> PomodoroSettings {
        let (focus, short_break, long_break) = match self {
            Self::Classic => (25, 5, 15),
            Self::Long => (50, 10, 30),
            Self::Short => (15, 3, 10),
        };
        PomodoroSettings::new(minutes(focus), minutes(short_break), minutes(long_break), 4)
            .unwrap_or_default()
    }

    /// O texto do menu.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Classic => "25 / 5 min (padrão)",
            Self::Long => "50 / 10 min",
            Self::Short => "15 / 3 min",
        }
    }

    /// O preset com exactamente estas duracoes, se houver (um ficheiro
    /// editado a mao pode ter outras: nenhum fica marcado no menu).
    pub(crate) fn of(settings: PomodoroSettings) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.settings() == settings)
    }
}

fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

/// "25 min", "90 s", "1 min 30 s": duracoes nos avisos.
fn duration_text(duration: Duration) -> String {
    let secs = duration.as_secs();
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Focus => "Foco",
        Phase::ShortBreak => "Pausa curta",
        Phase::LongBreak => "Pausa longa",
    }
}

/// Cor base da fase no botao: tomate no foco, verde nas pausas.
pub(crate) fn phase_color(phase: Phase) -> (u8, u8, u8) {
    match phase {
        Phase::Focus => FOCUS_COLOR,
        Phase::ShortBreak | Phase::LongBreak => BREAK_COLOR,
    }
}

/// O aviso centrado do fim de uma fase.
pub(crate) fn phase_end_message(event: PomodoroEvent, settings: PomodoroSettings) -> String {
    match event {
        PomodoroEvent::FocusFinished {
            completed_focus,
            next: Phase::LongBreak,
        } => format!(
            "{completed_focus} focos concluídos! Pausa longa de {}",
            duration_text(settings.long_break())
        ),
        PomodoroEvent::FocusFinished {
            next: Phase::ShortBreak,
            ..
        } => format!(
            "Foco concluído! Pausa curta de {}",
            duration_text(settings.short_break())
        ),
        // O motor nunca encadeia foco com foco; se um dia o fizer, o aviso
        // continua a dizer a verdade.
        PomodoroEvent::FocusFinished {
            next: Phase::Focus, ..
        } => format!(
            "Foco concluído! Novo foco de {}",
            duration_text(settings.focus())
        ),
        PomodoroEvent::BreakFinished { .. } => format!(
            "Pausa terminada — hora de focar ({})",
            duration_text(settings.focus())
        ),
    }
}

/// Espera ate ao proximo tique: exactamente ate o "mm:ss" mudar. O tempo e
/// arredondado para cima (`remaining_label`), por isso muda quando o que
/// falta passa por um segundo inteiro -- e o fim da fase cai num tique, nao
/// ate um segundo depois dele.
pub(crate) fn next_tick_delay(remaining: Duration) -> Duration {
    if remaining.is_zero() {
        return Duration::ZERO;
    }
    let fraction = Duration::from_nanos(u64::from(remaining.subsec_nanos()));
    if fraction.is_zero() {
        Duration::from_secs(1)
    } else {
        fraction
    }
}

/// O que o utilizador pode pedir ao Pomodoro: pelo clique, pelo menu ou pela
/// omnibox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroCommand {
    /// O clique esquerdo: parado inicia, a correr pausa, pausado retoma.
    Click,
    Start,
    Pause,
    Resume,
    Stop,
    Skip,
    Preset(PomodoroPreset),
}

/// Palavra depois de `pomodoro:` (sem caixa; espacos a volta ignorados).
/// `None` e uma palavra que nao existe: a omnibox mostra a ajuda.
pub(crate) fn parse_pomodoro_command(word: &str) -> Option<PomodoroCommand> {
    let word = word.trim().to_lowercase();
    // "pomodoro:25 min" e o mesmo que "pomodoro:25" (mas "min" sozinho nao
    // e "iniciar").
    let word = match word.strip_suffix("min") {
        Some(number) if !number.trim().is_empty() => number.trim_end(),
        _ => word.as_str(),
    };
    Some(match word {
        "" | "iniciar" | "inicia" | "começar" | "comecar" | "start" => PomodoroCommand::Start,
        "pausar" | "pausa" | "pause" => PomodoroCommand::Pause,
        "retomar" | "continuar" | "resume" => PomodoroCommand::Resume,
        "parar" | "stop" => PomodoroCommand::Stop,
        "pular" | "saltar" | "skip" => PomodoroCommand::Skip,
        "25" => PomodoroCommand::Preset(PomodoroPreset::Classic),
        "50" => PomodoroCommand::Preset(PomodoroPreset::Long),
        "15" => PomodoroCommand::Preset(PomodoroPreset::Short),
        _ => return None,
    })
}

/// A ajuda do `pomodoro:` com uma palavra desconhecida.
pub(crate) const POMODORO_COMMAND_HELP: &str =
    "Use pomodoro:iniciar, pausar, retomar, parar, pular, 25, 50 ou 15.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroState {
    Stopped,
    Running,
    Paused,
}

/// Um tique a agendar: o evento leva `token`, e so o tique com o token da
/// cadeia viva conta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TickSchedule {
    pub(crate) token: u64,
    pub(crate) delay: Duration,
}

/// Quem entrega os tiques: na app o `Timers` (o evento `PomodoroTick` volta
/// ao event loop), nos testes um agendador de mentira. `run_command` e
/// `run_tick` agendam por aqui -- o `App` nao tem de se lembrar de o fazer.
pub(crate) trait TickScheduler {
    fn schedule(&self, tick: TickSchedule);
}

/// A janela no instante do fim de uma fase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowAttention {
    /// Minimizada: o aviso (um popup owned) esconde-se com a dona.
    pub(crate) minimized: bool,
    /// E a janela da frente (`GetForegroundWindow`).
    pub(crate) foreground: bool,
}

/// O que o `App` faz no fim de uma fase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PhaseEnd {
    /// O aviso a mostrar ja. `None` com a janela minimizada: o popup nao se
    /// via, e o aviso espera por `window_back`.
    pub(crate) show: Option<String>,
    /// Piscar o botao na barra de tarefas (`FlashWindowEx` com
    /// `FLASHW_TRAY | FLASHW_TIMERNOFG`: nao ativa nada nem rouba o foco, e
    /// para sozinho quando a janela volta a frente).
    pub(crate) flash: bool,
}

/// Um tique da cadeia viva, ja com o seguinte agendado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveTick {
    /// Fim de fase neste tique.
    pub(crate) phase_end: Option<PhaseEnd>,
}

/// O que um comando pede ao `App`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PomodoroOutcome {
    /// A nova cadeia de tiques (so quando fica a correr). Qualquer cadeia
    /// anterior morreu com este comando.
    pub(crate) tick: Option<TickSchedule>,
    /// O aviso centrado.
    pub(crate) notice: Option<String>,
    /// Opcoes novas a gravar em `<data_dir>/pomodoro`.
    pub(crate) save: Option<PomodoroSettings>,
}

/// O que um tique decide (`run_tick` faz o agendamento e o aviso).
#[derive(Debug, Clone, PartialEq, Eq)]
enum PomodoroTick {
    /// Tique de uma cadeia que ja morreu (pausa, paragem, novo arranque):
    /// nao toca no motor e nao se reagenda.
    Stale,
    Live {
        next: Option<TickSchedule>,
        /// Fim de fase neste tique: o aviso e o som.
        finished: Option<String>,
    },
}

/// Uma linha do menu do botao direito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PomodoroMenuItem {
    Command {
        /// Id do `AppendMenuW` (a partir de 1: 0 e "menu fechado").
        id: usize,
        label: &'static str,
        command: PomodoroCommand,
        enabled: bool,
        checked: bool,
    },
    Separator,
}

/// O menu para o estado atual: so a accao que se aplica (Iniciar, Pausar ou
/// Retomar), Pular fase e Parar (apagados com o Pomodoro parado), e os
/// presets com a marca no que esta em uso.
pub(crate) fn pomodoro_menu_items(
    state: PomodoroState,
    active: Option<PomodoroPreset>,
) -> Vec<PomodoroMenuItem> {
    let (label, command) = match state {
        PomodoroState::Stopped => ("Iniciar", PomodoroCommand::Start),
        PomodoroState::Running => ("Pausar", PomodoroCommand::Pause),
        PomodoroState::Paused => ("Retomar", PomodoroCommand::Resume),
    };
    let active_session = state != PomodoroState::Stopped;
    let mut items = vec![
        PomodoroMenuItem::Command {
            id: 1,
            label,
            command,
            enabled: true,
            checked: false,
        },
        PomodoroMenuItem::Command {
            id: 2,
            label: "Pular fase",
            command: PomodoroCommand::Skip,
            enabled: active_session,
            checked: false,
        },
        PomodoroMenuItem::Command {
            id: 3,
            label: "Parar",
            command: PomodoroCommand::Stop,
            enabled: active_session,
            checked: false,
        },
        PomodoroMenuItem::Separator,
    ];
    for (offset, preset) in PomodoroPreset::ALL.into_iter().enumerate() {
        items.push(PomodoroMenuItem::Command {
            id: 4 + offset,
            label: preset.label(),
            command: PomodoroCommand::Preset(preset),
            enabled: true,
            checked: active == Some(preset),
        });
    }
    items
}

/// O comando da linha que o `TrackPopupMenu` devolveu (0: fechado sem
/// escolha). Uma linha apagada nao faz nada.
pub(crate) fn pomodoro_menu_command(
    items: &[PomodoroMenuItem],
    picked: usize,
) -> Option<PomodoroCommand> {
    items.iter().find_map(|item| match *item {
        PomodoroMenuItem::Command {
            id,
            command,
            enabled: true,
            ..
        } if id == picked => Some(command),
        _ => None,
    })
}

/// O Pomodoro da app: o motor e o numero da cadeia de tiques viva.
///
/// Cada comando mata a cadeia que houver (a geracao sobe) e, se o Pomodoro
/// ficar a correr, devolve a nova. Assim ha no maximo UMA cadeia viva, e ela
/// existe exactamente enquanto o Pomodoro corre: parado ou pausado nao ha
/// tique nenhum.
#[derive(Debug, Clone)]
pub(crate) struct PomodoroController {
    timer: Pomodoro,
    generation: u64,
    /// O ultimo fim de fase que a janela pode nao ter visto (estava
    /// minimizada ou atras de outra): volta a aparecer quando ela voltar.
    unseen: Option<String>,
}

impl PomodoroController {
    pub(crate) fn new(settings: PomodoroSettings) -> Self {
        Self {
            timer: Pomodoro::new(settings),
            generation: 0,
            unseen: None,
        }
    }

    /// Um comando pelo caminho da app: aplica-o e agenda a cadeia nova (se
    /// o Pomodoro ficar a correr) em `timers`.
    pub(crate) fn run_command(
        &mut self,
        command: PomodoroCommand,
        now: Instant,
        timers: &impl TickScheduler,
    ) -> PomodoroOutcome {
        let outcome = self.command(command, now);
        if let Some(tick) = outcome.tick {
            timers.schedule(tick);
        }
        outcome
    }

    /// Um tique pelo caminho da app. `None`: tique de uma cadeia morta, nada
    /// a fazer. Vivo, o seguinte ja ficou agendado em `timers` -- sem isso a
    /// etiqueta congelava e nenhuma fase acabava -- e o fim de fase diz o
    /// que mostrar e se a barra de tarefas pisca, conforme a janela.
    pub(crate) fn run_tick(
        &mut self,
        token: u64,
        now: Instant,
        timers: &impl TickScheduler,
        window: WindowAttention,
    ) -> Option<LiveTick> {
        let PomodoroTick::Live { next, finished } = self.tick(token, now) else {
            return None;
        };
        if let Some(tick) = next {
            timers.schedule(tick);
        }
        let phase_end = finished.map(|message| {
            // Fora da frente nao ha garantia de que o aviso foi visto (o
            // popup owned fica atras de outra janela, ou some com a dona
            // minimizada): guarda-se para quando ela voltar.
            self.unseen = (!window.foreground).then(|| message.clone());
            PhaseEnd {
                show: (!window.minimized).then_some(message),
                flash: !window.foreground,
            }
        });
        Some(LiveTick { phase_end })
    }

    /// A janela voltou a frente (restaurada ou ativada): o fim de fase que
    /// ela nao viu, uma vez.
    pub(crate) fn window_back(&mut self) -> Option<String> {
        self.unseen.take()
    }

    pub(crate) fn state(&self) -> PomodoroState {
        if self.timer.is_running() {
            PomodoroState::Running
        } else if self.timer.is_paused() {
            PomodoroState::Paused
        } else {
            PomodoroState::Stopped
        }
    }

    #[cfg(test)]
    fn phase(&self) -> Phase {
        self.timer.phase()
    }

    #[cfg(test)]
    fn completed_focus(&self) -> u32 {
        self.timer.completed_focus()
    }

    pub(crate) fn preset(&self) -> Option<PomodoroPreset> {
        PomodoroPreset::of(self.timer.settings())
    }

    pub(crate) fn menu_items(&self) -> Vec<PomodoroMenuItem> {
        pomodoro_menu_items(self.state(), self.preset())
    }

    /// Fase a colorir no botao, enquanto ha uma sessao (a correr ou pausada).
    pub(crate) fn active_phase(&self) -> Option<Phase> {
        (self.state() != PomodoroState::Stopped).then(|| self.timer.phase())
    }

    /// O tempo ao lado do icone: "mm:ss" a correr, "⏸ mm:ss" pausado,
    /// nada parado.
    pub(crate) fn label(&self, now: Instant) -> Option<String> {
        let time = self.timer.remaining_label(now);
        match self.state() {
            PomodoroState::Running => Some(time),
            PomodoroState::Paused => Some(format!("{PAUSE_MARKER} {time}")),
            PomodoroState::Stopped => None,
        }
    }

    /// A dica do botao com uma sessao em curso: fase, o que falta, focos
    /// feitos e o que o clique faz agora. Parado: `None` (fica a dica fixa).
    pub(crate) fn hint(&self, now: Instant) -> Option<String> {
        let (paused, click) = match self.state() {
            PomodoroState::Stopped => return None,
            PomodoroState::Running => ("", "pausar"),
            PomodoroState::Paused => (" (pausado)", "retomar"),
        };
        let done = match self.timer.completed_focus() {
            1 => "1 foco concluído".to_string(),
            count => format!("{count} focos concluídos"),
        };
        Some(format!(
            "Pomodoro — {}{paused}: faltam {} · {done}\nClique: {click} · botão direito: opções",
            phase_name(self.timer.phase()),
            self.timer.remaining_label(now),
        ))
    }

    /// Aplica um comando em `now`.
    pub(crate) fn command(&mut self, command: PomodoroCommand, now: Instant) -> PomodoroOutcome {
        let mut save = None;
        let state = self.state();
        let notice = match command {
            PomodoroCommand::Click => match state {
                PomodoroState::Stopped => self.start(now),
                PomodoroState::Running => self.pause(now),
                PomodoroState::Paused => self.resume(now),
            },
            PomodoroCommand::Start => match state {
                PomodoroState::Stopped => self.start(now),
                PomodoroState::Paused => self.resume(now),
                PomodoroState::Running => Some(self.already_running(now)),
            },
            PomodoroCommand::Pause => match state {
                PomodoroState::Running => self.pause(now),
                PomodoroState::Paused => Some("O Pomodoro já está pausado.".to_string()),
                PomodoroState::Stopped => Some("O Pomodoro está parado.".to_string()),
            },
            PomodoroCommand::Resume => match state {
                PomodoroState::Paused => self.resume(now),
                PomodoroState::Running => Some(self.already_running(now)),
                PomodoroState::Stopped => {
                    Some("O Pomodoro está parado — use pomodoro:iniciar.".to_string())
                }
            },
            PomodoroCommand::Stop => match state {
                PomodoroState::Stopped => Some("O Pomodoro já está parado.".to_string()),
                PomodoroState::Running | PomodoroState::Paused => {
                    self.timer.stop();
                    Some("Pomodoro parado.".to_string())
                }
            },
            PomodoroCommand::Skip => match self.timer.skip(now) {
                Some(event) => Some(format!(
                    "Fase pulada. {}",
                    phase_end_message(event, self.timer.settings())
                )),
                None => Some("Nada para pular: o Pomodoro está parado.".to_string()),
            },
            PomodoroCommand::Preset(preset) => {
                let settings = preset.settings();
                if settings == self.timer.settings() {
                    // O preset que ja esta em uso: um clique distraido no
                    // menu nao deita fora o foco que vai a meio.
                    Some(format!("O Pomodoro já está em {}.", preset.label()))
                } else {
                    save = Some(settings);
                    self.timer = Pomodoro::new(settings);
                    if state == PomodoroState::Stopped {
                        Some(format!(
                            "Pomodoro: {} (pausa longa de {}).",
                            preset.label(),
                            duration_text(settings.long_break())
                        ))
                    } else {
                        // Recomeca limpo no foco com as duracoes novas; ver
                        // o gate `a_new_preset_restarts_a_session_cleanly`.
                        self.timer.start(now);
                        Some(format!(
                            "Pomodoro reiniciado: foco de {}.",
                            duration_text(settings.focus())
                        ))
                    }
                }
            }
        };
        PomodoroOutcome {
            tick: self.restart_chain(now),
            notice,
            save,
        }
    }

    /// Um tique chegou. So o da cadeia viva mexe no motor. Privado: a app
    /// passa por `run_tick`, que agenda o seguinte.
    fn tick(&mut self, token: u64, now: Instant) -> PomodoroTick {
        if token != self.generation || !self.timer.is_running() {
            return PomodoroTick::Stale;
        }
        let finished = self
            .timer
            .tick(now)
            .map(|event| phase_end_message(event, self.timer.settings()));
        PomodoroTick::Live {
            next: self.next_tick(token, now),
            finished,
        }
    }

    fn start(&mut self, now: Instant) -> Option<String> {
        self.timer.start(now).then(|| {
            format!(
                "Pomodoro iniciado: foco de {}",
                duration_text(self.timer.remaining(now))
            )
        })
    }

    fn pause(&mut self, now: Instant) -> Option<String> {
        // `pause` recusa em 00:00: a fase acabou e o tique que a fecha ja
        // esta a caminho.
        self.timer
            .pause(now)
            .then(|| format!("Pomodoro pausado em {}", self.timer.remaining_label(now)))
    }

    fn resume(&mut self, now: Instant) -> Option<String> {
        self.timer.resume(now).then(|| {
            format!(
                "Pomodoro retomado: faltam {}",
                self.timer.remaining_label(now)
            )
        })
    }

    fn already_running(&self, now: Instant) -> String {
        format!(
            "O Pomodoro já está rodando: {} com {} pela frente.",
            phase_name(self.timer.phase()),
            self.timer.remaining_label(now)
        )
    }

    fn restart_chain(&mut self, now: Instant) -> Option<TickSchedule> {
        self.generation = self.generation.wrapping_add(1);
        self.next_tick(self.generation, now)
    }

    fn next_tick(&self, token: u64, now: Instant) -> Option<TickSchedule> {
        self.timer.is_running().then(|| TickSchedule {
            token,
            delay: next_tick_delay(self.timer.remaining(now)),
        })
    }
}

/// As opcoes em texto, uma chave por linha, em minutos -- legivel e editavel
/// a mao, como o `<data_dir>/theme` e o `<data_dir>/gmail`.
pub(crate) fn settings_text(settings: PomodoroSettings) -> String {
    format!(
        "# NeuralIA — Pomodoro (minutos)\nfoco={}\npausa_curta={}\npausa_longa={}\npausa_longa_a_cada={}\n",
        settings.focus().as_secs() / 60,
        settings.short_break().as_secs() / 60,
        settings.long_break().as_secs() / 60,
        settings.long_break_every(),
    )
}

/// Leitura tolerante: cada chave em falta ou estragada fica no padrao, e o
/// conjunto que o motor recusar volta todo ao padrao. Nunca falha.
pub(crate) fn parse_settings(text: &str) -> PomodoroSettings {
    let defaults = PomodoroSettings::default();
    let mut focus = defaults.focus();
    let mut short_break = defaults.short_break();
    let mut long_break = defaults.long_break();
    let mut every = defaults.long_break_every();
    let minutes_value = |value: &str| {
        value
            .parse::<u64>()
            .ok()
            .filter(|count| (1..=MAX_MINUTES).contains(count))
            .map(minutes)
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "foco" => focus = minutes_value(value).unwrap_or(focus),
            "pausa_curta" => short_break = minutes_value(value).unwrap_or(short_break),
            "pausa_longa" => long_break = minutes_value(value).unwrap_or(long_break),
            "pausa_longa_a_cada" => {
                every = value
                    .parse::<u32>()
                    .ok()
                    .filter(|count| (1..=MAX_LONG_BREAK_EVERY).contains(count))
                    .unwrap_or(every);
            }
            _ => {}
        }
    }
    PomodoroSettings::new(focus, short_break, long_break, every).unwrap_or(defaults)
}

/// `<data_dir>/pomodoro`; sem ficheiro, ilegivel ou estragado: o padrao
/// (25 / 5 / 15, pausa longa a cada 4).
pub(crate) fn load_settings(path: &Path) -> PomodoroSettings {
    let mut text = String::new();
    match std::fs::File::open(path)
        .and_then(|file| file.take(SETTINGS_MAX_BYTES).read_to_string(&mut text))
    {
        Ok(_) => parse_settings(&text),
        Err(_) => PomodoroSettings::default(),
    }
}

/// Grava num temporario ao lado e troca de nome: um corte de energia a meio
/// deixa o ficheiro antigo inteiro, nunca meio escrito.
pub(crate) fn save_settings(path: &Path, settings: PomodoroSettings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(settings_text(settings).as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(count: u64) -> Duration {
        Duration::from_secs(count)
    }

    fn classic() -> PomodoroController {
        PomodoroController::new(PomodoroPreset::Classic.settings())
    }

    /// Gate: o clique esquerdo e o ciclo parado -> a correr -> pausado ->
    /// a correr, com o tempo congelado na pausa e a contar do ponto onde
    /// parou depois dela.
    #[test]
    fn click_starts_pauses_and_resumes() {
        let mut pomodoro = classic();
        let t0 = Instant::now();
        assert_eq!(pomodoro.state(), PomodoroState::Stopped);
        assert_eq!(pomodoro.label(t0), None);

        let started = pomodoro.command(PomodoroCommand::Click, t0);
        assert_eq!(pomodoro.state(), PomodoroState::Running);
        assert_eq!(
            started.notice.as_deref(),
            Some("Pomodoro iniciado: foco de 25 min")
        );
        assert!(started.tick.is_some(), "a correr ha tique");
        assert_eq!(pomodoro.label(t0 + secs(60)).as_deref(), Some("24:00"));

        let paused = pomodoro.command(PomodoroCommand::Click, t0 + secs(60));
        assert_eq!(pomodoro.state(), PomodoroState::Paused);
        assert_eq!(paused.tick, None, "pausado nao ha tique");
        assert_eq!(paused.notice.as_deref(), Some("Pomodoro pausado em 24:00"));
        // Uma hora de pausa nao gasta o foco.
        assert_eq!(pomodoro.label(t0 + secs(3600)).as_deref(), Some("⏸ 24:00"));

        let resumed = pomodoro.command(PomodoroCommand::Click, t0 + secs(3600));
        assert_eq!(pomodoro.state(), PomodoroState::Running);
        assert!(resumed.tick.is_some());
        assert_eq!(
            resumed.notice.as_deref(),
            Some("Pomodoro retomado: faltam 24:00")
        );
        assert_eq!(
            pomodoro.label(t0 + secs(3600 + 30)).as_deref(),
            Some("23:30")
        );

        // E de novo: o clique volta a pausar.
        pomodoro.command(PomodoroCommand::Click, t0 + secs(3700));
        assert_eq!(pomodoro.state(), PomodoroState::Paused);
        // Parar desliga tudo; o proximo clique arranca do inicio do foco.
        let stopped = pomodoro.command(PomodoroCommand::Stop, t0 + secs(3701));
        assert_eq!(stopped.tick, None);
        assert_eq!(pomodoro.state(), PomodoroState::Stopped);
        pomodoro.command(PomodoroCommand::Click, t0 + secs(4000));
        assert_eq!(pomodoro.label(t0 + secs(4000)).as_deref(), Some("25:00"));
    }

    /// A janela a frente: o aviso aparece ja e nada pisca.
    const FRONT: WindowAttention = WindowAttention {
        minimized: false,
        foreground: true,
    };

    /// Um agendador de mentira com o relogio do teste: guarda os tiques que
    /// o caminho da app (`run_command`, `run_tick`) lhe pede e entrega-os
    /// quando o relogio passa por eles. Nao dorme.
    struct FakeTimers {
        now: std::cell::Cell<Instant>,
        queue: std::cell::RefCell<Vec<(Instant, u64)>>,
    }

    impl TickScheduler for FakeTimers {
        fn schedule(&self, tick: TickSchedule) {
            self.queue
                .borrow_mut()
                .push((self.now.get() + tick.delay, tick.token));
        }
    }

    impl FakeTimers {
        fn new(now: Instant) -> Self {
            Self {
                now: std::cell::Cell::new(now),
                queue: std::cell::RefCell::new(Vec::new()),
            }
        }

        /// Um comando em `at`, como a app o da: o tique fica agendado aqui.
        fn command(
            &self,
            pomodoro: &mut PomodoroController,
            command: PomodoroCommand,
            at: Instant,
        ) -> PomodoroOutcome {
            self.now.set(at);
            pomodoro.run_command(command, at, self)
        }

        fn pending(&self) -> usize {
            self.queue.borrow().len()
        }

        /// Tiques ainda por entregar que a cadeia viva reconhece.
        fn live(&self, pomodoro: &PomodoroController) -> usize {
            self.queue
                .borrow()
                .iter()
                .filter(|(_, token)| *token == pomodoro.generation)
                .count()
        }

        /// Entrega tudo o que vence ate `until`, por ordem, como o
        /// `neural-timers`, pelo `run_tick` da app; devolve os avisos de fim
        /// de fase.
        fn run_until(&self, pomodoro: &mut PomodoroController, until: Instant) -> Vec<String> {
            let mut finished = Vec::new();
            loop {
                let next = {
                    let mut queue = self.queue.borrow_mut();
                    queue.sort_by_key(|(due, _)| *due);
                    match queue.first() {
                        Some(&(due, token)) if due <= until => {
                            queue.remove(0);
                            Some((due, token))
                        }
                        _ => None,
                    }
                };
                let Some((due, token)) = next else {
                    break;
                };
                self.now.set(due);
                if let Some(LiveTick {
                    phase_end: Some(end),
                }) = pomodoro.run_tick(token, due, self, FRONT)
                {
                    finished.extend(end.show);
                }
                assert!(self.live(pomodoro) <= 1, "duas cadeias vivas");
            }
            finished
        }
    }

    /// Gate: ha no maximo UMA cadeia de tiques viva, e so enquanto o
    /// Pomodoro corre. Um tique velho (de antes de uma pausa, paragem ou
    /// novo arranque) nao mexe no motor nem se reagenda -- mesmo entregue
    /// depois do fim da fase.
    #[test]
    fn only_one_tick_chain_is_ever_alive() {
        let mut pomodoro = classic();
        let timers = FakeTimers::new(Instant::now());
        let t0 = timers.now.get();

        let check = |pomodoro: &PomodoroController, timers: &FakeTimers, at: &str| {
            let expected = usize::from(pomodoro.state() == PomodoroState::Running);
            assert_eq!(timers.live(pomodoro), expected, "{at}");
        };

        timers.command(&mut pomodoro, PomodoroCommand::Click, t0);
        check(&pomodoro, &timers, "iniciado");
        // Cliques e comandos repetidos: pausa, retoma, "iniciar" a correr,
        // pausa, retoma -- cada um deixa tiques velhos na fila.
        let mut now = t0;
        for (step, command) in [
            PomodoroCommand::Click,
            PomodoroCommand::Click,
            PomodoroCommand::Start,
            PomodoroCommand::Resume,
            PomodoroCommand::Click,
            PomodoroCommand::Start,
        ]
        .into_iter()
        .enumerate()
        {
            now += Duration::from_millis(300);
            timers.command(&mut pomodoro, command, now);
            check(&pomodoro, &timers, &format!("passo {step} ({command:?})"));
        }
        assert_eq!(pomodoro.state(), PomodoroState::Running);

        // Dez minutos de tiques: um por segundo, nunca dois.
        let ten = now + secs(600);
        let finished = timers.run_until(&mut pomodoro, ten);
        assert!(finished.is_empty());
        let delivered = timers.pending();
        assert_eq!(delivered, 1, "so a cadeia viva continua na fila");
        check(&pomodoro, &timers, "dez minutos depois");

        // Pausado: o tique que estava na fila chega e morre ali, mesmo muito
        // depois do fim da fase.
        timers.command(&mut pomodoro, PomodoroCommand::Pause, ten);
        let label = pomodoro.label(ten);
        let finished = timers.run_until(&mut pomodoro, ten + secs(3 * 3600));
        assert!(finished.is_empty(), "um tique velho acabou a fase pausada");
        assert_eq!(timers.pending(), 0, "o tique velho reagendou-se");
        assert_eq!(pomodoro.label(ten + secs(3 * 3600)), label);
        check(&pomodoro, &timers, "pausado");

        // Retoma: a cadeia nova leva o foco ate ao fim, com um aviso so.
        let back = ten + secs(3 * 3600);
        timers.command(&mut pomodoro, PomodoroCommand::Resume, back);
        // Faltavam 898,8 s (601,2 s corridos); a pausa curta vai ate 1198,8.
        let finished = timers.run_until(&mut pomodoro, back + secs(1000));
        assert_eq!(finished, vec!["Foco concluído! Pausa curta de 5 min"]);
        assert_eq!(pomodoro.phase(), Phase::ShortBreak);
        check(&pomodoro, &timers, "na pausa curta");

        // Parado: nenhum tique fica vivo.
        let end = back + secs(1000);
        timers.command(&mut pomodoro, PomodoroCommand::Stop, end);
        let finished = timers.run_until(&mut pomodoro, end + secs(3600));
        assert!(finished.is_empty());
        assert_eq!(timers.pending(), 0);
        check(&pomodoro, &timers, "parado");
    }

    /// Gate: o tique cai no segundo em que o "mm:ss" muda, e o fim da fase
    /// e visto no proprio tique em que acontece.
    #[test]
    fn ticks_land_when_the_label_changes() {
        assert_eq!(
            next_tick_delay(Duration::from_millis(12_400)),
            Duration::from_millis(400)
        );
        assert_eq!(next_tick_delay(secs(12)), secs(1));
        assert_eq!(next_tick_delay(Duration::ZERO), Duration::ZERO);

        let mut pomodoro = classic();
        let timers = FakeTimers::new(Instant::now());
        let t0 = timers.now.get();
        let first = timers
            .command(&mut pomodoro, PomodoroCommand::Click, t0)
            .tick
            .expect("tique");
        // Retomado a meio de um segundo: o primeiro tique acerta o passo.
        timers.command(
            &mut pomodoro,
            PomodoroCommand::Pause,
            t0 + Duration::from_millis(1_600),
        );
        let resumed = timers
            .command(&mut pomodoro, PomodoroCommand::Resume, t0 + secs(10))
            .tick
            .expect("tique");
        assert_eq!(first.delay, secs(1));
        assert_eq!(resumed.delay, Duration::from_millis(400));
        let before = pomodoro.label(t0 + secs(10)).expect("etiqueta");
        let after = pomodoro
            .label(t0 + secs(10) + resumed.delay)
            .expect("etiqueta");
        assert_eq!((before.as_str(), after.as_str()), ("24:59", "24:58"));

        // Tique a tique ate ao fim do foco (faltavam 1498,4 s): o ultimo
        // tique cai no instante exacto do fim, e e ele que o anuncia. Os
        // tiques das cadeias mortas (o do arranque) morrem pelo caminho.
        let end = t0 + secs(10) + Duration::from_millis(1_498_400);
        assert!(
            timers
                .run_until(&mut pomodoro, end - Duration::from_nanos(1))
                .is_empty()
        );
        let finished = timers.run_until(&mut pomodoro, end);
        assert_eq!(finished, vec!["Foco concluído! Pausa curta de 5 min"]);
    }

    /// Gate: um fim de fase com a janela minimizada ou atras de outra nao se
    /// perde. O popup do aviso e owned pela janela e some com ela: a barra
    /// de tarefas pisca (sem roubar o foco) e o aviso aparece quando ela
    /// volta -- uma vez. A frente, aparece ja e nada pisca.
    #[test]
    fn a_phase_end_the_window_did_not_see_waits_for_it() {
        let finished = "Foco concluído! Pausa curta de 5 min".to_string();
        for (window, show_now, flash, later) in [
            (FRONT, true, false, false),
            (
                WindowAttention {
                    minimized: true,
                    foreground: false,
                },
                false,
                true,
                true,
            ),
            (
                WindowAttention {
                    minimized: false,
                    foreground: false,
                },
                true,
                true,
                true,
            ),
        ] {
            let mut pomodoro = classic();
            let timers = FakeTimers::new(Instant::now());
            let t0 = timers.now.get();
            timers.command(&mut pomodoro, PomodoroCommand::Click, t0);
            let end = t0 + secs(25 * 60);
            // Tique a tique ate ao fim do foco, pelo caminho da app.
            let mut seen = None;
            loop {
                let next = timers.queue.borrow_mut().pop();
                let Some((due, token)) = next else {
                    break;
                };
                timers.now.set(due);
                let at = if due >= end { window } else { FRONT };
                if let Some(LiveTick {
                    phase_end: Some(phase_end),
                }) = pomodoro.run_tick(token, due, &timers, at)
                {
                    seen = Some(phase_end);
                    break;
                }
            }
            let phase_end = seen.expect("o foco acabou");
            assert_eq!(
                phase_end.show.as_deref(),
                show_now.then_some(finished.as_str()),
                "{window:?}"
            );
            assert_eq!(phase_end.flash, flash, "{window:?}");
            assert_eq!(
                pomodoro.window_back().as_deref(),
                later.then_some(finished.as_str()),
                "{window:?}"
            );
            assert_eq!(pomodoro.window_back(), None, "uma vez so: {window:?}");
            assert_eq!(timers.live(&pomodoro), 1, "a pausa curta continua a contar");
        }
    }

    /// Gate: o texto de cada fim de fase, com as duracoes em uso.
    #[test]
    fn phase_end_messages_name_the_next_phase() {
        let classic = PomodoroPreset::Classic.settings();
        let long = PomodoroPreset::Long.settings();
        let cases = [
            (
                PomodoroEvent::FocusFinished {
                    completed_focus: 1,
                    next: Phase::ShortBreak,
                },
                classic,
                "Foco concluído! Pausa curta de 5 min",
            ),
            (
                PomodoroEvent::FocusFinished {
                    completed_focus: 4,
                    next: Phase::LongBreak,
                },
                classic,
                "4 focos concluídos! Pausa longa de 15 min",
            ),
            (
                PomodoroEvent::BreakFinished { next: Phase::Focus },
                classic,
                "Pausa terminada — hora de focar (25 min)",
            ),
            (
                PomodoroEvent::FocusFinished {
                    completed_focus: 8,
                    next: Phase::LongBreak,
                },
                long,
                "8 focos concluídos! Pausa longa de 30 min",
            ),
            (
                PomodoroEvent::FocusFinished {
                    completed_focus: 2,
                    next: Phase::ShortBreak,
                },
                long,
                "Foco concluído! Pausa curta de 10 min",
            ),
            (
                PomodoroEvent::BreakFinished { next: Phase::Focus },
                long,
                "Pausa terminada — hora de focar (50 min)",
            ),
        ];
        for (event, settings, text) in cases {
            assert_eq!(phase_end_message(event, settings), text, "{event:?}");
        }

        // E pelo caminho do tique: o quarto foco anuncia a pausa longa.
        let mut pomodoro = classic_controller_at_fourth_focus();
        let t = pomodoro.1;
        let token = pomodoro.0.generation;
        assert_eq!(
            pomodoro.0.tick(token, t + secs(25 * 60)),
            PomodoroTick::Live {
                next: Some(TickSchedule {
                    token,
                    delay: secs(1)
                }),
                finished: Some("4 focos concluídos! Pausa longa de 15 min".to_string()),
            }
        );
        assert_eq!(pomodoro.0.phase(), Phase::LongBreak);
    }

    /// Um Pomodoro classico no inicio do quarto foco, e o instante.
    fn classic_controller_at_fourth_focus() -> (PomodoroController, Instant) {
        let mut pomodoro = classic();
        let mut t = Instant::now();
        pomodoro.command(PomodoroCommand::Click, t);
        for _ in 0..3 {
            t += secs(25 * 60);
            let token = pomodoro.generation;
            assert!(matches!(
                pomodoro.tick(token, t),
                PomodoroTick::Live {
                    finished: Some(_),
                    ..
                }
            ));
            t += secs(5 * 60);
            assert!(matches!(
                pomodoro.tick(token, t),
                PomodoroTick::Live {
                    finished: Some(_),
                    ..
                }
            ));
        }
        assert_eq!(pomodoro.completed_focus(), 3);
        assert_eq!(pomodoro.phase(), Phase::Focus);
        (pomodoro, t)
    }

    /// Gate: a etiqueta a correr, pausada e parada, e a dica com fase, tempo
    /// e contagem.
    #[test]
    fn label_and_hint_follow_the_session() {
        let mut pomodoro = classic();
        let t0 = Instant::now();
        assert_eq!(pomodoro.label(t0), None);
        assert_eq!(pomodoro.hint(t0), None, "parado fica a dica fixa");
        assert_eq!(pomodoro.active_phase(), None);

        pomodoro.command(PomodoroCommand::Start, t0);
        // Arredonda para cima: com 0,4 s a faltar ainda se ve 00:01.
        let late = t0 + secs(25 * 60) - Duration::from_millis(400);
        assert_eq!(pomodoro.label(late).as_deref(), Some("00:01"));
        assert_eq!(pomodoro.label(t0 + secs(754)).as_deref(), Some("12:26"));
        assert_eq!(
            pomodoro.hint(t0 + secs(754)).as_deref(),
            Some(
                "Pomodoro — Foco: faltam 12:26 · 0 focos concluídos\nClique: pausar · botão direito: opções"
            )
        );
        assert_eq!(pomodoro.active_phase(), Some(Phase::Focus));

        pomodoro.command(PomodoroCommand::Pause, t0 + secs(754));
        assert_eq!(pomodoro.label(t0 + secs(9999)).as_deref(), Some("⏸ 12:26"));
        assert_eq!(
            pomodoro.hint(t0 + secs(9999)).as_deref(),
            Some(
                "Pomodoro — Foco (pausado): faltam 12:26 · 0 focos concluídos\nClique: retomar · botão direito: opções"
            )
        );
        assert_eq!(pomodoro.active_phase(), Some(Phase::Focus));

        // Um foco feito: a pausa curta a verde, contagem no singular.
        pomodoro.command(PomodoroCommand::Resume, t0 + secs(9999));
        let token = pomodoro.generation;
        let end = t0 + secs(9999 + 754);
        assert!(matches!(
            pomodoro.tick(token, end),
            PomodoroTick::Live {
                finished: Some(_),
                ..
            }
        ));
        assert_eq!(pomodoro.active_phase(), Some(Phase::ShortBreak));
        assert_eq!(phase_color(Phase::ShortBreak), BREAK_COLOR);
        assert_eq!(phase_color(Phase::Focus), FOCUS_COLOR);
        assert_eq!(
            pomodoro.hint(end).as_deref(),
            Some(
                "Pomodoro — Pausa curta: faltam 05:00 · 1 foco concluído\nClique: pausar · botão direito: opções"
            )
        );
    }

    /// Gate: o menu do botao direito no estado de cada momento, a marca no
    /// preset em uso e o id devolvido pelo Windows ligado ao comando certo.
    #[test]
    fn menu_items_map_to_commands() {
        let t0 = Instant::now();
        let mut pomodoro = classic();
        for (state, first, first_command) in [
            (PomodoroState::Stopped, "Iniciar", PomodoroCommand::Start),
            (PomodoroState::Running, "Pausar", PomodoroCommand::Pause),
            (PomodoroState::Paused, "Retomar", PomodoroCommand::Resume),
        ] {
            if state != PomodoroState::Stopped {
                pomodoro.command(PomodoroCommand::Click, t0);
            }
            assert_eq!(pomodoro.state(), state);
            let items = pomodoro.menu_items();
            let labels: Vec<&str> = items
                .iter()
                .map(|item| match item {
                    PomodoroMenuItem::Command { label, .. } => *label,
                    PomodoroMenuItem::Separator => "---",
                })
                .collect();
            assert_eq!(
                labels,
                [
                    first,
                    "Pular fase",
                    "Parar",
                    "---",
                    "25 / 5 min (padrão)",
                    "50 / 10 min",
                    "15 / 3 min"
                ],
                "{state:?}"
            );
            let active = state != PomodoroState::Stopped;
            assert_eq!(pomodoro_menu_command(&items, 1), Some(first_command));
            // Parado, Pular e Parar estao apagados e nao fazem nada.
            assert_eq!(
                pomodoro_menu_command(&items, 2),
                active.then_some(PomodoroCommand::Skip)
            );
            assert_eq!(
                pomodoro_menu_command(&items, 3),
                active.then_some(PomodoroCommand::Stop)
            );
            for (id, preset) in [
                (4, PomodoroPreset::Classic),
                (5, PomodoroPreset::Long),
                (6, PomodoroPreset::Short),
            ] {
                assert_eq!(
                    pomodoro_menu_command(&items, id),
                    Some(PomodoroCommand::Preset(preset))
                );
            }
            assert_eq!(pomodoro_menu_command(&items, 0), None, "menu fechado");
            assert_eq!(pomodoro_menu_command(&items, 7), None);
        }

        // A marca segue o preset em uso -- e nenhum com duracoes a mao.
        for (preset, checked_id) in [
            (Some(PomodoroPreset::Classic), Some(4)),
            (Some(PomodoroPreset::Long), Some(5)),
            (Some(PomodoroPreset::Short), Some(6)),
            (None, None),
        ] {
            let checked: Vec<usize> = pomodoro_menu_items(PomodoroState::Stopped, preset)
                .iter()
                .filter_map(|item| match item {
                    PomodoroMenuItem::Command {
                        id, checked: true, ..
                    } => Some(*id),
                    _ => None,
                })
                .collect();
            assert_eq!(checked, checked_id.into_iter().collect::<Vec<_>>());
        }
        let custom =
            PomodoroSettings::new(secs(20 * 60), secs(5 * 60), secs(15 * 60), 4).expect("valido");
        assert_eq!(PomodoroController::new(custom).preset(), None);
        assert_eq!(
            PomodoroController::new(PomodoroPreset::Long.settings()).preset(),
            Some(PomodoroPreset::Long)
        );
    }

    /// Gate da decisao sobre trocar de preset com uma sessao em curso:
    /// recomeca LIMPO no foco, com as duracoes novas, a correr, e grava. O
    /// motor nao tem como trocar duracoes a meio de uma fase (o "faltam"
    /// de um foco de 25 nao tem traducao honesta num de 50); recomecar e
    /// previsivel e o aviso diz o que aconteceu. O preset que ja esta em uso
    /// nao recomeca nada: um clique distraido nao deita fora o foco.
    #[test]
    fn a_new_preset_restarts_a_session_cleanly() {
        let t0 = Instant::now();
        let (mut pomodoro, t) = classic_controller_at_fourth_focus();
        let at = t + secs(600);

        // O mesmo preset: nada muda, nada se grava.
        let same = pomodoro.command(PomodoroCommand::Preset(PomodoroPreset::Classic), at);
        assert_eq!(same.save, None);
        assert_eq!(pomodoro.completed_focus(), 3);
        assert_eq!(pomodoro.label(at).as_deref(), Some("15:00"));
        assert!(same.tick.is_some(), "a sessao continua a correr");

        // Outro preset a correr: foco novo de 50 min, contagem a zero.
        let changed = pomodoro.command(PomodoroCommand::Preset(PomodoroPreset::Long), at);
        assert_eq!(changed.save, Some(PomodoroPreset::Long.settings()));
        assert_eq!(pomodoro.state(), PomodoroState::Running);
        assert_eq!(pomodoro.phase(), Phase::Focus);
        assert_eq!(pomodoro.completed_focus(), 0);
        assert_eq!(pomodoro.label(at).as_deref(), Some("50:00"));
        assert_eq!(
            changed.notice.as_deref(),
            Some("Pomodoro reiniciado: foco de 50 min.")
        );
        assert!(changed.tick.is_some());

        // Pausado tambem recomeca a correr (a pausa era da sessao antiga).
        pomodoro.command(PomodoroCommand::Pause, at + secs(5));
        let from_pause =
            pomodoro.command(PomodoroCommand::Preset(PomodoroPreset::Short), at + secs(9));
        assert_eq!(pomodoro.state(), PomodoroState::Running);
        assert_eq!(pomodoro.label(at + secs(9)).as_deref(), Some("15:00"));
        assert_eq!(from_pause.save, Some(PomodoroPreset::Short.settings()));

        // Parado: so muda as duracoes; fica parado e sem tique.
        let mut stopped = classic();
        let quiet = stopped.command(PomodoroCommand::Preset(PomodoroPreset::Long), t0);
        assert_eq!(stopped.state(), PomodoroState::Stopped);
        assert_eq!(quiet.tick, None);
        assert_eq!(quiet.save, Some(PomodoroPreset::Long.settings()));
        assert_eq!(
            quiet.notice.as_deref(),
            Some("Pomodoro: 50 / 10 min (pausa longa de 30 min).")
        );
        stopped.command(PomodoroCommand::Click, t0);
        assert_eq!(stopped.label(t0).as_deref(), Some("50:00"));
    }

    /// Gate: as opcoes voltam iguais do disco; ficheiro estragado, vazio ou
    /// com valores que o motor recusa volta ao padrao, sem falhar.
    #[test]
    fn settings_round_trip_and_survive_a_corrupted_file() {
        let dir = std::env::temp_dir().join(format!(
            "neuralia-pomodoro-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("pomodoro");

        assert_eq!(
            load_settings(&path),
            PomodoroSettings::default(),
            "sem ficheiro"
        );
        for preset in PomodoroPreset::ALL {
            save_settings(&path, preset.settings()).expect("grava");
            assert_eq!(load_settings(&path), preset.settings(), "{preset:?}");
            assert_eq!(PomodoroPreset::of(load_settings(&path)), Some(preset));
        }
        assert!(!path.with_extension("tmp").exists(), "o temporario ficou");

        let default = PomodoroSettings::default();
        // Bytes sem sentido (nem UTF-8): padrao.
        std::fs::write(&path, [0xFF, 0xFE, 0x00, 0x9C, b'=', 0x80]).expect("escreve");
        assert_eq!(load_settings(&path), default);
        // Vazio: padrao.
        std::fs::write(&path, "").expect("escreve");
        assert_eq!(load_settings(&path), default);
        // Zeros, negativos, texto, fora do limite: cada chave fica no padrao.
        std::fs::write(
            &path,
            "foco=0\npausa_curta=-5\npausa_longa=quinze\npausa_longa_a_cada=0\n",
        )
        .expect("escreve");
        assert_eq!(load_settings(&path), default);
        std::fs::write(&path, "foco=100\npausa_longa_a_cada=99999999999\n").expect("escreve");
        assert_eq!(load_settings(&path), default);
        // Chaves validas sobrevivem ao lixo a volta delas.
        std::fs::write(&path, "lixo\n foco = 40 \n???=\npausa_curta=8").expect("escreve");
        let partial = load_settings(&path);
        assert_eq!(partial.focus(), secs(40 * 60));
        assert_eq!(partial.short_break(), secs(8 * 60));
        assert_eq!(partial.long_break(), default.long_break());
        assert_eq!(partial.long_break_every(), default.long_break_every());
        // Um ficheiro enorme nao e lido inteiro nem rebenta.
        let mut huge = "foco=30\n".to_string();
        huge.push_str(&"x".repeat(1 << 20));
        std::fs::write(&path, huge).expect("escreve");
        assert_eq!(load_settings(&path).focus(), secs(30 * 60));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Gate: as palavras do `pomodoro:` (depois do prefixo, que a omnibox
    /// tira) e a ajuda para as que nao existem.
    #[test]
    fn command_words_parse() {
        use PomodoroCommand::*;
        for (word, command) in [
            ("", Start),
            ("  ", Start),
            ("iniciar", Start),
            ("INICIAR", Start),
            ("começar", Start),
            ("start", Start),
            ("pausar", Pause),
            ("Pausa", Pause),
            ("retomar", Resume),
            ("continuar", Resume),
            ("parar", Stop),
            ("stop", Stop),
            ("pular", Skip),
            ("skip", Skip),
            ("25", Preset(PomodoroPreset::Classic)),
            (" 50 ", Preset(PomodoroPreset::Long)),
            ("15 min", Preset(PomodoroPreset::Short)),
            ("15min", Preset(PomodoroPreset::Short)),
        ] {
            assert_eq!(parse_pomodoro_command(word), Some(command), "{word:?}");
        }
        for word in ["30", "25x", "abrir", "min", "pomodoro"] {
            assert_eq!(parse_pomodoro_command(word), None, "{word:?}");
        }
    }
}
