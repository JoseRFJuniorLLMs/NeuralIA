use std::{fs, io, path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{MemoryDocument, MemoryRelation};
use crate::research::ResearchSession;

const SCHEMA_VERSION: i64 = 1;
const SCHEMA_V01: &str = include_str!("schema_v01.sql");
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn schema_hash() -> String {
    format!("{:x}", Sha256::digest(SCHEMA_V01.as_bytes()))
}

fn remove_sqlite_sidecars(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(format!("{}-wal", path.to_string_lossy()));
    let _ = fs::remove_file(format!("{}-shm", path.to_string_lossy()));
}

fn table_exists(connection: &Connection, name: &str) -> io::Result<bool> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1 LIMIT 1",
            [name],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(io_error)
}

fn has_only_legacy_mirror_schema(connection: &Connection) -> io::Result<bool> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type IN ('table','view')
               AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(io_error)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(io_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error)?;

    if names.is_empty() {
        return Err(io::Error::other("empty schema is not legacy"));
    }
    Ok(names
        .iter()
        .all(|name| matches!(name.as_str(), "schema_meta" | "documents" | "memory_fts")))
}

fn configure_connection(connection: &Connection) -> io::Result<()> {
    connection.busy_timeout(BUSY_TIMEOUT).map_err(io_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(io_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(io_error)?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(io_error)?;

    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(io_error)?;
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(io_error)?;
    if foreign_keys != 1 || !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(io::Error::other(
            "SQLite memory index did not enable foreign_keys/WAL",
        ));
    }
    Ok(())
}

fn ensure_schema(connection: &mut Connection) -> io::Result<()> {
    if table_exists(connection, "schema_version")? {
        let (version, hash): (i64, String) = connection
            .query_row(
                "SELECT version, schema_sha256
                 FROM schema_version
                 ORDER BY version DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(io_error)?;
        if version != SCHEMA_VERSION {
            return Err(io::Error::other(format!(
                "unsupported memory schema version {version}"
            )));
        }
        let expected = schema_hash();
        if hash != expected {
            return Err(io::Error::other(
                "memory schema hash mismatch; derived index must be rebuilt",
            ));
        }
        return Ok(());
    }

    let user_table_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master
             WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(io_error)?;
    if user_table_count != 0 {
        return Err(io::Error::other(
            "partial or legacy memory schema requires rebuild",
        ));
    }

    let hash = schema_hash();
    let now = super::unix_seconds() as i64;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io_error)?;
    transaction.execute_batch(SCHEMA_V01).map_err(io_error)?;
    transaction
        .execute(
            "INSERT INTO schema_version(version, applied_at, app_version, schema_sha256)
             VALUES (?1, ?2, ?3, ?4)",
            params![SCHEMA_VERSION, now, env!("CARGO_PKG_VERSION"), hash],
        )
        .map_err(io_error)?;

    let integrity: String = transaction
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(io_error)?;
    if integrity != "ok" {
        return Err(io::Error::other(format!(
            "SQLite integrity_check failed while creating V01: {integrity}"
        )));
    }
    transaction.commit().map_err(io_error)
}

fn open_ready(path: &Path) -> io::Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut connection = Connection::open(path).map_err(io_error)?;
    configure_connection(&connection)?;

    match ensure_schema(&mut connection) {
        Ok(()) => Ok(connection),
        Err(error)
            if error
                .to_string()
                .contains("partial or legacy memory schema requires rebuild")
                && has_only_legacy_mirror_schema(&connection).unwrap_or(false) =>
        {
            drop(connection);
            remove_sqlite_sidecars(path);
            let mut connection = Connection::open(path).map_err(io_error)?;
            configure_connection(&connection)?;
            ensure_schema(&mut connection)?;
            Ok(connection)
        }
        Err(error) => Err(error),
    }
}

fn enum_text(value: &impl Serialize) -> io::Result<String> {
    serde_json::to_value(value)
        .map_err(io_error)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("memory enum did not serialize as text"))
}

