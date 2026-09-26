use super::*;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, RwLock};

use neural_core::adblock::{
    AdblockRules, AdblockSettings, DomainSet, ListClient, ListEndpoint, RefreshState,
    RefreshSurface, ResourceKind, SETTINGS_MAX_BYTES, SETTINGS_VERSION, STORED_LIST_MAX_BYTES,
    STORED_LIST_VERSION, StoredList, download_on_activation, is_storable_site,
    page_is_always_exempt, refresh_due, site_key,
};
#[cfg(test)]
use neural_core::distraction::DistractionPolicy;
use neural_core::distraction::{MAX_DISTRACTION_SITES, SiteChange, distraction_site_key};
use neural_core::json_store::{StoreGrant, VersionedJsonStore};

use crate::stores::{ADBLOCK_LIST_STORE, ADBLOCK_SETTINGS_STORE};

// ===================== o bloqueio de anuncios no app (adblock) =====================
//
// O nucleo (`neural_core::adblock`) decide; aqui vive o que o liga ao
// produto:
//
// - `AdblockShared`: o que os handlers do WebView2 leem na thread da
//   interface -- as regras em vigor (`None` desligado ou sem lista ainda) e
//   se as WebViews novas recebem o filtro `*` do `WebResourceRequested`.
//   O despachante (`resource_gate_answers`, em `webview_hooks.rs`) so o
//   consulta nas colunas, na fonte ao lado e na Web completa; a fonte
//   privada, as paginas locais e os servicos nunca o recebem.
// - `AdblockState`: a escolha do utilizador (`adblock-settings.json`, loja
//   `Setting`) e a lista em memoria. A lista baixada vive em
//   `adblock-list.json` (loja `Automatic`), lida e escrita so nas threads
//   `neural-adblock`. O mesmo ficheiro guarda, no campo `distraction`, a
//   politica da anti-distracao (`distraction.rs`); as escolhas feitas no
//   Split privado ficam so em memoria (`set_distraction_site`).
// - O menu do botao direito (`adblock_menu`): "Ativar bloqueio de
//   anuncios" enquanto desligado; ligado, a caixa "Bloquear anuncios em
//   <host> (N bloqueados)" e "Desativar bloqueio de anuncios"; numa pagina
//   de IA ou de login, "Sempre desligado nas paginas das IAs", cinzento.
//
// A rede: so o clique em "Ativar" baixa a lista (se a guardada tiver mais
// de uma semana ou nao existir), e a renovacao automatica corre no maximo
// uma vez por semana quando uma pagina web acaba de carregar -- nunca na
// Home (`refresh_due`). Nada disto acontece sem o clique: com o bloqueio
// desligado nenhuma WebView recebe o filtro e nada se le do disco.

/// O que os handlers do WebView2 veem do bloqueio.
#[derive(Debug, Clone, Default)]
pub(in crate::windows_app) enum AdblockView {
    /// Nunca ativado, ou desativado.
    #[default]
    Off,
    /// Ligado (ou a ligar) sem a lista em memoria: a ler do disco ou a
    /// baixar.
    Preparing,
    /// A bloquear com estas regras.
    Active(Arc<AdblockRules>),
}

/// O estado partilhado com os handlers. Tudo o que o toca corre na thread
/// da interface; o `RwLock` e o `AtomicBool` so tornam isso seguro de
/// partilhar pelos `Arc` que os handlers guardam.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct AdblockShared {
    view: RwLock<AdblockView>,
    /// O bloqueio esta ligado: as WebViews novas das colunas, da fonte ao
    /// lado e da Web completa recebem o filtro `*`.
    filtering: AtomicBool,
}

impl AdblockShared {
    pub(in crate::windows_app) fn view(&self) -> AdblockView {
        self.view
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// As regras em vigor, se o bloqueio esta ativo e a lista carregada.
    pub(in crate::windows_app) fn rules(&self) -> Option<Arc<AdblockRules>> {
        match &*self
            .view
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            AdblockView::Active(rules) => Some(Arc::clone(rules)),
            AdblockView::Off | AdblockView::Preparing => None,
        }
    }

    pub(in crate::windows_app) fn filtering(&self) -> bool {
        self.filtering.load(Ordering::Acquire)
    }

    pub(in crate::windows_app) fn set_view(&self, view: AdblockView) {
        *self
            .view
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = view;
    }

