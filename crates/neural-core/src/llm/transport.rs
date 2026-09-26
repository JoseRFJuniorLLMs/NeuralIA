//! O transporte das chamadas de IA: um `ApiClient` sobre o ureq 3.4.2 (ja no
//! Cargo.lock; nenhum crate novo), com a mesma politica do `ReaderClient`:
//!
//! - `proxy(None)`: o ureq 3 le as variaveis de proxy do ambiente por
//!   omissao; uma chave nunca passa por um proxy que alguem pos no ambiente.
//! - `max_redirects(0)`: um 3xx da API nao se segue, vira erro. Seguir levava
//!   o pedido (e o cabecalho com a chave) para onde a resposta mandasse.
//! - `RootCerts::PlatformVerifier`: os certificados do sistema.
//! - `http_status_as_error(false)`: o estado HTTP chega aqui e e classificado
//!   por `errors::classify_status`, sem o corpo nunca sair desta camada.
//! - prazo por chamada (`timeout_global`): 60 s a gerar, 15 s a listar.
//! - tecto do corpo DESCODIFICADO (1 MiB por omissao), com a verificacao
//!   dupla do Reader: o `.limit()` do ureq conta bytes antes do gzip/brotli,
//!   por isso uma bomba de descompressao passava-o; a contagem do laco de
//!   leitura e a que trava o corpo descodificado.
//! - desistencia cooperativa, testada antes do pedido e entre blocos.
//! - `User-Agent: NeuralIA/<versao>`.
//!
//! Os URLs so se montam a partir das constantes de host fixadas
//! (`Endpoint::pinned`). O `Endpoint::loopback` so compila nos testes: os
//! construtores e leitores sao os mesmos, por isso os testes sobre um
//! servidor local cobrem o codigo que embarca. A chave (`ApiCredential`) so
//! entra no cabecalho de autenticacao do fornecedor; os construtores dos
//! pedidos nunca a veem.
//!
//! Cada `Endpoint` tem a sua `Locality` e o agente resolve nomes com o
//! resolvedor dela (infra-llm-untrusted): um host fixado e `Public` e usa o
//! `PublicResolver` do Reader, por isso um DNS que respondesse com o
//! loopback, a rede local ou o link-local nunca leva a chave para la. O
//! loopback dos testes e `Loopback`; o `Lan` fica para os servidores da rede
//! local do byom-backends.

use std::io::Read;
use std::time::{Duration, Instant};

use ureq::{
    Agent, Body,
    config::Config,
    http::Response,
    tls::{RootCerts, TlsConfig},
    unversioned::transport::DefaultConnector,
};

use super::errors::{ApiError, classify_status};
use crate::security::{LanResolver, Locality, LoopbackResolver, PublicResolver};

/// O host da API Gemini (Google AI Studio). Constante: nenhum URL do
/// transporte nasce de configuracao, de uma variavel de ambiente ou de uma
/// resposta.
pub const GEMINI_HOST: &str = "generativelanguage.googleapis.com";

/// O tecto do corpo descodificado de uma resposta com sucesso.
pub const DEFAULT_MAX_BODY_BYTES: usize = 1024 * 1024;

/// O prazo de uma chamada que gera texto.
pub const GENERATION_TIMEOUT: Duration = Duration::from_secs(60);

/// O prazo de cada pagina de uma listagem de modelos.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// Do corpo de uma resposta de erro so se le isto: chega para a
/// classificacao e nunca sai desta camada.
const ERROR_BODY_MAX_BYTES: usize = 64 * 1024;

const READ_CHUNK_BYTES: usize = 16 * 1024;

/// Os fornecedores com host fixado. Os proximos itens do plano acrescentam
/// OpenAI (judge-openai-client) e Anthropic (copilot-brain) a ESTE enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Gemini,
}

impl Provider {
    /// O host fixado do fornecedor.
    pub const fn host(self) -> &'static str {
        match self {
            Self::Gemini => GEMINI_HOST,
        }
    }

    /// O nome que o utilizador ve.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gemini => "Gemini",
        }
    }

    /// O cabecalho que leva a chave. A Gemini aceita a chave em `?key=` no
    /// URL; aqui nunca: o URL acaba em logs, historicos e cabecalhos
    /// `Referer`, o cabecalho nao.
    pub const fn auth_header(self) -> &'static str {
        match self {
            Self::Gemini => "x-goog-api-key",
        }
    }
}

