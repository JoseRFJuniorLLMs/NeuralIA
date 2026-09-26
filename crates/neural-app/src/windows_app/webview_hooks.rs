use super::*;

use std::sync::RwLock;

use neural_core::adblock::AdblockRules;

use wry::PageLoadEvent;

// ===================== os ganchos de cada WebView (infra-webview-hooks) =====================
//
// Cada WebView que a app constroi nasce em dois passos, e os dois passam por
// aqui e so por aqui:
//
// - `App::hooked_builder` (antes do `build`): a metade que o `WebViewBuilder`
//   do wry aceita -- a trava de navegacao (`web_navigation_verdict`, a cadeia
//   do hospedeiro), a recusa de downloads nas paginas locais e o aviso de
//   pagina carregada (`WebViewEvent::PageLoaded`);
// - `App::install_webview_hooks` (depois do `build`, privada ao modulo): a
//   metade que so o COM do WebView2 da -- os itens do NeuralIA no menu do
//   botao direito (`WEBVIEW_MENU_ITEMS`), o `AcceleratorKeyPressed` de cada
//   WebView, o gestor de downloads (`downloads.rs`) nas paginas da internet
//   e o `WebResourceRequested` do bloqueio de anuncios (`register_resource_gate`)
//   nas colunas, na fonte ao lado e na Web completa.
//
// As duas metades sao uma so chamada para quem constroi: `hooked_builder`
// devolve um `HookedBuilder` com o hospedeiro que recebeu, e o `build` do
// wry so se chama dentro dele (`build_hooked`, `build_hooked_as_child`),
// que regista a metade do COM na WebView que sai. Um sitio nao consegue
// construir uma WebView sem a cadeia de navegacao, nem dar a cadeia de um
// hospedeiro ao builder e o menu de outro ao COM.
//
// O que cada hospedeiro recebe esta numa tabela pura, `webview_hooks(host)`,
// que os gates leem sem janela. Uma feature nova (bloqueio de anuncios,
// distracoes, gestor de downloads, atalhos) preenche a sua casa na tabela
// ou o seu slot (`accelerator_lookup`, `resource_gate_answers`); nenhuma
// chama o COM a partir de um builder.
//
// Aceleradores: o spike de CI (infra-accel-spike) mediu que um
// `AcceleratorKeyPressed` com `Handled = TRUE` guarda cada atalho nativo da
// pagina e do `act()` do mapa de teclas nos nove hospedeiros, e dispara uma
// so vez com a tecla presa -- o despacho nativo e o desenho, sem fallback.
// A pagina continua a receber um `keypress` (um ou dois por toque, o
// caractere de controlo de um Ctrl+letra): nao chega ao `keydown` que o
// mapa de teclas e as paginas ouvem, e nao faz diferenca para os ganchos.
// O spike prendia tambem a subida e nao ouvia o `keyup`; a decisao que
// embarca deixa a subida passar, por isso a pagina recebe tambem o `keyup`
// de um atalho preso (o que o spike nao mediu).
// O `accelerator_lookup` e a decisao do mapa de teclas (`keymap.rs`,
// infra-commands-keymap) com o hospedeiro como origem; hoje so o Ctrl+J
// dos Downloads (downloads-ui, `Global`) e preso nas WebViews (cada atalho
// chega no PR do seu comando), e o `Handled` so muda quando ela prende a
// tecla.

/// Que WebView e esta: quem decide o que ela recebe da tabela e de onde vem
/// um atalho ou um item de menu (a origem e o hospedeiro, nunca a pagina).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum WebViewHost {
    /// Uma das colunas das IAs no comparador.
    Column(usize),
    /// A fonte aberta ao lado da coluna indicada -- tambem a unica pagina a
    /// vista em tela cheia.
    Split(usize),
    /// A resposta de uma IA pedida no painel privado, ao lado da coluna
    /// indicada: perfil InPrivate.
    PrivateSplit(usize),
    /// A Web completa (um link externo, e a pagina do agente).
    External,
    /// O Modo Leitura: HTML local montado do artigo.
    Reader,
    /// O nosso visualizador de PDF, na origem `neuralia-pdf`.
    Pdf,
    /// A biblioteca e o leitor de livros, na origem `neuralia-epub`.
    Epub,
    /// O painel do Gemini Live, na origem `neuralia-live`.
    Live,
    /// O monitor escondido do Gmail (1x1, fora do ecra, nunca com o foco).
    GmailMonitor,
    /// Historico, memoria e notas, a direita.
    SidePanel,
    /// Meet, WhatsApp, YouTube, Gmail e a Respiracao no painel.
    Service(Service),
}

impl WebViewHost {
    /// Um representante de cada tipo de hospedeiro (os indexados com a
    /// coluna 0; o servico com o YouTube). E o que os gates percorrem.
    #[cfg(test)]
    pub(in crate::windows_app) const ALL: [WebViewHost; 11] = [
        WebViewHost::Column(0),
        WebViewHost::Split(0),
        WebViewHost::PrivateSplit(0),
        WebViewHost::External,
        WebViewHost::Reader,
        WebViewHost::Pdf,
        WebViewHost::Epub,
        WebViewHost::Live,
        WebViewHost::GmailMonitor,
        WebViewHost::SidePanel,
        WebViewHost::Service(Service::YouTube),
    ];

    /// A fonte ao lado da coluna `source_index`, privada ou nao.
    pub(in crate::windows_app) fn split(source_index: usize, private: bool) -> WebViewHost {
        if private {
            WebViewHost::PrivateSplit(source_index)
        } else {
            WebViewHost::Split(source_index)
        }
    }

    /// O nome do tipo, sem o indice: o que os gates usam.
    #[cfg(test)]
    pub(in crate::windows_app) fn kind(self) -> &'static str {
        match self {
            WebViewHost::Column(_) => "Column",
            WebViewHost::Split(_) => "Split",
            WebViewHost::PrivateSplit(_) => "PrivateSplit",
            WebViewHost::External => "External",
            WebViewHost::Reader => "Reader",
            WebViewHost::Pdf => "Pdf",
            WebViewHost::Epub => "Epub",
            WebViewHost::Live => "Live",
            WebViewHost::GmailMonitor => "GmailMonitor",
            WebViewHost::SidePanel => "SidePanel",
            WebViewHost::Service(_) => "Service",
        }
    }

    /// O hospedeiro para uma linha de log, com o indice ou o servico.
    pub(in crate::windows_app) fn describe(self) -> String {
        match self {
            WebViewHost::Column(index) => format!("coluna {index}"),
            WebViewHost::Split(index) => format!("fonte {index}"),
            WebViewHost::PrivateSplit(index) => format!("fonte privada {index}"),
            WebViewHost::External => "web".to_string(),
            WebViewHost::Reader => "leitor".to_string(),
            WebViewHost::Pdf => "pdf".to_string(),
            WebViewHost::Epub => "livros".to_string(),
            WebViewHost::Live => "live".to_string(),
            WebViewHost::GmailMonitor => "monitor do gmail".to_string(),
            WebViewHost::SidePanel => "painel".to_string(),
            WebViewHost::Service(service) => format!("servico {service:?}"),
        }
    }
}

