CREATE TABLE schema_version (
    version       INTEGER PRIMARY KEY,
    applied_at    INTEGER NOT NULL,
    app_version   TEXT NOT NULL,
    schema_sha256 TEXT NOT NULL
);

CREATE TABLE research_space (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    description TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    CHECK (updated_at >= created_at)
);

CREATE TABLE research_session (
    id                TEXT PRIMARY KEY,
    research_space_id TEXT REFERENCES research_space(id) ON DELETE SET NULL,
    title             TEXT NOT NULL,
    initial_intent    TEXT NOT NULL,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    CHECK (updated_at >= created_at)
);

CREATE INDEX research_session_space_idx
    ON research_session(research_space_id, updated_at DESC);

CREATE TABLE navigation_observation (
    id                  TEXT PRIMARY KEY,
    research_session_id TEXT REFERENCES research_session(id) ON DELETE SET NULL,
    source_kind         TEXT NOT NULL,
    provider            TEXT,
    url                 TEXT,
    title               TEXT,
    query_text          TEXT,
    sanitized_text      TEXT,
    content_hash        TEXT,
    created_at          INTEGER NOT NULL,
    private             INTEGER NOT NULL DEFAULT 0 CHECK (private = 0)
);

CREATE INDEX navigation_observation_session_idx
    ON navigation_observation(research_session_id, created_at DESC);
CREATE INDEX navigation_observation_url_idx
    ON navigation_observation(url);

CREATE TABLE knowledge_page (
    page_pk             INTEGER PRIMARY KEY,
    id                  TEXT NOT NULL UNIQUE,
    kind                TEXT NOT NULL,
    source_kind         TEXT NOT NULL,
    research_space_id   TEXT REFERENCES research_space(id) ON DELETE SET NULL,
    research_session_id TEXT REFERENCES research_session(id) ON DELETE SET NULL,
    provider            TEXT,
    title               TEXT NOT NULL,
    url                 TEXT,
    body                TEXT NOT NULL,
    content_hash        TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    last_seen_at        INTEGER NOT NULL,
    CHECK (last_seen_at >= created_at)
);

CREATE INDEX knowledge_page_session_idx
    ON knowledge_page(research_session_id, last_seen_at DESC);
CREATE INDEX knowledge_page_space_idx
    ON knowledge_page(research_space_id, last_seen_at DESC);
CREATE INDEX knowledge_page_provider_idx
    ON knowledge_page(provider);
CREATE INDEX knowledge_page_url_idx
    ON knowledge_page(url);
CREATE INDEX knowledge_page_hash_idx
    ON knowledge_page(content_hash);

CREATE VIRTUAL TABLE knowledge_page_fts USING fts5(
    title,
    body,
    provider,
    content='knowledge_page',
    content_rowid='page_pk',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER knowledge_page_ai AFTER INSERT ON knowledge_page BEGIN
    INSERT INTO knowledge_page_fts(rowid, title, body, provider)
    VALUES (new.page_pk, new.title, new.body, coalesce(new.provider, ''));
END;

CREATE TRIGGER knowledge_page_ad AFTER DELETE ON knowledge_page BEGIN
    INSERT INTO knowledge_page_fts(
        knowledge_page_fts, rowid, title, body, provider
    )
    VALUES (
        'delete', old.page_pk, old.title, old.body, coalesce(old.provider, '')
    );
END;

CREATE TRIGGER knowledge_page_au AFTER UPDATE ON knowledge_page BEGIN
    INSERT INTO knowledge_page_fts(
        knowledge_page_fts, rowid, title, body, provider
    )
    VALUES (
        'delete', old.page_pk, old.title, old.body, coalesce(old.provider, '')
    );
    INSERT INTO knowledge_page_fts(rowid, title, body, provider)
    VALUES (new.page_pk, new.title, new.body, coalesce(new.provider, ''));
END;

CREATE TABLE source_relation (
    id                      INTEGER PRIMARY KEY,
    from_page_id            TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    to_page_id              TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    kind                    TEXT NOT NULL,
    evidence_observation_id TEXT REFERENCES navigation_observation(id) ON DELETE SET NULL,
    created_at              INTEGER NOT NULL,
    UNIQUE(from_page_id, to_page_id, kind)
);

CREATE INDEX source_relation_to_idx
    ON source_relation(to_page_id, kind);

CREATE TABLE memory_entity (
    id              TEXT PRIMARY KEY,
    canonical_name  TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    entity_type     TEXT NOT NULL DEFAULT '',
    UNIQUE(normalized_name, entity_type)
);

CREATE TABLE knowledge_page_entity (
    page_id   TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES memory_entity(id) ON DELETE CASCADE,
    weight    REAL NOT NULL DEFAULT 1.0 CHECK (weight >= 0.0),
    PRIMARY KEY(page_id, entity_id)
);

CREATE INDEX knowledge_page_entity_entity_idx
    ON knowledge_page_entity(entity_id, weight DESC);

CREATE TABLE memory_embedding (
    page_id     TEXT NOT NULL REFERENCES knowledge_page(id) ON DELETE CASCADE,
    model_id    TEXT NOT NULL,
    dimension   INTEGER NOT NULL CHECK (dimension > 0),
    vector      BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY(page_id, model_id)
);

CREATE TABLE memory_feedback (
    id                  INTEGER PRIMARY KEY,
    page_id             TEXT REFERENCES knowledge_page(id) ON DELETE CASCADE,
    research_session_id TEXT REFERENCES research_session(id) ON DELETE CASCADE,
    signal              TEXT NOT NULL,
    value               REAL NOT NULL,
    created_at          INTEGER NOT NULL,
    CHECK (page_id IS NOT NULL OR research_session_id IS NOT NULL)
);

CREATE INDEX memory_feedback_page_idx
    ON memory_feedback(page_id, created_at DESC);

CREATE TABLE tombstones (
    id          INTEGER PRIMARY KEY,
    object_type TEXT NOT NULL,
    object_id   TEXT NOT NULL,
    scope       TEXT,
    reason      TEXT,
    created_at  INTEGER NOT NULL,
    UNIQUE(object_type, object_id)
);

CREATE TABLE audit_log (
    id          INTEGER PRIMARY KEY,
    operation   TEXT NOT NULL,
    object_type TEXT NOT NULL,
    object_id   TEXT,
    detail_json TEXT,
    created_at  INTEGER NOT NULL
);

CREATE INDEX audit_log_object_idx
    ON audit_log(object_type, object_id, created_at DESC);