fn upsert_session(transaction: &Transaction<'_>, session: &ResearchSession) -> io::Result<()> {
    transaction
        .execute(
            "INSERT INTO research_session(
                id, research_space_id, title, initial_intent, created_at, updated_at
             ) VALUES (?1, NULL, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                title=excluded.title,
                initial_intent=excluded.initial_intent,
                updated_at=excluded.updated_at",
            params![
                session.id,
                session.title,
                session.question,
                session.created_at as i64,
                session.updated_at as i64
            ],
        )
        .map_err(io_error)?;
    Ok(())
}

fn known_session_id<'a>(
    transaction: &Transaction<'_>,
    document: &'a MemoryDocument,
) -> io::Result<Option<&'a str>> {
    let Some(id) = document.session_id.as_deref() else {
        return Ok(None);
    };
    let exists = transaction
        .query_row(
            "SELECT 1 FROM research_session WHERE id=?1 LIMIT 1",
            [id],
            |_| Ok(()),
        )
        .optional()
        .map_err(io_error)?
        .is_some();

    if !exists {
        transaction
            .execute(
                "INSERT INTO audit_log(operation, object_type, object_id, detail_json, created_at)
                 VALUES ('orphan-session-reference', 'knowledge_page', ?1, ?2, ?3)",
                params![
                    document.id,
                    serde_json::json!({"legacy_session_id": id}).to_string(),
                    super::unix_seconds() as i64
                ],
            )
            .map_err(io_error)?;
        return Ok(None);
    }
    Ok(Some(id))
}

fn embedding_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn entity_id(normalized: &str, entity_type: &str) -> String {
    let material = format!("entity\0{normalized}\0{entity_type}");
    format!("{:x}", Sha256::digest(material.as_bytes()))[..24].to_string()
}

fn upsert_page_base(transaction: &Transaction<'_>, document: &MemoryDocument) -> io::Result<()> {
    if document.private {
        return Err(io::Error::other(
            "private document reached SQLite storage boundary",
        ));
    }

    let session_id = known_session_id(transaction, document)?;
    let kind = enum_text(&document.kind)?;
    let source_kind = enum_text(&document.source_kind)?;

    transaction
        .execute(
            "INSERT INTO knowledge_page(
                id, kind, source_kind, research_space_id, research_session_id,
                provider, title, url, body, content_hash, created_at, last_seen_at
             ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                kind=excluded.kind,
                source_kind=excluded.source_kind,
                research_session_id=excluded.research_session_id,
                provider=excluded.provider,
                title=excluded.title,
                url=excluded.url,
                body=excluded.body,
                content_hash=excluded.content_hash,
                created_at=excluded.created_at,
                last_seen_at=excluded.last_seen_at",
            params![
                document.id,
                kind,
                source_kind,
                session_id,
                document.provider,
                document.title,
                document.url,
                document.body,
                document.content_hash,
                document.created_at as i64,
                document.last_seen_at as i64
            ],
        )
        .map_err(io_error)?;

    transaction
        .execute(
            "DELETE FROM knowledge_page_entity WHERE page_id=?1",
            [&document.id],
        )
        .map_err(io_error)?;
    for canonical in &document.entities {
        let canonical = canonical.trim();
        if canonical.is_empty() {
            continue;
        }
        let normalized = canonical.to_lowercase();
        let entity_type = "";
        let id = entity_id(&normalized, entity_type);
        transaction
            .execute(
                "INSERT INTO memory_entity(id, canonical_name, normalized_name, entity_type)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(normalized_name, entity_type) DO UPDATE SET
                    canonical_name=excluded.canonical_name",
                params![id, canonical, normalized, entity_type],
            )
            .map_err(io_error)?;
        let stored_id: String = transaction
            .query_row(
                "SELECT id FROM memory_entity
                 WHERE normalized_name=?1 AND entity_type=?2",
                params![normalized, entity_type],
                |row| row.get(0),
            )
            .map_err(io_error)?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO knowledge_page_entity(page_id, entity_id, weight)
                 VALUES (?1, ?2, 1.0)",
                params![document.id, stored_id],
            )
            .map_err(io_error)?;
    }

    transaction
        .execute(
            "DELETE FROM memory_embedding WHERE page_id=?1",
            [&document.id],
        )
        .map_err(io_error)?;
    if !document.embedding.is_empty() {
        let vector = embedding_bytes(&document.embedding);
        transaction
            .execute(
                "INSERT INTO memory_embedding(page_id, model_id, dimension, vector, created_at)
                 VALUES (?1, 'hashing-v1-384', ?2, ?3, ?4)",
                params![
                    document.id,
                    document.embedding.len() as i64,
                    vector,
                    document.last_seen_at as i64
                ],
            )
            .map_err(io_error)?;
    }

    Ok(())
}

