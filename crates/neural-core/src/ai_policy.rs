//! A politica de base da IA (infra-egress, plano 2.3): que dado pode ir para
//! onde, e que motor faz uma tarefa. Pura, sem rede, sem disco e sem relogio.
//!
//! - `DataClass`: o que se quer mandar (o que o utilizador escreveu, o texto
//!   de uma pagina, respostas das IAs, a memoria local, audio ou imagem).
//! - `Locality`: para onde vai. `Local` e dentro do processo (o pacote
//!   semantico, os motores deterministas); `Loopback` e um servidor neste PC
//!   (127.0.0.1, `::1`); `Lan` e a rede local; `Remote` e a Internet.
//! - `may_send`: a regra que nao depende do gatilho nem do consentimento --
//!   nada sai do PC sem consentimento, e a memoria local nunca vai para a
//!   Internet. O gatilho (clique ou segundo plano), o modo privado, o
//!   consentimento e o limite mensal sao do `EgressGate` do neural-app
//!   (`crates/neural-app/src/egress.rs`), que parte desta regra.
//! - `choose_engine`: o esboco da escolha do motor de uma tarefa, que o
//!   local-ai-tasks completa. Ja cumpre as regras que nunca mudam: nada
//!   Remote/Lan no modo privado, a memoria nunca na Internet, o texto de uma
//!   pagina para fora do PC so com o consentimento do perfil.

/// O que se quer mandar a um modelo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DataClass {
    /// O que o utilizador escreveu ou selecionou para mandar (uma pergunta,
    /// um tema do Radar, um termo do Explicar).
    UserTyped,
    /// O texto de uma pagina, de um PDF ou de um livro.
    PageContent,
    /// As respostas das IAs das colunas (o juiz, a sintese).
    ProviderAnswers,
    /// A memoria semantica local.
    Memory,
    /// Audio (ditado) ou imagem.
    Media,
}

impl DataClass {
    pub const ALL: [Self; 5] = [
        Self::UserTyped,
        Self::PageContent,
        Self::ProviderAnswers,
        Self::Memory,
        Self::Media,
    ];

    /// Como o cartao de consentimento o diz.
    pub const fn label(self) -> &'static str {
        match self {
            Self::UserTyped => "o texto que você escreveu",
            Self::PageContent => "o texto desta página",
            Self::ProviderAnswers => "as respostas das IAs",
            Self::Memory => "a sua memória local",
            Self::Media => "áudio ou imagem",
        }
    }
}

/// Para onde vai o dado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Locality {
    /// Dentro do processo: o pacote semantico, os motores deterministas.
    Local,
    /// Um servidor neste PC (127.0.0.0/8, `::1`, `localhost`).
    Loopback,
    /// A rede local (RFC 1918, ULA).
    Lan,
    /// A Internet.
    Remote,
}

impl Locality {
    pub const ALL: [Self; 4] = [Self::Local, Self::Loopback, Self::Lan, Self::Remote];

    /// Se o dado sai deste PC.
    pub const fn leaves_the_pc(self) -> bool {
        matches!(self, Self::Lan | Self::Remote)
    }

    /// O selo que o utilizador ve.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local | Self::Loopback => "Este PC",
            Self::Lan => "Rede local",
            Self::Remote => "Internet",
        }
    }
}

/// O que `may_send` responde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaySend {
    /// Nada sai do PC: vai sem perguntar.
    Yes,
    /// Sai do PC: so com o consentimento do utilizador.
    WithConsent,
    /// Nunca, com consentimento ou sem ele.
    Never,
}

/// A regra de base: o que fica no PC vai sem perguntar; o que sai precisa
/// de consentimento; a memoria local nunca vai para a Internet.
pub const fn may_send(data: DataClass, to: Locality) -> MaySend {
    match (data, to) {
        (_, Locality::Local | Locality::Loopback) => MaySend::Yes,
        (DataClass::Memory, Locality::Remote) => MaySend::Never,
        (_, Locality::Lan | Locality::Remote) => MaySend::WithConsent,
    }
}

/// Uma estimativa grosseira de tokens para o cartao: um token por cada
/// quatro caracteres, arredondado para cima.
pub fn estimate_tokens(chars: usize) -> u32 {
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

/// Uma tarefa de IA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTask {
    Summarize,
    Synthesize,
    CompareAssist,
    Judge,
    Embed,
    Chat,
}

/// Um modelo configurado (BYOM ou nuvem) que a escolha pode usar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProfile {
    pub id: u32,
    pub locality: Locality,
    /// O dono deixou mandar o texto de paginas a este perfil.
    pub page_consent: bool,
    /// Gera texto.
    pub chats: bool,
    /// Faz embeddings.
    pub embeds: bool,
    /// O modelo de conversa por omissao.
    pub default_chat: bool,
}

/// O que ha para escolher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiInventory {
    pub profiles: Vec<EngineProfile>,
    /// O pacote semantico local esta instalado.
    pub pack: bool,
}

/// O motor escolhido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// O perfil com este id.
    Llm(u32),
    /// O pacote semantico local.
    Pack,
    /// Os motores deterministas (hashing, extrativo).
    Deterministic,
}

