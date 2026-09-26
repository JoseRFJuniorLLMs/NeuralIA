//! Ler uma pagina por script (infra-llm-untrusted, plano 2.3). §7: scripts
//! so-leitura; cada um entra com o sim do dono e os numeros da sabotagem
//! (OQ2).
//!
//! `evaluate_script_with_callback` devolve, quando o WebView2 acabar -- ou
//! nunca --, o JSON do que o script retornou. Entre o pedido e a resposta a
//! pagina pode ter navegado, a resposta pode ser enorme e pode chegar
//! depois de quem pediu ja ter desistido. `PageReads` trata as tres:
//!
//! - um TOKEN por leitura, so do lado nativo (o script nao o ve, a pagina
//!   nao o forja), e no maximo `MAX_PENDING_READS` em voo;
//! - um PRAZO no servico `Timers` (`PageEvalEvent::Expired`), e o instante
//!   da chegada comparado com ele: uma resposta tardia nunca e entregue;
//! - um TECTO de bytes crus ANTES do serde: acima dele o callback nem leva o
//!   texto ao event loop (`RawArrival::OverCap`), e a entrega volta a medir;
//! - a GERACAO de navegacao da vista (`NavEpoch`, que o navigation handler
//!   dela incrementa) e o URL, conferidos na chegada: a leitura da pagina
//!   anterior -- outro URL, ou o mesmo recarregado -- cai.
//!
//! So scripts so-leitura correm por aqui: `ReadOnlyScript` so se constroi
//! neste modulo, `read` recusa um que nao esteja em `READ_ONLY_SCRIPTS`, e o
//! gate `page_eval_scripts_are_read_only` (windows_app/tests.rs) prova que
//! nenhum publica, busca, escuta, agenda, navega, guarda ou escreve HTML.
//!
//! Quem le (a Traducao, o leitor de respostas do Consenso, o Copiloto, o
//! Escudo) traz o seu `UserEvent`, o seu `PageReads` e liga o `NavEpoch` ao
//! navigation handler das vistas que le. Ate o primeiro chegar, este modulo
//! so corre nos testes.

use super::*;

/// Leituras em voo por `PageReads`. O prazo de cada uma liberta o lugar;
/// sem isto, respostas que nunca chegam enchiam a lista.
pub(in crate::windows_app) const MAX_PENDING_READS: usize = 8;

/// Um script que so le a pagina. So este modulo o constroi.
#[derive(Debug)]
pub(in crate::windows_app) struct ReadOnlyScript {
    name: &'static str,
    source: &'static str,
}

impl ReadOnlyScript {
    pub(in crate::windows_app) fn name(&self) -> &'static str {
        self.name
    }

    pub(in crate::windows_app) fn source(&self) -> &'static str {
        self.source
    }
}

/// A selecao da pagina para uma nota (Ctrl+Shift+Z): o primeiro script da
/// lista. O Ctrl+Shift+Z continua a chama-lo como sempre; estar aqui pe-lo
/// debaixo do gate de so-leitura.
pub(in crate::windows_app) static NOTE_CAPTURE_READ: ReadOnlyScript = ReadOnlyScript {
    name: "note-capture",
    source: NOTE_CAPTURE_SCRIPT,
};

/// Os unicos scripts que `PageReads::read` corre.
pub(in crate::windows_app) static READ_ONLY_SCRIPTS: &[&ReadOnlyScript] = &[&NOTE_CAPTURE_READ];

fn registered(script: &ReadOnlyScript) -> bool {
    READ_ONLY_SCRIPTS
        .iter()
        .any(|known| known.name == script.name && known.source == script.source)
}

/// A geracao de navegacao de uma vista: o navigation handler dela chama
/// `bump` a cada navegacao que comeca (tambem as recusadas: cair a mais e
/// o lado seguro).
#[derive(Debug, Clone, Default)]
pub(in crate::windows_app) struct NavEpoch(Arc<AtomicU64>);

