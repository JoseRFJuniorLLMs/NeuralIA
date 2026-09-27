//! O portao da persistencia (infra-privacy-guard, plano 2.4; SPEC-0006
//! «Persistence chokepoint»).
//!
//! `PrivacyGuard` e dono do registo das lojas (`StoreRegistry`, cunhado uma
//! vez no `App::new` e entregue aqui inteiro), do modo partilhado que o
//! registo passa a cada grant, e dos tres escritores `Automatic` que sao
//! efeito lateral de navegar: o historico (`HistoryWriter`), a memoria
//! semantica (`MemoryWorker`) e as abas (`TabPersistence`). O `App` nunca ve
//! esses tres nem o registo: grava pelo guard (`record`, `capture`,
//! `save_session`, `save_tabs`, `save_tabs_due`, `rebuild_memory`) e pede
//! grants pelo guard (`store`). Enquanto o modo disser `Private`, as
//! gravacoes `Automatic` nao fazem nada e as leituras (`recent_history`,
//! `query_memory`, `restore_tabs`) nao escrevem; hoje o produto so conhece
//! `Normal` -- `Private` chega com o private-mode-core e ate la so existe
//! pelo construtor de teste.
//!
//! Gates (`windows_app/tests.rs`): `registry_is_minted_once_and_owned_by_the_guard`,
//! `history_memory_and_tabs_are_written_only_through_the_privacy_guard`,
//! `private_guard_turns_automatic_stores_into_noops`,
//! `the_e2e_allowlist_covers_a_normal_guard_session`,
//! `no_raw_data_dir_write_outside_a_grant`.

use neural_core::json_store::{
    AlreadyMinted, GrantError, StoreGrant, StoreMode, StoreRegistry, StoreSpec,
};
use neural_core::{CoreConfig, HistoryEntry, HistoryStore, MemoryDocument, ResearchSession};

use crate::stores::{HISTORY_STORE, MEMORY_STORE, TABS_STORE};
use crate::windows_app::{
    COMPARATOR_COLUMNS, ContextGroup, ContextTab, EventSink, HistoryWriter, MemoryWorker,
    RestoredTabs, TabPersistence, TabSave,
};

/// O modo do guard: o mesmo que o registo passa a cada grant (`StoreMode`).
/// `Private` nao e alcancavel no produto ainda (private-mode-core).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivacyMode {
    Normal,
    /// As gravacoes das lojas `Automatic` nao fazem nada.
    Private,
}

impl From<StoreMode> for PrivacyMode {
    fn from(mode: StoreMode) -> Self {
        match mode {
            StoreMode::Normal => Self::Normal,
            StoreMode::Private => Self::Private,
        }
    }
}

impl From<PrivacyMode> for StoreMode {
    fn from(mode: PrivacyMode) -> Self {
        match mode {
            PrivacyMode::Normal => Self::Normal,
            PrivacyMode::Private => Self::Private,
        }
    }
}

/// O portao. Sem campos publicos: o registo, os grants e os workers so se
/// usam pelos metodos daqui.
pub(crate) struct PrivacyGuard {
    registry: StoreRegistry,
    history: HistoryWriter,
    /// `history.jsonl`, `Automatic`: `writes_allowed()` e o modo.
    history_grant: StoreGrant,
    memory: MemoryWorker,
    /// `memory/`, `Automatic`.
    memory_grant: StoreGrant,
    tabs: TabPersistence,
    /// `tabs.json`, `Automatic`.
    tabs_grant: StoreGrant,
}

impl PrivacyGuard {
    /// O portao do produto: recebe a cunhagem do `App::new` inteira (a unica
    /// do processo), pede ao registo os grants do historico, da memoria e
    /// das abas, e arranca os dois workers (`neural-history`,
    /// `neural-memory`) e a ponte das abas, como o `App::new` fazia. O modo
    /// nasce `Normal` e nada aqui o muda.
    pub(crate) fn new(
        minted: Result<StoreRegistry, AlreadyMinted>,
        config: &CoreConfig,
        sink: EventSink,
    ) -> Self {
        let registry =
            minted.expect("o registo das lojas cunha-se uma vez por processo, no App::new");
        Self::with_registry(registry, config, sink)
    }

    /// Um portao sobre uma pasta de teste, no `mode` pedido, sem gastar a
    /// cunhagem do processo. E a unica porta para `Private` ate ao
    /// private-mode-core.
    #[cfg(test)]
    pub(crate) fn for_test(
        dir: impl Into<std::path::PathBuf>,
        mode: PrivacyMode,
        sink: EventSink,
    ) -> Self {
        let dir = dir.into();
        let registry = StoreRegistry::mint_for_test(&dir);
        registry.set_mode(mode.into());
        let config = CoreConfig {
            data_dir: dir,
            ..CoreConfig::default()
        };
        Self::with_registry(registry, &config, sink)
    }

    fn with_registry(registry: StoreRegistry, config: &CoreConfig, sink: EventSink) -> Self {
        // Os tres nomes vem da tabela (`stores::APP_STORES`) e sao os
        // primeiros pedidos ao registo: um recusado seria um erro de
        // programacao, nao um estado.
        let own = |spec: StoreSpec, granted: Result<StoreGrant, GrantError>| {
            granted.unwrap_or_else(|error| panic!("{}: {error}", spec.name))
        };
        let history_grant = own(HISTORY_STORE, registry.grant(HISTORY_STORE));
        let memory_grant = own(MEMORY_STORE, registry.grant(MEMORY_STORE));
        let tabs_grant = own(TABS_STORE, registry.grant(TABS_STORE));
        let history_store = HistoryStore::with_limit(history_grant.path(), config.history_limit);
        let history = HistoryWriter::new(history_store, sink.clone());
        let memory = MemoryWorker::new(memory_grant.path().to_path_buf(), sink);
        let tabs = TabPersistence::open(&config.data_dir);
        Self {
            registry,
            history,
            history_grant,
            memory,
            memory_grant,
            tabs,
            tabs_grant,
        }
    }

