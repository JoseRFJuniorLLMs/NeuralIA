use super::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use neural_core::downloads::{
    DeleteReason, DownloadEntry, DownloadEvent, DownloadId, DownloadManager, DownloadNotice,
    DownloadState, RecordOutcome, file_name_of,
};
use neural_core::file_risk::{
    BlockReason, MACRO_EXTENSIONS, block_reason, default_app_name_allowed, default_app_target,
    display_label,
};

use crate::notify::{Notice, NoticeAction, NoticeKind, NoticeReply};

// ===================== a interface dos downloads (downloads-ui) =====================
//
// O que o gestor (`downloads.rs`, `neural_core::downloads`) decide chega a
// quem usa por aqui:
//
// - a seta do canto (`RIGHT_CLUSTER`, `BarHit::Downloads`) e o Ctrl+J
//   (`CommandId::Downloads`, ambito `Global`: a janela, a omnibox e cada
//   WebView menos o monitor do Gmail) abrem o painel do Ctrl+H na seccao
//   Downloads; de novo, fecham-no;
// - a seccao Downloads do painel (`assets/panel/downloads.*`): a lista da
//   sessao e do `downloads.json`, o progresso («Baixando relatorio.pdf ·
//   3,2 de 12 MB · 1 min») e «Permitir baixar programas». Os pedidos da
//   pagina (`downloads-*`) levam SO o numero da linha (`DownloadRows`) ou o
//   estado do interruptor: nunca um caminho nem um endereco;
// - «Abrir» so passa por `shell_open_checked`, que exige o
//   `DefaultAppTarget` do `neural_core::file_risk` (o nome e os primeiros
//   4 KiB do arquivo no disco, relidos no clique) e abre com o verbo "open";
//   um tipo recusado so mostra «Mostrar na pasta» (critica C15);
// - os avisos (bloqueado, apagado, concluido) sao `Notice`s do centro de
//   avisos (`crate::notify`, o toast do canto), do tipo `Download`;
// - dois cartoes nativos (`NativeCard`: token, armar de 600 ms, prazo, so o
//   pintado): «Baixar programa?», quando «Permitir baixar programas» esta
//   ligada, e a saida (Home ou fechar a janela) com downloads a correr
//   (`leave_decision`).

/// Quanto tempo o toast de um download fica no canto.
pub(in crate::windows_app) const DOWNLOAD_TOAST_TTL: Duration = Duration::from_secs(8);
/// O «Baixar programa?» sem resposta conta como Cancelar: o deferral nao
/// fica preso.
pub(in crate::windows_app) const PROGRAM_CARD_SECONDS: u64 = 30;
/// A pergunta da saida sem resposta: fica-se.
pub(in crate::windows_app) const LEAVE_CARD_SECONDS: u64 = 20;
/// O maior numero de linha que a pagina pode mandar (inteiro exato em JS).
pub(in crate::windows_app) const PANEL_ROW_ID_MAX: u64 = (1 << 53) - 1;

/// Mostra a seccao Downloads (o painel abre-se nela).
pub(in crate::windows_app) const PANEL_SHOW_DOWNLOADS_SCRIPT: &str =
    "window.neuraliaShowSection && window.neuraliaShowSection('downloads')";
/// A seta ou o Ctrl+J com o painel aberto: nos Downloads fecha (pelo X, que
/// salva o editor das notas antes), noutra seccao mostra os Downloads.
pub(in crate::windows_app) const PANEL_DOWNLOADS_BUTTON_SCRIPT: &str =
    "window.__neuraliaDownloads && window.__neuraliaDownloads.button()";

/// O que a feature recebe pelo event loop: uma variante `UserEvent`, o resto
/// aqui (o padrao do infra-seams).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum DownloadsUiEvent {
    /// Ctrl+J ou a seta da barra: abre a seccao; nela, fecha o painel.
    Show,
    /// Clique num botao do cartao. `token` e o do cartao que estava pintado
    /// quando o botao foi solto.
    CardAnswer {
        token: u64,
        button: DownloadCardButton,
    },
    /// O prazo do cartao `token` passou.
    CardExpired(u64),
}

// ===================== textos =====================

/// O que um nome bloqueado e, pela razao que o bloqueou.
fn what_it_is(reason: BlockReason) -> &'static str {
    match reason {
        BlockReason::Program => "um programa",
        BlockReason::Script => "um script",
        BlockReason::Shortcut => "um atalho do Windows",
        BlockReason::DiskImage => "uma imagem de disco",
        BlockReason::DatabaseApp => "uma base do Access",
        BlockReason::Masquerade => "um disfarce",
        BlockReason::BadName => "um nome inseguro",
    }
}

/// O tipo de fachada de um disfarce (`fatura.pdf.exe` -> `PDF`).
fn decoy_label(name: &str) -> Option<String> {
    let clean = name.trim_end_matches([' ', '.']);
    let (stem, _) = clean.rsplit_once('.')?;
    let (_, decoy) = stem.trim_end().rsplit_once('.')?;
    let decoy = decoy.trim();
    (!decoy.is_empty()).then(|| decoy.to_uppercase())
}

/// O que um disfarce e de verdade: a extensao final, pela mesma tabela do
/// `file_risk` (`fatura.pdf.exe` e um programa, `fatura.pdf.vbs` um
/// script, `fatura.pdf.docm` um documento com macros).
fn masquerade_truth(name: &str) -> &'static str {
    let clean = name.trim_end_matches([' ', '.']);
    let ext = clean
        .rsplit_once('.')
        .map(|(_, ext)| ext.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if MACRO_EXTENSIONS.contains(&ext.as_str()) {
        return "um documento com macros";
    }
    match block_reason(&format!("arquivo.{ext}")) {
        Some(
            reason @ (BlockReason::Program
            | BlockReason::Script
            | BlockReason::Shortcut
            | BlockReason::DiskImage
            | BlockReason::DatabaseApp),
        ) => what_it_is(reason),
        _ => "um programa",
    }
}

/// O titulo (escrito aqui, nunca com conteudo) e o corpo (o nome do
/// arquivo, passado por `display_label`: um bidi ou um invisivel aparece
/// marcado) do aviso de cada `DownloadNotice`.
pub(in crate::windows_app) fn download_notice_lines(
    notice: &DownloadNotice,
) -> (&'static str, String) {
    match notice {
        DownloadNotice::Blocked { name, reason, .. } => {
            let label = display_label(name);
            let body = match reason {
                BlockReason::Masquerade => {
                    let truth = masquerade_truth(name);
                    match decoy_label(name) {
                        Some(decoy) => format!("{label} finge ser um {decoy}, mas é {truth}."),
                        None => format!("{label} se disfarça de outro tipo, mas é {truth}."),
                    }
                }
                BlockReason::BadName => format!("O nome «{label}» não é seguro."),
                other => format!(
                    "{label} é {}. O NeuralIA não baixa programas nem scripts (ajuste em Downloads).",
                    what_it_is(*other)
                ),
            };
            ("Download bloqueado", body)
        }
        DownloadNotice::Deleted { name, reason, .. } => {
            let label = display_label(name);
            let body = match reason {
                DeleteReason::Unreadable => format!("Não deu para verificar {label}."),
                DeleteReason::DangerousContent | DeleteReason::BlockedName(_) => {
                    format!("{label} era um programa disfarçado.")
                }
            };
            ("Download apagado", body)
        }
        DownloadNotice::NotDeleted { name, .. } => (
            "Download perigoso",
            format!(
                "{} é perigoso e não deu para apagar. Não o abra.",
                display_label(name)
            ),
        ),
    }
}

/// O aviso numa linha so ("titulo — corpo"): o que o toast mostra em duas.
/// So os gates o leem assim (o texto do plano).
#[cfg(test)]
pub(in crate::windows_app) fn download_notice_text(notice: &DownloadNotice) -> String {
    let (title, body) = download_notice_lines(notice);
    format!("{title} — {body}")
}