impl NavEpoch {
    pub(in crate::windows_app) fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    pub(in crate::windows_app) fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// O que uma leitura pede.
#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct PageEvalSpec {
    pub(in crate::windows_app) script: &'static ReadOnlyScript,
    /// O tecto do JSON cru que o WebView2 devolve.
    pub(in crate::windows_app) max_raw_bytes: usize,
    /// Quanto se espera pela resposta.
    pub(in crate::windows_app) deadline: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::windows_app) struct PageReadToken(u64);

/// A resposta crua, ja medida no callback do WebView2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum RawArrival {
    Within(String),
    /// Acima do tecto: o texto ficou no callback.
    OverCap {
        bytes: usize,
    },
}

impl RawArrival {
    pub(in crate::windows_app) fn capped(raw: String, max_raw_bytes: usize) -> Self {
        if raw.len() > max_raw_bytes {
            Self::OverCap { bytes: raw.len() }
        } else {
            Self::Within(raw)
        }
    }
}

/// O que volta ao event loop, dentro do `UserEvent` de quem pediu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum PageEvalEvent {
    Arrived {
        token: PageReadToken,
        raw: RawArrival,
    },
    /// O prazo, pelo servico `Timers`.
    Expired(PageReadToken),
}

/// Porque uma leitura caiu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum PageEvalDrop {
    /// Passou o prazo.
    Late,
    OverCap {
        bytes: usize,
        limit: usize,
    },
    /// A vista navegou (outra geracao ou outro URL) ou ja nao existe.
    Navigated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum PageEvalOutcome {
    /// A resposta, crua e dentro do tecto, da pagina que foi pedida.
    Delivered {
        token: PageReadToken,
        script: &'static str,
        raw: String,
    },
    /// A leitura acabou sem resposta; dito uma vez por token.
    Dropped {
        token: PageReadToken,
        reason: PageEvalDrop,
    },
    /// Ja resolvida (o prazo chegou antes, ou foi cancelada): nada a fazer.
    Stale(PageReadToken),
}

/// Porque uma leitura nem comecou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum PageEvalRefusal {
    /// O script nao esta em `READ_ONLY_SCRIPTS`.
    Unregistered,
    /// A vista nao diz em que pagina esta.
    NoPage,
    /// `MAX_PENDING_READS` em voo.
    Busy,
    /// O WebView2 recusou o script.
    EvalFailed,
}

/// A vista que se le: a `WebView` do wry no produto, uma falsa nos testes.
pub(in crate::windows_app) trait EvalView {
    fn page_url(&self) -> Option<String>;
    fn eval_with_callback(
        &self,
        script: &str,
        callback: Box<dyn Fn(String) + Send + 'static>,
    ) -> bool;
}

impl EvalView for WebView {
    fn page_url(&self) -> Option<String> {
        self.url().ok()
    }

    fn eval_with_callback(
        &self,
        script: &str,
        callback: Box<dyn Fn(String) + Send + 'static>,
    ) -> bool {
        self.evaluate_script_with_callback(script, callback).is_ok()
    }
}

#[derive(Debug)]
struct PendingRead {
    token: PageReadToken,
    script: &'static str,
    epoch: NavEpoch,
    generation: u64,
    url: String,
    deadline: Instant,
    max_raw_bytes: usize,
}

/// As leituras em voo de quem le.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct PageReads {
    next_token: u64,
    pending: Vec<PendingRead>,
}

