//! Decisoes puras do chrome nativo a volta da Home e dos paineis da direita.
//!
//! Tudo aqui e aritmetica e maquinas de estado sem uma unica chamada ao
//! Windows: os botoes da janela que so aparecem com o rato por perto, a
//! largura do painel lateral arrastada e gravada, o destino da roda do rato
//! sobre o painel e os modos do painel de servicos (minimizado, tela cheia).
//! `windows_app.rs` so copia estas decisoes para o Win32/WebView2; os gates
//! vivem aqui e correm tambem no runner Linux do CI.

use std::path::Path;

/// Um retangulo meio-aberto: a borda direita e a de baixo ja sao do vizinho.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Area {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.width > 0.0
            && self.height > 0.0
            && x >= self.x
            && x < self.x + self.width
            && y >= self.y
            && y < self.y + self.height
    }

    /// O mesmo retangulo com `margin` a mais para cada lado.
    pub fn grow(&self, margin: f64) -> Area {
        let margin = margin.max(0.0);
        Area {
            x: self.x - margin,
            y: self.y - margin,
            width: self.width + 2.0 * margin,
            height: self.height + 2.0 * margin,
        }
    }
}

// ---------------------------------------------------------------------------
// Home: minimizar / maximizar / fechar so a vista com o rato por perto.
// ---------------------------------------------------------------------------

/// Quanto tempo os botoes da janela ficam a vista depois de o rato sair da
/// zona deles. Curto o bastante para a Home voltar a ficar limpa, longo o
/// bastante para um rato que passa de raspao nao os fazer piscar.
pub const CAPTION_HIDE_DELAY_MS: u64 = 300;
/// Folga, em pixels logicos, a volta dos tres botoes: o rato que vai na
/// direcao deles ja os ve antes de lhes tocar.
pub const CAPTION_HOT_MARGIN: f64 = 24.0;

/// A zona que acorda os botoes: os proprios botoes mais a folga.
pub fn caption_hot_zone(buttons: Area, margin: f64) -> Area {
    buttons.grow(margin)
}

/// O que a janela nativa dos botoes tem de fazer depois de uma observacao.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevealStep {
    /// Mostrar ja (sem ativar).
    Show,
    /// Esconder ja: deixa de se pintar e de aceitar o clique.
    Hide,
    /// O rato saiu: voltar a olhar daqui a tantos milissegundos.
    ScheduleHide(u64),
    Nothing,
}

/// Os botoes da janela na Home: escondidos ate o rato entrar na zona deles,
/// e escondidos outra vez `CAPTION_HIDE_DELAY_MS` depois de ele sair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptionReveal {
    shown: bool,
    hide_at: Option<u64>,
}

impl CaptionReveal {
    pub fn shown(&self) -> bool {
        self.shown
    }

    /// Esconde sem esperar (entrar na Home, sair dela).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// O rato esta (ou nao) na zona dos botoes no instante `now_ms`.
    pub fn observe(&mut self, inside: bool, now_ms: u64) -> RevealStep {
        if inside {
            self.hide_at = None;
            if self.shown {
                return RevealStep::Nothing;
            }
            self.shown = true;
            return RevealStep::Show;
        }
        if !self.shown {
            return RevealStep::Nothing;
        }
        match self.hide_at {
            None => {
                self.hide_at = Some(now_ms.saturating_add(CAPTION_HIDE_DELAY_MS));
                RevealStep::ScheduleHide(CAPTION_HIDE_DELAY_MS)
            }
            Some(at) if now_ms >= at => {
                self.shown = false;
                self.hide_at = None;
                RevealStep::Hide
            }
            Some(_) => RevealStep::Nothing,
        }
    }
}

// ---------------------------------------------------------------------------
// Largura do painel da direita: arrastada pela borda esquerda e gravada.
// ---------------------------------------------------------------------------

/// O painel nunca fica mais estreito do que isto (pixels logicos)...
pub const PANEL_MIN_WIDTH: f64 = 300.0;
/// ...nem mais largo do que esta fracao da janela: as colunas das IAs
/// continuam a ver-se ao lado dele.
pub const PANEL_MAX_FRACTION: f64 = 0.60;
/// Largura, em pixels logicos, da pega que arrasta a borda do painel.
pub const PANEL_HANDLE_WIDTH: f64 = 7.0;
/// Ficheiro, dentro do `data_dir`, com as larguras escolhidas.
pub const PANEL_WIDTHS_FILE: &str = "panel-width.json";

