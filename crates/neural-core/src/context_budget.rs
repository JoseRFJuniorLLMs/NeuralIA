//! Orcamento de contexto (context-budget, plano 2.5; SPEC-0102). O que
//! entra num prompt e o que fica de fora decide-se aqui, antes de qualquer
//! chamada, sem I/O e sem rede: um modelo tem um limite de entrada, a
//! resposta precisa de espaco, e o que sobra reparte-se pelas fontes (a
//! pagina, o Leitor, um PDF, uma nota, a memoria...) sem nunca passar do
//! limite. E a peca que o consenso e as respostas com contexto usam depois.
//!
//! O caminho de `build_context`:
//!
//! 1. **sanitizar** (`untrusted::sanitize`): invisiveis, bidi e controlos
//!    fora; com destino `Remote` as linhas sensiveis passam pelo
//!    `redact_sensitive_text` que ja existe, e cada URL que leva uma
//!    credencial (parametro, fragmento ou `utilizador:senha@`) pelo
//!    `redact_url` -- o URL da fonte, os que estao no texto e os que estao
//!    no nome da fonte, que tambem passa pelo `redact_sensitive_text` (o
//!    titulo de uma aba sai da maquina no cabecalho). Uma fonte de uma
//!    sessao privada nunca vai para um destino remoto
//!    (`ContextError::PrivateContent`).
//! 2. **partir em trechos** de ~`chunk_tokens` (160 por omissao) em
//!    fronteiras de frase: `Dr. Silva` e `3.5 GHz` ficam inteiros
//!    (abreviaturas, iniciais e decimais nao fecham frase), e os terminais
//!    do CJK (`。！？`) fecham sem espaco a seguir. O corte de cada trecho
//!    acha-se por busca exponencial e bissecao: O(n log n) bytes medidos.
//! 3. **deduplicar**, pela ordem da pontuacao, para a copia que fica ser a
//!    mais pontuada (numa igualdade, a primeira): o SHA-256 do texto
//!    normalizado apanha as copias exactas; o SimHash das janelas
//!    (`untrusted::Shingles`) filtra as parecidas e o Jaccard >= 0.8
//!    confirma. Entre fontes diferentes, duas copias parecidas que divergem
//!    nos numeros ou nas negacoes (`baixou 12%` contra `subiu 40%`,
//!    `funciona` contra `nao funciona`) ficam as duas: a divergencia e o
//!    que o consenso tem de ver. Cada repeticao removida fica registada com
//!    a proveniencia (quem ficou, quem saiu, a semelhanca).
//! 4. **pontuar**: 0.5 x BM25-lite contra a pergunta + 0.4 x cosseno do
//!    `Embedder` + 0.1 x prioridade da fonte.
//! 5. **garantir o piso** de cada fonte (`floor_per_source`), por rondas:
//!    os melhores trechos de cada uma entram primeiro, com o que as outras
//!    ainda precisam reservado -- um trecho so entra inteiro se deixar esse
//!    espaco, e senao entra cortado (as frases mais pontuadas, ou o inicio
//!    do trecho). Quando os pisos e os cabecalhos cabem, cada fonte recebe
//!    pelo menos `min(piso, o seu texto)`; quando nao cabem, cada uma recebe
//!    o maior piso comum que cabe.
//! 6. **alocar** o resto por pontuacao, com **compressao extractiva**
//!    (as frases mais pontuadas de um trecho que ja nao cabe inteiro).
//! 7. **cercar** com o nonce do `untrusted` (`fence_lines`,
//!    `neutralize_inside`): o texto do pacote e dado, nunca instrucao. Uma
//!    linha de um trecho que comeca por um parentese recto (`[2] resposta:`,
//!    `［3］`) leva um `\` a frente: so os cabecalhos das fontes comecam
//!    assim, e o texto de uma pagina nao se faz passar por outra fonte.
//!
//! O prompt inteiro cabe em `spec.available()`: as instrucoes da cerca
//! (`ContextPack::fence_instructions`, uma mensagem) e a pergunta com o
//! pacote (`ContextPack::user_message`, outra), com o nonce contado ao peso
//! maximo.
//!
//! O `ContextPack` que sai tem campos privados: le-se (`rendered`,
//! `est_tokens`, `sources` com as posicoes em bytes -- no pacote e, de
//! cada trecho, no texto sanitizado da fonte --, `duplicates`, `dropped`)
//! e nao se monta nem se altera de fora -- o unico caminho para um pacote
//! e este.
//!
//! A contagem de tokens (`estimate_tokens`) e o maior entre uma tabela por
//! classe de caractere x 1,10 e o numero de pre-tokens das regex de
//! pre-tokenizacao publicadas do o200k e do Llama 3, mais 4 por mensagem.
//! Os pre-tokens de cada regex sao um limite inferior duro dos tokens do
//! seu tokenizador (o BPE nunca junta dois) e seguram o texto onde a
//! tabela sozinha contava de menos: letras e digitos alternados (`a1a1`,
//! ids hexadecimais, lances de xadrez), mudancas de maiuscula (`aBaB`),
//! linhas curtas. A estimativa fica em ou acima das duas colunas da fixture
//! (`context_budget/token_fixture.tsv`), mas NAO e uma garantia contra os
//! tokenizadores reais: dentro de um pre-token o BPE pode partir mais do
//! que a tabela conta (um base64 ou um id aleatorio longos). A
//! `TokenCalibration` (EWMA da razao real/estimada, presa a [0,6; 2,0])
//! encosta-a ao modelo em uso quando a API devolve a contagem real.
//! `fit_conversation` corta a conversa mais antiga; `split_for_map_reduce`
//! parte um texto longo em pedacos que cabem, um a um, com a cerca.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use sha2::{Digest, Sha256};

use crate::agent_security::redact_url;
use crate::local_intelligence::{Embedder, cosine_similarity};
use crate::untrusted::{self, FenceNonce, Shingles, UntrustedText};

// ------------------------------------------------------------ tokens

/// A margem sobre a tabela: a estimativa e 10% acima da soma dos pesos.
pub const TOKEN_SAFETY_FACTOR: f64 = 1.10;
/// O custo fixo de cada mensagem (papel, separadores do modelo de conversa).
pub const MESSAGE_OVERHEAD_TOKENS: usize = 4;

/// O peso de um caractere na tabela, por classe, pensado para o texto
/// corrido: cada palavra e pelo menos um token, por isso o espaco que a
/// antecede vale quase um (0,7) e as letras pouco (0,22) -- `a b c d` sao
/// quatro tokens, `processador` dois ou tres; um digito custa quase um
/// token (`\p{N}{1,3}`: `2026-09-26` sao seis); a pontuacao ASCII e quase
/// sempre um token cada; um espaco a seguir a outro e uma sequencia, com
/// custo; um emoji ou uma letra fora do BMP pode cair em bytes (ate 4
/// tokens); o CJK ate 3 por caractere. Onde as letras pesam pouco demais
/// (`a1a1`, `aBaB`, linhas curtas) quem segura a contagem e o numero de
/// pre-tokens (`pretoken_count`).
fn char_weight(c: char, after_space: bool) -> f64 {
    match c {
        ' ' if after_space => 0.6,
        ' ' => 0.7,
        '\n' => 1.0,
        '\t' => 0.7,
        c if c.is_ascii_alphabetic() => 0.22,
        c if c.is_ascii_digit() => 0.80,
        c if c.is_ascii() => 1.0,
        '\u{00A0}'..='\u{00BF}'
        | '\u{2000}'..='\u{206F}'
        | '\u{2190}'..='\u{2BFF}'
        | '\u{3000}'..='\u{303F}' => 1.2,
        '\u{00C0}'..='\u{024F}' => 0.60,
        c if c.is_whitespace() => 0.7,
        '\u{2E80}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}' => 2.8,
        '\u{FF00}'..='\u{FFEF}' => 2.8,
        c if (c as u32) > 0xFFFF => 3.7,
        _ => 0.75,
    }
}

/// A soma dos pesos da tabela em `text` (aditiva: a de `a + b` e a de `a`
/// mais a de `b`, com a ressalva de um espaco a seguir a outro, que nao
/// muda com a juncao porque as partes juntam-se com uma mudanca de linha
/// ou depois de um texto sem espacos no fim).
fn table_weight(text: &str) -> f64 {
    let mut total = 0.0;
    let mut after_space = false;
    for c in text.chars() {
        total += char_weight(c, after_space);
        after_space = c == ' ';
    }
    total
}

// ------------------------------------------------------------ pre-tokens
//
// Os tokenizadores BPE partem o texto em pre-tokens com uma regex e so
// depois juntam bytes dentro de cada um: um pre-token e pelo menos um
// token, e dois nunca se juntam. Contar os pre-tokens da um limite
// inferior duro. As duas regex publicadas, simuladas a mao (sem crate de
// regex), com as classes do Unicode aproximadas pelas da `std`: `\p{L}` e
// alfabetica, nao numerica e nao marca; `\p{M}` sao os blocos de marcas
// combinantes de `is_mark`; `\p{Lt}` (`ǅ`) e a letra com caixa que nao e
// maiuscula nem minuscula. Em 14 000 textos aleatorios, comparados com as
// proprias regex (latino, acentos soltos, CJK, hangul, kana, hebraico,
// arabe, `ǅ`, `ʰ`, emoji, digitos de outras escritas), a contagem foi a das
// regex em todos, fora os sinais vocalicos das escritas indicas (`\p{Mc}`,
// alfabeticos para a `std`), que aqui contam como letras: o Llama 3, que
// os parte, pode ter mais pre-tokens nessas escritas, onde a tabela ja
// pesa 0,75 x 1,10 por caractere.
//
// o200k_base:
//   [^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?
//   |[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?
//   |\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n/]*|\s*[\r\n]+|\s+(?!\S)|\s+
// Llama 3:
//   (?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}{1,3}
//   | ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+

/// `\p{M}` nos blocos de marcas combinantes: os acentos soltos
/// (U+0300-036F e as extensoes), os sinais do cirilico, do hebraico e do
/// arabe, as marcas do kana e os seletores de variante.
fn is_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05BD}'
            | '\u{05BF}'
            | '\u{05C1}'..='\u{05C2}'
            | '\u{05C4}'..='\u{05C5}'
            | '\u{05C7}'
            | '\u{0610}'..='\u{061A}'
            | '\u{064B}'..='\u{065F}'
            | '\u{0670}'
            | '\u{06D6}'..='\u{06DC}'
            | '\u{06DF}'..='\u{06E4}'
            | '\u{06E7}'..='\u{06E8}'
            | '\u{06EA}'..='\u{06ED}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{302A}'..='\u{302F}'
            | '\u{3099}'..='\u{309A}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// `\p{L}`.
fn is_letter(c: char) -> bool {
    c.is_alphabetic() && !c.is_numeric() && !is_mark(c)
}

/// `\p{Lt}`: uma letra com caixa que nao e maiuscula nem minuscula (`ǅ`).
fn is_titlecase(c: char) -> bool {
    is_letter(c) && !c.is_lowercase() && !c.is_uppercase() && c.to_lowercase().next() != Some(c)
}

/// Sem caixa: nem maiuscula nem minuscula tem outra forma (`ʰ`, `ª`, CJK).
fn is_caseless(c: char) -> bool {
    c.to_uppercase().next() == Some(c) && c.to_lowercase().next() == Some(c)
}

/// `[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]`: uma letra que nao e minuscula (as
/// modificadoras como `ʰ`, minusculas para a `std`, nao tem caixa e contam
/// dos dois lados), ou uma marca.
fn is_upper_side(c: char) -> bool {
    (is_letter(c) && (!c.is_lowercase() || is_caseless(c))) || is_mark(c)
}