impl PageReads {
    /// Corre `spec.script` em `view`. `deliver` corre no callback do
    /// WebView2 e leva a resposta ao event loop no `UserEvent` de quem pediu
    /// (`move |event| { let _ = proxy.send_event(UserEvent::X(event)); }`);
    /// `schedule` agenda o prazo no servico `Timers`
    /// (`|delay, event| timers.after(delay, UserEvent::X(event))`). Os dois
    /// eventos voltam a `settle`.
    pub(in crate::windows_app) fn read<V: EvalView + ?Sized>(
        &mut self,
        view: &V,
        spec: PageEvalSpec,
        epoch: &NavEpoch,
        now: Instant,
        deliver: impl Fn(PageEvalEvent) + Send + 'static,
        schedule: impl FnOnce(Duration, PageEvalEvent),
    ) -> Result<PageReadToken, PageEvalRefusal> {
        if !registered(spec.script) {
            return Err(PageEvalRefusal::Unregistered);
        }
        if self.pending.len() >= MAX_PENDING_READS {
            return Err(PageEvalRefusal::Busy);
        }
        // A pagina e a geracao de ANTES de o script correr.
        let Some(url) = view.page_url() else {
            return Err(PageEvalRefusal::NoPage);
        };
        let generation = epoch.current();
        self.next_token = self.next_token.wrapping_add(1);
        let token = PageReadToken(self.next_token);
        let max_raw_bytes = spec.max_raw_bytes;
        let asked = view.eval_with_callback(
            spec.script.source,
            Box::new(move |raw| {
                deliver(PageEvalEvent::Arrived {
                    token,
                    raw: RawArrival::capped(raw, max_raw_bytes),
                });
            }),
        );
        if !asked {
            return Err(PageEvalRefusal::EvalFailed);
        }
        self.pending.push(PendingRead {
            token,
            script: spec.script.name,
            epoch: epoch.clone(),
            generation,
            url,
            deadline: now + spec.deadline,
            max_raw_bytes,
        });
        schedule(spec.deadline, PageEvalEvent::Expired(token));
        Ok(token)
    }

    /// Um evento de leitura chegou ao event loop. `page_url` e o URL que a
    /// vista lida mostra AGORA (`None` se ja nao existe).
    pub(in crate::windows_app) fn settle(
        &mut self,
        event: PageEvalEvent,
        page_url: impl FnOnce() -> Option<String>,
        now: Instant,
    ) -> PageEvalOutcome {
        let (token, raw) = match event {
            PageEvalEvent::Expired(token) => {
                return match self.take(token) {
                    Some(_) => PageEvalOutcome::Dropped {
                        token,
                        reason: PageEvalDrop::Late,
                    },
                    None => PageEvalOutcome::Stale(token),
                };
            }
            PageEvalEvent::Arrived { token, raw } => (token, raw),
        };
        let Some(read) = self.take(token) else {
            return PageEvalOutcome::Stale(token);
        };
        let dropped = |reason| PageEvalOutcome::Dropped { token, reason };
        if now >= read.deadline {
            return dropped(PageEvalDrop::Late);
        }
        let raw = match raw {
            RawArrival::OverCap { bytes } => {
                return dropped(PageEvalDrop::OverCap {
                    bytes,
                    limit: read.max_raw_bytes,
                });
            }
            RawArrival::Within(raw) if raw.len() > read.max_raw_bytes => {
                return dropped(PageEvalDrop::OverCap {
                    bytes: raw.len(),
                    limit: read.max_raw_bytes,
                });
            }
            RawArrival::Within(raw) => raw,
        };
        if read.epoch.current() != read.generation {
            return dropped(PageEvalDrop::Navigated);
        }
        if page_url().as_deref() != Some(read.url.as_str()) {
            return dropped(PageEvalDrop::Navigated);
        }
        PageEvalOutcome::Delivered {
            token,
            script: read.script,
            raw,
        }
    }

    /// A vista fechou ou quem le desistiu: tudo o que estava em voo passa a
    /// `Stale`.
    pub(in crate::windows_app) fn cancel_all(&mut self) -> usize {
        let cancelled = self.pending.len();
        self.pending.clear();
        cancelled
    }

    pub(in crate::windows_app) fn in_flight(&self) -> usize {
        self.pending.len()
    }

    fn take(&mut self, token: PageReadToken) -> Option<PendingRead> {
        let at = self.pending.iter().position(|read| read.token == token)?;
        Some(self.pending.swap_remove(at))
    }
}

/// O JSON de uma leitura entregue. O tecto confere-se outra vez antes do
/// serde: quem chama pode ter um mais apertado que o da leitura.
pub(in crate::windows_app) fn parse_page_json(
    raw: &str,
    max_raw_bytes: usize,
) -> Option<serde_json::Value> {
    if raw.len() > max_raw_bytes {
        return None;
    }
    serde_json::from_str(raw).ok()
}