/// A origem dos pedidos de um `ApiClient`. Em release so existe a fixada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    origin: String,
    /// O fornecedor do host fixado; `None` so no loopback dos testes.
    provider: Option<Provider>,
    /// Onde os enderecos resolvidos podem estar.
    locality: Locality,
}

impl Endpoint {
    /// O unico construtor que embarca: o host constante do fornecedor, por
    /// HTTPS, resolvido so para enderecos publicos.
    pub fn pinned(provider: Provider) -> Self {
        Self {
            origin: format!("https://{}", provider.host()),
            provider: Some(provider),
            locality: Locality::Public,
        }
    }

    #[cfg(test)]
    pub(crate) fn loopback(port: u16) -> Self {
        Self {
            origin: format!("http://127.0.0.1:{port}"),
            provider: None,
            locality: Locality::Loopback,
        }
    }

    /// So nos testes: a mesma origem com o resolvedor de outra localidade,
    /// para provar que o `Public` de um host fixado nao liga ao stub local.
    #[cfg(test)]
    pub(crate) fn resolved_as(mut self, locality: Locality) -> Self {
        self.locality = locality;
        self
    }

    /// A localidade dos enderecos que este endpoint aceita.
    pub fn locality(&self) -> Locality {
        self.locality
    }

    /// O URL de um pedido: a origem mais o caminho que um construtor montou.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    fn serves(&self, provider: Provider) -> bool {
        self.provider.is_none_or(|pinned| pinned == provider)
    }
}

/// Uma chave de API. Implementada pelo `ApiKey` do cofre do app; o
/// transporte le-a so para o cabecalho de autenticacao.
pub trait ApiCredential {
    fn secret(&self) -> &str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// Um pedido montado por um construtor puro: sem chave, sem host. O caminho
/// comeca sempre por `/` e so leva constantes, identificadores validados
/// (`ModelId`) e texto codificado para URL.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    pub(crate) provider: Provider,
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) body: Vec<u8>,
    pub(crate) timeout: Duration,
}

