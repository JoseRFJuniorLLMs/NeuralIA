//! O portao de saida da IA (infra-egress, plano 2.3): o sitio que decide se
//! um dado sai para um modelo, para as features de IA da 2.3 em diante (a
//! Traducao e a primeira a pedi-lo; o Gemini Live, anterior, ainda nao
//! passa por aqui). Sem rede, sem Win32; portatil e testado tambem no
//! runner Linux.
//!
//! `EgressGate::request(pedido)` responde `Send`, `Ask(cartao)` ou
//! `Refuse(razao)`. O pedido diz a finalidade (`AiPurpose`), a classe do
//! dado (`DataClass`), o destino (cerebro, `Locality`, hosts, modelo, faixa
//! de preco, se e pago), o site de onde vem o dado, a estimativa de tokens,
//! o numero de chamadas pagas, o gatilho e a privacidade. As regras, por
//! esta ordem (`decide`, pura):
//!
//! 1. Modo privado (`EgressPrivacy::PrivateMode`, esboco ate a onda 4):
//!    nada sai do PC (`Lan`/`Remote` recusados).
//! 2. A regra de base (`neural_core::ai_policy::may_send`): a memoria local
//!    nunca vai para a Internet.
//! 3. Segundo plano (`Trigger::Background`): NUNCA pergunta; so manda com
//!    uma `StandingGrant` que bata certo (a mesma vigia, o mesmo cerebro,
//!    nao revogada, dentro do prazo de no maximo 30 dias e do limite por
//!    dia), so para o que o utilizador escreveu (`UserTyped`: o texto de
//!    uma pagina, a memoria, respostas e media sao sempre recusados) e so
//!    abaixo do limite mensal.
//! 4. Clique: o que fica no PC vai; o que sai precisa de consentimento
//!    para o par (cerebro, site) -- da sessao (`ConsentLedger`, so em
//!    memoria, nunca num ficheiro) ou, so onde um desenho o pede (a
//!    Traducao, «Sempre neste site»), persistente (`SiteGrants`, uma loja
//!    `StoreKind::Setting`, relida em cada pedido: revogar noutra janela
//!    vale no pedido seguinte desta). Sem site (um ficheiro local, um PDF,
//!    o ditado), o consentimento da sessao e por (cerebro, finalidade,
//!    classe do dado): o sim ao audio do ditado nao manda o texto de um PDF.
//!    E uma chamada paga acima do limite mensal pede sempre confirmacao
//!    («Limite mensal atingido (200 chamadas) — continuar?»), nunca calada.
//!    Uma so pergunta junta as duas coisas.
//!
//! Cada `Send` de um destino pago conta as suas chamadas no `ai/usage.json`
//! (`ai_settings::UsageBook`, `StoreKind::Setting`: o modo privado das
//! lojas nao o apaga nem o salta, e o limite nao recomeca) -- quem manda
//! passa pelo portao, e o portao conta: nao ha "mandar sem contar". A
//! gravacao vai para a thread `neural-usage`, um `LazyWorker` que so nasce
//! na primeira chamada paga (nunca no `App::new`: a Home fica igual). Antes
//! de decidir o limite de um destino pago, o portao rele o `usage.json`: o
//! que as outras janelas ja gravaram conta. O limite e suave: dois cliques
//! no mesmo instante em duas janelas, antes de qualquer das duas gravar,
//! podem passar ambos (e ambos contam). O limite mensal (200 por omissao)
//! vem do `ai/settings.json`.
//!
//! O cartao (`ConsentCard`) e o que o `NativeCard` do infra-notify-popups
//! pinta: hosts, tokens, modelo, faixa de preco, e o limite quando conta.
//! A resposta (`EgressGate::answer`) regista o consentimento pedido e so
//! entao manda. «Sempre neste site» so existe fora de qualquer contexto
//! privado, e so para as finalidades que o oferecem; gravado, vive so na
//! loja (nao tambem na sessao), e `EgressGate::revoke_site_grant` tira-o da
//! loja e tira o consentimento da sessao desse par.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use neural_core::ai_policy::{DataClass, Locality, MaySend, may_send};
use neural_core::json_store::{
    Degraded, LoadOutcome, SaveOutcome, StoreGrant, StoreKind, StoreRegistry, VersionedJsonStore,
};
use neural_core::llm::{Pick, PriceTier, Provider};
use serde::{Deserialize, Serialize};

use crate::ai_settings::{
    AI_SETTINGS_MAX_BYTES, AI_SETTINGS_VERSION, AI_USAGE_MAX_BYTES, AI_USAGE_VERSION, AiPurpose,
    AiSettings, Day, UsageBook, UsageDelta,
};
use crate::lazy_worker::{JobContext, LazyWorker};
use crate::stores::{AI_SETTINGS_STORE, AI_USAGE_STORE};

/// O nome da thread que grava o consumo.
pub(crate) const USAGE_WORKER_NAME: &str = "neural-usage";
/// O prazo maximo de uma autorizacao em segundo plano.
pub(crate) const MAX_STANDING_GRANT_DAYS: u32 = 30;
/// O maximo de envios por dia de uma autorizacao em segundo plano.
pub(crate) const MAX_STANDING_GRANT_PER_DAY: u32 = 24;
/// Os tectos da loja «Sempre neste site».
pub(crate) const SITE_GRANTS_VERSION: u32 = 1;
pub(crate) const SITE_GRANTS_MAX_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_SITE_GRANTS: usize = 2000;

/// Quem responde: um fornecedor na nuvem com a chave do dono, um perfil
/// BYOM ou os motores deste processo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Brain {
    Cloud(Provider),
    Byom(u8),
    OnDevice,
}

impl Brain {
    /// A chave nos ficheiros e no consentimento (estavel).
    pub(crate) fn key(self) -> String {
        match self {
            Self::Cloud(Provider::Gemini) => "gemini".to_string(),
            Self::Byom(id) => format!("byom-{id}"),
            Self::OnDevice => "local".to_string(),
        }
    }

    /// O nome no cartao.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Cloud(provider) => provider.label().to_string(),
            Self::Byom(id) => format!("servidor {id}"),
            Self::OnDevice => "este PC".to_string(),
        }
    }
}

/// Para onde vai o pedido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Destination {
    pub(crate) brain: Brain,
    pub(crate) locality: Locality,
    /// Os hosts que recebem o pedido: o cartao mostra-os.
    pub(crate) hosts: Vec<String>,
    pub(crate) model: Option<String>,
    pub(crate) price_tier: Option<PriceTier>,
    /// Cobrado na chave do dono: conta no limite mensal.
    pub(crate) paid: bool,
}

impl Destination {
    /// Um fornecedor na nuvem com o modelo que a regra (ou o dono)
    /// escolheu: o host fixado, pago.
    pub(crate) fn cloud(pick: &Pick) -> Self {
        Self {
            brain: Brain::Cloud(pick.provider),
            locality: Locality::Remote,
            hosts: vec![pick.provider.host().to_string()],
            model: Some(pick.model.to_string()),
            price_tier: Some(pick.price_tier),
            paid: true,
        }
    }
}

/// A origem (`https://exemplo.com`) de onde vem o dado. So http(s), na
/// forma canonica do `url` (host em minusculas, IDN em punycode).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SiteOrigin(String);

impl SiteOrigin {
    pub(crate) fn of_url(raw: &str) -> Option<Self> {
        let url = url::Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let origin = url.origin();
        if !origin.is_tuple() {
            return None;
        }
        let text = origin.ascii_serialization();
        (text.len() <= 256).then_some(Self(text))
    }

    /// So uma origem ja canonica (a leitura da loja nao aceita outra forma).
    fn canonical(text: &str) -> Option<Self> {
        Self::of_url(text).filter(|origin| origin.0 == text)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// O host, para o cartao.
    pub(crate) fn host(&self) -> &str {
        self.0.split_once("://").map_or(&self.0, |(_, rest)| rest)
    }
}

/// A privacidade do pedido.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EgressPrivacy {
    #[default]
    Normal,
    /// Vem de uma superficie privada (o Split privado): pode perguntar, mas
    /// nada fica no disco -- nem se oferece nem se le «Sempre neste site».
    PrivateSurface,
    /// O Modo privado (esboco ate a onda 4): nada sai do PC.
    PrivateMode,
}

/// O gatilho do pedido.
#[derive(Debug)]
pub(crate) enum Trigger<'a> {
    /// Um clique do utilizador agora.
    Click,
    /// Uma vigia em segundo plano (o Radar), com a autorizacao que tiver.
    Background {
        watch_id: &'a str,
        grant: Option<&'a mut StandingGrant>,
    },
}

/// Um pedido de saida.
#[derive(Debug)]
pub(crate) struct EgressRequest<'a> {
    pub(crate) feature: AiPurpose,
    pub(crate) data: DataClass,
    pub(crate) destination: Destination,
    pub(crate) origin: Option<SiteOrigin>,
    pub(crate) token_estimate: u32,
    /// As chamadas pagas que o pedido faz (a traducao de 5 blocos faz 5).
    /// Pelo menos uma.
    pub(crate) calls: u32,
    pub(crate) trigger: Trigger<'a>,
    pub(crate) privacy: EgressPrivacy,
}

impl EgressRequest<'_> {
    fn calls(&self) -> u32 {
        self.calls.max(1)
    }
}

/// Porque nao vai.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefuseReason {
    PrivateMode,
    NeverLeavesThePc(DataClass),
    BackgroundData(DataClass),
    BackgroundWithoutGrant,
    GrantForAnotherWatch,
    GrantForAnotherBrain,
    GrantRevoked,
    GrantExpired,
    GrantDailyLimit,
    OverCap { used: u32, cap: u32 },
    Cancelled,
}

impl RefuseReason {
    /// A frase que o utilizador ve.
    pub(crate) fn message(self) -> String {
        match self {
            Self::PrivateMode => {
                "Modo privado: só um modelo deste PC pode ser usado. Nada foi enviado.".to_string()
            }
            Self::NeverLeavesThePc(data) => {
                format!("Nada foi enviado: {} nunca sai deste PC.", data.label())
            }
            Self::BackgroundData(data) => format!(
                "Em segundo plano, {} nunca é enviado. Nada foi enviado.",
                data.label()
            ),
            Self::BackgroundWithoutGrant
            | Self::GrantForAnotherWatch
            | Self::GrantForAnotherBrain => {
                "Em segundo plano nada é enviado sem a autorização desta vigia.".to_string()
            }
            Self::GrantRevoked => "A autorização desta vigia foi revogada.".to_string(),
            Self::GrantExpired => "A autorização desta vigia expirou.".to_string(),
            Self::GrantDailyLimit => {
                "Esta vigia já usou os envios de hoje; volta amanhã.".to_string()
            }
            Self::OverCap { cap, .. } => format!(
                "Limite mensal atingido ({} chamadas): nada foi enviado em segundo plano.",
                thousands(cap)
            ),
            Self::Cancelled => "Nada foi enviado.".to_string(),
        }
    }
}

/// O limite mensal no momento do pedido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CapReached {
    pub(crate) used: u32,
    pub(crate) cap: u32,
}

impl CapReached {
    /// A pergunta do limite: atingido, ou passado por este pedido.
    pub(crate) fn question(self) -> String {
        if self.used >= self.cap {
            format!(
                "Limite mensal atingido ({} chamadas) — continuar?",
                thousands(self.cap)
            )
        } else {
            format!(
                "Este pedido passa o limite mensal ({} de {} chamadas) — continuar?",
                thousands(self.used),
                thousands(self.cap)
            )
        }
    }
}