// ===================== a tabela =====================

/// O que o WebView faz com um download que a pagina comeca.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum DownloadPolicy {
    /// O gestor de downloads (`downloads.rs`) decide cada um: recusa
    /// programas e disfarces e marca o ficheiro acabado com a marca da Web.
    Managed,
    /// Recusado antes de comecar: uma pagina local nossa nao descarrega
    /// nada, e o monitor do Gmail, que ninguem ve, tambem nao.
    Deny,
}

/// O que o despachante de recursos (`resource_gate_answers`) faz com os
/// pedidos deste hospedeiro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum ResourceGatePolicy {
    /// Nenhum pedido e respondido pelo NeuralIA (e nenhum
    /// `WebResourceRequested` e registado).
    Open,
    /// O bloqueio de anuncios (`adblock.rs`): as colunas, a fonte ao lado
    /// e a Web completa. A fonte privada nao.
    Adblock,
}

/// A politica de recursos de um hospedeiro (a casa `resource_gate` da
/// tabela), sem montar a linha inteira: o despachante le-a a cada pedido.
pub(in crate::windows_app) fn resource_gate_policy(host: WebViewHost) -> ResourceGatePolicy {
    if adblock_host(host) {
        ResourceGatePolicy::Adblock
    } else {
        ResourceGatePolicy::Open
    }
}

/// A cadeia de navegacao de cada hospedeiro: qual das trava de sempre
/// decide as navegacoes de topo dele (`web_navigation_verdict`). A tabela
/// diz a cadeia; o builder so acrescenta a origem local que o utilizador
/// autorizou, quando ha uma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum NavGate {
    /// As colunas, as fontes ao lado e a Web completa: `neuralia:` nunca
    /// navega; a internet, e a origem local autorizada, sim; `view-source:`
    /// pela mesma regra.
    Web,
    /// O visualizador de PDF: a propria origem; um link da internet sai
    /// para a Web completa; nada mais.
    Pdf,
    /// O Modo Leitura: so `about:blank` e as accoes `neuralia:` do artigo.
    Reader,
    /// Os livros: a biblioteca e o leitor, na origem `neuralia-epub`.
    Epub,
    /// O Gemini Live: so a pagina do painel.
    Live,
    /// O monitor do Gmail: so o Gmail e o login da Google, em https.
    Gmail,
    /// O painel lateral: so o proprio HTML local.
    SidePanel,
    /// Um servico: a politica dele (`service_panel_navigation`).
    Service(Service),
}

/// A linha da tabela de um hospedeiro: cada slot que os ganchos ligam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct WebViewHooks {
    /// Os itens do NeuralIA no menu do botao direito (ids de
    /// `WEBVIEW_MENU_ITEMS`); vazio deixa o menu nativo como o WebView2 o
    /// traz.
    pub(in crate::windows_app) menu: Vec<usize>,
    pub(in crate::windows_app) downloads: DownloadPolicy,
    pub(in crate::windows_app) resource_gate: ResourceGatePolicy,
    pub(in crate::windows_app) nav_gate: NavGate,
    /// O `AcceleratorKeyPressed` de `accelerator_lookup`: em todas.
    pub(in crate::windows_app) accelerators: bool,
    /// O script contra distracoes a injetar no documento de topo. Vazio
    /// ate anti-distracao o trazer (com o sim do dono, §7).
    pub(in crate::windows_app) distraction: Option<&'static str>,
}

/// A tabela: o que cada hospedeiro recebe. Pura, para os gates a lerem sem
/// janela.
pub(in crate::windows_app) fn webview_hooks(host: WebViewHost) -> WebViewHooks {
    let (downloads, nav_gate) = match host {
        WebViewHost::Column(_)
        | WebViewHost::Split(_)
        | WebViewHost::PrivateSplit(_)
        | WebViewHost::External => (DownloadPolicy::Managed, NavGate::Web),
        WebViewHost::Reader => (DownloadPolicy::Deny, NavGate::Reader),
        WebViewHost::Pdf => (DownloadPolicy::Deny, NavGate::Pdf),
        WebViewHost::Epub => (DownloadPolicy::Deny, NavGate::Epub),
        WebViewHost::Live => (DownloadPolicy::Deny, NavGate::Live),
        WebViewHost::GmailMonitor => (DownloadPolicy::Deny, NavGate::Gmail),
        WebViewHost::SidePanel => (DownloadPolicy::Deny, NavGate::SidePanel),
        WebViewHost::Service(service) => (DownloadPolicy::Managed, NavGate::Service(service)),
    };
    WebViewHooks {
        menu: webview_menu_items(host)
            .into_iter()
            .map(|item| item.id)
            .collect(),
        downloads,
        resource_gate: resource_gate_policy(host),
        nav_gate,
        accelerators: true,
        distraction: None,
    }
}

// ===================== a metade do builder =====================