/// Esboco da escolha do motor (o local-ai-tasks completa as preferencias).
/// Um perfil so e candidato se a regra de base o deixa receber `data`, se o
/// modo privado nao o tira (so `Local`/`Loopback` no privado) e, para o
/// texto de uma pagina fora do PC, se o dono lhe deu esse consentimento.
/// Embeddings: o pacote, depois um perfil deste PC que os faca, depois o
/// determinista. Gerar: o perfil deste PC por omissao, outro deste PC, um
/// de fora com consentimento (o por omissao primeiro), e por fim o
/// determinista.
pub fn choose_engine(
    task: AiTask,
    data: DataClass,
    inventory: &AiInventory,
    private: bool,
) -> Engine {
    let eligible = |profile: &EngineProfile| {
        let leaves = profile.locality.leaves_the_pc();
        may_send(data, profile.locality) != MaySend::Never
            && !(private && leaves)
            && !(leaves && data == DataClass::PageContent && !profile.page_consent)
    };
    let candidates: Vec<&EngineProfile> = inventory
        .profiles
        .iter()
        .filter(|profile| eligible(profile))
        .collect();
    if task == AiTask::Embed {
        if inventory.pack {
            return Engine::Pack;
        }
        return candidates
            .iter()
            .find(|profile| profile.embeds && !profile.locality.leaves_the_pc())
            .map_or(Engine::Deterministic, |profile| Engine::Llm(profile.id));
    }
    // Entre iguais, o primeiro da lista (`max_by_key` da o ultimo).
    let best = |outside: bool| {
        candidates
            .iter()
            .rev()
            .filter(|profile| profile.chats && profile.locality.leaves_the_pc() == outside)
            .max_by_key(|profile| profile.default_chat)
    };
    best(false)
        .or_else(|| best(true))
        .map_or(Engine::Deterministic, |profile| Engine::Llm(profile.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn may_send_table() {
        for data in DataClass::ALL {
            for to in Locality::ALL {
                let expected = if !to.leaves_the_pc() {
                    MaySend::Yes
                } else if data == DataClass::Memory && to == Locality::Remote {
                    MaySend::Never
                } else {
                    MaySend::WithConsent
                };
                assert_eq!(may_send(data, to), expected, "{data:?} -> {to:?}");
            }
        }
        // As duas linhas que nunca mudam, ditas a mao.
        assert_eq!(
            may_send(DataClass::Memory, Locality::Remote),
            MaySend::Never
        );
        assert_eq!(
            may_send(DataClass::PageContent, Locality::Remote),
            MaySend::WithConsent
        );
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(9), 3);
        assert_eq!(estimate_tokens(12_800), 3_200);
    }

    fn profile(id: u32, locality: Locality) -> EngineProfile {
        EngineProfile {
            id,
            locality,
            page_consent: false,
            chats: true,
            embeds: false,
            default_chat: false,
        }
    }

    #[test]
    fn choose_engine_keeps_the_rules_that_never_change() {
        let loopback = profile(1, Locality::Loopback);
        let remote = profile(2, Locality::Remote);
        let lan = profile(3, Locality::Lan);
        let inventory = AiInventory {
            profiles: vec![remote.clone(), lan],
            pack: false,
        };
        // Nada de fora no modo privado.
        for data in DataClass::ALL {
            assert_eq!(
                choose_engine(AiTask::Chat, data, &inventory, true),
                Engine::Deterministic,
                "{data:?}"
            );
        }
        // A memoria nunca vai para a Internet (a rede local ainda serve).
        let only_remote = AiInventory {
            profiles: vec![remote.clone()],
            pack: false,
        };
        assert_eq!(
            choose_engine(AiTask::Chat, DataClass::Memory, &only_remote, false),
            Engine::Deterministic
        );
        assert_eq!(
            choose_engine(AiTask::Chat, DataClass::UserTyped, &only_remote, false),
            Engine::Llm(2)
        );
        // O texto de uma pagina so sai com o consentimento do perfil.
        assert_eq!(
            choose_engine(
                AiTask::Summarize,
                DataClass::PageContent,
                &only_remote,
                false
            ),
            Engine::Deterministic
        );
        let consented = AiInventory {
            profiles: vec![EngineProfile {
                page_consent: true,
                ..remote.clone()
            }],
            pack: false,
        };
        assert_eq!(
            choose_engine(AiTask::Summarize, DataClass::PageContent, &consented, false),
            Engine::Llm(2)
        );
        // Deste PC antes de fora; o por omissao antes dos outros.
        let mixed = AiInventory {
            profiles: vec![
                remote,
                loopback.clone(),
                EngineProfile {
                    default_chat: true,
                    ..profile(4, Locality::Loopback)
                },
            ],
            pack: false,
        };
        assert_eq!(
            choose_engine(AiTask::Chat, DataClass::PageContent, &mixed, true),
            Engine::Llm(4)
        );
        // Embeddings: o pacote, depois um perfil deste PC, depois hashing.
        let embeds = AiInventory {
            profiles: vec![EngineProfile {
                embeds: true,
                ..loopback
            }],
            pack: false,
        };
        assert_eq!(
            choose_engine(AiTask::Embed, DataClass::Memory, &embeds, false),
            Engine::Llm(1)
        );
        let with_pack = AiInventory {
            pack: true,
            ..embeds
        };
        assert_eq!(
            choose_engine(AiTask::Embed, DataClass::Memory, &with_pack, false),
            Engine::Pack
        );
        assert_eq!(
            choose_engine(
                AiTask::Embed,
                DataClass::Memory,
                &AiInventory::default(),
                false
            ),
            Engine::Deterministic
        );
    }
}