/// `[\p{Ll}\p{Lm}\p{Lo}\p{M}]`: uma letra que nao e maiuscula nem de titulo,
/// ou uma marca.
fn is_lower_side(c: char) -> bool {
    (is_letter(c) && !c.is_uppercase() && !is_titlecase(c)) || is_mark(c)
}

fn is_line_break(c: char) -> bool {
    c == '\r' || c == '\n'
}

/// `[^\r\n\p{L}\p{N}]`: o que pode ir colado a frente de uma palavra.
fn is_word_lead(c: char) -> bool {
    !is_line_break(c) && !is_letter(c) && !c.is_numeric()
}

/// `[^\s\p{L}\p{N}]`: pontuacao e simbolos.
fn is_symbol(c: char) -> bool {
    !c.is_whitespace() && !is_letter(c) && !c.is_numeric()
}

/// `(?i:'s|'t|'re|'ve|'m|'ll|'d)` a comecar em `at`: onde acaba.
fn contraction_end(chars: &[char], at: usize) -> Option<usize> {
    if chars.get(at) != Some(&'\'') {
        return None;
    }
    let lower = |offset: usize| chars.get(at + offset).map(|c| c.to_ascii_lowercase());
    match (lower(1), lower(2)) {
        (Some('s' | 't' | 'm' | 'd'), _) => Some(at + 2),
        (Some('r' | 'v'), Some('e')) | (Some('l'), Some('l')) => Some(at + 3),
        _ => None,
    }
}

/// `\p{N}{1,3}`.
fn digits_end(chars: &[char], at: usize) -> usize {
    let mut end = at;
    while end < chars.len() && end - at < 3 && chars[end].is_numeric() {
        end += 1;
    }
    end
}

/// ` ?[^\s\p{L}\p{N}]+` seguido de `[\r\n/]*` (o200k) ou `[\r\n]*` (Llama 3).
fn symbol_run_end(chars: &[char], at: usize, slash_tail: bool) -> Option<usize> {
    let start = if chars[at] == ' ' && chars.get(at + 1).is_some_and(|c| is_symbol(*c)) {
        at + 1
    } else {
        at
    };
    if !is_symbol(chars[start]) {
        return None;
    }
    let mut end = start;
    while end < chars.len() && is_symbol(chars[end]) {
        end += 1;
    }
    while end < chars.len() && (is_line_break(chars[end]) || (slash_tail && chars[end] == '/')) {
        end += 1;
    }
    Some(end)
}

/// `\s*[\r\n]+|\s+(?!\S)|\s+`, a comecar num espaco em `at`.
fn whitespace_end(chars: &[char], at: usize) -> usize {
    let mut run = at;
    while run < chars.len() && chars[run].is_whitespace() {
        run += 1;
    }
    if let Some(last) = (at..run).rev().find(|&index| is_line_break(chars[index])) {
        return last + 1;
    }
    if run == chars.len() || run - at < 2 {
        return run;
    }
    // O ultimo espaco vai com a palavra que se segue.
    run - 1
}

fn upper_run_end(chars: &[char], start: usize) -> usize {
    let mut end = start;
    while end < chars.len() && is_upper_side(chars[end]) {
        end += 1;
    }
    end
}

/// `U*L+` a partir de `start`: o `U*` mais longo que deixa um `L` a seguir,
/// e o `L+` inteiro.
fn upper_then_lower_end(chars: &[char], start: usize) -> Option<usize> {
    let split = (start..=upper_run_end(chars, start))
        .rev()
        .find(|&split| chars.get(split).is_some_and(|c| is_lower_side(*c)))?;
    let mut end = split;
    while end < chars.len() && is_lower_side(chars[end]) {
        end += 1;
    }
    Some(end)
}

/// O pre-token do o200k que comeca em `at`: onde acaba.
fn o200k_next(chars: &[char], at: usize) -> usize {
    // `P?`: primeiro com o caractere de `at` colado a frente, depois sem ele
    // (uma marca e as duas coisas); a primeira alternativa, `U*L+`, e
    // tentada das duas formas antes da segunda, `U+L*`.
    let starts = [is_word_lead(chars[at]).then_some(at + 1), Some(at)];
    let word = starts
        .iter()
        .flatten()
        .find_map(|&start| upper_then_lower_end(chars, start))
        .or_else(|| {
            starts
                .iter()
                .flatten()
                .find(|&&start| chars.get(start).is_some_and(|c| is_upper_side(*c)))
                .map(|&start| upper_run_end(chars, start))
        });
    if let Some(end) = word {
        return contraction_end(chars, end).unwrap_or(end);
    }
    if chars[at].is_numeric() {
        return digits_end(chars, at);
    }
    symbol_run_end(chars, at, true).unwrap_or_else(|| whitespace_end(chars, at))
}

/// O pre-token do Llama 3 que comeca em `at`: onde acaba.
fn llama3_next(chars: &[char], at: usize) -> usize {
    if let Some(end) = contraction_end(chars, at) {
        return end;
    }
    let start = if is_word_lead(chars[at]) { at + 1 } else { at };
    if chars.get(start).is_some_and(|c| is_letter(*c)) {
        let mut end = start;
        while end < chars.len() && is_letter(chars[end]) {
            end += 1;
        }
        return end;
    }
    if chars[at].is_numeric() {
        return digits_end(chars, at);
    }
    symbol_run_end(chars, at, false).unwrap_or_else(|| whitespace_end(chars, at))
}

fn count_pretokens(chars: &[char], next: fn(&[char], usize) -> usize) -> usize {
    let mut count = 0;
    let mut at = 0;
    while at < chars.len() {
        at = next(chars, at).max(at + 1);
        count += 1;
    }
    count
}

/// O maior dos dois numeros de pre-tokens de `text`, o do o200k e o do
/// Llama 3. Cada um e um limite inferior dos tokens do seu tokenizador; a
/// estimativa, que nunca fica abaixo deste numero, nunca fica abaixo de
/// nenhum dos dois.
pub fn pretoken_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut stack_chars = ['\0'; 256];
    let mut len = 0;
    let mut iter = text.chars();
    for c in iter.by_ref() {
        if len < stack_chars.len() {
            stack_chars[len] = c;
            len += 1;
        } else {
            let mut heap_chars = Vec::with_capacity(len + iter.size_hint().0 + 1);
            heap_chars.extend_from_slice(&stack_chars);
            heap_chars.push(c);
            heap_chars.extend(iter);
            return count_pretokens(&heap_chars, o200k_next)
                .max(count_pretokens(&heap_chars, llama3_next));
        }
    }
    count_pretokens(&stack_chars[..len], o200k_next)
        .max(count_pretokens(&stack_chars[..len], llama3_next))
}

/// O custo de `text` em tokens, antes de arredondar e de calibrar: o maior
/// entre a tabela x 1,10 e o numero de pre-tokens. Subaditivo nas juncoes
/// que o pacote usa (uma mudanca de linha, ou um espaco depois de um texto
/// sem espacos no fim): a tabela soma-se e os pre-tokens de `a`, da juncao
/// e de `b` nunca sao menos do que os do texto junto. E isso que deixa a
/// alocacao contar por partes e garantir a contagem do texto final.
fn weight(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    (table_weight(text) * TOKEN_SAFETY_FACTOR).max(pretoken_count(text) as f64)
}

/// `weight` de um texto que leva o nonce da cerca, ao peso maximo de
/// qualquer nonce do mesmo tamanho: a tabela com o nonce so de digitos (o
/// caractere hexadecimal mais pesado) e os pre-tokens com digitos e letras
/// alternados (um pre-token por caractere, o maximo; comeca num digito,
/// que nao se cola ao `=` de antes, e acaba numa letra). Nao depende do
/// sorteio e fica em ou acima do peso do texto com o nonce verdadeiro.
fn weight_with_nonce(text: &str, nonce: &FenceNonce) -> f64 {
    let nonce = nonce.as_str();
    if nonce.is_empty() || !text.contains(nonce) {
        return weight(text);
    }
    let digits = "9".repeat(nonce.len());
    let alternating: String = (0..nonce.len())
        .map(|index| if index % 2 == 0 { '9' } else { 'a' })
        .collect();
    let table = table_weight(&text.replace(nonce, &digits)) * TOKEN_SAFETY_FACTOR;
    table.max(pretoken_count(&text.replace(nonce, &alternating)) as f64)
}

fn tokens_from_weight(weight: f64) -> usize {
    if weight <= 0.0 {
        return 0;
    }
    weight.ceil() as usize
}

/// Os tokens de um texto: o maior entre a tabela por classe de caractere
/// x 1,10 (arredondada para cima) e o numero de pre-tokens. Zero para o
/// texto vazio.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    tokens_from_weight(weight(text))
}

/// Os tokens de uma mensagem: o texto mais `MESSAGE_OVERHEAD_TOKENS`.
pub fn estimate_message_tokens(text: &str) -> usize {
    estimate_tokens(text) + MESSAGE_OVERHEAD_TOKENS
}

/// O menor factor de calibracao.
pub const CALIBRATION_MIN: f64 = 0.6;
/// O maior factor de calibracao.
pub const CALIBRATION_MAX: f64 = 2.0;
/// O peso de cada observacao nova na media movel.
pub const CALIBRATION_ALPHA: f64 = 0.25;

/// A razao entre os tokens que a API contou e os que se estimaram, como
/// media movel exponencial presa a [0,6; 2,0]. Comeca em 1,0; a primeira
/// observacao substitui-a, as seguintes pesam `CALIBRATION_ALPHA`. Uma
/// observacao com zero de um dos lados e ignorada.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenCalibration {
    factor: f64,
    samples: u32,
}

impl Default for TokenCalibration {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCalibration {
    pub fn new() -> Self {
        Self {
            factor: 1.0,
            samples: 0,
        }
    }

    /// O factor actual, dentro de [`CALIBRATION_MIN`, `CALIBRATION_MAX`].
    pub fn factor(&self) -> f64 {
        self.factor
    }

    /// Quantas observacoes entraram.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Uma contagem real da API contra a estimativa que a precedeu.
    pub fn observe(&mut self, estimated: usize, actual: usize) {
        if estimated == 0 || actual == 0 {
            return;
        }
        let ratio = (actual as f64 / estimated as f64).clamp(CALIBRATION_MIN, CALIBRATION_MAX);
        self.factor = if self.samples == 0 {
            ratio
        } else {
            CALIBRATION_ALPHA * ratio + (1.0 - CALIBRATION_ALPHA) * self.factor
        };
        self.factor = self.factor.clamp(CALIBRATION_MIN, CALIBRATION_MAX);
        self.samples = self.samples.saturating_add(1);
    }

    /// Uma estimativa corrigida pelo factor, arredondada para cima.
    pub fn apply(&self, estimated: usize) -> usize {
        if estimated == 0 {
            return 0;
        }
        (estimated as f64 * self.factor).ceil() as usize
    }
}

// ------------------------------------------------------------ fontes

/// Para onde vai o prompt.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Destination {
    /// O modelo corre neste computador.
    Local,
    /// O texto sai da maquina para `host`: leva a redacao das linhas
    /// sensiveis e recusa fontes de sessoes privadas.
    Remote { host: String },
}