    /// O modo de agora, o mesmo que cada grant ja passado ve.
    pub(crate) fn mode(&self) -> PrivacyMode {
        self.registry.mode().into()
    }

    /// Um grant para uma loja da tabela (`crate::stores`): a unica maneira de
    /// uma feature abrir a sua loja. `None` se o registo recusou (nome
    /// invalido ou o mesmo nome ja dado com outro tipo) -- a feature fica
    /// sem disco, como sem pasta de dados.
    pub(crate) fn store(&self, spec: StoreSpec) -> Option<StoreGrant> {
        self.registry.grant(spec).ok()
    }

    // ----- o historico (`history.jsonl`, Automatic) -----

    /// Uma entrada no historico. Nunca faz I/O aqui (o worker grava); no
    /// modo privado nao faz nada.
    pub(crate) fn record(&self, entry: HistoryEntry) {
        if !self.history_grant.writes_allowed() {
            return;
        }
        self.history.append(entry);
    }

    /// As `limit` entradas mais recentes, pelo worker (`HistoryLoaded`). Le,
    /// em qualquer modo; numa pasta onde nunca se gravou historico nao cria
    /// o `history.jsonl.lock` (`HistoryStore::recent`).
    pub(crate) fn recent_history(&self, limit: usize) -> Option<Result<Vec<HistoryEntry>, String>> {
        self.history.recent(limit)
    }

    /// "Apagar historico": apaga em qualquer modo (e privacidade).
    pub(crate) fn clear_history(&self) -> Option<Result<(), String>> {
        self.history.clear()
    }

    // ----- a memoria semantica (`memory/`, Automatic) -----

    /// Um documento para a memoria. No modo privado nao faz nada.
    pub(crate) fn capture(&self, document: MemoryDocument) {
        if !self.memory_grant.writes_allowed() {
            return;
        }
        self.memory.capture(document);
    }

    /// A sessao de pesquisa viva, para `memory/sessions/<id>.json`. No modo
    /// privado nao faz nada.
    pub(crate) fn save_session(&self, session: ResearchSession) {
        if !self.memory_grant.writes_allowed() {
            return;
        }
        self.memory.save_session(session);
    }

    /// Uma pesquisa na memoria (`MemoryQueryReady`): le, em qualquer modo.
    /// So um indice que nao e o de agora (o da v2.0.x, ou o que um rebuild
    /// interrompido deixou) e reparado na pesquisa: dados derivados, nada de
    /// novo (SPEC-0006, excecoes da fase 0).
    pub(crate) fn query_memory(&self, query: String) {
        self.memory.query(query);
    }

    /// `memory:rebuild`: o indice derivado dos documentos que ja la estao.
    /// Reescreve o SQLite e o manifesto de `memory/db` (`Automatic`): no modo
    /// privado nao faz nada e devolve `false` (nada agendado).
    pub(crate) fn rebuild_memory(&self) -> bool {
        if !self.memory_grant.writes_allowed() {
            return false;
        }
        self.memory.rebuild();
        true
    }

    /// "Apagar historico": esquece tambem a sessao viva. Em qualquer modo.
    pub(crate) fn clear_memory(&self, current_research: &mut Option<ResearchSession>) {
        self.memory.clear(current_research);
    }

    // ----- as abas (`tabs.json`, Automatic) -----

    /// As abas da sessao anterior e o aviso para o dono, se houver. No modo
    /// privado le sem escrever: um `tabs.json` estragado fica onde esta, sem
    /// ir para o `tabs.json.bak`.
    pub(crate) fn restore_tabs(&mut self) -> (RestoredTabs, Option<String>) {
        if !self.tabs_grant.writes_allowed() {
            return self.tabs.restore_read_only();
        }
        self.tabs.restore()
    }

    /// O modelo depois de um lote de eventos: o bilhete da gravacao a
    /// agendar, se mudou.
    pub(crate) fn observe_tabs(
        &mut self,
        contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> Option<u64> {
        self.tabs.observe(contexts, groups, split)
    }

    /// Grava ja, se o disco estiver atrasado (ao destruir o comparador e ao
    /// sair). No modo privado nao grava: `TabSave::SkippedPrivate`.
    pub(crate) fn save_tabs(
        &mut self,
        contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> std::io::Result<TabSave> {
        if !self.tabs_grant.writes_allowed() {
            return Ok(TabSave::SkippedPrivate);
        }
        self.tabs.save_now(contexts, groups, split)
    }

    /// O `SaveTabSession(token)` do fim do atraso: so o ultimo agendado
    /// grava. No modo privado nao grava.
    pub(crate) fn save_tabs_due(
        &mut self,
        token: u64,
        contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> Option<std::io::Result<TabSave>> {
        if !self.tabs_grant.writes_allowed() {
            return self
                .tabs
                .save_due_token(token)
                .then_some(Ok(TabSave::SkippedPrivate));
        }
        self.tabs.save_due(token, contexts, groups, split)
    }

    /// "Apagar historico": o modelo vivo, o ficheiro e as copias. Em
    /// qualquer modo.
    pub(crate) fn forget_tabs(
        &mut self,
        contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
        groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
        split: Option<(usize, Option<u64>, bool)>,
    ) -> std::io::Result<()> {
        self.tabs.forget(contexts, groups, split)
    }
}
