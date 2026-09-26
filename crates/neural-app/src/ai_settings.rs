//! As definicoes e o consumo da IA (infra-egress, plano 2.3), minimos: o
//! esqueleto que o consensus-judge, o copilot-brain e o byom-backends
//! alargam.
//!
//! - `<data_dir>/ai/settings.json` (`stores::AI_SETTINGS_STORE`,
//!   `StoreKind::Setting`: so muda por uma escolha em IA › Cérebros ou
//!   Consumo): as finalidades (`AiPurpose`: Tradução, Ditado), cada uma com o
//!   cerebro e o modelo que o dono fixou, e o limite mensal suave de
//!   chamadas pagas (200 por omissao). O limite vive aqui, e nao no
//!   `usage.json`, porque um ficheiro tem um tipo so: quem o muda e o dono.
//! - `<data_dir>/ai/usage.json` (`stores::AI_USAGE_STORE`,
//!   `StoreKind::Automatic`: escreve-se como efeito lateral de cada chamada
//!   paga): os contadores do mes por cerebro e finalidade (`UsageBook`),
//!   partilhados entre janelas. Quem conta e o `EgressGate` (`egress.rs`).
//!
//! O mes e o do calendario UTC (`Day`): o contador vira a meia-noite UTC do
//! dia 1, nao a local. Portatil: testado tambem no runner Linux.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A versao que este codigo escreve nos dois ficheiros.
pub(crate) const AI_SETTINGS_VERSION: u32 = 1;
pub(crate) const AI_USAGE_VERSION: u32 = 1;
/// Os tectos dos dois ficheiros, conferidos antes de ler.
pub(crate) const AI_SETTINGS_MAX_BYTES: u64 = 64 * 1024;
pub(crate) const AI_USAGE_MAX_BYTES: u64 = 64 * 1024;

/// O limite mensal suave por omissao: chamadas pagas de todas as
/// finalidades somadas.
pub(crate) const DEFAULT_MONTHLY_SOFT_CAP: u32 = 200;
/// O maior limite que o ficheiro pode pedir.
pub(crate) const MAX_MONTHLY_SOFT_CAP: u32 = 100_000;
/// Quantos meses o `usage.json` guarda (o atual e os onze anteriores).
pub(crate) const USAGE_MONTHS_KEPT: usize = 12;

/// Uma finalidade de IA: o que o utilizador escolhe em IA › Cérebros e o
/// que o consumo conta. Os itens seguintes acrescentam as suas (Juiz do
/// Consenso, Copiloto, Pesquisa profunda, Radar, Escudo, Organizar abas,
/// Resumo/Síntese, Explicar, Extrair tabela).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum AiPurpose {
    Translation,
    Dictation,
}

impl AiPurpose {
    pub(crate) const ALL: [Self; 2] = [Self::Translation, Self::Dictation];

    /// A chave nos ficheiros (estavel: nunca muda de nome).
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::Translation => "traducao",
            Self::Dictation => "ditado",
        }
    }

    /// O nome que o utilizador ve.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Translation => "Tradução",
            Self::Dictation => "Ditado",
        }
    }
}

/// O que o dono escolheu para uma finalidade. Vazio = o de omissao.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PurposeSettings {
    /// O cerebro (`gemini`, `byom-2`...), pela chave de `egress::Brain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) brain: Option<String>,
    /// O modelo fixado em IA › Cérebros; sem ele vale a regra de escolha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model_pin: Option<String>,
}

/// `ai/settings.json`. Chaves de finalidade que este codigo nao conhece
/// (de um NeuralIA mais novo com a mesma versao) passam intactas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AiSettings {
    #[serde(default = "default_purposes")]
    pub(crate) purposes: BTreeMap<String, PurposeSettings>,
    #[serde(default = "default_monthly_soft_cap")]
    pub(crate) monthly_soft_cap: u32,
}

fn default_purposes() -> BTreeMap<String, PurposeSettings> {
    AiPurpose::ALL
        .iter()
        .map(|purpose| (purpose.key().to_string(), PurposeSettings::default()))
        .collect()
}