impl Destination {
    pub fn remote(host: impl Into<String>) -> Self {
        Self::Remote { host: host.into() }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// O destino da cerca: e daqui que a redacao remota depende.
    fn fence(&self) -> untrusted::Destination {
        match self {
            Self::Local => untrusted::Destination::Local,
            Self::Remote { .. } => untrusted::Destination::Remote,
        }
    }
}

/// O tamanho maximo de um `SourceId`.
pub const SOURCE_ID_MAX_LEN: usize = 16;

/// O id de uma fonte: `[A-Za-z0-9_-]{1,16}`. Vai na proveniencia e na
/// linha de cabecalho de cada fonte dentro da cerca.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(String);

impl SourceId {
    pub fn parse(raw: &str) -> Option<Self> {
        let valid = (1..=SOURCE_ID_MAX_LEN).contains(&raw.len())
            && raw
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        valid.then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// De onde vem uma fonte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Answer,
    Page,
    Reader,
    Pdf,
    Epub,
    Transcript,
    Note,
    Memory,
    Selection,
    ChatTurn,
    Tab,
}

impl SourceKind {
    /// O nome no cabecalho da fonte, em pt-BR.
    pub fn label_pt(self) -> &'static str {
        match self {
            Self::Answer => "resposta",
            Self::Page => "página",
            Self::Reader => "leitor",
            Self::Pdf => "PDF",
            Self::Epub => "livro",
            Self::Transcript => "transcrição",
            Self::Note => "nota",
            Self::Memory => "memória",
            Self::Selection => "seleção",
            Self::ChatTurn => "conversa",
            Self::Tab => "aba",
        }
    }
}

/// Onde, dentro da fonte, o texto esta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locator {
    /// Um bloco do Leitor ou de uma pagina.
    Block(u32),
    /// Uma pagina de um PDF (a partir de 1).
    PdfPage(u32),
    /// Um item da espinha de um EPUB e um bloco dentro dele.
    Epub { spine: u32, block: u32 },
    /// Um instante de uma transcricao, em segundos.
    Time(u32),
}

impl Locator {
    pub fn label_pt(&self) -> String {
        match self {
            Self::Block(block) => format!("bloco {block}"),
            Self::PdfPage(page) => format!("p. {page}"),
            Self::Epub { spine, block } => format!("cap. {spine}, bloco {block}"),
            Self::Time(secs) => {
                let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
                if hours > 0 {
                    format!("{hours:02}:{minutes:02}:{seconds:02}")
                } else {
                    format!("{minutes:02}:{seconds:02}")
                }
            }
        }
    }
}

/// A prioridade mais alta de uma fonte.
pub const PRIORITY_MAX: u8 = 100;
/// A prioridade por omissao.
pub const PRIORITY_DEFAULT: u8 = 50;

/// Uma fonte de contexto: o texto de fora (`UntrustedText`, so sai
/// sanitizado) com o que o pacote precisa para o citar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSource {
    id: SourceId,
    label: String,
    kind: SourceKind,
    locator: Option<Locator>,
    url: Option<String>,
    text: UntrustedText,
    priority: u8,
    private: bool,
}

impl ContextSource {
    /// Uma fonte com o id (`[A-Za-z0-9_-]{1,16}`), o nome que aparece no
    /// cabecalho, a origem e o texto.
    pub fn new(
        id: &str,
        label: impl Into<String>,
        kind: SourceKind,
        text: impl Into<String>,
    ) -> Result<Self, ContextError> {
        let id =
            SourceId::parse(id).ok_or_else(|| ContextError::InvalidSourceId(id.to_string()))?;
        Ok(Self {
            id,
            label: clean_line(&label.into(), 120),
            kind,
            locator: None,
            url: None,
            text: UntrustedText::new(text),
            priority: PRIORITY_DEFAULT,
            private: false,
        })
    }

    pub fn with_locator(mut self, locator: Locator) -> Self {
        self.locator = Some(locator);
        self
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        let url = clean_line(&url.into(), 512);
        self.url = (!url.is_empty()).then_some(url);
        self
    }

    /// 0 a 100; acima de 100 fica 100.
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.min(PRIORITY_MAX);
        self
    }

    /// Uma fonte de uma aba ou sessao privada: nunca sai da maquina.
    pub fn with_private(mut self, private: bool) -> Self {
        self.private = private;
        self
    }

    pub fn id(&self) -> &SourceId {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    pub fn locator(&self) -> Option<Locator> {
        self.locator
    }

    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn priority(&self) -> u8 {
        self.priority
    }

    pub fn is_private(&self) -> bool {
        self.private
    }

    /// O tamanho do texto cru, em bytes.
    pub fn text_len(&self) -> usize {
        self.text.len()
    }
}

/// Uma linha so, sem invisiveis nem controlos, ate `max` caracteres.
fn clean_line(raw: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in untrusted::strip_invisible(raw).chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
        if out.chars().count() >= max {
            break;
        }
    }
    out
}

// ------------------------------------------------------------ orcamento

/// O piso por fonte por omissao, em tokens.
pub const DEFAULT_FLOOR_TOKENS: usize = 96;
/// O tamanho dos trechos por omissao, em tokens.
pub const DEFAULT_CHUNK_TOKENS: usize = 160;
/// O menor trecho que vale a pena mandar.
pub const MIN_PASSAGE_TOKENS: usize = 16;
const MIN_CHUNK_TOKENS: usize = 16;

/// O orcamento de uma chamada.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetSpec {
    max_input: usize,
    reserve_output: usize,
    floor_per_source: usize,
    chunk_tokens: usize,
    destination: Destination,
    calibration: TokenCalibration,
}

impl BudgetSpec {
    /// `max_input` e o limite de entrada do modelo; `reserve_output` fica
    /// livre para a resposta e para as instrucoes da tarefa, se as houver
    /// (texto do codigo, como a instrucao do mapa). As instrucoes da cerca,
    /// a pergunta e o pacote contam-se no resto (`available`).
    pub fn new(max_input: usize, reserve_output: usize, destination: Destination) -> Self {
        Self {
            max_input,
            reserve_output,
            floor_per_source: DEFAULT_FLOOR_TOKENS,
            chunk_tokens: DEFAULT_CHUNK_TOKENS,
            destination,
            calibration: TokenCalibration::new(),
        }
    }

    /// Os tokens que cada fonte com conteudo recebe antes da alocacao por
    /// pontuacao (0 desliga o piso): pelo menos `min(piso, o seu texto)`
    /// quando os pisos de todas, com os cabecalhos, cabem; senao, o maior
    /// piso comum que cabe.
    pub fn with_floor_per_source(mut self, tokens: usize) -> Self {
        self.floor_per_source = tokens;
        self
    }

    /// O tamanho dos trechos (pelo menos 16).
    pub fn with_chunk_tokens(mut self, tokens: usize) -> Self {
        self.chunk_tokens = tokens.max(MIN_CHUNK_TOKENS);
        self
    }

    pub fn with_calibration(mut self, calibration: TokenCalibration) -> Self {
        self.calibration = calibration;
        self
    }

    pub fn max_input(&self) -> usize {
        self.max_input
    }

    pub fn reserve_output(&self) -> usize {
        self.reserve_output
    }

    pub fn floor_per_source(&self) -> usize {
        self.floor_per_source
    }

    pub fn chunk_tokens(&self) -> usize {
        self.chunk_tokens
    }

    pub fn destination(&self) -> &Destination {
        &self.destination
    }

    pub fn calibration(&self) -> TokenCalibration {
        self.calibration
    }

    /// O que a entrada pode ocupar: o limite menos a reserva da resposta.
    pub fn available(&self) -> usize {
        self.max_input.saturating_sub(self.reserve_output)
    }

    /// Os tokens de um texto, calibrados.
    pub fn tokens(&self, text: &str) -> usize {
        self.calibration.apply(estimate_tokens(text))
    }

    /// Os tokens de uma mensagem, calibrados, com o custo fixo.
    pub fn message_tokens(&self, text: &str) -> usize {
        self.tokens(text) + MESSAGE_OVERHEAD_TOKENS
    }

    /// Os tokens da mensagem com as instrucoes da cerca
    /// (`ContextPack::fence_instructions`), calibrados, com o nonce ao peso
    /// maximo: o mesmo numero para qualquer nonce.
    pub fn fence_instructions_tokens(&self) -> usize {
        let nonce = FenceNonce::fresh();
        self.tokens_of_weight(weight_with_nonce(&fence_instructions_text(&nonce), &nonce))
            + MESSAGE_OVERHEAD_TOKENS
    }

    /// O que sobra para o pacote em `available()` depois das instrucoes da
    /// cerca (uma mensagem), da pergunta (outra, a do utilizador) e da linha
    /// em branco que a separa do pacote. `None` quando nem isto cabe.
    pub fn pack_budget(&self, question: &str) -> Option<usize> {
        self.available()
            .checked_sub(self.fence_instructions_tokens())?
            .checked_sub(self.message_tokens(question))?
            .checked_sub(self.tokens(QUESTION_SEPARATOR))
    }

    fn tokens_of_weight(&self, weight: f64) -> usize {
        self.calibration.apply(tokens_from_weight(weight))
    }
}

// ------------------------------------------------------------ erros

/// O que impede um pacote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextError {
    /// Nenhuma fonte tem texto depois da limpeza.
    EmptySources,
    /// O limite do modelo nao chega para a pergunta e um trecho.
    ModelTooSmall { limit: usize },
    /// Uma fonte privada com destino remoto.
    PrivateContent,
    /// Um id fora de `[A-Za-z0-9_-]{1,16}`.
    InvalidSourceId(String),
    /// Duas fontes com o mesmo id.
    DuplicateSourceId(String),
}

impl fmt::Display for ContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySources => f.write_str("Nada para enviar: as fontes estão vazias."),
            Self::ModelTooSmall { limit } => write!(
                f,
                "O limite deste modelo ({} tokens) é pequeno demais para esta pergunta; escolha um modelo com mais contexto.",
                group_thousands(*limit)
            ),
            Self::PrivateContent => {
                f.write_str("Modo privado: este conteúdo não pode sair do computador.")
            }
            Self::InvalidSourceId(id) => write!(f, "id de fonte inválido: {id:?}"),
            Self::DuplicateSourceId(id) => write!(f, "id de fonte repetido: {id}"),
        }
    }
}

impl std::error::Error for ContextError {}

/// `3200` como `3 200` (grupos de tres, separados por espaco).
pub fn group_thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// A linha do resumo: «≈ 3 200 tokens · 4 fontes · 2 trechos repetidos
/// removidos». Sem repeticoes a ultima parte nao aparece.
pub fn summary_line_pt(tokens: usize, sources: usize, duplicates: usize) -> String {
    let mut line = format!("≈ {} tokens · ", group_thousands(tokens));
    if sources == 1 {
        line.push_str("1 fonte");
    } else {
        line.push_str(&format!("{sources} fontes"));
    }
    match duplicates {
        0 => {}
        1 => line.push_str(" · 1 trecho repetido removido"),
        n => line.push_str(&format!(" · {n} trechos repetidos removidos")),
    }
    line
}

// ------------------------------------------------------------ frases

const TERMINALS: &[char] = &['.', '!', '?', '…'];
/// Os terminais do CJK (`。`, `！`, `？`, o `．` largo e o `｡` estreito):
/// fecham a frase sem espaco a seguir, porque o chines e o japones nao
/// separam as frases com espacos (CB-10).
const WIDE_TERMINALS: &[char] = &['。', '！', '？', '．', '｡'];
const CLOSERS: &[char] = &[
    '"', '\'', '”', '’', '»', ')', ']', '}', '」', '』', '）', '］', '】', '〕', '〗', '〙', '〛',
    '｣', '＂', '＇',
];

fn is_terminal(c: char) -> bool {
    TERMINALS.contains(&c) || WIDE_TERMINALS.contains(&c)
}

/// Abreviaturas (sem o ponto, em minusculas) que nao fecham uma frase.
const ABBREVIATIONS: &[&str] = &[
    "dr", "dra", "drs", "sr", "sra", "srs", "sras", "srta", "prof", "profa", "profs", "eng",
    "enga", "exmo", "exma", "ilmo", "ilma", "av", "al", "rod", "ltda", "cia", "tel", "cel", "fig",
    "figs", "cap", "caps", "art", "arts", "pág", "pag", "págs", "pags", "pp", "vol", "vols", "ed",
    "eds", "ref", "refs", "no", "nº", "num", "obs", "aprox", "séc", "sec", "min", "máx", "max",
    "jan", "fev", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez", "cf", "op", "cit",
    "apud", "dep", "sen", "gov", "pres", "adv", "rel", "res", "proc", "esp", "esq", "dir", "fl",
    "fls", "inc", "mr", "mrs", "ms", "st", "jr", "ltd", "co", "corp", "vs", "mt", "ft", "gen",
    "col", "capt", "lt", "sgt", "rev", "hon", "rep", "dept", "univ", "assn", "bros", "approx",
    "est", "feb", "mar", "apr", "aug", "sep", "sept", "oct", "dec",
];