/// O toast de um `DownloadNotice`: tipo `Download`, sem botoes, e o corpo
/// leva o nome do arquivo (`content_bearing`: o modo privado neutraliza-o).
pub(in crate::windows_app) fn download_toast(notice: &DownloadNotice) -> Notice {
    let (title, body) = download_notice_lines(notice);
    Notice {
        kind: NoticeKind::Download,
        title: title.to_string(),
        body,
        actions: Vec::new(),
        ttl: DOWNLOAD_TOAST_TTL,
        content_bearing: true,
    }
}

/// O botao «Ver» do toast de um download acabado: abre a seccao Downloads.
pub(in crate::windows_app) const DOWNLOAD_TOAST_SHOW: NoticeAction = NoticeAction {
    label: "Ver",
    width: 54.0,
    primary: true,
    reply: NoticeReply::Open,
};

/// O toast de um download que acabou e ficou no disco.
pub(in crate::windows_app) fn completed_toast(entry: &DownloadEntry) -> Notice {
    let label = display_label(&entry.name);
    Notice {
        kind: NoticeKind::Download,
        title: "Download concluído".to_string(),
        body: if entry.warn {
            format!("{label} · tem macros: o Office abre-o no Modo de Exibição Protegido")
        } else {
            label
        },
        actions: vec![DOWNLOAD_TOAST_SHOW],
        ttl: DOWNLOAD_TOAST_TTL,
        content_bearing: true,
    }
}

// ===================== o progresso =====================

/// A unidade de um tamanho: 1024 por degrau, como o Chrome. `None`: bytes.
fn size_unit(bytes: u64) -> Option<(f64, &'static str)> {
    const KB: u64 = 1024;
    if bytes >= KB * KB * KB {
        Some(((KB * KB * KB) as f64, "GB"))
    } else if bytes >= KB * KB {
        Some(((KB * KB) as f64, "MB"))
    } else if bytes >= KB {
        Some((KB as f64, "KB"))
    } else {
        None
    }
}

/// Um numero em pt-BR: uma casa decimal abaixo de 10 (sem o ",0"), inteiro
/// acima.
fn number_pt(value: f64) -> String {
    let one = format!("{value:.1}");
    if one.len() <= 3 {
        // "9,5" e "3,0": ate 9,9 leva a casa (o ",0" sai).
        return one
            .strip_suffix(".0")
            .map_or_else(|| one.replace('.', ","), str::to_string);
    }
    format!("{}", value.round() as u64)
}

/// "850 B", "3,2 KB", "12 MB", "1,4 GB".
pub(in crate::windows_app) fn format_size(bytes: u64) -> String {
    match size_unit(bytes) {
        Some((unit, name)) => format!("{} {name}", number_pt(bytes as f64 / unit)),
        None => format!("{bytes} B"),
    }
}

/// Quanto falta: "40 s", "1 min", "12 min", "1 h 5 min".
pub(in crate::windows_app) fn format_eta(eta: Duration) -> String {
    let secs = eta.as_secs();
    if secs < 60 {
        return format!("{} s", secs.max(1));
    }
    let minutes = (secs + 30) / 60;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    match (minutes / 60, minutes % 60) {
        (hours, 0) => format!("{hours} h"),
        (hours, rest) => format!("{hours} h {rest} min"),
    }
}

/// «Baixando relatorio.pdf · 3,2 de 12 MB · 1 min»: o recebido na unidade
/// do total; sem total, so o recebido; sem previsao, sem o fim.
pub(in crate::windows_app) fn progress_text(
    name: &str,
    received: u64,
    total: Option<u64>,
    eta: Option<Duration>,
) -> String {
    let label = display_label(name);
    let amount = match total.filter(|total| *total > 0) {
        Some(total) => match size_unit(total) {
            Some((unit, unit_name)) => format!(
                "{} de {} {unit_name}",
                number_pt(received.min(total) as f64 / unit),
                number_pt(total as f64 / unit)
            ),
            None => format!("{} de {total} B", received.min(total)),
        },
        None => format_size(received),
    };
    match eta {
        Some(eta) => format!("Baixando {label} · {amount} · {}", format_eta(eta)),
        None => format!("Baixando {label} · {amount}"),
    }
}

/// A percentagem de um download com total conhecido.
pub(in crate::windows_app) fn download_percent(received: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    Some((u128::from(received.min(total)) * 100 / u128::from(total)) as u8)
}

/// A velocidade de UM download, para a previsao: uma media que esquece
/// devagar (70% da anterior), com amostras de pelo menos 200 ms.
#[derive(Debug, Clone, Default)]
pub(in crate::windows_app) struct RateMeter {
    last: Option<(Instant, u64)>,
    /// Bytes por segundo.
    rate: Option<f64>,
}

impl RateMeter {
    pub(in crate::windows_app) fn observe(&mut self, now: Instant, received: u64) {
        let Some((then, bytes)) = self.last else {
            self.last = Some((now, received));
            return;
        };
        if received < bytes {
            // Recomecou (retomado do zero): outra medida.
            *self = Self {
                last: Some((now, received)),
                rate: None,
            };
            return;
        }
        let elapsed = now.saturating_duration_since(then).as_secs_f64();
        if elapsed < 0.2 {
            return;
        }
        let sample = (received - bytes) as f64 / elapsed;
        self.rate = Some(match self.rate {
            Some(rate) => rate * 0.7 + sample * 0.3,
            None => sample,
        });
        self.last = Some((now, received));
    }

    /// Quanto falta ao ritmo de agora (no maximo 100 horas); sem total ou
    /// sem velocidade, nada.
    pub(in crate::windows_app) fn eta(
        &self,
        received: u64,
        total: Option<u64>,
    ) -> Option<Duration> {
        let total = total.filter(|total| *total > 0)?;
        let rate = self.rate.filter(|rate| *rate >= 1.0)?;
        let left = total.saturating_sub(received) as f64;
        Some(Duration::from_secs_f64((left / rate).min(360_000.0)))
    }
}

/// A seta do canto: quantos downloads correm e a percentagem de todos os
/// que tem total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::windows_app) struct DownloadsBadge {
    pub(in crate::windows_app) active: u32,
    pub(in crate::windows_app) percent: Option<u8>,
}

pub(in crate::windows_app) fn downloads_badge(manager: &DownloadManager) -> DownloadsBadge {
    let mut active = 0u32;
    let (mut received, mut total) = (0u64, 0u64);
    for entry in manager.entries().filter(|entry| entry.is_active()) {
        active += 1;
        if let Some(size) = entry.total.filter(|size| *size > 0) {
            received = received.saturating_add(entry.received.min(size));
            total = total.saturating_add(size);
        }
    }
    DownloadsBadge {
        active,
        percent: (active > 0)
            .then(|| download_percent(received, Some(total)))
            .flatten(),
    }
}

/// A dica da seta: o atalho, e o que corre.
pub(in crate::windows_app) fn downloads_tooltip(badge: DownloadsBadge) -> String {
    match (badge.active, badge.percent) {
        (0, _) => "Downloads (Ctrl+J)".to_string(),
        (1, Some(percent)) => format!("Downloads (Ctrl+J) · 1 em andamento, {percent}%"),
        (1, None) => "Downloads (Ctrl+J) · 1 em andamento".to_string(),
        (count, Some(percent)) => {
            format!("Downloads (Ctrl+J) · {count} em andamento, {percent}%")
        }
        (count, None) => format!("Downloads (Ctrl+J) · {count} em andamento"),
    }
}

// ===================== a saida com downloads a correr =====================

/// Por onde a janela sai do que tem downloads: a Home (destroi as colunas,
/// a fonte ao lado, a Web completa e os paineis) ou fechar a NeuralIA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum LeaveKind {
    Home,
    Close,
}

/// A pergunta da saida: o que o cartao diz e o que o «Cancelar e sair»
/// cancela.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct LeavePrompt {
    pub(in crate::windows_app) kind: LeaveKind,
    pub(in crate::windows_app) title: String,
    pub(in crate::windows_app) body: String,
    pub(in crate::windows_app) cancel: Vec<DownloadId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum LeaveDecision {
    /// Nada a correr: sai ja.
    Leave,
    /// Pergunta primeiro (o cartao): «Continuar baixando» fica, «Cancelar e
    /// sair» cancela e sai.
    Ask(LeavePrompt),
}

