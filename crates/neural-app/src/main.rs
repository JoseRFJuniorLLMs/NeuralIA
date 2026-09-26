#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

// O parser do canal IPC (SPEC-0108) não tem uma única chamada ao Windows: é
// JSON, validação de argumentos e comparação em tempo constante. Estava atrás
// de `cfg(target_os = "windows")` por arrastamento, o que deixava a superfície
// mais sensível do produto sem ser compilada nem testada fora do Windows.
// Fora do Windows ninguém o chama ainda; daí o `allow(dead_code)`.
// O leitor de EPUB (servidor da origem, parser do IPC, worker da biblioteca)
// também é portátil: os gates correm no runner Linux do CI.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod epub_app;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod ipc;
// Botoes da janela na Home, largura do painel lateral, roda do rato sobre ele
// e modos do painel de servicos: decisoes puras, testadas tambem no Linux.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod panel_chrome;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod pdf_assets;
// Abas e grupos guardados entre sessoes: JSON e ficheiros, portatil e testado
// tambem no runner Linux. So o Windows o chama.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod tab_session;
// O script da leitura em voz alta e os seus gates (node:vm) nao dependem do
// Windows: correm tambem no CI Linux.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod read_aloud;
// Spike do AcceleratorKeyPressed (infra-accel-spike, plano 2.3): so nos
// testes e no build de CI com `--features accel-spike`. O exe publicado e
// compilado sem a feature e nao o tem (scripts/test-accel-spike-marker.ps1).
#[cfg(any(test, feature = "accel-spike"))]
mod accel_spike;
// Gemini Live: origem propria, canal fechado e chave com DPAPI (Windows).
#[cfg(target_os = "windows")]
mod gemini_live;
// O cofre das chaves (infra-settings-keys, plano 2.3): a DPAPI com entropia
// por uso, os slots, a `ApiKey` e a redacao do log (Windows). Os
// consumidores (traducao, juiz, BYOM, conectores) chegam nas ondas
// seguintes; ate la parte dele so corre nos testes.
#[cfg(target_os = "windows")]
#[cfg_attr(not(test), allow(dead_code))]
mod secrets;
// As lojas da pasta de dados e o tipo de cada uma: portatil, testado tambem
// no runner Linux.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod stores;
#[cfg(target_os = "windows")]
mod windows_app;

// Pomodoro da barra: so decisoes (clique, menu, `pomodoro:`, tique, opcoes em
// disco), sem Win32 -- testavel em qualquer plataforma.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod pomodoro_ui;

// A saida da IA (infra-egress, plano 2.3): o portao que decide se um dado
// sai para um modelo (consentimento, segundo plano, modo privado, limite
// mensal), as definicoes e o consumo em `ai/`, e o worker que so nasce no
// primeiro trabalho. Portateis, testados tambem no Linux. A Traducao (G13) e
// a primeira feature a pedir o portao (`App::egress_gate`); ate la o
// dead_code so e exigido nos testes do Windows, que os usam todos.
#[cfg_attr(not(all(test, target_os = "windows")), allow(dead_code))]
mod ai_settings;
#[cfg_attr(not(all(test, target_os = "windows")), allow(dead_code))]
mod egress;
#[cfg_attr(not(all(test, target_os = "windows")), allow(dead_code))]
mod lazy_worker;

// Centro de avisos (infra-notify-popups): a fila, a entrega pelo Foco e pela
// privacidade e o unico aviso do canto, sem Win32 -- testado tambem no Linux.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod notify;

#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_app::run()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!(
        "NeuralIA desktop v{} é Windows-first.",
        env!("CARGO_PKG_VERSION")
    );
    println!("O núcleo continua portátil: cargo test -p neural-core");
    println!("O parser do canal IPC também: cargo test -p neural-app");
}
