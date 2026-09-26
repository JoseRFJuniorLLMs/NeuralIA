use super::*;

use std::collections::HashMap;
use std::sync::{LazyLock, PoisonError, RwLock};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_BACK, VK_DELETE, VK_F1, VK_OEM_1, VK_TAB,
};
use winit::keyboard::ModifiersState;

// ===================== o mapa de teclas (infra-commands-keymap) =====================
//
// Um atalho e uma `Chord` (a tecla virtual e os modificadores, exatos) num
// ambito (`KeyScope`). Quem pergunta -- o `AcceleratorKeyPressed` de cada
// WebView, a janela (winit) e a omnibox da Home (o subclass do EDIT) --
// pergunta a MESMA coisa: `accelerator_decision(mapa, tecla, origem)`, pura.
// A origem diz a cadeia de ambitos (`scope_chain`) e o primeiro ambito que
// declara o atalho decide.
//
// No `AcceleratorKeyPressed` a decisao corre dentro do callback de entrada
// sincrona do WebView2: por isso e uma leitura do mapa (`KEYMAP`, com um
// `RwLock` so de leitura aqui) e um `HashMap` por ambito (no maximo tres),
// e o que ela devolve e so um evento para o `proxy.send_event` -- nenhuma
// chamada COM la dentro (RPC_E_CANTCALLOUT_ININPUTSYNCCALL). Quem corre o
// comando e o event loop, contra a origem que veio com o evento
// (`resolve_command`).
//
// O que decide:
// - so uma DESCIDA de um atalho preso e tratada (`Handled = TRUE`: a pagina
//   nao ve o keydown); a subida e as teclas soltas ficam como estavam;
// - a tecla presa (a repeticao, `WasKeyDown`) continua tratada mas nao
//   dispara outra vez;
// - nos hospedeiros das IAs (`is_provider_origin`) os atalhos do ChatGPT
//   (`PROVIDER_DENYLIST`) nunca sao presos, seja qual for o ambito que os
//   declare -- a Captura (Ctrl+Shift+S, quando chegar) fica assim sem
//   atalho nas IAs;
// - a superficie de arquivos declara ja a sua tabela (`FILES_OVERRIDES`):
//   nesses atalhos os arquivos ganham, e so neles.
//
// O spike de CI (infra-accel-spike) mediu o despacho nativo nos nove
// hospedeiros sem fallback: com `Handled = TRUE` a pagina nunca ve o
// keydown de um atalho preso, mas ainda recebe um ou dois `keypress` (o
// caractere de controlo de um Ctrl+letra). O spike prendia a descida, a
// repeticao E a subida, e a sonda dele so ouvia `keydown` e `keypress`:
// esta decisao deixa a subida passar (so a descida e a repeticao sao
// tratadas), por isso a pagina recebe tambem o `keyup` de um atalho preso
// -- o que o spike nao mediu. Os atalhos das paginas -- o
// `NEURALIA_KEYMAP_SCRIPT` e os dos sites -- que ouvem o keydown nao
// disparam num atalho preso; uma pagina que reaja ao `keypress` ou ao
// `keyup` ve o toque (e nem essa faz correr o comando, que nasce aqui).
//
// O unico atalho que as WebViews prendem e o Ctrl+D dos favoritos (ambito
// `Global`, `CommandId::Bookmark`); os outros atalhos das paginas
// continuam no `NEURALIA_KEYMAP_SCRIPT` (intocado), e os da 2.2.0 no
// ambito `Window` (a janela e a omnibox). Cada atalho novo da 2.3 entra com
// o seu comando, no PR dele (regra C13).

/// Uma combinacao de teclas: a tecla virtual (a que o
/// `AcceleratorKeyPressed` e o `WM_KEYDOWN` dao) e os tres modificadores,
/// exatos -- Ctrl+R nao e Ctrl+Shift+R.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::windows_app) struct Chord {
    pub(in crate::windows_app) vk: u32,
    pub(in crate::windows_app) ctrl: bool,
    pub(in crate::windows_app) shift: bool,
    pub(in crate::windows_app) alt: bool,
}

impl Chord {
    /// Ctrl+tecla.
    pub(in crate::windows_app) const fn ctrl(vk: u8) -> Chord {
        Chord {
            vk: vk as u32,
            ctrl: true,
            shift: false,
            alt: false,
        }
    }