impl ApiRequest {
    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn method(&self) -> Method {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

fn user_agent() -> String {
    format!("NeuralIA/{}", env!("CARGO_PKG_VERSION"))
}

/// O cliente das chamadas de IA de um `Endpoint`.
pub struct ApiClient {
    agent: Agent,
    endpoint: Endpoint,
    max_body: usize,
}

impl ApiClient {
    pub fn new(endpoint: Endpoint) -> Self {
        Self::with_body_cap(endpoint, DEFAULT_MAX_BODY_BYTES)
    }

    pub fn with_body_cap(endpoint: Endpoint, max_body: usize) -> Self {
        Self {
            agent: policy_agent(endpoint.provider.is_some(), endpoint.locality),
            endpoint,
            max_body,
        }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// A configuracao do agente (so leitura), para o gate da politica de
    /// rede (`tests/llm_client_policy.rs`).
    pub fn agent_config(&self) -> &ureq::config::Config {
        self.agent.config()
    }

    /// Envia o pedido e devolve o corpo de uma resposta 2xx, ate ao tecto.
    /// Qualquer outro estado vira `ApiError` sem o corpo; a chave, quando ha,
    /// vai so no cabecalho de autenticacao do fornecedor.
    pub fn send(
        &self,
        request: &ApiRequest,
        key: Option<&dyn ApiCredential>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, ApiError> {
        if cancelled() {
            return Err(ApiError::Cancelled);
        }
        // Um pedido da Gemini nunca sai por um host fixado de outro
        // fornecedor (nem a chave dele).
        if !self.endpoint.serves(request.provider) || !request.path.starts_with('/') {
            return Err(ApiError::ServiceUnavailable { status: None });
        }
        let deadline = Instant::now() + request.timeout;
        let mut response = self.call(request, key)?;

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let retry_after = header(&response, "retry-after").map(str::to_owned);
            let body = read_body(
                &mut response,
                ERROR_BODY_MAX_BYTES,
                deadline,
                cancelled,
                Overflow::Truncate,
            )?;
            return Err(classify_status(status, retry_after.as_deref(), &body));
        }

        // Um `Content-Length` acima do tecto e recusado antes de se ler um
        // byte.
        if let Some(declared) =
            header(&response, "content-length").and_then(|value| value.parse::<u64>().ok())
            && declared > self.max_body as u64
        {
            return Err(ApiError::TooLarge {
                limit: self.max_body as u64,
            });
        }
        read_body(
            &mut response,
            self.max_body,
            deadline,
            cancelled,
            Overflow::Fail,
        )
    }

    fn call(
        &self,
        request: &ApiRequest,
        key: Option<&dyn ApiCredential>,
    ) -> Result<Response<Body>, ApiError> {
        let url = self.endpoint.url(&request.path);
        let auth = request.provider.auth_header();
        let result = match request.method {
            Method::Get => {
                let mut builder = self.agent.get(&url).header("accept", "application/json");
                if let Some(key) = key {
                    builder = builder.header(auth, key.secret());
                }
                builder
                    .config()
                    .timeout_global(Some(request.timeout))
                    .build()
                    .call()
            }
            Method::Post => {
                let mut builder = self
                    .agent
                    .post(&url)
                    .header("accept", "application/json")
                    .header("content-type", "application/json");
                if let Some(key) = key {
                    builder = builder.header(auth, key.secret());
                }
                builder
                    .config()
                    .timeout_global(Some(request.timeout))
                    .build()
                    .send(request.body.as_slice())
            }
        };
        result.map_err(map_transport_error)
    }
}

/// O agente com a politica do transporte: sem proxy do ambiente, sem
/// redirects, o estado HTTP entregue ao classificador, HTTPS so num host
/// fixado, os certificados do sistema, o User-Agent do NeuralIA e o
/// resolvedor da localidade. O `ApiClient` e o cliente da lista do bloqueio
/// de anuncios (`crate::adblock::ListClient`) nascem os dois daqui: a lista
/// anda com a mesma politica que uma chamada de IA.
pub(crate) fn policy_agent(https_only: bool, locality: Locality) -> Agent {
    let config = Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .https_only(https_only)
        .user_agent(user_agent())
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build();
    agent_for(config, locality)
}

/// O agente da localidade: o conector do sistema e o resolvedor que so
/// deixa ligar a enderecos dela.
fn agent_for(config: Config, locality: Locality) -> Agent {
    match locality {
        Locality::Public => {
            Agent::with_parts(config, DefaultConnector::new(), PublicResolver::default())
        }
        Locality::Loopback => {
            Agent::with_parts(config, DefaultConnector::new(), LoopbackResolver::default())
        }
        Locality::Lan => Agent::with_parts(config, DefaultConnector::new(), LanResolver::default()),
    }
}

pub(crate) fn header<'a>(response: &'a Response<Body>, name: &str) -> Option<&'a str> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
}

/// Os erros do ureq viram `ApiError` sem levar nada deles: o texto de um
/// erro do ureq pode citar o URL ou um cabecalho.
pub(crate) fn map_transport_error(error: ureq::Error) -> ApiError {
    match error {
        ureq::Error::Timeout(_) => ApiError::Timeout,
        ureq::Error::Io(ref io) if io.kind() == std::io::ErrorKind::TimedOut => ApiError::Timeout,
        ureq::Error::BodyExceedsLimit(limit) => ApiError::TooLarge { limit },
        ureq::Error::StatusCode(status) => classify_status(status, None, &[]),
        _ => ApiError::ServiceUnavailable { status: None },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overflow {
    /// Corpo de sucesso: passar do tecto e `TooLarge`.
    Fail,
    /// Corpo de erro: guarda-se o inicio, o resto nao interessa.
    Truncate,
}

/// Le o corpo em blocos, para haver onde desistir e onde ver o prazo.
pub(crate) fn read_body(
    response: &mut Response<Body>,
    cap: usize,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
    overflow: Overflow,
) -> Result<Vec<u8>, ApiError> {
    let capacity = response
        .body()
        .content_length()
        .map_or(0, |length| length.min(cap as u64) as usize);
    let mut reader = response.body_mut().with_config().limit(cap as u64).reader();

    let mut body = Vec::with_capacity(capacity);
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        if cancelled() {
            return Err(ApiError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ApiError::Timeout);
        }
        let read = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) if overflow == Overflow::Truncate => break,
            Err(error) => return Err(map_transport_error(ureq::Error::from(error))),
        };
        // O `.limit()` do ureq conta bytes ANTES do gzip/brotli (o
        // LimitReader e a camada mais interna do BodyReader); esta contagem e
        // a unica que trava o corpo DESCODIFICADO.
        let decoded = body.len().saturating_add(read);
        if decoded > cap {
            if overflow == Overflow::Truncate {
                body.extend_from_slice(&chunk[..cap - body.len()]);
                break;
            }
            return Err(ApiError::TooLarge { limit: cap as u64 });
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Ok(body)
}