    fn set_filtering(&self, on: bool) {
        self.filtering.store(on, Ordering::Release);
    }
}

/// Os hospedeiros onde o bloqueio vale: as colunas das IAs, a fonte aberta
/// ao lado (nao a privada) e a Web completa.
pub(in crate::windows_app) fn adblock_host(host: WebViewHost) -> bool {
    matches!(
        host,
        WebViewHost::Column(_) | WebViewHost::Split(_) | WebViewHost::External
    )
}

/// A pagina de um pedido: o topo (o `Source` do WebView que o fez) e o tipo
/// do recurso.
#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct ResourcePage<'a> {
    pub(in crate::windows_app) top: &'a str,
    pub(in crate::windows_app) kind: ResourceKind,
}

/// O veredicto do bloqueio para um pedido: sem regras (desligado, sem
/// lista), com um endereco que nao se le, bloqueia nada.
pub(in crate::windows_app) fn adblock_blocks(
    uri: &str,
    page: ResourcePage<'_>,
    rules: Option<&AdblockRules>,
) -> bool {
    let Some(rules) = rules else {
        return false;
    };
    let Ok(request) = Url::parse(uri.trim()) else {
        return false;
    };
    let top = Url::parse(page.top.trim())
        .ok()
        .filter(|top| matches!(top.scheme(), "http" | "https"));
    rules.decide(&request, top.as_ref(), page.kind).blocks()
}

/// O `ResourceContext` do WebView2 como o nucleo o le. So o documento e
/// especial (nunca se bloqueia); o resto e o tipo que o recurso tem.
pub(in crate::windows_app) fn resource_kind_of(context: i32) -> ResourceKind {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
    };
    match context {
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT.0 => ResourceKind::Document,
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET.0 => ResourceKind::Stylesheet,
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE.0 => ResourceKind::Image,
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA.0 => ResourceKind::Media,
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT.0 => ResourceKind::Font,
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT.0 => ResourceKind::Script,
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST.0 => ResourceKind::Xhr,
        c if c == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH.0 => ResourceKind::Fetch,
        _ => ResourceKind::Other,
    }
}

/// Quantos pedidos se bloquearam na pagina que esta a vista numa WebView:
/// o que o menu mostra entre parenteses. Um por WebView, partilhado pelo
/// `WebResourceRequested` (que conta) e pelo `ContextMenuRequested` (que
/// le); outra pagina recomeca do zero.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct PageBlocked {
    page: Mutex<(String, u32)>,
}

/// A pagina sem o fragmento: um `#secao` nao e outra pagina.
fn page_identity(top: &str) -> &str {
    top.split('#').next().unwrap_or(top)
}

impl PageBlocked {
    pub(in crate::windows_app) fn record(&self, top: &str) {
        let mut page = self
            .page
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let top = page_identity(top);
        if page.0 != top {
            *page = (top.to_string(), 0);
        }
        page.1 = page.1.saturating_add(1);
    }

    pub(in crate::windows_app) fn count(&self, top: &str) -> u32 {
        let page = self
            .page
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if page.0 == page_identity(top) {
            page.1
        } else {
            0
        }
    }
}

// ===================== o menu do botao direito =====================

/// Id da caixa "Bloquear anuncios em <host>" (ou "Ativar...", ou o
/// cinzento das IAs) nos menus das WebViews.
pub(in crate::windows_app) const ADBLOCK_MENU_SITE: usize = 2;
/// Id do "Desativar bloqueio de anuncios".
pub(in crate::windows_app) const ADBLOCK_MENU_OFF: usize = 3;

pub(in crate::windows_app) const LABEL_ADBLOCK_ACTIVATE: &str = "Ativar bloqueio de anúncios";
pub(in crate::windows_app) const LABEL_ADBLOCK_PREPARING: &str =
    "Bloqueio de anúncios · a preparar a lista…";
pub(in crate::windows_app) const LABEL_ADBLOCK_ALWAYS_OFF: &str =
    "Bloqueio de anúncios · Sempre desligado nas páginas das IAs";
pub(in crate::windows_app) const LABEL_ADBLOCK_DEACTIVATE: &str = "Desativar bloqueio de anúncios";