    /// Ctrl+Shift+tecla.
    pub(in crate::windows_app) const fn ctrl_shift(vk: u8) -> Chord {
        Chord {
            vk: vk as u32,
            ctrl: true,
            shift: true,
            alt: false,
        }
    }

    /// A tecla sozinha (so F1-F24 servem de atalho assim).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::windows_app) const fn key(vk: u8) -> Chord {
        Chord {
            vk: vk as u32,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// A combinacao de uma tecla lida.
    pub(in crate::windows_app) fn of(input: AcceleratorInput) -> Chord {
        Chord {
            vk: input.vk,
            ctrl: input.ctrl,
            shift: input.shift,
            alt: input.alt,
        }
    }

    /// Pode ser atalho: F1-F24, ou Ctrl OU Alt com uma tecla. Uma tecla sem
    /// modificador e texto; Ctrl+Alt e o AltGr do teclado ABNT2 (Ctrl+Alt+Q
    /// escreve "/"), e prende-lo tirava caracteres a quem escreve.
    pub(in crate::windows_app) fn bindable(self) -> bool {
        let function_key = (u32::from(VK_F1)..u32::from(VK_F1) + 24).contains(&self.vk);
        if self.ctrl && self.alt {
            return false;
        }
        function_key || self.ctrl || self.alt
    }
}

/// O ambito de um atalho: onde ele vale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::windows_app) enum KeyScope {
    /// Em toda a parte que tem teclado: a janela, a omnibox e cada WebView
    /// (menos o monitor escondido do Gmail).
    Global,
    /// As paginas: colunas, fonte ao lado (privada incluida), Web completa,
    /// Leitor, PDF e livros.
    Page,
    /// Os paineis: o do Ctrl+H, os servicos e o Gemini Live.
    Panel,
    /// A superficie de arquivos (o hospedeiro chega com os itens files-*):
    /// ganha a `Page` e a `Global` nos atalhos que declara.
    Files,
    /// O chrome nativo: a janela com o teclado (depois de um clique na
    /// barra) e a omnibox da Home. Tem os atalhos que a 2.2.0 ja tinha la;
    /// nas paginas, os mesmos atalhos continuam a ser do
    /// `NEURALIA_KEYMAP_SCRIPT`, e por isso nao sao `Global`.
    Window,
}

/// O que um ambito faz com um atalho que declara.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// Corre este comando.
    Run(CommandId),
    /// O atalho e deste ambito, mas fica para a pagina: nem o comando de um
    /// ambito mais largo o apanha. E o que a tabela dos arquivos declara
    /// ate cada item files-* ligar o seu comando.
    Pass,
}

/// Os atalhos de cada ambito, lidos por `accelerator_decision`.
#[derive(Debug, Default)]
pub(in crate::windows_app) struct Keymap {
    table: HashMap<(KeyScope, Chord), Binding>,
}

/// Porque uma tabela de comandos nao vira mapa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum KeymapError {
    /// Dois comandos com o mesmo atalho no mesmo ambito.
    Duplicate {
        scope: KeyScope,
        chord: Chord,
        first: CommandId,
        second: CommandId,
    },
    /// Uma tecla que nao pode ser atalho (`Chord::bindable`).
    NotAChord { command: CommandId, chord: Chord },
}

impl Keymap {
    /// O mapa de uma tabela de comandos, mais a tabela dos arquivos. Um
    /// atalho repetido num ambito, ou uma tecla que nao pode ser atalho,
    /// recusa a tabela inteira: nada a meio.
    pub(in crate::windows_app) fn build(rows: &[CommandRow]) -> Result<Keymap, KeymapError> {
        let mut table = HashMap::new();
        for row in rows {
            for spec in row.chords {
                if !spec.chord.bindable() {
                    return Err(KeymapError::NotAChord {
                        command: row.id,
                        chord: spec.chord,
                    });
                }
                let slot = (spec.scope, spec.chord);
                if let Some(Binding::Run(first)) = table.get(&slot) {
                    return Err(KeymapError::Duplicate {
                        scope: spec.scope,
                        chord: spec.chord,
                        first: *first,
                        second: row.id,
                    });
                }
                table.insert(slot, Binding::Run(row.id));
            }
        }
        for chord in FILES_OVERRIDES {
            table
                .entry((KeyScope::Files, chord))
                .or_insert(Binding::Pass);
        }
        Ok(Keymap { table })
    }

