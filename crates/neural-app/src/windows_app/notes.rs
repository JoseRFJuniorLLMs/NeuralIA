use super::*;

/// O que o editor do painel manda gravar. `id: None` e uma nota nova; um id
/// que chega aqui ja passou por `is_valid_note_id` -- nunca vira caminho sem
/// isso. A fonte nao vem do painel: fica a que a nota ja tinha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NoteEdit {
    pub(super) id: Option<String>,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) tags: Vec<String>,
    /// A revisao da nota que o editor abriu (`note_rev`): se o ficheiro ja
    /// nao e esse -- outra janela do NeuralIA ou o Obsidian gravaram por
    /// cima --, o salvar vai para uma copia em vez de esmagar o outro.
    pub(super) rev: Option<String>,
}

pub(super) const NOTE_TITLE_MAX_CHARS: usize = 300;
/// Tecto do corpo de uma nota, em bytes UTF-8 (o que vai para o disco).
pub(super) const NOTE_BODY_MAX_BYTES: usize = 200 * 1024;
pub(super) const NOTE_TAGS_MAX: usize = 20;
pub(super) const NOTE_TAG_MAX_CHARS: usize = 60;
/// O UNICO pedido do painel que pode passar dos 4 KiB: `note-save`, porque
/// leva o corpo da nota. O `JSON.stringify` do painel escreve cada byte, no
/// pior caso, como `\u00XX` (6 bytes): o tecto cobre um corpo, um titulo e
/// as tags no maximo com esse pior caso, e mais o envelope.
pub(super) const NOTE_SAVE_MESSAGE_MAX_BYTES: usize = 6
    * (NOTE_BODY_MAX_BYTES + 4 * (NOTE_TITLE_MAX_CHARS + NOTE_TAGS_MAX * NOTE_TAG_MAX_CHARS))
    + 1024;

/// Uma linha para o titulo e as tags: cada controlo (o TAB de uma celula de
/// tabela colada, uma quebra de linha, o titulo de uma nota que o Obsidian
/// gravou com TAB) vira um espaco. Antes era recusado, e o note-save morria
/// em silencio.
pub(super) fn note_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `{"id": null | "<id>", "title", "body", "tags": [..]}` e, opcional,
/// `"rev": null | "<16 hex>"` -- e mais nada.
pub(super) fn parse_note_edit(args: &serde_json::Value) -> Option<NoteEdit> {
    let args = args.as_object()?;
    let rev = match args.get("rev") {
        None => None,
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(rev)) if is_note_rev(rev) => Some(rev.clone()),
        Some(_) => return None,
    };
    if args.len() != 4 + usize::from(args.contains_key("rev")) {
        return None;
    }
    let id = match args.get("id")? {
        serde_json::Value::Null => None,
        serde_json::Value::String(id) if is_valid_note_id(id) => Some(id.clone()),
        _ => return None,
    };
    let title = note_line(args.get("title")?.as_str()?);
    if title.chars().count() > NOTE_TITLE_MAX_CHARS {
        return None;
    }
    let body = args.get("body")?.as_str()?;
    if body.len() > NOTE_BODY_MAX_BYTES {
        return None;
    }
    let raw_tags = args.get("tags")?.as_array()?;
    if raw_tags.len() > NOTE_TAGS_MAX {
        return None;
    }
    let mut tags = Vec::with_capacity(raw_tags.len());
    for tag in raw_tags {
        let tag = note_line(tag.as_str()?);
        if tag.chars().count() > NOTE_TAG_MAX_CHARS {
            return None;
        }
        if !tag.is_empty() {
            tags.push(tag);
        }
    }
    Some(NoteEdit {
        id,
        title,
        body: body.to_string(),
        tags,
        rev,
    })
}