/// O que `hook_webview_builder` chama no builder, com os nomes do wry. O
/// produto passa o `WebViewBuilder`; o gate passa um registo e chama os
/// handlers que o WebView2 receberia.
pub(in crate::windows_app) trait HookedWebViewBuilder: Sized {
    fn with_navigation_handler(self, handler: impl Fn(String) -> bool + 'static) -> Self;
    fn with_download_started_handler(
        self,
        handler: impl FnMut(String, &mut PathBuf) -> bool + 'static,
    ) -> Self;
    fn with_on_page_load_handler(self, handler: impl Fn(PageLoadEvent, String) + 'static) -> Self;
}

impl HookedWebViewBuilder for WebViewBuilder<'_> {
    fn with_navigation_handler(self, handler: impl Fn(String) -> bool + 'static) -> Self {
        WebViewBuilder::with_navigation_handler(self, handler)
    }
    fn with_download_started_handler(
        self,
        handler: impl FnMut(String, &mut PathBuf) -> bool + 'static,
    ) -> Self {
        WebViewBuilder::with_download_started_handler(self, handler)
    }
    fn with_on_page_load_handler(self, handler: impl Fn(PageLoadEvent, String) + 'static) -> Self {
        WebViewBuilder::with_on_page_load_handler(self, handler)
    }
}

/// A metade do builder da tabela: a trava de navegacao do hospedeiro (com
/// a origem local autorizada, se houver), a recusa de downloads onde a
/// tabela manda e o aviso de pagina carregada. `send` e o proxy do event
/// loop no produto e um registo no gate.
pub(in crate::windows_app) fn hook_webview_builder<B, S>(
    builder: B,
    host: WebViewHost,
    local_origin: Option<String>,
    send: S,
) -> B
where
    B: HookedWebViewBuilder,
    S: Fn(UserEvent) + Clone + 'static,
{
    let hooks = webview_hooks(host);
    let builder = builder.with_navigation_handler(webview_navigation(
        hooks.nav_gate,
        local_origin,
        send.clone(),
    ));
    let builder = match hooks.downloads {
        DownloadPolicy::Deny => builder.with_download_started_handler(|_, _| false),
        DownloadPolicy::Managed => builder,
    };
    builder.with_on_page_load_handler(move |event, url| {
        if let Some(event) = page_loaded_event(host, event, url) {
            send(event);
        }
    })
}

/// O handler de navegacao de um hospedeiro, com a assinatura do wry: o
/// veredicto da cadeia, e o evento que um veredicto traz vai para `send`.
pub(in crate::windows_app) fn webview_navigation<S>(
    gate: NavGate,
    local_origin: Option<String>,
    send: S,
) -> impl Fn(String) -> bool
where
    S: Fn(UserEvent) + 'static,
{
    move |target| match web_navigation_verdict(gate, local_origin.as_deref(), &target) {
        NavVerdict::Allow => true,
        NavVerdict::Deny => false,
        NavVerdict::DenyWith(event) => {
            send(event);
            false
        }
    }
}

/// So o fim do carregamento vira evento; o inicio nao interessa a ninguem.
pub(in crate::windows_app) fn page_loaded_event(
    host: WebViewHost,
    event: PageLoadEvent,
    url: String,
) -> Option<UserEvent> {
    match event {
        PageLoadEvent::Finished => Some(UserEvent::WebView(WebViewEvent::PageLoaded {
            page: host,
            url,
        })),
        PageLoadEvent::Started => None,
    }
}

// ===================== a cadeia de navegacao (NavGate) =====================

/// O que a cadeia decide para uma navegacao de topo.
#[derive(Debug)]
pub(in crate::windows_app) enum NavVerdict {
    Allow,
    Deny,
    /// Recusada, e o lado nativo faz isto em vez dela (um link do PDF abre
    /// na Web completa; uma accao `neuralia:` do Leitor).
    DenyWith(UserEvent),
}

fn allow_if(allowed: bool) -> NavVerdict {
    if allowed {
        NavVerdict::Allow
    } else {
        NavVerdict::Deny
    }
}

/// `neuralia:` em qualquer caixa: uma accao pedida por navegacao, nunca uma
/// navegacao.
fn is_neuralia_scheme(target: &str) -> bool {
    target
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("neuralia:"))
}

/// A cadeia de cada hospedeiro, elo a elo, na ordem em que sempre correu
/// nos builders: os veredictos sao os de antes, byte a byte (gate
/// `navigation_verdicts_are_the_ones_the_builders_gave`). `local_origin`
/// so conta para `NavGate::Web`.
pub(in crate::windows_app) fn web_navigation_verdict(
    gate: NavGate,
    local_origin: Option<&str>,
    target: &str,
) -> NavVerdict {
    match gate {
        NavGate::Web => {
            if is_neuralia_scheme(target) {
                return NavVerdict::Deny;
            }
            allow_if(
                remote_web_target(target, local_origin)
                    || is_view_source_target(target, local_origin),
            )
        }
        NavGate::Pdf => {
            if is_neuralia_scheme(target) {
                return NavVerdict::Deny;
            }
            if is_pdf_internal_target(target) {
                return NavVerdict::Allow;
            }
            if remote_web_target(target, None) {
                return NavVerdict::DenyWith(UserEvent::OpenExternal(target.to_string()));
            }
            NavVerdict::Deny
        }
        NavGate::Reader => {
            if target.starts_with("about:blank") {
                return NavVerdict::Allow;
            }
            let Ok(action_url) = Url::parse(target) else {
                return NavVerdict::Deny;
            };
            if action_url.scheme() != "neuralia" {
                return NavVerdict::Deny;
            }
            if let Some(event) = neuralia_action(target) {
                return NavVerdict::DenyWith(event);
            }
            match action_url.path().trim_matches('/') {
                "home" => NavVerdict::DenyWith(UserEvent::HomeRequested),
                "web" => {
                    if let Some((_, value)) = action_url.query_pairs().find(|(key, _)| key == "url")
                        && neural_core::validate_web_url(value.as_ref()).is_ok()
                    {
                        NavVerdict::DenyWith(UserEvent::OpenExternal(value.into_owned()))
                    } else {
                        NavVerdict::Deny
                    }
                }
                _ => NavVerdict::Deny,
            }
        }
        NavGate::Epub => allow_if(crate::epub_app::epub_navigation_allowed(target)),
        NavGate::Live => allow_if(crate::gemini_live::live_panel_allows_navigation(target)),
        NavGate::Gmail => {
            if is_neuralia_scheme(target) {
                return NavVerdict::Deny;
            }
            allow_if(Url::parse(target).ok().is_some_and(|url| {
                url.scheme() == "https"
                    && matches!(
                        url.host_str(),
                        Some("mail.google.com") | Some("accounts.google.com")
                    )
            }))
        }
        NavGate::SidePanel => allow_if(panel_allows_navigation(target)),
        NavGate::Service(service) => allow_if(service_panel_navigation(service, target)),
    }
}

// ===================== o despachante de recursos (ResourceGate) =====================

// O bloqueio de anuncios regista o `WebResourceRequested` nos hospedeiros
// com `ResourceGatePolicy::Adblock` (`register_resource_gate`) e o handler
// pergunta aqui, a cada pedido, se o NeuralIA responde ele proprio (um 403
// pelo `CreateWebResourceResponse`). O handler le a pagina e o ambiente do
// `sender` do evento, nunca de uma WebView capturada; e um pedido a um
// esquema proprio do wry nunca e respondido aqui (gate
// `custom_schemes_are_never_answered_by_the_resource_gate`).