fn default_monthly_soft_cap() -> u32 {
    DEFAULT_MONTHLY_SOFT_CAP
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            purposes: default_purposes(),
            monthly_soft_cap: DEFAULT_MONTHLY_SOFT_CAP,
        }
    }
}

impl AiSettings {
    /// O limite mensal, dentro de `0..=MAX_MONTHLY_SOFT_CAP` (0 = perguntar
    /// em cada chamada paga).
    pub(crate) fn soft_cap(&self) -> u32 {
        self.monthly_soft_cap.min(MAX_MONTHLY_SOFT_CAP)
    }

    /// O que o dono escolheu para `purpose` (o de omissao se nada).
    pub(crate) fn purpose(&self, purpose: AiPurpose) -> PurposeSettings {
        self.purposes
            .get(purpose.key())
            .cloned()
            .unwrap_or_default()
    }
}

/// Um dia do calendario UTC, em dias desde 1970-01-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Day(pub(crate) u32);

impl Day {
    /// Hoje, pelo relogio do sistema (UTC).
    pub(crate) fn today() -> Self {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        Self(u32::try_from(secs / 86_400).unwrap_or(u32::MAX))
    }

    pub(crate) fn plus(self, days: u32) -> Self {
        Self(self.0.saturating_add(days))
    }

    /// `AAAA-MM` do mes deste dia.
    pub(crate) fn month_key(self) -> String {
        let (year, month, _) = civil_from_days(i64::from(self.0));
        format!("{year:04}-{month:02}")
    }
}

/// Dias desde 1970-01-01 para (ano, mes, dia), calendario gregoriano
/// proleptico (Howard Hinnant; o mesmo do `zettel.rs`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = (if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    }) as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Uma chamada (ou varias) contada num mes, para um cerebro e finalidade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageDelta {
    pub(crate) month: String,
    pub(crate) brain: String,
    pub(crate) purpose: String,
    pub(crate) calls: u32,
}

/// `ai/usage.json`: mes (`AAAA-MM`) -> cerebro -> finalidade -> chamadas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageBook {
    #[serde(default)]
    pub(crate) months: BTreeMap<String, BTreeMap<String, BTreeMap<String, u32>>>,
}

impl UsageBook {
    /// Soma a chamada e guarda no maximo `USAGE_MONTHS_KEPT` meses.
    pub(crate) fn add(&mut self, delta: &UsageDelta) {
        let count = self
            .months
            .entry(delta.month.clone())
            .or_default()
            .entry(delta.brain.clone())
            .or_default()
            .entry(delta.purpose.clone())
            .or_default();
        *count = count.saturating_add(delta.calls);
        // Sai o mes mais velho, nunca o que acabou de contar: um ficheiro com
        // meses "do futuro" (relogio adiantado) nao apaga o mes corrente e
        // nao desliga o limite.
        while self.months.len() > USAGE_MONTHS_KEPT {
            let Some(oldest) = self
                .months
                .keys()
                .find(|month| **month != delta.month)
                .cloned()
            else {
                break;
            };
            self.months.remove(&oldest);
        }
    }

    /// Todas as chamadas pagas do mes, de todos os cerebros e finalidades.
    pub(crate) fn month_total(&self, month: &str) -> u32 {
        self.months.get(month).map_or(0, |brains| {
            brains
                .values()
                .flat_map(|purposes| purposes.values())
                .fold(0u32, |total, calls| total.saturating_add(*calls))
        })
    }