fn is_abbreviation(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    // Uma inicial ("J. R. R. Tolkien").
    if chars.next().is_none() && first.is_alphabetic() {
        return true;
    }
    // "e.g", "S.A", "U.S": ponto por dentro e letras.
    if token.contains('.') && token.chars().any(char::is_alphabetic) {
        return true;
    }
    // O numero de uma lista ("1. Introdução").
    if token.len() <= 2 && token.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    ABBREVIATIONS.contains(&token.to_lowercase().as_str())
}

/// A palavra que acaba em `at` (letras, digitos e pontos por dentro).
fn token_before(text: &str, at: usize) -> &str {
    let mut start = at;
    for (index, c) in text[..at].char_indices().rev() {
        if c.is_alphanumeric() || c == '.' {
            start = index;
        } else {
            break;
        }
    }
    text[start..at].trim_matches('.')
}

/// As frases de `text`, como intervalos de bytes sem espacos nas pontas.
/// Fecham uma frase: uma mudanca de linha, ou `.`, `!`, `?`, `…` (com os
/// fechos `"»)]` colados) seguidos de espaco e de algo que nao e minuscula
/// -- desde que a palavra antes do ponto nao seja uma abreviatura, uma
/// inicial ou o numero de uma lista. Um ponto entre digitos (`3.5`) nunca
/// e seguido de espaco, e por isso nunca fecha. Os terminais do CJK (`。`,
/// `！`, `？`, `．`, `｡`, com os fechos `」』）` colados) fecham mesmo sem
/// espaco a seguir; o `．` entre digitos (`３．５`) nao.
pub fn sentence_spans(text: &str) -> Vec<Range<usize>> {
    fn push(spans: &mut Vec<Range<usize>>, text: &str, start: usize, end: usize) {
        let trimmed = text[start..end].trim_end();
        if !trimmed.is_empty() {
            spans.push(start..start + trimmed.len());
        }
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        if c == '\n' {
            if let Some(from) = start.take() {
                push(&mut spans, text, from, at);
            }
            i += 1;
            continue;
        }
        if start.is_none() {
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            start = Some(at);
        }
        if is_terminal(c) {
            let mut j = i + 1;
            while j < chars.len() && (is_terminal(chars[j].1) || CLOSERS.contains(&chars[j].1)) {
                j += 1;
            }
            let end = chars.get(j).map_or(text.len(), |(byte, _)| *byte);
            let at_end = j >= chars.len();
            if chars[i..j].iter().any(|(_, c)| WIDE_TERMINALS.contains(c)) {
                let decimal = c == '．'
                    && j == i + 1
                    && i > 0
                    && chars[i - 1].1.is_numeric()
                    && chars.get(j).is_some_and(|(_, next)| next.is_numeric());
                if !decimal && let Some(from) = start.take() {
                    push(&mut spans, text, from, end);
                }
            } else if at_end || chars[j].1.is_whitespace() {
                let next = chars[j..]
                    .iter()
                    .map(|(_, c)| *c)
                    .find(|c| !c.is_whitespace());
                let continues = next.is_some_and(char::is_lowercase);
                let ellipsis = i + 1 < j && TERMINALS.contains(&chars[i + 1].1);
                let abbreviation = c == '.' && !ellipsis && is_abbreviation(token_before(text, at));
                if !continues
                    && !abbreviation
                    && let Some(from) = start.take()
                {
                    push(&mut spans, text, from, end);
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    if let Some(from) = start {
        push(&mut spans, text, from, text.len());
    }
    spans
}

/// Os trechos de `text`: frases inteiras juntas ate `limit` tokens (pelo
/// `tokens`); uma frase maior que o limite e partida em palavras e, se uma
/// palavra ainda for maior, em caracteres. Cada corte acha-se por busca
/// exponencial e bissecao (`pack_units`): O(n log n) bytes medidos, e nao
/// o trecho aberto medido de novo a cada frase, palavra ou caractere, que
/// era O(n x janela) (CB-9).
pub fn chunk_spans(text: &str, limit: usize, tokens: &dyn Fn(&str) -> usize) -> Vec<Range<usize>> {
    let limit = limit.max(1);
    let mut chunks = Vec::new();
    let sentences = sentence_spans(text);
    let mut run = 0;
    for (index, sentence) in sentences.iter().enumerate() {
        if measure(tokens, &text[sentence.clone()]) > limit {
            pack_units(text, &sentences[run..index], limit, tokens, &mut chunks);
            hard_split(text, sentence.clone(), limit, tokens, &mut chunks);
            run = index + 1;
        }
    }
    pack_units(text, &sentences[run..], limit, tokens, &mut chunks);
    chunks
}

/// Os tokens de `piece` pelo `tokens` de quem parte o texto. E aqui que os
/// testes contam o trabalho da particao (os bytes medidos).
fn measure(tokens: &dyn Fn(&str) -> usize, piece: &str) -> usize {
    #[cfg(test)]
    tests::count_measured(piece.len());
    tokens(piece)
}

/// Junta as `units` (intervalos de `text`, pela ordem) em trechos: cada um
/// vai do inicio de uma unidade ao fim da ultima que ainda cabe em `limit`,
/// e leva sempre pelo menos uma. O corte acha-se por busca exponencial (1,
/// 2, 4... unidades a mais) ate a primeira medicao que nao cabe, e depois
/// por bissecao: um trecho de k unidades custa O(log k) medicoes de ate 2k
/// unidades. E o corte da juncao unidade a unidade sempre que a contagem so
/// cresce quando o trecho cresce -- a do `estimate_tokens` cresce nas
/// juncoes da particao (espacos e terminais: a tabela so soma, e os
/// pre-tokens do texto de antes da juncao ficam). Numa contagem que desce
/// (dentro de uma palavra, uma mudanca de caixa que o o200k junta), o corte
/// cai noutra unidade, mas cada trecho continua medido e dentro do limite.
fn pack_units(
    text: &str,
    units: &[Range<usize>],
    limit: usize,
    tokens: &dyn Fn(&str) -> usize,
    out: &mut Vec<Range<usize>>,
) {
    let mut first = 0;
    while first < units.len() {
        let start = units[first].start;
        let fits = |last: usize| measure(tokens, &text[start..units[last].end]) <= limit;
        // `good` e a ultima unidade que cabe (a primeira entra sempre);
        // `bad`, a primeira que nao cabe, ou o fim.
        let mut good = first;
        let mut bad = units.len();
        let mut step = 1;
        while good + 1 < bad {
            let probe = (good + step).min(bad - 1);
            if fits(probe) {
                good = probe;
                step = step.saturating_mul(2);
            } else {
                bad = probe;
                break;
            }
        }
        while good + 1 < bad {
            let middle = good + (bad - good) / 2;
            if fits(middle) {
                good = middle;
            } else {
                bad = middle;
            }
        }
        out.push(start..units[good].end);
        first = good + 1;
    }
}

/// Uma frase maior que o limite: palavras inteiras ate `limit`, e uma
/// palavra maior que o limite em pedacos de caracteres.
fn hard_split(
    text: &str,
    span: Range<usize>,
    limit: usize,
    tokens: &dyn Fn(&str) -> usize,
    out: &mut Vec<Range<usize>>,
) {
    let slice = &text[span.clone()];
    let mut words: Vec<Range<usize>> = Vec::new();
    let mut word_start: Option<usize> = None;
    for (index, c) in slice.char_indices() {
        if c.is_whitespace() {
            if let Some(start) = word_start.take() {
                words.push(span.start + start..span.start + index);
            }
        } else if word_start.is_none() {
            word_start = Some(index);
        }
    }
    if let Some(start) = word_start {
        words.push(span.start + start..span.end);
    }
    let mut run = 0;
    for (index, word) in words.iter().enumerate() {
        if measure(tokens, &text[word.clone()]) > limit {
            pack_units(text, &words[run..index], limit, tokens, out);
            split_word(text, word.clone(), limit, tokens, out);
            run = index + 1;
        }
    }
    pack_units(text, &words[run..], limit, tokens, out);
}

/// Uma palavra maior que o limite, em pedacos de caracteres inteiros.
fn split_word(
    text: &str,
    span: Range<usize>,
    limit: usize,
    tokens: &dyn Fn(&str) -> usize,
    out: &mut Vec<Range<usize>>,
) {
    let mut stack_chars: [Range<usize>; 64] = std::array::from_fn(|_| 0..0);
    let mut len = 0;
    let mut iter = text[span.clone()].char_indices();
    for (index, c) in iter.by_ref() {
        let range = span.start + index..span.start + index + c.len_utf8();
        if len < stack_chars.len() {
            stack_chars[len] = range;
            len += 1;
        } else {
            let mut heap_chars = Vec::with_capacity(len + iter.size_hint().0 + 1);
            heap_chars.extend_from_slice(&stack_chars);
            heap_chars.push(range);
            for (idx, ch) in iter {
                heap_chars.push(span.start + idx..span.start + idx + ch.len_utf8());
            }
            pack_units(text, &heap_chars, limit, tokens, out);
            return;
        }
    }
    pack_units(text, &stack_chars[..len], limit, tokens, out);
}

// ------------------------------------------------------------ pontuacao

/// Palavras vazias, ja sem acentos (as palavras comparadas tambem o sao).
/// Ordenadas alfabeticamente para permitir busca binaria O(log n).
const STOPWORDS: &[&str] = &[
    "an", "and", "ao", "aos", "are", "as", "at", "ate", "be", "by", "com", "como", "da", "das",
    "de", "depois", "did", "do", "does", "dos", "ela", "elas", "ele", "eles", "em", "entre",
    "era", "essa", "esse", "esta", "estao", "este", "eu", "foi", "for", "foram", "from", "ha",
    "how", "in", "into", "is", "isso", "isto", "it", "its", "ja", "mais", "mas", "me", "mesmo",
    "meu", "minha", "muito", "na", "nao", "nas", "nem", "no", "nos", "not", "num", "numa", "of",
    "on", "or", "os", "ou", "para", "pela", "pelas", "pelo", "pelos", "por", "quais", "qual",
    "quando", "quanto", "que", "quem", "se", "sem", "ser", "seu", "seus", "so", "sobre", "sua",
    "suas", "tambem", "tem", "ter", "than", "that", "the", "their", "then", "there", "these",
    "this", "those", "tinha", "to", "um", "uma", "umas", "uns", "voce", "was", "were", "what",
    "which", "with",
];

fn fold_char(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'ê' | 'è' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

/// As palavras de um texto para o BM25: minusculas sem acentos, sem as
/// vazias da lista, com pelo menos duas letras ou digitos.
fn terms(text: &str) -> Vec<String> {
    let folded: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold_char)
        .collect();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 2)
        .filter(|word| STOPWORDS.binary_search(word).is_err())
        .map(str::to_string)
        .collect()
}

/// BM25 sem o corpus de fora: o IDF vem dos proprios trechos da chamada.
struct Bm25 {
    idf: BTreeMap<String, f64>,
    average_len: f64,
}

impl Bm25 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    fn new(documents: &[Vec<String>]) -> Self {
        let total = documents.len() as f64;
        let mut frequency: BTreeMap<&str, usize> = BTreeMap::new();
        for document in documents {
            let mut seen = std::collections::BTreeSet::new();
            for term in document {
                if seen.insert(term.as_str()) {
                    *frequency.entry(term.as_str()).or_insert(0) += 1;
                }
            }
        }
        let idf = frequency
            .into_iter()
            .map(|(term, n)| {
                let n = n as f64;
                (term.to_string(), ((total - n + 0.5) / (n + 0.5) + 1.0).ln())
            })
            .collect();
        let average_len = if documents.is_empty() {
            1.0
        } else {
            (documents.iter().map(Vec::len).sum::<usize>() as f64 / total).max(1.0)
        };
        Self { idf, average_len }
    }

    fn score(&self, query: &[String], document: &[String]) -> f64 {
        if query.is_empty() || document.is_empty() {
            return 0.0;
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for term in document {
            *counts.entry(term.as_str()).or_insert(0) += 1;
        }
        let length = document.len() as f64 / self.average_len;
        let mut score = 0.0;
        let mut seen = std::collections::BTreeSet::new();
        for term in query {
            if !seen.insert(term.as_str()) {
                continue;
            }
            let Some(&count) = counts.get(term.as_str()) else {
                continue;
            };
            let idf = self.idf.get(term.as_str()).copied().unwrap_or(0.0);
            let tf = count as f64;
            score += idf * (tf * (Self::K1 + 1.0))
                / (tf + Self::K1 * (1.0 - Self::B + Self::B * length));
        }
        score
    }
}

/// O peso de cada componente da pontuacao.
pub const SCORE_WEIGHT_BM25: f64 = 0.5;
pub const SCORE_WEIGHT_COSINE: f64 = 0.4;
pub const SCORE_WEIGHT_PRIORITY: f64 = 0.1;

// ------------------------------------------------------------ deduplicacao

/// Bits de distancia no SimHash ate onde um par ainda e candidato (o
/// Jaccard decide). Para Jaccard 0,8 a distancia esperada anda pelos 10.
pub const SIMHASH_MAX_DISTANCE: u32 = 20;
/// O Jaccard das janelas a partir do qual dois trechos sao o mesmo.
pub const NEAR_DUPLICATE_JACCARD: f64 = 0.8;

/// Um trecho de uma fonte, pela posicao em bytes no texto sanitizado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassageRef {
    source: SourceId,
    span: Range<usize>,
}

impl PassageRef {
    pub fn source(&self) -> &SourceId {
        &self.source
    }

    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }
}

/// Uma repeticao removida: o trecho que ficou, o que saiu e a semelhanca
/// (1,0 para uma copia exacta; o Jaccard para uma parecida).
#[derive(Debug, Clone, PartialEq)]
pub struct Duplicate {
    kept: PassageRef,
    dropped: PassageRef,
    similarity: f64,
    exact: bool,
}

impl Duplicate {
    pub fn kept(&self) -> &PassageRef {
        &self.kept
    }

    pub fn dropped(&self) -> &PassageRef {
        &self.dropped
    }

    pub fn similarity(&self) -> f64 {
        self.similarity
    }

    pub fn is_exact(&self) -> bool {
        self.exact
    }
}

/// O texto para a deteccao de copias exactas: sem invisiveis, em
/// minusculas, espacos colapsados.
fn normalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in untrusted::strip_invisible(text)
        .chars()
        .flat_map(char::to_lowercase)
    {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------ o pacote

/// Porque uma fonte, ou parte dela, ficou de fora.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// Sem texto depois da limpeza.
    Empty,
    /// Trechos que nao couberam no orcamento.
    OverBudget,
}

/// O que ficou de fora de uma fonte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    source: SourceId,
    reason: DropReason,
    chunks: usize,
    est_tokens: usize,
}