/// Os esquemas proprios que o wry serve por `with_custom_protocol`: o PDF, os
/// livros e o Gemini Live. No Windows chegam as paginas como
/// `http://<esquema>.localhost`.
pub(in crate::windows_app) const CUSTOM_SCHEMES: [&str; 3] = [
    "neuralia-pdf",
    crate::epub_app::EPUB_SCHEME,
    crate::gemini_live::LIVE_PROTOCOL,
];

/// Um pedido a um dos esquemas proprios, pelo esquema ou pela origem
/// `<esquema>.localhost` (sem porta) com que o WebView2 o apresenta.
pub(in crate::windows_app) fn is_custom_scheme_request(uri: &str) -> bool {
    let Ok(url) = Url::parse(uri.trim()) else {
        return false;
    };
    CUSTOM_SCHEMES.iter().any(|scheme| {
        url.scheme() == *scheme
            || (matches!(url.scheme(), "http" | "https")
                && url.port().is_none()
                && url
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case(&format!("{scheme}.localhost"))))
    })
}

/// O despachante do `WebResourceRequested`: diz se o NeuralIA responde ele
/// proprio a um pedido deste hospedeiro (um bloqueio) em vez de o deixar
/// seguir. Um pedido a um esquema proprio nunca e respondido por aqui --
/// quem o serve e o protocolo do wry, e uma resposta daqui deixava o PDF,
/// os livros ou o Live sem pagina. `rules` sao as do bloqueio de anuncios
/// em vigor (`None`: desligado ou sem lista ainda).
pub(in crate::windows_app) fn resource_gate_answers(
    host: WebViewHost,
    uri: &str,
    page: ResourcePage<'_>,
    rules: Option<&AdblockRules>,
) -> bool {
    if is_custom_scheme_request(uri) {
        return false;
    }
    match resource_gate_policy(host) {
        ResourceGatePolicy::Open => false,
        ResourceGatePolicy::Adblock => adblock_blocks(uri, page, rules),
    }
}

// ===================== o registo dos itens de menu =====================

/// Id do item de rolagem nos menus de uma coluna: o que o `TrackPopupMenu` da
/// pilula devolve e o que o item acrescentado ao menu do WebView2 entrega.
/// Zero e o "fechou sem escolher" do Win32, por isso nunca e um comando.
pub(in crate::windows_app) const COLUMN_MENU_AUTO_SCROLL: usize = 1;

/// O estado partilhado que os rotulos leem no instante do botao direito.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct MenuFlags {
    pub(in crate::windows_app) auto_scroll: bool,
    /// O bloqueio de anuncios para a pagina deste botao direito.
    pub(in crate::windows_app) adblock: AdblockMenu,
}

/// O que um item faz quando escolhido. Decidido no pedido, com a origem do
/// hospedeiro; nunca vem da pagina.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum MenuAction {
    /// Um comando de coluna: o mesmo despacho do atalho (`column_menu_event`).
    Column { column: usize, command: usize },
    /// Um item do bloqueio de anuncios.
    Adblock(AdblockAction),
    /// Um item cinzento: nada.
    None,
}

impl MenuAction {
    pub(in crate::windows_app) fn event(&self) -> Option<UserEvent> {
        match self {
            MenuAction::Column { column, command } => column_menu_event(*column, *command),
            MenuAction::Adblock(action) => Some(UserEvent::Adblock(action.event())),
            MenuAction::None => None,
        }
    }
}

/// Um item como aparece neste botao direito: o rotulo, a marca (so numa
/// caixa), se se pode escolher e o que faz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct MenuItemView {
    pub(in crate::windows_app) label: String,
    /// `Some`: uma caixa, marcada ou nao.
    pub(in crate::windows_app) checked: Option<bool>,
    pub(in crate::windows_app) enabled: bool,
    pub(in crate::windows_app) action: MenuAction,
}

/// Um item que o NeuralIA acrescenta ao menu nativo do botao direito de uma
/// WebView: o id, os hospedeiros que o recebem e o que mostra e faz em
/// cada pedido (`None`: neste pedido nao aparece).
pub(in crate::windows_app) struct MenuItemSpec {
    pub(in crate::windows_app) id: usize,
    pub(in crate::windows_app) hosts: fn(WebViewHost) -> bool,
    pub(in crate::windows_app) view: fn(WebViewHost, &MenuFlags) -> Option<MenuItemView>,
}

fn auto_scroll_hosts(host: WebViewHost) -> bool {
    context_menu_column(host).is_some()
}

fn auto_scroll_view(host: WebViewHost, flags: &MenuFlags) -> Option<MenuItemView> {
    Some(MenuItemView {
        label: auto_scroll_menu_label(flags.auto_scroll).to_string(),
        checked: None,
        enabled: true,
        action: MenuAction::Column {
            column: context_menu_column(host)?,
            command: COLUMN_MENU_AUTO_SCROLL,
        },
    })
}

fn adblock_site_view(_host: WebViewHost, flags: &MenuFlags) -> Option<MenuItemView> {
    adblock_site_item(&flags.adblock)
}

fn adblock_off_view(_host: WebViewHost, flags: &MenuFlags) -> Option<MenuItemView> {
    adblock_off_item(&flags.adblock)
}

/// O registo: cada item do NeuralIA nos menus das WebViews, ids unicos e
/// nunca zero (gate `context_menu_commands_are_unique`).
pub(in crate::windows_app) const WEBVIEW_MENU_ITEMS: &[MenuItemSpec] = &[
    MenuItemSpec {
        id: COLUMN_MENU_AUTO_SCROLL,
        hosts: auto_scroll_hosts,
        view: auto_scroll_view,
    },
    MenuItemSpec {
        id: ADBLOCK_MENU_SITE,
        hosts: adblock_host,
        view: adblock_site_view,
    },
    MenuItemSpec {
        id: ADBLOCK_MENU_OFF,
        hosts: adblock_host,
        view: adblock_off_view,
    },
];

/// Os itens do registo que este hospedeiro recebe, pela ordem do registo.
pub(in crate::windows_app) fn webview_menu_items(host: WebViewHost) -> Vec<&'static MenuItemSpec> {
    WEBVIEW_MENU_ITEMS
        .iter()
        .filter(|item| (item.hosts)(host))
        .collect()
}

/// O item do registo com este id (o que o gate dos ids unicos procura).
#[cfg(test)]
pub(in crate::windows_app) fn webview_menu_item(id: usize) -> Option<&'static MenuItemSpec> {
    WEBVIEW_MENU_ITEMS.iter().find(|item| item.id == id)
}