/// O que o bloqueio poe no menu desta pagina.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum AdblockMenu {
    /// Nao e uma pagina web (ou o hospedeiro nao tem bloqueio).
    Hidden,
    /// Desligado: "Ativar bloqueio de anuncios".
    Activate,
    /// Ligado, a preparar a lista.
    Preparing,
    /// Uma pagina de IA ou de login.
    AlwaysOff,
    /// A caixa deste site.
    Site {
        site: String,
        blocking: bool,
        blocked: u32,
    },
}

/// A decisao do menu para a pagina `page` (o `Source` do WebView no
/// instante do botao direito), pura.
pub(in crate::windows_app) fn adblock_menu(
    view: &AdblockView,
    page: Option<&str>,
    blocked: u32,
) -> AdblockMenu {
    let Some(page) = page
        .and_then(|page| Url::parse(page.trim()).ok())
        .filter(|page| matches!(page.scheme(), "http" | "https"))
    else {
        return AdblockMenu::Hidden;
    };
    match view {
        AdblockView::Off => AdblockMenu::Activate,
        _ if page_is_always_exempt(&page) => AdblockMenu::AlwaysOff,
        AdblockView::Preparing => AdblockMenu::Preparing,
        // Uma pagina num IP ou em `localhost`: a escolha por site nao se
        // guardaria, por isso o bloqueio nao poe nada no menu.
        AdblockView::Active(rules) => match site_key(&page).filter(|site| is_storable_site(site)) {
            Some(site) => AdblockMenu::Site {
                blocking: !rules.allow_sites().contains(&site),
                blocked,
                site,
            },
            None => AdblockMenu::Hidden,
        },
    }
}

/// "12 bloqueados", "1 bloqueado".
fn blocked_count_label(count: u32) -> String {
    if count == 1 {
        "1 bloqueado".to_string()
    } else {
        format!("{} bloqueados", group_thousands(u64::from(count)))
    }
}

/// `4512` como "4 512".
pub(in crate::windows_app) fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(digit);
    }
    out
}

/// A caixa do bloqueio no menu, conforme o estado.
pub(in crate::windows_app) fn adblock_site_item(menu: &AdblockMenu) -> Option<MenuItemView> {
    Some(match menu {
        AdblockMenu::Hidden => return None,
        AdblockMenu::Activate => MenuItemView {
            label: LABEL_ADBLOCK_ACTIVATE.to_string(),
            checked: None,
            enabled: true,
            action: MenuAction::Adblock(AdblockAction::Activate),
        },
        AdblockMenu::Preparing => MenuItemView {
            label: LABEL_ADBLOCK_PREPARING.to_string(),
            checked: None,
            enabled: false,
            action: MenuAction::None,
        },
        AdblockMenu::AlwaysOff => MenuItemView {
            label: LABEL_ADBLOCK_ALWAYS_OFF.to_string(),
            checked: Some(false),
            enabled: false,
            action: MenuAction::None,
        },
        AdblockMenu::Site {
            site,
            blocking,
            blocked,
        } => MenuItemView {
            label: if *blocking {
                format!(
                    "Bloquear anúncios em {site} ({})",
                    blocked_count_label(*blocked)
                )
            } else {
                format!("Bloquear anúncios em {site}")
            },
            checked: Some(*blocking),
            enabled: true,
            action: MenuAction::Adblock(AdblockAction::SetSite {
                site: site.clone(),
                block: !*blocking,
            }),
        },
    })
}

/// "Desativar bloqueio de anuncios": so com o bloqueio ligado.
pub(in crate::windows_app) fn adblock_off_item(menu: &AdblockMenu) -> Option<MenuItemView> {
    match menu {
        AdblockMenu::Hidden | AdblockMenu::Activate => None,
        AdblockMenu::Preparing | AdblockMenu::AlwaysOff | AdblockMenu::Site { .. } => {
            Some(MenuItemView {
                label: LABEL_ADBLOCK_DEACTIVATE.to_string(),
                checked: None,
                enabled: true,
                action: MenuAction::Adblock(AdblockAction::Deactivate),
            })
        }
    }
}

/// O que o menu le do bloqueio de UMA WebView: o estado partilhado e o
/// contador dessa WebView.
#[derive(Debug, Clone)]
pub(in crate::windows_app) struct AdblockMenuSource {
    pub(in crate::windows_app) shared: Arc<AdblockShared>,
    pub(in crate::windows_app) blocked: Arc<PageBlocked>,
}

