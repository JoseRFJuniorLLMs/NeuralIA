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
//!    `redact_sensitive_text` que ja existe, e o URL de cada fonte pelo
//!    `redact_url`. Uma fonte de uma sessao privada nunca vai para um
//!    destino remoto (`ContextError::PrivateContent`).
//! 2. **partir em trechos** de ~`chunk_tokens` (160 por omissao) em
//!    fronteiras de frase: `Dr. Silva` e `3.5 GHz` ficam inteiros
//!    (abreviaturas, iniciais e decimais nao fecham frase).
//! 3. **deduplicar**: o SHA-256 do texto normalizado apanha as copias
//!    exactas; o SimHash das janelas (`untrusted::Shingles`) filtra as
//!    parecidas e o Jaccard >= 0.8 confirma. Cada repeticao fica registada
//!    com a proveniencia (quem ficou, quem saiu, a semelhanca).
//! 4. **pontuar**: 0.5 x BM25-lite contra a pergunta + 0.4 x cosseno do
//!    `Embedder` + 0.1 x prioridade da fonte.
//! 5. **garantir o piso** de cada fonte (`floor_per_source`): os melhores
//!    trechos de cada uma entram primeiro, para uma fonte forte nao apagar
//!    as outras.
//! 6. **alocar** o resto por pontuacao, com **compressao extractiva**
//!    (as frases mais pontuadas de um trecho que ja nao cabe inteiro).
//! 7. **cercar** com o nonce do `untrusted` (`fence_lines`,
//!    `neutralize_inside`): o texto do pacote e dado, nunca instrucao.
//!
//! O `ContextPack` que sai tem campos privados: le-se (`rendered`,
//! `est_tokens`, `sources` com as posicoes em bytes, `duplicates`,
//! `dropped`) e nao se monta nem se altera de fora -- o unico caminho para
//! um pacote e este.
//!
//! A contagem de tokens (`estimate_tokens`) e uma tabela por classe de
//! caractere x 1,10, mais 4 por mensagem; por construcao fica ACIMA dos
//! tokenizadores reais (fixture em `context_budget/token_fixture.tsv`), e
//! a `TokenCalibration` (EWMA da razao real/estimada, presa a [0,6; 2,0])
//! encosta-a ao modelo em uso quando a API devolve a contagem real.
//! `fit_conversation` corta a conversa mais antiga; `split_for_map_reduce`
//! parte um texto longo em pedacos que cabem, um a um.

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

/// O peso de um caractere na estimativa, por classe. Calibrado para ficar
/// ACIMA do o200k e do tokenizador do Llama 3: cada palavra e pelo menos
/// um token, por isso o espaco que a antecede vale quase um (0,7) e as
/// letras pouco (0,22) -- `a b c d` sao quatro tokens, `processador` dois
/// ou tres; um digito custa quase um token (`\p{N}{1,3}`: `2026-09-26` sao
/// seis); a pontuacao ASCII e quase sempre um token cada; um espaco a
/// seguir a outro e uma sequencia, com custo; um emoji ou uma letra fora
/// do BMP pode cair em bytes (ate 4 tokens); o CJK ate 3 por caractere.
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

/// A soma dos pesos de `text` (aditiva: o peso de `a + b` e o de `a` mais
/// o de `b`, com a ressalva de um espaco a seguir a outro, que nao muda com
/// a juncao porque as partes juntam-se sempre com uma mudanca de linha).
fn weight(text: &str) -> f64 {
    let mut total = 0.0;
    let mut after_space = false;
    for c in text.chars() {
        total += char_weight(c, after_space);
        after_space = c == ' ';
    }
    total
}

fn tokens_from_weight(weight: f64) -> usize {
    if weight <= 0.0 {
        return 0;
    }
    (weight * TOKEN_SAFETY_FACTOR).ceil() as usize
}

