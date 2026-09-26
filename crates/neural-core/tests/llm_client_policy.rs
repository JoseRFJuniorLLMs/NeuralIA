//! Gate da politica de rede do cliente de IA (infra-llm-transport): o
//! `ApiClient` que embarca ignora as variaveis de proxy do ambiente, nao
//! segue redirects, recebe o estado HTTP para o classificar, so fala HTTPS com
//! o host fixado e usa os certificados do sistema.
//!
//! O cliente da lista do bloqueio de anuncios (`adblock::ListClient`) nasce
//! do mesmo agente e fica preso as mesmas asserções.
//!
//! Binario proprio com um unico teste: o ureq 3 le `ALL_PROXY`/`HTTPS_PROXY`/
//! `HTTP_PROXY` quando se cria o agente, e so num processo sem outras threads
//! se pode mudar o ambiente sem corrida. Nenhum pedido sai daqui.

use neural_core::adblock::{ListClient, ListEndpoint};
use neural_core::llm::{ApiClient, Endpoint, Provider};
use ureq::config::AutoHeaderValue;
use ureq::tls::RootCerts;

#[test]
fn the_ai_client_ignores_proxy_variables_and_keeps_its_policy() {
    // SAFETY: este binario de teste tem um so teste e nenhuma outra thread
    // corre enquanto o ambiente muda.
    unsafe {
        for name in ["ALL_PROXY", "HTTPS_PROXY", "HTTP_PROXY"] {
            std::env::set_var(name, "http://127.0.0.1:9");
        }
    }
    assert!(
        ureq::Agent::new_with_defaults().config().proxy().is_some(),
        "the check must see the proxy a default agent would take"
    );

    let client = ApiClient::new(Endpoint::pinned(Provider::Gemini));
    let list = ListClient::new(ListEndpoint::pinned());
    for config in [client.agent_config(), list.agent_config()] {
        assert_policy(config);
    }
}

fn assert_policy(config: &ureq::config::Config) {
    assert!(
        config.proxy().is_none(),
        "the AI client never goes through a proxy from the environment"
    );
    assert_eq!(config.max_redirects(), 0, "a 3xx is never followed");
    assert!(
        !config.http_status_as_error(),
        "every status reaches the classifier"
    );
    assert!(config.https_only(), "the pinned endpoint is HTTPS only");
    assert!(
        matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ),
        "the operating system verifies the certificates"
    );
    let expected = format!("NeuralIA/{}", env!("CARGO_PKG_VERSION"));
    assert!(
        matches!(config.user_agent(), AutoHeaderValue::Provided(value) if **value == expected),
        "User-Agent {expected}: {:?}",
        config.user_agent()
    );
}