impl AdblockMenuSource {
    /// O menu para a pagina que o WebView mostra no instante do pedido.
    pub(in crate::windows_app) fn menu(&self, page: Option<&str>) -> AdblockMenu {
        let blocked = page.map_or(0, |page| self.blocked.count(page));
        adblock_menu(&self.shared.view(), page, blocked)
    }
}

/// O que um item do bloqueio faz quando escolhido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum AdblockAction {
    Activate,
    Deactivate,
    SetSite { site: String, block: bool },
}

impl AdblockAction {
    pub(in crate::windows_app) fn event(&self) -> AdblockEvent {
        match self {
            AdblockAction::Activate => AdblockEvent::Activate,
            AdblockAction::Deactivate => AdblockEvent::Deactivate,
            AdblockAction::SetSite { site, block } => AdblockEvent::SetSite {
                site: site.clone(),
                block: *block,
            },
        }
    }
}

// ===================== os eventos e o estado do app =====================

/// Uma lista pronta a usar: quando foi baixada e os dominios.
#[derive(Debug, Clone)]
pub(in crate::windows_app) struct AdblockList {
    pub(in crate::windows_app) fetched_ms: u64,
    pub(in crate::windows_app) domains: Arc<DomainSet>,
}

/// O que chega ao event loop do bloqueio: os itens do menu e as threads
/// `neural-adblock`.
#[derive(Debug)]
pub(in crate::windows_app) enum AdblockEvent {
    /// "Ativar bloqueio de anuncios".
    Activate,
    /// "Desativar bloqueio de anuncios".
    Deactivate,
    /// A caixa de um site: `block` e o estado novo.
    SetSite { site: String, block: bool },
    /// A lista guardada, lida no arranque (`None`: nenhuma que sirva).
    Loaded(Option<AdblockList>),
    /// Fim de um download: `activation` diz se foi o clique em "Ativar".
    Downloaded {
        result: Result<AdblockList, String>,
        activation: bool,
    },
}

/// A escolha do utilizador, a lista em memoria e os downloads.
pub(in crate::windows_app) struct AdblockState {
    pub(in crate::windows_app) shared: Arc<AdblockShared>,
    settings: AdblockSettings,
    settings_store: Option<VersionedJsonStore<AdblockSettings>>,
    list: Option<AdblockList>,
    /// Um download (ativacao ou renovacao) esta a correr.
    downloading: bool,
    /// O clique em "Ativar" espera pelo download.
    activating: bool,
    /// A lista guardada esta a ser lida (arranque).
    loading: bool,
    last_failure_ms: Option<u64>,
    /// A anti-distracao (`distraction.rs`): a politica gravada (campo
    /// `distraction` das `settings`) e as escolhas do Split privado, como
    /// os handlers as leem.
    pub(in crate::windows_app) distraction: Arc<DistractionShared>,
    /// As escolhas «Ocultar distrações neste site» feitas no Split privado:
    /// so em memoria, NUNCA gravadas (gate
    /// `a_toggle_in_private_is_never_written`).
    distraction_private: BTreeMap<String, bool>,
}

/// Agora, em ms desde 1970.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// A lista guardada, lida pelo seu grant; `None` sem ficheiro, estragada
/// ou vazia.
fn read_stored_list(grant: StoreGrant) -> Option<AdblockList> {
    let mut store =
        VersionedJsonStore::<StoredList>::open(grant, STORED_LIST_VERSION, STORED_LIST_MAX_BYTES)
            .ok()?;
    let stored = store.load().into_value();
    if stored.is_empty() {
        return None;
    }
    let parsed = stored.parse();
    (!parsed.domains.is_empty()).then(|| AdblockList {
        fetched_ms: stored.fetched_ms,
        domains: Arc::new(parsed.domains),
    })
}