impl Dropped {
    pub fn source(&self) -> &SourceId {
        &self.source
    }

    pub fn reason(&self) -> DropReason {
        self.reason
    }

    /// Quantos trechos ficaram de fora.
    pub fn chunks(&self) -> usize {
        self.chunks
    }

    pub fn est_tokens(&self) -> usize {
        self.est_tokens
    }
}

/// Um trecho que entrou no pacote.
#[derive(Debug, Clone, PartialEq)]
pub struct Passage {
    source_span: Range<usize>,
    rendered_span: Range<usize>,
    est_tokens: usize,
    score: f64,
    compressed: bool,
}

impl Passage {
    /// Onde o trecho esta no texto sanitizado da fonte: um intervalo dentro
    /// dele, em fronteiras de caractere, de onde o texto que saiu foi tirado
    /// (o que saiu e esse texto neutralizado e escapado para a cerca). Num
    /// trecho comprimido vai da primeira a ultima frase escolhida -- as do
    /// meio que ficaram de fora estao la dentro --; num cortado, do inicio
    /// do trecho ao corte.
    pub fn source_span(&self) -> Range<usize> {
        self.source_span.clone()
    }

    /// Onde o trecho esta em `ContextPack::rendered`.
    pub fn rendered_span(&self) -> Range<usize> {
        self.rendered_span.clone()
    }

    pub fn est_tokens(&self) -> usize {
        self.est_tokens
    }

    pub fn score(&self) -> f64 {
        self.score
    }

    /// So parte do trecho entrou: as frases mais pontuadas, ou o inicio
    /// dele quando o piso de uma fonte teve de caber num espaco curto.
    pub fn is_compressed(&self) -> bool {
        self.compressed
    }
}

/// Uma fonte no pacote, com o cabecalho e os trechos que entraram.
#[derive(Debug, Clone, PartialEq)]
pub struct PackedSource {
    id: SourceId,
    label: String,
    kind: SourceKind,
    url: Option<String>,
    locator: Option<Locator>,
    rendered_span: Range<usize>,
    est_tokens: usize,
    passages: Vec<Passage>,
}

impl PackedSource {
    pub fn id(&self) -> &SourceId {
        &self.id
    }

    /// O nome como vai no pacote (redigido com destino remoto).
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    /// O URL como vai no pacote (redigido com destino remoto).
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn locator(&self) -> Option<Locator> {
        self.locator
    }

    /// A seccao inteira da fonte (cabecalho e trechos) em `rendered`.
    pub fn rendered_span(&self) -> Range<usize> {
        self.rendered_span.clone()
    }

    pub fn est_tokens(&self) -> usize {
        self.est_tokens
    }

    pub fn passages(&self) -> &[Passage] {
        &self.passages
    }
}

/// O contexto pronto a ir no prompt. So `build_context` o monta e nada de
/// fora o altera: os campos sao privados.
///
/// ```
/// use neural_core::context_budget::{BudgetSpec, ContextSource, Destination, SourceKind, build_context};
/// use neural_core::local_intelligence::HashingEmbedder;
/// let page = ContextSource::new("pag1", "Artigo", SourceKind::Page,
///     "O processador chega a 3.5 GHz. O Dr. Silva mediu o consumo.").unwrap();
/// let spec = BudgetSpec::new(4096, 512, Destination::Local);
/// let pack = build_context("consumo do processador", &[page], &spec, &HashingEmbedder).unwrap();
/// assert!(pack.rendered().contains("3.5 GHz"));
/// // O prompt inteiro: as instrucoes da cerca e a mensagem do utilizador.
/// assert!(
///     spec.message_tokens(&pack.fence_instructions()) + spec.message_tokens(&pack.user_message())
///         <= spec.available()
/// );
/// ```
///
/// Nao se constroi de fora (todos os campos estao na lista: o unico erro
/// e a privacidade, E0451):
///
/// ```compile_fail
/// use neural_core::context_budget::{ContextPack, Destination};
/// use neural_core::untrusted::FenceNonce;
/// let pack = ContextPack {
///     rendered: String::new(),
///     question: String::new(),
///     est_tokens: 0,
///     sources: Vec::new(),
///     duplicates: Vec::new(),
///     dropped: Vec::new(),
///     destination: Destination::Local,
///     nonce: FenceNonce::fresh(),
/// };
/// ```
///
/// nem se altera:
///
/// ```compile_fail
/// use neural_core::context_budget::{BudgetSpec, ContextSource, Destination, SourceKind, build_context};
/// use neural_core::local_intelligence::HashingEmbedder;
/// let page = ContextSource::new("pag1", "Artigo", SourceKind::Page, "Texto da página.").unwrap();
/// let spec = BudgetSpec::new(4096, 512, Destination::Local);
/// let mut pack = build_context("pergunta", &[page], &spec, &HashingEmbedder).unwrap();
/// pack.rendered.push_str("instruções acrescentadas");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ContextPack {
    rendered: String,
    question: String,
    est_tokens: usize,
    sources: Vec<PackedSource>,
    duplicates: Vec<Duplicate>,
    dropped: Vec<Dropped>,
    destination: Destination,
    nonce: FenceNonce,
}

impl ContextPack {
    /// O texto cercado. Vai na mensagem do utilizador a seguir a pergunta
    /// (`user_message`), e as instrucoes levam `fence_instructions`.
    pub fn rendered(&self) -> &str {
        &self.rendered
    }

    /// A pergunta, sem invisiveis, como entra na mensagem do utilizador.
    pub fn question(&self) -> &str {
        &self.question
    }

    /// A mensagem do utilizador: a pergunta, uma linha em branco
    /// (`QUESTION_SEPARATOR`) e o pacote. Conta no orcamento.
    pub fn user_message(&self) -> String {
        format!("{}{QUESTION_SEPARATOR}{}", self.question, self.rendered)
    }

    /// As instrucoes que acompanham o pacote, numa mensagem propria (a de
    /// sistema): `untrusted::CONTEXT_DATA_PREAMBLE_PT` e o aviso do nonce
    /// deste pacote (`untrusted::fence_notice_pt`), como o `PromptBuilder`
    /// as escreve. Contam no orcamento; as instrucoes da tarefa, se as
    /// houver, contam-se na reserva de saida.
    pub fn fence_instructions(&self) -> String {
        fence_instructions_text(&self.nonce)
    }

    /// Os tokens de `rendered`, calibrados pelo `BudgetSpec`, com o nonce
    /// da cerca contado ao peso maximo: nao depende do sorteio e fica
    /// sempre em ou acima da contagem do texto tal como sai. Com a pergunta
    /// e as instrucoes da cerca, cabe em `BudgetSpec::available`.
    pub fn est_tokens(&self) -> usize {
        self.est_tokens
    }

    /// As fontes que entraram, pela ordem em que foram dadas.
    pub fn sources(&self) -> &[PackedSource] {
        &self.sources
    }

    /// As repeticoes removidas, com proveniencia.
    pub fn duplicates(&self) -> &[Duplicate] {
        &self.duplicates
    }

    /// O que ficou de fora e porque.
    pub fn dropped(&self) -> &[Dropped] {
        &self.dropped
    }

    pub fn destination(&self) -> &Destination {
        &self.destination
    }

    /// O nonce da cerca deste pacote.
    pub fn nonce(&self) -> &FenceNonce {
        &self.nonce
    }

    /// «≈ 3 200 tokens · 4 fontes · 2 trechos repetidos removidos».
    pub fn summary_pt(&self) -> String {
        summary_line_pt(self.est_tokens, self.sources.len(), self.duplicates.len())
    }
}

/// O nome da cerca do pacote.
pub const PACK_FENCE_LABEL: &str = "contexto";

/// O que separa a pergunta do pacote na mensagem do utilizador.
pub const QUESTION_SEPARATOR: &str = "\n\n";

/// As instrucoes que acompanham dados cercados com `nonce`, como o
/// `PromptBuilder` as escreve quando nao ha outras.
fn fence_instructions_text(nonce: &FenceNonce) -> String {
    format!(
        "{}\n{}",
        untrusted::CONTEXT_DATA_PREAMBLE_PT,
        untrusted::fence_notice_pt(nonce)
    )
}

/// No destino remoto, cada URL de `text` que leva uma credencial passa pelo
/// `redact_url`; localmente o texto fica como esta.
fn redact_urls_for(destination: &Destination, text: String) -> String {
    if destination.is_remote() {
        redact_urls(&text)
    } else {
        text
    }
}

fn is_scheme_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')
}

