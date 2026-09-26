//! Centro de avisos (infra-notify-popups, plano 2.3): o que decide se um
//! aviso aparece ja no canto, espera na fila ou morre -- sem Win32 e sem
//! relogio, testavel em qualquer plataforma.
//!
//! Um `Notice` e um aviso de um tipo (`NoticeKind`) com titulo, corpo, no
//! maximo duas accoes e um prazo. `notify_route` decide a entrega pelo modo
//! Foco e pela privacidade: com o Foco ligado o aviso vai para a fila (e sai
//! resumido quando o Foco acaba -- "Gmail · 3 e-mails durante o foco --
//! abrir?"); no modo privado um corpo que traz conteudo (`content_bearing`)
//! passa a uma linha neutra. `NotifyCentre` e o estado do UNICO aviso do
//! canto: no maximo um a vista; outro do mesmo tipo substitui-o (como o do
//! Gmail sempre fez), um de outro tipo espera na fila, deduplicada por tipo.
//! A janela do aviso (sem ativacao, nunca TOPMOST) vive em
//! `windows_app/toast.rs`.

use std::time::Duration;

/// A linha que substitui o corpo de um aviso com conteudo no modo privado.
pub(crate) const PRIVATE_NEUTRAL_BODY: &str = "Conteúdo oculto no modo privado";

/// No maximo duas accoes por aviso: sao dois botoes no canto do toast.
pub(crate) const MAX_NOTICE_ACTIONS: usize = 2;

/// De onde vem um aviso. A fila deduplica por tipo. So o Gmail existe no
/// produto por agora; os outros sao dos itens seguintes do plano 2.3
/// (downloads-ui, copilot-radar, agentes, abas) e ja contam nos gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum NoticeKind {
    Gmail,
    Download,
    Radar,
    Agent,
    Pomodoro,
    Tabs,
    Generic,
}

impl NoticeKind {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 7] = [
        Self::Gmail,
        Self::Download,
        Self::Radar,
        Self::Agent,
        Self::Pomodoro,
        Self::Tabs,
        Self::Generic,
    ];

    /// O nome no resumo do Foco e a palavra que conta (singular, plural).
    fn summary_words(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Gmail => ("Gmail", "e-mail", "e-mails"),
            Self::Download => ("Downloads", "download", "downloads"),
            Self::Radar => ("Radar", "alerta", "alertas"),
            Self::Agent => ("Agente", "aviso", "avisos"),
            Self::Pomodoro => ("Pomodoro", "aviso", "avisos"),
            Self::Tabs => ("Abas", "aviso", "avisos"),
            Self::Generic => ("NeuralIA", "aviso", "avisos"),
        }
    }

    /// O Pomodoro E o foco: o fim de uma fase e o aviso que quem se
    /// concentra esta a espera de ver. Todos os outros esperam o fim dele.
    fn passes_focus(self) -> bool {
        self == Self::Pomodoro
    }
}

/// O que um botao do aviso faz. Quem executa e o `App`, pelo tipo do aviso
/// (no Gmail, `Open` abre o painel do Gmail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeReply {
    Open,
    Dismiss,
}

/// Um botao do aviso: o rotulo, a largura em pixeis logicos (a do Gmail e a
/// de sempre: 70 e 54) e se e o principal (cheio, na cor de destaque).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoticeAction {
    pub(crate) label: &'static str,
    pub(crate) width: f64,
    pub(crate) primary: bool,
    pub(crate) reply: NoticeReply,
}

/// Privacidade da janela no momento do aviso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Privacy {
    #[default]
    Normal,
    Private,
}

/// Um aviso. O titulo e escrito pelo nativo e nunca traz conteudo; o corpo
/// pode trazer (`content_bearing`: o remetente e o assunto de um e-mail, o
/// nome de um ficheiro) e e ele que o modo privado neutraliza.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Notice {
    pub(crate) kind: NoticeKind,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) actions: Vec<NoticeAction>,
    pub(crate) ttl: Duration,
    pub(crate) content_bearing: bool,
}

impl Notice {
    /// Um aviso que o canto consegue mostrar: com titulo, com prazo e com
    /// botoes que cabem.
    fn deliverable(&self) -> bool {
        !self.title.trim().is_empty()
            && !self.ttl.is_zero()
            && self.actions.len() <= MAX_NOTICE_ACTIONS
    }

    /// O mesmo aviso como o modo `privacy` o deixa mostrar.
    pub(crate) fn for_privacy(mut self, privacy: Privacy) -> Self {
        if privacy == Privacy::Private && self.content_bearing {
            self.body = PRIVATE_NEUTRAL_BODY.to_string();
            self.content_bearing = false;
        }
        self
    }
}