/// O item escolhido num menu de coluna vira o evento que o Ctrl+R premido
/// DENTRO dessa coluna produz -- o mesmo despacho, `column_ipc_event_impl`,
/// para o atalho e o menu nunca divergirem.
pub(in crate::windows_app) fn column_menu_event(
    col_index: usize,
    command: usize,
) -> Option<UserEvent> {
    if col_index >= COMPARATOR_COLUMNS {
        return None;
    }
    match command {
        COLUMN_MENU_AUTO_SCROLL => App::column_ipc_event_impl(col_index, IpcAction::AutoScroll),
        _ => None,
    }
}

/// Recebem o item de rolagem as paginas que rolam sozinhas: as colunas das
/// IAs e a fonte aberta ao lado, privada ou nao (o `auto_scroll_tick` rola-a
/// e o Ctrl+R funciona nela -- sem o item, a resposta de uma IA aberta no
/// painel privado rolava sem nenhum botao direito para a parar). O resto
/// nao rola: fica com o menu nativo do WebView2 tal como vem.
pub(in crate::windows_app) fn context_menu_column(host: WebViewHost) -> Option<usize> {
    match host {
        WebViewHost::Column(index)
        | WebViewHost::Split(index)
        | WebViewHost::PrivateSplit(index)
            if index < COMPARATOR_COLUMNS =>
        {
            Some(index)
        }
        _ => None,
    }
}

/// Onde os itens do NeuralIA entram num menu nativo com `native` itens:
/// DEPOIS de todos eles, separados por uma linha quando ha algo acima, um a
/// seguir ao outro. Copiar, colar, inspecionar e o resto ficam nos lugares
/// em que o WebView2 os pos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct MenuPlacement {
    pub(in crate::windows_app) separator_at: Option<u32>,
    pub(in crate::windows_app) first_item_at: u32,
}

pub(in crate::windows_app) fn menu_placement(native: u32) -> MenuPlacement {
    if native == 0 {
        MenuPlacement {
            separator_at: None,
            first_item_at: 0,
        }
    } else {
        MenuPlacement {
            separator_at: Some(native),
            first_item_at: native + 1,
        }
    }
}

/// Um item do NeuralIA num botao direito concreto: o rotulo, a marca e o
/// estado lidos no instante do pedido, o indice em que entra no menu e o
/// que faz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct MenuItemRequest {
    pub(in crate::windows_app) host: WebViewHost,
    pub(in crate::windows_app) id: usize,
    pub(in crate::windows_app) label: String,
    pub(in crate::windows_app) checked: Option<bool>,
    pub(in crate::windows_app) enabled: bool,
    pub(in crate::windows_app) at: u32,
    pub(in crate::windows_app) action: MenuAction,
}

impl MenuItemRequest {
    /// O evento de escolher o item, com a origem do hospedeiro. Um item
    /// cinzento nao faz nada.
    pub(in crate::windows_app) fn selected(&self) -> Option<UserEvent> {
        if !self.enabled {
            return None;
        }
        self.action.event()
    }
}

/// O que UM botao direito acrescenta: o separador (se ha itens nativos e
/// itens nossos) e os itens, por ordem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) struct MenuRequest {
    pub(in crate::windows_app) separator_at: Option<u32>,
    pub(in crate::windows_app) items: Vec<MenuItemRequest>,
}

impl MenuRequest {
    /// O item com este id (o que o `TrackPopupMenu` da pilula devolveu).
    pub(in crate::windows_app) fn item(&self, id: usize) -> Option<&MenuItemRequest> {
        self.items.iter().find(|item| item.id == id)
    }
}

/// O que responde a cada botao direito de uma WebView. Criado UMA vez,
/// quando a WebView e registada (ou quando a pilula abre o menu), e chamado
/// a cada pedido: por isso o rotulo le o `SharedFlag` dentro da resposta,
/// nunca na criacao -- um rotulo lido no registo ficava preso ao estado do
/// arranque. `register_webview_context_menu` e `column_pill_menu` so copiam
/// para o Win32/COM o que isto decide. `page` e o endereco que a WebView
/// mostra no instante do pedido (o `Source` do `sender`); `adblock` e o
/// bloqueio dessa WebView (`None` na pilula e nos hospedeiros sem ele).
pub(in crate::windows_app) fn webview_menu_responder(
    host: WebViewHost,
    auto_scroll: SharedFlag,
    adblock: Option<AdblockMenuSource>,
) -> impl Fn(u32, Option<&str>) -> MenuRequest {
    move |native, page| {
        let flags = MenuFlags {
            auto_scroll: auto_scroll.get(),
            adblock: adblock
                .as_ref()
                .map_or(AdblockMenu::Hidden, |source| source.menu(page)),
        };
        let items: Vec<(usize, MenuItemView)> = webview_menu_items(host)
            .into_iter()
            .filter_map(|item| Some((item.id, (item.view)(host, &flags)?)))
            .collect();
        let placement = menu_placement(native);
        MenuRequest {
            separator_at: placement.separator_at.filter(|_| !items.is_empty()),
            items: items
                .into_iter()
                .enumerate()
                .map(|(index, (id, view))| MenuItemRequest {
                    host,
                    id,
                    label: view.label,
                    checked: view.checked,
                    enabled: view.enabled,
                    at: placement.first_item_at + index as u32,
                    action: view.action,
                })
                .collect(),
        }
    }
}