/// A revisao de uma nota: FNV-1a de 64 bits do Markdown que ela e no disco.
/// Muda com qualquer mudanca de titulo, corpo, tags, fonte, datas ou das
/// propriedades que o Obsidian la escreveu.
pub(super) fn note_rev(note: &Note) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in note.to_markdown().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub(super) fn is_note_rev(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// O lado nativo segue o que o editor tem por salvar: o `note-draft` mais
/// recente, e nada depois de um `note-save` (o salvar leva o texto todo).
pub(super) fn track_note_draft(draft: &mut Option<NoteEdit>, message: &PanelMessage) {
    match message {
        PanelMessage::NoteDraft(edit) => draft.clone_from(edit),
        PanelMessage::NoteSave(_) => *draft = None,
        PanelMessage::Ready
        | PanelMessage::Search(_)
        | PanelMessage::Open(_)
        | PanelMessage::Close
        | PanelMessage::NotesList
        | PanelMessage::NotesSearch(_)
        | PanelMessage::NoteOpen(_)
        | PanelMessage::NoteSaveRefused
        | PanelMessage::NoteDelete(_)
        | PanelMessage::Downloads(_)
        | PanelMessage::Bookmarks(_) => {}
    }
}

/// O painel fecha por fora (a pagina ja nao corre): o que estava por salvar
/// vai para o disco pelo worker. Uma nota nova cujo primeiro Salvar ainda
/// nao voltou pode sair em duplicado -- nunca perdida.
pub(super) fn take_note_draft_on_close(draft: &mut Option<NoteEdit>) -> Option<NotesCommand> {
    draft.take().map(NotesCommand::Save)
}

// Notas (Zettelkasten): a pasta `<data_dir>/zettel`, em Markdown que o
// Obsidian abre (`neural_core::zettel`). O `ZettelStore` le o disco a cada
// chamada, e `list`, `search` e `backlinks` leem TODAS as notas: numa pasta
// grande isso nao pode correr no event loop. Corre no `ZettelWorker`, e o
// resultado volta como `UserEvent::NotesReady`.

/// Quantas notas a lista do painel recebe de uma vez (a busca refina).
pub(super) const NOTES_LIST_LIMIT: usize = 500;
/// Tecto do texto selecionado que vira nota. O script ja corta, mas quem
/// responde e a pagina: o lado nativo corta outra vez.
pub(super) const NOTE_SELECTION_MAX_CHARS: usize = 20_000;
/// O que se aceita de volta do ExecuteScript antes de o ler: a selecao
/// cortada, escapada em JSON no pior caso, mais o endereco e o titulo.
pub(super) const NOTE_CAPTURE_MAX_BYTES: usize = 512 * 1024;
pub(super) const NOTE_SOURCE_MAX_CHARS: usize = 2048;
/// Sem titulo na pagina, as primeiras palavras da selecao dao o titulo.
pub(super) const NOTE_TITLE_WORDS: usize = 8;
pub(super) const NOTE_FALLBACK_TITLE_MAX_CHARS: usize = 80;

/// Uma nota nova feita pelo NeuralIA (hoje, a partir de uma selecao).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NoteDraft {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) tags: Vec<String>,
    pub(super) source: Option<String>,
}

/// O que o worker das notas faz. Lista fechada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NotesCommand {
    List,
    Search(String),
    Open(String),
    Save(NoteEdit),
    Delete(String),
    Create(NoteDraft),
}

/// Porque e que uma nota vai para o editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteOpened {
    Open,
    Saved,
    Created,
}

impl NoteOpened {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Saved => "saved",
            Self::Created => "created",
        }
    }
}

/// Uma linha da lista (ou dos backlinks): sem o corpo, que a lista nao mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NoteSummary {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) updated_unix: u64,
    pub(super) tags: Vec<String>,
}

