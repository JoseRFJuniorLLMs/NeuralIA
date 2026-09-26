//! Que modelo usar para cada finalidade. Nenhum identificador de modelo e
//! escrito neste modulo (gate `no_model_id_is_hard_coded`): os modelos vem da
//! listagem da API em tempo de execucao e passam por tres passos.
//!
//! 1. listar (`gemini::list_models`);
//! 2. filtrar: so os que geram texto (`generateContent`) e cujo id e seguro
//!    para um caminho de URL (`ModelId`);
//! 3. escolher por finalidade com uma REGRA (`PickRule`): a forma permitida
//!    do fornecedor (familia, versao numerica, termos obrigatorios), os
//!    termos negados (`DENY_TOKENS`) e so versoes estaveis; ganha a versao
//!    mais nova. A fixacao do dono (IA > Cerebros) passa a frente da regra
//!    quando o modelo fixado esta na listagem.
//!
//! O modelo escolhido e a sua faixa de preco (`PriceTier`) vao para o cartao
//! de consentimento (`Pick::consent_line`). A constante do Gemini Live
//! (`neural-app`) nao passa por aqui e nao e tocada.

use super::transport::Provider;

/// Termos que tiram um modelo da escolha automatica, comparados por
/// segmentos do id (separados por `-`): `pro` nega `...-pro` mas nao
/// `...-preview`. Tempo real, audio, imagem, voz, transcricao, pesquisa e
/// codigo sao outras ferramentas; `pro` e o custo alto; `nano` e `oss` a
/// qualidade; `chat-latest` e um alias que muda; `live` e `native-audio` sao
/// da voz em tempo real; `embedding` nao gera texto.
pub const DENY_TOKENS: &[&str] = &[
    "realtime",
    "audio",
    "image",
    "tts",
    "transcribe",
    "search",
    "codex",
    "pro",
    "nano",
    "oss",
    "chat-latest",
    "live",
    "native-audio",
    "embedding",
];

/// Segmentos que marcam uma versao que nao e estavel.
const UNSTABLE_SEGMENTS: &[&str] = &["preview", "exp", "experimental", "beta", "alpha"];

const MAX_MODEL_ID_BYTES: usize = 128;
const MAX_DISPLAY_NAME_CHARS: usize = 120;

/// O id de um modelo, validado para ir num caminho de URL: 1 a 128 de
/// `[a-z0-9._-]`, a comecar por letra ou digito, sem `..`. Vem de uma
/// resposta da API (texto nao confiavel) ou da fixacao do dono.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(String);

impl ModelId {
    pub fn parse(raw: &str) -> Option<Self> {
        let bytes = raw.as_bytes();
        let valid = (1..=MAX_MODEL_ID_BYTES).contains(&bytes.len())
            && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
            && bytes.iter().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'_')
            })
            && !raw.contains("..");
        valid.then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn segments(&self) -> Vec<&str> {
        self.0.split('-').collect()
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Um modelo da listagem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: ModelId,
    /// Texto da API, cortado a 120 caracteres; so para mostrar.
    pub display_name: String,
    /// Aceita `generateContent`.
    pub generates_text: bool,
    pub input_token_limit: Option<u64>,
    pub output_token_limit: Option<u64>,
}

impl ModelInfo {
    pub(crate) fn new(
        id: ModelId,
        display_name: &str,
        generates_text: bool,
        input_token_limit: Option<u64>,
        output_token_limit: Option<u64>,
    ) -> Self {
        Self {
            id,
            display_name: display_name
                .chars()
                .filter(|character| !character.is_control())
                .take(MAX_DISPLAY_NAME_CHARS)
                .collect(),
            generates_text,
            input_token_limit,
            output_token_limit,
        }
    }
}

/// Passo 2: so os modelos que geram texto.
pub fn filter_generation_models(models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    models
        .into_iter()
        .filter(|model| model.generates_text)
        .collect()
}

/// Os termos negados que um id contem, na ordem de `DENY_TOKENS`.
pub fn deny_hits(id: &ModelId) -> Vec<&'static str> {
    let segments = id.segments();
    DENY_TOKENS
        .iter()
        .copied()
        .filter(|token| {
            let wanted: Vec<&str> = token.split('-').collect();
            segments
                .windows(wanted.len())
                .any(|window| window == wanted.as_slice())
        })
        .collect()
}

/// A finalidade de uma chamada. Os proximos itens acrescentam as deles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    Translation,
}

impl Purpose {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Translation => "Tradução",
        }
    }
}

/// A faixa de preco de um modelo, pelos termos do id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceTier {
    Low,
    Medium,
    High,
    Unknown,
}

impl PriceTier {
    pub fn of(id: &ModelId) -> Self {
        let segments = id.segments();
        let has = |tokens: &[&str]| segments.iter().any(|segment| tokens.contains(segment));
        if has(&["lite", "nano", "mini", "haiku", "small"]) {
            Self::Low
        } else if has(&["pro", "opus", "ultra"]) {
            Self::High
        } else if has(&["flash", "sonnet", "medium"]) {
            Self::Medium
        } else {
            Self::Unknown
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "custo baixo",
            Self::Medium => "custo médio",
            Self::High => "custo alto",
            Self::Unknown => "custo desconhecido",
        }
    }
}

/// A regra de escolha de uma finalidade num fornecedor. A forma permitida:
/// o primeiro segmento e `family`, o segundo uma versao numerica
/// (`2`, `2.5`), logo a seguir os `required`; o resto e livre, salvo termos
/// negados e marcas de versao instavel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickRule {
    pub purpose: Purpose,
    pub provider: Provider,
    pub family: &'static str,
    pub required: &'static [&'static str],
}