/// A tabela da saida. A Home e o fechar destroem todas as WebViews que
/// descarregam (o gestor cancela os downloads delas no `WebViewGone`), por
/// isso cada download a correr ou a espera conta. O cartao nomeia o mais
/// antigo: «1 download em andamento (x.zip, 43%).».
pub(in crate::windows_app) fn leave_decision(
    kind: LeaveKind,
    manager: &DownloadManager,
) -> LeaveDecision {
    let active: Vec<&DownloadEntry> = manager.entries().filter(|e| e.is_active()).collect();
    let Some(first) = active.first() else {
        return LeaveDecision::Leave;
    };
    let count = active.len();
    let what = match (first.state, download_percent(first.received, first.total)) {
        (DownloadState::Running, Some(percent)) => {
            format!("{}, {percent}%", display_label(&first.name))
        }
        _ => display_label(&first.name),
    };
    let title = if count == 1 {
        format!("1 download em andamento ({what}).")
    } else {
        format!(
            "{count} downloads em andamento ({what} e mais {}).",
            count - 1
        )
    };
    let them = if count == 1 {
        "o download"
    } else {
        "os downloads"
    };
    let body = match kind {
        LeaveKind::Home => format!("Voltar à Home cancela {them}."),
        LeaveKind::Close => format!("Fechar a NeuralIA cancela {them}."),
    };
    LeaveDecision::Ask(LeavePrompt {
        kind,
        title,
        body,
        cancel: active.iter().map(|entry| entry.id).collect(),
    })
}

// ===================== o cartao =====================

/// O que um botao do cartao faz: `Confirm` e o que avanca e arrisca
/// («Baixar mesmo assim», «Cancelar e sair», so depois do armar);
/// `Dismiss` e o seguro («Cancelar», «Continuar baixando», ja).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum DownloadCardButton {
    Confirm,
    Dismiss,
}

/// O pedido de um cartao.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum DownloadPrompt {
    /// «Baixar programa?» de um download preso no deferral.
    Program {
        id: DownloadId,
        name: String,
        reason: BlockReason,
    },
    /// Sair com downloads a correr.
    Leave(LeavePrompt),
}

/// Um botao pintado, pela ordem da esquerda para a direita.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct CardButtonView {
    pub(in crate::windows_app) label: &'static str,
    pub(in crate::windows_app) role: DownloadCardButton,
    /// Cheio, na cor de destaque: sempre o seguro.
    pub(in crate::windows_app) primary: bool,
}

/// O que o cartao pinta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct DownloadCardView {
    pub(in crate::windows_app) token: u64,
    pub(in crate::windows_app) title: String,
    pub(in crate::windows_app) body: String,
    pub(in crate::windows_app) buttons: [CardButtonView; 2],
}

impl DownloadPrompt {
    pub(in crate::windows_app) fn view(&self, token: u64) -> DownloadCardView {
        match self {
            DownloadPrompt::Program { name, reason, .. } => DownloadCardView {
                token,
                title: "Baixar programa?".to_string(),
                body: format!(
                    "{} é {}. Um programa baixado da internet pode danificar o computador: baixe só se confia em quem o enviou.",
                    display_label(name),
                    what_it_is(*reason)
                ),
                buttons: [
                    CardButtonView {
                        label: "Baixar mesmo assim",
                        role: DownloadCardButton::Confirm,
                        primary: false,
                    },
                    CardButtonView {
                        label: "Cancelar",
                        role: DownloadCardButton::Dismiss,
                        primary: true,
                    },
                ],
            },
            DownloadPrompt::Leave(prompt) => DownloadCardView {
                token,
                title: prompt.title.clone(),
                body: prompt.body.clone(),
                buttons: [
                    CardButtonView {
                        label: "Continuar baixando",
                        role: DownloadCardButton::Dismiss,
                        primary: true,
                    },
                    CardButtonView {
                        label: "Cancelar e sair",
                        role: DownloadCardButton::Confirm,
                        primary: false,
                    },
                ],
            },
        }
    }

    /// Quanto o cartao espera por uma resposta.
    pub(in crate::windows_app) fn expiry(&self) -> Duration {
        Duration::from_secs(match self {
            DownloadPrompt::Program { .. } => PROGRAM_CARD_SECONDS,
            DownloadPrompt::Leave(_) => LEAVE_CARD_SECONDS,
        })
    }
}

/// O que entra no cartao.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum DownloadCardInput {
    Request(DownloadPrompt),
    Answer {
        token: u64,
        button: DownloadCardButton,
    },
    Expire(u64),
}

/// O que uma volta do cartao pede ao `App`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::windows_app) struct CardStep {
    /// Pintar este cartao (e agendar o fim dele).
    pub(in crate::windows_app) show: Option<(DownloadCardView, Duration)>,
    /// Tirar o cartao do ecra.
    pub(in crate::windows_app) hide: bool,
    /// Os eventos que seguem para o gestor, pela ordem: a resposta ao
    /// «Baixar programa?» (tambem a de um pedido trocado por outro, que
    /// nunca fica preso no deferral) e os cancelamentos da saida.
    pub(in crate::windows_app) events: Vec<DownloadEvent>,
    /// Sair agora (o «Cancelar e sair» armado).
    pub(in crate::windows_app) leave: Option<LeaveKind>,
}

/// A resposta que um pedido deixado sem sim da ao gestor: um programa que
/// ninguem confirmou e recusado; uma saida sem resposta fica.
fn unanswered(prompt: DownloadPrompt) -> Vec<DownloadEvent> {
    match prompt {
        DownloadPrompt::Program { id, .. } => vec![DownloadEvent::Answered { id, allow: false }],
        DownloadPrompt::Leave(_) => Vec::new(),
    }
}

/// Uma volta do cartao, pura: o `NativeCard` decide (token, armar, prazo,
/// um de cada vez) e a volta diz o que isso faz aos downloads.
pub(in crate::windows_app) fn download_card_step(
    card: &mut NativeCard<DownloadPrompt>,
    input: DownloadCardInput,
    now: Instant,
) -> CardStep {
    let mut step = CardStep::default();
    match input {
        DownloadCardInput::Request(prompt) => {
            // O pedido trocado responde-se ja: um deferral nunca fica preso.
            if let Some(previous) = card.pending.take() {
                step.events = unanswered(previous.payload);
            }
            let expiry = prompt.expiry();
            let view_of = prompt.clone();
            let (token, _) = card.request(prompt, now);
            step.show = Some((view_of.view(token), expiry));
        }
        DownloadCardInput::Answer { token, button } => {
            let pending = card
                .pending
                .as_ref()
                .filter(|pending| pending.token == token)
                .map(|pending| pending.payload.clone());
            let confirm = button == DownloadCardButton::Confirm;
            match card.answer(token, confirm, now, |payload| Some(payload.clone())) {
                CardAnswer::Confirmed(prompt) => {
                    step.hide = true;
                    match prompt {
                        DownloadPrompt::Program { id, .. } => {
                            step.events
                                .push(DownloadEvent::Answered { id, allow: true });
                        }
                        DownloadPrompt::Leave(leave) => {
                            step.events.extend(
                                leave
                                    .cancel
                                    .iter()
                                    .map(|id| DownloadEvent::CancelRequested { id: *id }),
                            );
                            step.leave = Some(leave.kind);
                        }
                    }
                }
                CardAnswer::Cancelled => {
                    step.hide = true;
                    if let Some(prompt) = pending {
                        step.events = unanswered(prompt);
                    }
                }
                CardAnswer::Ignored => {}
            }
        }
        DownloadCardInput::Expire(token) => {
            let pending = card
                .pending
                .as_ref()
                .filter(|pending| pending.token == token)
                .map(|pending| pending.payload.clone());
            if card.expire(token) {
                step.hide = true;
                if let Some(prompt) = pending {
                    step.events = unanswered(prompt);
                }
            }
        }
    }
    step
}

pub(in crate::windows_app) const DOWNLOAD_CARD_SUBCLASS_ID: usize = 0x4E44;
/// O cartao, em pixeis logicos, centrado na janela.
pub(in crate::windows_app) const DOWNLOAD_CARD_WIDTH: f64 = 460.0;
pub(in crate::windows_app) const DOWNLOAD_CARD_HEIGHT: f64 = 188.0;

