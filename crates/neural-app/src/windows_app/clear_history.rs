use super::*;

// ===================== "Apagar histórico" (Ctrl+Shift+Delete) =====================
//
// Modulo de feature (o padrao de `theme.rs`): o que o "Apagar historico"
// apaga esta numa TABELA com nome, `CLEAR_HISTORY_TARGETS`, e o event loop
// so a percorre. Uma feature que passe a guardar algo que o Ctrl+Shift+Delete
// deve levar acrescenta uma variante em `ClearTarget`, a sua linha na tabela
// e o seu braco no `ClearHistorySink` do `App` (abaixo) -- e o gate
// `clear_history_runs_every_registered_target` prova que o event loop
// chega a cada alvo registado, pela ordem, uma vez. O que sobrevive a um
// apagar (chaves, favoritos, definicoes) nunca entra aqui.

/// Cada coisa que o "Apagar historico" apaga, pelo nome. Ordem = a ordem em
/// que se apaga: o historico vai por ultimo porque e ele que muda de ecra
/// (Home) e escreve o estado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum ClearTarget {
    /// `tabs.json`: as abas e os grupos do comparador.
    Tabs,
    /// A memoria semantica local e a pesquisa em curso.
    Memory,
    /// Na biblioteca de livros, quando cada livro foi aberto ("Continuar
    /// lendo", recentes); posicoes e marcadores ficam.
    EpubLibrary,
    /// `history.jsonl`: o historico cronologico.
    History,
}

impl ClearTarget {
    /// Todos os alvos, na ordem em que o event loop os apaga. E a lista que
    /// `CLEAR_HISTORY_TARGETS` tem de cobrir inteira.
    pub(in crate::windows_app) const ALL: [Self; 4] =
        [Self::Tabs, Self::Memory, Self::EpubLibrary, Self::History];
}

/// A tabela: o que um Ctrl+Shift+Delete confirmado apaga, pela ordem.
pub(in crate::windows_app) const CLEAR_HISTORY_TARGETS: &[ClearTarget] = &ClearTarget::ALL;

/// Quem sabe apagar um alvo. O `App` e o unico que embarca; os testes metem
/// aqui um gravador para provar que o percurso chega a todos.
pub(in crate::windows_app) trait ClearHistorySink {
    fn clear(&mut self, target: ClearTarget);
}

/// Percorre a tabela, um alvo de cada vez, pela ordem. Nao pergunta: a
/// confirmacao ("Apagar TODO o historico...?") fica a cargo de quem chama.
pub(in crate::windows_app) fn clear_history_targets(sink: &mut impl ClearHistorySink) {
    for target in CLEAR_HISTORY_TARGETS {
        sink.clear(*target);
    }
}

impl ClearHistorySink for App {
    fn clear(&mut self, target: ClearTarget) {
        match target {
            ClearTarget::Tabs => self.forget_tab_session(),
            ClearTarget::Memory => self.memory.clear(&mut self.current_research),
            ClearTarget::EpubLibrary => {
                // Na biblioteca de livros, some quando cada livro foi aberto
                // ("Continuar lendo", recentes); posições e marcadores ficam.
                if self.epub.is_some()
                    || self
                        .config
                        .data_dir
                        .join("library")
                        .join(neural_core::library::INDEX_FILE)
                        .exists()
                {
                    self.submit_epub_job(EpubJob::ClearReadingHistory);
                }
            }
            ClearTarget::History => match self.history.clear() {
                None => {
                    self.show_home();
                    self.status = Some("A apagar o histórico local…".to_string());
                    self.request_redraw();
                }
                Some(result) => self.report_history_cleared(result),
            },
        }
    }
}

impl App {
    /// O braco `UserEvent::ClearHistory` do event loop: pergunta primeiro
    /// (`confirm_clear_history`, com o "Nao" por omissao) e so com o "Sim"
    /// percorre a tabela dos alvos.
    pub(in crate::windows_app) fn clear_history(&mut self) {
        if !self.confirm_clear_history() {
            return;
        }
        clear_history_targets(self);
    }
}