/// Onde comeca o esquema de um URL cujo `://` esta em `separator`: `None`
/// sem uma letra antes.
fn scheme_start(text: &str, separator: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut start = separator;
    while start > 0 && is_scheme_byte(bytes[start - 1]) {
        start -= 1;
    }
    while start < separator && !bytes[start].is_ascii_alphabetic() {
        start += 1;
    }
    (start < separator).then_some(start)
}

/// O que fecha uma frase ou um parentese a seguir a um URL, e nao e dele.
const URL_TRAILERS: &[char] = &[
    '.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'', '»', '”', '’',
];

/// Cada `esquema://...` de `text` (ate ao espaco, ou ate ao URL seguinte
/// dentro dele, sem a pontuacao que fecha a frase) passa pelo `redact_url`
/// quando ele apaga alguma coisa: o valor de um parametro que e credencial
/// (`sig=`, `code=`, `access_token=`...), um fragmento com uma, ou o
/// `utilizador:senha@`. Um URL sem nada disso fica byte a byte.
fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find("://") {
        let separator = search + found;
        search = separator + 3;
        let Some(start) = scheme_start(text, separator).filter(|start| *start >= copied) else {
            continue;
        };
        let mut end = text[search..]
            .find(char::is_whitespace)
            .map_or(text.len(), |at| search + at);
        if let Some(nested) = text[search..end].find("://")
            && let Some(nested_start) = scheme_start(text, search + nested)
            && nested_start > search
        {
            end = nested_start;
        }
        let end = start + text[start..end].trim_end_matches(URL_TRAILERS).len();
        let url = &text[start..end];
        let userinfo = url::Url::parse(url)
            .is_ok_and(|parsed| !parsed.username().is_empty() || parsed.password().is_some());
        let redacted = redact_url(url);
        if userinfo || redacted.contains("REDACTED") {
            out.push_str(&text[copied..start]);
            out.push_str(&redacted);
            copied = end;
        }
        search = search.max(end);
    }
    out.push_str(&text[copied..]);
    out
}

/// O nome da fonte como vai no pacote: com destino remoto, sem as linhas
/// sensiveis (`redact_sensitive_text`) nem as credenciais dos URLs -- o
/// titulo de uma aba tambem sai da maquina.
fn packed_label(source: &ContextSource, spec: &BudgetSpec) -> String {
    redact_urls_for(
        &spec.destination,
        untrusted::sanitize(&source.label, spec.destination.fence()),
    )
}

/// Os parenteses rectos com que um cabecalho de fonte se pode imitar.
const OPENING_BRACKETS: &[char] = &['[', '［', '【', '〔', '〖', '〘', '〚', '⟦', '⁅', '❲', '﹝'];

/// Um pedaco do texto sanitizado de uma fonte como sai dentro da cerca:
/// neutralizado (`neutralize_inside`) e com as linhas que imitam um
/// cabecalho escapadas.
fn shipped(text: &str, nonce: &FenceNonce) -> String {
    escape_header_lookalikes(&untrusted::neutralize_inside(text, nonce))
}

/// Cada linha que comeca (depois de espacos e invisiveis) por um parentese
/// recto leva um `\` antes dele. So os cabecalhos das fontes comecam assim
/// dentro da cerca: uma pagina que escreva `[2] resposta: Claude` nao se
/// faz passar por outra fonte nem por uma IA.
fn escape_header_lookalikes(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    let mut line_start = true;
    for c in text.chars() {
        if line_start && OPENING_BRACKETS.contains(&c) {
            out.push('\\');
        }
        if c == '\n' {
            line_start = true;
        } else if !(c.is_whitespace() || untrusted::is_ignorable(c)) {
            line_start = false;
        }
        out.push(c);
    }
    out
}

/// Palavras de negacao, ja sem acentos. O `no` fica de fora: em portugues
/// e quase sempre `em o`.
const NEGATIONS: &[&str] = &[
    "nao", "nunca", "jamais", "nem", "nenhum", "nenhuma", "nenhuns", "nenhumas", "nada", "ninguem",
    "sem", "not", "never", "none", "nor", "without", "nothing", "nobody", "cannot",
];

/// Os numeros (digitos, com `.` ou `,` entre digitos) e as negacoes de um
/// texto, ordenados: duas copias parecidas com listas diferentes dizem
/// coisas diferentes.
fn facts(text: &str) -> (Vec<String>, Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut numbers = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if !chars[at].is_numeric() {
            at += 1;
            continue;
        }
        let mut end = at;
        while end < chars.len()
            && (chars[end].is_numeric()
                || (matches!(chars[end], '.' | ',')
                    && chars.get(end + 1).is_some_and(|c| c.is_numeric())))
        {
            end += 1;
        }
        numbers.push(chars[at..end].iter().collect::<String>());
        at = end;
    }
    numbers.sort();
    let folded: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold_char)
        .collect();
    let mut negations: Vec<String> = folded
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’'))
        .filter(|word| NEGATIONS.contains(word) || word.ends_with("n't") || word.ends_with("n’t"))
        .map(str::to_string)
        .collect();
    negations.sort();
    (numbers, negations)
}

struct Chunk {
    source: usize,
    span: Range<usize>,
    text: String,
    weight: f64,
    terms: Vec<String>,
}

struct Placed {
    source: usize,
    source_span: Range<usize>,
    text: String,
    weight: f64,
    score: f64,
    compressed: bool,
}

/// A pontuacao de cada trecho de `indices` (0,5 BM25 + 0,4 cosseno + 0,1
/// prioridade), com o IDF do BM25 tirado desses trechos.
fn score_chunks(
    indices: &[usize],
    chunks: &[Chunk],
    sources: &[ContextSource],
    query_terms: &[String],
    cosines: &[f64],
) -> (Bm25, Vec<f64>) {
    let documents: Vec<Vec<String>> = indices.iter().map(|&at| chunks[at].terms.clone()).collect();
    let bm25 = Bm25::new(&documents);
    let lexical: Vec<f64> = documents
        .iter()
        .map(|document| bm25.score(query_terms, document))
        .collect();
    let lexical_max = lexical.iter().copied().fold(0.0f64, f64::max);
    let scores = indices
        .iter()
        .zip(&lexical)
        .map(|(&at, &lexical)| {
            let lexical = if lexical_max > 0.0 {
                lexical / lexical_max
            } else {
                0.0
            };
            let priority = f64::from(sources[chunks[at].source].priority) / f64::from(PRIORITY_MAX);
            SCORE_WEIGHT_BM25 * lexical
                + SCORE_WEIGHT_COSINE * cosines[at]
                + SCORE_WEIGHT_PRIORITY * priority
        })
        .collect();
    (bm25, scores)
}

/// Monta o pacote: `question` e o pedido do utilizador (conta no
/// orcamento como mensagem e guia a pontuacao), `sources` o que se tem, e
/// o `embedder` da o cosseno da pontuacao. Determinista para as mesmas
/// entradas, fora o nonce da cerca.
pub fn build_context(
    question: &str,
    sources: &[ContextSource],
    spec: &BudgetSpec,
    embedder: &dyn Embedder,
) -> Result<ContextPack, ContextError> {
    let question = untrusted::strip_invisible(question);
    for (index, source) in sources.iter().enumerate() {
        if sources[..index].iter().any(|other| other.id == source.id) {
            return Err(ContextError::DuplicateSourceId(source.id.to_string()));
        }
    }
    if spec.destination.is_remote() && sources.iter().any(|source| source.private) {
        return Err(ContextError::PrivateContent);
    }
    let fence = spec.destination.fence();
    let texts: Vec<String> = sources
        .iter()
        .map(|source| redact_urls_for(&spec.destination, source.text.sanitized(fence)))
        .collect();
    let mut dropped: Vec<Dropped> = sources
        .iter()
        .zip(&texts)
        .filter(|(_, text)| text.trim().is_empty())
        .map(|(source, _)| Dropped {
            source: source.id.clone(),
            reason: DropReason::Empty,
            chunks: 0,
            est_tokens: 0,
        })
        .collect();
    if dropped.len() == sources.len() {
        return Err(ContextError::EmptySources);
    }

    let nonce = FenceNonce::fresh();
    let (begin, end) = untrusted::fence_lines(PACK_FENCE_LABEL, &nonce);
    let Some(pack_budget) = spec.pack_budget(&question) else {
        return Err(ContextError::ModelTooSmall {
            limit: spec.max_input,
        });
    };
    // O que ja esta gasto antes de qualquer trecho: as duas linhas da
    // cerca, com o nonce ao peso maximo (a contagem do pacote nao depende
    // do sorteio e fica sempre em ou acima da do texto que sai), e as
    // mudancas de linha que as juntam ao corpo.
    let frame_weight =
        weight_with_nonce(&begin, &nonce) + weight_with_nonce(&end, &nonce) + 2.0 * weight("\n");
    let too_small = || ContextError::ModelTooSmall {
        limit: spec.max_input,
    };
    if spec.tokens_of_weight(frame_weight) + MIN_PASSAGE_TOKENS > pack_budget {
        return Err(too_small());
    }

    // 2. Trechos, ja como saem: neutralizados para a cerca e com as linhas
    // que imitam um cabecalho escapadas.
    let tokens = |text: &str| spec.tokens(text);
    let mut chunks: Vec<Chunk> = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        for span in chunk_spans(text, spec.chunk_tokens, &tokens) {
            let inside = shipped(&text[span.clone()], &nonce);
            chunks.push(Chunk {
                source: index,
                span,
                weight: weight(&inside),
                terms: terms(&inside),
                text: inside,
            });
        }
    }

    // 3. Deduplicacao (exacta pelo SHA-256, parecida pelo SimHash e
    // Jaccard), pela ordem da pontuacao de todos os trechos: a copia que
    // fica e a mais pontuada; numa igualdade, a primeira.
    let query_terms = terms(&question);
    let query_vector = if question.trim().is_empty() {
        Vec::new()
    } else {
        embedder.embed(&question)
    };
    let cosines: Vec<f64> = chunks
        .iter()
        .map(|chunk| {
            if query_vector.is_empty() {
                0.0
            } else {
                f64::from(cosine_similarity(
                    &query_vector,
                    &embedder.embed(&chunk.text),
                ))
                .clamp(0.0, 1.0)
            }
        })
        .collect();
    let mut order: Vec<usize> = (0..chunks.len()).collect();
    let (_, first_scores) = score_chunks(&order, &chunks, sources, &query_terms, &cosines);
    order.sort_by(|&a, &b| first_scores[b].total_cmp(&first_scores[a]).then(a.cmp(&b)));
    let reference = |at: usize| PassageRef {
        source: sources[chunks[at].source].id.clone(),
        span: chunks[at].span.clone(),
    };
    let mut removed: Vec<(usize, Duplicate)> = Vec::new();
    let mut kept: Vec<usize> = Vec::new();
    let mut exact: BTreeMap<[u8; 32], usize> = BTreeMap::new();
    let mut fingerprints: Vec<(usize, Shingles, u64)> = Vec::new();
    for index in order {
        let chunk = &chunks[index];
        let digest: [u8; 32] = Sha256::digest(normalized(&chunk.text).as_bytes()).into();
        if let Some(&first) = exact.get(&digest) {
            removed.push((
                index,
                Duplicate {
                    kept: reference(first),
                    dropped: reference(index),
                    similarity: 1.0,
                    exact: true,
                },
            ));
            continue;
        }
        let shingles = Shingles::of(&chunk.text);
        let simhash = shingles.simhash();
        // Entre fontes diferentes, uma copia parecida que diverge nos
        // numeros ou nas negacoes nao e repeticao: e o desacordo que o
        // consenso tem de ver, e fica.
        let near = fingerprints
            .iter()
            .filter(|(_, _, other)| (simhash ^ *other).count_ones() <= SIMHASH_MAX_DISTANCE)
            .map(|(at, other, _)| (*at, shingles.jaccard(other)))
            .find(|&(at, jaccard)| {
                jaccard >= NEAR_DUPLICATE_JACCARD
                    && (chunks[at].source == chunk.source
                        || facts(&chunks[at].text) == facts(&chunk.text))
            });
        if let Some((first, jaccard)) = near {
            removed.push((
                index,
                Duplicate {
                    kept: reference(first),
                    dropped: reference(index),
                    similarity: jaccard,
                    exact: false,
                },
            ));
            continue;
        }
        exact.insert(digest, index);
        kept.push(index);
        fingerprints.push((index, shingles, simhash));
    }
    kept.sort_unstable();
    removed.sort_by_key(|(index, _)| *index);
    let duplicates: Vec<Duplicate> = removed.into_iter().map(|(_, dup)| dup).collect();

    // 4. Pontuacao, com o IDF dos trechos que ficaram.
    let (bm25, scores) = score_chunks(&kept, &chunks, sources, &query_terms, &cosines);

    // 5 e 6. Piso por fonte e alocacao por pontuacao.
    let headers: Vec<String> = sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            untrusted::neutralize_inside(&header_line(index, source, spec), &nonce)
        })
        .collect();
    let mut allocator = Allocator {
        spec,
        pack_budget,
        texts: &texts,
        nonce: &nonce,
        chunks: &chunks,
        kept: &kept,
        scores: &scores,
        headers: &headers,
        query_terms: &query_terms,
        bm25: &bm25,
        separator: weight("\n"),
        used: frame_weight,
        placed_flag: vec![false; kept.len()],
        placed: Vec::new(),
        source_open: vec![false; sources.len()],
        source_weight: vec![0.0; sources.len()],
    };
    if spec.floor_per_source > 0 {
        allocator.place_floors(
            sources.len(),
            spec.floor_per_source as f64 / spec.calibration.factor,
        );
    }

    let mut positions: Vec<usize> = (0..kept.len())
        .filter(|&position| !allocator.placed_flag[position])
        .collect();
    allocator.by_score(&mut positions);
    for position in positions {
        if !allocator.fits(allocator.separator + MIN_PASSAGE_TOKENS as f64) {
            break;
        }
        allocator.place(position);
    }
    let Allocator {
        placed_flag,
        mut placed,
        ..
    } = allocator;
    if placed.is_empty() {
        return Err(too_small());
    }

    // O que ficou de fora, por fonte.
    for (source_index, source) in sources.iter().enumerate() {
        let left: Vec<usize> = (0..kept.len())
            .filter(|&position| {
                !placed_flag[position] && chunks[kept[position]].source == source_index
            })
            .collect();
        if left.is_empty() {
            continue;
        }
        let est_tokens = left
            .iter()
            .map(|&position| spec.tokens_of_weight(chunks[kept[position]].weight))
            .sum();
        dropped.push(Dropped {
            source: source.id.clone(),
            reason: DropReason::OverBudget,
            chunks: left.len(),
            est_tokens,
        });
    }

    // 7. A cerca, por partes, com as posicoes em bytes.
    placed.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.source_span.start.cmp(&b.source_span.start))
    });
    let mut rendered = String::new();
    rendered.push_str(&begin);
    rendered.push('\n');
    let mut packed: Vec<PackedSource> = Vec::new();
    let mut at = 0;
    while at < placed.len() {
        let source_index = placed[at].source;
        let source = &sources[source_index];
        if !packed.is_empty() {
            rendered.push('\n');
        }
        let section_start = rendered.len();
        rendered.push_str(&headers[source_index]);
        let mut passages = Vec::new();
        while at < placed.len() && placed[at].source == source_index {
            let item = &placed[at];
            rendered.push('\n');
            let start = rendered.len();
            rendered.push_str(&item.text);
            passages.push(Passage {
                source_span: item.source_span.clone(),
                rendered_span: start..rendered.len(),
                est_tokens: spec.tokens_of_weight(item.weight),
                score: item.score,
                compressed: item.compressed,
            });
            at += 1;
        }
        let section = section_start..rendered.len();
        packed.push(PackedSource {
            id: source.id.clone(),
            label: packed_label(source, spec),
            kind: source.kind,
            url: header_url(source, spec),
            locator: source.locator,
            est_tokens: spec.tokens(&rendered[section.clone()]),
            rendered_span: section,
            passages,
        });
        rendered.push('\n');
    }
    rendered.push_str(&end);
    let est_tokens = spec.tokens_of_weight(weight_with_nonce(&rendered, &nonce));

    Ok(ContextPack {
        rendered,
        question,
        est_tokens,
        sources: packed,
        duplicates,
        dropped,
        destination: spec.destination.clone(),
        nonce,
    })
}