/// Para onde o cartao manda o clique: o proxy do event loop no app, um
/// registo nos gates. Em caixa dupla: o `reference_data` da subclasse e um
/// ponteiro fino.
pub(in crate::windows_app) type DownloadCardSink = Box<dyn Fn(UserEvent)>;

/// O cartao a pintar. Fora do `App` porque quem pinta e o procedimento de
/// janela.
pub(in crate::windows_app) static DOWNLOAD_CARD_VIEW: Mutex<Option<DownloadCardView>> =
    Mutex::new(None);
/// O token do cartao que a ultima pintura mostrou: um clique leva ESTE, e
/// nao o de um pedido que o trocou e ainda nao foi pintado.
pub(in crate::windows_app) static DOWNLOAD_CARD_PAINTED: AtomicU64 = AtomicU64::new(0);
/// O botao onde o rato desceu (indice na ordem pintada).
pub(in crate::windows_app) static DOWNLOAD_CARD_PRESSED: AtomicUsize =
    AtomicUsize::new(NATIVE_BUTTON_NONE);

/// Onde fica cada coisa no cartao, em pixeis do cliente: uma so funcao para
/// o desenho e o clique concordarem.
pub(in crate::windows_app) struct DownloadCardLayout {
    pub(in crate::windows_app) title: RECT,
    pub(in crate::windows_app) body: RECT,
    pub(in crate::windows_app) buttons: [RECT; 2],
}

pub(in crate::windows_app) fn download_card_scale(client: &RECT) -> f64 {
    ((client.bottom - client.top) as f64 / DOWNLOAD_CARD_HEIGHT).max(1.0)
}

/// A largura logica de um botao, pelo rotulo.
fn card_button_width(label: &str) -> f64 {
    (label.chars().count() as f64 * 8.0 + 36.0).max(110.0)
}

pub(in crate::windows_app) fn download_card_layout(
    client: &RECT,
    scale: f64,
    labels: [&str; 2],
) -> DownloadCardLayout {
    let px = |value: f64| (value * scale).round() as i32;
    let pad = px(24.0);
    let bottom = client.bottom - px(18.0);
    let top = bottom - px(36.0);
    let mut right = client.right - pad;
    let mut buttons = [RECT::default(); 2];
    for index in (0..2).rev() {
        let width = px(card_button_width(labels[index]));
        buttons[index] = RECT {
            left: right - width,
            top,
            right,
            bottom,
        };
        right = buttons[index].left - px(12.0);
    }
    let title = RECT {
        left: client.left + pad,
        top: client.top + px(18.0),
        right: client.right - pad,
        bottom: client.top + px(50.0),
    };
    let body = RECT {
        left: title.left,
        top: title.bottom + px(4.0),
        right: title.right,
        bottom: top - px(10.0),
    };
    DownloadCardLayout {
        title,
        body,
        buttons,
    }
}

/// O botao debaixo de (x, y), pela ordem pintada; bordas semiabertas.
pub(in crate::windows_app) fn download_card_hit(
    client: &RECT,
    labels: [&str; 2],
    x: i32,
    y: i32,
) -> Option<usize> {
    let layout = download_card_layout(client, download_card_scale(client), labels);
    layout
        .buttons
        .iter()
        .position(|rect| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
}

fn card_labels(view: &DownloadCardView) -> [&'static str; 2] {
    [view.buttons[0].label, view.buttons[1].label]
}

/// O cartao dos downloads: nativo, owned pela janela principal, nunca
/// ativo (`create_native_card`, `popup_no_activate_message`). Um clique so
/// conta se desceu e subiu no mesmo botao com o rato preso ao cartao.
pub(in crate::windows_app) unsafe extern "system" fn download_card_subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    if let Some(result) = popup_no_activate_message(message) {
        return result;
    }
    let view = || DOWNLOAD_CARD_VIEW.lock().ok().and_then(|view| view.clone());
    match message {
        WM_LBUTTONDOWN => {
            let mut client = RECT::default();
            if GetClientRect(hwnd, &mut client) != 0
                && let Some(view) = view()
            {
                let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
                let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
                if let Some(index) = download_card_hit(&client, card_labels(&view), x, y) {
                    DOWNLOAD_CARD_PRESSED.store(index, Ordering::Release);
                    SetCapture(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let captured = GetCapture() == hwnd;
            let pressed = take_native_pressed_button(&DOWNLOAD_CARD_PRESSED, || {
                if captured {
                    ReleaseCapture();
                }
            });
            let mut client = RECT::default();
            if !captured || GetClientRect(hwnd, &mut client) == 0 || reference_data == 0 {
                return 0;
            }
            let Some(view) = view() else {
                return 0;
            };
            let x = (lparam as u32 & 0xffff) as u16 as i16 as i32;
            let y = ((lparam as u32 >> 16) & 0xffff) as u16 as i16 as i32;
            let released = download_card_hit(&client, card_labels(&view), x, y);
            if let Some(index) = native_release_matches(pressed, released) {
                let sink = &*(reference_data as *const DownloadCardSink);
                sink(UserEvent::DownloadsUi(DownloadsUiEvent::CardAnswer {
                    token: DOWNLOAD_CARD_PAINTED.load(Ordering::Acquire),
                    button: view.buttons[index].role,
                }));
            }
            0
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            DOWNLOAD_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
            0
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            if !hdc.is_null() {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client) != 0
                    && let Some(view) = view()
                {
                    paint_download_card(hdc, &client, &view);
                    // So agora quem usa ve este cartao: e ele que um clique
                    // a seguir responde.
                    DOWNLOAD_CARD_PAINTED.store(view.token, Ordering::Release);
                }
                EndPaint(hwnd, &paint);
            }
            0
        }
        _ => DefSubclassProc(hwnd, message, wparam, lparam),
    }
}

/// O titulo na cor de destaque, o texto em varias linhas e os dois botoes
/// (o seguro cheio).
unsafe fn paint_download_card(hdc: *mut core::ffi::c_void, client: &RECT, view: &DownloadCardView) {
    let theme = Theme::system();
    let background = CreateSolidBrush(rgb3(theme.surface));
    FillRect(hdc, client, background);
    DeleteObject(background as _);

    let scale = download_card_scale(client);
    let layout = download_card_layout(client, scale, card_labels(view));
    let title_font = create_font((-18.0 * scale) as i32, FW_BOLD as i32);
    let body_font = create_font((-14.0 * scale) as i32, FW_NORMAL as i32);
    let button_font = create_font((-14.0 * scale) as i32, FW_BOLD as i32);
    let old_font = SelectObject(hdc, title_font as _);
    SetBkMode(hdc, TRANSPARENT as i32);

    SetTextColor(hdc, rgb3(theme.accent));
    let mut title = layout.title;
    draw_text(
        hdc,
        &view.title,
        &mut title,
        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    SelectObject(hdc, body_font as _);
    SetTextColor(hdc, rgb3(theme.fg));
    let mut body = layout.body;
    draw_text(
        hdc,
        &view.body,
        &mut body,
        DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX,
    );

    for (rect, button) in layout.buttons.iter().zip(view.buttons) {
        let pill = UiRect {
            x: rect.left as f64,
            y: rect.top as f64,
            width: (rect.right - rect.left) as f64,
            height: (rect.bottom - rect.top) as f64,
        };
        let style = if button.primary {
            PillStyle::new(theme.accent, theme.accent, on_color(theme.accent))
        } else {
            PillStyle::new(theme.surface_line, theme.surface_line, theme.fg)
        };
        draw_pill(
            hdc,
            pill,
            button.label,
            style,
            scale,
            button_font,
            theme.surface,
        );
    }

    SelectObject(hdc, old_font);
    DeleteObject(title_font as _);
    DeleteObject(body_font as _);
    DeleteObject(button_font as _);
}

// ===================== a seccao Downloads do painel =====================

/// O que a pagina da seccao Downloads pode pedir. Lista fechada; uma linha
/// e so o seu numero (`DownloadRows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum DownloadsPanelRequest {
    /// A seccao abriu: a lista, ja.
    List,
    Open(u64),
    /// «Mostrar na pasta».
    Show(u64),
    Cancel(u64),
    /// «Permitir baixar programas».
    AllowPrograms(bool),
}

/// O numero de uma linha: `args` so com `id`, um inteiro de 1 a 2^53-1. Um
/// texto (um caminho, um endereco), um numero com casas, negativo ou
/// zero morre aqui.
fn panel_row_id(args: Option<&serde_json::Value>) -> Option<u64> {
    exact_keys(args, &["id"])?
        .get("id")?
        .as_u64()
        .filter(|id| (1..=PANEL_ROW_ID_MAX).contains(id))
}

/// O parser da seccao Downloads (`PANEL_SECTIONS`, prefixo `downloads`).
pub(in crate::windows_app) fn parse_downloads_action(
    action: &str,
    args: Option<&serde_json::Value>,
) -> Option<PanelMessage> {
    let request = match action {
        "downloads-list" => exact_keys(args, &[]).map(|_| DownloadsPanelRequest::List),
        "downloads-open" => panel_row_id(args).map(DownloadsPanelRequest::Open),
        "downloads-show" => panel_row_id(args).map(DownloadsPanelRequest::Show),
        "downloads-cancel" => panel_row_id(args).map(DownloadsPanelRequest::Cancel),
        "downloads-allow-programs" => exact_keys(args, &["on"])?
            .get("on")?
            .as_bool()
            .map(DownloadsPanelRequest::AllowPrograms),
        _ => None,
    }?;
    Some(PanelMessage::Downloads(request))
}

/// De onde vem uma linha: um download desta sessao, ou um registo do
/// `downloads.json` de antes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum RowTarget {
    Session(DownloadId),
    Record(RecordKey),
}

/// Um registo pelo que ele e: o mesmo registo da sempre o mesmo numero.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::windows_app) struct RecordKey {
    pub(in crate::windows_app) name: String,
    pub(in crate::windows_app) path: Option<PathBuf>,
    pub(in crate::windows_app) at: u64,
    pub(in crate::windows_app) outcome: RecordOutcome,
}

