//! O cliente unico das chamadas de IA (infra-llm-transport, plano 2.3; parte
//! 1 de 2 do antigo `infra-llm-core`). Todo o pedido a uma API de modelos
//! passa por aqui: os revisores recusam um segundo cliente HTTP para IA.
//!
//! - `transport`: o `ApiClient` sobre o ureq 3.4.2 -- sem proxy, sem
//!   redirects, verificador de certificados do sistema, prazo por chamada,
//!   tecto do corpo DESCODIFICADO e desistencia cooperativa. So fala com os
//!   hosts fixados (`Endpoint::pinned`), resolvidos so para enderecos
//!   publicos (`security::Locality::Public`, o `PublicResolver` do Reader);
//!   o loopback so existe nos testes.
//! - `errors`: `ApiError`, com a mensagem pt-BR que o utilizador ve. Nunca
//!   leva a chave nem o corpo da resposta.
//! - `models`: listar, filtrar e escolher o modelo por finalidade (forma
//!   permitida, termos negados, fixacao do dono), com a faixa de preco para
//!   o cartao de consentimento. Nenhum identificador de modelo e escrito aqui.
//! - `gemini`: `models.list` com paginas e `generateContent`, como
//!   construtores puros (sem chave) e leitores das respostas.
//!
//! A chave vem do cofre do app (`ApiKey` em `neural-app/src/secrets.rs`,
//! que implementa `ApiCredential`) e so viaja no cabecalho de autenticacao.
//! O texto de uma pagina so entra num pedido pelo `untrusted::PromptBuilder`
//! (parte 2, `infra-llm-untrusted`): instrucoes no `system`, o pedido do
//! utilizador e os dados cercados no `user`. A Traducao (`crate::translate`)
//! e a primeira feature do produto a chama-lo.

pub mod errors;
pub mod gemini;
pub mod models;
pub mod transport;

#[cfg(test)]
mod tests;

pub use errors::ApiError;
pub use models::{
    DENY_TOKENS, ModelId, ModelInfo, Pick, PickRule, PickSource, PriceTier, Purpose, deny_hits,
    filter_generation_models, pick,
};
pub use transport::{
    ApiClient, ApiCredential, ApiRequest, DEFAULT_MAX_BODY_BYTES, Endpoint, GENERATION_TIMEOUT,
    LIST_TIMEOUT, Method, Provider,
};