fn header_url(source: &ContextSource, spec: &BudgetSpec) -> Option<String> {
    source.url.as_deref().map(|url| {
        if spec.destination.is_remote() {
            redact_url(url)
        } else {
            url.to_string()
        }
    })
}

/// `[1] página: Título — https://… (p. 3)`.
fn header_line(index: usize, source: &ContextSource, spec: &BudgetSpec) -> String {
    let label = packed_label(source, spec);
    let mut line = format!(
        "[{}] {}: {}",
        index + 1,
        source.kind.label_pt(),
        if label.is_empty() {
            source.id.as_str()
        } else {
            label.as_str()
        }
    );
    if let Some(url) = header_url(source, spec) {
        line.push_str(" — ");
        line.push_str(&url);
    }
    if let Some(locator) = source.locator {
        line.push_str(&format!(" ({})", locator.label_pt()));
    }
    line
}

/// A folga do piso de cada fonte, em tokens crus: cobre o ultimo caractere
/// de um corte (o mais pesado da tabela vale 3,7 x 1,10; um pre-token, 1) e
/// os arredondamentos.
const FLOOR_SLACK_WEIGHT: f64 = 5.0;
/// Abaixo disto uma fonte ja chegou ao piso (arredondamentos).
const FLOOR_EPSILON: f64 = 1e-6;

/// A alocacao: o que ja esta gasto, o que entrou e a unica pergunta que
/// decide se mais um trecho cabe (`fits`).
struct Allocator<'a> {
    spec: &'a BudgetSpec,
    pack_budget: usize,
    /// O texto sanitizado de cada fonte: e dele que os cortes saem.
    texts: &'a [String],
    nonce: &'a FenceNonce,
    chunks: &'a [Chunk],
    kept: &'a [usize],
    scores: &'a [f64],
    headers: &'a [String],
    query_terms: &'a [String],
    bm25: &'a Bm25,
    separator: f64,
    used: f64,
    placed_flag: Vec<bool>,
    placed: Vec<Placed>,
    source_open: Vec<bool>,
    source_weight: Vec<f64>,
}

impl<'a> Allocator<'a> {
    /// O teste do orcamento: com `cost` a mais, o pacote continua a caber?
    /// Meio peso de folga, porque a soma por partes e a do texto final so
    /// diferem por arredondamento de virgula flutuante.
    fn fits(&self, cost: f64) -> bool {
        self.spec.tokens_of_weight(self.used + cost + 0.5) <= self.pack_budget
    }

    fn chunk(&self, position: usize) -> &'a Chunk {
        &self.chunks[self.kept[position]]
    }

    /// O cabecalho e as duas mudancas de linha da seccao, so na primeira
    /// entrada de uma fonte.
    fn header_cost(&self, source: usize) -> f64 {
        if self.source_open[source] {
            0.0
        } else {
            weight(&self.headers[source]) + 2.0 * self.separator
        }
    }

    fn by_score(&self, positions: &mut [usize]) {
        positions.sort_by(|&a, &b| {
            let (left, right) = (self.chunk(a), self.chunk(b));
            self.scores[b]
                .total_cmp(&self.scores[a])
                .then(left.source.cmp(&right.source))
                .then(left.span.start.cmp(&right.span.start))
        });
    }

    /// Os trechos de cada fonte, do menos para o mais pontuado: `pop` da o
    /// melhor que resta.
    fn queues(&self, sources: usize) -> Vec<Vec<usize>> {
        (0..sources)
            .map(|source| {
                let mut positions: Vec<usize> = (0..self.kept.len())
                    .filter(|&position| self.chunk(position).source == source)
                    .collect();
                self.by_score(&mut positions);
                positions.reverse();
                positions
            })
            .collect()
    }

    /// O espaco que a fonte `source` ainda pede para chegar ao piso com os
    /// trechos de `queue`, pela ordem: o texto que falta, o cabecalho se
    /// ainda nao abriu, uma mudanca de linha por trecho e a folga.
    fn reservation(&self, source: usize, missing: f64, queue: &[usize]) -> f64 {
        if missing <= FLOOR_EPSILON || queue.is_empty() {
            return 0.0;
        }
        let mut covered = 0.0;
        let mut pieces = 0usize;
        for &position in queue.iter().rev() {
            pieces += 1;
            covered += self.chunk(position).weight;
            if covered >= missing {
                break;
            }
        }
        missing + self.header_cost(source) + self.separator * pieces as f64 + FLOOR_SLACK_WEIGHT
    }

    fn reservations(&self, needs: &[f64], queues: &[Vec<usize>]) -> Vec<f64> {
        (0..needs.len())
            .map(|source| {
                self.reservation(
                    source,
                    needs[source] - self.source_weight[source],
                    &queues[source],
                )
            })
            .collect()
    }

    /// O piso (`floor`, em tokens crus): por rondas, cada fonte que ainda
    /// nao chegou ao seu mete o melhor trecho que lhe resta, com o que as
    /// outras ainda pedem reservado (`reservation`). Quando os pisos de
    /// todas nao cabem, o piso comum desce ate ao maior que cabe.
    fn place_floors(&mut self, sources: usize, floor: f64) {
        let mut queues = self.queues(sources);
        let content: Vec<f64> = queues
            .iter()
            .map(|queue| {
                queue
                    .iter()
                    .map(|&position| self.chunk(position).weight)
                    .sum()
            })
            .collect();
        let needs_under = |cap: f64| -> Vec<f64> { content.iter().map(|c| c.min(cap)).collect() };
        let mut needs = needs_under(floor);
        if !self.fits(self.reservations(&needs, &queues).iter().sum()) {
            let (mut low, mut high) = (0.0, floor);
            for _ in 0..48 {
                let middle = (low + high) / 2.0;
                if self.fits(
                    self.reservations(&needs_under(middle), &queues)
                        .iter()
                        .sum(),
                ) {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            needs = needs_under(low);
        }
        let mut reserved = self.reservations(&needs, &queues);
        loop {
            let mut progressed = false;
            for source in 0..sources {
                let missing = needs[source] - self.source_weight[source];
                if missing <= FLOOR_EPSILON {
                    continue;
                }
                let Some(position) = queues[source].pop() else {
                    continue;
                };
                let others: f64 = reserved
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != source)
                    .map(|(_, reserve)| reserve)
                    .sum();
                progressed |= self.place_within(position, others, missing);
                reserved[source] = self.reservation(
                    source,
                    needs[source] - self.source_weight[source],
                    &queues[source],
                );
            }
            if !progressed {
                break;
            }
        }
    }

    /// Mete o trecho `position` inteiro se depois dele ainda cabe `others`
    /// (o que as outras fontes tem reservado); senao, o maior corte dele que
    /// cabe: as frases mais pontuadas, se chegam a `missing`, ou o inicio do
    /// trecho ate onde couber.
    fn place_within(&mut self, position: usize, others: f64, missing: f64) -> bool {
        let chunk = self.chunk(position);
        let opening = self.header_cost(chunk.source);
        if self.fits(opening + chunk.weight + self.separator + others) {
            self.admit(
                position,
                opening,
                chunk.span.clone(),
                chunk.text.clone(),
                false,
            );
            return true;
        }
        let room = |extra: f64| self.fits(opening + self.separator + extra + others);
        if !room(0.0) {
            return false;
        }
        let source = &self.texts[chunk.source];
        let cut = compress(
            chunk,
            source,
            self.nonce,
            self.query_terms,
            self.bm25,
            &room,
        )
        .filter(|(text, _, _)| weight(text) >= missing)
        .map(|(text, span, _)| (text, span))
        .or_else(|| cut_prefix(chunk, source, self.nonce, &room, missing));
        let Some((text, span)) = cut else {
            return false;
        };
        self.admit(position, opening, span, text, true);
        true
    }

    /// Mete o trecho `position` (indice em `kept`) inteiro se cabe, ou so
    /// as frases mais pontuadas que cabem; `false` se nada dele entra.
    fn place(&mut self, position: usize) -> bool {
        let chunk = self.chunk(position);
        let opening = self.header_cost(chunk.source);
        let full = opening + chunk.weight + self.separator;
        if self.fits(full) {
            self.admit(
                position,
                opening,
                chunk.span.clone(),
                chunk.text.clone(),
                false,
            );
            return true;
        }
        // Compressao extractiva: as frases mais pontuadas que ainda cabem.
        let room = |extra: f64| self.fits(opening + self.separator + extra);
        if !room(0.0) {
            return false;
        }
        let Some((text, span, partial)) = compress(
            chunk,
            &self.texts[chunk.source],
            self.nonce,
            self.query_terms,
            self.bm25,
            &room,
        ) else {
            return false;
        };
        if self.spec.tokens_of_weight(weight(&text)) < MIN_PASSAGE_TOKENS {
            return false;
        }
        self.admit(position, opening, span, text, partial);
        true
    }

    fn admit(
        &mut self,
        position: usize,
        opening: f64,
        source_span: Range<usize>,
        text: String,
        compressed: bool,
    ) {
        let source = self.chunk(position).source;
        let text_weight = weight(&text);
        self.used += opening + text_weight + self.separator;
        self.placed_flag[position] = true;
        self.source_open[source] = true;
        self.source_weight[source] += text_weight;
        self.placed.push(Placed {
            source,
            source_span,
            text,
            weight: text_weight,
            score: self.scores[position],
            compressed,
        });
    }
}