/// Uma linha como a pagina a recebe: so texto e booleanos, nunca um
/// caminho nem um endereco (o anfitriao entra no texto).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct PanelRow {
    pub(in crate::windows_app) id: u64,
    pub(in crate::windows_app) name: String,
    pub(in crate::windows_app) status: String,
    pub(in crate::windows_app) percent: Option<u8>,
    /// «Abrir»: so um arquivo acabado cujo tipo o `DefaultAppTarget` aceita.
    pub(in crate::windows_app) open: bool,
    /// «Mostrar na pasta»: um arquivo que ficou no disco.
    pub(in crate::windows_app) show: bool,
    pub(in crate::windows_app) cancel: bool,
    /// running, done, warn, blocked.
    pub(in crate::windows_app) tone: &'static str,
}

/// Os numeros das linhas. Um download da sessao tem sempre o mesmo; um
/// registo tambem (pelo que ele e). Um numero que ja nao esta na lista
/// mostrada nao resolve nada.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct DownloadRows {
    next: u64,
    session: BTreeMap<DownloadId, u64>,
    records: HashMap<RecordKey, u64>,
    shown: BTreeMap<u64, RowTarget>,
}

/// O texto do estado de um desfecho.
fn outcome_status(outcome: RecordOutcome, bytes: Option<u64>, host: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    match outcome {
        RecordOutcome::Completed { warn } => {
            parts.push("Concluído".to_string());
            if let Some(bytes) = bytes {
                parts.push(format_size(bytes));
            }
            if let Some(host) = host {
                parts.push(host.to_string());
            }
            if warn {
                parts.push("documento com macros".to_string());
            }
            return parts.join(" · ");
        }
        RecordOutcome::Blocked { reason } => {
            parts.push(format!("Bloqueado — é {}", what_it_is(reason)));
        }
        RecordOutcome::Deleted { reason } => parts.push(match reason {
            DeleteReason::Unreadable => "Apagado — não deu para verificar".to_string(),
            DeleteReason::DangerousContent | DeleteReason::BlockedName(_) => {
                "Apagado — era um programa disfarçado".to_string()
            }
        }),
        RecordOutcome::NotDeleted { .. } => {
            parts.push("Perigoso — não deu para apagar. Não o abra.".to_string())
        }
        RecordOutcome::Cancelled => parts.push("Cancelado".to_string()),
        RecordOutcome::Interrupted => parts.push("Interrompido".to_string()),
    }
    if let Some(host) = host {
        parts.push(host.to_string());
    }
    parts.join(" · ")
}

fn outcome_tone(outcome: RecordOutcome) -> &'static str {
    match outcome {
        RecordOutcome::Completed { warn: false } => "done",
        RecordOutcome::Completed { warn: true } | RecordOutcome::NotDeleted { .. } => "warn",
        RecordOutcome::Blocked { .. } | RecordOutcome::Deleted { .. } => "blocked",
        RecordOutcome::Cancelled | RecordOutcome::Interrupted => "done",
    }
}

/// O arquivo de um desfecho, se ele ficou no disco, e as accoes que ele
/// admite: «Abrir» pelo nome no disco (`default_app_name_allowed`, a mesma
/// regra do `DefaultAppTarget`), «Mostrar na pasta» sempre que ficou.
fn file_actions(outcome: RecordOutcome, path: Option<&Path>) -> (bool, bool) {
    let Some(path) = path else {
        return (false, false);
    };
    match outcome {
        RecordOutcome::Completed { .. } => (default_app_name_allowed(file_name_of(path)), true),
        RecordOutcome::NotDeleted { .. } => (false, true),
        _ => (false, false),
    }
}

impl DownloadRows {
    fn id_for_session(&mut self, id: DownloadId) -> u64 {
        let next = &mut self.next;
        *self.session.entry(id).or_insert_with(|| {
            *next += 1;
            *next
        })
    }

    fn id_for_record(&mut self, key: &RecordKey) -> u64 {
        if let Some(id) = self.records.get(key) {
            return *id;
        }
        self.next += 1;
        self.records.insert(key.clone(), self.next);
        self.next
    }

    /// A linha `id` da ultima lista mostrada.
    pub(in crate::windows_app) fn target(&self, id: u64) -> Option<&RowTarget> {
        self.shown.get(&id)
    }

    /// A lista: os downloads da sessao (os mais novos primeiro) e depois os
    /// registos de antes -- sem os desta sessao, que o gestor ja pos no
    /// registo. A previsao de cada download a correr vem de `meters`.
    pub(in crate::windows_app) fn list(
        &mut self,
        manager: &DownloadManager,
        meters: &BTreeMap<DownloadId, RateMeter>,
    ) -> Vec<PanelRow> {
        let mut rows = Vec::new();
        let mut shown = BTreeMap::new();
        let mut in_session: HashMap<(String, u64), usize> = HashMap::new();
        // `entries` vem pelo numero, do mais antigo para o mais novo.
        let session: Vec<&DownloadEntry> = manager.entries().collect();
        for entry in session.into_iter().rev() {
            let id = self.id_for_session(entry.id);
            *in_session
                .entry((entry.name.clone(), entry.at))
                .or_default() += 1;
            shown.insert(id, RowTarget::Session(entry.id));
            rows.push(self.session_row(id, entry, meters.get(&entry.id)));
        }
        let mut kept = HashMap::new();
        for record in manager.log().entries {
            let pair = (record.name.clone(), record.at);
            if let Some(count) = in_session.get_mut(&pair).filter(|count| **count > 0) {
                *count -= 1;
                continue;
            }
            let key = RecordKey {
                name: record.name.clone(),
                path: record.path.clone(),
                at: record.at,
                outcome: record.outcome,
            };
            let id = self.id_for_record(&key);
            let (open, show) = file_actions(record.outcome, record.path.as_deref());
            rows.push(PanelRow {
                id,
                name: display_label(&record.name),
                status: outcome_status(record.outcome, record.bytes, record.host.as_deref()),
                percent: None,
                open,
                show,
                cancel: false,
                tone: outcome_tone(record.outcome),
            });
            shown.insert(id, RowTarget::Record(key.clone()));
            kept.insert(key, id);
        }
        // So ficam os numeros dos registos que ainda existem: o que o
        // Ctrl+Shift+Delete apagou deixa de ter numero (e nome) em memoria.
        self.records = kept;
        self.shown = shown;
        rows
    }