/// Acrescenta ao menu nativo do botao direito de uma WebView os itens do
/// registo para o hospedeiro, com o rotulo do estado no instante do clique,
/// no lugar que `menu_placement` decide. Precisa do ContextMenuRequested
/// (ICoreWebView2_11 e ICoreWebView2Environment9); num runtime sem ele
/// devolve o erro e a WebView fica so com o menu nativo. Uma falha a montar
/// um menu concreto fica no log e esse menu abre como o WebView2 o trouxe.
fn register_webview_context_menu(
    webview: &WebView,
    host: WebViewHost,
    auto_scroll: SharedFlag,
    adblock: Option<AdblockMenuSource>,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    use webview2_com::{
        ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_CHECK_BOX,
            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
            COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR, ICoreWebView2, ICoreWebView2_11,
            ICoreWebView2ContextMenuRequestedEventArgs, ICoreWebView2Environment9,
        },
        take_pwstr,
    };
    use windows_core::{HSTRING, Interface, PWSTR};
    use wry::WebViewExtWindows;

    let core = webview
        .webview()
        .cast::<ICoreWebView2_11>()
        .map_err(|error| format!("ICoreWebView2_11 indisponível: {error}"))?;
    let environment = webview
        .environment()
        .cast::<ICoreWebView2Environment9>()
        .map_err(|error| format!("ICoreWebView2Environment9 indisponível: {error}"))?;

    let respond = webview_menu_responder(host, auto_scroll, adblock);
    let add_items = move |sender: Option<&ICoreWebView2>,
                          args: &ICoreWebView2ContextMenuRequestedEventArgs|
          -> windows_core::Result<()> {
        unsafe {
            let menu = args.MenuItems()?;
            let mut native = 0u32;
            menu.Count(&mut native)?;
            // A pagina deste pedido, do `sender` do evento: o que o menu
            // do bloqueio de anuncios decide (o site, a contagem).
            let page = match sender {
                Some(sender) => {
                    let mut source = PWSTR::null();
                    sender.Source(&mut source).ok().map(|()| take_pwstr(source))
                }
                None => None,
            };
            let request = respond(native, page.as_deref());
            // Tudo criado antes de mexer no menu: uma falha a meio nao
            // deixa um separador solto no fim do menu nativo.
            let mut created = Vec::with_capacity(request.items.len());
            for item in &request.items {
                let label = HSTRING::from(item.label.as_str());
                let kind = if item.checked.is_some() {
                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_CHECK_BOX
                } else {
                    COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND
                };
                let entry = environment.CreateContextMenuItem(&label, None, kind)?;
                if let Some(checked) = item.checked {
                    entry.SetIsChecked(checked)?;
                }
                if item.enabled {
                    let proxy = proxy.clone();
                    let item_request = item.clone();
                    let selected = CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                        if let Some(event) = item_request.selected() {
                            let _ = proxy.send_event(event);
                        }
                        Ok(())
                    }));
                    let mut selected_token = 0i64;
                    entry.add_CustomItemSelected(&selected, &mut selected_token)?;
                } else {
                    entry.SetIsEnabled(false)?;
                }
                created.push((item.at, entry));
            }
            let separator = match request.separator_at {
                Some(index) => Some((
                    index,
                    environment.CreateContextMenuItem(
                        &HSTRING::new(),
                        None,
                        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR,
                    )?,
                )),
                None => None,
            };
            if let Some((index, separator)) = separator {
                menu.InsertValueAtIndex(index, &separator)?;
            }
            for (index, entry) in created {
                menu.InsertValueAtIndex(index, &entry)?;
            }
        }
        Ok(())
    };
    let described = host.describe();
    let handler = ContextMenuRequestedEventHandler::create(Box::new(move |sender, args| {
        if let Some(args) = args
            && let Err(error) = add_items(sender.as_ref(), &args)
        {
            debug_log(format_args!(
                "context menu: {described} abriu sem os itens do NeuralIA ({error})"
            ));
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core.add_ContextMenuRequested(&handler, &mut token) }
        .map_err(|error| format!("add_ContextMenuRequested falhou: {error}"))
}

// ===================== os aceleradores =====================

/// O que o `AcceleratorKeyPressed` le de uma tecla: a tecla virtual, se e
/// uma descida, os modificadores (`GetKeyState` na thread da interface, que
/// e onde o handler corre) e se e a repeticao de uma tecla presa
/// (`WasKeyDown` do `PhysicalKeyStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct AcceleratorInput {
    pub(in crate::windows_app) vk: u32,
    pub(in crate::windows_app) down: bool,
    pub(in crate::windows_app) ctrl: bool,
    pub(in crate::windows_app) shift: bool,
    pub(in crate::windows_app) alt: bool,
    pub(in crate::windows_app) repeat: bool,
}

/// A decisao para uma tecla: `handled` marca `Handled = TRUE` no WebView2
/// (a pagina nao ve o keydown); `event` e o que o lado nativo faz.
#[derive(Debug)]
pub(in crate::windows_app) struct AcceleratorDecision {
    pub(in crate::windows_app) handled: bool,
    pub(in crate::windows_app) event: Option<UserEvent>,
}

/// A consulta que o handler faz a cada tecla: a decisao do mapa de teclas
/// (`keymap_decision_in`: uma leitura do mapa, o ambito do hospedeiro, a
/// repeticao filtrada, `Handled` so nos atalhos presos, a lista do ChatGPT
/// fora dos hospedeiros das IAs), com o hospedeiro que o handler recebeu no
/// registo como origem -- sem chamadas COM la dentro. O handler passa o
/// mapa do produto (`product_keymap()`); os gates chamam esta mesma funcao
/// com um mapa com atalhos em todos os ambitos e veem a origem que sai de
/// cada hospedeiro.
pub(in crate::windows_app) fn accelerator_lookup(
    keymap: &RwLock<Keymap>,
    host: WebViewHost,
    input: AcceleratorInput,
) -> AcceleratorDecision {
    keymap_decision_in(keymap, input, CommandOrigin::Host(host))
}

/// O `AcceleratorKeyPressed` de uma WebView acabada de construir: le a
/// tecla, pergunta a `accelerator_lookup` sobre o mapa do produto com o
/// hospedeiro deste registo -- o unico que o handler conhece; ele nunca
/// nomeia outro (gate `the_accelerator_callback_calls_out_to_nothing`) --
/// e so toca no `Handled` quando a decisao e prender a tecla: um handler
/// que nada prende deixa o WebView2 exatamente como estava.
fn register_webview_accelerators(
    webview: &WebView,
    host: WebViewHost,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    use webview2_com::{
        AcceleratorKeyPressedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
            COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
        },
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_MENU};
    use wry::WebViewExtWindows;

    fn held(vk: u16) -> bool {
        unsafe { GetKeyState(i32::from(vk)) < 0 }
    }

    let controller = webview.controller();
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
        let mut vk = 0u32;
        let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
        unsafe {
            args.KeyEventKind(&mut kind)?;
            args.VirtualKey(&mut vk)?;
            args.PhysicalKeyStatus(&mut status)?;
        }
        let input = AcceleratorInput {
            vk,
            down: kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                || kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
            ctrl: held(VK_CONTROL),
            shift: held(VK_SHIFT),
            alt: held(VK_MENU),
            repeat: status.WasKeyDown.as_bool(),
        };
        let decision = accelerator_lookup(product_keymap(), host, input);
        if decision.handled {
            unsafe { args.SetHandled(true)? };
        }
        if let Some(event) = decision.event {
            let _ = proxy.send_event(event);
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) }
        .map_err(|error| format!("add_AcceleratorKeyPressed falhou: {error}"))
}

// ===================== a metade depois do build =====================

