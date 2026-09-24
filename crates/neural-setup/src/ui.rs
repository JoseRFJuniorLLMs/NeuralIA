//! A geometria do instalador.
//!
//! So contas: onde fica cada coisa e que fatia da barra esta acesa. O desenho
//! em si esta no `paint`, que pega nestes retangulos. Separado assim, o que
//! decide posicoes testa-se sem abrir janela nenhuma -- e e onde os erros de
//! layout costumam esconder-se, porque um botao fora do ecra continua a
//! compilar.

/// Um retangulo em pixeis do cliente.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// Um retangulo sem area nao contem nada. Sem esta guarda, um botao que
    /// encolheu ate zero continuava a acertar na sua propria borda esquerda --
    /// um clique invisivel, que e o pior tipo de clique.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.width > 0.0
            && self.height > 0.0
            && x >= self.x
            && x <= self.x + self.width
            && y >= self.y
            && y <= self.y + self.height
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    /// So os testes o usam desde que a zona de silencio saiu do tecido.
    #[cfg(test)]
    pub fn center_x(&self) -> f64 {
        self.x + self.width / 2.0
    }

    pub fn center_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
}

/// Tamanho da janela em unidades logicas, antes do DPI.
pub const WINDOW_W: f64 = 760.0;
pub const WINDOW_H: f64 = 500.0;
/// Proporcao de `assets/neuralia-home.png` (1200x868), a arte da marca. O
/// retangulo dela sai daqui: esticar a marca para caber num quadrado
/// descaracteriza-a.
pub const BRAND_ASPECT: f64 = 1200.0 / 868.0;

/// De quanto em quanto tempo o temporizador acorda a janela, em milissegundos:
/// ~30 quadros por segundo, o mesmo ritmo do tecido na Home da NeuralIA.
pub const FRAME_MS: u32 = 33;
/// O maior passo que o tecido da de uma vez. Uma janela que esteve parada (o
/// disco a meio de uma escrita grande, um depurador) retoma de onde estava em
/// vez de dar um salto que parece um defeito.
pub const MAX_FRAME_STEP: f64 = 0.1;

/// O relogio do tecido de fundo: os neuronios mexem-se com o tempo real, mas
/// so enquanto a janela se ve. Minimizada, o relogio para -- e ao voltar o
/// tecido continua de onde estava, sem saltar o tempo em que ninguem o viu.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TissueClock {
    /// O instante do tecido que se desenha agora, em segundos.
    pub seconds: f64,
    /// O relogio de parede no ultimo passo.
    last: Option<f64>,
}

impl TissueClock {
    /// Um passo do temporizador: `now` e o relogio de parede em segundos.
    pub fn advance(self, now: f64, visible: bool) -> Self {
        let step = self
            .last
            .map_or(0.0, |last| (now - last).clamp(0.0, MAX_FRAME_STEP));
        Self {
            seconds: if visible {
                self.seconds + step
            } else {
                self.seconds
            },
            last: Some(now),
        }
    }
}

/// A tela do tecido para esta janela: a janela inteira, como na Home do
/// navegador. A zona de silencio que aqui havia abria uma elipse escura a
/// volta da marca e do texto -- o dono viu-a como um defeito ("tira esse
/// fundo, deixa os neuronios passarem por tras da logo"). A legibilidade do
/// texto vem agora do contorno de `paint::text_on_tissue`.
pub fn tissue_field(layout: &Layout) -> neural_core::tissue::Field {
    neural_core::tissue::Field::new(layout.client.width, layout.client.height, layout.scale)
}

/// O que esta debaixo do rato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Close,
    Primary,
    Secondary,
    DesktopShortcut,
    /// A faixa de cima, que arrasta a janela.
    Caption,
}

/// Em que ecra estamos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Working,
    Finished,
    Failed,
}

#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub scale: f64,
    pub client: Rect,
    pub caption: Rect,
    pub close: Rect,
    /// A arte da marca, ao centro em cima.
    pub logo: Rect,
    pub title: Rect,
    pub subtitle: Rect,
    /// Caminho de instalacao / mensagem de erro.
    pub note: Rect,
    pub track: Rect,
    pub stage: Rect,
    pub percent: Rect,
    pub checkbox: Rect,
    pub checkbox_label: Rect,
    pub primary: Rect,
    pub secondary: Rect,
}

impl Layout {
    pub fn new(width: f64, height: f64, scale: f64) -> Self {
        let scale = scale.max(1.0);
        let width = width.max(320.0 * scale);
        let height = height.max(240.0 * scale);
        let client = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };

        let caption_h = 38.0 * scale;
        let caption = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: caption_h,
        };
        let close = Rect {
            x: width - 46.0 * scale,
            y: 0.0,
            width: 46.0 * scale,
            height: caption_h,
        };

        // A marca tem de caber: numa janela baixa encolhe em vez de empurrar
        // os botoes para fora do ecra, que e o que acontecia com uma altura
        // fixa. E mantem a proporcao da arte -- esticada, a marca deixa de
        // ser a marca.
        let brand_h = (height * 0.30).clamp(72.0 * scale, 190.0 * scale);
        let brand_w = (brand_h * BRAND_ASPECT).min(width * 0.72);
        let brand_h = brand_w / BRAND_ASPECT;
        let logo = Rect {
            x: (width - brand_w) / 2.0,
            y: caption_h + 14.0 * scale,
            width: brand_w,
            height: brand_h,
        };

        let title = Rect {
            x: 0.0,
            y: logo.bottom() + 10.0 * scale,
            width,
            height: 38.0 * scale,
        };
        let subtitle = Rect {
            x: 0.0,
            y: title.bottom() + 2.0 * scale,
            width,
            height: 20.0 * scale,
        };

        // De baixo para cima: os botoes sao a ancora, porque sao a unica coisa
        // que nunca pode ficar fora do ecra.
        let button_h = 40.0 * scale;
        let margin = 28.0 * scale;
        let gap = 12.0 * scale;
        // Os dois botoes repartem o que ha, em vez de levarem uma largura
        // fixa: numa janela estreita a largura fixa empurrava o segundo para
        // fora do ecra, e um botao fora do ecra nao se carrega.
        let available = (width - margin * 2.0).max(0.0);
        let primary_w = (168.0 * scale).min(((available - gap) * 0.56).max(0.0));
        let secondary_w = (132.0 * scale).min((available - gap - primary_w).max(0.0));
        let primary = Rect {
            x: width - margin - primary_w,
            y: height - margin - button_h,
            width: primary_w,
            height: button_h,
        };
        let secondary = Rect {
            x: primary.x - gap - secondary_w,
            y: primary.y,
            width: secondary_w,
            height: button_h,
        };

        let checkbox_side = 18.0 * scale;
        let checkbox = Rect {
            x: margin,
            y: primary.center_y() - checkbox_side / 2.0,
            width: checkbox_side,
            height: checkbox_side,
        };
        let checkbox_label = Rect {
            x: checkbox.right() + 9.0 * scale,
            y: primary.y,
            // Sem `max(0)` uma janela estreita dava largura negativa, e o
            // `contains` de um retangulo invertido acerta em todo o lado.
            width: (secondary.x - checkbox.right() - 18.0 * scale).max(0.0),
            height: button_h,
        };

        let track = Rect {
            x: margin,
            y: primary.y - 46.0 * scale,
            width: width - margin * 2.0,
            height: 10.0 * scale,
        };
        let stage = Rect {
            x: margin,
            y: track.y - 26.0 * scale,
            width: width - margin * 2.0 - 70.0 * scale,
            height: 22.0 * scale,
        };
        let percent = Rect {
            x: width - margin - 70.0 * scale,
            y: stage.y,
            width: 70.0 * scale,
            height: 22.0 * scale,
        };
        let note = Rect {
            x: margin,
            y: subtitle.bottom() + 8.0 * scale,
            width: width - margin * 2.0,
            height: (stage.y - subtitle.bottom() - 16.0 * scale).max(18.0 * scale),
        };

        Self {
            scale,
            client,
            caption,
            close,
            logo,
            title,
            subtitle,
            note,
            track,
            stage,
            percent,
            checkbox,
            checkbox_label,
            primary,
            secondary,
        }
    }

    /// O que esta debaixo do rato, sabendo em que ecra estamos: a caixa do
    /// atalho so existe no ecra inicial, e clicar onde ela estaria durante a
    /// instalacao nao pode mudar nada.
    pub fn hit(&self, screen: Screen, x: f64, y: f64) -> Option<Hit> {
        if self.close.contains(x, y) {
            return Some(Hit::Close);
        }
        if screen != Screen::Working {
            if self.primary.contains(x, y) {
                return Some(Hit::Primary);
            }
            if self.secondary.contains(x, y) {
                return Some(Hit::Secondary);
            }
            // A caixa do atalho so faz sentido antes de instalar.
            if screen == Screen::Welcome
                && (self.checkbox.contains(x, y) || self.checkbox_label.contains(x, y))
            {
                return Some(Hit::DesktopShortcut);
            }
        }
        if self.caption.contains(x, y) {
            return Some(Hit::Caption);
        }
        None
    }

    /// A parte acesa da barra.
    pub fn filled(&self, progress: f64) -> Rect {
        Rect {
            width: self.track.width * progress.clamp(0.0, 1.0),
            ..self.track
        }
    }

    /// O brilho que corre por cima da parte acesa. Devolve `None` quando ainda
    /// nao ha barra suficiente para ele caber -- senao aparecia um risco solto
    /// a esquerda antes de a instalacao comecar.
    pub fn shimmer(&self, progress: f64, seconds: f64) -> Option<Rect> {
        let filled = self.filled(progress);
        let width = 64.0 * self.scale;
        if filled.width < width * 0.75 {
            return None;
        }
        let travel = (seconds * 0.55).fract();
        let x = filled.x + travel * (filled.width + width) - width;
        let left = x.max(filled.x);
        let right = (x + width).min(filled.right());
        if right <= left {
            return None;
        }
        Some(Rect {
            x: left,
            y: filled.y,
            width: right - left,
            height: filled.height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout::new(WINDOW_W, WINDOW_H, 1.0)
    }

    #[test]
    fn everything_stays_inside_the_window_at_every_scale_and_size() {
        // O caso que estraga tudo: 150% de DPI numa janela pequena empurra os
        // botoes para fora e o instalador fica sem forma de avancar.
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for (w, h) in [
                (WINDOW_W, WINDOW_H),
                (WINDOW_W * scale, WINDOW_H * scale),
                (420.0, 300.0),
                (1280.0, 820.0),
            ] {
                let layout = Layout::new(w, h, scale);
                let client = layout.client;
                for (name, rect) in [
                    ("close", layout.close),
                    ("logo", layout.logo),
                    ("track", layout.track),
                    ("primary", layout.primary),
                    ("secondary", layout.secondary),
                    ("checkbox", layout.checkbox),
                ] {
                    assert!(
                        rect.x >= -0.5
                            && rect.y >= -0.5
                            && rect.right() <= client.width + 0.5
                            && rect.bottom() <= client.height + 0.5,
                        "{name} sai do ecra a {scale}x em {w}x{h}: {rect:?}"
                    );
                    assert!(rect.width > 0.0 && rect.height > 0.0, "{name} vazio");
                }
            }
        }
    }

    #[test]
    fn nothing_overlaps_the_buttons() {
        let layout = layout();
        assert!(
            layout.track.bottom() < layout.primary.y,
            "a barra passava por cima dos botoes"
        );
        assert!(
            layout.checkbox_label.right() <= layout.secondary.x,
            "o texto do atalho entrava no botao"
        );
        assert!(layout.note.bottom() <= layout.stage.y);
    }

    #[test]
    fn during_the_install_the_only_thing_that_still_answers_is_the_close_button() {
        // Clicar em "Instalar" outra vez a meio da instalacao lancava um
        // segundo trabalho por cima do primeiro.
        let layout = layout();
        let p = layout.primary;
        assert_eq!(
            layout.hit(Screen::Welcome, p.center_x(), p.center_y()),
            Some(Hit::Primary)
        );
        assert_eq!(
            layout.hit(Screen::Working, p.center_x(), p.center_y()),
            None
        );
        let c = layout.close;
        assert_eq!(
            layout.hit(Screen::Working, c.center_x(), c.center_y()),
            Some(Hit::Close)
        );
    }

    #[test]
    fn the_desktop_checkbox_only_exists_on_the_first_screen() {
        let layout = layout();
        let b = layout.checkbox;
        assert_eq!(
            layout.hit(Screen::Welcome, b.center_x(), b.center_y()),
            Some(Hit::DesktopShortcut)
        );
        for screen in [Screen::Working, Screen::Finished, Screen::Failed] {
            assert_ne!(
                layout.hit(screen, b.center_x(), b.center_y()),
                Some(Hit::DesktopShortcut),
                "{screen:?} ainda responde a caixa do atalho"
            );
        }
    }

    #[test]
    fn the_caption_drags_the_window_but_the_close_button_does_not() {
        let layout = layout();
        assert_eq!(layout.hit(Screen::Welcome, 300.0, 12.0), Some(Hit::Caption));
        let c = layout.close;
        assert_eq!(
            layout.hit(Screen::Welcome, c.center_x(), c.center_y()),
            Some(Hit::Close)
        );
    }

    #[test]
    fn a_button_squeezed_to_nothing_stops_answering_clicks() {
        // Numa janela apertada o botao secundario encolhe ate desaparecer. Se
        // continuasse a responder, ficava um clique invisivel na borda.
        let empty = Rect {
            x: 100.0,
            y: 100.0,
            width: 0.0,
            height: 40.0,
        };
        assert!(!empty.contains(100.0, 120.0));
        assert!(!empty.contains(100.0, 100.0));
    }

    #[test]
    fn the_lit_part_of_the_bar_tracks_the_progress() {
        let layout = layout();
        assert_eq!(layout.filled(0.0).width, 0.0);
        assert_eq!(layout.filled(1.0).width, layout.track.width);
        assert!((layout.filled(0.5).width - layout.track.width / 2.0).abs() < 1e-9);
        // Fora da escala nao transborda da calha.
        assert_eq!(layout.filled(4.0).width, layout.track.width);
        assert_eq!(layout.filled(-1.0).width, 0.0);
    }

    /// O tecido tal como o instalador o desenha, no instante do relogio.
    fn frame(clock: TissueClock) -> neural_core::tissue::Tissue {
        neural_core::tissue::tissue_at(&tissue_field(&layout()), clock.seconds)
    }

    #[test]
    fn the_neurons_pass_behind_the_brand_and_the_text() {
        // O dono, com o print da 2.1.6: "tira esse fundo, deixa os neuronios
        // passarem por tras da logo". Nao ha zona de silencio, e ao longo de
        // alguns segundos ha neuronios por tras da marca e das linhas de texto.
        let layout = layout();
        let field = tissue_field(&layout);
        assert!(
            field.quiet.is_none(),
            "voltou a zona de silencio: {:?}",
            field.quiet
        );
        let (mut behind_logo, mut behind_text) = (false, false);
        for step in 0..60 {
            let tissue = neural_core::tissue::tissue_at(&field, f64::from(step) * 0.25);
            behind_logo |= tissue.nodes.iter().any(|n| layout.logo.contains(n.x, n.y));
            behind_text |= tissue
                .nodes
                .iter()
                .any(|n| layout.title.contains(n.x, n.y) || layout.note.contains(n.x, n.y));
        }
        assert!(behind_logo, "nenhum neuronio passa por tras da logo");
        assert!(behind_text, "nenhum neuronio passa por tras do texto");
    }

    #[test]
    fn the_neurons_move_from_one_timer_tick_to_the_next() {
        // O dono quer o instalador "com os neuronios se movimentando". Cada
        // passo do temporizador tem de dar um tecido diferente do anterior --
        // um relogio parado desenha sempre o mesmo quadro, e o fundo fica uma
        // fotografia.
        let tick = f64::from(FRAME_MS) / 1000.0;
        let mut clock = TissueClock::default().advance(0.0, true);
        let mut previous = frame(clock);
        for step in 1..=30 {
            clock = clock.advance(step as f64 * tick, true);
            let current = frame(clock);
            assert_ne!(
                current.nodes, previous.nodes,
                "o tecido ficou parado no passo {step} (t={:.3}s)",
                clock.seconds
            );
            previous = current;
        }
        assert!(
            (clock.seconds - 30.0 * tick).abs() < 1e-9,
            "o relogio do tecido devia andar com o tempo: {}",
            clock.seconds
        );
    }

    #[test]
    fn a_minimised_installer_stops_animating_and_resumes_where_it_was() {
        let tick = f64::from(FRAME_MS) / 1000.0;
        let shown = TissueClock::default()
            .advance(0.0, true)
            .advance(tick, true);
        let mut hidden = shown;
        for step in 2..200 {
            hidden = hidden.advance(step as f64 * tick, false);
        }
        assert_eq!(
            hidden.seconds, shown.seconds,
            "minimizado, o tecido continuou a andar"
        );
        assert_eq!(frame(hidden).nodes, frame(shown).nodes);
        // Ao voltar, um passo normal -- nao os seis segundos em que esteve
        // escondido de uma vez.
        let back = hidden.advance(200.0 * tick, true);
        assert!(
            back.seconds - hidden.seconds <= MAX_FRAME_STEP + 1e-12,
            "voltou com um salto de {}s",
            back.seconds - hidden.seconds
        );
        assert!(back.seconds > hidden.seconds);
    }

    #[test]
    fn a_clock_that_goes_backwards_or_stalls_does_not_jerk_the_tissue() {
        let clock = TissueClock::default().advance(10.0, true);
        assert_eq!(clock.advance(9.0, true).seconds, clock.seconds);
        let stalled = clock.advance(40.0, true);
        assert!((stalled.seconds - clock.seconds - MAX_FRAME_STEP).abs() < 1e-12);
    }

    #[test]
    fn the_shimmer_never_leaves_the_lit_part() {
        // Um brilho a correr por fora da barra le-se como um defeito de
        // desenho, e foi o primeiro que apareceu.
        let layout = layout();
        for step in 0..200 {
            let seconds = step as f64 * 0.11;
            for progress in [0.0, 0.05, 0.2, 0.5, 0.9, 1.0] {
                let Some(shimmer) = layout.shimmer(progress, seconds) else {
                    continue;
                };
                let filled = layout.filled(progress);
                assert!(
                    shimmer.x >= filled.x - 1e-9 && shimmer.right() <= filled.right() + 1e-9,
                    "brilho fora da barra em t={seconds} p={progress}: {shimmer:?} vs {filled:?}"
                );
                assert!(shimmer.width > 0.0);
            }
        }
        assert!(
            layout.shimmer(0.0, 1.0).is_none(),
            "sem barra acesa nao ha brilho nenhum a mostrar"
        );
    }
}