    fn session_row(&self, id: u64, entry: &DownloadEntry, meter: Option<&RateMeter>) -> PanelRow {
        let private = if entry.private { " · privado" } else { "" };
        let (status, tone, percent) = match entry.state {
            DownloadState::Running => (
                progress_text(
                    &entry.name,
                    entry.received,
                    entry.total,
                    meter.and_then(|meter| meter.eta(entry.received, entry.total)),
                ),
                "running",
                download_percent(entry.received, entry.total),
            ),
            DownloadState::Asking(reason) => (
                format!("Esperando a sua resposta — é {}", what_it_is(reason)),
                "warn",
                None,
            ),
            DownloadState::Finalizing => ("Verificando o arquivo…".to_string(), "running", None),
            DownloadState::Done(outcome) => (
                outcome_status(
                    outcome,
                    matches!(
                        outcome,
                        RecordOutcome::Completed { .. } | RecordOutcome::NotDeleted { .. }
                    )
                    .then_some(entry.received),
                    entry.host.as_deref(),
                ),
                outcome_tone(outcome),
                None,
            ),
        };
        let (open, show) = match entry.state {
            DownloadState::Done(outcome) => file_actions(outcome, Some(&entry.path)),
            _ => (false, false),
        };
        PanelRow {
            id,
            name: display_label(&entry.name),
            status: format!("{status}{private}"),
            percent,
            open,
            show,
            cancel: matches!(
                entry.state,
                DownloadState::Running | DownloadState::Asking(_)
            ),
            tone,
        }
    }
}

/// O JS que preenche a seccao. Os dados vao como JSON e a pagina so os usa
/// com `textContent`.
pub(in crate::windows_app) fn downloads_render_script(
    rows: &[PanelRow],
    allow_programs: bool,
) -> String {
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "name": row.name,
                "status": row.status,
                "percent": row.percent,
                "open": row.open,
                "show": row.show,
                "cancel": row.cancel,
                "tone": row.tone,
            })
        })
        .collect();
    let data = serde_json::json!({ "rows": rows, "allowPrograms": allow_programs });
    format!("window.__neuraliaDownloads && window.__neuraliaDownloads.render({data});")
}

/// O arquivo de uma linha, com o desfecho: so o de um download acabado (ou
/// que devia ter sido apagado) que ficou no disco.
pub(in crate::windows_app) fn row_file(
    manager: &DownloadManager,
    rows: &DownloadRows,
    id: u64,
) -> Option<(PathBuf, RecordOutcome)> {
    match rows.target(id)? {
        RowTarget::Session(download) => {
            let entry = manager.entry(*download)?;
            match entry.state {
                DownloadState::Done(
                    outcome @ (RecordOutcome::Completed { .. } | RecordOutcome::NotDeleted { .. }),
                ) => Some((entry.path.clone(), outcome)),
                _ => None,
            }
        }
        RowTarget::Record(key) => Some((key.path.clone()?, key.outcome)),
    }
}

// ===================== abrir e mostrar na pasta =====================

/// O verbo do `ShellExecuteW`: so este -- nunca o que pede elevacao
/// (gate de ausencia em `abrir_goes_only_through_default_app_target`).
pub(in crate::windows_app) const SHELL_OPEN_VERB: &str = "open";

/// O shell do Windows, como os downloads o usam: o produto passa o Win32
/// (`WindowsShell`); os gates um registo.
pub(in crate::windows_app) trait ShellHost {
    /// O `ShellExecuteW` com `verb` sobre `file`. So `shell_open_checked` o
    /// chama.
    fn shell_execute(&mut self, verb: &'static str, file: &Path) -> Result<(), String>;
    /// «Mostrar na pasta»: o Explorador na pasta, com o arquivo escolhido.
    /// Nao corre nada.
    fn reveal_in_folder(&mut self, file: &Path) -> Result<(), String>;
}

/// O que um «Abrir» deu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum OpenOutcome {
    Opened,
    /// O `DefaultAppTarget` recusou (o tipo, ou o conteudo no disco).
    Refused,
    /// A linha ja nao existe, ou o arquivo nao ficou no disco.
    NoFile,
    Failed(String),
}

/// A unica porta para o `ShellExecuteW`: o arquivo passa pelo
/// `DefaultAppTarget` -- o nome e os primeiros 4 KiB do arquivo, relidos
/// agora -- e abre com "open" o caminho que ele validou. Um programa, um
/// script, um documento com macros, um disfarce ou um atalho nunca chegam
/// ao shell (critica C15).
pub(in crate::windows_app) fn shell_open_checked(
    path: &Path,
    shell: &mut impl ShellHost,
) -> OpenOutcome {
    let Some(target) = default_app_target(path) else {
        return OpenOutcome::Refused;
    };
    match shell.shell_execute(SHELL_OPEN_VERB, target.path()) {
        Ok(()) => OpenOutcome::Opened,
        Err(error) => OpenOutcome::Failed(error),
    }
}

/// O «Abrir» de uma linha: so um download acabado, e so pelo
/// `shell_open_checked`.
pub(in crate::windows_app) fn open_download_row(
    manager: &DownloadManager,
    rows: &DownloadRows,
    id: u64,
    shell: &mut impl ShellHost,
) -> OpenOutcome {
    match row_file(manager, rows, id) {
        // Apagado ou movido depois de acabar: nao e uma recusa.
        Some((path, RecordOutcome::Completed { .. })) if !path.is_file() => OpenOutcome::NoFile,
        Some((path, RecordOutcome::Completed { .. })) => shell_open_checked(&path, shell),
        Some(_) => OpenOutcome::Refused,
        None => OpenOutcome::NoFile,
    }
}

/// O «Mostrar na pasta» de uma linha.
pub(in crate::windows_app) fn show_download_row(
    manager: &DownloadManager,
    rows: &DownloadRows,
    id: u64,
    shell: &mut impl ShellHost,
) -> OpenOutcome {
    match row_file(manager, rows, id) {
        Some((path, _)) => match shell.reveal_in_folder(&path) {
            Ok(()) => OpenOutcome::Opened,
            Err(error) => OpenOutcome::Failed(error),
        },
        None => OpenOutcome::NoFile,
    }
}

/// O shell do produto.
pub(in crate::windows_app) struct WindowsShell {
    pub(in crate::windows_app) owner: HWND,
}

impl ShellHost for WindowsShell {
    fn shell_execute(&mut self, verb: &'static str, file: &Path) -> Result<(), String> {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let verb = wide_null(verb);
        let file_w = wide_null(&file.to_string_lossy());
        let dir = file.parent().map(|dir| wide_null(&dir.to_string_lossy()));
        let result = unsafe {
            ShellExecuteW(
                self.owner,
                verb.as_ptr(),
                file_w.as_ptr(),
                std::ptr::null(),
                dir.as_ref().map_or(std::ptr::null(), |dir| dir.as_ptr()),
                SW_SHOWNORMAL,
            )
        };
        // Acima de 32 e sucesso (o contrato do ShellExecute).
        if result as isize > 32 {
            Ok(())
        } else {
            Err(format!("ShellExecute devolveu {}", result as isize))
        }
    }

    fn reveal_in_folder(&mut self, file: &Path) -> Result<(), String> {
        use windows_sys::Win32::UI::Shell::{
            ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems,
        };
        if !file.exists() {
            return Err("o arquivo já não está lá".to_string());
        }
        let path = wide_null(&file.to_string_lossy());
        unsafe {
            let item = ILCreateFromPathW(path.as_ptr());
            if item.is_null() {
                return Err("o Windows não encontrou o arquivo".to_string());
            }
            let hr = SHOpenFolderAndSelectItems(item, 0, std::ptr::null(), 0);
            ILFree(item);
            if hr < 0 {
                return Err(format!("o Explorador recusou (0x{:08X})", hr as u32));
            }
        }
        Ok(())
    }
}

// ===================== o estado e o App =====================

