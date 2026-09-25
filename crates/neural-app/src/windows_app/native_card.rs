//! O cartao nativo (infra-notify-popups, critica C4): as regras que protegem
//! quem le, generalizadas do cartao da barra de selecao ("Mandar para as 3
//! IAs?", "Traduzir nas 3 IAs?") para os cartoes que vem a seguir (o
//! Explicar, as aprovacoes do agente).
//!
//! - `NativeCard<T>`: um cartao de cada vez, com um TOKEN por pedido (um
//!   clique num cartao que ja nao esta la nao responde a nada), um ARMAR de
//!   600 ms (um clique que a pagina pediu no sitio onde o cartao ia nascer
//!   nao o confirma), uma EXPIRACAO (quem o chama agenda o fim; o fim de um
//!   cartao substituido nao conta) e a confirmacao SO do que foi pintado
//!   (quem confirma diz o que a pintura mostrou; nada pintado, nada vai).
//! - A janela: popup owned pela janela principal, `WS_EX_NOACTIVATE`, nunca
//!   TOPMOST, invisivel ao nascer e mostrada sem ativacao, e o clique nao a
//!   ativa (`MA_NOACTIVATE`): o foco fica na pagina. Gate real Win32
//!   `native_card_never_activates` (so CI).

use super::*;

/// Um confirmar que chega antes disto, contado desde que o cartao (ou o
/// pedido que o trocou) apareceu, nao conta.
pub(in crate::windows_app) const NATIVE_CARD_ARM: Duration = Duration::from_millis(600);

/// O pedido que espera o clique: o token, o que ele leva e quando apareceu.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::windows_app) struct PendingCard<T> {
    pub(in crate::windows_app) token: u64,
    pub(in crate::windows_app) payload: T,
    pub(in crate::windows_app) shown_at: Instant,
}

/// O que um clique num cartao da.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::windows_app) enum CardAnswer<R> {
    /// Confirmado, com o que a pintura mostrou.
    Confirmed(R),
    Cancelled,
    /// Nada muda: outro cartao, cedo demais, nada a vista.
    Ignored,
}

/// Um cartao de cada vez: pendente -> confirmado, cancelado, expirado ou
/// trocado por um pedido novo.
#[derive(Debug)]
pub(in crate::windows_app) struct NativeCard<T> {
    pub(in crate::windows_app) pending: Option<PendingCard<T>>,
    pub(in crate::windows_app) last_token: u64,
}

impl<T> Default for NativeCard<T> {
    fn default() -> Self {
        Self {
            pending: None,
            last_token: 0,
        }
    }
}

impl<T> NativeCard<T> {
    /// Um pedido novo: token novo (nunca 0), e substitui o que esperava.
    /// Devolve o token e se trocou um pedido pendente.
    pub(in crate::windows_app) fn request(&mut self, payload: T, now: Instant) -> (u64, bool) {
        self.last_token = self.last_token.wrapping_add(1).max(1);
        let token = self.last_token;
        let replaced = self
            .pending
            .replace(PendingCard {
                token,
                payload,
                shown_at: now,
            })
            .is_some();
        (token, replaced)
    }

    /// Um clique no cartao `token`: `confirm` e o botao de confirmar (o
    /// outro e Cancelar). `painted` diz o que a pintura mostrou do pedido
    /// (`None`: nada que se possa confirmar). Confirmar cedo demais, ou sem
    /// nada a vista, deixa o cartao a espera de um clique a serio.
    pub(in crate::windows_app) fn answer<R>(
        &mut self,
        token: u64,
        confirm: bool,
        now: Instant,
        painted: impl FnOnce(&T) -> Option<R>,
    ) -> CardAnswer<R> {
        let Some(pending) = self.pending.take_if(|pending| pending.token == token) else {
            return CardAnswer::Ignored;
        };
        if !confirm {
            return CardAnswer::Cancelled;
        }
        if now.saturating_duration_since(pending.shown_at) >= NATIVE_CARD_ARM
            && let Some(seen) = painted(&pending.payload)
        {
            return CardAnswer::Confirmed(seen);
        }
        self.pending = Some(pending);
        CardAnswer::Ignored
    }

    /// O prazo do cartao `token` passou: sai, se ainda for ele.
    pub(in crate::windows_app) fn expire(&mut self, token: u64) -> bool {
        self.pending
            .take_if(|pending| pending.token == token)
            .is_some()
    }
}

/// As mensagens em que um popup nosso nunca se deixa ativar nem atravessar:
/// os cliques sao dele (um STATIC devolve HTTRANSPARENT e o clique ia para
/// a pagina) e clicar nao o ativa. Os cartoes e o aviso do canto passam
/// por aqui antes de tudo.
pub(in crate::windows_app) fn popup_no_activate_message(message: u32) -> Option<LRESULT> {
    match message {
        WM_NCHITTEST => Some(HTCLIENT as LRESULT),
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as LRESULT),
        _ => None,
    }
}

/// A receita de criacao de um cartao: owned por `owner` (acima do WebView2
/// e da pagina, nao acima das outras aplicacoes: nunca TOPMOST), invisivel
/// e sem ativacao, com cantos de `corner` px e a subclasse do cartao.
pub(in crate::windows_app) unsafe fn create_native_card(
    owner: HWND,
    width: i32,
    height: i32,
    subclass: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT,
    subclass_id: usize,
    reference: usize,
    corner: i32,
) -> Option<HWND> {
    let created = CreateWindowExW(
        AUX_POPUP_EX_STYLE,
        windows_sys::w!("STATIC"),
        windows_sys::w!(""),
        AUX_POPUP_STYLE,
        0,
        0,
        width,
        height,
        owner,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null(),
    );
    if created.is_null() {
        return None;
    }
    if SetWindowSubclass(created, Some(subclass), subclass_id, reference) == 0 {
        DestroyWindow(created);
        return None;
    }
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, corner, corner);
    if !region.is_null() {
        SetWindowRgn(created, region, 1);
    }
    Some(created)
}