/// Quem regista no WebView2 o que a tabela manda para uma WebView acabada de
/// construir. O produto passa o COM (`ComHookRegistrar`); o gate passa um
/// registo que anota o que foi pedido para cada hospedeiro.
pub(in crate::windows_app) trait HookRegistrar {
    /// Os itens do registo (ids de `WEBVIEW_MENU_ITEMS`) no menu do botao
    /// direito deste hospedeiro.
    fn context_menu(&mut self, host: WebViewHost, items: &[usize]) -> Result<(), String>;
    /// O `AcceleratorKeyPressed` de `accelerator_lookup` neste hospedeiro.
    fn accelerators(&mut self, host: WebViewHost) -> Result<(), String>;
    /// O gestor de downloads (`downloads.rs`): o `DownloadStarting` e a
    /// pasta escolhida, nos hospedeiros com `DownloadPolicy::Managed`.
    fn downloads(&mut self, host: WebViewHost) -> Result<(), String>;
    /// O `WebResourceRequested` do despachante (`resource_gate_answers`)
    /// neste hospedeiro.
    fn resource_gate(&mut self, host: WebViewHost) -> Result<(), String>;
}

/// O que `install_webview_hooks` faz com uma WebView acabada de construir:
/// pede ao registador cada slot da tabela que esta ligado, e devolve a linha
/// de log de cada registo que falhou -- um runtime WebView2 sem o
/// ContextMenuRequested, por exemplo. Uma falha nao sobe: a WebView abre,
/// com o que o WebView2 traz, e so esse gancho fica de fora.
pub(in crate::windows_app) fn install_hooks_with(
    host: WebViewHost,
    hooks: &WebViewHooks,
    registrar: &mut impl HookRegistrar,
) -> Vec<String> {
    let mut missing = Vec::new();
    if !hooks.menu.is_empty()
        && let Err(error) = registrar.context_menu(host, &hooks.menu)
    {
        missing.push(format!(
            "context menu: {} sem os itens do NeuralIA ({error})",
            host.describe()
        ));
    }
    if hooks.accelerators
        && let Err(error) = registrar.accelerators(host)
    {
        missing.push(format!(
            "accelerators: {} sem AcceleratorKeyPressed ({error})",
            host.describe()
        ));
    }
    // Sem o gestor, o WebView2 grava cada ficheiro como sempre gravou. So
    // falha num runtime sem ICoreWebView2_4 (o `DownloadStarting`), onde
    // nem o wry consegue recusar: fica no log.
    if hooks.downloads == DownloadPolicy::Managed
        && let Err(error) = registrar.downloads(host)
    {
        missing.push(format!(
            "downloads: {} sem o gestor de downloads ({error})",
            host.describe()
        ));
    }
    if hooks.resource_gate == ResourceGatePolicy::Adblock
        && let Err(error) = registrar.resource_gate(host)
    {
        missing.push(format!(
            "resource gate: {} sem WebResourceRequested ({error})",
            host.describe()
        ));
    }
    missing
}

/// O registador do produto: o COM do WebView2 de uma WebView.
struct ComHookRegistrar<'a> {
    webview: &'a WebView,
    auto_scroll: SharedFlag,
    downloads: &'a DownloadsShared,
    proxy: EventLoopProxy<UserEvent>,
    /// O bloqueio de anuncios partilhado e o contador DESTA WebView (o
    /// menu le o que o `WebResourceRequested` dela conta).
    adblock: Arc<AdblockShared>,
    blocked: Arc<PageBlocked>,
}

impl HookRegistrar for ComHookRegistrar<'_> {
    fn context_menu(&mut self, host: WebViewHost, _items: &[usize]) -> Result<(), String> {
        let adblock = adblock_host(host).then(|| AdblockMenuSource {
            shared: Arc::clone(&self.adblock),
            blocked: Arc::clone(&self.blocked),
        });
        register_webview_context_menu(
            self.webview,
            host,
            self.auto_scroll.clone(),
            adblock,
            self.proxy.clone(),
        )
    }

    fn accelerators(&mut self, host: WebViewHost) -> Result<(), String> {
        register_webview_accelerators(self.webview, host, self.proxy.clone())
    }

    fn downloads(&mut self, host: WebViewHost) -> Result<(), String> {
        register_download_manager(self.webview, host, self.downloads, self.proxy.clone())
    }

    fn resource_gate(&mut self, host: WebViewHost) -> Result<(), String> {
        register_resource_gate(
            self.webview,
            host,
            Arc::clone(&self.adblock),
            Arc::clone(&self.blocked),
        )
    }
}

// ===================== o WebResourceRequested (ResourceGate) =====================

/// A razao do 403 com que um pedido bloqueado e respondido.
const BLOCKED_REASON: &str = "Blocked by NeuralIA";

/// O filtro `*` do `WebResourceRequested`, em todos os contextos e (com o
/// `ICoreWebView2_22`, quando o runtime o tem) em todas as origens do
/// pedido -- os workers incluidos. Liga-se com o bloqueio: sem ele nenhum
/// pedido passa pelo handler.
pub(in crate::windows_app) fn set_resource_filter(
    webview: &WebView,
    on: bool,
) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        ICoreWebView2_22,
    };
    use windows_core::{HSTRING, Interface};
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let filter = HSTRING::from("*");
    let result = unsafe {
        match core.cast::<ICoreWebView2_22>() {
            Ok(core) if on => core.AddWebResourceRequestedFilterWithRequestSourceKinds(
                &filter,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            ),
            Ok(core) => core.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                &filter,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            ),
            Err(_) if on => {
                core.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
            }
            Err(_) => core
                .RemoveWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL),
        }
    };
    result.map_err(|error| format!("filtro do WebResourceRequested: {error}"))
}

