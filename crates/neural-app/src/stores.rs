//! As lojas do NeuralIA na pasta de dados e o tipo de cada uma
//! (infra-settings-keys, plano 2.3; regra do tipo em
//! `neural_core::json_store`).
//!
//! `APP_STORES` e a tabela de tudo o que o produto guarda em `<data_dir>`.
//! O gate `existing_stores_have_a_declared_kind` (em `windows_app/tests.rs`)
//! percorre o codigo que embarca e falha se aparecer um `data_dir.join(...)`
//! que nao esteja aqui: uma loja nova nasce com o seu tipo declarado. O
//! registo das lojas so e cunhado no `App::new`; hoje abrem as suas por grant
//! o cofre das chaves (`KEYS_STORE`, `LIVE_KEY_STORE`) e o portao de saida da
//! IA (`AI_SETTINGS_STORE`, `AI_USAGE_STORE`, em `egress.rs`) e a Traducao
//! (`TRANSLATE_STORE`, a loja «Sempre neste site»). Levar as outras
//! para grants e o `no_raw_data_dir_write_outside_a_grant` do
//! infra-privacy-guard.

use neural_core::json_store::StoreKind::{Automatic, Explicit, Setting};
use neural_core::json_store::StoreShape::{Dir, File};
use neural_core::json_store::StoreSpec;

/// `<data_dir>/keys/<slot>.key`: as chaves de API (`secrets::KeyVault`).
/// O utilizador colou-as: `Explicit`. Nunca entram no Ctrl+Shift+Delete.
pub(crate) const KEYS_STORE: StoreSpec = StoreSpec::new("keys", Explicit, Dir);
/// A chave do Gemini Live, a do slot `KeySlot::Gemini` do cofre.
pub(crate) const LIVE_KEY_STORE: StoreSpec = StoreSpec::new("gemini-live.key", Explicit, File);
/// `<data_dir>/ai/settings.json`: as finalidades da IA e o limite mensal
/// (`ai_settings.rs`). So muda por uma escolha em IA › Cérebros: `Setting`.
pub(crate) const AI_SETTINGS_STORE: StoreSpec = StoreSpec::new("ai/settings.json", Setting, File);
/// `<data_dir>/ai/usage.json`: as chamadas pagas do mes, contadas pelo
/// `EgressGate` a cada envio. `Setting`, como o limite que ele guarda: e a
/// base do limite mensal que o dono escolheu, e como `Automatic` o modo
/// privado das lojas saltaria as gravacoes e o limite recomecaria na sessao
/// seguinte (gate `egress::tests::usage_survives_the_private_store_mode`).
pub(crate) const AI_USAGE_STORE: StoreSpec = StoreSpec::new("ai/usage.json", Setting, File);
/// `<data_dir>/translate.json`: «Sempre neste site» da Traducao -- os pares
/// (cerebro, origem) que o dono autorizou num cartao (`egress::SiteGrants`).
/// So muda por um clique nesse botao, que nunca existe num contexto
/// privado: `Setting`. Nunca leva texto de uma pagina nem uma traducao.
pub(crate) const TRANSLATE_STORE: StoreSpec = StoreSpec::new("translate.json", Setting, File);
/// `<data_dir>/downloads.json`: o registo dos downloads acabados
/// (`neural_core::downloads::DownloadLog`), escrito ao fim de cada um --
/// efeito lateral do uso. Nunca leva um download do Split privado nem de
/// um servico InPrivate, e no Modo privado nao se escreve. Sai no
/// Ctrl+Shift+Delete.
pub(crate) const DOWNLOADS_LOG_STORE: StoreSpec = StoreSpec::new("downloads.json", Automatic, File);
/// `<data_dir>/downloads-settings.json`: a pasta dos downloads e
/// «Permitir baixar programas». Hoje o produto so o le; quem o escreve e a
/// seccao Downloads do downloads-ui.
pub(crate) const DOWNLOADS_SETTINGS_STORE: StoreSpec =
    StoreSpec::new("downloads-settings.json", Setting, File);

/// Tudo o que o produto guarda em `<data_dir>`, com o tipo. Um ficheiro, um
/// tipo; a regra: `Setting` = escolha num menu ou definicao; `Explicit` =
/// o utilizador pediu para guardar; `Automatic` = efeito lateral do uso.
/// Hoje so o gate a le; o infra-privacy-guard abre cada loja por ela.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const APP_STORES: &[StoreSpec] = &[
    // O historico cronologico: cada pagina aberta.
    StoreSpec::new("history.jsonl", Automatic, File),
    // A memoria semantica local, capturada ao ler.
    StoreSpec::new("memory", Automatic, Dir),
    // As abas e os grupos do comparador, gravados ao mudar; o trinco da
    // primeira janela e a geracao do "Apagar historico".
    StoreSpec::new("tabs.json", Automatic, File),
    StoreSpec::new("tabs.lock", Automatic, File),
    StoreSpec::new("tabs.cleared", Automatic, File),
    // A largura escolhida para os paineis: geometria, mas escrita ao
    // arrastar a pega -- efeito lateral do uso.
    StoreSpec::new("panel-width.json", Automatic, File),
    // Os registos e a auditoria de cada corrida do agente.
    StoreSpec::new("agent", Automatic, Dir),
    // O perfil do WebView2 (cookies, cache, inicios de sessao).
    StoreSpec::new("WebView2", Automatic, Dir),
    // Escolhas nos menus: tema, avisos do Gmail, duracoes do Pomodoro.
    StoreSpec::new("theme", Setting, File),
    StoreSpec::new("gmail", Setting, File),
    StoreSpec::new("pomodoro", Setting, File),
    // As notas (Zettelkasten) que o utilizador escreveu.
    StoreSpec::new("zettel", Explicit, Dir),
    // Os livros que o utilizador acrescentou. O indice guarda tambem a
    // posicao de leitura e quando cada livro foi aberto (efeito lateral do
    // uso, apagado pelo Ctrl+Shift+Delete): a divisao fica para o
    // infra-privacy-guard, que decide o modo privado da biblioteca.
    StoreSpec::new("library", Explicit, Dir),
    // "Exportar pesquisa": um Markdown pedido pelo utilizador.
    StoreSpec::new("research-exports", Explicit, Dir),
    DOWNLOADS_LOG_STORE,
    DOWNLOADS_SETTINGS_STORE,
    LIVE_KEY_STORE,
    KEYS_STORE,
    AI_SETTINGS_STORE,
    AI_USAGE_STORE,
    TRANSLATE_STORE,
];

#[cfg(test)]
mod tests {
    use super::*;
    use neural_core::json_store::StoreRegistry;

    #[test]
    fn the_store_table_grants_cleanly_from_one_registry() {
        // Nomes unicos, validos, e um so tipo por nome: a tabela inteira
        // cabe num registo. Unicos sem maiusculas: no NTFS `WebView2` e
        // `webview2` sao a mesma pasta.
        let registry = StoreRegistry::mint_for_test(std::env::temp_dir().join("neuralia-stores"));
        let mut names: Vec<String> = APP_STORES
            .iter()
            .map(|spec| spec.name.to_ascii_lowercase())
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), APP_STORES.len(), "nomes repetidos na tabela");
        for spec in APP_STORES {
            let grant = registry
                .grant(*spec)
                .unwrap_or_else(|error| panic!("{}: {error}", spec.name));
            assert_eq!((grant.kind(), grant.shape()), (spec.kind, spec.shape));
        }
    }
}