/// Os paineis da direita tem larguras proprias: o historico e uma lista, o
/// WhatsApp e o Meet precisam de espaco.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    /// Historico inteligente (Ctrl+H).
    History,
    /// Servicos (Meet, WhatsApp, YouTube, Gmail) e o Gemini Live.
    Service,
}

/// A largura de sempre, antes de a pessoa arrastar a borda.
pub fn default_panel_width(kind: PanelKind, logical_w: f64) -> f64 {
    match kind {
        PanelKind::History => (logical_w * 0.34).clamp(320.0, 440.0),
        PanelKind::Service => (logical_w * 0.42).clamp(400.0, 640.0),
    }
}

/// Entre `PANEL_MIN_WIDTH` e `PANEL_MAX_FRACTION` da janela, e nunca mais
/// larga do que a propria janela. Numa janela tao estreita que 60% nao chega
/// ao minimo, fica o minimo (ou a janela inteira, se nem isso couber).
pub fn clamp_panel_width(width: f64, logical_w: f64) -> f64 {
    let window = logical_w.max(0.0);
    let max = (window * PANEL_MAX_FRACTION).max(PANEL_MIN_WIDTH);
    let width = if width.is_finite() {
        width
    } else {
        PANEL_MIN_WIDTH
    };
    width.clamp(PANEL_MIN_WIDTH, max).min(window)
}

/// A largura em vigor: a escolhida (se houver) ou a de sempre, presa aos
/// limites da janela de agora.
pub fn panel_width(kind: PanelKind, chosen: Option<f64>, logical_w: f64) -> f64 {
    clamp_panel_width(
        chosen.unwrap_or_else(|| default_panel_width(kind, logical_w)),
        logical_w,
    )
}

/// A borda esquerda do painel seguiu o rato ate `cursor_x` (pixels logicos
/// do cliente): a largura nova, ja presa aos limites.
pub fn panel_width_from_drag(cursor_x: f64, logical_w: f64) -> f64 {
    clamp_panel_width(logical_w - cursor_x, logical_w)
}

/// Onde fica o painel encostado a direita, abaixo de `top`.
pub fn panel_area(width: f64, logical_w: f64, logical_h: f64, top: f64) -> Area {
    let top = top.clamp(0.0, logical_h.max(0.0));
    let width = width.min(logical_w.max(0.0)).max(0.0);
    Area {
        x: logical_w - width,
        y: top,
        width,
        height: (logical_h - top).max(0.0),
    }
}

/// A pega de arrastar: uma faixa fina centrada na borda esquerda do painel.
/// Fica do lado ESQUERDO do painel, longe da barra de rolagem dele, que e a
/// da direita.
pub fn panel_handle_area(panel: Area, handle_width: f64) -> Area {
    let width = handle_width.max(1.0);
    Area {
        x: panel.x - width / 2.0,
        y: panel.y,
        width,
        height: panel.height,
    }
}

/// As larguras escolhidas pela pessoa, uma por tipo de painel.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PanelWidths {
    pub history: Option<f64>,
    pub service: Option<f64>,
}

impl PanelWidths {
    pub fn get(&self, kind: PanelKind) -> Option<f64> {
        match kind {
            PanelKind::History => self.history,
            PanelKind::Service => self.service,
        }
    }

    pub fn set(&mut self, kind: PanelKind, width: f64) {
        let slot = match kind {
            PanelKind::History => &mut self.history,
            PanelKind::Service => &mut self.service,
        };
        *slot = width.is_finite().then_some(width.round());
    }

    /// `{"historico":380,"servicos":560}`; o que nao for um numero plausivel
    /// fica de fora e vale a largura de sempre.
    pub fn to_json(&self) -> String {
        let mut map = serde_json::Map::new();
        if let Some(width) = self.history {
            map.insert("historico".into(), serde_json::json!(width));
        }
        if let Some(width) = self.service {
            map.insert("servicos".into(), serde_json::json!(width));
        }
        serde_json::Value::Object(map).to_string()
    }