    /// As chamadas de um cerebro numa finalidade, num mes.
    pub(crate) fn calls(&self, month: &str, brain: &str, purpose: &str) -> u32 {
        self.months
            .get(month)
            .and_then(|brains| brains.get(brain))
            .and_then(|purposes| purposes.get(purpose))
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_skeleton_and_round_trip() {
        let defaults = AiSettings::default();
        assert_eq!(defaults.soft_cap(), 200);
        let keys: Vec<&str> = defaults.purposes.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["ditado", "traducao"]);
        assert_eq!(AiPurpose::ALL.map(AiPurpose::label), ["Tradução", "Ditado"]);
        // Um ficheiro vazio de campos vale o esqueleto; o que nao se conhece
        // passa intacto.
        let empty: AiSettings = serde_json::from_str("{}").expect("vazio");
        assert_eq!(empty, defaults);
        let newer: AiSettings = serde_json::from_str(
            r#"{"purposes":{"traducao":{"brain":"gemini","model_pin":"m-1"},"juiz":{"brain":"openai"}},"monthly_soft_cap":999999999}"#,
        )
        .expect("mais novo");
        assert_eq!(newer.soft_cap(), MAX_MONTHLY_SOFT_CAP);
        assert_eq!(
            newer.purpose(AiPurpose::Translation),
            PurposeSettings {
                brain: Some("gemini".into()),
                model_pin: Some("m-1".into())
            }
        );
        assert_eq!(
            newer.purpose(AiPurpose::Dictation),
            PurposeSettings::default()
        );
        let again: AiSettings =
            serde_json::from_str(&serde_json::to_string(&newer).expect("gravar")).expect("ler");
        assert!(again.purposes.contains_key("juiz"));
    }

    #[test]
    fn days_and_months() {
        assert_eq!(Day(0).month_key(), "1970-01");
        // 2026-09-30 e 2026-10-01 (dias 20726 e 20727 desde 1970).
        assert_eq!(Day(20_726).month_key(), "2026-09");
        assert_eq!(Day(20_727).month_key(), "2026-10");
        assert_eq!(Day(20_726).plus(1), Day(20_727));
        assert_eq!(Day(u32::MAX).plus(1), Day(u32::MAX));
        // 2028-02-29 existe.
        assert_eq!(Day(21_243).month_key(), "2028-02");
        assert_eq!(Day(21_244).month_key(), "2028-03");
        assert!(Day::today() > Day(20_000));
    }

    #[test]
    fn usage_book_counts_and_keeps_twelve_months() {
        let mut book = UsageBook::default();
        let delta = |month: &str, brain: &str, purpose: &str, calls| UsageDelta {
            month: month.into(),
            brain: brain.into(),
            purpose: purpose.into(),
            calls,
        };
        book.add(&delta("2026-09", "gemini", "traducao", 3));
        book.add(&delta("2026-09", "gemini", "traducao", 2));
        book.add(&delta("2026-09", "gemini", "ditado", 1));
        book.add(&delta("2026-09", "byom-2", "traducao", u32::MAX));
        assert_eq!(book.calls("2026-09", "gemini", "traducao"), 5);
        assert_eq!(
            book.month_total("2026-09"),
            u32::MAX,
            "satura, nao da a volta"
        );
        assert_eq!(book.month_total("2026-08"), 0);
        // Mais 13 meses (2026-10 a 2027-10): ficam os 12 mais novos.
        for day in (0..13).map(|month| Day(20_727 + month * 31)) {
            book.add(&delta(&day.month_key(), "gemini", "traducao", 1));
        }
        assert_eq!(book.months.len(), USAGE_MONTHS_KEPT);
        assert!(!book.months.contains_key("2026-09"));
        assert!(!book.months.contains_key("2026-10"));
        assert!(book.months.contains_key("2026-11"));
        assert!(book.months.contains_key("2027-10"));
        // Doze meses "do futuro" (relogio adiantado, ficheiro mexido) nao
        // apagam o mes que conta: o limite continua a ver as chamadas.
        let mut skewed = UsageBook::default();
        for month in 1..=12 {
            skewed.add(&delta(&format!("2099-{month:02}"), "gemini", "traducao", 1));
        }
        skewed.add(&delta("2026-09", "gemini", "traducao", 7));
        assert_eq!(skewed.months.len(), USAGE_MONTHS_KEPT);
        assert_eq!(skewed.month_total("2026-09"), 7);
    }
}