impl NoteSummary {
    pub(super) fn of(note: &Note) -> Self {
        Self {
            id: note.id.clone(),
            title: note.title.clone(),
            updated_unix: note.updated_unix,
            tags: note.tags.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NotesReply {
    Listed {
        /// `None` para a lista toda; a busca volta com a consulta, para o
        /// painel ignorar a resposta a uma busca que ja nao e a da caixa.
        query: Option<String>,
        total: usize,
        notes: Vec<NoteSummary>,
    },
    Opened {
        cause: NoteOpened,
        note: Note,
        backlinks: Vec<NoteSummary>,
    },
    Deleted {
        id: String,
    },
    Missing {
        id: String,
    },
    /// A nota `original` mudou fora deste editor desde que ele a abriu; o
    /// que o editor tinha ficou na copia `note`, e a original ficou como o
    /// outro a deixou.
    Conflict {
        original: String,
        note: Note,
    },
    Failed(String),
}

/// De onde veio o pedido: a resposta a uma selecao abre o painel; a do
/// painel so vai para o painel, se ainda estiver aberto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NotesOrigin {
    Panel,
    Selection,
    /// O "Salvar nota" da barra de selecao: a resposta so diz, no aviso do
    /// meio, que a nota ficou salva (`bar_note_notice`) -- sem abrir o
    /// painel. `private`: veio do Split privado.
    Bar {
        private: bool,
    },
    /// O rascunho que o painel deixou ao fechar por fora: a resposta so diz
    /// no aviso do meio se ficou salvo.
    Closed,
}

/// O tecto de bytes de cada acao das notas: so o `note-save` e o
/// `note-draft` levam o corpo de uma nota; o resto fica nos 4 KiB do painel.
pub(super) fn notes_message_max_bytes(action: &str) -> usize {
    if matches!(action, "note-save" | "note-draft") {
        NOTE_SAVE_MESSAGE_MAX_BYTES
    } else {
        PANEL_MESSAGE_MAX_BYTES
    }
}

/// O parser da secao Notas do painel (`PANEL_SECTIONS`): as acoes `note-*`
/// e `notes-*`, com o envelope ja lido e o tamanho ja preso pelo delegador
/// (`parse_panel_message`).
pub(super) fn parse_notes_action(
    action: &str,
    args: Option<&serde_json::Value>,
) -> Option<PanelMessage> {
    // `{"id": "<id valido>"}` e mais nada: um `../x` ou `C:\x` nunca chega ao
    // disco, nem sequer ao worker das notas.
    let note_id = || -> Option<String> {
        let id = exact_keys(args, &["id"])?.get("id")?.as_str()?;
        is_valid_note_id(id).then(|| id.to_string())
    };
    match action {
        "notes-list" => exact_keys(args, &[]).map(|_| PanelMessage::NotesList),
        "notes-search" => {
            panel_text(args, "query", PANEL_QUERY_MAX_CHARS).map(PanelMessage::NotesSearch)
        }
        "note-open" => note_id().map(PanelMessage::NoteOpen),
        "note-delete" => note_id().map(PanelMessage::NoteDelete),
        // Um note-save recusado responde "failed" (`NoteSaveRefused`); o
        // resto do que o parser recusa continua a morrer aqui.
        "note-save" => Some(
            args.and_then(parse_note_edit)
                .map_or(PanelMessage::NoteSaveRefused, PanelMessage::NoteSave),
        ),
        "note-draft" => {
            let args = args?;
            if args.as_object()?.is_empty() {
                Some(PanelMessage::NoteDraft(None))
            } else {
                parse_note_edit(args).map(|edit| PanelMessage::NoteDraft(Some(edit)))
            }
        }
        _ => None,
    }
}

/// O pedido do painel que e das notas. Exaustivo de proposito: uma mensagem
/// nova do painel tem de dizer aqui se e ou nao das notas.
pub(super) fn notes_command_for(message: PanelMessage) -> Option<NotesCommand> {
    Some(match message {
        PanelMessage::NotesList => NotesCommand::List,
        PanelMessage::NotesSearch(query) => NotesCommand::Search(query),
        PanelMessage::NoteOpen(id) => NotesCommand::Open(id),
        PanelMessage::NoteSave(edit) => NotesCommand::Save(edit),
        PanelMessage::NoteDelete(id) => NotesCommand::Delete(id),
        // O rascunho fica do lado nativo (`track_note_draft`); a recusa
        // responde sem passar pelo worker (`NOTE_SAVE_REFUSED`).
        PanelMessage::NoteDraft(_) | PanelMessage::NoteSaveRefused => return None,
        PanelMessage::Ready
        | PanelMessage::Search(_)
        | PanelMessage::Open(_)
        | PanelMessage::Close
        | PanelMessage::Downloads(_)
        | PanelMessage::Bookmarks(_) => {
            return None;
        }
    })
}

/// A resposta a um note-save que o parser recusou.
pub(super) const NOTE_SAVE_REFUSED: &str =
    "A nota não foi salva: o título, as tags ou o tamanho passam dos limites.";

/// O que o worker das notas lembra entre pedidos: a revisao que ELE
/// escreveu em cada nota. Um salvar com a revisao de antes de um salvar
/// nosso (o segundo Ctrl+S antes da resposta ao primeiro) nao e conflito.
#[derive(Debug, Default)]
pub(super) struct NotesSession {
    pub(super) written: std::collections::HashMap<String, String>,
}

/// Um pedido fora de uma sessao (os gates que nao sao sobre conflitos).
#[cfg(test)]
pub(super) fn run_notes_command(
    store: &ZettelStore,
    command: NotesCommand,
    now_unix: u64,
) -> NotesReply {
    run_notes_command_in(&mut NotesSession::default(), store, command, now_unix)
}

/// O trabalho do worker das notas, sem thread nem janela: e isto que os
/// gates correm, sobre uma pasta temporaria.
pub(super) fn run_notes_command_in(
    session: &mut NotesSession,
    store: &ZettelStore,
    command: NotesCommand,
    now_unix: u64,
) -> NotesReply {
    match command {
        NotesCommand::List => notes_listed(store.list(), None),
        NotesCommand::Search(query) => notes_listed(store.search(&query), Some(query)),
        NotesCommand::Open(id) => match store.get(&id) {
            Ok(Some(note)) => notes_opened(store, NoteOpened::Open, note),
            Ok(None) => NotesReply::Missing { id },
            Err(error) => NotesReply::Failed(format!("Não foi possível abrir a nota: {error}")),
        },
        NotesCommand::Save(edit) => {
            let saved = match edit.id {
                None => store
                    .create(&edit.title, &edit.body, edit.tags, None, now_unix)
                    .map(|note| (None, note)),
                // A fonte, as datas e as propriedades que o Obsidian escreveu
                // ficam as do ficheiro. Se a nota foi apagada por fora enquanto
                // estava aberta, grava-se o que o editor tem.
                Some(id) => store.get(&id).and_then(|existing| {
                    // Mudou fora deste editor desde que ele a abriu (outra
                    // janela do NeuralIA, o Obsidian) e nao por um salvar
                    // nosso: o texto do editor vai para uma copia e a nota
                    // fica como o outro a deixou. Antes o salvar esmagava-a.
                    if let (Some(current), Some(opened)) = (&existing, &edit.rev) {
                        let now = note_rev(current);
                        if &now != opened && session.written.get(&id) != Some(&now) {
                            let title = format!("{} (conflito)", edit.title).trim().to_string();
                            return store
                                .create(
                                    &title,
                                    &edit.body,
                                    edit.tags,
                                    current.source.clone(),
                                    now_unix,
                                )
                                .map(|copy| (Some(id), copy));
                        }
                    }
                    let mut note = existing.unwrap_or_else(|| Note {
                        id,
                        ..Note::default()
                    });
                    note.title = edit.title;
                    note.body = edit.body;
                    note.tags = edit.tags;
                    store.save(&note, now_unix).map(|note| (None, note))
                }),
            };
            match saved {
                Ok((conflict, note)) => {
                    session.written.insert(note.id.clone(), note_rev(&note));
                    match conflict {
                        Some(original) => NotesReply::Conflict { original, note },
                        None => notes_opened(store, NoteOpened::Saved, note),
                    }
                }
                Err(error) => {
                    NotesReply::Failed(format!("Não foi possível salvar a nota: {error}"))
                }
            }
        }
        NotesCommand::Delete(id) => match store.delete(&id) {
            Ok(_) => NotesReply::Deleted { id },
            Err(ZettelError::NotFound(_)) => NotesReply::Missing { id },
            Err(error) => NotesReply::Failed(format!("Não foi possível excluir a nota: {error}")),
        },
        NotesCommand::Create(draft) => {
            match store.create(
                &draft.title,
                &draft.body,
                draft.tags,
                draft.source,
                now_unix,
            ) {
                Ok(note) => notes_opened(store, NoteOpened::Created, note),
                Err(error) => NotesReply::Failed(format!("Não foi possível criar a nota: {error}")),
            }
        }
    }
}

pub(super) fn notes_listed(
    result: Result<Vec<Note>, ZettelError>,
    query: Option<String>,
) -> NotesReply {
    match result {
        Ok(notes) => NotesReply::Listed {
            query,
            total: notes.len(),
            notes: notes
                .iter()
                .take(NOTES_LIST_LIMIT)
                .map(NoteSummary::of)
                .collect(),
        },
        Err(error) => NotesReply::Failed(format!("Não foi possível ler as notas: {error}")),
    }
}

pub(super) fn notes_opened(store: &ZettelStore, cause: NoteOpened, note: Note) -> NotesReply {
    // Os backlinks leem a pasta toda; uma falha ali nao esconde a nota.
    let backlinks = zettel::backlinks(store, &note.id)
        .map(|notes| notes.iter().map(NoteSummary::of).collect())
        .unwrap_or_default();
    NotesReply::Opened {
        cause,
        note,
        backlinks,
    }
}

/// O JS que entrega uma resposta ao painel. Os dados vao como JSON
/// (`serde_json::to_string`: um literal JS valido, com aspas, barras e
/// `</script>` escapados) e a pagina so os usa como texto.
pub(super) fn notes_reply_script(reply: &NotesReply) -> String {
    let note_json = |note: &Note| {
        serde_json::json!({
            "id": note.id,
            "title": note.title,
            "body": note.body,
            "tags": note.tags,
            "source": note.source,
            "created": note.created_unix,
            "updated": note.updated_unix,
            "rev": note_rev(note),
        })
    };
    let summary = |note: &NoteSummary| {
        serde_json::json!({
            "id": note.id,
            "title": note.title,
            "updated": note.updated_unix,
            "tags": note.tags,
        })
    };
    let data = match reply {
        NotesReply::Listed {
            query,
            total,
            notes,
        } => serde_json::json!({
            "kind": "listed",
            "query": query.as_deref().unwrap_or(""),
            "total": total,
            "notes": notes.iter().map(summary).collect::<Vec<_>>(),
        }),
        NotesReply::Opened {
            cause,
            note,
            backlinks,
        } => serde_json::json!({
            "kind": "opened",
            "cause": cause.as_str(),
            "note": note_json(note),
            "backlinks": backlinks.iter().map(summary).collect::<Vec<_>>(),
        }),
        NotesReply::Conflict { original, note } => serde_json::json!({
            "kind": "conflict",
            "original": original,
            "note": note_json(note),
        }),
        NotesReply::Deleted { id } => serde_json::json!({ "kind": "deleted", "id": id }),
        NotesReply::Missing { id } => serde_json::json!({ "kind": "missing", "id": id }),
        NotesReply::Failed(message) => serde_json::json!({ "kind": "failed", "message": message }),
    };
    let payload = serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string());
    format!("window.__neuraliaNotes && window.__neuraliaNotes.receive({payload});")
}

pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub(super) struct NotesJob {
    /// `None`: so uma marca na fila (`settle`) -- nada corre e ninguem
    /// recebe resposta.
    pub(super) command: Option<NotesCommand>,
    pub(super) origin: NotesOrigin,
    /// Avisado depois do trabalho feito: e por aqui que a saida da app
    /// espera pela fila (`settle`).
    pub(super) done: Option<SyncSender<()>>,
}

/// A thread das notas morreu: o que estava por salvar nao tem para onde ir.
pub(super) const NOTES_WORKER_GONE: &str =
    "Não foi possível salvar a nota: as notas deixaram de responder.";

/// A resposta a um pedido que pode esperar pelo utilizador (listar, buscar,
/// abrir, excluir, criar da selecao) com a fila cheia.
pub(super) const NOTES_BUSY: &str = "As notas estão ocupadas; tente de novo.";

/// Quantos pedidos (na fila ou a correr) as notas aceitam antes de
/// responder `NOTES_BUSY` a um pedido que se pode repetir. O texto do
/// utilizador (`ZettelWorker::keep`) nao conta com este tecto.
pub(super) const NOTES_QUEUE_LIMIT: usize = 64;

/// Quanto a saida da app espera pelas notas (um disco que nao responde nao
/// prende o fecho da janela para sempre).
pub(super) const NOTES_EXIT_WAIT: Duration = Duration::from_secs(3);

/// Uma thread para as notas, como o historico e a memoria, e UMA fila, por
/// ordem de chegada: um salvar nunca passa a frente do abrir que o
/// antecedeu, o rascunho de um fecho nunca passa a frente do salvar de
/// antes, e a marca do `settle` so volta depois de tudo o que entrou antes
/// dela -- o rascunho do fecho da janela incluido. O event loop nunca le a
/// pasta nem espera pela fila.
#[derive(Clone)]
pub(super) struct ZettelWorker {
    pub(super) tx: Sender<NotesJob>,
    /// Pedidos na fila ou a correr; a thread desconta cada um depois de o
    /// acabar.
    pub(super) queued: Arc<AtomicUsize>,
}

impl ZettelWorker {
    pub(super) fn new(dir: std::path::PathBuf, proxy: EventLoopProxy<UserEvent>) -> Self {
        Self::spawn(dir, move |origin, reply| {
            let _ = proxy.send_event(UserEvent::NotesReady { origin, reply });
        })
    }