    /// O comando de `chord` para quem consulta os ambitos de `chain` por
    /// esta ordem. O primeiro ambito que declara o atalho decide: um
    /// `Pass` deixa-o a pagina sem descer aos outros.
    pub(in crate::windows_app) fn lookup(
        &self,
        chain: &[KeyScope],
        chord: Chord,
    ) -> Option<CommandId> {
        for scope in chain {
            match self.table.get(&(*scope, chord)) {
                Some(Binding::Run(command)) => return Some(*command),
                Some(Binding::Pass) => return None,
                None => {}
            }
        }
        None
    }
}

/// Os ambitos que cada origem consulta, do mais estreito ao mais largo.
pub(in crate::windows_app) fn scope_chain(origin: CommandOrigin) -> &'static [KeyScope] {
    const CHROME: &[KeyScope] = &[KeyScope::Window, KeyScope::Global];
    const PAGE: &[KeyScope] = &[KeyScope::Page, KeyScope::Global];
    const PANEL: &[KeyScope] = &[KeyScope::Panel, KeyScope::Global];
    match origin {
        CommandOrigin::Window | CommandOrigin::Omnibox => CHROME,
        CommandOrigin::Host(host) => match host {
            WebViewHost::Column(_)
            | WebViewHost::Split(_)
            | WebViewHost::PrivateSplit(_)
            | WebViewHost::External
            | WebViewHost::Reader
            | WebViewHost::Pdf
            | WebViewHost::Epub => PAGE,
            WebViewHost::Live | WebViewHost::SidePanel | WebViewHost::Service(_) => PANEL,
            // 1x1, fora do ecra, nunca com o foco: nenhum atalho.
            WebViewHost::GmailMonitor => &[],
        },
    }
}

/// A cadeia do hospedeiro de arquivos, quando ele chegar (files-viewer,
/// files-explorer, files-editor acrescentam-no a `scope_chain`): os
/// arquivos primeiro, depois as paginas e o resto.
#[cfg_attr(not(test), allow(dead_code))]
pub(in crate::windows_app) const FILES_CHAIN: &[KeyScope] =
    &[KeyScope::Files, KeyScope::Page, KeyScope::Global];

/// Os atalhos que a superficie de arquivos toma para si (os do Explorador
/// do Windows e de um editor): nela ganham a qualquer `Page` ou `Global`,
/// e fora dela nao existem. Ate cada item files-* ligar o seu comando, sao
/// `Pass`: ficam para a pagina dos arquivos.
pub(in crate::windows_app) const FILES_OVERRIDES: [Chord; 8] = [
    Chord::ctrl(b'D'),
    Chord::ctrl(b'N'),
    Chord::ctrl(b'W'),
    Chord::ctrl(VK_TAB as u8),
    Chord::ctrl(b'G'),
    Chord::ctrl(b'O'),
    Chord::ctrl(b'S'),
    Chord::ctrl_shift(b'S'),
];

/// Os atalhos do ChatGPT, que a NeuralIA nunca prende nos hospedeiros das
/// IAs por omissao (a pagina fica com eles): conversa nova, copiar o
/// ultimo bloco de codigo, copiar a ultima resposta, instrucoes
/// personalizadas, a barra lateral (a mesma tecla da Captura) e apagar a
/// conversa.
pub(in crate::windows_app) const PROVIDER_DENYLIST: [Chord; 6] = [
    Chord::ctrl_shift(b'O'),
    Chord::ctrl_shift(VK_OEM_1 as u8),
    Chord::ctrl_shift(b'C'),
    Chord::ctrl_shift(b'I'),
    Chord::ctrl_shift(b'S'),
    Chord::ctrl_shift(VK_BACK as u8),
];

/// Um hospedeiro das IAs: as colunas do comparador e a resposta de uma IA
/// no painel privado.
pub(in crate::windows_app) fn is_provider_origin(origin: CommandOrigin) -> bool {
    matches!(
        origin,
        CommandOrigin::Host(WebViewHost::Column(_) | WebViewHost::PrivateSplit(_))
    )
}

