//! As lojas do NeuralIA na pasta de dados e o tipo de cada uma
//! (infra-settings-keys, plano 2.3; regra do tipo em
//! `neural_core::json_store`).
//!
//! `APP_STORES` e a tabela de tudo o que o produto guarda em `<data_dir>`.
//! O gate `existing_stores_have_a_declared_kind` (em `windows_app/tests.rs`)
//! percorre o codigo que embarca e falha se aparecer um `data_dir.join(...)`
//! que nao esteja aqui: uma loja nova nasce com o seu tipo declarado. O
//! registo das lojas e cunhado uma vez, no `App::new`, e entregue ao
//! `PrivacyGuard` (`crate::privacy`), que e quem passa os grants
//! (`guard.store(spec)`): o cofre das chaves (`KEYS_STORE`,
//! `LIVE_KEY_STORE`), o portao de saida da IA (`AI_SETTINGS_STORE`,
//! `AI_USAGE_STORE`, em `egress.rs`), a Traducao (`TRANSLATE_STORE`), o
//! bloqueio, os downloads e os favoritos abrem as suas por grant; o
//! historico, a memoria e as abas (`HISTORY_STORE`, `MEMORY_STORE`,
//! `TABS_STORE`) sao do proprio guard, que e o unico que as escreve. O que
//! ainda escreve pelo caminho esta na tabela da fase 0 (SPEC-0006) e na
//! lista do gate `no_raw_data_dir_write_outside_a_grant`.

use neural_core::json_store::StoreKind::{Automatic, Explicit, Setting};
use neural_core::json_store::StoreShape::{Dir, File};
use neural_core::json_store::StoreSpec;

/// `<data_dir>/history.jsonl`: o historico cronologico, cada pagina aberta.
/// Efeito lateral do uso: `Automatic`. So o `PrivacyGuard` o escreve
/// (`record`), e no modo privado nao escreve.
pub(crate) const HISTORY_STORE: StoreSpec = StoreSpec::new("history.jsonl", Automatic, File);
/// `<data_dir>/memory`: a memoria semantica local, capturada ao ler
/// (documentos, wiki, tombstones, o indice SQLite derivado e as sessoes de
/// pesquisa em `sessions/`). `Automatic`: so o `PrivacyGuard` a escreve
/// (`capture`, `save_session`), e no modo privado nao escreve.
pub(crate) const MEMORY_STORE: StoreSpec = StoreSpec::new("memory", Automatic, Dir);
/// `<data_dir>/tabs.json`: as abas e os grupos do comparador, gravados ao
/// mudar. `Automatic`: so o `PrivacyGuard` os grava (`save_tabs`), e no
/// modo privado nao grava. `tabs.lock` e `tabs.cleared` (o trinco da
/// primeira janela e a geracao do "Apagar historico") vivem ao lado.
pub(crate) const TABS_STORE: StoreSpec = StoreSpec::new("tabs.json", Automatic, File);
/// `<data_dir>/keys/<slot>.key`: as chaves de API (`secrets::KeyVault`).
/// O utilizador colou-as: `Explicit`. Nunca entram no Ctrl+Shift+Delete.
pub(crate) const KEYS_STORE: StoreSpec = StoreSpec::new("keys", Explicit, Dir);
/// A chave do Gemini Live, a do slot `KeySlot::Gemini` do cofre.
pub(crate) const LIVE_KEY_STORE: StoreSpec = StoreSpec::new("gemini-live.key", Explicit, File);
/// `<data_dir>/adblock-settings.json`: o bloqueio de anuncios ligado e os
/// sites com anuncios permitidos (e o campo reservado da anti-distracao).
/// Muda so por uma escolha no menu: `Setting`.
pub(crate) const ADBLOCK_SETTINGS_STORE: StoreSpec =
    StoreSpec::new("adblock-settings.json", Setting, File);
/// `<data_dir>/adblock-list.json`: a lista de Peter Lowe baixada. Escrita
/// como efeito lateral de ter o bloqueio ligado (a renovacao semanal):
/// `Automatic` -- no modo privado nao se renova.
pub(crate) const ADBLOCK_LIST_STORE: StoreSpec =
    StoreSpec::new("adblock-list.json", Automatic, File);
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
/// `<data_dir>/bookmarks.json`: os favoritos (`neural_core::bookmarks`).
/// O utilizador pediu cada um (Ctrl+D, a estrela, importar): `Explicit` --
/// no Split privado e no Modo privado grava na mesma, e o Ctrl+Shift+Delete
/// nunca o apaga. Partilhado entre janelas pelo trinco
/// `bookmarks.json.lock`; so a thread `neural-bookmarks` o escreve.
pub(crate) const BOOKMARKS_STORE: StoreSpec = StoreSpec::new("bookmarks.json", Explicit, File);
/// `<data_dir>/downloads-settings.json`: a pasta dos downloads e
/// «Permitir baixar programas». A seccao Downloads (downloads-ui) grava o
/// interruptor (`App::set_allow_programs`); a pasta ainda so se le.
pub(crate) const DOWNLOADS_SETTINGS_STORE: StoreSpec =
    StoreSpec::new("downloads-settings.json", Setting, File);
/// Conversas e estado do hub de agentes externos. Automatic: uma sessao
/// privada nao persiste conversa; as credenciais do canal so existem quando
/// a loja permite escrita.
pub(crate) const AGENTS_STORE: StoreSpec = StoreSpec::new("agents", Automatic, Dir);

/// Tudo o que o produto guarda em `<data_dir>`, com o tipo. Um ficheiro, um
/// tipo; a regra: `Setting` = escolha num menu ou definicao; `Explicit` =
/// o utilizador pediu para guardar; `Automatic` = efeito lateral do uso.
/// So o gate a le; cada loja que abre por grant tem a sua constante acima.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const APP_STORES: &[StoreSpec] = &[
    HISTORY_STORE,
    MEMORY_STORE,
    // O trinco da primeira janela e a geracao do "Apagar historico", ao
    // lado do `TABS_STORE`.
    TABS_STORE,
    StoreSpec::new("tabs.lock", Automatic, File),
    StoreSpec::new("tabs.cleared", Automatic, File),
    // A largura escolhida para os paineis: geometria, mas escrita ao
    // arrastar a pega -- efeito lateral do uso.
    StoreSpec::new("panel-width.json", Automatic, File),
    // Os registos e a auditoria de cada corrida do agente.
    StoreSpec::new("agent", Automatic, Dir),
    AGENTS_STORE,
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
    ADBLOCK_SETTINGS_STORE,
    ADBLOCK_LIST_STORE,
    AI_SETTINGS_STORE,
    AI_USAGE_STORE,
    TRANSLATE_STORE,
    BOOKMARKS_STORE,
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