    /// A thread das notas; cada resposta vai para `reply` (no app, o event
    /// loop; nos gates, um canal).
    pub(super) fn spawn(
        dir: std::path::PathBuf,
        reply: impl Fn(NotesOrigin, NotesReply) + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel::<NotesJob>();
        let queued = Arc::new(AtomicUsize::new(0));
        let pending = Arc::clone(&queued);
        let _ = thread::Builder::new()
            .name("neural-zettel".into())
            .spawn(move || {
                let mut session = NotesSession::default();
                while let Ok(job) = rx.recv() {
                    if let Some(command) = job.command {
                        // `open` so cria a pasta se faltar; abrir a cada
                        // pedido aguenta a pasta ter sido apagada com o app
                        // aberto.
                        let answer = match ZettelStore::open(&dir) {
                            Ok(store) => {
                                run_notes_command_in(&mut session, &store, command, unix_now())
                            }
                            Err(error) => NotesReply::Failed(format!(
                                "Não foi possível abrir a pasta das notas: {error}"
                            )),
                        };
                        reply(job.origin, answer);
                    }
                    if let Some(done) = job.done {
                        let _ = done.try_send(());
                    }
                    pending.fetch_sub(1, Ordering::SeqCst);
                }
            });
        Self { tx, queued }
    }