    pub fn from_json(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return Self::default();
        };
        let read = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .filter(|width| width.is_finite() && *width >= 1.0 && *width <= 100_000.0)
        };
        Self {
            history: read("historico"),
            service: read("servicos"),
        }
    }

    /// Ficheiro ausente ou estragado: as larguras de sempre.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::from_json(&text))
            .unwrap_or_default()
    }

    /// Escrita atomica: um ficheiro temporario ao lado e um `rename` por
    /// cima. Um arranque a meio de uma escrita le a largura antiga, nunca
    /// meio ficheiro.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, self.to_json())?;
        std::fs::rename(&temp, path)
    }
}

// ---------------------------------------------------------------------------
// Roda do rato por cima do painel da direita.
// ---------------------------------------------------------------------------

/// Retangulo em pixels de ecra (fisicos), `right`/`bottom` exclusivos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScreenRect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Para onde vai um giro da roda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelRoute {
    /// Nao e connosco: o Windows entrega-o como sempre.
    PassThrough,
    /// Esta por cima do painel aberto: vai para a janela do painel debaixo do
    /// cursor, seja qual for a que tem o foco do teclado.
    Panel,
}

/// A decisao inteira: so com o NeuralIA em primeiro plano, so com um painel a
/// vista, e so com o cursor dentro dele. Tudo o resto passa intocado.
pub fn wheel_route(
    point: (i32, i32),
    panel: Option<ScreenRect>,
    app_in_foreground: bool,
) -> WheelRoute {
    match panel {
        Some(rect) if app_in_foreground && rect.contains(point.0, point.1) => WheelRoute::Panel,
        _ => WheelRoute::PassThrough,
    }
}

/// `WM_MOUSEWHEEL`: a palavra alta do `wParam` e o giro, a baixa as teclas e
/// botoes em baixo (MK_*); o `lParam` e o ponto em coordenadas de ecra.
pub fn wheel_message_params(delta: i16, keys: u16, x: i32, y: i32) -> (usize, isize) {
    let wparam = ((delta as u16 as u32) << 16) | keys as u32;
    let lparam = ((y as i16 as u16 as u32) << 16) | (x as i16 as u16 as u32);
    (wparam as usize, lparam as i32 as isize)
}

// ---------------------------------------------------------------------------
// Painel de servicos (YouTube e companhia): minimizado, tela cheia, fechado.
// ---------------------------------------------------------------------------

/// Altura, em pixels logicos, da faixa nativa de controlos por cima do painel
/// de servicos (so no comparador, onde ha barra).
pub const SERVICE_STRIP_HEIGHT: f64 = 34.0;

/// O que pode acontecer a um painel de servicos aberto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceInput {
    /// "— Minimizar" da faixa: sai da frente, a pagina continua viva.
    Minimize,
    /// "⛶ Tela cheia" da faixa, ou o "Sair da tela cheia" nativo.
    ToggleFullscreen,
    /// O icone do servico na barra.
    IconClick,
    /// A propria pagina entrou (true) ou saiu (false) de tela cheia -- o
    /// botao de tela cheia do YouTube, por exemplo.
    PageFullscreen(bool),
    /// Esc com o painel em tela cheia.
    Escape,
    /// "× Fechar" da faixa.
    Close,
}

/// O que o nativo tem de fazer depois de um passo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceEffect {
    /// Mudou o modo: recolocar o painel, a janela e as colunas.
    Relayout,
    /// Pedir a pagina que saia da tela cheia dela (`document.exitFullscreen`);
    /// o fim chega depois como `PageFullscreen(false)`.
    ExitPageFullscreen,
    /// Destruir o painel.
    Close,
    Nothing,
}

/// O modo do painel de servicos aberto.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServicePanelState {
    minimized: bool,
    /// A pagina pediu tela cheia (API Fullscreen do DOM).
    page_fullscreen: bool,
    /// A pessoa pediu tela cheia pela faixa nativa.
    app_fullscreen: bool,
}