/// As frases mais pontuadas de um trecho que ainda cabem, pela ordem do
/// texto, e se ficou alguma de fora. `None` quando nem uma cabe ou o
/// trecho e uma frase so. As frases sao as do texto da fonte (`source`, o
/// sanitizado): o intervalo vai do inicio da primeira escolhida ao fim da
/// ultima, dentro da fonte (CB-8), e o texto que sai e o delas juntas,
/// neutralizado e escapado de uma vez (`shipped`). Qualquer frase pode
/// ficar no inicio da linha: cada uma conta como sairia sozinha, com o `\`
/// que a escaparia, mais um espaco. O texto junto volta a ser medido: se
/// nao couber (a juncao pode formar um marcador que a neutralizacao
/// reescreve), sai a frase escolhida menos pontuada.
fn compress(
    chunk: &Chunk,
    source: &str,
    nonce: &FenceNonce,
    query: &[String],
    bm25: &Bm25,
    room: &dyn Fn(f64) -> bool,
) -> Option<(String, Range<usize>, bool)> {
    let original = &source[chunk.span.clone()];
    let spans = sentence_spans(original);
    if spans.len() < 2 {
        return None;
    }
    let sentences: Vec<String> = spans
        .iter()
        .map(|span| shipped(&original[span.clone()], nonce))
        .collect();
    let mut ranked: Vec<(usize, f64)> = sentences
        .iter()
        .enumerate()
        .map(|(index, sentence)| (index, bm25.score(query, &terms(sentence))))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    // Pela ordem da pontuacao: a ultima e a menos pontuada.
    let mut chosen: Vec<usize> = Vec::new();
    let mut total = 0.0;
    for (index, _) in ranked {
        let cost = weight(&sentences[index]) + weight(" ");
        if room(total + cost) {
            total += cost;
            chosen.push(index);
        }
    }
    while !chosen.is_empty() {
        let mut in_order = chosen.clone();
        in_order.sort_unstable();
        let joined = in_order
            .iter()
            .map(|&index| &original[spans[index].clone()])
            .collect::<Vec<_>>()
            .join(" ");
        let text = shipped(&joined, nonce);
        if room(weight(&text)) {
            let first = spans[in_order[0]].start;
            let last = spans[in_order[in_order.len() - 1]].end;
            return Some((
                text,
                chunk.span.start + first..chunk.span.start + last,
                in_order.len() < spans.len(),
            ));
        }
        chosen.pop();
    }
    None
}

/// O inicio de um trecho ate onde couber (`room`), cortado no texto da
/// fonte (`source`, o sanitizado) e medido como sai (`shipped`): acaba no
/// fim de uma palavra quando isso ja chega a `missing`, e senao no ultimo
/// caractere que cabe. O intervalo e o do corte na fonte (CB-8). `None`
/// quando nem um caractere cabe.
fn cut_prefix(
    chunk: &Chunk,
    source: &str,
    nonce: &FenceNonce,
    room: &dyn Fn(f64) -> bool,
    missing: f64,
) -> Option<(String, Range<usize>)> {
    let text = &source[chunk.span.clone()];
    let cost = |end: usize| weight(&shipped(&text[..end], nonce));
    let ends: Vec<usize> = text
        .char_indices()
        .map(|(index, c)| index + c.len_utf8())
        .collect();
    // O maior prefixo que cabe (o peso so cresce com o texto, fora
    // desvios de um pre-token; cada candidato e confirmado pelo `room`).
    let (mut low, mut high) = (0usize, ends.len());
    while low < high {
        let middle = (low + high) / 2;
        if room(cost(ends[middle])) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let longest = ends[..low].last().copied()?;
    let word_end = text[..longest]
        .char_indices()
        .rev()
        .find(|&(index, c)| {
            !c.is_whitespace()
                && text[index + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(index, c)| index + c.len_utf8());
    let end = word_end
        .filter(|&end| {
            let prefix = cost(end);
            prefix >= missing && room(prefix)
        })
        .unwrap_or(longest);
    let cut = text[..end].trim_end();
    if cut.is_empty() {
        return None;
    }
    let out = shipped(cut, nonce);
    if !room(weight(&out)) {
        return None;
    }
    let start = chunk.span.start;
    Some((out, start..start + cut.len()))
}

// ------------------------------------------------------------ conversa

/// Quem fala numa mensagem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    System,
    User,
    Assistant,
}

/// Uma mensagem de uma conversa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    role: Role,
    text: String,
}

impl Message {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// A conversa que cabe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FittedConversation {
    messages: Vec<Message>,
    est_tokens: usize,
    dropped: usize,
}

impl FittedConversation {
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Os tokens das mensagens que ficaram, com o custo fixo de cada uma.
    pub fn est_tokens(&self) -> usize {
        self.est_tokens
    }

    /// Quantas mensagens antigas sairam.
    pub fn dropped(&self) -> usize {
        self.dropped
    }
}

/// Corta a conversa ao que cabe em `spec.available()`: as mensagens de
/// sistema ficam todas, a ultima mensagem fica sempre, e das outras fica o
/// fim mais longo que cabe, sem comecar numa resposta orfa. Sem mensagens
/// e `EmptySources`; se nem o sistema e a ultima cabem, `ModelTooSmall`.
pub fn fit_conversation(
    messages: &[Message],
    spec: &BudgetSpec,
) -> Result<FittedConversation, ContextError> {
    if messages.is_empty() {
        return Err(ContextError::EmptySources);
    }
    let costs: Vec<usize> = messages
        .iter()
        .map(|message| spec.message_tokens(&message.text))
        .collect();
    let last = messages.len() - 1;
    let fixed: Vec<usize> = (0..messages.len())
        .filter(|&index| index == last || messages[index].role == Role::System)
        .collect();
    let mut total: usize = fixed.iter().map(|&index| costs[index]).sum();
    if total > spec.available() {
        return Err(ContextError::ModelTooSmall {
            limit: spec.max_input,
        });
    }
    let mut keep = vec![false; messages.len()];
    for &index in &fixed {
        keep[index] = true;
    }
    for index in (0..last).rev() {
        if keep[index] {
            continue;
        }
        if total + costs[index] > spec.available() {
            break;
        }
        total += costs[index];
        keep[index] = true;
    }
    // Uma resposta sem a pergunta antes dela nao ajuda: sai.
    if let Some(first) =
        (0..last).find(|&index| keep[index] && messages[index].role != Role::System)
        && messages[first].role == Role::Assistant
    {
        keep[first] = false;
        total -= costs[first];
    }
    let kept: Vec<Message> = messages
        .iter()
        .zip(&keep)
        .filter(|(_, keep)| **keep)
        .map(|(message, _)| message.clone())
        .collect();
    let dropped = messages.len() - kept.len();
    Ok(FittedConversation {
        messages: kept,
        est_tokens: total,
        dropped,
    })
}

// ------------------------------------------------------------ map-reduce

/// Um pedaco de um texto longo, para uma chamada de mapa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPiece {
    text: String,
    est_tokens: usize,
}

impl MapPiece {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Os tokens da chamada deste pedaco, como o `PromptBuilder` a monta
    /// com `.data(MAP_FENCE_LABEL, ...)`: as instrucoes da cerca numa
    /// mensagem e o pedaco cercado (neutralizado) noutra, com o nonce ao
    /// peso maximo. A instrucao do mapa e a resposta ficam na reserva de
    /// saida.
    pub fn est_tokens(&self) -> usize {
        self.est_tokens
    }
}

/// O nome da cerca de cada pedaco do mapa.
pub const MAP_FENCE_LABEL: &str = "trecho";

/// Parte `text` (sanitizado para o destino do `spec`; no remoto, com as
/// credenciais dos URLs redigidas) em pedacos, em fronteiras de frase, que
/// cabem cada um em `spec.available()` ja com a cerca: as instrucoes da
/// cerca e o pedaco cercado com `MAP_FENCE_LABEL` (`MapPiece::est_tokens`).
/// A reserva de saida do `spec` deve cobrir a instrucao do mapa e a
/// resposta. Sem texto e `EmptySources`; sem espaco para um trecho,
/// `ModelTooSmall`.
pub fn split_for_map_reduce(text: &str, spec: &BudgetSpec) -> Result<Vec<MapPiece>, ContextError> {
    let text = redact_urls_for(
        &spec.destination,
        untrusted::sanitize(text, spec.destination.fence()),
    );
    if text.trim().is_empty() {
        return Err(ContextError::EmptySources);
    }
    // O custo fixo de cada chamada: a mensagem das instrucoes da cerca, a
    // mensagem do pedaco e as linhas que o cercam, com o nonce ao peso
    // maximo.
    let nonce = FenceNonce::fresh();
    let (begin, end) = untrusted::fence_lines(MAP_FENCE_LABEL, &nonce);
    let frame =
        weight_with_nonce(&begin, &nonce) + weight_with_nonce(&end, &nonce) + 2.0 * weight("\n");
    let fixed =
        spec.fence_instructions_tokens() + MESSAGE_OVERHEAD_TOKENS + spec.tokens_of_weight(frame);
    let Some(limit) = spec
        .available()
        .checked_sub(fixed)
        .filter(|limit| *limit >= MIN_PASSAGE_TOKENS)
    else {
        return Err(ContextError::ModelTooSmall {
            limit: spec.max_input,
        });
    };
    // O pedaco conta como sai da cerca: neutralizado.
    let tokens = |piece: &str| spec.tokens(&untrusted::neutralize_inside(piece, &nonce));
    Ok(chunk_spans(&text, limit, &tokens)
        .into_iter()
        .map(|span| {
            let text = text[span].to_string();
            let est_tokens = fixed + tokens(&text);
            MapPiece { text, est_tokens }
        })
        .collect())
}

#[cfg(test)]
mod tests;