    /// Um pedido do painel ou de uma pagina. Nunca espera. Um salvar leva o
    /// texto todo, e o lado nativo ja largou a copia dele
    /// (`track_note_draft`): entra sempre (`keep`), mesmo com a fila cheia
    /// -- o X do painel manda o salvar e logo a seguir o `close`, e a pagina
    /// ja nao esta la para tentar outra vez. O resto, com a fila cheia,
    /// volta ja com `NOTES_BUSY`, para quem pediu repetir.
    pub(super) fn submit(&self, command: NotesCommand, origin: NotesOrigin) -> Result<(), String> {
        let job = NotesJob {
            command: Some(command),
            origin,
            done: None,
        };
        if matches!(job.command, Some(NotesCommand::Save(_))) {
            return self.keep(job);
        }
        if self
            .queued
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |queued| {
                (queued < NOTES_QUEUE_LIMIT).then_some(queued + 1)
            })
            .is_err()
        {
            return Err(NOTES_BUSY.to_string());
        }
        self.tx.send(job).map_err(|_| {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            NOTES_WORKER_GONE.to_string()
        })
    }

    /// Poe `job` no fim da fila, sem tecto e sem esperar: nem o event loop
    /// fica preso a um disco que nao responde, nem o texto e deitado fora,
    /// nem passa a frente (ou fica atras) do que entrou antes dele. So a
    /// thread das notas morta o perde -- e ai o erro diz.
    pub(super) fn keep(&self, job: NotesJob) -> Result<(), String> {
        self.queued.fetch_add(1, Ordering::SeqCst);
        self.tx.send(job).map_err(|_| {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            NOTES_WORKER_GONE.to_string()
        })
    }
}