/// O ponto no icone do servico enquanto o painel esta minimizado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceBadge {
    /// Minimizado e a tocar som.
    Playing,
    /// Minimizado, em silencio (pausado ou sem som).
    Minimized,
}

impl ServicePanelState {
    pub fn minimized(&self) -> bool {
        self.minimized
    }

    pub fn fullscreen(&self) -> bool {
        !self.minimized && (self.page_fullscreen || self.app_fullscreen)
    }

    /// So a tela cheia pedida pela faixa tem um "Sair" nativo: a da pagina
    /// tem o botao dela e o Esc.
    pub fn app_fullscreen(&self) -> bool {
        !self.minimized && self.app_fullscreen
    }

    pub fn docked(&self) -> bool {
        !self.minimized && !self.fullscreen()
    }

    pub fn step(&mut self, input: ServiceInput) -> ServiceEffect {
        match input {
            ServiceInput::Close => ServiceEffect::Close,
            ServiceInput::Minimize => {
                if !self.docked() {
                    return ServiceEffect::Nothing;
                }
                self.minimized = true;
                ServiceEffect::Relayout
            }
            ServiceInput::IconClick => {
                if self.minimized {
                    self.minimized = false;
                    ServiceEffect::Relayout
                } else {
                    // Aberto: o icone fecha, como sempre fez.
                    ServiceEffect::Close
                }
            }
            ServiceInput::ToggleFullscreen => {
                if self.minimized {
                    return ServiceEffect::Nothing;
                }
                if self.fullscreen() {
                    self.app_fullscreen = false;
                    if self.page_fullscreen {
                        return ServiceEffect::ExitPageFullscreen;
                    }
                } else {
                    self.app_fullscreen = true;
                }
                ServiceEffect::Relayout
            }
            ServiceInput::PageFullscreen(on) => {
                if self.minimized || self.page_fullscreen == on {
                    // Uma pagina escondida nao toma o ecra; e um aviso repetido
                    // nao muda nada.
                    self.page_fullscreen = on && !self.minimized;
                    return ServiceEffect::Nothing;
                }
                let was = self.fullscreen();
                self.page_fullscreen = on;
                if was == self.fullscreen() {
                    ServiceEffect::Nothing
                } else {
                    ServiceEffect::Relayout
                }
            }
            ServiceInput::Escape => {
                if self.page_fullscreen && !self.minimized {
                    ServiceEffect::ExitPageFullscreen
                } else if self.app_fullscreen() {
                    self.app_fullscreen = false;
                    ServiceEffect::Relayout
                } else {
                    ServiceEffect::Nothing
                }
            }
        }
    }

    /// A largura que o comparador cede ao painel: nada com ele minimizado
    /// (as colunas voltam a ocupar a janela toda).
    pub fn reserved_width(&self, width: f64) -> f64 {
        if self.minimized { 0.0 } else { width }
    }

    /// Onde fica a WebView do painel; `None` = escondida (minimizado).
    pub fn panel_area(
        &self,
        width: f64,
        logical_w: f64,
        logical_h: f64,
        top: f64,
        strip: f64,
    ) -> Option<Area> {
        if self.minimized {
            return None;
        }
        if self.fullscreen() {
            return Some(Area {
                x: 0.0,
                y: 0.0,
                width: logical_w.max(0.0),
                height: logical_h.max(0.0),
            });
        }
        Some(panel_area(
            width,
            logical_w,
            logical_h,
            top + strip.max(0.0),
        ))
    }

    /// A faixa nativa de controlos, por cima do painel encostado.
    pub fn strip_area(&self, width: f64, logical_w: f64, top: f64, strip: f64) -> Option<Area> {
        if !self.docked() || strip <= 0.0 {
            return None;
        }
        let panel = panel_area(width, logical_w, top + strip, top);
        Some(Area {
            height: strip,
            ..panel
        })
    }

    /// O ponto no icone da barra: so enquanto minimizado.
    pub fn badge(&self, playing_audio: bool) -> Option<ServiceBadge> {
        self.minimized.then_some(if playing_audio {
            ServiceBadge::Playing
        } else {
            ServiceBadge::Minimized
        })
    }
}