/// O que `notify_route` decide para um aviso.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NoticeRoute {
    /// Ao canto, ja (ou logo que o aviso de outro tipo a vista saia).
    Toast(Notice),
    /// A fila do Foco: sai resumido quando ele acabar.
    Queue(Notice),
    /// Nada a mostrar: sem titulo, sem prazo ou com botoes a mais.
    Drop,
}

/// A decisao de entrega, pura. O aviso que sai ja vem como a privacidade o
/// deixa mostrar: no modo privado, um corpo com conteudo e a linha neutra,
/// tambem na fila (o que espera o fim do Foco nao guarda o assunto).
pub(crate) fn notify_route(notice: &Notice, focus_shield: bool, privacy: Privacy) -> NoticeRoute {
    if !notice.deliverable() {
        return NoticeRoute::Drop;
    }
    let shown = notice.clone().for_privacy(privacy);
    if focus_shield && !notice.kind.passes_focus() {
        NoticeRoute::Queue(shown)
    } else {
        NoticeRoute::Toast(shown)
    }
}

/// Uma entrada da fila: o aviso mais recente do tipo, quantos chegaram e se
/// algum chegou durante o Foco.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QueuedNotice {
    pub(crate) notice: Notice,
    pub(crate) count: usize,
    pub(crate) during_focus: bool,
}

impl QueuedNotice {
    /// O que sai da fila: varios do mesmo tipo durante o Foco saem num
    /// resumo; um so, ou os que esperaram so por outro aviso, saem como o
    /// mais recente.
    fn delivered(self) -> Notice {
        if self.during_focus && self.count > 1 {
            focus_summary(&self.notice, self.count)
        } else {
            self.notice
        }
    }
}

/// "Gmail · 3 e-mails durante o foco — abrir?": o resumo do que chegou
/// durante o Foco, com os botoes e o prazo do mais recente e o corpo dele
/// (ja neutro, se chegou no modo privado).
pub(crate) fn focus_summary(latest: &Notice, count: usize) -> Notice {
    let (name, one, many) = latest.kind.summary_words();
    let noun = if count == 1 { one } else { many };
    let ask = if latest
        .actions
        .iter()
        .any(|action| action.reply == NoticeReply::Open)
    {
        " — abrir?"
    } else {
        ""
    };
    Notice {
        title: format!("{name} · {count} {noun} durante o foco{ask}"),
        ..latest.clone()
    }
}

/// A fila, deduplicada por tipo: um tipo que ja la esta fica no lugar dele
/// (a ordem e a da primeira chegada) com o aviso mais recente e a conta.
#[derive(Debug, Default)]
pub(crate) struct NoticeQueue {
    entries: Vec<QueuedNotice>,
}

impl NoticeQueue {
    pub(crate) fn push(&mut self, notice: Notice, during_focus: bool) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.notice.kind == notice.kind)
        {
            entry.notice = notice;
            entry.count += 1;
            entry.during_focus |= during_focus;
        } else {
            self.entries.push(QueuedNotice {
                notice,
                count: 1,
                during_focus,
            });
        }
    }

    /// O proximo a sair, pela ordem de chegada do tipo.
    pub(crate) fn pop(&mut self) -> Option<Notice> {
        self.pop_first(|_| true)
    }

    /// O primeiro, pela ordem de chegada do tipo, entre os tipos que
    /// `allowed` deixa sair; os outros ficam no lugar deles.
    fn pop_first(&mut self, allowed: impl Fn(NoticeKind) -> bool) -> Option<Notice> {
        let index = self
            .entries
            .iter()
            .position(|entry| allowed(entry.notice.kind))?;
        Some(self.entries.remove(index).delivered())
    }

    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[QueuedNotice] {
        &self.entries
    }
}

/// O que o canto passa a mostrar: o aviso e o token do temporizador dele.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToastFrame {
    pub(crate) token: u64,
    pub(crate) notice: Notice,
}

/// O fim de um aviso (o prazo dele passou).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToastHide {
    /// Temporizador de um aviso que ja foi substituido.
    Stale,
    /// Sai; `next` e o que esperava na fila (durante o Foco, so um tipo que
    /// passa o Foco: o Pomodoro que esperava por um aviso de outro tipo).
    Hide { next: Option<ToastFrame> },
}