/// O painel do Ctrl+H grava o que o editor tinha por salvar por aqui.
impl super::side_panel::DraftRescue for ZettelWorker {
    fn rescue(&self, command: NotesCommand) -> Result<(), String> {
        self.keep(NotesJob {
            command: Some(command),
            origin: NotesOrigin::Closed,
            done: None,
        })
    }

    /// A marca vai pela MESMA fila que o `rescue` (`keep`): so volta depois
    /// de tudo o que entrou antes dela estar no disco.
    fn settle(&self, limit: Duration) {
        let (done, finished) = sync_channel(1);
        let barrier = NotesJob {
            command: None,
            origin: NotesOrigin::Closed,
            done: Some(done),
        };
        if self.keep(barrier).is_ok() {
            let _ = finished.recv_timeout(limit);
        }
    }
}

/// A selecao nao pode virar nota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteCaptureError {
    /// Nada selecionado (so espacos conta como nada).
    EmptySelection,
    /// A resposta nao e o objeto que o script devolve.
    Unreadable,
}

/// O endereco que fica como fonte: so paginas da web, e nunca a origem do
/// nosso visualizador de PDF.
pub(super) fn note_source(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.chars().count() > NOTE_SOURCE_MAX_CHARS {
        return None;
    }
    let parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.origin().ascii_serialization() == PDF_ORIGIN
    {
        return None;
    }
    Some(parsed.to_string())
}

/// Uma linha, sem controlos nem espacos repetidos, com no maximo `max` chars.
pub(super) fn one_line(text: &str, max: usize) -> String {
    let joined = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(max).collect()
}

/// A nota que o Ctrl+Shift+Z cria: titulo = o da pagina (ou as primeiras
/// palavras da selecao); corpo = a selecao como citacao Markdown, uma linha
/// em branco e "Fonte: <url>"; fonte = o endereco; tag "web".
///
/// `raw` e o JSON que o `NOTE_CAPTURE_SCRIPT` devolveu -- dado da pagina.
/// `source` e o endereco que o lado nativo conhece e a pagina nao (o
/// artigo do Leitor, o PDF aberto); quando existe, manda ele.
pub(super) fn note_draft_from_capture(
    raw: &str,
    source: Option<&str>,
) -> Result<NoteDraft, NoteCaptureError> {
    let value = note_capture_value(raw)?;
    let selection = note_capture_selection(&value)?;
    let field = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    };
    let source = match source {
        Some(known) => note_source(known),
        None => note_source(field("url")),
    };
    let title = Some(one_line(field("title"), NOTE_TITLE_MAX_CHARS))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            let words = selection
                .split_whitespace()
                .take(NOTE_TITLE_WORDS)
                .collect::<Vec<_>>()
                .join(" ");
            one_line(&words, NOTE_FALLBACK_TITLE_MAX_CHARS)
        });
    Ok(quoted_note(&selection, title, source))
}

/// A resposta do `NOTE_CAPTURE_SCRIPT` lida como o objeto que ele devolve;
/// grande demais, ou outra coisa, nao se le.
pub(super) fn note_capture_value(raw: &str) -> Result<serde_json::Value, NoteCaptureError> {
    if raw.len() > NOTE_CAPTURE_MAX_BYTES {
        return Err(NoteCaptureError::Unreadable);
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| NoteCaptureError::Unreadable)?;
    if !value.is_object() {
        return Err(NoteCaptureError::Unreadable);
    }
    Ok(value)
}

/// A selecao da resposta, cortada outra vez aqui (quem responde e a
/// pagina), em LF e aparada; so espacos conta como nada.
pub(super) fn note_capture_selection(
    value: &serde_json::Value,
) -> Result<String, NoteCaptureError> {
    note_selection(
        value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    )
}

/// O texto de uma nota: em LF, cortado a `NOTE_SELECTION_MAX_CHARS` e
/// aparado; so espacos conta como nada.
pub(super) fn note_selection(text: &str) -> Result<String, NoteCaptureError> {
    let selection: String = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .take(NOTE_SELECTION_MAX_CHARS)
        .collect();
    let selection = selection.trim();
    if selection.is_empty() {
        return Err(NoteCaptureError::EmptySelection);
    }
    Ok(selection.to_string())
}

