use super::*;

pub(in crate::windows_app) const SPLASH_SUBCLASS_ID: usize = 0x4E4C;
pub(in crate::windows_app) const SPLASH_WIDTH: f64 = 470.0;
pub(in crate::windows_app) const SPLASH_HEIGHT: f64 = 46.0;

/// Texto do aviso flutuante. Vive fora do App porque quem o pinta e o
/// procedimento de janela, que nao tem acesso ao estado da aplicacao.
pub(in crate::windows_app) static SPLASH_TEXT: Mutex<String> = Mutex::new(String::new());

/// Um botao da pergunta do meio da janela.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct SplashButton {
    pub(in crate::windows_app) label: &'static str,
    /// O principal: cheio, na cor de destaque.
    pub(in crate::windows_app) primary: bool,
}

/// Quem faz a pergunta: a resposta volta para ele (`App::answer_splash`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum SplashAsker {
    /// "Rolar a pagina sozinho...?"
    AutoScroll,
}

/// Uma pergunta centrada na janela com os seus proprios botoes
/// (infra-notify-popups: substitui o `SPLASH_ASKS`, que era sempre Sim/Nao
/// e sempre da rolagem). Os botoes ficam a direita do texto, pela ordem da
/// lista; o clique no botao `index` chega como
/// `UserEvent::SplashAnswer { asker, index }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct SplashQuestion {
    pub(in crate::windows_app) asker: SplashAsker,
    pub(in crate::windows_app) buttons: &'static [SplashButton],
}

/// A pergunta da rolagem automatica: Sim (principal) e Nao.
pub(in crate::windows_app) const AUTO_SCROLL_QUESTION: SplashQuestion = SplashQuestion {
    asker: SplashAsker::AutoScroll,
    buttons: &[
        SplashButton {
            label: "Sim",
            primary: true,
        },
        SplashButton {
            label: "Não",
            primary: false,
        },
    ],
};

/// A pergunta a vista; `None` enquanto a janela e so um aviso (sem botoes e
/// transparente aos cliques).
pub(in crate::windows_app) static SPLASH_QUESTION: Mutex<Option<SplashQuestion>> = Mutex::new(None);

fn splash_question() -> Option<SplashQuestion> {
    SPLASH_QUESTION.lock().ok().and_then(|question| *question)
}

/// Os `count` botoes, encostados a direita: cada um com um quinto da
/// largura, separados por um quadragesimo. Uma so funcao para o desenho e
/// o clique concordarem sempre; com dois sao o Sim e o Nao de sempre.
pub(in crate::windows_app) fn splash_buttons(client: &RECT, count: usize) -> Vec<RECT> {
    let width = client.right - client.left;
    let button = width / 5;
    let margin = width / 40;
    let mut right = client.right - margin;
    let mut rects: Vec<RECT> = (0..count)
        .map(|_| {
            let rect = RECT {
                left: right - button,
                top: client.top + margin,
                right,
                bottom: client.bottom - margin,
            };
            right = rect.left - margin;
            rect
        })
        .collect();
    rects.reverse();
    rects
}

/// O botao da pergunta na coluna `x` (so a coluna conta, como sempre).
pub(in crate::windows_app) fn splash_button_at(buttons: &[RECT], x: i32) -> Option<usize> {
    buttons
        .iter()
        .position(|button| x >= button.left && x < button.right)
}

/// Aviso flutuante no fundo do ecra. Tem de ser nativo e nao injetado na
/// pagina: por cima de um PDF nao ha pagina nossa onde escrever -- o
/// visualizador do Edge e outro documento, noutra origem e noutro processo.
pub(in crate::windows_app) unsafe extern "system" fn splash_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    let question = if matches!(message, WM_NCHITTEST | WM_LBUTTONUP | WM_PAINT) {
        splash_question()
    } else {
        None
    };
    // Quando a janela faz uma pergunta tem de receber cliques; uma janela da
    // classe STATIC devolve HTTRANSPARENT e o clique atravessava-a.
    if message == WM_NCHITTEST && question.is_some() {
        return HTCLIENT as LRESULT;
    }

    if message == WM_LBUTTONUP
        && reference_data != 0
        && let Some(question) = question
    {
        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client) != 0 {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let buttons = splash_buttons(&client, question.buttons.len());
            if let Some(index) = splash_button_at(&buttons, x) {
                let proxy = &*(reference_data as *const EventLoopProxy<UserEvent>);
                let _ = proxy.send_event(UserEvent::SplashAnswer {
                    asker: question.asker,
                    index,
                });
            }
        }
        return 0;
    }

    if message == WM_PAINT {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        if !hdc.is_null() {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0 {
                let theme = Theme::system();
                let height = (client.bottom - client.top) as f64;

                let background = CreateSolidBrush(rgb3(theme.surface));
                FillRect(hdc, &client, background);
                DeleteObject(background as _);

                let scale = (height / SPLASH_HEIGHT).max(1.0);
                let font = create_font((-14.0 * scale) as i32, FW_NORMAL as i32);
                let old_font = SelectObject(hdc, font as _);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, rgb3(theme.fg));

                let text = SPLASH_TEXT
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();

                if let Some(question) = question {
                    let buttons = splash_buttons(&client, question.buttons.len());
                    let first = buttons.first().map_or(client.right, |button| button.left);
                    let mut asked = RECT {
                        left: client.left + (18.0 * scale) as i32,
                        top: client.top,
                        right: first - (10.0 * scale) as i32,
                        bottom: client.bottom,
                    };
                    draw_text(
                        hdc,
                        &text,
                        &mut asked,
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                    );

                    for (rect, button) in buttons.iter().zip(question.buttons) {
                        let pill = UiRect {
                            x: rect.left as f64,
                            y: rect.top as f64,
                            width: (rect.right - rect.left) as f64,
                            height: (rect.bottom - rect.top) as f64,
                        };
                        let style = if button.primary {
                            PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
                        } else {
                            PillStyle::new(
                                mix(theme.surface, theme.fg, 0.10),
                                theme.surface_line,
                                theme.fg,
                            )
                        };
                        draw_pill(hdc, pill, button.label, style, scale, font, theme.surface);
                    }
                } else {
                    let mut rect = client;
                    draw_text(
                        hdc,
                        &text,
                        &mut rect,
                        DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
                    );
                }

                SelectObject(hdc, old_font);
                DeleteObject(font as _);
            }
            EndPaint(hwnd, &paint);
        }
        return 0;
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