/// Um download, fora da thread da interface. No clique em "Ativar" usa a
/// lista guardada se ela ainda serve (menos de uma semana); senao baixa-a
/// pela politica do transporte (`ListClient`), valida e grava pelo grant.
/// No modo privado nunca baixa (o grant `Automatic` nao escreve).
fn download_job(grant: StoreGrant, activation: bool) -> Result<AdblockList, String> {
    let now = now_ms();
    let private = !grant.writes_allowed();
    let mut store =
        VersionedJsonStore::<StoredList>::open(grant, STORED_LIST_VERSION, STORED_LIST_MAX_BYTES)
            .map_err(|error| error.to_string())?;
    if activation {
        let stored = store.load().into_value();
        if !stored.is_empty() && !download_on_activation(now, Some(stored.fetched_ms), private) {
            let parsed = stored.parse();
            if !parsed.domains.is_empty() {
                return Ok(AdblockList {
                    fetched_ms: stored.fetched_ms,
                    domains: Arc::new(parsed.domains),
                });
            }
        }
    }
    if private {
        return Err("modo privado".to_string());
    }
    let parsed = ListClient::new(ListEndpoint::pinned())
        .fetch(&|| false)
        .map_err(|error| error.pt_br_message())?;
    let list = AdblockList {
        fetched_ms: now,
        domains: Arc::new(parsed.domains),
    };
    if let Err(error) = store.save(&StoredList::from_set(&list.domains, now)) {
        debug_log(format_args!("adblock: a lista nao foi gravada ({error})"));
    }
    Ok(list)
}

/// Corre `job` numa thread `neural-adblock` e manda o evento que ela
/// devolve ao event loop. `false` se a thread nao arrancou.
fn spawn_adblock_job(
    job: impl FnOnce() -> AdblockEvent + Send + 'static,
    proxy: EventLoopProxy<UserEvent>,
) -> bool {
    std::thread::Builder::new()
        .name("neural-adblock".into())
        .spawn(move || {
            let event = job();
            let _ = proxy.send_event(UserEvent::Adblock(event));
        })
        .map_err(|error| debug_log(format_args!("adblock: a thread nao arrancou ({error})")))
        .is_ok()
}

/// A superficie da renovacao: a Home nunca, o Leitor, o PDF e os livros
/// tambem nao; o comparador e a Web completa sim.
pub(in crate::windows_app) fn adblock_refresh_surface(surface: Surface) -> RefreshSurface {
    match surface {
        Surface::Home => RefreshSurface::Home,
        Surface::Comparator | Surface::External => RefreshSurface::Web,
        Surface::Reader | Surface::Pdf | Surface::Epub => RefreshSurface::Local,
    }
}

impl AdblockState {
    /// No arranque: le a escolha (um ficheiro pequeno, como o tema) e, so
    /// com o bloqueio ligado, manda ler a lista guardada numa thread.
    /// Desligado -- o caso de quem nunca clicou em "Ativar" -- nada mais.
    pub(in crate::windows_app) fn open(
        stores: Option<&StoreRegistry>,
        proxy: &EventLoopProxy<UserEvent>,
    ) -> Self {
        let mut state = Self::load(stores);
        if state.settings.enabled {
            state.shared.set_filtering(true);
            if let Some(grant) = stores.and_then(|registry| registry.grant(ADBLOCK_LIST_STORE).ok())
            {
                state.loading = spawn_adblock_job(
                    move || AdblockEvent::Loaded(read_stored_list(grant)),
                    proxy.clone(),
                );
            }
        }
        state.publish();
        state
    }

    /// A escolha gravada (`adblock-settings.json`, pelo grant `Setting`),
    /// sem threads: o que `open` faz antes de ler a lista.
    pub(in crate::windows_app) fn load(stores: Option<&StoreRegistry>) -> Self {
        let mut settings_store = stores
            .and_then(|registry| registry.grant(ADBLOCK_SETTINGS_STORE).ok())
            .and_then(|grant| {
                VersionedJsonStore::<AdblockSettings>::open(
                    grant,
                    SETTINGS_VERSION,
                    SETTINGS_MAX_BYTES,
                )
                .ok()
            });
        let settings = settings_store
            .as_mut()
            .map(|store| store.load().into_value().sanitized())
            .unwrap_or_default();
        let distraction = Arc::new(DistractionShared::new(settings.distraction.clone()));
        Self {
            shared: Arc::new(AdblockShared::default()),
            settings,
            settings_store,
            list: None,
            downloading: false,
            activating: false,
            loading: false,
            last_failure_ms: None,
            distraction,
            distraction_private: BTreeMap::new(),
        }
    }