/// A resposta a um cartao.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsentAnswer {
    /// So este envio (e «Continuar» no cartao do limite).
    Once,
    /// Este cerebro com este site ate fechar o NeuralIA (so em memoria).
    Session,
    /// «Sempre neste site» (so onde o cartao o oferece).
    AlwaysOnSite,
    Cancel,
}

/// O cartao que o `NativeCard` pinta antes de um envio. Os campos sao
/// privados: so o portao o cria.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConsentCard {
    feature: AiPurpose,
    data: DataClass,
    destination: Destination,
    origin: Option<SiteOrigin>,
    token_estimate: u32,
    calls: u32,
    privacy: EgressPrivacy,
    needs_consent: bool,
    cap: Option<CapReached>,
    offer_always: bool,
}

impl ConsentCard {
    pub(crate) fn hosts(&self) -> &[String] {
        &self.destination.hosts
    }

    pub(crate) fn token_estimate(&self) -> u32 {
        self.token_estimate
    }

    pub(crate) fn model(&self) -> Option<&str> {
        self.destination.model.as_deref()
    }

    pub(crate) fn price_tier(&self) -> Option<PriceTier> {
        self.destination.price_tier
    }

    pub(crate) fn needs_consent(&self) -> bool {
        self.needs_consent
    }

    pub(crate) fn cap(&self) -> Option<CapReached> {
        self.cap
    }

    pub(crate) fn offers_always(&self) -> bool {
        self.offer_always
    }

    /// Os botoes do cartao, pela ordem.
    pub(crate) fn answers(&self) -> Vec<ConsentAnswer> {
        let mut answers = vec![ConsentAnswer::Once];
        if self.needs_consent {
            answers.push(ConsentAnswer::Session);
            if self.offer_always {
                answers.push(ConsentAnswer::AlwaysOnSite);
            }
        }
        answers.push(ConsentAnswer::Cancel);
        answers
    }

    pub(crate) fn answer_label(&self, answer: ConsentAnswer) -> &'static str {
        match answer {
            ConsentAnswer::Once if self.needs_consent => "Enviar desta vez",
            ConsentAnswer::Once => "Continuar",
            ConsentAnswer::Session => "Sempre nesta sessão",
            ConsentAnswer::AlwaysOnSite => "Sempre neste site",
            ConsentAnswer::Cancel => "Cancelar",
        }
    }

    /// O titulo: a pergunta do envio, ou a do limite quando so ele conta.
    pub(crate) fn title(&self) -> String {
        match (self.needs_consent, self.cap) {
            (false, Some(cap)) => cap.question(),
            _ => format!(
                "Enviar {} para {}?",
                self.data.label(),
                self.destination.brain.label()
            ),
        }
    }

    /// As linhas do cartao: para onde vai, quanto, que modelo, de que site,
    /// e o limite quando conta.
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "Vai para {} ({})",
            self.destination.hosts.join(", "),
            self.destination.locality.label()
        )];
        lines.push(format!("≈ {} tokens", thousands(self.token_estimate)));
        match (&self.destination.model, self.destination.price_tier) {
            (Some(model), Some(tier)) => lines.push(format!("Modelo: {model} · {}", tier.label())),
            (Some(model), None) => lines.push(format!("Modelo: {model}")),
            (None, Some(tier)) => lines.push(tier.label().to_string()),
            (None, None) => {}
        }
        if let Some(origin) = &self.origin {
            lines.push(format!("Site: {}", origin.host()));
        }
        if let (true, Some(cap)) = (self.needs_consent, self.cap) {
            lines.push(cap.question());
        }
        lines
    }

    /// O pedido que o cartao descreve, como um clique.
    fn into_click(self) -> (EgressRequest<'static>, bool) {
        let request = EgressRequest {
            feature: self.feature,
            data: self.data,
            destination: self.destination,
            origin: self.origin,
            token_estimate: self.token_estimate,
            calls: self.calls,
            trigger: Trigger::Click,
            privacy: self.privacy,
        };
        (request, self.offer_always)
    }
}

/// O cartao de uma autorizacao em segundo plano (o Radar a criar uma vigia
/// de tema): sempre um cartao, mesmo com consentimento na sessao.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StandingGrantCard {
    card: ConsentCard,
    max_per_day: u32,
    days: u32,
}

impl StandingGrantCard {
    pub(crate) fn card(&self) -> &ConsentCard {
        &self.card
    }

    pub(crate) fn title(&self) -> String {
        format!(
            "Autorizar {} em segundo plano?",
            self.card.destination.brain.label()
        )
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines = self.card.lines();
        lines.push(format!(
            "Só o tema que você escreveu, até {} vezes por dia, durante {} dias; revogável a qualquer momento.",
            self.max_per_day, self.days
        ));
        lines
    }
}

/// Uma autorizacao em segundo plano (critica C7): uma vigia, um cerebro,
/// so o que o utilizador escreveu, um limite por dia, no maximo 30 dias,
/// revogavel. So nasce da resposta a um `StandingGrantCard`; quem a guarda
/// e a vigia (o Radar, numa loja `Setting`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StandingGrant {
    watch_id: String,
    brain: String,
    max_per_day: u32,
    issued: Day,
    expires: Day,
    #[serde(default)]
    revoked: bool,
    #[serde(default)]
    used_on: Option<Day>,
    #[serde(default)]
    used_today: u32,
}

impl StandingGrant {
    /// A autorizacao que o cartao pediu, se a resposta foi sim e a vigia tem
    /// um id valido.
    pub(crate) fn issue(
        card: &StandingGrantCard,
        answer: ConsentAnswer,
        watch_id: &str,
        today: Day,
    ) -> Option<Self> {
        let valid_id = (1..=64).contains(&watch_id.len())
            && watch_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if answer == ConsentAnswer::Cancel
            || !valid_id
            || card.card.data != DataClass::UserTyped
            || card.card.privacy != EgressPrivacy::Normal
        {
            return None;
        }
        Some(Self {
            watch_id: watch_id.to_string(),
            brain: card.card.destination.brain.key(),
            max_per_day: card.max_per_day,
            issued: today,
            expires: today.plus(card.days),
            revoked: false,
            used_on: None,
            used_today: 0,
        })
    }

    pub(crate) fn watch_id(&self) -> &str {
        &self.watch_id
    }

    pub(crate) fn expires(&self) -> Day {
        self.expires
    }

    pub(crate) fn revoke(&mut self) {
        self.revoked = true;
    }

    /// Se serve para esta vigia, este cerebro, hoje. Um ficheiro escrito a
    /// mao nao estica o prazo alem de 30 dias nem o limite por dia.
    fn admits(&self, watch_id: &str, brain: Brain, today: Day) -> Result<(), RefuseReason> {
        if self.watch_id != watch_id {
            return Err(RefuseReason::GrantForAnotherWatch);
        }
        if self.brain != brain.key() {
            return Err(RefuseReason::GrantForAnotherBrain);
        }
        if self.revoked {
            return Err(RefuseReason::GrantRevoked);
        }
        let expires = self.expires.min(self.issued.plus(MAX_STANDING_GRANT_DAYS));
        if today >= expires || today < self.issued {
            return Err(RefuseReason::GrantExpired);
        }
        let used = if self.used_on == Some(today) {
            self.used_today
        } else {
            0
        };
        if used >= self.max_per_day.min(MAX_STANDING_GRANT_PER_DAY) {
            return Err(RefuseReason::GrantDailyLimit);
        }
        Ok(())
    }

    fn record_use(&mut self, today: Day) {
        if self.used_on == Some(today) {
            self.used_today = self.used_today.saturating_add(1);
        } else {
            self.used_on = Some(today);
            self.used_today = 1;
        }
    }
}

/// A decisao.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decision {
    Send,
    Ask(ConsentCard),
    Refuse(RefuseReason),
}

/// O que o portao sabe no momento do pedido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    /// Ha consentimento para (cerebro, site): da sessao ou «Sempre».
    pub(crate) consented: bool,
    /// Chamadas pagas ja feitas este mes.
    pub(crate) used: u32,
    /// O limite mensal suave.
    pub(crate) cap: u32,
    pub(crate) today: Day,
    /// A finalidade tem a loja «Sempre neste site» aberta.
    pub(crate) site_grants: bool,
}

/// As finalidades cujo desenho pede «Sempre neste site».
pub(crate) fn offers_site_grant(feature: AiPurpose) -> bool {
    matches!(feature, AiPurpose::Translation)
}

/// A decisao, pura (ver o cabecalho do modulo).
pub(crate) fn decide(request: &EgressRequest<'_>, facts: &Facts) -> Decision {
    let destination = &request.destination;
    let leaves = destination.locality.leaves_the_pc();
    if request.privacy == EgressPrivacy::PrivateMode && leaves {
        return Decision::Refuse(RefuseReason::PrivateMode);
    }
    let rule = may_send(request.data, destination.locality);
    if rule == MaySend::Never {
        return Decision::Refuse(RefuseReason::NeverLeavesThePc(request.data));
    }
    let over_cap = destination.paid && facts.used.saturating_add(request.calls()) > facts.cap;
    match &request.trigger {
        Trigger::Background { watch_id, grant } => {
            if request.data != DataClass::UserTyped {
                return Decision::Refuse(RefuseReason::BackgroundData(request.data));
            }
            let Some(grant) = grant else {
                return Decision::Refuse(RefuseReason::BackgroundWithoutGrant);
            };
            if let Err(reason) = grant.admits(watch_id, destination.brain, facts.today) {
                return Decision::Refuse(reason);
            }
            if over_cap {
                return Decision::Refuse(RefuseReason::OverCap {
                    used: facts.used,
                    cap: facts.cap,
                });
            }
            Decision::Send
        }
        Trigger::Click => {
            let needs_consent = rule == MaySend::WithConsent && !facts.consented;
            let cap = over_cap.then_some(CapReached {
                used: facts.used,
                cap: facts.cap,
            });
            if !needs_consent && cap.is_none() {
                return Decision::Send;
            }
            Decision::Ask(ConsentCard {
                feature: request.feature,
                data: request.data,
                destination: destination.clone(),
                origin: request.origin.clone(),
                token_estimate: request.token_estimate,
                calls: request.calls(),
                privacy: request.privacy,
                needs_consent,
                cap,
                offer_always: needs_consent
                    && facts.site_grants
                    && offers_site_grant(request.feature)
                    && request.privacy == EgressPrivacy::Normal
                    && request.origin.is_some(),
            })
        }
    }
}

/// A que vale um «Sempre nesta sessão»: um site; ou, sem site (`file:`,
/// `neuralia-pdf:`, `data:`, uma origem longa demais, o ditado), so a mesma
/// finalidade com a mesma classe do dado -- nunca «tudo o que nao tem site».
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ConsentScope {
    Site(SiteOrigin),
    NoSite(AiPurpose, DataClass),
}

impl ConsentScope {
    fn of(feature: AiPurpose, data: DataClass, origin: Option<&SiteOrigin>) -> Self {
        match origin {
            Some(origin) => Self::Site(origin.clone()),
            None => Self::NoSite(feature, data),
        }
    }
}

/// O consentimento da sessao, por (cerebro, `ConsentScope`). So em memoria:
/// morre com o processo e nunca toca no disco.
#[derive(Debug, Default)]
pub(crate) struct ConsentLedger {
    allowed: BTreeSet<(String, ConsentScope)>,
}

impl ConsentLedger {
    fn allow(&mut self, brain: String, scope: ConsentScope) {
        self.allowed.insert((brain, scope));
    }

    fn allows(&self, brain: &str, scope: &ConsentScope) -> bool {
        self.allowed
            .iter()
            .any(|(allowed, granted)| allowed == brain && granted == scope)
    }

