//! Pomodoro: a maquina de estados do temporizador de foco da NeuralIA.
//!
//! Nao le o relogio. Cada metodo que precisa de tempo recebe `now`: quem
//! desenha a barra passa o `Instant::now()` do seu timer, e os testes andam
//! horas num instante sem dormir. Sem Win32, sem threads, sem estado global.

use std::time::{Duration, Instant};

use thiserror::Error;

/// Fase do ciclo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}

/// Configuracao rejeitada por [`PomodoroSettings::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PomodoroSettingsError {
    #[error("a duração da fase {0:?} não pode ser zero")]
    ZeroDuration(Phase),
    #[error("a pausa longa tem de vir a cada 1 ou mais focos")]
    ZeroLongBreakEvery,
}

/// Duracoes do ciclo. So existe validada: via [`PomodoroSettings::new`] ou
/// `Default` (25 / 5 / 15 min, pausa longa a cada 4 focos).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PomodoroSettings {
    focus: Duration,
    short_break: Duration,
    long_break: Duration,
    long_break_every: u32,
}

impl PomodoroSettings {
    /// Valida e constroi.
    pub fn new(
        focus: Duration,
        short_break: Duration,
        long_break: Duration,
        long_break_every: u32,
    ) -> Result<Self, PomodoroSettingsError> {
        // Uma fase de zero acaba no proprio tick que a comeca: o ciclo daria
        // uma volta por tick, cada fase com o seu aviso no ecra.
        for (phase, duration) in [
            (Phase::Focus, focus),
            (Phase::ShortBreak, short_break),
            (Phase::LongBreak, long_break),
        ] {
            if duration.is_zero() {
                return Err(PomodoroSettingsError::ZeroDuration(phase));
            }
        }
        // Nenhum contador e multiplo de zero: a pausa longa nunca chegaria.
        if long_break_every == 0 {
            return Err(PomodoroSettingsError::ZeroLongBreakEvery);
        }
        Ok(Self {
            focus,
            short_break,
            long_break,
            long_break_every,
        })
    }

    pub fn focus(&self) -> Duration {
        self.focus
    }

    pub fn short_break(&self) -> Duration {
        self.short_break
    }

    pub fn long_break(&self) -> Duration {
        self.long_break
    }

    pub fn long_break_every(&self) -> u32 {
        self.long_break_every
    }

    /// Duracao inteira de `phase`.
    pub fn duration(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Focus => self.focus,
            Phase::ShortBreak => self.short_break,
            Phase::LongBreak => self.long_break,
        }
    }
}

impl Default for PomodoroSettings {
    fn default() -> Self {
        Self {
            focus: Duration::from_secs(25 * 60),
            short_break: Duration::from_secs(5 * 60),
            long_break: Duration::from_secs(15 * 60),
            long_break_every: 4,
        }
    }
}

/// Fim de uma fase, reportado uma unica vez por [`Pomodoro::tick`] ou
/// [`Pomodoro::skip`]. E a deixa para o aviso centrado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PomodoroEvent {
    /// Acabou um foco; `completed_focus` ja o inclui.
    FocusFinished { completed_focus: u32, next: Phase },
    /// Acabou uma pausa; `next` e sempre `Focus`.
    BreakFinished { next: Phase },
}