    /// «Ocultar distrações neste site» (`distraction.rs`). Fora do Split
    /// privado, a escolha muda a politica e grava-se no
    /// `adblock-settings.json`; no Split privado fica so em memoria, por
    /// cima da gravada, e o ficheiro nunca a ve.
    pub(in crate::windows_app) fn set_distraction_site(
        &mut self,
        site: &str,
        on: bool,
        private: bool,
    ) -> DistractionToggle {
        let Some(site) = distraction_site_key(site) else {
            return DistractionToggle::Refused;
        };
        let outcome = if private {
            let mut effective = self
                .settings
                .distraction
                .with_overlay(&self.distraction_private);
            match effective.set_site(&site, on) {
                SiteChange::Changed
                    if self.distraction_private.len() < MAX_DISTRACTION_SITES
                        || self.distraction_private.contains_key(&site) =>
                {
                    self.distraction_private.insert(site, on);
                    DistractionToggle::MemoryOnly
                }
                SiteChange::Unchanged => DistractionToggle::Unchanged,
                SiteChange::Changed | SiteChange::Refused => DistractionToggle::Refused,
            }
        } else {
            match self.settings.distraction.set_site(&site, on) {
                SiteChange::Changed => {
                    self.save_settings();
                    DistractionToggle::Saved
                }
                SiteChange::Unchanged => DistractionToggle::Unchanged,
                SiteChange::Refused => DistractionToggle::Refused,
            }
        };
        self.distraction
            .publish(&self.settings.distraction, &self.distraction_private);
        outcome
    }

    /// A politica gravada da anti-distracao (a do ficheiro).
    #[cfg(test)]
    pub(in crate::windows_app) fn distraction_policy(&self) -> &DistractionPolicy {
        &self.settings.distraction
    }

    /// As regras da lista em memoria com os sites permitidos de agora.
    fn rules(&self) -> Option<Arc<AdblockRules>> {
        self.list.as_ref().map(|list| {
            Arc::new(AdblockRules::new(
                Arc::clone(&list.domains),
                self.settings.allow_sites.clone(),
            ))
        })
    }

    /// Poe no estado partilhado o que a escolha e a lista dizem.
    fn publish(&self) {
        let view = if self.settings.enabled {
            self.rules()
                .map_or(AdblockView::Preparing, AdblockView::Active)
        } else if self.activating {
            AdblockView::Preparing
        } else {
            AdblockView::Off
        };
        self.shared.set_view(view);
    }

    fn save_settings(&mut self) {
        let Some(store) = self.settings_store.as_mut() else {
            return;
        };
        if let Err(error) = store.save(&self.settings) {
            debug_log(format_args!("adblock: escolha nao gravada ({error})"));
        }
    }
}

impl App {
    /// O unico braco do bloqueio no `user_event`.
    pub(in crate::windows_app) fn adblock_event(&mut self, event: AdblockEvent) {
        match event {
            AdblockEvent::Activate => self.adblock_activate(),
            AdblockEvent::Deactivate => self.adblock_deactivate(),
            AdblockEvent::SetSite { site, block } => self.adblock_set_site(&site, block),
            AdblockEvent::Loaded(list) => self.adblock_loaded(list),
            AdblockEvent::Downloaded { result, activation } => {
                self.adblock_downloaded(result, activation)
            }
        }
    }

    fn adblock_list_grant(&self) -> Option<StoreGrant> {
        self.stores.as_ref()?.grant(ADBLOCK_LIST_STORE).ok()
    }

    /// O clique em "Ativar": a lista guardada se ainda serve, senao o
    /// download. O bloqueio so fica ligado quando a lista chega.
    fn adblock_activate(&mut self) {
        if self.adblock.settings.enabled || self.adblock.activating {
            return;
        }
        let Some(grant) = self.adblock_list_grant() else {
            self.show_splash(
                "Bloqueio de anúncios indisponível: sem pasta de dados.".to_string(),
                4,
            );
            return;
        };
        self.adblock.activating = true;
        self.adblock.downloading = true;
        self.adblock.publish();
        let started = spawn_adblock_job(
            move || AdblockEvent::Downloaded {
                result: download_job(grant, true),
                activation: true,
            },
            self.proxy.clone(),
        );
        if started {
            self.show_splash(
                "A preparar o bloqueio de anúncios (lista de Peter Lowe)…".to_string(),
                4,
            );
        } else {
            self.adblock.activating = false;
            self.adblock.downloading = false;
            self.adblock.publish();
        }
    }

    fn adblock_deactivate(&mut self) {
        self.adblock.activating = false;
        if self.adblock.settings.enabled {
            self.adblock.settings.enabled = false;
            self.adblock.save_settings();
        }
        self.adblock.list = None;
        self.adblock.shared.set_filtering(false);
        self.adblock.publish();
        self.adblock_sync_filters(false);
        self.show_splash("Bloqueio de anúncios desativado".to_string(), 3);
    }