/// Esc (tecla em baixo) e connosco so com o painel em tela cheia; fora disso
/// pertence a pagina.
pub fn escape_is_ours(virtual_key: u32, key_down: bool, state: ServicePanelState) -> bool {
    const VK_ESCAPE: u32 = 0x1B;
    virtual_key == VK_ESCAPE && key_down && state.fullscreen()
}

/// O script (nosso, constante, sem dados da pagina) que pede a pagina para
/// sair da tela cheia do DOM.
pub const EXIT_PAGE_FULLSCREEN_SCRIPT: &str = "(function(){try{if(document.fullscreenElement&&document.exitFullscreen){document.exitFullscreen().catch(function(){});}}catch(e){}})();";

#[cfg(test)]
mod tests {
    use super::*;

    const BUTTONS: Area = Area {
        x: 1000.0,
        y: 0.0,
        width: 138.0,
        height: 34.0,
    };

    #[test]
    fn caption_buttons_appear_near_the_cursor_and_hide_300_ms_after_it_leaves() {
        let zone = caption_hot_zone(BUTTONS, CAPTION_HOT_MARGIN);
        let mut reveal = CaptionReveal::default();
        let at = |x: f64, y: f64| zone.contains(x, y);

        // A Home abre limpa: longe dos botoes, nada.
        assert!(!reveal.shown());
        assert_eq!(reveal.observe(at(400.0, 300.0), 0), RevealStep::Nothing);
        assert!(!reveal.shown());

        // A caminho deles: ainda fora dos botoes, ja dentro da folga.
        assert!(!BUTTONS.contains(980.0, 40.0));
        assert_eq!(reveal.observe(at(980.0, 40.0), 100), RevealStep::Show);
        assert!(reveal.shown());
        // Mexer la dentro nao repete o Show.
        assert_eq!(reveal.observe(at(1050.0, 10.0), 150), RevealStep::Nothing);

        // Sai: continuam a vista 300 ms.
        assert_eq!(
            reveal.observe(at(600.0, 400.0), 1_000),
            RevealStep::ScheduleHide(CAPTION_HIDE_DELAY_MS)
        );
        assert!(reveal.shown());
        assert_eq!(reveal.observe(at(600.0, 420.0), 1_150), RevealStep::Nothing);
        assert!(reveal.shown(), "escondeu antes dos 300 ms");
        assert_eq!(reveal.observe(at(600.0, 420.0), 1_299), RevealStep::Nothing);
        assert!(reveal.shown());
        assert_eq!(reveal.observe(at(600.0, 420.0), 1_300), RevealStep::Hide);
        assert!(!reveal.shown());
        // Escondidos, o temporizador atrasado nao faz mais nada.
        assert_eq!(reveal.observe(false, 1_400), RevealStep::Nothing);

        // Voltar a zona antes do prazo cancela o esconder.
        assert_eq!(reveal.observe(true, 2_000), RevealStep::Show);
        assert_eq!(
            reveal.observe(false, 2_100),
            RevealStep::ScheduleHide(CAPTION_HIDE_DELAY_MS)
        );
        assert_eq!(reveal.observe(true, 2_200), RevealStep::Nothing);
        assert_eq!(reveal.observe(true, 2_450), RevealStep::Nothing);
        assert!(
            reveal.shown(),
            "o prazo antigo nao pode esconder um rato que voltou"
        );
        // E o prazo conta de novo a partir da saida seguinte.
        assert_eq!(
            reveal.observe(false, 3_000),
            RevealStep::ScheduleHide(CAPTION_HIDE_DELAY_MS)
        );
        assert_eq!(reveal.observe(false, 3_299), RevealStep::Nothing);
        assert_eq!(reveal.observe(false, 3_300), RevealStep::Hide);

        reveal.observe(true, 4_000);
        reveal.reset();
        assert!(!reveal.shown());
    }