/// A nota de uma selecao: a citacao Markdown, uma linha em branco e
/// "Fonte: <url>" (so com fonte), a tag "web".
pub(super) fn quoted_note(selection: &str, title: String, source: Option<String>) -> NoteDraft {
    let quote = selection
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                ">".to_string()
            } else {
                format!("> {}", line.trim_end())
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let body = match &source {
        Some(url) => format!("{quote}\n\nFonte: {url}\n"),
        None => format!("{quote}\n"),
    };
    NoteDraft {
        title,
        body,
        tags: vec!["web".to_string()],
        source,
    }
}

/// Quantos caracteres do inicio da selecao dao o titulo de uma nota do
/// "Salvar nota" -- e o aviso "Nota salva: ...".
pub(super) const BAR_NOTE_TITLE_CHARS: usize = 60;
/// Um "Salvar nota" com o mesmo texto antes disto nao grava outra nota.
pub(super) const BAR_NOTE_REPEAT: Duration = Duration::from_secs(2);

/// O titulo de uma nota do "Salvar nota": os primeiros 60 caracteres da
/// selecao numa linha, com "…" se ela continua. Vem do texto, nunca do
/// `document.title` que a pagina escolhe.
pub(super) fn bar_note_title(selection: &str) -> String {
    let line = one_line(selection, BAR_NOTE_TITLE_CHARS + 1);
    if line.chars().count() <= BAR_NOTE_TITLE_CHARS {
        return line;
    }
    let start: String = line.chars().take(BAR_NOTE_TITLE_CHARS).collect();
    // Na ultima palavra inteira, se nao ficar curto demais.
    let whole_words = line.chars().nth(BAR_NOTE_TITLE_CHARS) == Some(' ');
    let cut = match start.rfind(' ') {
        Some(at) if !whole_words && at >= start.len() / 2 => &start[..at],
        _ => start.as_str(),
    };
    format!("{}…", cut.trim_end())
}

/// O aviso do meio da janela depois de um "Salvar nota" gravado.
pub(super) fn bar_note_notice(private: bool, title: &str) -> String {
    if private {
        "Modo privado: a nota foi guardada".to_string()
    } else {
        format!("Nota salva: {title}")
    }
}

/// Os textos que o "Salvar nota" (ou o Ctrl+Shift+Z, noutra guarda) gravou
/// nos ultimos `BAR_NOTE_REPEAT`, e quando: o mesmo texto outra vez antes
/// disso nao e outra nota -- tambem com outro texto gravado pelo meio.
#[derive(Debug, Default)]
pub(super) struct BarNoteGuard {
    pub(super) recent: Vec<(String, Instant)>,
}

impl BarNoteGuard {
    /// `true` e fica registado; um repetido nao conta como novo (nem adia a
    /// vez seguinte). So guarda o que ainda esta dentro da janela.
    pub(super) fn admit(&mut self, selection: &str, now: Instant) -> bool {
        self.recent
            .retain(|(_, at)| now.saturating_duration_since(*at) < BAR_NOTE_REPEAT);
        if self.recent.iter().any(|(saved, _)| saved == selection) {
            return false;
        }
        self.recent.push((selection.to_string(), now));
        true
    }
}

/// O que um "Salvar nota" (ou um Ctrl+Shift+Z) faz com o texto que chegou.
#[derive(Debug, PartialEq)]
pub(super) enum BarNoteStep {
    /// Gravar esta nota nova (`NotesCommand::Create`), e so isso.
    Save(NoteDraft),
    /// O mesmo texto ha menos de 2 s: nada.
    Repeated,
    Refused(NoteCaptureError),
}

/// A decisao do "Salvar nota", sem janela. `text` e o que a barra mostrava
/// e mandou no pedido (`NoteVia::Bar`) -- nunca uma resposta que a pagina de
/// depois. A fonte e `native_source`, o endereco que o nativo conhece da
/// WebView (`note_capture_source`), e o titulo o inicio do texto.
pub(super) fn bar_note_step(
    guard: &mut BarNoteGuard,
    text: &str,
    native_source: Option<&str>,
    now: Instant,
) -> BarNoteStep {
    let selection = match note_selection(text) {
        Ok(selection) => selection,
        Err(error) => return BarNoteStep::Refused(error),
    };
    if !guard.admit(&selection, now) {
        return BarNoteStep::Repeated;
    }
    BarNoteStep::Save(quoted_note(
        &selection,
        bar_note_title(&selection),
        native_source.and_then(note_source),
    ))
}

/// A decisao do Ctrl+Shift+Z, sem janela: a nota da resposta da pagina
/// (`note_draft_from_capture`) e, como no Salvar nota, a mesma nota outra
/// vez em menos de 2 s nao e outra -- a tecla presa repete o keydown.
pub(super) fn shortcut_note_step(
    guard: &mut BarNoteGuard,
    raw: &str,
    source: Option<&str>,
    now: Instant,
) -> BarNoteStep {
    match note_draft_from_capture(raw, source) {
        Ok(draft) if guard.admit(&draft.body, now) => BarNoteStep::Save(draft),
        Ok(_) => BarNoteStep::Repeated,
        Err(error) => BarNoteStep::Refused(error),
    }
}