/// O unico aviso do canto e a fila. O token sobe a cada aviso mostrado: so
/// o temporizador do ultimo o tira, e so um clique no ultimo responde.
#[derive(Debug, Default)]
pub(crate) struct NotifyCentre {
    token: u64,
    current: Option<Notice>,
    queue: NoticeQueue,
    focus_shield: bool,
    privacy: Privacy,
}

impl NotifyCentre {
    /// Um aviso novo. `Some`: mostrar ja (e agendar o fim pelo `ttl`).
    pub(crate) fn post(&mut self, notice: Notice) -> Option<ToastFrame> {
        match notify_route(&notice, self.focus_shield, self.privacy) {
            NoticeRoute::Drop => None,
            NoticeRoute::Queue(notice) => {
                self.queue.push(notice, true);
                None
            }
            NoticeRoute::Toast(notice) => match &self.current {
                // Um de cada vez: o de outro tipo espera pelo fim deste.
                Some(current) if current.kind != notice.kind => {
                    self.queue.push(notice, false);
                    None
                }
                // Nada a vista, ou o mesmo tipo: substitui ja.
                _ => Some(self.frame(notice)),
            },
        }
    }

    fn frame(&mut self, notice: Notice) -> ToastFrame {
        let notice = notice.for_privacy(self.privacy);
        self.token = self.token.wrapping_add(1);
        self.current = Some(notice.clone());
        ToastFrame {
            token: self.token,
            notice,
        }
    }

    /// O prazo do aviso `token` passou. Sai o proximo da fila; durante o
    /// Foco so sai um tipo que o passa (um Pomodoro que chegou com um aviso
    /// de outro tipo a vista), e o resto espera o fim do Foco.
    pub(crate) fn hidden(&mut self, token: u64) -> ToastHide {
        if token != self.token {
            return ToastHide::Stale;
        }
        self.current = None;
        let shield = self.focus_shield;
        let next = self
            .queue
            .pop_first(|kind| !shield || kind.passes_focus())
            .map(|notice| self.frame(notice));
        ToastHide::Hide { next }
    }

    /// Clique no botao `index` do aviso `token`: o tipo e a resposta, se o
    /// aviso e o que esta a vista e o botao existe.
    pub(crate) fn answer(&self, token: u64, index: usize) -> Option<(NoticeKind, NoticeReply)> {
        if token != self.token {
            return None;
        }
        let current = self.current.as_ref()?;
        current
            .actions
            .get(index)
            .map(|action| (current.kind, action.reply))
    }

    /// O tipo do aviso a vista, se houver.
    pub(crate) fn current_kind(&self) -> Option<NoticeKind> {
        self.current.as_ref().map(|notice| notice.kind)
    }