/// As regras. Traducao: o `*-flash-lite` estavel mais novo da Gemini.
pub const PICK_RULES: &[PickRule] = &[PickRule {
    purpose: Purpose::Translation,
    provider: Provider::Gemini,
    family: "gemini",
    required: &["flash", "lite"],
}];

/// A ordem de uma candidata: versao mais nova, depois menos segmentos a mais
/// (o alias canonico antes da versao fixada), depois o numero fixado mais
/// alto (`002` antes de `001`), e por fim o id, para ser determinista.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    version: Vec<u32>,
    fewer_extra: std::cmp::Reverse<usize>,
    pin: u32,
    id: std::cmp::Reverse<String>,
}

impl PickRule {
    pub fn for_purpose(purpose: Purpose, provider: Provider) -> Option<&'static Self> {
        PICK_RULES
            .iter()
            .find(|rule| rule.purpose == purpose && rule.provider == provider)
    }

    /// A forma e a estabilidade, sem os termos negados.
    fn shape_rank(&self, id: &ModelId) -> Option<Rank> {
        let segments = id.segments();
        let start = 2 + self.required.len();
        if segments.len() < start
            || segments[0] != self.family
            || segments[2..start] != *self.required
        {
            return None;
        }
        let version = parse_version(segments[1])?;
        let extra = &segments[start..];
        if extra
            .iter()
            .any(|segment| UNSTABLE_SEGMENTS.contains(segment))
        {
            return None;
        }
        let pin = match extra {
            [only] if only.len() == 3 && only.bytes().all(|byte| byte.is_ascii_digit()) => {
                only.parse().unwrap_or(0)
            }
            _ => 0,
        };
        Some(Rank {
            version,
            fewer_extra: std::cmp::Reverse(extra.len()),
            pin,
            id: std::cmp::Reverse(id.as_str().to_string()),
        })
    }

    /// Se a regra aceita o id (forma, estabilidade e nenhum termo negado).
    pub fn accepts(&self, id: &ModelId) -> bool {
        self.rank(id).is_some()
    }

    fn rank(&self, id: &ModelId) -> Option<Rank> {
        if !deny_hits(id).is_empty() {
            return None;
        }
        self.shape_rank(id)
    }

    /// A forma e a estabilidade aceitam o id quando se ignoram os termos
    /// negados: e o que prova, na tabela, que cada termo negado tira da
    /// escolha um modelo que de outro modo ganhava.
    #[cfg(test)]
    pub(crate) fn accepts_ignoring_deny(&self, id: &ModelId) -> bool {
        self.shape_rank(id).is_some()
    }

    #[cfg(test)]
    pub(crate) fn newer(&self, left: &ModelId, right: &ModelId) -> bool {
        match (self.shape_rank(left), self.shape_rank(right)) {
            (Some(left), Some(right)) => left.version > right.version,
            _ => false,
        }
    }
}

fn parse_version(segment: &str) -> Option<Vec<u32>> {
    segment
        .split('.')
        .map(|part| {
            (!part.is_empty() && part.len() <= 4 && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse::<u32>().ok())
                .flatten()
        })
        .collect()
}

/// De onde veio a escolha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickSource {
    /// O dono fixou este modelo e ele esta na listagem.
    OwnerPin,
    /// A regra da finalidade.
    Rule,
    /// A regra, porque o modelo fixado pelo dono nao esta na listagem.
    RuleAfterMissingPin,
}

/// O modelo escolhido, para a chamada e para o cartao de consentimento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub purpose: Purpose,
    pub provider: Provider,
    pub model: ModelId,
    pub price_tier: PriceTier,
    pub source: PickSource,
}

impl Pick {
    /// A linha do cartao de consentimento: finalidade, fornecedor, modelo e
    /// faixa de preco.
    pub fn consent_line(&self) -> String {
        format!(
            "{}: {} {} ({})",
            self.purpose.label(),
            self.provider.label(),
            self.model,
            self.price_tier.label()
        )
    }
}

/// Passo 3: escolhe o modelo de uma finalidade entre os listados. A fixacao
/// do dono ganha quando o modelo fixado esta na listagem e gera texto (a
/// escolha e explicita e o cartao mostra a faixa de preco); senao aplica-se
/// a regra. `None`: nenhum modelo listado serve.
pub fn pick(
    purpose: Purpose,
    provider: Provider,
    models: &[ModelInfo],
    owner_pin: Option<&ModelId>,
) -> Option<Pick> {
    let usable = || models.iter().filter(|model| model.generates_text);
    let chosen = |model: &ModelId, source| Pick {
        purpose,
        provider,
        model: model.clone(),
        price_tier: PriceTier::of(model),
        source,
    };
    if let Some(pin) = owner_pin
        && let Some(model) = usable().find(|model| &model.id == pin)
    {
        return Some(chosen(&model.id, PickSource::OwnerPin));
    }
    let rule = PickRule::for_purpose(purpose, provider)?;
    let best = usable()
        .filter_map(|model| rule.rank(&model.id).map(|rank| (rank, &model.id)))
        .max_by(|left, right| left.0.cmp(&right.0))?;
    let source = if owner_pin.is_some() {
        PickSource::RuleAfterMissingPin
    } else {
        PickSource::Rule
    };
    Some(chosen(best.1, source))
}