/// A decisao para uma tecla, pura: `handled` so para a descida de um
/// atalho preso nesta origem, e o evento so na primeira descida (a tecla
/// presa fica tratada sem disparar de novo). O evento leva a origem que
/// veio com a tecla -- o hospedeiro da WebView, a janela ou a omnibox --,
/// nunca nada da pagina.
pub(in crate::windows_app) fn accelerator_decision(
    keymap: &Keymap,
    input: AcceleratorInput,
    origin: CommandOrigin,
) -> AcceleratorDecision {
    let pass = AcceleratorDecision {
        handled: false,
        event: None,
    };
    if !input.down {
        return pass;
    }
    let chord = Chord::of(input);
    if is_provider_origin(origin) && PROVIDER_DENYLIST.contains(&chord) {
        return pass;
    }
    let Some(key) = keymap.lookup(scope_chain(origin), chord) else {
        return pass;
    };
    AcceleratorDecision {
        handled: true,
        event: (!input.repeat).then_some(UserEvent::RunCommandKey { key, origin }),
    }
}

/// O mapa do produto: `COMMANDS` e a tabela dos arquivos. So se le (um
/// `RwLock` para os atalhos personalizados o poderem trocar depois). Uma
/// tabela que nao vira mapa -- o gate `keymap_chords_are_unique_per_scope`
/// nao a deixa chegar ao exe -- fica no log e deixa o mapa vazio.
static KEYMAP: LazyLock<RwLock<Keymap>> = LazyLock::new(|| {
    RwLock::new(Keymap::build(COMMANDS).unwrap_or_else(|error| {
        debug_log(format_args!(
            "keymap: tabela de comandos recusada ({error:?})"
        ));
        Keymap::default()
    }))
});

/// O mapa do produto, partilhado.
pub(in crate::windows_app) fn product_keymap() -> &'static RwLock<Keymap> {
    &KEYMAP
}

/// A decisao sobre o mapa do produto.
pub(in crate::windows_app) fn keymap_decision(
    input: AcceleratorInput,
    origin: CommandOrigin,
) -> AcceleratorDecision {
    keymap_decision_in(product_keymap(), input, origin)
}

/// A decisao sobre um mapa partilhado: uma leitura, sem nada mais preso
/// depois de ela sair.
pub(in crate::windows_app) fn keymap_decision_in(
    keymap: &RwLock<Keymap>,
    input: AcceleratorInput,
    origin: CommandOrigin,
) -> AcceleratorDecision {
    let keymap = keymap.read().unwrap_or_else(PoisonError::into_inner);
    accelerator_decision(&keymap, input, origin)
}

/// As teclas de funcao pela ordem das teclas virtuais (F1 = 0x70).
const FUNCTION_KEYS: [NamedKey; 24] = [
    NamedKey::F1,
    NamedKey::F2,
    NamedKey::F3,
    NamedKey::F4,
    NamedKey::F5,
    NamedKey::F6,
    NamedKey::F7,
    NamedKey::F8,
    NamedKey::F9,
    NamedKey::F10,
    NamedKey::F11,
    NamedKey::F12,
    NamedKey::F13,
    NamedKey::F14,
    NamedKey::F15,
    NamedKey::F16,
    NamedKey::F17,
    NamedKey::F18,
    NamedKey::F19,
    NamedKey::F20,
    NamedKey::F21,
    NamedKey::F22,
    NamedKey::F23,
    NamedKey::F24,
];

/// Uma descida de tecla da janela (winit) como a do `AcceleratorKeyPressed`:
/// a tecla virtual de uma letra ou digito (a letra, sem distinguir
/// maiusculas), de F1-F24, Delete, Backspace ou Tab. O resto (Esc, Enter,
/// setas...) nao e atalho do mapa e segue o caminho que ja tinha.
pub(in crate::windows_app) fn window_accelerator_input(
    key: &Key,
    modifiers: ModifiersState,
    repeat: bool,
) -> Option<AcceleratorInput> {
    let vk = match key {
        Key::Character(text) => {
            let mut chars = text.chars();
            let letter = chars.next()?;
            if chars.next().is_some() || !letter.is_ascii_alphanumeric() {
                return None;
            }
            u32::from(letter.to_ascii_uppercase())
        }
        Key::Named(NamedKey::Delete) => u32::from(VK_DELETE),
        Key::Named(NamedKey::Backspace) => u32::from(VK_BACK),
        Key::Named(NamedKey::Tab) => u32::from(VK_TAB),
        Key::Named(named) => {
            let index = FUNCTION_KEYS.iter().position(|key| key == named)?;
            u32::from(VK_F1) + index as u32
        }
        _ => return None,
    };
    Some(AcceleratorInput {
        vk,
        down: true,
        ctrl: modifiers.control_key(),
        shift: modifiers.shift_key(),
        alt: modifiers.alt_key(),
        repeat,
    })
}