    #[test]
    fn caption_hot_zone_is_the_buttons_plus_the_margin_and_nothing_else() {
        let zone = caption_hot_zone(BUTTONS, CAPTION_HOT_MARGIN);
        // Os botoes e a folga a volta.
        assert!(zone.contains(BUTTONS.x, 0.0));
        assert!(zone.contains(BUTTONS.x - CAPTION_HOT_MARGIN, 10.0));
        assert!(zone.contains(1100.0, BUTTONS.height + CAPTION_HOT_MARGIN - 0.5));
        // Um pixel alem da folga ja nao acorda nada.
        assert!(!zone.contains(BUTTONS.x - CAPTION_HOT_MARGIN - 1.0, 10.0));
        assert!(!zone.contains(1100.0, BUTTONS.height + CAPTION_HOT_MARGIN));
        assert!(!zone.contains(500.0, 10.0));
    }

    #[test]
    fn panel_width_is_clamped_to_300_and_60_percent_of_the_window() {
        assert_eq!(clamp_panel_width(100.0, 1600.0), PANEL_MIN_WIDTH);
        assert_eq!(clamp_panel_width(500.0, 1600.0), 500.0);
        assert_eq!(clamp_panel_width(1200.0, 1600.0), 960.0);
        // Janela estreita: 60% nao chega aos 300, fica o minimo.
        assert_eq!(clamp_panel_width(900.0, 400.0), PANEL_MIN_WIDTH);
        // Nunca mais larga que a janela.
        assert_eq!(clamp_panel_width(900.0, 250.0), 250.0);
        assert_eq!(clamp_panel_width(f64::NAN, 1600.0), PANEL_MIN_WIDTH);

        // Sem escolha, a largura de sempre (dentro dos limites novos).
        assert_eq!(panel_width(PanelKind::History, None, 1600.0), 440.0);
        assert_eq!(panel_width(PanelKind::Service, None, 1600.0), 640.0);
        assert_eq!(panel_width(PanelKind::Service, Some(820.0), 1600.0), 820.0);
        // Uma escolha feita numa janela maior e presa a janela de agora.
        assert_eq!(panel_width(PanelKind::Service, Some(820.0), 1000.0), 600.0);

        // Arrastar a borda: o painel fica com o que sobra a direita do rato.
        assert_eq!(panel_width_from_drag(1100.0, 1600.0), 500.0);
        assert_eq!(panel_width_from_drag(1500.0, 1600.0), PANEL_MIN_WIDTH);
        assert_eq!(panel_width_from_drag(100.0, 1600.0), 960.0);
    }