    /// Liga ou desliga o Foco. Ao desligar, com o canto livre, sai o
    /// primeiro da fila (os outros saem um a um, no fim de cada um).
    // A feature `focus` do plano 2.3 liga isto; ate la o `App` nunca ativa
    // o Foco e so os gates o chamam.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_focus_shield(&mut self, on: bool) -> Option<ToastFrame> {
        self.focus_shield = on;
        if on || self.current.is_some() {
            return None;
        }
        self.queue.pop().map(|notice| self.frame(notice))
    }

    /// Entra ou sai do modo privado: o que ainda sai da fila sai ja como
    /// ele o deixa mostrar.
    // A feature `private-mode-surfaces` liga isto; ate la so os gates.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_privacy(&mut self, privacy: Privacy) {
        self.privacy = privacy;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn queue(&self) -> &NoticeQueue {
        &self.queue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> NoticeAction {
        NoticeAction {
            label: "Abrir",
            width: 70.0,
            primary: true,
            reply: NoticeReply::Open,
        }
    }

    fn dismiss() -> NoticeAction {
        NoticeAction {
            label: "Não",
            width: 54.0,
            primary: false,
            reply: NoticeReply::Dismiss,
        }
    }

    fn notice(kind: NoticeKind, body: &str) -> Notice {
        Notice {
            kind,
            title: format!("{kind:?} · aviso"),
            body: body.to_string(),
            actions: vec![open(), dismiss()],
            ttl: Duration::from_secs(12),
            content_bearing: true,
        }
    }

    /// Gate (critico, sabotado): a tabela inteira da entrega -- cada tipo
    /// com o Foco desligado e ligado, no modo normal e no privado, com e sem
    /// conteudo -- mais os avisos que nao se mostram. Um `notify_route` que
    /// ignore o Foco poe o Gmail no canto a meio dele; um que ignore a
    /// privacidade mostra o assunto de um e-mail no modo privado.
    #[test]
    fn notify_route_table() {
        for kind in NoticeKind::ALL {
            for focus in [false, true] {
                for privacy in [Privacy::Normal, Privacy::Private] {
                    for content_bearing in [false, true] {
                        let mut sent = notice(kind, "Ana · Relatório de março");
                        sent.content_bearing = content_bearing;
                        let hidden = privacy == Privacy::Private && content_bearing;
                        let body = if hidden {
                            PRIVATE_NEUTRAL_BODY
                        } else {
                            "Ana · Relatório de março"
                        };
                        let mut shown = sent.clone();
                        shown.body = body.to_string();
                        shown.content_bearing = content_bearing && !hidden;
                        let expected = if focus && kind != NoticeKind::Pomodoro {
                            NoticeRoute::Queue(shown)
                        } else {
                            NoticeRoute::Toast(shown)
                        };
                        assert_eq!(
                            notify_route(&sent, focus, privacy),
                            expected,
                            "{kind:?} foco={focus} {privacy:?} conteudo={content_bearing}"
                        );
                    }
                }
            }
        }

        // O titulo nunca e neutralizado: e escrito pelo nativo.
        let private = notify_route(&notice(NoticeKind::Gmail, "a · b"), false, Privacy::Private);
        let NoticeRoute::Toast(shown) = private else {
            panic!("{private:?}");
        };
        assert_eq!(shown.title, "Gmail · aviso");
        assert_eq!(shown.actions, vec![open(), dismiss()]);

        // O que o canto nao consegue mostrar morre, com ou sem Foco.
        let mut untitled = notice(NoticeKind::Download, "x");
        untitled.title = "  ".to_string();
        let mut instant = notice(NoticeKind::Radar, "x");
        instant.ttl = Duration::ZERO;
        let mut crowded = notice(NoticeKind::Agent, "x");
        crowded.actions.push(open());
        for bad in [untitled, instant, crowded] {
            for focus in [false, true] {
                assert_eq!(
                    notify_route(&bad, focus, Privacy::Normal),
                    NoticeRoute::Drop,
                    "{bad:?}"
                );
            }
        }
    }

    /// Gate: a fila sai pela ordem da primeira chegada de cada tipo, com um
    /// lugar por tipo (o mais recente e a conta); varios do mesmo tipo
    /// durante o Foco saem num resumo, e um aviso de outro tipo espera pelo
    /// que esta a vista em vez de o tapar. Durante o Foco, o fim de um aviso
    /// solta so o Pomodoro que esperava por ele (nunca o prende ate ao fim
    /// do Foco); os outros esperam o Foco acabar.
    #[test]
    fn queue_order_and_dedupe() {
        // A fila sozinha: ordem de chegada, dedupe por tipo.
        let mut queue = NoticeQueue::default();
        queue.push(notice(NoticeKind::Gmail, "g1"), true);
        queue.push(notice(NoticeKind::Download, "d1"), true);
        queue.push(notice(NoticeKind::Gmail, "g2"), true);
        queue.push(notice(NoticeKind::Radar, "r1"), false);
        queue.push(notice(NoticeKind::Gmail, "g3"), true);
        let kinds: Vec<(NoticeKind, usize)> = queue
            .entries()
            .iter()
            .map(|entry| (entry.notice.kind, entry.count))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (NoticeKind::Gmail, 3),
                (NoticeKind::Download, 1),
                (NoticeKind::Radar, 1)
            ]
        );
        let gmail = queue.pop().expect("o Gmail chegou primeiro");
        assert_eq!(gmail.title, "Gmail · 3 e-mails durante o foco — abrir?");
        assert_eq!(gmail.body, "g3", "o resumo leva o mais recente");
        assert_eq!(gmail.actions, vec![open(), dismiss()]);
        assert_eq!(queue.pop().map(|n| n.body), Some("d1".to_string()));
        assert_eq!(queue.pop().map(|n| n.body), Some("r1".to_string()));
        assert_eq!(queue.pop(), None);

        // Sem botao de abrir, o resumo nao pergunta.
        let mut quiet = notice(NoticeKind::Download, "d");
        quiet.actions = vec![dismiss()];
        assert_eq!(
            focus_summary(&quiet, 2).title,
            "Downloads · 2 downloads durante o foco"
        );

        // O centro: o Foco enche a fila e so a esvazia quando acaba.
        let mut centre = NotifyCentre::default();
        assert_eq!(centre.set_focus_shield(true), None);
        assert_eq!(centre.post(notice(NoticeKind::Gmail, "a")), None);
        assert_eq!(centre.post(notice(NoticeKind::Tabs, "t")), None);
        assert_eq!(centre.post(notice(NoticeKind::Gmail, "b")), None);
        assert_eq!(centre.post(notice(NoticeKind::Gmail, "c")), None);
        // O Pomodoro passa o Foco.
        let pomodoro = centre
            .post(notice(NoticeKind::Pomodoro, "fim do foco"))
            .expect("o fim de fase aparece durante o foco");
        assert_eq!(pomodoro.notice.kind, NoticeKind::Pomodoro);
        // Durante o Foco, o fim do Pomodoro nao solta a fila.
        assert_eq!(
            centre.hidden(pomodoro.token),
            ToastHide::Hide { next: None }
        );
        let first = centre
            .set_focus_shield(false)
            .expect("o fim do foco solta a fila");
        assert_eq!(
            first.notice.title,
            "Gmail · 3 e-mails durante o foco — abrir?"
        );
        assert_eq!(first.notice.body, "c");
        // Os seguintes saem um a um, no fim do anterior.
        let ToastHide::Hide { next: Some(tabs) } = centre.hidden(first.token) else {
            panic!("as abas esperavam");
        };
        assert_eq!(tabs.notice.kind, NoticeKind::Tabs);
        assert_eq!(tabs.notice.body, "t", "um so sai como chegou");
        assert_eq!(centre.hidden(tabs.token), ToastHide::Hide { next: None });

        // Um aviso de outro tipo a vista quando o Foco liga nao prende o
        // Pomodoro ate ao fim do Foco: no fim desse aviso sai o Pomodoro,
        // mesmo atras de um que chegou antes e espera o Foco.
        let mut centre = NotifyCentre::default();
        let gmail = centre
            .post(notice(NoticeKind::Gmail, "antes do foco"))
            .expect("ja");
        assert_eq!(centre.set_focus_shield(true), None);
        assert_eq!(centre.post(notice(NoticeKind::Download, "d")), None);
        assert_eq!(
            centre.post(notice(NoticeKind::Pomodoro, "fim da pausa")),
            None,
            "um de cada vez: o Pomodoro espera pelo Gmail a vista"
        );
        let hide = centre.hidden(gmail.token);
        let ToastHide::Hide {
            next: Some(pomodoro),
        } = hide
        else {
            panic!("o Pomodoro ficou na fila durante o foco: {hide:?}");
        };
        assert_eq!(pomodoro.notice.kind, NoticeKind::Pomodoro);
        assert_eq!(pomodoro.notice.body, "fim da pausa");
        assert_eq!(
            centre.hidden(pomodoro.token),
            ToastHide::Hide { next: None },
            "o download espera o fim do foco"
        );
        let download = centre
            .set_focus_shield(false)
            .expect("o fim do foco solta a fila");
        assert_eq!(download.notice.kind, NoticeKind::Download);
        assert!(centre.queue().entries().is_empty());

        // Fora do Foco: o mesmo tipo substitui ja; outro tipo espera.
        let mut centre = NotifyCentre::default();
        let g1 = centre.post(notice(NoticeKind::Gmail, "g1")).expect("ja");
        let g2 = centre.post(notice(NoticeKind::Gmail, "g2")).expect("ja");
        assert_eq!(g2.token, g1.token + 1);
        assert_eq!(centre.post(notice(NoticeKind::Download, "d1")), None);
        assert_eq!(centre.post(notice(NoticeKind::Download, "d2")), None);
        assert_eq!(centre.hidden(g1.token), ToastHide::Stale);
        let ToastHide::Hide {
            next: Some(download),
        } = centre.hidden(g2.token)
        else {
            panic!("o download esperava pelo Gmail");
        };
        assert_eq!(
            download.notice.body, "d2",
            "fora do foco nao ha resumo: sai o mais recente"
        );
        assert_eq!(
            centre.answer(download.token, 0),
            Some((NoticeKind::Download, NoticeReply::Open))
        );
        assert_eq!(centre.answer(download.token, 2), None);
        assert_eq!(centre.answer(g2.token, 0), None, "um clique velho");

        // O modo privado vale tambem para o que ja estava na fila.
        let mut centre = NotifyCentre::default();
        centre.set_focus_shield(true);
        centre.post(notice(NoticeKind::Gmail, "Ana · segredo"));
        centre.set_privacy(Privacy::Private);
        let shown = centre.set_focus_shield(false).expect("sai");
        assert_eq!(shown.notice.body, PRIVATE_NEUTRAL_BODY);
        assert!(centre.queue().entries().is_empty());
    }
}