/// A fonte que acompanha a leitura da selecao. No Ctrl+Shift+Z, so a do
/// Leitor e a do PDF (`note_page_source`); nas outras paginas vale o
/// `location.href` que o script devolve. No "Salvar nota" da barra e sempre
/// uma que o nativo conhece: essa, ou o endereco da propria WebView
/// (`webview_url`, o `Source` do WebView2) -- nunca um que a pagina diga.
pub(super) fn note_capture_source(
    via: &NoteVia,
    target: Option<PageTarget>,
    surface: Surface,
    page_source: Option<&str>,
    webview_url: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let known = note_page_source(target, surface, page_source);
    match via {
        NoteVia::Shortcut => known,
        NoteVia::Bar { .. } => known.or_else(webview_url),
    }
}

/// De que WebView e a nota de um Ctrl+Shift+Z (que le a selecao dela) ou de
/// um Salvar nota (que traz o texto e so pede a fonte e o modo privado).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoteCapture {
    Read,
    /// O Ctrl+Shift+Z no Split privado: nada se le, nada se grava.
    RefusePrivate,
    /// A WebView ja nao existe (o Split fechou entretanto).
    NoPage,
}

/// Decide no momento de ler, nao no do pedido: entre o Ctrl+Shift+Z num
/// Split normal e o evento chegar aqui, o Split pode ter sido trocado por um
/// privado. `split_private` e o do Split que existe AGORA. O "Salvar nota"
/// da barra le tambem o Split privado: e um pedido explicito de quem le.
pub(super) fn note_capture_decision(
    target: Option<PageTarget>,
    via: &NoteVia,
    split_private: Option<bool>,
) -> NoteCapture {
    match target {
        Some(PageTarget::Split) => match split_private {
            Some(true) if *via == NoteVia::Shortcut => NoteCapture::RefusePrivate,
            Some(_) => NoteCapture::Read,
            None => NoteCapture::NoPage,
        },
        Some(PageTarget::Column(_)) | None => NoteCapture::Read,
    }
}

/// A WebView de que um Ctrl+Shift+Z le a selecao (e de que um Salvar nota
/// tira a fonte), no momento de ler: a da coluna que o pediu (nunca a
/// vizinha), a do Split que
/// existe AGORA -- e, no Ctrl+Shift+Z, nunca se ele for privado -- ou a
/// WebView unica (Externo, Leitor, PDF). `columns` e `split` sao os do
/// proprio comparador (`comp.views`, `comp.split`): o `private` e lido aqui,
/// do Split, e nao passado a parte -- e volta com a WebView, para o aviso do
/// "Salvar nota" dizer que foi no modo privado. Generica para o gate a correr
/// sem WebViews.
pub(super) fn note_read_view<'a, V>(
    target: Option<PageTarget>,
    via: &NoteVia,
    columns: &'a [ComparatorView<V>],
    split: Option<&'a SplitView<V>>,
    main: Option<&'a V>,
) -> Result<NoteRead<'a, V>, NoteCapture> {
    match note_capture_decision(target, via, split.map(|split| split.private)) {
        NoteCapture::Read => {}
        refused => return Err(refused),
    }
    let read = |view: &'a V, private: bool| NoteRead { view, private };
    match target {
        Some(PageTarget::Column(index)) => columns
            .get(index)
            .map(|column| read(&column.webview, false)),
        Some(PageTarget::Split) => split.map(|split| read(&split.webview, split.private)),
        None => main.map(|view| read(view, false)),
    }
    .ok_or(NoteCapture::NoPage)
}

/// A WebView de que se le a selecao e se ela e a do Split privado (so o
/// "Salvar nota" chega a le-la; o aviso diz que a nota foi guardada no modo
/// privado).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct NoteRead<'a, V> {
    pub(super) view: &'a V,
    pub(super) private: bool,
}

/// O aviso do painel privado.
pub(super) const NOTE_PRIVATE_REFUSAL: &str = "Modo privado: notas não são criadas";

/// A fonte que o lado nativo conhece e a pagina nao: o artigo do Leitor (o
/// HTML e local) e o PDF (o visualizador e nosso). So vale para a WebView
/// unica dessas superficies; nas colunas e no Split manda o endereco da pagina.
pub(super) fn note_page_source(
    target: Option<PageTarget>,
    surface: Surface,
    page_source: Option<&str>,
) -> Option<String> {
    match (target, surface) {
        (None, Surface::Reader | Surface::Pdf) => page_source.map(str::to_string),
        _ => None,
    }
}