/// O `WebResourceRequested` de uma WebView com `ResourceGatePolicy::Adblock`.
/// O handler le o pedido, o tipo e a pagina de topo do `sender` do evento --
/// nunca de uma WebView capturada --, pergunta ao despachante e, num
/// bloqueio, responde 403 pelo `CreateWebResourceResponse` do ambiente do
/// mesmo `sender`. Sem regras (desligado, sem lista) sai logo. O filtro so
/// entra se o bloqueio estiver ligado; ligado depois, `set_resource_filter`
/// poe-no nas WebViews abertas.
fn register_resource_gate(
    webview: &WebView,
    host: WebViewHost,
    shared: Arc<AdblockShared>,
    blocked: Arc<PageBlocked>,
) -> Result<(), String> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_WEB_RESOURCE_CONTEXT, ICoreWebView2_2},
        WebResourceRequestedEventHandler, take_pwstr,
    };
    use windows_core::{HSTRING, Interface, PWSTR};
    use wry::WebViewExtWindows;

    let filtering = shared.filtering();
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
        let Some(rules) = shared.rules() else {
            return Ok(());
        };
        let (Some(sender), Some(args)) = (sender, args) else {
            return Ok(());
        };
        let (uri, top, context) = unsafe {
            let request = args.Request()?;
            let mut uri = PWSTR::null();
            request.Uri(&mut uri)?;
            let uri = take_pwstr(uri);
            let mut top = PWSTR::null();
            sender.Source(&mut top)?;
            let top = take_pwstr(top);
            let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT(0);
            args.ResourceContext(&mut context)?;
            (uri, top, context)
        };
        let page = ResourcePage {
            top: &top,
            kind: resource_kind_of(context.0),
        };
        if !resource_gate_answers(host, &uri, page, Some(&rules)) {
            return Ok(());
        }
        unsafe {
            let environment = sender.cast::<ICoreWebView2_2>()?.Environment()?;
            let response = environment.CreateWebResourceResponse(
                None,
                403,
                &HSTRING::from(BLOCKED_REASON),
                &HSTRING::new(),
            )?;
            args.SetResponse(&response)?;
        }
        blocked.record(&top);
        Ok(())
    }));
    let core = webview.webview();
    let mut token = 0i64;
    unsafe { core.add_WebResourceRequested(&handler, &mut token) }
        .map_err(|error| format!("add_WebResourceRequested falhou: {error}"))?;
    if filtering {
        set_resource_filter(webview, true)?;
    }
    Ok(())
}

// ===================== o evento do modulo =====================

/// O que os ganchos das WebViews mandam ao event loop. Lista fechada deste
/// modulo: um aviso novo de uma WebView e uma variante aqui, nao mais uma em
/// `UserEvent`.
#[derive(Debug)]
pub(in crate::windows_app) enum WebViewEvent {
    /// A pagina de `page` acabou de carregar `url` (o `NavigationCompleted`
    /// do WebView2). No Leitor e no PDF a `url` e o documento local (o HTML
    /// do artigo, o nosso visualizador): o endereco verdadeiro dessas duas
    /// paginas e `App::page_source`.
    PageLoaded { page: WebViewHost, url: String },
}

/// Um builder que ja passou pela metade do builder da tabela, com o
/// hospedeiro que a recebeu: a unica coisa que constroi uma WebView no
/// produto. O `build`/`build_as_child` do wry so se chama aqui dentro (gate
/// `every_webview_gets_the_hooks`: nenhum fora deste modulo), e a WebView
/// que sai ja tem a metade do COM registada para o mesmo hospedeiro -- um
/// sitio nao consegue construir sem os ganchos nem registar com outro
/// hospedeiro. Os campos sao privados: nem os sitios nem os testes o
/// montam de outra forma.
#[must_use = "um builder com os ganchos so serve construido: build_hooked ou build_hooked_as_child"]
pub(in crate::windows_app) struct HookedBuilder<'app> {
    app: &'app App,
    builder: WebViewBuilder<'static>,
    host: WebViewHost,
}

impl HookedBuilder<'_> {
    /// Uma WebView de topo (a Web completa, o Leitor, o PDF, os livros),
    /// ja com os ganchos do hospedeiro.
    pub(in crate::windows_app) fn build_hooked(self, window: &Window) -> wry::Result<WebView> {
        let webview = self.builder.build(window)?;
        self.app.install_webview_hooks(&webview, self.host);
        Ok(webview)
    }

    /// Uma WebView filha (as colunas, a fonte ao lado, os paineis, o
    /// monitor do Gmail), ja com os ganchos do hospedeiro.
    pub(in crate::windows_app) fn build_hooked_as_child(
        self,
        window: &Window,
    ) -> wry::Result<WebView> {
        let webview = self.builder.build_as_child(window)?;
        self.app.install_webview_hooks(&webview, self.host);
        Ok(webview)
    }
}

impl App {
    /// A metade do builder para uma WebView deste hospedeiro, com a origem
    /// local que o utilizador autorizou (a Web completa e as fontes ao
    /// lado; `None` nas outras). Cada builder passa aqui antes do `build`,
    /// e o `HookedBuilder` devolvido e o unico que constroi: o hospedeiro
    /// que este argumento declara e o unico que a WebView tem, nas duas
    /// metades.
    pub(in crate::windows_app) fn hooked_builder(
        &self,
        builder: WebViewBuilder<'static>,
        host: WebViewHost,
        local_origin: Option<String>,
    ) -> HookedBuilder<'_> {
        let proxy = self.proxy.clone();
        let builder = hook_webview_builder(builder, host, local_origin, move |event| {
            let _ = proxy.send_event(event);
        });
        HookedBuilder {
            app: self,
            builder,
            host,
        }
    }

    /// A unica porta para o COM de uma WebView acabada de construir: o
    /// `HookedBuilder` traz aqui cada uma que constroi, com o hospedeiro
    /// que recebeu, e ela recebe o que a tabela manda -- os itens do menu
    /// do botao direito nas paginas que rolam, o `AcceleratorKeyPressed`
    /// em todas, o gestor de downloads nas que tem `DownloadPolicy::Managed`,
    /// o `WebResourceRequested` nas que tem `ResourceGatePolicy::Adblock`.
    /// Privado ao modulo: nenhum sitio regista por conta propria.
    /// Um runtime WebView2 sem um dos eventos deixa a WebView sem esse
    /// gancho e fica no log.
    fn install_webview_hooks(&self, webview: &WebView, host: WebViewHost) {
        let mut registrar = ComHookRegistrar {
            webview,
            auto_scroll: self.auto_scroll.clone(),
            downloads: &self.downloads.shared,
            proxy: self.proxy.clone(),
            adblock: Arc::clone(&self.adblock.shared),
            blocked: Arc::new(PageBlocked::default()),
        };
        for line in install_hooks_with(host, &webview_hooks(host), &mut registrar) {
            debug_log(format_args!("{line}"));
        }
    }

    /// O unico braco dos ganchos no `user_event`: tudo o que as WebViews
    /// avisam passa por aqui.
    pub(in crate::windows_app) fn webview_event(&mut self, event: WebViewEvent) {
        match event {
            WebViewEvent::PageLoaded { page, url } => self.page_loaded(page, url),
        }
    }

    /// Uma pagina acabou de carregar. O endereco fica no evento para quem
    /// vier (favoritos); o bloqueio de anuncios so precisa do hospedeiro
    /// (a renovacao semanal da lista, nunca na Home). O log de depuracao
    /// nunca leva URLs, so a transicao.
    fn page_loaded(&mut self, page: WebViewHost, _url: String) {
        debug_log(format_args!("webview: {} carregou", page.describe()));
        self.adblock_page_loaded(page);
    }
}