    #[test]
    fn panel_widths_survive_a_restart_and_a_broken_file_falls_back() {
        let dir = std::env::temp_dir().join(format!(
            "neuralia-panel-width-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(PANEL_WIDTHS_FILE);

        // Sem ficheiro: as larguras de sempre.
        assert_eq!(PanelWidths::load(&path), PanelWidths::default());

        let mut widths = PanelWidths::default();
        widths.set(PanelKind::History, 377.4);
        widths.set(PanelKind::Service, 612.0);
        widths.save(&path).expect("gravar");
        assert!(!path.with_extension("tmp").exists(), "o temporario ficou");
        let loaded = PanelWidths::load(&path);
        assert_eq!(loaded.get(PanelKind::History), Some(377.0));
        assert_eq!(loaded.get(PanelKind::Service), Some(612.0));

        // Gravar outra vez por cima: ganha a ultima.
        widths.set(PanelKind::Service, 700.0);
        widths.save(&path).expect("gravar de novo");
        assert_eq!(
            PanelWidths::load(&path).get(PanelKind::Service),
            Some(700.0)
        );

        // Ficheiro estragado ou com lixo: nada escolhido, nada rebenta.
        std::fs::write(&path, "{\"historico\":").expect("escrever lixo");
        assert_eq!(PanelWidths::load(&path), PanelWidths::default());
        std::fs::write(&path, "{\"historico\":\"largo\",\"servicos\":-5}").expect("lixo");
        assert_eq!(PanelWidths::load(&path), PanelWidths::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_resize_handle_sits_on_the_left_edge_away_from_the_scrollbar() {
        let panel = panel_area(500.0, 1600.0, 900.0, 88.0);
        assert_eq!(panel.x, 1100.0);
        assert_eq!(panel.y, 88.0);
        assert_eq!(panel.height, 812.0);
        let handle = panel_handle_area(panel, PANEL_HANDLE_WIDTH);
        // A borda esquerda do painel fica debaixo da pega...
        assert!(handle.contains(1100.0, 400.0));
        assert!(handle.contains(1100.0 - PANEL_HANDLE_WIDTH / 2.0, 400.0));
        assert!(!handle.contains(1100.0 - PANEL_HANDLE_WIDTH / 2.0 - 1.0, 400.0));
        // ...e a barra de rolagem (os 16 px da direita) nunca.
        for x in [1584.0, 1590.0, 1599.0] {
            assert!(!handle.contains(x, 400.0), "a pega cobre a barra em {x}");
        }
        // Nada acima do painel (a barra do comparador) nem abaixo.
        assert!(!handle.contains(1100.0, 87.0));
        assert!(!handle.contains(1100.0, 900.0));
    }

    #[test]
    fn the_wheel_over_the_open_panel_goes_to_the_panel_and_nothing_else_is_touched() {
        let panel = ScreenRect {
            left: 1100,
            top: 88,
            right: 1600,
            bottom: 900,
        };
        assert_eq!(
            wheel_route((1300, 400), Some(panel), true),
            WheelRoute::Panel
        );
        assert_eq!(
            wheel_route((1100, 88), Some(panel), true),
            WheelRoute::Panel
        );
        // As colunas, a barra e fora da janela: o Windows decide como sempre.
        assert_eq!(
            wheel_route((1099, 400), Some(panel), true),
            WheelRoute::PassThrough
        );
        assert_eq!(
            wheel_route((1300, 87), Some(panel), true),
            WheelRoute::PassThrough
        );
        assert_eq!(
            wheel_route((1600, 400), Some(panel), true),
            WheelRoute::PassThrough
        );
        // Outra aplicacao em primeiro plano, ou nenhum painel a vista.
        assert_eq!(
            wheel_route((1300, 400), Some(panel), false),
            WheelRoute::PassThrough
        );
        assert_eq!(
            wheel_route((1300, 400), None, true),
            WheelRoute::PassThrough
        );

        // O giro e o ponto chegam intactos a janela do painel.
        let (wparam, lparam) = wheel_message_params(-120, 0x0008, 1300, 400);
        assert_eq!((wparam >> 16) as u16 as i16, -120);
        assert_eq!(wparam & 0xffff, 0x0008);
        assert_eq!((lparam & 0xffff) as u16 as i16, 1300);
        assert_eq!(((lparam >> 16) & 0xffff) as u16 as i16, 400);
        // Monitor a esquerda do principal: x negativo.
        let (_, lparam) = wheel_message_params(120, 0, -500, 20);
        assert_eq!((lparam & 0xffff) as u16 as i16, -500);
    }

    #[test]
    fn service_panel_minimize_fullscreen_and_close_follow_one_state_machine() {
        let mut state = ServicePanelState::default();
        assert!(state.docked());
        assert_eq!(state.reserved_width(560.0), 560.0);
        assert_eq!(state.badge(true), None);

        // Minimizar: some da frente, as colunas recuperam a largura, o icone
        // ganha o ponto -- vermelho a tocar, neutro em silencio.
        assert_eq!(state.step(ServiceInput::Minimize), ServiceEffect::Relayout);
        assert!(state.minimized());
        assert_eq!(state.reserved_width(560.0), 0.0);
        assert_eq!(state.panel_area(560.0, 1600.0, 900.0, 88.0, 34.0), None);
        assert_eq!(state.strip_area(560.0, 1600.0, 88.0, 34.0), None);
        assert_eq!(state.badge(true), Some(ServiceBadge::Playing));
        assert_eq!(state.badge(false), Some(ServiceBadge::Minimized));
        // Minimizado, a pagina nao toma o ecra nem a faixa faz nada.
        assert_eq!(
            state.step(ServiceInput::PageFullscreen(true)),
            ServiceEffect::Nothing
        );
        assert!(!state.fullscreen());
        assert_eq!(
            state.step(ServiceInput::ToggleFullscreen),
            ServiceEffect::Nothing
        );
        // O icone restaura (nao fecha).
        assert_eq!(state.step(ServiceInput::IconClick), ServiceEffect::Relayout);
        assert!(state.docked());
        assert_eq!(state.badge(true), None);
        let docked = state
            .panel_area(560.0, 1600.0, 900.0, 88.0, 34.0)
            .expect("a vista");
        assert_eq!(
            (docked.x, docked.y, docked.width, docked.height),
            (1040.0, 122.0, 560.0, 778.0)
        );
        let strip = state.strip_area(560.0, 1600.0, 88.0, 34.0).expect("faixa");
        assert_eq!(
            (strip.x, strip.y, strip.width, strip.height),
            (1040.0, 88.0, 560.0, 34.0)
        );

        // A pagina (botao do YouTube) entra em tela cheia: a janela inteira.
        assert_eq!(
            state.step(ServiceInput::PageFullscreen(true)),
            ServiceEffect::Relayout
        );
        assert!(state.fullscreen());
        assert!(
            !state.app_fullscreen(),
            "a tela cheia da pagina tem a saida dela"
        );
        let full = state
            .panel_area(560.0, 1600.0, 900.0, 88.0, 34.0)
            .expect("a vista");
        assert_eq!(
            (full.x, full.y, full.width, full.height),
            (0.0, 0.0, 1600.0, 900.0)
        );
        assert_eq!(state.strip_area(560.0, 1600.0, 88.0, 34.0), None);
        // Aviso repetido nao muda nada; o Esc pede a pagina que saia.
        assert_eq!(
            state.step(ServiceInput::PageFullscreen(true)),
            ServiceEffect::Nothing
        );
        assert_eq!(
            state.step(ServiceInput::Escape),
            ServiceEffect::ExitPageFullscreen
        );
        assert!(state.fullscreen(), "so sai quando a pagina confirmar");
        assert_eq!(
            state.step(ServiceInput::PageFullscreen(false)),
            ServiceEffect::Relayout
        );
        assert!(state.docked());

        // Tela cheia pela faixa: o Esc volta ao painel encostado.
        assert_eq!(
            state.step(ServiceInput::ToggleFullscreen),
            ServiceEffect::Relayout
        );
        assert!(state.app_fullscreen());
        assert_eq!(state.step(ServiceInput::Escape), ServiceEffect::Relayout);
        assert!(state.docked());
        // Esc fora de tela cheia e da pagina.
        assert_eq!(state.step(ServiceInput::Escape), ServiceEffect::Nothing);

        // As duas ao mesmo tempo: sair da pagina deixa a da faixa.
        state.step(ServiceInput::ToggleFullscreen);
        state.step(ServiceInput::PageFullscreen(true));
        assert_eq!(
            state.step(ServiceInput::PageFullscreen(false)),
            ServiceEffect::Nothing
        );
        assert!(state.fullscreen());
        // O "Sair" nativo com a pagina em tela cheia sai das duas.
        state.step(ServiceInput::PageFullscreen(true));
        assert_eq!(
            state.step(ServiceInput::ToggleFullscreen),
            ServiceEffect::ExitPageFullscreen
        );
        assert_eq!(
            state.step(ServiceInput::PageFullscreen(false)),
            ServiceEffect::Relayout
        );
        assert!(state.docked());

        // Minimizar so a partir do painel encostado.
        state.step(ServiceInput::ToggleFullscreen);
        assert_eq!(state.step(ServiceInput::Minimize), ServiceEffect::Nothing);
        state.step(ServiceInput::ToggleFullscreen);

        // Aberto, o icone fecha; o x fecha de qualquer modo.
        assert_eq!(state.step(ServiceInput::IconClick), ServiceEffect::Close);
        state.step(ServiceInput::Minimize);
        assert_eq!(state.step(ServiceInput::Close), ServiceEffect::Close);
    }

    #[test]
    fn escape_belongs_to_the_page_unless_the_panel_is_fullscreen() {
        let mut state = ServicePanelState::default();
        assert!(!escape_is_ours(0x1B, true, state));
        state.step(ServiceInput::PageFullscreen(true));
        assert!(escape_is_ours(0x1B, true, state));
        assert!(!escape_is_ours(0x1B, false, state), "so a tecla em baixo");
        assert!(!escape_is_ours(0x0D, true, state));
        assert!(EXIT_PAGE_FULLSCREEN_SCRIPT.contains("document.exitFullscreen()"));
    }
}