impl DownloadsState {
    /// «Permitir baixar programas»: grava so a escolha em
    /// `downloads-settings.json` (debaixo do trinco da loja: a pasta que la
    /// esta fica como esta) e, gravada, o gestor passa a usa-la. Sem loja,
    /// ou com a gravacao recusada, nada muda.
    pub(in crate::windows_app) fn set_allow_programs(&mut self, on: bool) -> Result<(), String> {
        let store = self
            .settings_store
            .as_mut()
            .ok_or_else(|| "sem a pasta de dados".to_string())?;
        store
            .update(|settings| settings.allow_programs = on)
            .map_err(|error| error.to_string())?;
        let mut next = self.manager.settings().clone();
        next.allow_programs = on;
        self.manager.on_event(DownloadEvent::SettingsChanged(next));
        Ok(())
    }
}

/// O estado da feature no `App`.
pub(in crate::windows_app) struct DownloadsUiState {
    pub(in crate::windows_app) rows: DownloadRows,
    pub(in crate::windows_app) meters: BTreeMap<DownloadId, RateMeter>,
    pub(in crate::windows_app) card: NativeCard<DownloadPrompt>,
    pub(in crate::windows_app) card_popup: Option<HWND>,
    pub(in crate::windows_app) card_sink: Box<DownloadCardSink>,
    /// A pagina do painel pediu a lista (a seccao Downloads ja se abriu
    /// nela): as mudancas seguem para la enquanto o painel estiver aberto.
    pub(in crate::windows_app) panel_live: bool,
    /// Os downloads acabados que ja deram o toast.
    pub(in crate::windows_app) announced: BTreeSet<DownloadId>,
    /// A ultima seta pintada: so se repinta a barra quando ela muda.
    pub(in crate::windows_app) badge: DownloadsBadge,
}

impl DownloadsUiState {
    pub(in crate::windows_app) fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            rows: DownloadRows::default(),
            meters: BTreeMap::new(),
            card: NativeCard::default(),
            card_popup: None,
            card_sink: Box::new(Box::new(move |event| {
                let _ = proxy.send_event(event);
            })),
            panel_live: false,
            announced: BTreeSet::new(),
            badge: DownloadsBadge::default(),
        }
    }
}

/// O que um download que mudou pede a interface, sem janela: o toast de
/// um acabado (uma vez so) e a medida da velocidade de um que corre.
pub(in crate::windows_app) fn download_changed(
    ui: &mut DownloadsUiState,
    entry: &DownloadEntry,
    now: Instant,
) -> Option<Notice> {
    match entry.state {
        DownloadState::Running => {
            ui.meters
                .entry(entry.id)
                .or_default()
                .observe(now, entry.received);
            None
        }
        DownloadState::Asking(_) | DownloadState::Finalizing => None,
        DownloadState::Done(outcome) => {
            ui.meters.remove(&entry.id);
            let completed = matches!(outcome, RecordOutcome::Completed { .. });
            (completed && ui.announced.insert(entry.id)).then(|| completed_toast(entry))
        }
    }
}