impl PomodoroEvent {
    /// Fase que comecou com este evento.
    pub fn next(self) -> Phase {
        match self {
            PomodoroEvent::FocusFinished { next, .. } | PomodoroEvent::BreakFinished { next } => {
                next
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clock {
    Idle,
    Running { ends_at: Instant },
    // Guarda-se o que falta, nao o instante do fim: enquanto pausado o tempo
    // nao corre, e o fim so volta a existir no `resume`.
    Paused { remaining: Duration },
}

/// O temporizador. Comeca parado, no foco, com o contador a zero.
#[derive(Debug, Clone)]
pub struct Pomodoro {
    settings: PomodoroSettings,
    phase: Phase,
    completed_focus: u32,
    clock: Clock,
}

impl Default for Pomodoro {
    fn default() -> Self {
        Self::new(PomodoroSettings::default())
    }
}

impl Pomodoro {
    pub fn new(settings: PomodoroSettings) -> Self {
        Self {
            settings,
            phase: Phase::Focus,
            completed_focus: 0,
            clock: Clock::Idle,
        }
    }

    pub fn settings(&self) -> PomodoroSettings {
        self.settings
    }

    /// Arranca a fase atual do inicio. So age parado; devolve se arrancou.
    pub fn start(&mut self, now: Instant) -> bool {
        if self.clock != Clock::Idle {
            return false;
        }
        self.clock = Clock::Running {
            ends_at: now + self.settings.duration(self.phase),
        };
        true
    }

    /// Congela o que falta. So age a correr; devolve se pausou.
    pub fn pause(&mut self, now: Instant) -> bool {
        let Clock::Running { ends_at } = self.clock else {
            return false;
        };
        let remaining = ends_at.saturating_duration_since(now);
        // A fase ja acabou e a transicao e do proximo `tick`. Uma pausa em
        // 00:00 engoliria o aviso ate alguem retomar.
        if remaining.is_zero() {
            return false;
        }
        self.clock = Clock::Paused { remaining };
        true
    }

    /// Retoma de onde pausou. So age pausado; devolve se retomou.
    pub fn resume(&mut self, now: Instant) -> bool {
        let Clock::Paused { remaining } = self.clock else {
            return false;
        };
        self.clock = Clock::Running {
            ends_at: now + remaining,
        };
        true
    }

    /// Volta ao estado inicial: parado, no foco, contador a zero.
    pub fn stop(&mut self) {
        self.phase = Phase::Focus;
        self.completed_focus = 0;
        self.clock = Clock::Idle;
    }

    /// Da a fase atual por terminada em `now` e arranca a seguinte a correr,
    /// mesmo que estivesse pausada. Parado nao ha o que saltar: `None`.
    pub fn skip(&mut self, now: Instant) -> Option<PomodoroEvent> {
        if self.clock == Clock::Idle {
            return None;
        }
        // Um foco saltado conta como feito: o contador e a posicao no ciclo,
        // e se saltar nao contasse, quem salta focos nunca chegaria a pausa
        // longa.
        Some(self.advance(now))
    }

    /// Reporta o fim da fase atual, uma unica vez, e arranca a seguinte.
    ///
    /// Um tick muito depois do fim (o PC dormiu) reporta UMA transicao, e a
    /// fase seguinte conta a partir desse tick, inteira. Nao se repoem fases
    /// perdidas.
    pub fn tick(&mut self, now: Instant) -> Option<PomodoroEvent> {
        let Clock::Running { ends_at } = self.clock else {
            return None;
        };
        if now < ends_at {
            return None;
        }
        Some(self.advance(now))
    }

    fn advance(&mut self, now: Instant) -> PomodoroEvent {
        let event = match self.phase {
            Phase::Focus => {
                self.completed_focus = self.completed_focus.saturating_add(1);
                let next = if self
                    .completed_focus
                    .is_multiple_of(self.settings.long_break_every)
                {
                    Phase::LongBreak
                } else {
                    Phase::ShortBreak
                };
                PomodoroEvent::FocusFinished {
                    completed_focus: self.completed_focus,
                    next,
                }
            }
            Phase::ShortBreak | Phase::LongBreak => {
                PomodoroEvent::BreakFinished { next: Phase::Focus }
            }
        };
        self.phase = event.next();
        // Conta a partir de `now`, nao do fim da fase anterior: depois de 3 h
        // de suspensao, encadear a partir do fim antigo despejaria uma rajada
        // de avisos de fases que ninguem viveu.
        self.clock = Clock::Running {
            ends_at: now + self.settings.duration(self.phase),
        };
        event
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// A contar agora (nem parado nem pausado).
    pub fn is_running(&self) -> bool {
        matches!(self.clock, Clock::Running { .. })
    }

    pub fn is_paused(&self) -> bool {
        matches!(self.clock, Clock::Paused { .. })
    }

    /// Focos terminados desde o ultimo `stop`.
    pub fn completed_focus(&self) -> u32 {
        self.completed_focus
    }

    /// Tempo que falta na fase atual. Parado: a fase inteira.
    pub fn remaining(&self, now: Instant) -> Duration {
        match self.clock {
            Clock::Idle => self.settings.duration(self.phase),
            Clock::Running { ends_at } => ends_at.saturating_duration_since(now),
            Clock::Paused { remaining } => remaining,
        }
    }

    /// `remaining` como "mm:ss" (os minutos podem passar de 99).
    pub fn remaining_label(&self, now: Instant) -> String {
        let remaining = self.remaining(now);
        // Arredonda para cima: com 0,4 s a faltar a barra mostra "00:01".
        // "00:00" so aparece quando a fase acabou de facto.
        let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn mins(m: u64) -> Duration {
        Duration::from_secs(m * 60)
    }

    const NANO: Duration = Duration::from_nanos(1);

    #[test]
    fn default_cycle_takes_long_break_after_every_fourth_focus() {
        let mut p = Pomodoro::default();
        let mut t = Instant::now();
        assert!(p.start(t));

        let breaks = [
            (Phase::ShortBreak, mins(5)),
            (Phase::ShortBreak, mins(5)),
            (Phase::ShortBreak, mins(5)),
            (Phase::LongBreak, mins(15)),
            (Phase::ShortBreak, mins(5)),
            (Phase::ShortBreak, mins(5)),
            (Phase::ShortBreak, mins(5)),
            (Phase::LongBreak, mins(15)),
        ];
        for (i, (next, break_len)) in breaks.into_iter().enumerate() {
            let block = i as u32 + 1;
            assert_eq!(p.phase(), Phase::Focus, "bloco {block}");
            assert_eq!(p.remaining(t), mins(25), "bloco {block}");
            t += mins(25);
            assert_eq!(
                p.tick(t),
                Some(PomodoroEvent::FocusFinished {
                    completed_focus: block,
                    next
                }),
                "fim do foco {block}"
            );
            assert_eq!(p.phase(), next);
            assert_eq!(p.remaining(t), break_len, "pausa depois do foco {block}");
            t += break_len;
            assert_eq!(
                p.tick(t),
                Some(PomodoroEvent::BreakFinished { next: Phase::Focus }),
                "fim da pausa {block}"
            );
        }
        assert_eq!(p.completed_focus(), 8);
    }

    #[test]
    fn long_break_follows_the_configured_count() {
        let settings = PomodoroSettings::new(secs(10), secs(2), secs(5), 2).unwrap();
        let mut p = Pomodoro::new(settings);
        let mut t = Instant::now();
        p.start(t);
        let mut nexts = Vec::new();
        for _ in 0..4 {
            t += secs(10);
            nexts.push(p.tick(t).unwrap().next());
            t += p.remaining(t);
            p.tick(t).unwrap();
        }
        assert_eq!(
            nexts,
            [
                Phase::ShortBreak,
                Phase::LongBreak,
                Phase::ShortBreak,
                Phase::LongBreak
            ]
        );
    }

    #[test]
    fn pause_freezes_remaining_and_resume_continues_from_there() {
        let mut p = Pomodoro::default();
        let t0 = Instant::now();
        p.start(t0);

        let paused_at = t0 + mins(10);
        assert!(p.pause(paused_at));
        assert!(p.is_paused() && !p.is_running());
        assert!(!p.pause(paused_at), "pausar duas vezes nao faz nada");

        // Duas horas pausado: o que falta nao mexe e a fase nao acaba.
        let back = paused_at + mins(120);
        assert_eq!(p.remaining(paused_at), mins(15));
        assert_eq!(p.remaining(back), mins(15));
        assert_eq!(p.tick(back), None);
        assert_eq!(p.remaining_label(back), "15:00");

        assert!(p.resume(back));
        assert!(p.is_running() && !p.is_paused());
        assert!(!p.resume(back), "retomar a correr nao faz nada");
        assert_eq!(p.remaining(back + mins(5)), mins(10));
        assert_eq!(p.tick(back + mins(15) - NANO), None);
        assert_eq!(
            p.tick(back + mins(15)),
            Some(PomodoroEvent::FocusFinished {
                completed_focus: 1,
                next: Phase::ShortBreak
            })
        );
    }

    #[test]
    fn pause_after_the_phase_ended_is_refused_so_tick_still_reports_it() {
        let mut p = Pomodoro::default();
        let t0 = Instant::now();
        p.start(t0);
        let late = t0 + mins(26);
        assert!(!p.pause(late));
        assert!(p.is_running());
        assert!(matches!(
            p.tick(late),
            Some(PomodoroEvent::FocusFinished { .. })
        ));
    }

    #[test]
    fn skip_ends_the_phase_now_and_counts_toward_the_long_break() {
        let mut p = Pomodoro::default();
        let t0 = Instant::now();
        assert_eq!(p.skip(t0), None, "parado nao ha o que saltar");
        assert_eq!(p.phase(), Phase::Focus);
        assert!(!p.is_running());

        p.start(t0);
        let t1 = t0 + mins(3);
        assert_eq!(
            p.skip(t1),
            Some(PomodoroEvent::FocusFinished {
                completed_focus: 1,
                next: Phase::ShortBreak
            })
        );
        // A pausa conta a partir do salto, inteira.
        assert_eq!(p.remaining(t1), mins(5));
        assert_eq!(p.tick(t1 + mins(5) - NANO), None);

        // Saltar uma pausa pausada arranca o foco a correr.
        let t2 = t1 + mins(1);
        assert!(p.pause(t2));
        assert_eq!(
            p.skip(t2),
            Some(PomodoroEvent::BreakFinished { next: Phase::Focus })
        );
        assert!(p.is_running());
        assert_eq!(p.remaining(t2), mins(25));

        // Mais tres focos saltados: o quarto leva a pausa longa.
        let nexts: Vec<Phase> = (0..3)
            .map(|_| {
                let next = p.skip(t2).unwrap().next();
                p.skip(t2);
                next
            })
            .collect();
        assert_eq!(
            nexts,
            [Phase::ShortBreak, Phase::ShortBreak, Phase::LongBreak]
        );
        assert_eq!(p.completed_focus(), 4);
    }

    #[test]
    fn stop_resets_phase_counter_and_clock() {
        let mut p = Pomodoro::default();
        let mut t = Instant::now();
        p.start(t);
        for _ in 0..2 {
            t += mins(25);
            p.tick(t).unwrap();
            t += mins(5);
            p.tick(t).unwrap();
        }
        t += mins(7);
        assert_eq!(p.completed_focus(), 2);

        p.stop();
        assert_eq!(p.phase(), Phase::Focus);
        assert_eq!(p.completed_focus(), 0);
        assert!(!p.is_running() && !p.is_paused());
        assert_eq!(p.remaining(t + mins(600)), mins(25));
        assert_eq!(p.tick(t + mins(600)), None);

        // Recomeca do zero: o proximo fim de foco e o primeiro.
        assert!(p.start(t));
        assert!(!p.start(t), "arrancar a correr nao reinicia a fase");
        assert_eq!(
            p.tick(t + mins(25)),
            Some(PomodoroEvent::FocusFinished {
                completed_focus: 1,
                next: Phase::ShortBreak
            })
        );
    }

    #[test]
    fn phase_end_is_reported_exactly_once() {
        let settings = PomodoroSettings::new(secs(10), secs(3), secs(7), 4).unwrap();
        let mut p = Pomodoro::new(settings);
        let t0 = Instant::now();
        p.start(t0);

        // A UI tica a cada 250 ms durante 12 s: um foco de 10 s acaba uma vez.
        let events: Vec<(u64, PomodoroEvent)> = (0..=48u64)
            .filter_map(|i| {
                let ms = i * 250;
                p.tick(t0 + Duration::from_millis(ms)).map(|e| (ms, e))
            })
            .collect();
        assert_eq!(
            events,
            [(
                10_000,
                PomodoroEvent::FocusFinished {
                    completed_focus: 1,
                    next: Phase::ShortBreak
                }
            )]
        );
        assert_eq!(p.tick(t0 + secs(10)), None, "o mesmo instante outra vez");
    }

    #[test]
    fn tick_after_a_long_sleep_reports_one_transition_and_restarts_from_the_tick() {
        let mut p = Pomodoro::default();
        let t0 = Instant::now();
        p.start(t0);

        // O PC dormiu 3 h a meio do primeiro foco.
        let wake = t0 + mins(180);
        assert_eq!(
            p.tick(wake),
            Some(PomodoroEvent::FocusFinished {
                completed_focus: 1,
                next: Phase::ShortBreak
            })
        );
        assert_eq!(p.tick(wake), None, "sem rajada de fases perdidas");
        assert_eq!(p.completed_focus(), 1);
        assert_eq!(p.phase(), Phase::ShortBreak);
        // A pausa comeca ao acordar, inteira.
        assert_eq!(p.remaining(wake), mins(5));
        assert_eq!(p.tick(wake + mins(5) - NANO), None);
        assert_eq!(
            p.tick(wake + mins(5)),
            Some(PomodoroEvent::BreakFinished { next: Phase::Focus })
        );
    }

    #[test]
    fn remaining_label_is_mm_ss_rounding_partial_seconds_up() {
        let mut p = Pomodoro::default();
        let t0 = Instant::now();
        assert_eq!(
            p.remaining_label(t0),
            "25:00",
            "parado mostra a fase inteira"
        );

        p.start(t0);
        assert_eq!(p.remaining_label(t0), "25:00");
        assert_eq!(p.remaining_label(t0 + NANO), "25:00");
        assert_eq!(p.remaining_label(t0 + secs(1)), "24:59");
        assert_eq!(
            p.remaining_label(t0 + Duration::from_millis(1_500)),
            "24:59"
        );
        assert_eq!(p.remaining_label(t0 + mins(25) - secs(545)), "09:05");
        assert_eq!(p.remaining_label(t0 + mins(25) - NANO), "00:01");
        assert_eq!(p.remaining_label(t0 + mins(25)), "00:00");

        let long = PomodoroSettings::new(mins(120), mins(5), mins(15), 4).unwrap();
        assert_eq!(Pomodoro::new(long).remaining_label(t0), "120:00");
    }

    #[test]
    fn running_label_never_reads_zero_before_the_phase_ends() {
        let settings = PomodoroSettings::new(secs(10), secs(3), secs(7), 4).unwrap();
        let mut p = Pomodoro::new(settings);
        let t0 = Instant::now();
        p.start(t0);
        for ms in 0..10_000u64 {
            let now = t0 + Duration::from_millis(ms) + Duration::from_micros(500);
            assert_ne!(p.remaining_label(now), "00:00", "a {ms} ms");
        }
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let build = |f, s, l, n| PomodoroSettings::new(f, s, l, n);
        assert_eq!(
            build(Duration::ZERO, mins(5), mins(15), 4),
            Err(PomodoroSettingsError::ZeroDuration(Phase::Focus))
        );
        assert_eq!(
            build(mins(25), Duration::ZERO, mins(15), 4),
            Err(PomodoroSettingsError::ZeroDuration(Phase::ShortBreak))
        );
        assert_eq!(
            build(mins(25), mins(5), Duration::ZERO, 4),
            Err(PomodoroSettingsError::ZeroDuration(Phase::LongBreak))
        );
        assert_eq!(
            build(mins(25), mins(5), mins(15), 0),
            Err(PomodoroSettingsError::ZeroLongBreakEvery)
        );

        let custom = build(mins(50), mins(10), mins(30), 1).unwrap();
        assert_eq!(custom.focus(), mins(50));
        assert_eq!(custom.short_break(), mins(10));
        assert_eq!(custom.long_break(), mins(30));
        assert_eq!(custom.long_break_every(), 1);
        assert_eq!(custom.duration(Phase::ShortBreak), mins(10));
        assert_eq!(
            build(mins(25), mins(5), mins(15), 4),
            Ok(PomodoroSettings::default())
        );
    }
}