fn replace_relations(transaction: &Transaction<'_>, document: &MemoryDocument) -> io::Result<()> {
    transaction
        .execute(
            "DELETE FROM source_relation WHERE from_page_id=?1",
            [&document.id],
        )
        .map_err(io_error)?;

    for MemoryRelation { kind, target_id } in &document.relations {
        let target_exists = transaction
            .query_row(
                "SELECT 1 FROM knowledge_page WHERE id=?1 LIMIT 1",
                [target_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(io_error)?
            .is_some();
        if !target_exists {
            transaction
                .execute(
                    "INSERT INTO audit_log(operation, object_type, object_id, detail_json, created_at)
                     VALUES ('orphan-relation', 'knowledge_page', ?1, ?2, ?3)",
                    params![
                        document.id,
                        serde_json::json!({"target_id": target_id, "kind": kind}).to_string(),
                        super::unix_seconds() as i64
                    ],
                )
                .map_err(io_error)?;
            continue;
        }

        transaction
            .execute(
                "INSERT OR IGNORE INTO source_relation(
                    from_page_id, to_page_id, kind, evidence_observation_id, created_at
                 ) VALUES (?1, ?2, ?3, NULL, ?4)",
                params![document.id, target_id, kind, document.last_seen_at as i64],
            )
            .map_err(io_error)?;
    }
    Ok(())
}

fn validate_integrity(transaction: &Transaction<'_>) -> io::Result<()> {
    let integrity: String = transaction
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(io_error)?;
    if integrity != "ok" {
        return Err(io::Error::other(format!(
            "SQLite integrity_check failed: {integrity}"
        )));
    }

    let page_count: i64 = transaction
        .query_row("SELECT count(*) FROM knowledge_page", [], |row| row.get(0))
        .map_err(io_error)?;
    let fts_count: i64 = transaction
        .query_row("SELECT count(*) FROM knowledge_page_fts", [], |row| {
            row.get(0)
        })
        .map_err(io_error)?;
    if page_count != fts_count {
        return Err(io::Error::other(format!(
            "FTS/content count mismatch: pages={page_count} fts={fts_count}"
        )));
    }
    Ok(())
}

pub(super) fn upsert(
    path: &Path,
    document: &MemoryDocument,
    session: Option<&ResearchSession>,
) -> io::Result<()> {
    let mut connection = open_ready(path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io_error)?;
    if let Some(session) = session {
        upsert_session(&transaction, session)?;
    }
    upsert_page_base(&transaction, document)?;
    replace_relations(&transaction, document)?;
    validate_integrity(&transaction)?;
    transaction.commit().map_err(io_error)
}

pub(super) fn rebuild(
    path: &Path,
    documents: &[MemoryDocument],
    sessions: &[ResearchSession],
) -> io::Result<()> {
    let mut connection = open_ready(path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(io_error)?;

    transaction
        .execute_batch(
            "DELETE FROM source_relation;
             DELETE FROM knowledge_page_entity;
             DELETE FROM memory_embedding;
             DELETE FROM knowledge_page;
             DELETE FROM navigation_observation;
             DELETE FROM research_session;
             DELETE FROM research_space;
             DELETE FROM memory_entity;
             DELETE FROM audit_log;",
        )
        .map_err(io_error)?;

    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO research_session(
                    id, research_space_id, title, initial_intent, created_at, updated_at
                 ) VALUES (?1, NULL, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    title=excluded.title,
                    initial_intent=excluded.initial_intent,
                    updated_at=excluded.updated_at",
            )
            .map_err(io_error)?;
        for session in sessions {
            statement
                .execute(params![
                    session.id,
                    session.title,
                    session.question,
                    session.created_at as i64,
                    session.updated_at as i64
                ])
                .map_err(io_error)?;
        }
    }

    for document in documents {
        upsert_page_base(&transaction, document)?;
    }
    for document in documents {
        replace_relations(&transaction, document)?;
    }

    transaction
        .execute(
            "INSERT INTO audit_log(operation, object_type, detail_json, created_at)
             VALUES ('rebuild', 'memory-index', ?1, ?2)",
            params![
                serde_json::json!({
                    "documents": documents.len(),
                    "sessions": sessions.len()
                })
                .to_string(),
                super::unix_seconds() as i64
            ],
        )
        .map_err(io_error)?;

    validate_integrity(&transaction)?;
    transaction.commit().map_err(io_error)
}

#[cfg(test)]
fn inspect_pragmas(path: &Path) -> io::Result<(i64, String, i64)> {
    let connection = open_ready(path)?;
    let foreign_keys = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(io_error)?;
    let journal_mode = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(io_error)?;
    let synchronous = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(io_error)?;
    Ok((foreign_keys, journal_mode, synchronous))
}

#[cfg(test)]
fn fts_count(path: &Path, query: &str) -> io::Result<i64> {
    let connection = open_ready(path)?;
    connection
        .query_row(
            "SELECT count(*) FROM knowledge_page_fts WHERE knowledge_page_fts MATCH ?1",
            [query],
            |row| row.get(0),
        )
        .map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryKind, MemorySourceKind};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!(
                "neuralia-sqlite-v01-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
            .join("memory.sqlite")
    }

    fn document(body: &str) -> MemoryDocument {
        MemoryDocument::new(
            MemoryKind::Source,
            MemorySourceKind::Web,
            "Página",
            Some("https://example.com".into()),
            body,
        )
    }

    #[test]
    fn fresh_db_applies_pragmas_before_schema_transaction_and_reopens() {
        let path = temp_path("pragmas");
        rebuild(&path, &[], &[]).unwrap();
        let (foreign_keys, journal_mode, synchronous) = inspect_pragmas(&path).unwrap();
        assert_eq!(foreign_keys, 1);
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(synchronous, 1);

        let connection = open_ready(&path).unwrap();
        let (version, hash): (i64, String) = connection
            .query_row(
                "SELECT version, schema_sha256 FROM schema_version",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(version, 1);
        assert_eq!(hash, schema_hash());

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn fts_triggers_follow_insert_update_and_delete() {
        let path = temp_path("fts");
        let mut first = document("café rust texto vetorial");
        upsert(&path, &first, None).unwrap();
        assert_eq!(fts_count(&path, "cafe").unwrap(), 1);

        first.body = "python novo outro corpus".into();
        first.content_hash = format!("{:x}", Sha256::digest(first.body.as_bytes()));
        first.last_seen_at = first.last_seen_at.saturating_add(1);
        upsert(&path, &first, None).unwrap();
        assert_eq!(fts_count(&path, "cafe").unwrap(), 0);
        assert_eq!(fts_count(&path, "python").unwrap(), 1);

        rebuild(&path, &[], &[]).unwrap();
        assert_eq!(fts_count(&path, "python").unwrap(), 0);

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn orphan_session_is_not_fabricated() {
        let path = temp_path("orphan-session");
        let doc = document("texto").session("missing-session");
        upsert(&path, &doc, None).unwrap();

        let connection = open_ready(&path).unwrap();
        let stored: Option<String> = connection
            .query_row(
                "SELECT research_session_id FROM knowledge_page WHERE id=?1",
                [&doc.id],
                |row| row.get(0),
            )
            .unwrap();
        let fake_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM research_session WHERE id='missing-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let audit_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM audit_log
                 WHERE operation='orphan-session-reference'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(stored, None);
        assert_eq!(fake_count, 0);
        assert_eq!(audit_count, 1);

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn private_document_is_rejected_at_sqlite_boundary() {
        let path = temp_path("private");
        let doc = document("secret").private(true);
        assert!(upsert(&path, &doc, None).is_err());

        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }
}