impl App {
    /// O braco `UserEvent::DownloadsUi`. Quando o «Cancelar e sair» armado
    /// foi confirmado, a janela sai ja por onde o cartao disse
    /// (`leave_confirmed`).
    pub(in crate::windows_app) fn downloads_ui_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: DownloadsUiEvent,
    ) {
        let leave = match event {
            DownloadsUiEvent::Show => {
                self.toggle_downloads_panel();
                None
            }
            DownloadsUiEvent::CardAnswer { token, button } => {
                self.download_card(DownloadCardInput::Answer { token, button })
            }
            DownloadsUiEvent::CardExpired(token) => {
                self.download_card(DownloadCardInput::Expire(token))
            }
        };
        if let Some(kind) = leave {
            self.leave_confirmed(event_loop, kind);
        }
    }

    /// Sair por `kind` depois do «Cancelar e sair» armado: o cartao ja
    /// cancelou cada download (`download_card_step`). So o
    /// `downloads_ui_event` chama isto (gate
    /// `home_and_exit_callers_are_a_named_allowlist`).
    fn leave_confirmed(&mut self, event_loop: &ActiveEventLoop, kind: LeaveKind) {
        match kind {
            LeaveKind::Home => self.show_home(),
            LeaveKind::Close => self.exit_now(event_loop),
        }
    }

    /// Fechar a janela (o X, Alt+F4, o comando Sair): com downloads a
    /// correr, pergunta antes (`leave_guard`); sem nada a correr, sai ja.
    pub(in crate::windows_app) fn request_close(&mut self, event_loop: &ActiveEventLoop) {
        if self.leave_guard(LeaveKind::Close) {
            self.exit_now(event_loop);
        }
    }

    /// A unica saida da janela: o rascunho das notas vai para o disco e o
    /// event loop acaba. So `request_close` e `leave_confirmed` a chamam.
    fn exit_now(&mut self, event_loop: &ActiveEventLoop) {
        self.save_notes_draft_before_exit();
        event_loop.exit();
    }

    /// Uma volta do cartao e o que ela pede.
    pub(in crate::windows_app) fn download_card(
        &mut self,
        input: DownloadCardInput,
    ) -> Option<LeaveKind> {
        let step = download_card_step(&mut self.downloads_ui.card, input, Instant::now());
        if step.hide {
            self.hide_download_card();
        }
        if let Some((view, expiry)) = step.show {
            let token = view.token;
            self.show_download_card(view);
            self.timers.after(
                expiry,
                UserEvent::DownloadsUi(DownloadsUiEvent::CardExpired(token)),
            );
        }
        for event in step.events {
            self.download_event(event);
        }
        step.leave
    }

    /// Antes de sair por `kind`: `true` sai ja; `false` ficou a pergunta
    /// (`leave_decision`). Um «Baixar programa?» a vista responde-se «nao»
    /// primeiro -- a saida destroi a WebView dele de qualquer forma.
    pub(in crate::windows_app) fn leave_guard(&mut self, kind: LeaveKind) -> bool {
        if let Some(pending) = &self.downloads_ui.card.pending
            && let DownloadPrompt::Program { id, .. } = pending.payload
        {
            let token = pending.token;
            self.downloads_ui.card.expire(token);
            self.hide_download_card();
            self.download_event(DownloadEvent::Answered { id, allow: false });
        }
        match leave_decision(kind, &self.downloads.manager) {
            LeaveDecision::Leave => true,
            LeaveDecision::Ask(prompt) => {
                // Fechar pela barra de tarefas com a janela minimizada: o
                // cartao e owned por ela e nao se veria.
                if let Some(window) = &self.window
                    && window.is_minimized() == Some(true)
                {
                    window.set_minimized(false);
                }
                self.download_card(DownloadCardInput::Request(DownloadPrompt::Leave(prompt)));
                false
            }
        }
    }

    /// A Home, a nao ser que haja downloads a correr (entao pergunta).
    /// `true`: foi para a Home.
    pub(in crate::windows_app) fn request_home(&mut self) -> bool {
        if !self.leave_guard(LeaveKind::Home) {
            return false;
        }
        self.show_home();
        true
    }

    /// A Home da sonda do CI (`UserEvent::LifecycleProbeHome`): sem o
    /// cartao, porque o que o measure-cycles.ps1 e o spike do
    /// test-downloads.ps1 medem e a WebView destruida -- no spike, com o
    /// download a correr. Sem NEURALIA_LIFECYCLE_PROBE e uma Home como as
    /// outras, com a pergunta.
    pub(in crate::windows_app) fn lifecycle_probe_home(&mut self) {
        if lifecycle_probe_enabled() {
            self.show_home();
        } else {
            self.request_home();
        }
    }

    /// Depois de cada volta do gestor: os avisos para o toast, a pergunta
    /// «Baixar programa?», a medida e o toast de cada download que mudou, a
    /// lista do painel e a seta da barra.
    pub(in crate::windows_app) fn downloads_ui_after(
        &mut self,
        later: Vec<neural_core::downloads::DownloadEffect>,
    ) {
        use neural_core::downloads::DownloadEffect;
        let now = Instant::now();
        let mut changed = false;
        for effect in later {
            match effect {
                DownloadEffect::Notice(notice) => self.notify(download_toast(&notice)),
                DownloadEffect::Ask { id, reason } => {
                    let name = self
                        .downloads
                        .manager
                        .entry(id)
                        .map(|entry| entry.name.clone())
                        .unwrap_or_default();
                    self.download_card(DownloadCardInput::Request(DownloadPrompt::Program {
                        id,
                        name,
                        reason,
                    }));
                }
                DownloadEffect::Changed(id) => {
                    changed = true;
                    let Some(entry) = self.downloads.manager.entry(id).cloned() else {
                        continue;
                    };
                    if let Some(notice) = download_changed(&mut self.downloads_ui, &entry, now) {
                        self.notify(notice);
                    }
                    // A WebView dele foi-se (ou outra resposta chegou): o
                    // cartao ja nao pergunta nada.
                    if !matches!(entry.state, DownloadState::Asking(_))
                        && let Some(pending) = &self.downloads_ui.card.pending
                        && matches!(pending.payload, DownloadPrompt::Program { id: asked, .. } if asked == id)
                    {
                        let token = pending.token;
                        self.downloads_ui.card.expire(token);
                        self.hide_download_card();
                    }
                }
                DownloadEffect::EraseLog => changed = true,
                _ => {}
            }
        }
        if changed {
            self.render_downloads_panel();
            let badge = downloads_badge(&self.downloads.manager);
            if badge != self.downloads_ui.badge {
                self.downloads_ui.badge = badge;
                self.request_redraw();
            }
        }
    }

    /// A seta e o Ctrl+J: o painel na seccao Downloads; com o painel
    /// aberto, a pagina decide (nos Downloads fecha).
    pub(in crate::windows_app) fn toggle_downloads_panel(&mut self) {
        if self.side_panel.is_open() {
            self.panel_run(PANEL_DOWNLOADS_BUTTON_SCRIPT.to_string());
            return;
        }
        self.open_side_panel();
        if self.side_panel.is_open() {
            self.panel_run(PANEL_SHOW_DOWNLOADS_SCRIPT.to_string());
        }
    }

    /// O «Ver» do toast: a seccao Downloads, aberta ou nao -- nunca fecha.
    pub(in crate::windows_app) fn show_downloads_panel(&mut self) {
        if !self.side_panel.is_open() {
            self.open_side_panel();
        }
        if self.side_panel.is_open() {
            self.panel_run(PANEL_SHOW_DOWNLOADS_SCRIPT.to_string());
        }
    }

    /// A lista para a seccao Downloads, se ela ja se abriu na pagina viva.
    pub(in crate::windows_app) fn render_downloads_panel(&mut self) {
        if !self.downloads_ui.panel_live || !self.side_panel.is_open() {
            return;
        }
        let rows = self
            .downloads_ui
            .rows
            .list(&self.downloads.manager, &self.downloads_ui.meters);
        let allow = self.downloads.manager.settings().allow_programs;
        self.panel_run(downloads_render_script(&rows, allow));
    }

    /// Um pedido da seccao Downloads.
    pub(in crate::windows_app) fn downloads_panel_request(
        &mut self,
        request: DownloadsPanelRequest,
    ) {
        match request {
            DownloadsPanelRequest::List => {
                self.downloads_ui.panel_live = true;
                self.render_downloads_panel();
            }
            DownloadsPanelRequest::Open(id) => {
                let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
                    return;
                };
                let outcome = open_download_row(
                    &self.downloads.manager,
                    &self.downloads_ui.rows,
                    id,
                    &mut WindowsShell { owner },
                );
                self.report_open(outcome);
            }
            DownloadsPanelRequest::Show(id) => {
                let Some(owner) = self.window.as_ref().and_then(window_hwnd) else {
                    return;
                };
                let outcome = show_download_row(
                    &self.downloads.manager,
                    &self.downloads_ui.rows,
                    id,
                    &mut WindowsShell { owner },
                );
                self.report_open(outcome);
            }
            DownloadsPanelRequest::Cancel(id) => {
                if let Some(RowTarget::Session(download)) = self.downloads_ui.rows.target(id) {
                    let download = *download;
                    self.download_event(DownloadEvent::CancelRequested { id: download });
                }
            }
            DownloadsPanelRequest::AllowPrograms(on) => self.set_allow_programs(on),
        }
    }

    fn report_open(&mut self, outcome: OpenOutcome) {
        match outcome {
            OpenOutcome::Opened => {}
            OpenOutcome::Refused => self.show_splash(
                "O NeuralIA não abre este tipo de arquivo. Use «Mostrar na pasta».".to_string(),
                4,
            ),
            OpenOutcome::NoFile => {
                self.show_splash("O arquivo já não está lá.".to_string(), 3);
                self.render_downloads_panel();
            }
            OpenOutcome::Failed(error) => {
                self.show_splash(format!("Não foi possível abrir: {error}"), 4)
            }
        }
    }

    /// «Permitir baixar programas»: grava a escolha (so ela: a pasta que
    /// esta no disco fica como esta) e o gestor passa a usa-la. Mesmo
    /// ligada, cada programa pede o seu «Baixar programa?».
    pub(in crate::windows_app) fn set_allow_programs(&mut self, on: bool) {
        if let Err(error) = self.downloads.set_allow_programs(on) {
            self.show_splash(format!("A definição não foi gravada: {error}"), 4);
        }
        self.render_downloads_panel();
    }

    /// Pinta `view` no cartao (criando a janela, se preciso) e poe-no a
    /// vista sem o ativar.
    pub(in crate::windows_app) fn show_download_card(&mut self, view: DownloadCardView) {
        if let Ok(mut current) = DOWNLOAD_CARD_VIEW.lock() {
            *current = Some(view);
        }
        // Um clique a meio no cartao anterior nao passa para o novo.
        DOWNLOAD_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
        if self.downloads_ui.card_popup.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let Some(owner) = window_hwnd(window) else {
                return;
            };
            let scale = window.scale_factor().max(1.0);
            let width = (DOWNLOAD_CARD_WIDTH * scale).round() as i32;
            let height = (DOWNLOAD_CARD_HEIGHT * scale).round() as i32;
            let corner = (18.0 * scale).round() as i32;
            let created = unsafe {
                create_native_card(
                    owner,
                    width,
                    height,
                    download_card_subclass,
                    DOWNLOAD_CARD_SUBCLASS_ID,
                    (&*self.downloads_ui.card_sink as *const DownloadCardSink) as usize,
                    corner,
                )
            };
            let Some(created) = created else {
                return;
            };
            self.downloads_ui.card_popup = Some(created);
        }
        self.position_download_card();
    }

    pub(in crate::windows_app) fn hide_download_card(&mut self) {
        if let Some(card) = self.downloads_ui.card_popup.take() {
            unsafe {
                DestroyWindow(card);
            }
        }
        if let Ok(mut view) = DOWNLOAD_CARD_VIEW.lock() {
            *view = None;
        }
        DOWNLOAD_CARD_PAINTED.store(0, Ordering::Release);
        DOWNLOAD_CARD_PRESSED.store(NATIVE_BUTTON_NONE, Ordering::Release);
    }

    /// Centra o cartao na janela, em coordenadas de ECRA (como o cartao da
    /// barra de selecao): refaz-se quando a janela se mexe.
    pub(in crate::windows_app) fn position_download_card(&self) {
        let (Some(window), Some(card)) = (&self.window, self.downloads_ui.card_popup) else {
            return;
        };
        let Some(owner) = window_hwnd(window) else {
            return;
        };
        let scale = window.scale_factor().max(1.0);
        let width = (DOWNLOAD_CARD_WIDTH * scale).round() as i32;
        let height = (DOWNLOAD_CARD_HEIGHT * scale).round() as i32;
        let mut client = RECT::default();
        unsafe {
            if GetClientRect(owner, &mut client) == 0 {
                return;
            }
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            let (x, y) = splash_origin(client.right, client.bottom, width, height);
            SetWindowPos(
                card,
                std::ptr::null_mut(),
                origin.x + x,
                origin.y + y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            show_popup_without_activation(card);
            InvalidateRect(card, std::ptr::null(), 1);
        }
    }
}