    fn forget(&mut self, brain: &str, scope: &ConsentScope) {
        self.allowed
            .retain(|(allowed, granted)| !(allowed == brain && granted == scope));
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SiteGrantsFile {
    #[serde(default)]
    sites: Vec<SiteGrantEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SiteGrantEntry {
    brain: String,
    origin: String,
}

/// Porque a loja «Sempre neste site» nao abriu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SiteGrantsError {
    /// A loja tem de ser `StoreKind::Setting`.
    WrongKind,
    /// A loja tem de ser um ficheiro.
    WrongShape,
    /// A finalidade nao oferece «Sempre neste site».
    NotOffered,
}

/// «Sempre neste site» de uma finalidade: (cerebro, origem) numa loja
/// `Setting` que a finalidade da (a Traducao, a sua). Partilhada entre
/// janelas e relida em cada pergunta: uma revogacao noutra janela vale no
/// pedido seguinte desta. Uma entrada que nao e uma origem canonica e
/// ignorada.
#[derive(Debug)]
pub(crate) struct SiteGrants {
    store: VersionedJsonStore<SiteGrantsFile>,
    /// Revogados nesta janela: nao valem aqui mesmo que a loja nao tenha
    /// conseguido gravar a revogacao. Um «Sempre» novo neste par tira-o.
    revoked: BTreeSet<(String, SiteOrigin)>,
}

impl SiteGrants {
    pub(crate) fn open(grant: StoreGrant) -> Result<Self, SiteGrantsError> {
        if grant.kind() != StoreKind::Setting {
            return Err(SiteGrantsError::WrongKind);
        }
        let store = VersionedJsonStore::open(grant, SITE_GRANTS_VERSION, SITE_GRANTS_MAX_BYTES)
            .map_err(|_| SiteGrantsError::WrongShape)?
            .shared_between_windows();
        Ok(Self {
            store,
            revoked: BTreeSet::new(),
        })
    }

    fn read(file: &SiteGrantsFile) -> BTreeSet<(String, SiteOrigin)> {
        file.sites
            .iter()
            .take(MAX_SITE_GRANTS)
            .filter_map(|entry| {
                let origin = SiteOrigin::canonical(&entry.origin)?;
                (!entry.brain.is_empty() && entry.brain.len() <= 32)
                    .then(|| (entry.brain.clone(), origin))
            })
            .collect()
    }

    /// Os pares gravados AGORA (a loja relida). Uma loja que nada conserta
    /// nesta sessao (`sticky`) nao da nenhum.
    fn on_disk(&mut self) -> BTreeSet<(String, SiteOrigin)> {
        if sticky(self.store.read_only()) {
            return BTreeSet::new();
        }
        Self::read(&self.store.load().into_value())
    }

    fn allows(&mut self, brain: &str, origin: &SiteOrigin) -> bool {
        let pair = (brain.to_string(), origin.clone());
        !self.revoked.contains(&pair) && self.on_disk().contains(&pair)
    }

    /// Grava «Sempre neste site» para (cerebro, origem). Falso se o par nao
    /// ficou na loja (so de leitura, cheia, erro de disco): quem chama fica
    /// entao com o consentimento da sessao.
    fn allow(&mut self, brain: &str, origin: &SiteOrigin) -> bool {
        let pair = (brain.to_string(), origin.clone());
        let entry = SiteGrantEntry {
            brain: pair.0.clone(),
            origin: origin.as_str().to_string(),
        };
        self.revoked.remove(&pair);
        let written = self.store.update(|file| {
            if !file.sites.contains(&entry) && file.sites.len() < MAX_SITE_GRANTS {
                file.sites.push(entry.clone());
            }
            Self::read(file).contains(&pair)
        });
        matches!(written, Ok((true, SaveOutcome::Written)))
    }

    /// Tira «Sempre neste site» de (cerebro, origem): da loja (as outras
    /// janelas deixam de o ver no pedido seguinte) e, mesmo que a loja nao
    /// grave, desta janela. Verdadeiro se a loja gravou a revogacao.
    pub(crate) fn revoke(&mut self, brain: &str, origin: &SiteOrigin) -> bool {
        self.revoked.insert((brain.to_string(), origin.clone()));
        let written = self.store.update(|file| {
            file.sites
                .retain(|entry| !(entry.brain == brain && entry.origin == origin.as_str()));
        });
        matches!(written, Ok(((), SaveOutcome::Written)))
    }
}

/// Uma loja que nada conserta nesta sessao: estragada, de uma versao futura
/// ou grande demais (o NeuralIA nunca a reescreve). Nao se rele -- cada
/// leitura guardaria outra vez a copia `.bak`; uma que nao se conseguiu
/// abrir agora tenta outra vez no pedido seguinte.
fn sticky(degraded: Option<&Degraded>) -> bool {
    matches!(
        degraded,
        Some(Degraded::Corrupt | Degraded::FutureVersion { .. } | Degraded::TooLarge { .. })
    )
}

/// O que a janela sabe do consumo. O total do mes e `disk` + `unsaved` +
/// `kept`, e nenhuma chamada esta em dois deles ao mesmo tempo.
#[derive(Debug, Default)]
struct UsageView {
    /// O `usage.json` da ultima leitura ou gravacao (todas as janelas).
    disk: UsageBook,
    /// Contado nesta janela e ainda nao gravado: a espera da thread ou a
    /// ser gravado agora.
    unsaved: Vec<UsageDelta>,
    /// Contado so em memoria: sem loja, ou a loja recusou a gravacao
    /// (estragada, de uma versao futura, erro de disco).
    kept: UsageBook,
}

impl UsageView {
    fn month_total(&self, month: &str) -> u32 {
        self.unsaved
            .iter()
            .filter(|delta| delta.month == month)
            .fold(
                self.disk
                    .month_total(month)
                    .saturating_add(self.kept.month_total(month)),
                |total, delta| total.saturating_add(delta.calls),
            )
    }
}

fn lock_view(view: &Mutex<UsageView>) -> MutexGuard<'_, UsageView> {
    view.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// As duas pontas do `ai/usage.json` de uma janela: a janela rele-o antes
/// de decidir o limite (`reader`), a thread `neural-usage` grava-o
/// (`writer`). O trinco que as guarda serializa as duas: uma releitura
/// nunca cai a meio de uma gravacao desta janela, que contaria duas vezes
/// o que esta a ser gravado (no disco e ainda em `unsaved`).
#[derive(Default)]
struct UsageFiles {
    reader: Option<VersionedJsonStore<UsageBook>>,
    writer: Option<VersionedJsonStore<UsageBook>>,
}

fn lock_files(files: &Mutex<UsageFiles>) -> MutexGuard<'_, UsageFiles> {
    files
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// "Grava o que falta": pedidos seguidos juntam-se na vaga do worker.
struct UsageFlush;

/// Os contadores do mes: contados ja na janela, gravados no `ai/usage.json`
/// pela thread `neural-usage`, que so nasce na primeira chamada paga, e
/// relidos do disco antes de cada decisao do limite (o que as outras
/// janelas gravaram conta aqui). Duas janelas nunca perdem a conta uma da
/// outra: a gravacao rele o ficheiro debaixo do trinco `usage.json.lock` e
/// soma.
struct UsageLedger {
    files: Arc<Mutex<UsageFiles>>,
    view: Arc<Mutex<UsageView>>,
    worker: LazyWorker<UsageFlush>,
    persistent: bool,
}

impl UsageLedger {
    fn new(files: UsageFiles) -> Self {
        let persistent = files.writer.is_some();
        let files = Arc::new(Mutex::new(files));
        let view = Arc::new(Mutex::new(UsageView::default()));
        let (flush_files, flush_view) = (Arc::clone(&files), Arc::clone(&view));
        let worker = LazyWorker::new(USAGE_WORKER_NAME, move |_: UsageFlush, _: &JobContext| {
            flush_usage(&flush_files, &flush_view);
        });
        Self {
            files,
            view,
            worker,
            persistent,
        }
    }

    /// As chamadas pagas do mes, com o `usage.json` relido AGORA (o ficheiro
    /// tem no maximo 64 KiB e cada gravacao e um `rename` atomico: a leitura
    /// ve o ficheiro antigo ou o novo, nunca meio). Se nao se conseguiu ler,
    /// fica o que se leu antes -- nunca zero no lugar do que la estava.
    fn month_total(&mut self, month: &str) -> u32 {
        let mut files = lock_files(&self.files);
        if let Some(reader) = files.reader.as_mut()
            && !sticky(reader.read_only())
        {
            match reader.load() {
                LoadOutcome::Loaded(book) | LoadOutcome::Missing(book) => {
                    lock_view(&self.view).disk = book;
                }
                LoadOutcome::Degraded { .. } => {}
            }
        }
        drop(files);
        lock_view(&self.view).month_total(month)
    }

    fn count(&mut self, delta: UsageDelta) {
        if !self.persistent {
            lock_view(&self.view).kept.add(&delta);
            return;
        }
        lock_view(&self.view).unsaved.push(delta);
        let _ = self.worker.submit(UsageFlush);
    }
}

/// Grava o que a janela contou: soma as contas por gravar ao ficheiro
/// debaixo do trinco e, de uma vez, tira-as de `unsaved` e fica com o que o
/// disco diz (as outras janelas incluidas). Com a loja so de leitura ou um
/// erro de disco, essas contas ficam so em memoria (`kept`).
fn flush_usage(files: &Mutex<UsageFiles>, view: &Mutex<UsageView>) {
    let mut files = lock_files(files);
    let Some(writer) = files.writer.as_mut() else {
        return;
    };
    let batch = lock_view(view).unsaved.clone();
    if batch.is_empty() {
        return;
    }
    let written = writer.update(|book| {
        for delta in &batch {
            book.add(delta);
        }
        book.clone()
    });
    // `count` so acrescenta no fim e so esta thread tira: o lote ainda e o
    // comeco de `unsaved`.
    let mut view = lock_view(view);
    view.unsaved.drain(..batch.len());
    match written {
        Ok((fresh, SaveOutcome::Written)) => view.disk = fresh,
        _ => {
            for delta in &batch {
                view.kept.add(delta);
            }
        }
    }
}

/// O portao. Um por janela, criado na primeira vez que uma feature o pede
/// (`App::egress_gate`), nunca no `App::new`.
pub(crate) struct EgressGate {
    consents: ConsentLedger,
    site_grants: BTreeMap<AiPurpose, SiteGrants>,
    usage: UsageLedger,
    settings: Option<VersionedJsonStore<AiSettings>>,
    settings_cache: Option<AiSettings>,
}

impl EgressGate {
    /// O portao do produto: `ai/settings.json` e `ai/usage.json` pelos
    /// grants do registo. Nao le nem escreve nada e nao cria thread nenhuma
    /// (gate `lazy_worker_spawns_nothing_until_first_job`). Sem registo, o
    /// consumo conta so em memoria e o limite e o de omissao.
    pub(crate) fn for_app(stores: Option<&StoreRegistry>) -> Self {
        let settings = stores
            .and_then(|registry| registry.grant(AI_SETTINGS_STORE).ok())
            .and_then(|grant| {
                VersionedJsonStore::open(grant, AI_SETTINGS_VERSION, AI_SETTINGS_MAX_BYTES).ok()
            });
        let reader = stores
            .and_then(|registry| registry.grant(AI_USAGE_STORE).ok())
            .and_then(|grant| {
                VersionedJsonStore::open(grant, AI_USAGE_VERSION, AI_USAGE_MAX_BYTES).ok()
            });
        let writer = stores
            .and_then(|registry| registry.grant(AI_USAGE_STORE).ok())
            .and_then(|grant| {
                VersionedJsonStore::open(grant, AI_USAGE_VERSION, AI_USAGE_MAX_BYTES).ok()
            })
            .map(VersionedJsonStore::shared_between_windows);
        Self {
            consents: ConsentLedger::default(),
            site_grants: BTreeMap::new(),
            usage: UsageLedger::new(UsageFiles { reader, writer }),
            settings,
            settings_cache: None,
        }
    }

    /// Liga a loja «Sempre neste site» de uma finalidade que a oferece (a
    /// Traducao traz a sua, `Setting`).
    pub(crate) fn attach_site_grants(
        &mut self,
        feature: AiPurpose,
        grant: StoreGrant,
    ) -> Result<(), SiteGrantsError> {
        if !offers_site_grant(feature) {
            return Err(SiteGrantsError::NotOffered);
        }
        self.site_grants.insert(feature, SiteGrants::open(grant)?);
        Ok(())
    }

    /// Tira «Sempre neste site» de (cerebro, origem) numa finalidade: da
    /// loja (as outras janelas relem-na no pedido seguinte) e desta janela
    /// mesmo que a loja nao grave, e tira tambem o consentimento da sessao
    /// desse par -- o pedido seguinte volta a perguntar. Verdadeiro se a loja
    /// gravou a revogacao.
    pub(crate) fn revoke_site_grant(
        &mut self,
        feature: AiPurpose,
        brain: Brain,
        origin: &SiteOrigin,
    ) -> bool {
        let brain = brain.key();
        self.consents
            .forget(&brain, &ConsentScope::Site(origin.clone()));
        self.site_grants
            .get_mut(&feature)
            .is_some_and(|grants| grants.revoke(&brain, origin))
    }

    /// O limite mensal suave (do `ai/settings.json`, lido uma vez).
    pub(crate) fn soft_cap(&mut self) -> u32 {
        let settings = &mut self.settings;
        self.settings_cache
            .get_or_insert_with(|| {
                settings
                    .as_mut()
                    .map(|store| store.load().into_value())
                    .unwrap_or_default()
            })
            .soft_cap()
    }

    /// As chamadas pagas deste mes: o `ai/usage.json` relido agora (todas as
    /// janelas, ate a ultima gravacao de cada uma) mais o que esta janela
    /// contou e ainda nao gravou.
    pub(crate) fn usage_this_month(&mut self, today: Day) -> u32 {
        self.usage.month_total(&today.month_key())
    }

    fn facts(&mut self, request: &EgressRequest<'_>, today: Day) -> Facts {
        let brain = request.destination.brain.key();
        let origin = request.origin.as_ref();
        let scope = ConsentScope::of(request.feature, request.data, origin);
        let session = self.consents.allows(&brain, &scope);
        // «Sempre neste site» so conta fora de qualquer contexto privado, e
        // e relido da loja agora (uma revogacao noutra janela ja vale).
        let site = request.privacy == EgressPrivacy::Normal
            && origin.is_some_and(|origin| {
                self.site_grants
                    .get_mut(&request.feature)
                    .is_some_and(|grants| grants.allows(&brain, origin))
            });
        // So um destino pago conta no limite: so entao se rele o consumo.
        let used = if request.destination.paid {
            self.usage_this_month(today)
        } else {
            0
        };
        Facts {
            consented: session || site,
            used,
            cap: self.soft_cap(),
            today,
            site_grants: self.site_grants.contains_key(&request.feature),
        }
    }

    /// O pedido: a decisao, e num `Send` a conta das chamadas pagas e o uso
    /// da autorizacao em segundo plano.
    pub(crate) fn request(&mut self, mut request: EgressRequest<'_>, today: Day) -> Decision {
        let facts = self.facts(&request, today);
        let decision = decide(&request, &facts);
        if decision == Decision::Send {
            self.sent(&mut request, today);
        }
        decision
    }

    /// A resposta ao cartao: regista o consentimento que o utilizador deu e
    /// manda (o limite, se o cartao o mostrava, ficou confirmado para ESTE
    /// envio). O que nao depende da resposta (modo privado, a memoria) volta
    /// a ser conferido. «Sempre neste site» gravado vive so na loja (uma
    /// revogacao tira-o de vez); num cartao que nao o oferecia, ou com a loja
    /// sem o conseguir gravar, vale so para a sessao e nunca chega ao disco.
    pub(crate) fn answer(
        &mut self,
        card: ConsentCard,
        answer: ConsentAnswer,
        today: Day,
    ) -> Decision {
        if answer == ConsentAnswer::Cancel {
            return Decision::Refuse(RefuseReason::Cancelled);
        }
        let needs_consent = card.needs_consent;
        let (mut request, offer_always) = card.into_click();
        let brain = request.destination.brain.key();
        let persisted = needs_consent
            && answer == ConsentAnswer::AlwaysOnSite
            && offer_always
            && match (
                request.origin.as_ref(),
                self.site_grants.get_mut(&request.feature),
            ) {
                (Some(origin), Some(grants)) => grants.allow(&brain, origin),
                _ => false,
            };
        if needs_consent
            && !persisted
            && matches!(answer, ConsentAnswer::Session | ConsentAnswer::AlwaysOnSite)
        {
            let scope = ConsentScope::of(request.feature, request.data, request.origin.as_ref());
            self.consents.allow(brain, scope);
        }
        let answered = Facts {
            consented: true,
            used: 0,
            cap: u32::MAX,
            today,
            site_grants: false,
        };
        let decision = decide(&request, &answered);
        if decision == Decision::Send {
            self.sent(&mut request, today);
        }
        decision
    }

    /// O cartao que cria uma autorizacao em segundo plano: sempre um
    /// cartao, so para o que o utilizador escreveu, so fora de qualquer
    /// contexto privado. `max_per_day` fica em 1..=24 e `days` em 1..=30.
    pub(crate) fn standing_grant_card(
        &mut self,
        request: &EgressRequest<'_>,
        max_per_day: u32,
        days: u32,
    ) -> Result<StandingGrantCard, RefuseReason> {
        if request.privacy != EgressPrivacy::Normal {
            return Err(RefuseReason::PrivateMode);
        }
        if may_send(request.data, request.destination.locality) == MaySend::Never {
            return Err(RefuseReason::NeverLeavesThePc(request.data));
        }
        if request.data != DataClass::UserTyped {
            return Err(RefuseReason::BackgroundData(request.data));
        }
        Ok(StandingGrantCard {
            card: ConsentCard {
                feature: request.feature,
                data: request.data,
                destination: request.destination.clone(),
                origin: request.origin.clone(),
                token_estimate: request.token_estimate,
                calls: request.calls(),
                privacy: request.privacy,
                needs_consent: true,
                cap: None,
                offer_always: false,
            },
            max_per_day: max_per_day.clamp(1, MAX_STANDING_GRANT_PER_DAY),
            days: days.clamp(1, MAX_STANDING_GRANT_DAYS),
        })
    }

    fn sent(&mut self, request: &mut EgressRequest<'_>, today: Day) {
        if let Trigger::Background {
            grant: Some(grant), ..
        } = &mut request.trigger
        {
            grant.record_use(today);
        }
        if request.destination.paid {
            self.usage.count(UsageDelta {
                month: today.month_key(),
                brain: request.destination.brain.key(),
                purpose: request.feature.key().to_string(),
                calls: request.calls(),
            });
        }
    }

    /// Quantas threads o portao criou (0 ate a primeira chamada paga).
    #[cfg(test)]
    pub(crate) fn worker_threads_spawned(&self) -> usize {
        self.usage.worker.threads_spawned()
    }

    /// Espera que a gravacao do consumo acabe. So nos testes.
    #[cfg(test)]
    pub(crate) fn wait_usage_written(&self) -> bool {
        self.usage
            .worker
            .wait_idle(std::time::Duration::from_secs(10))
    }
}

/// `9800` -> `9.800`.
fn thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('.');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use neural_core::json_store::{StoreMode, StoreShape, StoreSpec};
    use neural_core::llm::{ModelId, PickSource, Purpose};
    use std::path::{Path, PathBuf};

    /// 2026-09-26.
    const TODAY: Day = Day(20_722);
    const WATCH: &str = "radar-7";

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("neuralia-egress-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Todos os ficheiros debaixo de `dir`, relativos, com `/`.
    fn files_under(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(next) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&next) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(relative) = path.strip_prefix(dir) {
                    out.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        out.sort();
        out
    }

    fn gemini() -> Destination {
        Destination::cloud(&Pick {
            purpose: Purpose::Translation,
            provider: Provider::Gemini,
            model: ModelId::parse("gemini-9.9-flash-lite").expect("id"),
            price_tier: PriceTier::Low,
            source: PickSource::Rule,
        })
    }

    /// Um destino em cada lugar: a Gemini na Internet, o perfil BYOM 1 na
    /// rede local ou neste PC, e os motores do processo.
    fn at(locality: Locality, paid: bool) -> Destination {
        let brain = match locality {
            Locality::Remote => Brain::Cloud(Provider::Gemini),
            Locality::Lan | Locality::Loopback => Brain::Byom(1),
            Locality::Local => Brain::OnDevice,
        };
        Destination {
            brain,
            locality,
            hosts: vec![format!("{locality:?}.test").to_ascii_lowercase()],
            model: None,
            price_tier: None,
            paid,
        }
    }

    fn click<'a>(
        feature: AiPurpose,
        data: DataClass,
        destination: Destination,
        privacy: EgressPrivacy,
    ) -> EgressRequest<'a> {
        EgressRequest {
            feature,
            data,
            destination,
            origin: SiteOrigin::of_url("https://Exemplo.com/artigo?x=1#y"),
            token_estimate: 3_200,
            calls: 1,
            trigger: Trigger::Click,
            privacy,
        }
    }