    fn adblock_set_site(&mut self, site: &str, block: bool) {
        if !self.adblock.settings.set_site_blocking(site, block) {
            return;
        }
        self.adblock.save_settings();
        self.adblock.publish();
        let message = if block {
            format!("Anúncios bloqueados em {site}")
        } else {
            format!("Anúncios permitidos em {site} · recarregue a página")
        };
        self.show_splash(message, 3);
    }

    fn adblock_loaded(&mut self, list: Option<AdblockList>) {
        self.adblock.loading = false;
        if !self.adblock.settings.enabled {
            return;
        }
        if self.adblock.list.is_none() {
            self.adblock.list = list;
        }
        self.adblock.publish();
    }

    fn adblock_downloaded(&mut self, result: Result<AdblockList, String>, activation: bool) {
        self.adblock.downloading = false;
        match result {
            Ok(list) => {
                self.adblock.last_failure_ms = None;
                if activation {
                    // Desativado enquanto baixava: a lista fica gravada, o
                    // bloqueio nao liga.
                    if !self.adblock.activating {
                        return;
                    }
                    let count = list.domains.len() as u64;
                    self.adblock.activating = false;
                    self.adblock.settings.enabled = true;
                    self.adblock.save_settings();
                    self.adblock.list = Some(list);
                    self.adblock.shared.set_filtering(true);
                    self.adblock.publish();
                    self.adblock_sync_filters(true);
                    self.show_splash(
                        format!("Bloqueio ativo · {} domínios", group_thousands(count)),
                        4,
                    );
                } else if self.adblock.settings.enabled {
                    self.adblock.list = Some(list);
                    self.adblock.publish();
                }
            }
            Err(message) => {
                self.adblock.last_failure_ms = Some(now_ms());
                if activation && self.adblock.activating {
                    self.adblock.activating = false;
                    self.adblock.publish();
                    self.show_splash(
                        format!("Não foi possível baixar a lista de anúncios: {message}"),
                        5,
                    );
                } else {
                    debug_log(format_args!("adblock: renovacao falhou ({message})"));
                }
            }
        }
    }

    /// Uma pagina acabou de carregar: numa coluna, na fonte ao lado ou na
    /// Web completa, com o bloqueio ligado e a lista com mais de uma
    /// semana, renova-a (`refresh_due`: nunca na Home, nunca no modo
    /// privado).
    pub(in crate::windows_app) fn adblock_page_loaded(&mut self, page: WebViewHost) {
        if !adblock_host(page) || !self.adblock.settings.enabled {
            return;
        }
        let Some(grant) = self.adblock_list_grant() else {
            return;
        };
        let state = RefreshState {
            enabled: self.adblock.settings.enabled,
            fetched_ms: self.adblock.list.as_ref().map(|list| list.fetched_ms),
            in_flight: self.adblock.downloading || self.adblock.activating || self.adblock.loading,
            last_failure_ms: self.adblock.last_failure_ms,
            private: !grant.writes_allowed(),
        };
        if !refresh_due(now_ms(), adblock_refresh_surface(self.surface), &state) {
            return;
        }
        self.adblock.downloading = spawn_adblock_job(
            move || AdblockEvent::Downloaded {
                result: download_job(grant, false),
                activation: false,
            },
            self.proxy.clone(),
        );
    }

    /// Liga ou desliga o filtro `*` nas WebViews abertas onde o bloqueio
    /// vale: as colunas, a fonte ao lado (nao a privada) e a Web completa.
    /// As que nascerem depois recebem-no (ou nao) no registo.
    fn adblock_sync_filters(&self, on: bool) {
        let mut views: Vec<&WebView> = Vec::new();
        if let Some(comparator) = &self.comparator {
            views.extend(comparator.views.iter().map(|view| &view.webview));
            if let Some(split) = comparator.split.as_ref().filter(|split| !split.private) {
                views.push(&split.webview);
            }
        }
        if self.surface == Surface::External
            && let Some(webview) = &self.webview
        {
            views.push(webview);
        }
        for webview in views {
            if let Err(error) = set_resource_filter(webview, on) {
                debug_log(format_args!("adblock: filtro nao mudou ({error})"));
            }
        }
    }
}