/// Os tokens de um texto: a tabela por classe de caractere x 1,10,
/// arredondada para cima. Zero para o texto vazio.
pub fn estimate_tokens(text: &str) -> usize {
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
    /// livre para a resposta.
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
    /// pontuacao (0 desliga o piso).
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

    /// O que sobra para o pacote depois da pergunta (como mensagem):
    /// `None` quando nem a pergunta cabe.
    pub fn pack_budget(&self, question: &str) -> Option<usize> {
        self.available().checked_sub(self.message_tokens(question))
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
const CLOSERS: &[char] = &['"', '\'', '”', '’', '»', ')', ']', '}'];

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
/// e seguido de espaco, e por isso nunca fecha.
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
        if TERMINALS.contains(&c) {
            let mut j = i + 1;
            while j < chars.len()
                && (TERMINALS.contains(&chars[j].1) || CLOSERS.contains(&chars[j].1))
            {
                j += 1;
            }
            let end = chars.get(j).map_or(text.len(), |(byte, _)| *byte);
            let at_end = j >= chars.len();
            if at_end || chars[j].1.is_whitespace() {
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
/// palavra ainda for maior, em caracteres.
pub fn chunk_spans(text: &str, limit: usize, tokens: &dyn Fn(&str) -> usize) -> Vec<Range<usize>> {
    let limit = limit.max(1);
    let mut chunks = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for sentence in sentence_spans(text) {
        if tokens(&text[sentence.clone()]) > limit {
            if let Some(open) = current.take() {
                chunks.push(open);
            }
            hard_split(text, sentence, limit, tokens, &mut chunks);
            continue;
        }
        current = Some(match current {
            Some(open) if tokens(&text[open.start..sentence.end]) <= limit => {
                open.start..sentence.end
            }
            Some(open) => {
                chunks.push(open);
                sentence
            }
            None => sentence,
        });
    }
    if let Some(open) = current {
        chunks.push(open);
    }
    chunks
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
    let mut open: Option<Range<usize>> = None;
    for word in words {
        if tokens(&text[word.clone()]) > limit {
            if let Some(range) = open.take() {
                out.push(range);
            }
            split_word(text, word, limit, tokens, out);
            continue;
        }
        open = Some(match open {
            Some(range) if tokens(&text[range.start..word.end]) <= limit => range.start..word.end,
            Some(range) => {
                out.push(range);
                word
            }
            None => word,
        });
    }
    if let Some(range) = open {
        out.push(range);
    }
}

fn split_word(
    text: &str,
    span: Range<usize>,
    limit: usize,
    tokens: &dyn Fn(&str) -> usize,
    out: &mut Vec<Range<usize>>,
) {
    let mut piece_start = span.start;
    let mut previous = span.start;
    for (index, c) in text[span.clone()].char_indices() {
        let end = span.start + index + c.len_utf8();
        if tokens(&text[piece_start..end]) > limit && previous > piece_start {
            out.push(piece_start..previous);
            piece_start = previous;
        }
        previous = end;
    }
    if piece_start < span.end {
        out.push(piece_start..span.end);
    }
}

// ------------------------------------------------------------ pontuacao

/// Palavras vazias, ja sem acentos (as palavras comparadas tambem o sao).
const STOPWORDS: &[&str] = &[
    "de", "da", "do", "das", "dos", "em", "um", "uma", "uns", "umas", "para", "por", "com", "sem",
    "que", "se", "no", "na", "nos", "nas", "os", "as", "ao", "aos", "nao", "mais", "como", "mas",
    "foi", "ele", "ela", "eles", "elas", "tem", "seu", "sua", "seus", "suas", "ou", "ser",
    "quando", "muito", "ha", "ja", "esta", "estao", "eu", "tambem", "so", "pelo", "pela", "pelos",
    "pelas", "ate", "isso", "isto", "entre", "era", "depois", "mesmo", "ter", "quem", "me", "esse",
    "essa", "este", "voce", "tinha", "foram", "num", "numa", "nem", "meu", "minha", "sobre",
    "qual", "quais", "quanto", "the", "of", "and", "to", "in", "is", "an", "that", "it", "for",
    "on", "with", "are", "this", "be", "by", "or", "at", "from", "was", "were", "which", "what",
    "how", "does", "did", "not", "its", "into", "than", "then", "there", "these", "those", "their",
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
        .filter(|word| !STOPWORDS.contains(word))
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
    /// Onde o trecho esta no texto sanitizado da fonte. Num trecho
    /// comprimido cujo texto a cerca teve de neutralizar (um marcador
    /// parecido la dentro), o intervalo e aproximado: as posicoes vem do
    /// texto neutralizado, que pode ter outro tamanho.
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

    /// So as frases mais pontuadas do trecho entraram.
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
/// assert!(pack.est_tokens() <= spec.available());
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
    est_tokens: usize,
    sources: Vec<PackedSource>,
    duplicates: Vec<Duplicate>,
    dropped: Vec<Dropped>,
    destination: Destination,
    nonce: FenceNonce,
}

impl ContextPack {
    /// O texto cercado, pronto para a mensagem do utilizador a seguir a
    /// pergunta. As instrucoes levam `untrusted::CONTEXT_DATA_PREAMBLE_PT`
    /// e `untrusted::fence_notice_pt(pack.nonce())`.
    pub fn rendered(&self) -> &str {
        &self.rendered
    }

    /// Os tokens de `rendered`, calibrados pelo `BudgetSpec`, com o nonce
    /// da cerca contado ao peso maximo: nao depende do sorteio e fica
    /// sempre em ou acima da contagem do texto tal como sai.
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
        .map(|source| source.text.sanitized(fence))
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
    // O nonce conta ao peso maximo (so digitos): a contagem do pacote nao
    // depende do sorteio e fica sempre em ou acima da do texto que sai.
    let heaviest_nonce = "9".repeat(nonce.as_str().len());
    let canonical = |text: &str| text.replace(nonce.as_str(), &heaviest_nonce);
    // O que ja esta gasto antes de qualquer trecho: as duas linhas da
    // cerca e as mudancas de linha que as juntam ao corpo.
    let frame_weight = weight(&canonical(&begin)) + weight(&canonical(&end)) + 2.0 * weight("\n");
    let too_small = || ContextError::ModelTooSmall {
        limit: spec.max_input,
    };
    if spec.tokens_of_weight(frame_weight) + MIN_PASSAGE_TOKENS > pack_budget {
        return Err(too_small());
    }

    // 2. Trechos.
    let tokens = |text: &str| spec.tokens(text);
    let mut chunks: Vec<Chunk> = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        for span in chunk_spans(text, spec.chunk_tokens, &tokens) {
            let inside = untrusted::neutralize_inside(&text[span.clone()], &nonce);
            chunks.push(Chunk {
                source: index,
                span,
                weight: weight(&inside),
                terms: terms(&inside),
                text: inside,
            });
        }
    }

    // 3. Deduplicacao: exacta pelo SHA-256, parecida pelo SimHash e Jaccard.
    let mut duplicates = Vec::new();
    let mut kept: Vec<usize> = Vec::new();
    let mut exact: BTreeMap<[u8; 32], usize> = BTreeMap::new();
    let mut fingerprints: Vec<(Shingles, u64)> = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let digest: [u8; 32] = Sha256::digest(normalized(&chunk.text).as_bytes()).into();
        let reference = |at: usize| PassageRef {
            source: sources[chunks[at].source].id.clone(),
            span: chunks[at].span.clone(),
        };
        if let Some(&first) = exact.get(&digest) {
            duplicates.push(Duplicate {
                kept: reference(first),
                dropped: reference(index),
                similarity: 1.0,
                exact: true,
            });
            continue;
        }
        let shingles = Shingles::of(&chunk.text);
        let simhash = shingles.simhash();
        let near = kept
            .iter()
            .zip(&fingerprints)
            .filter(|(_, (_, other))| (simhash ^ *other).count_ones() <= SIMHASH_MAX_DISTANCE)
            .map(|(&at, (other, _))| (at, shingles.jaccard(other)))
            .find(|(_, jaccard)| *jaccard >= NEAR_DUPLICATE_JACCARD);
        if let Some((first, jaccard)) = near {
            duplicates.push(Duplicate {
                kept: reference(first),
                dropped: reference(index),
                similarity: jaccard,
                exact: false,
            });
            continue;
        }
        exact.insert(digest, index);
        kept.push(index);
        fingerprints.push((shingles, simhash));
    }

    // 4. Pontuacao.
    let query_terms = terms(&question);
    let documents: Vec<Vec<String>> = kept.iter().map(|&at| chunks[at].terms.clone()).collect();
    let bm25 = Bm25::new(&documents);
    let lexical: Vec<f64> = documents
        .iter()
        .map(|document| bm25.score(&query_terms, document))
        .collect();
    let lexical_max = lexical.iter().copied().fold(0.0f64, f64::max);
    let query_vector = if question.trim().is_empty() {
        Vec::new()
    } else {
        embedder.embed(&question)
    };
    let scores: Vec<f64> = kept
        .iter()
        .zip(&lexical)
        .map(|(&at, &lexical)| {
            let chunk = &chunks[at];
            let bm25 = if lexical_max > 0.0 {
                lexical / lexical_max
            } else {
                0.0
            };
            let cosine = if query_vector.is_empty() {
                0.0
            } else {
                f64::from(cosine_similarity(
                    &query_vector,
                    &embedder.embed(&chunk.text),
                ))
                .clamp(0.0, 1.0)
            };
            let priority = f64::from(sources[chunk.source].priority) / f64::from(PRIORITY_MAX);
            SCORE_WEIGHT_BM25 * bm25
                + SCORE_WEIGHT_COSINE * cosine
                + SCORE_WEIGHT_PRIORITY * priority
        })
        .collect();

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

    // O piso: cada fonte com conteudo recebe ate `floor_per_source` dos
    // seus melhores trechos antes de a pontuacao global decidir o resto.
    // Se os pisos somados nao cabem, cada fonte recebe uma parte igual.
    if spec.floor_per_source > 0 {
        let floor_weight =
            spec.floor_per_source as f64 / TOKEN_SAFETY_FACTOR / spec.calibration.factor;
        let content_weight: Vec<f64> = (0..sources.len())
            .map(|source| {
                kept.iter()
                    .filter(|&&at| chunks[at].source == source)
                    .map(|&at| chunks[at].weight)
                    .sum()
            })
            .collect();
        let with_content = content_weight.iter().filter(|w| **w > 0.0).count().max(1);
        let mut needs: Vec<f64> = content_weight
            .iter()
            .map(|content| content.min(floor_weight))
            .collect();
        let budget_weight = (pack_budget as f64 / spec.calibration.factor / TOKEN_SAFETY_FACTOR
            - frame_weight)
            .max(0.0);
        if needs.iter().sum::<f64>() > budget_weight {
            let share = budget_weight / with_content as f64;
            for need in &mut needs {
                *need = need.min(share);
            }
        }
        // Por rondas: cada fonte mete um trecho por ronda, para a primeira
        // nao gastar o que era das seguintes quando o espaco e pouco.
        let mut queues: Vec<Vec<usize>> = (0..sources.len())
            .map(|source| {
                let mut positions: Vec<usize> = (0..kept.len())
                    .filter(|&position| chunks[kept[position]].source == source)
                    .collect();
                allocator.by_score(&mut positions);
                positions.reverse();
                positions
            })
            .collect();
        loop {
            let mut progressed = false;
            for (source, queue) in queues.iter_mut().enumerate() {
                if allocator.source_weight[source] >= needs[source] {
                    continue;
                }
                while let Some(position) = queue.pop() {
                    if allocator.place(position) {
                        progressed = true;
                        break;
                    }
                }
            }
            if !progressed {
                break;
            }
        }
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
            label: source.label.clone(),
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
    let est_tokens = spec.tokens_of_weight(weight(&canonical(&rendered)));

    Ok(ContextPack {
        rendered,
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
    let mut line = format!(
        "[{}] {}: {}",
        index + 1,
        source.kind.label_pt(),
        if source.label.is_empty() {
            source.id.as_str()
        } else {
            source.label.as_str()
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

/// A alocacao: o que ja esta gasto, o que entrou e a unica pergunta que
/// decide se mais um trecho cabe (`fits`).
struct Allocator<'a> {
    spec: &'a BudgetSpec,
    pack_budget: usize,
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

impl Allocator<'_> {
    /// O teste do orcamento: com `cost` a mais, o pacote continua a caber?
    /// Meio peso de folga, porque a soma por partes e a do texto final so
    /// diferem por arredondamento de virgula flutuante.
    fn fits(&self, cost: f64) -> bool {
        self.spec.tokens_of_weight(self.used + cost + 0.5) <= self.pack_budget
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
            let (left, right) = (&self.chunks[self.kept[a]], &self.chunks[self.kept[b]]);
            self.scores[b]
                .total_cmp(&self.scores[a])
                .then(left.source.cmp(&right.source))
                .then(left.span.start.cmp(&right.span.start))
        });
    }

    /// Mete o trecho `position` (indice em `kept`) inteiro se cabe, ou so
    /// as frases mais pontuadas que cabem; `false` se nada dele entra.
    fn place(&mut self, position: usize) -> bool {
        let chunk = &self.chunks[self.kept[position]];
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
        let Some((text, span, partial)) = compress(chunk, self.query_terms, self.bm25, &room)
        else {
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
        let source = self.chunks[self.kept[position]].source;
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
/// trecho e uma frase so.
fn compress(
    chunk: &Chunk,
    query: &[String],
    bm25: &Bm25,
    room: &dyn Fn(f64) -> bool,
) -> Option<(String, Range<usize>, bool)> {
    let spans = sentence_spans(&chunk.text);
    if spans.len() < 2 {
        return None;
    }
    let mut ranked: Vec<(usize, f64)> = spans
        .iter()
        .enumerate()
        .map(|(index, span)| (index, bm25.score(query, &terms(&chunk.text[span.clone()]))))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut chosen: Vec<usize> = Vec::new();
    let mut total = 0.0;
    for (index, _) in ranked {
        let cost = weight(&chunk.text[spans[index].clone()]) + weight(" ");
        if room(total + cost) {
            total += cost;
            chosen.push(index);
        }
    }
    if chosen.is_empty() {
        return None;
    }
    let partial = chosen.len() < spans.len();
    chosen.sort_unstable();
    let text = chosen
        .iter()
        .map(|&index| &chunk.text[spans[index].clone()])
        .collect::<Vec<_>>()
        .join(" ");
    let first = spans[chosen[0]].start;
    let last = spans[chosen[chosen.len() - 1]].end;
    Some((
        text,
        chunk.span.start + first..chunk.span.start + last,
        partial,
    ))
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

    /// Os tokens do pedaco como mensagem.
    pub fn est_tokens(&self) -> usize {
        self.est_tokens
    }
}

/// Parte `text` (sanitizado para o destino do `spec`) em pedacos que
/// cabem, cada um, em `spec.available()` como mensagem, em fronteiras de
/// frase. A reserva de saida do `spec` deve cobrir a instrucao do mapa e a
/// resposta. Sem texto e `EmptySources`; sem espaco para um trecho,
/// `ModelTooSmall`.
pub fn split_for_map_reduce(text: &str, spec: &BudgetSpec) -> Result<Vec<MapPiece>, ContextError> {
    let text = untrusted::sanitize(text, spec.destination.fence());
    if text.trim().is_empty() {
        return Err(ContextError::EmptySources);
    }
    let Some(limit) = spec
        .available()
        .checked_sub(MESSAGE_OVERHEAD_TOKENS)
        .filter(|limit| *limit >= MIN_PASSAGE_TOKENS)
    else {
        return Err(ContextError::ModelTooSmall {
            limit: spec.max_input,
        });
    };
    let tokens = |piece: &str| spec.tokens(piece);
    Ok(chunk_spans(&text, limit, &tokens)
        .into_iter()
        .map(|span| {
            let text = text[span].to_string();
            let est_tokens = spec.message_tokens(&text);
            MapPiece { text, est_tokens }
        })
        .collect())
}

#[cfg(test)]
mod tests;