    fn background<'a>(
        data: DataClass,
        destination: Destination,
        privacy: EgressPrivacy,
        watch_id: &'a str,
        grant: Option<&'a mut StandingGrant>,
    ) -> EgressRequest<'a> {
        EgressRequest {
            feature: AiPurpose::Dictation,
            data,
            destination,
            origin: None,
            token_estimate: 800,
            calls: 1,
            trigger: Trigger::Background { watch_id, grant },
            privacy,
        }
    }

    fn facts(consented: bool, used: u32, cap: u32, site_grants: bool) -> Facts {
        Facts {
            consented,
            used,
            cap,
            today: TODAY,
            site_grants,
        }
    }

    /// Uma autorizacao pelo caminho do produto: o cartao, e o sim.
    fn standing(watch_id: &str, destination: &Destination, per_day: u32) -> StandingGrant {
        let mut gate = EgressGate::for_app(None);
        let request = click(
            AiPurpose::Dictation,
            DataClass::UserTyped,
            destination.clone(),
            EgressPrivacy::Normal,
        );
        let card = gate
            .standing_grant_card(&request, per_day, 30)
            .expect("cartao");
        StandingGrant::issue(&card, ConsentAnswer::Once, watch_id, TODAY).expect("sim")
    }

    fn ask(decision: Decision) -> ConsentCard {
        match decision {
            Decision::Ask(card) => card,
            other => panic!("esperava um cartao, veio {other:?}"),
        }
    }

    /// Gate (critico, infra-egress): a tabela da decisao -- clique e
    /// segundo plano x classe do dado x autorizacao x privacidade x limite.
    /// Primeiro as linhas ditas a mao, depois as regras sobre o produto
    /// inteiro das entradas.
    #[test]
    fn decide_table() {
        use DataClass::*;
        use EgressPrivacy::*;
        let normal = |data, destination, consented, used, cap| {
            decide(
                &click(AiPurpose::Translation, data, destination, Normal),
                &facts(consented, used, cap, false),
            )
        };
        // Clique.
        let card = ask(normal(PageContent, gemini(), false, 0, 200));
        assert!(card.needs_consent() && card.cap().is_none() && !card.offers_always());
        assert_eq!(normal(PageContent, gemini(), true, 0, 200), Decision::Send);
        assert_eq!(
            normal(Memory, gemini(), true, 0, 200),
            Decision::Refuse(RefuseReason::NeverLeavesThePc(Memory))
        );
        assert!(normal(Memory, at(Locality::Lan, false), false, 0, 200) != Decision::Send);
        assert_eq!(
            normal(Media, at(Locality::Loopback, false), false, 0, 200),
            Decision::Send
        );
        assert_eq!(
            normal(Memory, at(Locality::Local, false), false, 999, 0),
            Decision::Send
        );
        // O limite: atingido pergunta, com ou sem consentimento; nao pago
        // nao conta.
        let card = ask(normal(PageContent, gemini(), true, 200, 200));
        assert!(!card.needs_consent());
        assert_eq!(
            card.cap(),
            Some(CapReached {
                used: 200,
                cap: 200
            })
        );
        assert_eq!(
            card.title(),
            "Limite mensal atingido (200 chamadas) — continuar?"
        );
        let card = ask(normal(PageContent, gemini(), false, 250, 200));
        assert!(card.needs_consent() && card.cap().is_some());
        assert_eq!(
            normal(UserTyped, at(Locality::Remote, false), true, 900, 200),
            Decision::Send
        );
        // Um pedido de 5 chamadas que passa o limite pergunta antes.
        let mut five = click(AiPurpose::Translation, PageContent, gemini(), Normal);
        five.calls = 5;
        let card = ask(decide(&five, &facts(true, 199, 200, false)));
        assert_eq!(
            card.cap().map(CapReached::question).as_deref(),
            Some("Este pedido passa o limite mensal (199 de 200 chamadas) — continuar?")
        );
        assert_eq!(decide(&five, &facts(true, 195, 200, false)), Decision::Send);
        // «Sempre neste site» so com a loja, so na Traducao, so fora do
        // privado, so com site.
        let offered = |feature, privacy, site_grants, origin: bool| {
            let mut request = click(feature, PageContent, gemini(), privacy);
            if !origin {
                request.origin = None;
            }
            match decide(&request, &facts(false, 0, 200, site_grants)) {
                Decision::Ask(card) => Some(card.offers_always()),
                _ => None,
            }
        };
        assert_eq!(
            offered(AiPurpose::Translation, Normal, true, true),
            Some(true)
        );
        assert_eq!(
            offered(AiPurpose::Translation, Normal, false, true),
            Some(false)
        );
        assert_eq!(
            offered(AiPurpose::Translation, Normal, true, false),
            Some(false)
        );
        assert_eq!(
            offered(AiPurpose::Dictation, Normal, true, true),
            Some(false)
        );
        assert_eq!(
            offered(AiPurpose::Translation, PrivateSurface, true, true),
            Some(false)
        );
        assert_eq!(
            offered(AiPurpose::Translation, PrivateMode, true, true),
            None
        );
        // Modo privado: nada sai; o que fica no PC vai.
        let private = |destination| {
            decide(
                &click(
                    AiPurpose::Translation,
                    PageContent,
                    destination,
                    PrivateMode,
                ),
                &facts(true, 0, 200, true),
            )
        };
        assert_eq!(
            private(gemini()),
            Decision::Refuse(RefuseReason::PrivateMode)
        );
        assert_eq!(
            private(at(Locality::Lan, false)),
            Decision::Refuse(RefuseReason::PrivateMode)
        );
        assert_eq!(private(at(Locality::Loopback, false)), Decision::Send);

        // Segundo plano.
        let remote = gemini();
        let mut grant = standing(WATCH, &remote, 3);
        let mut other = standing("outra-vigia", &remote, 3);
        let mut byom = standing(WATCH, &at(Locality::Loopback, false), 3);
        let consented = facts(true, 0, 200, true);
        let bg = |data: DataClass, grant: Option<&mut StandingGrant>, privacy: EgressPrivacy| {
            decide(
                &background(data, gemini(), privacy, WATCH, grant),
                &consented,
            )
        };
        assert_eq!(bg(UserTyped, Some(&mut grant), Normal), Decision::Send);
        assert_eq!(
            bg(UserTyped, None, Normal),
            Decision::Refuse(RefuseReason::BackgroundWithoutGrant)
        );
        assert_eq!(
            bg(PageContent, Some(&mut grant), Normal),
            Decision::Refuse(RefuseReason::BackgroundData(PageContent))
        );
        assert_eq!(
            bg(Memory, Some(&mut grant), Normal),
            Decision::Refuse(RefuseReason::NeverLeavesThePc(Memory))
        );
        assert_eq!(
            bg(UserTyped, Some(&mut other), Normal),
            Decision::Refuse(RefuseReason::GrantForAnotherWatch)
        );
        assert_eq!(
            bg(UserTyped, Some(&mut byom), Normal),
            Decision::Refuse(RefuseReason::GrantForAnotherBrain)
        );
        assert_eq!(
            bg(UserTyped, Some(&mut grant), PrivateMode),
            Decision::Refuse(RefuseReason::PrivateMode)
        );
        assert_eq!(
            decide(
                &background(UserTyped, gemini(), Normal, WATCH, Some(&mut grant)),
                &facts(true, 200, 200, false)
            ),
            Decision::Refuse(RefuseReason::OverCap {
                used: 200,
                cap: 200
            })
        );
        assert_eq!(
            decide(
                &background(
                    UserTyped,
                    at(Locality::Loopback, false),
                    Normal,
                    WATCH,
                    None
                ),
                &consented
            ),
            Decision::Refuse(RefuseReason::BackgroundWithoutGrant),
            "em segundo plano nem o loopback vai sem autorizacao"
        );

        // As regras sobre o produto inteiro das entradas. Gatilho 0 e o
        // clique; 1, 2 e 3 sao o segundo plano sem autorizacao, com a desta
        // vigia e com a de outra.
        let mut rows = 0;
        for locality in Locality::ALL {
            for paid in [false, true] {
                let destination = at(locality, paid);
                let matching = standing(WATCH, &destination, 3);
                let foreign = standing("outra", &destination, 3);
                for data in DataClass::ALL {
                    for consented in [false, true] {
                        for used in [0, 200] {
                            for privacy in [Normal, PrivateSurface, PrivateMode] {
                                for site_grants in [false, true] {
                                    for trigger in 0..4 {
                                        rows += 1;
                                        let mut matching = matching.clone();
                                        let mut foreign = foreign.clone();
                                        let request = match trigger {
                                            0 => click(
                                                AiPurpose::Translation,
                                                data,
                                                destination.clone(),
                                                privacy,
                                            ),
                                            1 => background(
                                                data,
                                                destination.clone(),
                                                privacy,
                                                WATCH,
                                                None,
                                            ),
                                            2 => background(
                                                data,
                                                destination.clone(),
                                                privacy,
                                                WATCH,
                                                Some(&mut matching),
                                            ),
                                            _ => background(
                                                data,
                                                destination.clone(),
                                                privacy,
                                                WATCH,
                                                Some(&mut foreign),
                                            ),
                                        };
                                        let facts = facts(consented, used, 200, site_grants);
                                        let decision = decide(&request, &facts);
                                        let leaves = locality.leaves_the_pc();
                                        let over = paid && used >= 200;
                                        let context = format!(
                                            "{data:?} {locality:?} pago={paid} consentido={consented} usado={used} {privacy:?} loja={site_grants} gatilho={trigger}"
                                        );
                                        if privacy == PrivateMode && leaves {
                                            assert_eq!(
                                                decision,
                                                Decision::Refuse(RefuseReason::PrivateMode),
                                                "{context}"
                                            );
                                        }
                                        if data == Memory && locality == Locality::Remote {
                                            assert!(
                                                matches!(decision, Decision::Refuse(_)),
                                                "{context}"
                                            );
                                        }
                                        if trigger > 0 {
                                            assert!(
                                                !matches!(decision, Decision::Ask(_)),
                                                "o segundo plano perguntou: {context}"
                                            );
                                            if decision == Decision::Send {
                                                assert!(
                                                    data == UserTyped && trigger == 2 && !over,
                                                    "{context}"
                                                );
                                            }
                                            continue;
                                        }
                                        let allowed = !(privacy == PrivateMode && leaves)
                                            && !(data == Memory && locality == Locality::Remote);
                                        match decision {
                                            Decision::Send => assert!(
                                                allowed && (!leaves || consented) && !over,
                                                "{context}"
                                            ),
                                            Decision::Ask(card) => {
                                                assert!(allowed, "{context}");
                                                assert!(
                                                    card.needs_consent() || card.cap().is_some(),
                                                    "{context}"
                                                );
                                                assert_eq!(
                                                    card.needs_consent(),
                                                    leaves && !consented,
                                                    "{context}"
                                                );
                                                assert_eq!(card.cap().is_some(), over, "{context}");
                                                if card.offers_always() {
                                                    assert!(
                                                        privacy == Normal
                                                            && site_grants
                                                            && card.needs_consent(),
                                                        "{context}"
                                                    );
                                                }
                                            }
                                            Decision::Refuse(_) => assert!(!allowed, "{context}"),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(rows, 4 * 2 * 5 * 2 * 2 * 3 * 2 * 4);
    }

    /// Gate (critico, infra-egress; critica C7): em segundo plano nada vai
    /// sem a autorizacao DESTA vigia e DESTE cerebro, valida hoje, e so o
    /// que o utilizador escreveu -- nem com consentimento da sessao, nem com
    /// «Sempre neste site». E nunca pergunta.
    #[test]
    fn background_never_sends_without_matching_grant() {
        let remote = gemini();
        let loopback = at(Locality::Loopback, false);
        let grants = |destination: &Destination| -> Vec<(&'static str, StandingGrant)> {
            let valid = standing(WATCH, destination, 3);
            let mut revoked = valid.clone();
            revoked.revoke();
            let mut exhausted = valid.clone();
            for _ in 0..3 {
                exhausted.record_use(TODAY);
            }
            let mut stretched = valid.clone();
            stretched.issued = Day(TODAY.0 - 31);
            stretched.expires = TODAY.plus(400);
            let mut future = valid.clone();
            future.issued = TODAY.plus(1);
            let mut greedy = valid.clone();
            greedy.max_per_day = 1000;
            greedy.used_on = Some(TODAY);
            greedy.used_today = MAX_STANDING_GRANT_PER_DAY;
            vec![
                ("valida", valid),
                ("outra vigia", standing("radar-8", destination, 3)),
                (
                    "outro cerebro",
                    standing(
                        WATCH,
                        &Destination {
                            brain: Brain::Byom(2),
                            ..destination.clone()
                        },
                        3,
                    ),
                ),
                ("revogada", revoked),
                ("esgotada hoje", exhausted),
                ("esticada no ficheiro", stretched),
                ("ainda nao valida", future),
                ("limite por dia inflado", greedy),
            ]
        };
        let mut sends = 0;
        for destination in [remote.clone(), loopback.clone()] {
            for (label, grant) in grants(&destination) {
                for data in DataClass::ALL {
                    for privacy in [
                        EgressPrivacy::Normal,
                        EgressPrivacy::PrivateSurface,
                        EgressPrivacy::PrivateMode,
                    ] {
                        for consented in [false, true] {
                            for used in [0, 200] {
                                let mut grant = grant.clone();
                                let decision = decide(
                                    &background(
                                        data,
                                        destination.clone(),
                                        privacy,
                                        WATCH,
                                        Some(&mut grant),
                                    ),
                                    &facts(consented, used, 200, true),
                                );
                                let context = format!(
                                    "{label} {data:?} {:?} {privacy:?} consentido={consented} usado={used}",
                                    destination.locality
                                );
                                assert!(!matches!(decision, Decision::Ask(_)), "{context}");
                                if decision == Decision::Send {
                                    sends += 1;
                                    assert_eq!(label, "valida", "{context}");
                                    assert_eq!(data, DataClass::UserTyped, "{context}");
                                    assert!(
                                        !(privacy == EgressPrivacy::PrivateMode
                                            && destination.locality.leaves_the_pc()),
                                        "{context}"
                                    );
                                }
                                let none = decide(
                                    &background(data, destination.clone(), privacy, WATCH, None),
                                    &facts(consented, used, 200, true),
                                );
                                assert!(
                                    matches!(none, Decision::Refuse(_)),
                                    "sem autorizacao: {context}"
                                );
                            }
                        }
                    }
                }
            }
        }
        // Nao e vacuo: a autorizacao valida manda o que o utilizador
        // escreveu (Internet: Normal e superficie privada, abaixo do limite;
        // loopback: os tres modos e os dois usos, porque nao e pago).
        assert_eq!(sends, 2 * 2 + 3 * 2 * 2);

        // Pelo portao: a autorizacao gasta-se por dia, e a sessao nao a
        // substitui.
        let mut gate = EgressGate::for_app(None);
        let card = ask(gate.request(
            click(
                AiPurpose::Dictation,
                DataClass::UserTyped,
                remote.clone(),
                EgressPrivacy::Normal,
            ),
            TODAY,
        ));
        assert_eq!(
            gate.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        let mut grant = standing(WATCH, &remote, 2);
        let mut run = |grant: Option<&mut StandingGrant>, today| {
            gate.request(
                background(
                    DataClass::UserTyped,
                    remote.clone(),
                    EgressPrivacy::Normal,
                    WATCH,
                    grant,
                ),
                today,
            )
        };
        assert_eq!(
            run(None, TODAY),
            Decision::Refuse(RefuseReason::BackgroundWithoutGrant)
        );
        assert_eq!(run(Some(&mut grant), TODAY), Decision::Send);
        assert_eq!(run(Some(&mut grant), TODAY), Decision::Send);
        assert_eq!(
            run(Some(&mut grant), TODAY),
            Decision::Refuse(RefuseReason::GrantDailyLimit)
        );
        assert_eq!(run(Some(&mut grant), TODAY.plus(1)), Decision::Send);
        assert_eq!(
            run(Some(&mut grant), TODAY.plus(30)),
            Decision::Refuse(RefuseReason::GrantExpired)
        );
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("pasta")).expect("pasta");
        std::fs::write(path, text).expect("escrever");
    }

    fn usage_on_disk(dir: &Path) -> serde_json::Value {
        let text = std::fs::read_to_string(dir.join("ai").join("usage.json")).expect("usage.json");
        serde_json::from_str(&text).expect("json")
    }

    /// Gate (critico, infra-egress; critica C4): cada chamada paga que o
    /// portao deixa sair conta no `ai/usage.json`, e acima do limite
    /// mensal o clique pergunta SEMPRE (nunca manda calado, nunca recusa
    /// calado); duas janelas nunca perdem a conta uma da outra.
    #[test]
    fn ledger_counts_every_paid_call_and_caps() {
        let dir = temp_dir("ledger");
        // O limite vem do ai/settings.json: 3 chamadas.
        write(
            &dir.join("ai").join("settings.json"),
            r#"{"version":1,"data":{"monthly_soft_cap":3}}"#,
        );
        let registry = StoreRegistry::mint_for_test(&dir);
        let mut gate = EgressGate::for_app(Some(&registry));
        assert_eq!(gate.soft_cap(), 3);
        let page = |calls| EgressRequest {
            calls,
            ..click(
                AiPurpose::Translation,
                DataClass::PageContent,
                gemini(),
                EgressPrivacy::Normal,
            )
        };
        // O primeiro envio pede consentimento; a resposta manda e conta.
        let card = ask(gate.request(page(1), TODAY));
        assert_eq!(gate.usage_this_month(TODAY), 0, "o cartao nao conta");
        assert_eq!(
            gate.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        assert_eq!(gate.usage_this_month(TODAY), 1);
        // Um pedido de 2 chamadas conta 2.
        assert_eq!(gate.request(page(2), TODAY), Decision::Send);
        assert_eq!(gate.usage_this_month(TODAY), 3);
        // No limite: pergunta, sem contar; «Continuar» manda e conta.
        let card = ask(gate.request(page(1), TODAY));
        assert!(!card.needs_consent());
        assert_eq!(
            card.title(),
            "Limite mensal atingido (3 chamadas) — continuar?"
        );
        assert_eq!(
            card.answers(),
            vec![ConsentAnswer::Once, ConsentAnswer::Cancel]
        );
        assert_eq!(card.answer_label(ConsentAnswer::Once), "Continuar");
        assert_eq!(gate.usage_this_month(TODAY), 3);
        assert_eq!(
            gate.answer(card, ConsentAnswer::Once, TODAY),
            Decision::Send
        );
        assert_eq!(gate.usage_this_month(TODAY), 4);
        // E pergunta de novo no seguinte: confirmar uma vez nao cala o
        // limite.
        let card = ask(gate.request(page(1), TODAY));
        assert_eq!(
            gate.answer(card, ConsentAnswer::Cancel, TODAY),
            Decision::Refuse(RefuseReason::Cancelled)
        );
        assert_eq!(gate.usage_this_month(TODAY), 4);
        // O que nao e pago nao conta nem e travado pelo limite.
        let byom_remote = at(Locality::Remote, false);
        assert_eq!(
            gate.request(
                click(
                    AiPurpose::Translation,
                    DataClass::PageContent,
                    byom_remote,
                    EgressPrivacy::Normal
                ),
                TODAY
            ),
            Decision::Send,
            "a Gemini tem consentimento na sessao para exemplo.com"
        );
        assert_eq!(
            gate.request(
                click(
                    AiPurpose::Translation,
                    DataClass::PageContent,
                    at(Locality::Loopback, false),
                    EgressPrivacy::Normal
                ),
                TODAY
            ),
            Decision::Send
        );
        assert_eq!(gate.usage_this_month(TODAY), 4);
        // Noutro mes o contador recomeca.
        assert_eq!(gate.usage_this_month(TODAY.plus(10)), 0);

        // No disco, por mes, cerebro e finalidade.
        assert!(gate.wait_usage_written());
        let disk = usage_on_disk(&dir);
        assert_eq!(disk["version"], 1);
        assert_eq!(disk["data"]["months"]["2026-09"]["gemini"]["traducao"], 4);

        // Outra janela (outro processo: outro registo, o mesmo ficheiro) le
        // o que esta conta, e as duas somam sem perder nada.
        let other_registry = StoreRegistry::mint_for_test(&dir);
        let mut other = EgressGate::for_app(Some(&other_registry));
        assert_eq!(other.usage_this_month(TODAY), 4);
        let card = ask(other.request(page(1), TODAY));
        assert_eq!(
            other.answer(card, ConsentAnswer::Once, TODAY),
            Decision::Send
        );
        let card = ask(gate.request(page(1), TODAY));
        assert_eq!(
            gate.answer(card, ConsentAnswer::Once, TODAY),
            Decision::Send
        );
        assert!(other.wait_usage_written() && gate.wait_usage_written());
        assert_eq!(
            usage_on_disk(&dir)["data"]["months"]["2026-09"]["gemini"]["traducao"],
            6
        );
        let fresh_registry = StoreRegistry::mint_for_test(&dir);
        assert_eq!(
            EgressGate::for_app(Some(&fresh_registry)).usage_this_month(TODAY),
            6
        );

        // Sem registo o consumo conta em memoria com o limite de omissao.
        let mut memory_only = EgressGate::for_app(None);
        assert_eq!(memory_only.soft_cap(), 200);
        let card = ask(memory_only.request(page(1), TODAY));
        assert_eq!(
            memory_only.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        for _ in 1..200 {
            assert_eq!(memory_only.request(page(1), TODAY), Decision::Send);
        }
        assert_eq!(memory_only.usage_this_month(TODAY), 200);
        assert!(ask(memory_only.request(page(1), TODAY)).cap().is_some());
        assert_eq!(memory_only.worker_threads_spawned(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Gate (critico, infra-egress; critica C4, revisao EG-1): o limite de
    /// uma janela conta o que as outras ja gravaram. Com o limite em 3, a
    /// janela A manda 1 e a B manda 2: o pedido seguinte de A pergunta, de 1
    /// ou de 2 chamadas. Antes, A decidia com o `usage.json` que leu no
    /// primeiro pedido (1 de 3) e mandava calada ate 5.
    #[test]
    fn the_cap_counts_what_other_windows_sent() {
        let dir = temp_dir("cap-windows");
        write(
            &dir.join("ai").join("settings.json"),
            r#"{"version":1,"data":{"monthly_soft_cap":3}}"#,
        );
        let page = |calls| EgressRequest {
            calls,
            ..click(
                AiPurpose::Translation,
                DataClass::PageContent,
                gemini(),
                EgressPrivacy::Normal,
            )
        };
        let on_disk =
            || usage_on_disk(&dir)["data"]["months"]["2026-09"]["gemini"]["traducao"].clone();
        // Duas janelas: dois registos, a mesma pasta de dados.
        let registry_a = StoreRegistry::mint_for_test(&dir);
        let registry_b = StoreRegistry::mint_for_test(&dir);
        let mut a = EgressGate::for_app(Some(&registry_a));
        let mut b = EgressGate::for_app(Some(&registry_b));
        let card = ask(a.request(page(1), TODAY));
        assert_eq!(
            a.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        assert!(a.wait_usage_written());
        let card = ask(b.request(page(2), TODAY));
        assert_eq!(card.cap(), None, "B ve o 1 de A: 1 + 2 cabe em 3");
        assert_eq!(
            b.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        assert!(b.wait_usage_written());
        assert_eq!(on_disk(), 3);
        // A tem consentimento na sessao: so o limite pergunta, e pergunta.
        for calls in [1, 2] {
            let card = ask(a.request(page(calls), TODAY));
            assert!(!card.needs_consent(), "{calls} chamadas");
            assert_eq!(card.cap(), Some(CapReached { used: 3, cap: 3 }));
            assert_eq!(
                card.title(),
                "Limite mensal atingido (3 chamadas) — continuar?"
            );
        }
        assert_eq!(a.usage_this_month(TODAY), 3);
        assert_eq!(on_disk(), 3, "nada saiu sem o sim");
        // E ao contrario: o «Continuar» de A conta no limite de B.
        let card = ask(a.request(page(1), TODAY));
        assert_eq!(a.answer(card, ConsentAnswer::Once, TODAY), Decision::Send);
        assert!(a.wait_usage_written());
        let card = ask(b.request(page(1), TODAY));
        assert_eq!(card.cap(), Some(CapReached { used: 4, cap: 3 }));
        assert_eq!(on_disk(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Gate (critico, infra-egress; revisao EG-4): `ai/usage.json` e
    /// `Setting`. Com o modo privado das lojas ligado (o `StoreMode` do
    /// registo e a `EgressPrivacy` do pedido podem divergir), as chamadas
    /// pagas continuam a ser gravadas e a sessao seguinte ve-as: o limite
    /// nao recomeca. Como `Automatic`, a gravacao saltava e a conta morria
    /// com a janela.
    #[test]
    fn usage_survives_the_private_store_mode() {
        let dir = temp_dir("usage-private");
        let registry = StoreRegistry::mint_for_test(&dir);
        registry.set_mode(StoreMode::Private);
        let mut gate = EgressGate::for_app(Some(&registry));
        let page = || {
            click(
                AiPurpose::Translation,
                DataClass::PageContent,
                gemini(),
                EgressPrivacy::Normal,
            )
        };
        let card = ask(gate.request(page(), TODAY));
        assert_eq!(
            gate.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        for _ in 0..4 {
            assert_eq!(gate.request(page(), TODAY), Decision::Send);
        }
        assert!(gate.wait_usage_written());
        assert!(
            dir.join("ai").join("usage.json").exists(),
            "o modo privado das lojas saltou a gravacao do consumo"
        );
        assert_eq!(
            usage_on_disk(&dir)["data"]["months"]["2026-09"]["gemini"]["traducao"],
            5
        );
        let next = StoreRegistry::mint_for_test(&dir);
        next.set_mode(StoreMode::Private);
        assert_eq!(EgressGate::for_app(Some(&next)).usage_this_month(TODAY), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A thread que grava o consumo so nasce na primeira chamada paga: criar
    /// o portao, perguntar, recusar, mandar de graca e ler o consumo nao a
    /// criam nem escrevem nada.
    #[test]
    fn the_usage_writer_starts_on_the_first_paid_call() {
        let dir = temp_dir("lazy");
        let registry = StoreRegistry::mint_for_test(&dir);
        let mut gate = EgressGate::for_app(Some(&registry));
        assert_eq!(gate.worker_threads_spawned(), 0);
        let page = || {
            click(
                AiPurpose::Translation,
                DataClass::PageContent,
                gemini(),
                EgressPrivacy::Normal,
            )
        };
        let card = ask(gate.request(page(), TODAY));
        assert!(matches!(
            gate.request(
                click(
                    AiPurpose::Translation,
                    DataClass::Memory,
                    gemini(),
                    EgressPrivacy::Normal
                ),
                TODAY
            ),
            Decision::Refuse(_)
        ));
        assert_eq!(
            gate.request(
                click(
                    AiPurpose::Translation,
                    DataClass::PageContent,
                    at(Locality::Loopback, false),
                    EgressPrivacy::Normal
                ),
                TODAY
            ),
            Decision::Send
        );
        assert_eq!(gate.usage_this_month(TODAY), 0);
        assert_eq!(gate.soft_cap(), 200);
        assert_eq!(gate.worker_threads_spawned(), 0, "sem chamada paga");
        assert!(files_under(&dir).is_empty(), "{:?}", files_under(&dir));

        assert_eq!(
            gate.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        assert_eq!(gate.worker_threads_spawned(), 1);
        // Cada pedido rele o usage.json com a thread a gravar ao lado: a
        // conta e exata a cada passo (o que esta a ser gravado nunca conta
        // duas vezes, no disco e por gravar).
        for sent in 2..=31 {
            assert_eq!(gate.request(page(), TODAY), Decision::Send);
            assert_eq!(gate.usage_this_month(TODAY), sent);
        }
        assert!(gate.wait_usage_written());
        assert_eq!(gate.worker_threads_spawned(), 1, "31 chamadas, uma thread");
        assert_eq!(gate.usage_this_month(TODAY), 31);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Gate (critico, infra-egress): o consentimento da sessao vive so em
    /// memoria -- nenhum ficheiro nasce dele, e uma sessao nova pergunta de
    /// novo. E por par (cerebro, site).
    #[test]
    fn consent_ledger_is_session_only() {
        let dir = temp_dir("session");
        let registry = StoreRegistry::mint_for_test(&dir);
        const SITES: StoreSpec =
            StoreSpec::new("translate-test.json", StoreKind::Setting, StoreShape::File);
        let session = |registry: &StoreRegistry| {
            let mut gate = EgressGate::for_app(Some(registry));
            gate.attach_site_grants(
                AiPurpose::Translation,
                registry.grant(SITES).expect("grant"),
            )
            .expect("loja");
            gate
        };
        // Um destino na Internet nao pago: nada de consumo a gravar.
        let free = at(Locality::Remote, false);
        let request = |url: &str, destination: Destination| EgressRequest {
            origin: SiteOrigin::of_url(url),
            ..click(
                AiPurpose::Translation,
                DataClass::PageContent,
                destination,
                EgressPrivacy::Normal,
            )
        };
        let mut gate = session(&registry);
        let card = ask(gate.request(request("https://exemplo.com/a", free.clone()), TODAY));
        assert!(card.offers_always());
        assert_eq!(
            gate.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        // A mesma origem (outro caminho, maiusculas) ja nao pergunta.
        assert_eq!(
            gate.request(request("https://EXEMPLO.com/b?c", free.clone()), TODAY),
            Decision::Send
        );
        // Outro site, outro cerebro, ou http em vez de https: pergunta.
        assert!(matches!(
            gate.request(request("https://outro.com/", free.clone()), TODAY),
            Decision::Ask(_)
        ));
        assert!(matches!(
            gate.request(
                request("https://exemplo.com/", at(Locality::Lan, false)),
                TODAY
            ),
            Decision::Ask(_)
        ));
        assert!(matches!(
            gate.request(request("http://exemplo.com/", free.clone()), TODAY),
            Decision::Ask(_)
        ));
        assert!(
            files_under(&dir).is_empty(),
            "o consentimento da sessao escreveu {:?}",
            files_under(&dir)
        );
        drop(gate);
        // Sessao nova: pergunta outra vez.
        let other_registry = StoreRegistry::mint_for_test(&dir);
        let mut gate = session(&other_registry);
        assert!(matches!(
            gate.request(request("https://exemplo.com/a", free), TODAY),
            Decision::Ask(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Gate (critico, infra-egress; revisao EG-2): sem site (`file:`,
    /// `neuralia-pdf:`, `data:`, uma origem com mais de 256 caracteres, o
    /// ditado), «Sempre nesta sessão» vale so para a mesma finalidade com a
    /// mesma classe do dado. Antes, o par (cerebro, sem site) cobria tudo: o
    /// sim ao audio do ditado mandava calado o texto de um PDF local.
    #[test]
    fn session_consent_without_a_site_stays_with_its_feature_and_data() {
        use AiPurpose::{Dictation, Translation};
        use DataClass::{Media, PageContent, UserTyped};
        let mut gate = EgressGate::for_app(None);
        let free = at(Locality::Remote, false);
        let from = |feature, data, url: &str| EgressRequest {
            origin: SiteOrigin::of_url(url),
            ..click(feature, data, free.clone(), EgressPrivacy::Normal)
        };
        // O ditado: sem site, e o cartao nao tem linha de site.
        let card = ask(gate.request(from(Dictation, Media, ""), TODAY));
        assert!(!card.lines().iter().any(|line| line.starts_with("Site:")));
        assert_eq!(
            card.answers(),
            vec![
                ConsentAnswer::Once,
                ConsentAnswer::Session,
                ConsentAnswer::Cancel
            ]
        );
        assert_eq!(
            gate.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        assert_eq!(
            gate.request(from(Dictation, Media, ""), TODAY),
            Decision::Send
        );
        // O mesmo cerebro, sem site, noutra finalidade ou com outro dado:
        // pergunta. Nem um site herda o sim dado sem site.
        let long = format!("https://{}.com/", vec!["a".repeat(60); 5].join("."));
        for (feature, data, url) in [
            (Translation, PageContent, "file:///C:/Users/x/contrato.pdf"),
            (
                Translation,
                PageContent,
                "neuralia-pdf://local/contrato.pdf",
            ),
            (Translation, Media, "data:text/html,x"),
            (Translation, PageContent, long.as_str()),
            (Dictation, UserTyped, ""),
        ] {
            let request = from(feature, data, url);
            assert_eq!(request.origin, None, "{url}");
            assert!(
                matches!(gate.request(request, TODAY), Decision::Ask(_)),
                "{feature:?} {data:?} {url}"
            );
        }
        assert!(matches!(
            gate.request(from(Dictation, Media, "https://exemplo.com/"), TODAY),
            Decision::Ask(_)
        ));
        // O sim ao texto de um ficheiro local na Traducao vale para outro
        // ficheiro local na Traducao; nao para outro cerebro.
        let card = ask(gate.request(from(Translation, PageContent, "file:///C:/a.pdf"), TODAY));
        assert_eq!(
            gate.answer(card, ConsentAnswer::Session, TODAY),
            Decision::Send
        );
        assert_eq!(
            gate.request(
                from(Translation, PageContent, "neuralia-pdf://local/b.pdf"),
                TODAY
            ),
            Decision::Send
        );
        let lan = EgressRequest {
            origin: None,
            ..click(
                Translation,
                PageContent,
                at(Locality::Lan, false),
                EgressPrivacy::Normal,
            )
        };
        assert!(matches!(gate.request(lan, TODAY), Decision::Ask(_)));
    }

    /// Gate (critico, infra-egress; revisao EG-3): «Sempre neste site» so
    /// onde o desenho o pede (a Traducao), numa loja `Setting`, nunca vindo
    /// de um contexto privado, e nunca lido num; e revogado, deixa de valer
    /// ja em todas as janelas abertas, nao so nas que nascem depois.
    #[test]
    fn site_grants_only_where_offered_and_never_from_private() {
        let dir = temp_dir("sites");
        let registry = StoreRegistry::mint_for_test(&dir);
        const SITES: StoreSpec =
            StoreSpec::new("translate-test.json", StoreKind::Setting, StoreShape::File);
        let free = at(Locality::Remote, false);
        let page = |privacy| {
            click(
                AiPurpose::Translation,
                DataClass::PageContent,
                free.clone(),
                privacy,
            )
        };
        let mut gate = EgressGate::for_app(Some(&registry));
        // Sem loja ligada, nao se oferece.
        assert!(!ask(gate.request(page(EgressPrivacy::Normal), TODAY)).offers_always());
        gate.attach_site_grants(
            AiPurpose::Translation,
            registry.grant(SITES).expect("grant"),
        )
        .expect("loja");
        // Na superficie privada: sem «Sempre»; forcado, vale so a sessao.
        let card = ask(gate.request(page(EgressPrivacy::PrivateSurface), TODAY));
        assert!(!card.offers_always());
        assert_eq!(
            card.answers(),
            vec![
                ConsentAnswer::Once,
                ConsentAnswer::Session,
                ConsentAnswer::Cancel
            ]
        );
        assert_eq!(
            gate.answer(card, ConsentAnswer::AlwaysOnSite, TODAY),
            Decision::Send
        );
        assert!(
            files_under(&dir).is_empty(),
            "um «Sempre» vindo do privado chegou ao disco: {:?}",
            files_under(&dir)
        );
        // Fora do privado: oferecido e gravado.
        let mut gate = EgressGate::for_app(Some(&registry));
        gate.attach_site_grants(
            AiPurpose::Translation,
            registry.grant(SITES).expect("grant"),
        )
        .expect("loja");
        let card = ask(gate.request(page(EgressPrivacy::Normal), TODAY));
        assert_eq!(
            card.answers(),
            vec![
                ConsentAnswer::Once,
                ConsentAnswer::Session,
                ConsentAnswer::AlwaysOnSite,
                ConsentAnswer::Cancel
            ]
        );
        assert_eq!(
            card.answer_label(ConsentAnswer::AlwaysOnSite),
            "Sempre neste site"
        );
        assert_eq!(
            gate.answer(card, ConsentAnswer::AlwaysOnSite, TODAY),
            Decision::Send
        );
        let text = std::fs::read_to_string(dir.join("translate-test.json")).expect("loja");
        assert!(text.contains("https://exemplo.com"), "{text}");
        // Outra sessao le a loja; a superficie privada nao.
        let mut next = EgressGate::for_app(Some(&registry));
        let mut sites = SiteGrants::open(registry.grant(SITES).expect("grant")).expect("loja");
        next.attach_site_grants(
            AiPurpose::Translation,
            registry.grant(SITES).expect("grant"),
        )
        .expect("loja");
        assert_eq!(
            next.request(page(EgressPrivacy::Normal), TODAY),
            Decision::Send
        );
        assert!(matches!(
            next.request(page(EgressPrivacy::PrivateSurface), TODAY),
            Decision::Ask(_)
        ));
        // Revogar noutra janela tira-o ja (revisao EG-3): a janela que o leu
        // e a que o deu voltam a perguntar no pedido seguinte, sem reiniciar
        // -- a loja e relida, e o «Sempre» gravado nao ficou tambem na sessao.
        let origin = SiteOrigin::of_url("https://exemplo.com").expect("origem");
        assert!(sites.revoke(&free.brain.key(), &origin));
        let text = std::fs::read_to_string(dir.join("translate-test.json")).expect("loja");
        assert!(!text.contains("https://exemplo.com"), "{text}");
        assert!(
            matches!(
                next.request(page(EgressPrivacy::Normal), TODAY),
                Decision::Ask(_)
            ),
            "a janela que leu o «Sempre» ainda manda depois da revogacao"
        );
        assert!(
            matches!(
                gate.request(page(EgressPrivacy::Normal), TODAY),
                Decision::Ask(_)
            ),
            "a janela que deu o «Sempre» ainda manda depois da revogacao"
        );
        let mut after = EgressGate::for_app(Some(&registry));
        after
            .attach_site_grants(
                AiPurpose::Translation,
                registry.grant(SITES).expect("grant"),
            )
            .expect("loja");
        assert!(matches!(
            after.request(page(EgressPrivacy::Normal), TODAY),
            Decision::Ask(_)
        ));
        // Pelo portao: `revoke_site_grant` tira-o da loja (as outras janelas
        // deixam de o ver) e tira o consentimento da sessao desse par; o de
        // outro site fica.
        let card = ask(after.request(page(EgressPrivacy::Normal), TODAY));
        assert_eq!(
            after.answer(card, ConsentAnswer::AlwaysOnSite, TODAY),
            Decision::Send
        );
        assert_eq!(
            next.request(page(EgressPrivacy::Normal), TODAY),
            Decision::Send,
            "o «Sempre» novo vale ja nas outras janelas"
        );
        assert!(after.revoke_site_grant(AiPurpose::Translation, free.brain, &origin));
        assert!(matches!(
            after.request(page(EgressPrivacy::Normal), TODAY),
            Decision::Ask(_)
        ));
        assert!(matches!(
            next.request(page(EgressPrivacy::Normal), TODAY),
            Decision::Ask(_)
        ));
        let other_site = || EgressRequest {
            origin: SiteOrigin::of_url("https://outro.com/"),
            ..page(EgressPrivacy::Normal)
        };
        for request in [page(EgressPrivacy::Normal), other_site()] {
            let card = ask(after.request(request, TODAY));
            assert_eq!(
                after.answer(card, ConsentAnswer::Session, TODAY),
                Decision::Send
            );
        }
        assert_eq!(
            after.request(page(EgressPrivacy::Normal), TODAY),
            Decision::Send
        );
        assert!(after.revoke_site_grant(AiPurpose::Translation, free.brain, &origin));
        assert!(
            matches!(
                after.request(page(EgressPrivacy::Normal), TODAY),
                Decision::Ask(_)
            ),
            "a revogacao deixou o consentimento da sessao desse site"
        );
        assert_eq!(after.request(other_site(), TODAY), Decision::Send);
        // So Setting, so ficheiro, so a Traducao.
        let automatic = registry
            .grant(StoreSpec::new(
                "auto-test.json",
                StoreKind::Automatic,
                StoreShape::File,
            ))
            .expect("grant");
        assert_eq!(
            SiteGrants::open(automatic).err(),
            Some(SiteGrantsError::WrongKind)
        );
        let folder = registry
            .grant(StoreSpec::new(
                "pasta-test",
                StoreKind::Setting,
                StoreShape::Dir,
            ))
            .expect("grant");
        assert_eq!(
            SiteGrants::open(folder).err(),
            Some(SiteGrantsError::WrongShape)
        );
        let dictation = registry
            .grant(StoreSpec::new(
                "ditado-test.json",
                StoreKind::Setting,
                StoreShape::File,
            ))
            .expect("grant");
        assert_eq!(
            after.attach_site_grants(AiPurpose::Dictation, dictation),
            Err(SiteGrantsError::NotOffered)
        );
        // Uma loja escrita a mao: so origens canonicas contam.
        write(
            &dir.join("translate-test.json"),
            r#"{"version":1,"data":{"sites":[{"brain":"byom-9","origin":"https://EXEMPLO.com/x"},{"brain":"byom-9","origin":"javascript:alert(1)"},{"brain":"","origin":"https://exemplo.com"}]}}"#,
        );
        let mut hand = SiteGrants::open(registry.grant(SITES).expect("grant")).expect("loja");
        assert!(!hand.allows("byom-9", &origin));
        assert!(!hand.allows("", &origin));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A autorizacao em segundo plano: so do cartao dela, so para o que o
    /// utilizador escreveu, no maximo 30 dias e 24 envios por dia.
    #[test]
    fn standing_grants_are_bounded() {
        let mut gate = EgressGate::for_app(None);
        let typed = |privacy| {
            click(
                AiPurpose::Dictation,
                DataClass::UserTyped,
                gemini(),
                privacy,
            )
        };
        let card = gate
            .standing_grant_card(&typed(EgressPrivacy::Normal), 100, 90)
            .expect("cartao");
        assert_eq!((card.max_per_day, card.days), (24, 30));
        assert_eq!(card.title(), "Autorizar Gemini em segundo plano?");
        assert!(
            card.lines()
                .iter()
                .any(|line| line.contains("até 24 vezes por dia, durante 30 dias"))
        );
        assert!(card.card().needs_consent() && !card.card().offers_always());
        let low = gate
            .standing_grant_card(&typed(EgressPrivacy::Normal), 0, 0)
            .expect("cartao");
        assert_eq!((low.max_per_day, low.days), (1, 1));
        for (request, reason) in [
            (
                typed(EgressPrivacy::PrivateSurface),
                RefuseReason::PrivateMode,
            ),
            (typed(EgressPrivacy::PrivateMode), RefuseReason::PrivateMode),
            (
                click(
                    AiPurpose::Dictation,
                    DataClass::PageContent,
                    gemini(),
                    EgressPrivacy::Normal,
                ),
                RefuseReason::BackgroundData(DataClass::PageContent),
            ),
            (
                click(
                    AiPurpose::Dictation,
                    DataClass::Memory,
                    gemini(),
                    EgressPrivacy::Normal,
                ),
                RefuseReason::NeverLeavesThePc(DataClass::Memory),
            ),
        ] {
            assert_eq!(
                gate.standing_grant_card(&request, 3, 30).err(),
                Some(reason)
            );
        }
        assert!(StandingGrant::issue(&card, ConsentAnswer::Cancel, WATCH, TODAY).is_none());
        assert!(StandingGrant::issue(&card, ConsentAnswer::Once, "com espaco", TODAY).is_none());
        assert!(StandingGrant::issue(&card, ConsentAnswer::Once, "", TODAY).is_none());
        let mut grant =
            StandingGrant::issue(&card, ConsentAnswer::Once, WATCH, TODAY).expect("sim");
        assert_eq!(grant.watch_id(), WATCH);
        assert_eq!(grant.expires(), TODAY.plus(30));
        let brain = Brain::Cloud(Provider::Gemini);
        assert_eq!(grant.admits(WATCH, brain, TODAY.plus(29)), Ok(()));
        assert_eq!(
            grant.admits(WATCH, brain, TODAY.plus(30)),
            Err(RefuseReason::GrantExpired)
        );
        // Guardada e relida (o Radar guarda-a numa loja Setting).
        let saved = serde_json::to_string(&grant).expect("gravar");
        let back: StandingGrant = serde_json::from_str(&saved).expect("ler");
        assert_eq!(back, grant);
        grant.revoke();
        assert_eq!(
            grant.admits(WATCH, brain, TODAY),
            Err(RefuseReason::GrantRevoked)
        );
    }

    /// O cartao diz para onde vai, quanto, que modelo e de que site; as
    /// razoes de recusa dizem que nada foi enviado.
    #[test]
    fn the_card_shows_hosts_tokens_model_and_price_tier() {
        let card = ask(decide(
            &click(
                AiPurpose::Translation,
                DataClass::PageContent,
                gemini(),
                EgressPrivacy::Normal,
            ),
            &facts(false, 201, 200, false),
        ));
        assert_eq!(card.hosts(), ["generativelanguage.googleapis.com"]);
        assert_eq!(card.token_estimate(), 3_200);
        assert_eq!(card.model(), Some("gemini-9.9-flash-lite"));
        assert_eq!(card.price_tier(), Some(PriceTier::Low));
        assert_eq!(card.title(), "Enviar o texto desta página para Gemini?");
        assert_eq!(
            card.lines(),
            vec![
                "Vai para generativelanguage.googleapis.com (Internet)".to_string(),
                "≈ 3.200 tokens".to_string(),
                "Modelo: gemini-9.9-flash-lite · custo baixo".to_string(),
                "Site: exemplo.com".to_string(),
                "Limite mensal atingido (200 chamadas) — continuar?".to_string(),
            ]
        );
        assert_eq!(card.answer_label(ConsentAnswer::Once), "Enviar desta vez");
        assert_eq!(
            card.answer_label(ConsentAnswer::Session),
            "Sempre nesta sessão"
        );
        assert_eq!(card.answer_label(ConsentAnswer::Cancel), "Cancelar");
        let origin = SiteOrigin::of_url("https://Exemplo.com:8443/x").expect("origem");
        assert_eq!(
            (origin.as_str(), origin.host()),
            ("https://exemplo.com:8443", "exemplo.com:8443")
        );
        assert_eq!(SiteOrigin::of_url("file:///C:/x.html"), None);
        assert_eq!(SiteOrigin::of_url("data:text/html,x"), None);
        for reason in [
            RefuseReason::PrivateMode,
            RefuseReason::NeverLeavesThePc(DataClass::Memory),
            RefuseReason::BackgroundData(DataClass::PageContent),
            RefuseReason::Cancelled,
        ] {
            assert!(reason.message().contains("Nada foi enviado"), "{reason:?}");
        }
        assert_eq!(
            RefuseReason::OverCap {
                used: 1500,
                cap: 1000
            }
            .message(),
            "Limite mensal atingido (1.000 chamadas): nada foi enviado em segundo plano."
        );
        for reason in [
            RefuseReason::BackgroundWithoutGrant,
            RefuseReason::GrantForAnotherWatch,
            RefuseReason::GrantForAnotherBrain,
            RefuseReason::GrantRevoked,
            RefuseReason::GrantExpired,
            RefuseReason::GrantDailyLimit,
        ] {
            assert!(!reason.message().is_empty());
        }
        assert_eq!(
            [0, 999, 1_000, 9_800, 1_234_567].map(thousands),
            ["0", "999", "1.000", "9.800", "1.234.567"]
        );
        assert_eq!(Brain::Byom(3).key(), "byom-3");
        assert_eq!(Brain::OnDevice.label(), "este PC");
    }
}