/// Canto do splash (pergunta da rolagem automatica e afins) relativo ao
/// cliente: centrado nos dois eixos. Ficava a 48 px do fundo, e ao arrancar
/// lia-se como um rodape perdido por baixo das colunas. Nunca sai pelo topo
/// nem pela esquerda numa janela mais pequena do que ele.
pub(in crate::windows_app) fn splash_origin(
    client_w: i32,
    client_h: i32,
    width: i32,
    height: i32,
) -> (i32, i32) {
    (
        ((client_w - width) / 2).max(0),
        ((client_h - height) / 2).max(0),
    )
}

/// Quem pede o popup do meio da janela.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum SplashKind {
    /// Resposta a um gesto (zoom, "Nota criada", "Pomodoro iniciado"...):
    /// aparece ja. Com a pergunta da rolagem a vista, tira-a SEM lhe
    /// responder -- ela volta a ser feita na proxima leitura.
    Notice,
    /// Aviso que chega sozinho (o fim de uma fase do Pomodoro): com a
    /// pergunta a vista, espera que ela saia.
    Background,
    /// Uma pergunta com os seus botoes ("Rolar a pagina sozinho...?" com
    /// Sim e Nao).
    Question(SplashQuestion),
}

/// O que o popup passa a mostrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct SplashFrame {
    pub(in crate::windows_app) text: String,
    /// A pergunta, com os botoes dela (`SPLASH_QUESTION`); `None` num aviso.
    pub(in crate::windows_app) question: Option<SplashQuestion>,
    pub(in crate::windows_app) seconds: u64,
    /// O `HideSplash` deste quadro.
    pub(in crate::windows_app) token: u64,
}

/// O fim de um quadro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum SplashHide {
    /// Temporizador de um quadro que ja foi substituido.
    Stale,
    Hide {
        /// A pergunta saiu sozinha, sem resposta: "nao" ate ao F8.
        question_expired: bool,
        /// O aviso que esperava pela pergunta, a mostrar agora.
        next: Option<SplashFrame>,
    },
}

/// O aviso e a pergunta da rolagem partilham UM popup. Antes, um aviso que
/// chegasse durante a pergunta (um fim de fase do Pomodoro, "Nota criada")
/// herdava o Sim/Nao -- um clique em "Sim" ligava a rolagem com o texto do
/// Pomodoro no ecra -- e o temporizador DELE apagava a pergunta como se ela
/// tivesse sido respondida "nao" para o resto da sessao. Aqui so a pergunta
/// tem botoes, e so o fim do quadro dela conta como resposta.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct SplashBoard {
    pub(in crate::windows_app) token: u64,
    /// Token da pergunta, enquanto e ela que esta a vista.
    pub(in crate::windows_app) question: Option<u64>,
    /// Um aviso de fundo que chegou com a pergunta a vista (so o ultimo).
    pub(in crate::windows_app) waiting: Option<(String, u64)>,
}

impl SplashBoard {
    /// `None`: fica a espera da pergunta (`SplashKind::Background`).
    pub(in crate::windows_app) fn show(
        &mut self,
        text: String,
        seconds: u64,
        kind: SplashKind,
    ) -> Option<SplashFrame> {
        if kind == SplashKind::Background && self.question.is_some() {
            self.waiting = Some((text, seconds));
            return None;
        }
        let question = match kind {
            SplashKind::Question(question) => Some(question),
            SplashKind::Notice | SplashKind::Background => None,
        };
        Some(self.frame(text, seconds, question))
    }

    pub(in crate::windows_app) fn frame(
        &mut self,
        text: String,
        seconds: u64,
        question: Option<SplashQuestion>,
    ) -> SplashFrame {
        self.token = self.token.wrapping_add(1);
        self.question = question.is_some().then_some(self.token);
        SplashFrame {
            text,
            question,
            seconds,
            token: self.token,
        }
    }

    /// O `HideSplash(token)` chegou.
    pub(in crate::windows_app) fn hide(&mut self, token: u64) -> SplashHide {
        if token != self.token {
            return SplashHide::Stale;
        }
        let question_expired = self.question.take() == Some(token);
        let next = self
            .waiting
            .take()
            .map(|(text, seconds)| self.frame(text, seconds, None));
        SplashHide::Hide {
            question_expired,
            next,
        }
    }

    /// A pergunta foi respondida (Sim, Nao ou F8): deixa de ser a pergunta.
    /// O quadro fica ate `hide` ou ate outro o substituir.
    pub(in crate::windows_app) fn answered(&mut self) {
        self.question = None;
    }

    /// O quadro a vista, para o esconder ja.
    pub(in crate::windows_app) fn current(&self) -> u64 {
        self.token
    }
}
