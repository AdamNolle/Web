use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use chrono::{Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::connectors::{
    CommentCompleteness, NormalizedComment, NormalizedPost, PageFinality, SourceKind,
    SourceSyncSpec, SyncBatch, SyncPage, validate_source_sync_spec,
};
use crate::domain::{
    Activity, AppError, AppResult, Dashboard, DigestItem, Edition, Evidence, FeedbackSignal,
    HostCapabilities, ModelStatus, RunnerStatus, Settings, Source, TimestampKind, Trend,
    TrendMethod,
};
use crate::inference::GroundedSummary;

#[derive(Debug, Clone)]
pub struct InferenceCandidate {
    pub post: NormalizedPost,
    pub comments: Vec<NormalizedComment>,
    pub comment_completeness: CommentCompleteness,
    pub comments_truncated: bool,
    pub evidence_hash: String,
    /// Hash of post content plus the exact ordered comment evidence and completeness state.
    pub input_hash: String,
}

impl InferenceCandidate {
    pub fn unavailable(post: NormalizedPost) -> Self {
        let input_hash =
            summary_input_hash_for(&post, &[], CommentCompleteness::Unavailable, false);
        Self {
            post,
            comments: Vec::new(),
            comment_completeness: CommentCompleteness::Unavailable,
            comments_truncated: false,
            evidence_hash: "unavailable".to_owned(),
            input_hash,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedPost {
    pub post: NormalizedPost,
    /// Cryptographic identity of the immutable inference candidate consumed by preparation.
    pub input_hash: String,
    pub summary: GroundedSummary,
    pub provider: String,
    pub model_id: Option<String>,
    pub prompt_version: String,
    pub summary_method: String,
}

#[derive(Debug, Clone)]
pub struct RssSourceSpec {
    pub id: String,
    pub generation: i64,
    pub label: String,
    pub requested_url: String,
    pub effective_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl RssSourceSpec {
    pub fn sync_url(&self) -> &str {
        self.effective_url.as_deref().unwrap_or(&self.requested_url)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSelectionMode {
    ManualOverride,
    ResidentDue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerLease {
    pub owner: String,
    pub token: i64,
    pub scheduled_for: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerOutcome {
    Complete,
    Partial,
    Failed,
    Unknown,
}

impl RunnerOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

const MIGRATION_1: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_2: &str = include_str!("../migrations/0002_hardening.sql");
const MIGRATION_3: &str = include_str!("../migrations/0003_foundation_integrity.sql");
const MIGRATION_4: &str = include_str!("../migrations/0004_resident_runner.sql");
const MIGRATION_5: &str = include_str!("../migrations/0005_concurrency_identity.sql");
const MIGRATION_6: &str = include_str!("../migrations/0006_privacy_epoch.sql");
const MIGRATION_7: &str = include_str!("../migrations/0007_validator_repair.sql");
const MIGRATION_8: &str = include_str!("../migrations/0008_private_replay_capabilities.sql");
const MIGRATION_9: &str = include_str!("../migrations/0009_connector_sync_metadata.sql");
const MIGRATION_10: &str = include_str!("../migrations/0010_comment_finality_provenance.sql");
const MIGRATION_11: &str = include_str!("../migrations/0011_comment_activation_closure.sql");
const MIGRATION_12: &str = include_str!("../migrations/0012_comment_identity_ledger.sql");
const LATEST_SCHEMA_VERSION: i64 = 12;

pub struct Database {
    connection: Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDisposition {
    New,
    Complete,
    Unknown,
}

impl Database {
    #[cfg(test)]
    pub(crate) fn connection_for_test(&self) -> &Connection {
        &self.connection
    }

    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn memory() -> Result<Self, rusqlite::Error> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, rusqlite::Error> {
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "busy_timeout", 5_000_i64)?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);",
            )?;
            let current = transaction.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            if current > LATEST_SCHEMA_VERSION {
                return Err(rusqlite::Error::InvalidQuery);
            }
            for (version, sql) in [
                (1_i64, MIGRATION_1),
                (2_i64, MIGRATION_2),
                (3_i64, MIGRATION_3),
                (4_i64, MIGRATION_4),
                (5_i64, MIGRATION_5),
                (6_i64, MIGRATION_6),
                (7_i64, MIGRATION_7),
                (8_i64, MIGRATION_8),
                (9_i64, MIGRATION_9),
                (10_i64, MIGRATION_10),
                (11_i64, MIGRATION_11),
                (12_i64, MIGRATION_12),
            ] {
                if current < version {
                    transaction.execute_batch(sql)?;
                    if version == 3 || version == 4 {
                        sanitize_legacy_links(&transaction)?;
                    }
                    transaction.execute(
                        "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, ?2)",
                        params![version, Utc::now().timestamp_millis()],
                    )?;
                }
            }
            transaction.commit()?;
        }
        let mut database = Self { connection };
        database.initialize_empty_state()?;
        database.apply_retention()?;
        Ok(database)
    }

    fn initialize_empty_state(&mut self) -> Result<(), rusqlite::Error> {
        let now = Utc::now().timestamp_millis();
        let transaction = self.connection.transaction()?;
        let settings_json =
            serde_json::to_string(&Settings::default()).expect("settings serialize");
        transaction.execute(
            "INSERT OR IGNORE INTO settings(key, value_json, updated_at) VALUES('app', ?1, ?2)",
            params![settings_json, now],
        )?;
        ensure_ready_edition(&transaction, now)?;
        transaction.commit()
    }

    pub fn begin_request(
        &self,
        request_id: &str,
        command: &str,
        payload_hash: &str,
    ) -> AppResult<RequestDisposition> {
        self.begin_request_with_deleted_source(request_id, command, payload_hash, None)
    }

    pub fn begin_delete_request(
        &self,
        request_id: &str,
        source_id: &str,
    ) -> AppResult<RequestDisposition> {
        validate_id(source_id)?;
        self.begin_request_with_deleted_source(
            request_id,
            "delete_source",
            &content_hash(source_id),
            Some(source_id),
        )
    }

    fn begin_request_with_deleted_source(
        &self,
        request_id: &str,
        command: &str,
        payload_hash: &str,
        deleted_source_id: Option<&str>,
    ) -> AppResult<RequestDisposition> {
        validate_id(request_id)?;
        if command.len() > 64 || payload_hash.len() > 128 {
            return Err(AppError::validation("The request metadata is invalid."));
        }
        // A crashed process cannot safely know whether a pending side effect committed. Convert
        // stale rows to fail-closed replay tombstones rather than retrying a possibly applied effect.
        let now = Utc::now().timestamp_millis();
        self.connection
            .execute(
                "UPDATE request_receipts SET state='complete', payload_hash='stale-pending-tombstone', completed_at=?1 WHERE state='pending' AND created_at < ?2",
                params![now, now - Duration::minutes(15).num_milliseconds()],
            )
            .map_err(|_| AppError::internal())?;
        let existing = self
            .connection
            .query_row(
                "SELECT command, payload_hash, state FROM request_receipts WHERE request_id=?1",
                [request_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        if let Some((stored_command, stored_hash, state)) = existing {
            let legacy_completion_tombstone = stored_hash == format!("tombstone:{command}");
            let unknown_tombstone = stored_hash == format!("tombstone-unknown:{command}")
                || stored_hash == "stale-pending-tombstone"
                || legacy_completion_tombstone;
            let private_delete = stored_hash.strip_prefix("private-delete:");
            let private_delete_matches = if let (Some(capability), Some(source_id)) =
                (private_delete, deleted_source_id)
            {
                self.connection
                    .query_row(
                        "SELECT 1 FROM source_tombstones WHERE source_id=?1 AND replay_capability=?2",
                        params![source_id, capability],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(|_| AppError::internal())?
                    .is_some()
            } else {
                false
            };
            let known_binding = stored_hash == payload_hash || private_delete_matches;
            if stored_command != command
                || (private_delete.is_some() && !private_delete_matches)
                || (!unknown_tombstone && private_delete.is_none() && !known_binding)
            {
                return Err(AppError::conflict(
                    "That request identifier was already used for a different action.",
                ));
            }
            return if state == "complete" {
                Ok(if unknown_tombstone {
                    RequestDisposition::Unknown
                } else {
                    RequestDisposition::Complete
                })
            } else {
                Err(AppError::conflict("That request is already in progress."))
            };
        }
        self.connection
            .execute(
                "INSERT INTO request_receipts(request_id, command, payload_hash, state, created_at) VALUES(?1, ?2, ?3, 'pending', ?4)",
                params![request_id, command, payload_hash, Utc::now().timestamp_millis()],
            )
            .map_err(|_| AppError::internal())?;
        Ok(RequestDisposition::New)
    }

    pub fn complete_request(&self, request_id: &str) -> AppResult<()> {
        self.connection
            .execute(
                "UPDATE request_receipts SET state='complete', completed_at=?1 WHERE request_id=?2 AND state='pending'",
                params![Utc::now().timestamp_millis(), request_id],
            )
            .map_err(|_| AppError::internal())?;
        Ok(())
    }

    pub fn abort_request(&self, request_id: &str) {
        let _ = self.connection.execute(
            "DELETE FROM request_receipts WHERE request_id=?1 AND state='pending'",
            [request_id],
        );
    }

    /// Seal an interrupted request without claiming that all of its effects are known. The
    /// command-only tombstone prevents replay while disclosing no source or payload identity.
    pub fn seal_request_unknown(&self, request_id: &str, command: &str) -> AppResult<()> {
        validate_id(request_id)?;
        self.connection
            .execute(
                "UPDATE request_receipts SET state='complete', payload_hash=?1, completed_at=?2 WHERE request_id=?3 AND command=?4 AND state='pending'",
                params![format!("tombstone-unknown:{command}"), Utc::now().timestamp_millis(), request_id, command],
            )
            .map_err(|_| AppError::internal())?;
        Ok(())
    }

    pub fn forget_request(&self, request_id: &str) {
        let _ = self.connection.execute(
            "DELETE FROM request_receipts WHERE request_id=?1",
            [request_id],
        );
    }

    #[cfg(test)]
    pub(crate) fn age_pending_request_for_test(&self, request_id: &str) {
        self.connection
            .execute(
                "UPDATE request_receipts SET created_at=0 WHERE request_id=?1 AND state='pending'",
                [request_id],
            )
            .expect("age pending request");
    }

    pub fn apply_retention(&mut self) -> Result<(), rusqlite::Error> {
        let settings = self.load_settings().unwrap_or_default();
        let now = Utc::now().timestamp_millis();
        let transaction = self.connection.transaction()?;
        apply_retention_in_transaction(&transaction, settings.retention_days, now)?;
        transaction.commit()
    }

    pub fn dashboard(&self, model: ModelStatus, host: HostCapabilities) -> AppResult<Dashboard> {
        let edition = self.load_edition().map_err(|_| AppError::internal())?;
        let items = self
            .load_items(&edition.id)
            .map_err(|_| AppError::internal())?;
        let trends = self
            .load_trends(&edition.id)
            .map_err(|_| AppError::internal())?;
        let sources = self.load_sources().map_err(|_| AppError::internal())?;
        let activity = self.load_activity().map_err(|_| AppError::internal())?;
        let mut settings = self.load_settings().map_err(|_| AppError::internal())?;
        settings.feedback_count = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM feedback WHERE retracted_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let privacy_epoch = self
            .connection
            .query_row(
                "SELECT privacy_epoch FROM app_state WHERE singleton=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| AppError::internal())?
            .max(0) as u64;
        Ok(Dashboard {
            privacy_epoch,
            edition,
            items,
            trends,
            sources,
            activity,
            settings,
            model,
            host,
            runner: self
                .load_runner_status(false, false)
                .map_err(|_| AppError::internal())?,
            connectors: crate::connectors::connector_descriptors(),
        })
    }

    pub fn run_digest(&mut self, request_id: &str) -> AppResult<()> {
        self.run_digest_fenced(request_id, None)
    }

    pub fn run_digest_fenced(
        &mut self,
        request_id: &str,
        lease: Option<&RunnerLease>,
    ) -> AppResult<()> {
        validate_id(request_id)?;
        let dedupe_key = format!("manual-digest:{request_id}");
        if self
            .connection
            .query_row(
                "SELECT 1 FROM jobs WHERE dedupe_key=?1",
                [dedupe_key.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map_err(|_| AppError::internal())?
            .is_some()
        {
            return Ok(());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        assert_runner_authority(&transaction, lease)?;
        let now = Utc::now();
        let now_ms = now.timestamp_millis();
        let digest_id = uuid::Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO jobs(id, kind, dedupe_key, state, attempts, run_after, message, created_at, updated_at) VALUES(?1, 'digest', ?2, 'running', 1, ?3, 'Preparing a deliberate edition', ?3, ?3)",
            params![uuid::Uuid::new_v4().to_string(), dedupe_key, now_ms],
        ).map_err(|_| AppError::internal())?;
        transaction.execute(
            "INSERT INTO digests(id, label, period_start, period_end, generated_at, next_edition_at, status, overview) VALUES(?1, 'Fresh edition', ?2, ?3, ?3, 0, 'ready', 'A finite local edition prepared from your connected sources.')",
            params![digest_id, now_ms - 43_200_000, now_ms],
        ).map_err(|_| AppError::internal())?;
        {
            let mut statement = transaction.prepare(
                "SELECT id FROM (
                    SELECT p.id, p.published_at,
                           ROW_NUMBER() OVER (PARTITION BY p.source_id ORDER BY p.published_at DESC, p.id) AS source_rank
                    FROM posts p WHERE p.deleted_at IS NULL
                    AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.post_id=p.id AND f.signal='not_relevant' AND f.retracted_at IS NULL)
                    AND NOT EXISTS (SELECT 1 FROM feedback f WHERE f.source_id=p.source_id AND f.signal='mute_source' AND f.retracted_at IS NULL)
                 ) WHERE source_rank <= 2 ORDER BY published_at DESC, id LIMIT 8"
            ).map_err(|_| AppError::internal())?;
            let post_ids = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|_| AppError::internal())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| AppError::internal())?;
            for (index, post_id) in post_ids.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO digest_items(digest_id, post_id, rank, reason, topic, importance) VALUES(?1, ?2, ?3, 'Recent · source-balanced baseline', 'Your sources', ?4)",
                    params![digest_id, post_id, i64::try_from(index + 1).unwrap_or(8), 0.8_f64 - (index as f64 * 0.04)],
                ).map_err(|_| AppError::internal())?;
            }
        }
        transaction.execute(
            "UPDATE jobs SET state='complete', message='Fresh edition prepared', updated_at=?1 WHERE dedupe_key=?2",
            params![now_ms, dedupe_key],
        ).map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())
    }

    pub fn record_feedback(
        &mut self,
        request_id: &str,
        post_id: &str,
        signal: &FeedbackSignal,
    ) -> AppResult<()> {
        validate_id(request_id)?;
        validate_id(post_id)?;
        let payload_hash = content_hash(&format!("{post_id}:{}", signal.as_str()));
        match self.begin_request(request_id, "feedback", &payload_hash)? {
            RequestDisposition::Complete => return Ok(()),
            RequestDisposition::Unknown => {
                return Err(AppError::conflict(
                    "That earlier feedback request has unknown finality and was not reported as saved. Refresh before choosing again.",
                ));
            }
            RequestDisposition::New => {}
        }
        let source_id: String = match self
            .connection
            .query_row(
                "SELECT source_id FROM posts WHERE id=?1",
                [post_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?
        {
            Some(source_id) => source_id,
            None => {
                self.abort_request(request_id);
                return Err(AppError::not_found("That edition item no longer exists."));
            }
        };
        let stored = (|| -> Result<(), rusqlite::Error> {
            let transaction = self.connection.transaction()?;
            transaction.execute(
                "INSERT INTO feedback(id, request_id, post_id, source_id, signal, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![uuid::Uuid::new_v4().to_string(), request_id, post_id, source_id, signal.as_str(), Utc::now().timestamp_millis()],
            )?;
            if matches!(
                signal,
                FeedbackSignal::NotRelevant | FeedbackSignal::MuteSource
            ) {
                transaction.execute(
                    "UPDATE app_state SET privacy_epoch=privacy_epoch+1 WHERE singleton=1",
                    [],
                )?;
            }
            let completed = transaction.execute(
                "UPDATE request_receipts SET state='complete', completed_at=?1 WHERE request_id=?2 AND command='feedback' AND state='pending'",
                params![Utc::now().timestamp_millis(), request_id],
            )?;
            if completed != 1 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            transaction.commit()
        })();
        if stored.is_err() {
            // The transaction rolled every feedback/epoch/receipt effect back. Remove only the
            // still-pending receipt so a deliberate retry can safely execute.
            self.abort_request(request_id);
            return Err(AppError::internal());
        }
        Ok(())
    }

    pub fn undo_feedback(&self, request_id: &str) -> AppResult<()> {
        validate_id(request_id)?;
        self.connection
            .execute(
                "UPDATE feedback SET retracted_at=COALESCE(retracted_at, ?1) WHERE request_id=?2",
                params![Utc::now().timestamp_millis(), request_id],
            )
            .map_err(|_| AppError::internal())?;
        // Undo is idempotent. A missing row after reset is already in the requested state.
        Ok(())
    }

    pub fn update_settings(&mut self, request_id: &str, settings: &Settings) -> AppResult<()> {
        validate_id(request_id)?;
        if settings.schedule_hour > 23
            || settings.quiet_hours_start > 23
            || settings.quiet_hours_end > 23
            || !(1..=365).contains(&settings.retention_days)
            || (settings.schedule_enabled
                && crate::scheduler::in_quiet_hours(
                    u32::from(settings.schedule_hour),
                    settings.quiet_hours_start,
                    settings.quiet_hours_end,
                ))
            || !settings.local_only
            || (!settings.selected_model.is_empty()
                && crate::inference::validate_model_name(&settings.selected_model).is_err())
        {
            return Err(AppError::validation(
                "One or more settings are outside the safe range.",
            ));
        }
        let value = serde_json::to_string(settings).map_err(|_| AppError::internal())?;
        let now = Utc::now().timestamp_millis();
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        transaction.execute(
            "INSERT INTO settings(key, value_json, updated_at) VALUES('app', ?1, ?2) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json, updated_at=excluded.updated_at",
            params![value, now],
        ).map_err(|_| AppError::internal())?;
        apply_retention_in_transaction(&transaction, settings.retention_days, now)
            .map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())
    }

    pub fn reset_learning(&mut self, request_id: &str) -> AppResult<()> {
        validate_id(request_id)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        transaction
            .execute("DELETE FROM feedback", [])
            .map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())
    }

    pub fn changed_posts(
        &self,
        source: &RssSourceSpec,
        posts: &[NormalizedPost],
    ) -> AppResult<Vec<NormalizedPost>> {
        self.changed_posts_fenced(source, posts, None)
    }

    pub fn changed_posts_fenced(
        &self,
        source: &RssSourceSpec,
        posts: &[NormalizedPost],
        lease: Option<&RunnerLease>,
    ) -> AppResult<Vec<NormalizedPost>> {
        if let Some(lease) = lease {
            assert_runner_authority_connection(&self.connection, lease)?;
        }
        let live_generation = self
            .connection
            .query_row(
                "SELECT generation FROM sources WHERE id=?1",
                [&source.id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        if live_generation != Some(source.generation) {
            return Err(AppError::conflict(
                "The source changed or was deleted while synchronization was running.",
            ));
        }
        let mut changed = Vec::new();
        for post in posts {
            let stored = self
                .connection
                .query_row(
                    "SELECT content_hash FROM posts WHERE source_id=?1 AND remote_id=?2 AND deleted_at IS NULL",
                    params![source.id, post.remote_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| AppError::internal())?;
            if stored.as_deref() != Some(post_content_hash(post).as_str()) {
                changed.push(post.clone());
            }
        }
        Ok(changed)
    }

    pub fn add_rss_source(
        &mut self,
        request_id: &str,
        label: &str,
        requested_url: &str,
        page: &SyncPage,
        prepared: Vec<PreparedPost>,
    ) -> AppResult<(String, usize)> {
        validate_id(request_id)?;
        validate_source_label(label)?;
        crate::connectors::validate_public_feed_url(requested_url)
            .map_err(|_| AppError::validation("The source URL is no longer allowed."))?;
        crate::connectors::validate_public_feed_url(&page.effective_url).map_err(|_| {
            AppError::validation("The source redirect target is no longer allowed.")
        })?;
        let now = Utc::now().timestamp_millis();
        let source_id = format!("rss-{}", &content_hash(requested_url)[..20]);
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        let prior_generation = transaction
            .query_row(
                "SELECT generation FROM source_tombstones WHERE source_id=?1",
                [&source_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?
            .unwrap_or(0);
        let generation = prior_generation.saturating_add(1);
        let config = serde_json::to_string(&serde_json::json!({ "url": requested_url }))
            .map_err(|_| AppError::internal())?;
        transaction.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, validator_url, etag, last_modified, last_success_at, next_poll_at, created_at, updated_at, generation)
             VALUES(?1, 'rss', ?2, 'RSS · publisher feed', 'healthy', ?3, ?4, ?5, ?6, ?7, ?8, ?7, ?7, ?9)",
            params![source_id, label.trim(), config, page.effective_url, page.etag, page.last_modified, now, now + Duration::hours(6).num_milliseconds(), generation],
        ).map_err(|_| AppError::conflict("That RSS source is already connected."))?;
        transaction.execute(
            "INSERT INTO source_sync_metadata(source_id, health_state, safe_detail, comments_status, comments_truncated, retry_at, updated_at) VALUES(?1, 'healthy', 'RSS synchronized.', 'unavailable', 0, ?2, ?3)",
            params![source_id, now + Duration::hours(6).num_milliseconds(), now],
        ).map_err(|_| AppError::internal())?;
        let changed = ingest_posts(&transaction, &source_id, &page.posts, prepared, now)?;
        set_comment_state(
            &transaction,
            &source_id,
            &page.posts,
            CommentCompleteness::Unavailable,
            false,
            now,
        )?;
        record_sync_job(&transaction, request_id, page.posts.len(), now)?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO receipt_sources(request_id, source_id) VALUES(?1, ?2)",
                params![request_id, source_id],
            )
            .map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())?;
        Ok((source_id, changed))
    }

    pub fn ingest_existing_rss(
        &mut self,
        source: &RssSourceSpec,
        request_id: &str,
        page: &SyncPage,
        prepared: Vec<PreparedPost>,
    ) -> AppResult<(usize, usize)> {
        self.ingest_existing_rss_fenced(source, request_id, page, prepared, None)
    }

    pub fn ingest_existing_rss_fenced(
        &mut self,
        source: &RssSourceSpec,
        request_id: &str,
        page: &SyncPage,
        prepared: Vec<PreparedPost>,
        lease: Option<&RunnerLease>,
    ) -> AppResult<(usize, usize)> {
        validate_id(request_id)?;
        crate::connectors::validate_public_feed_url(&page.effective_url).map_err(|_| {
            AppError::validation("The source redirect target is no longer allowed.")
        })?;
        let now = Utc::now().timestamp_millis();
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        assert_runner_authority(&transaction, lease)?;
        let changed_source = transaction.execute(
            "UPDATE sources SET status='healthy', validator_url=?1, etag=?2, last_modified=?3, last_success_at=?4, next_poll_at=?5, failure_count=0, updated_at=?4
             WHERE id=?6 AND generation=?7",
            params![page.effective_url, page.etag, page.last_modified, now, now + Duration::hours(6).num_milliseconds(), source.id, source.generation],
        ).map_err(|_| AppError::internal())?;
        if changed_source != 1 {
            return Err(AppError::conflict(
                "The source was deleted or replaced while synchronization was running.",
            ));
        }
        transaction.execute(
            "INSERT INTO source_sync_metadata(source_id, health_state, safe_detail, comments_status, comments_truncated, retry_at, updated_at)
             VALUES(?1, 'healthy', 'RSS synchronized.', 'unavailable', 0, ?2, ?3)
             ON CONFLICT(source_id) DO UPDATE SET health_state='healthy', safe_detail='RSS synchronized.', comments_status='unavailable', comments_truncated=0, retry_at=excluded.retry_at, updated_at=excluded.updated_at",
            params![source.id, now + Duration::hours(6).num_milliseconds(), now],
        ).map_err(|_| AppError::internal())?;
        let changed = ingest_posts(&transaction, &source.id, &page.posts, prepared, now)?;
        set_comment_state(
            &transaction,
            &source.id,
            &page.posts,
            CommentCompleteness::Unavailable,
            false,
            now,
        )?;
        record_sync_job(&transaction, request_id, page.posts.len(), now)?;
        transaction.commit().map_err(|_| AppError::internal())?;
        Ok((changed, page.posts.len().saturating_sub(changed)))
    }

    /// Classifies post or comment-evidence changes before inference. Unchanged comment snapshots
    /// do not consume the global model-attempt budget; comment-only changes return the stored post.
    pub fn changed_posts_for_sync_batch_fenced(
        &self,
        source: &SourceSyncSpec,
        batch: &SyncBatch,
        lease: Option<&RunnerLease>,
    ) -> AppResult<Vec<InferenceCandidate>> {
        validate_source_sync_spec(source)
            .map_err(|_| AppError::validation("The connector source specification is invalid."))?;
        batch
            .validate_for(source.kind)
            .map_err(|_| AppError::validation("The connector batch exceeded a safe bound."))?;
        if let Some(lease) = lease {
            assert_runner_authority_connection(&self.connection, lease)?;
        }
        let current = self
            .connection
            .query_row(
                "SELECT 1 FROM sources WHERE id=?1 AND generation=?2 AND connector_kind=?3 AND sync_cursor IS ?4",
                params![source.id, source.generation, source_kind_str(source.kind), source.cursor],
                |_| Ok(()),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        if current.is_none() {
            return Err(AppError::conflict(
                "The source was deleted or replaced while synchronization was running.",
            ));
        }
        assert_comment_id_assignments(&self.connection, &source.id, batch)?;
        changed_posts_for_batch(&self.connection, source, batch)
    }

    /// Provider-neutral fenced persistence seam. Official connectors remain disabled, but any
    /// future batch must pass the same source-generation and resident-owner checks as RSS.
    pub fn ingest_sync_batch_fenced(
        &mut self,
        source: &SourceSyncSpec,
        request_id: &str,
        batch: &SyncBatch,
        prepared: Vec<PreparedPost>,
        lease: Option<&RunnerLease>,
    ) -> AppResult<(usize, usize)> {
        validate_id(request_id)?;
        validate_source_sync_spec(source)
            .map_err(|_| AppError::validation("The connector source specification is invalid."))?;
        batch
            .validate_for(source.kind)
            .map_err(|_| AppError::validation("The connector batch exceeded a safe bound."))?;
        if source.kind == SourceKind::Rss
            && batch.comment_completeness != CommentCompleteness::Unavailable
        {
            return Err(AppError::validation(
                "RSS comments must remain unavailable.",
            ));
        }
        assert_comment_id_assignments(&self.connection, &source.id, batch)?;
        let now = Utc::now().timestamp_millis();
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        assert_runner_authority(&transaction, lease)?;
        assert_comment_id_assignments(&transaction, &source.id, batch)?;
        let expected = changed_posts_for_batch(&transaction, source, batch)?;
        validate_prepared_posts(&expected, &prepared)?;
        insert_comment_identities(&transaction, source, batch)?;
        let changed_source = transaction.execute(
            "UPDATE sources SET sync_cursor=?1, last_success_at=?2, next_poll_at=?3, failure_count=0, updated_at=?2
             WHERE id=?4 AND generation=?5 AND connector_kind=?6 AND sync_cursor IS ?7",
            params![batch.cursor, now, batch.health.retry_at, source.id, source.generation, source_kind_str(source.kind), source.cursor],
        ).map_err(|_| AppError::internal())?;
        if changed_source != 1 {
            return Err(AppError::conflict(
                "The source was deleted or replaced while synchronization was running.",
            ));
        }
        transaction.execute(
            "INSERT INTO source_sync_metadata(source_id, health_state, safe_detail, comments_status, comments_truncated, retry_at, updated_at, page_finality)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(source_id) DO UPDATE SET health_state=excluded.health_state, safe_detail=excluded.safe_detail, comments_status=excluded.comments_status, comments_truncated=excluded.comments_truncated, retry_at=excluded.retry_at, updated_at=excluded.updated_at, page_finality=excluded.page_finality",
            params![source.id, batch.health.state.as_str(), bounded_safe_detail(&batch.health.safe_detail), batch.comment_completeness.as_str(), i64::from(batch.comments_truncated), batch.health.retry_at, now, match batch.page_finality { PageFinality::Complete => "complete", PageFinality::Partial => "partial" }],
        ).map_err(|_| AppError::internal())?;
        upsert_posts_only(&transaction, &source.id, &batch.posts, now)?;
        let removed_comment_evidence = reconcile_comments(&transaction, &source.id, batch, now)?;
        update_comment_state(&transaction, &source.id, batch, now)?;
        let comment_evidence_changed =
            batch.comment_completeness != CommentCompleteness::Unavailable && !expected.is_empty();
        if removed_comment_evidence || comment_evidence_changed {
            transaction
                .execute(
                    "UPDATE app_state SET privacy_epoch=privacy_epoch+1 WHERE singleton=1",
                    [],
                )
                .map_err(|_| AppError::internal())?;
        }
        persist_prepared_summaries(&transaction, &source.id, prepared, now)?;
        record_connector_sync_job(
            &transaction,
            request_id,
            source_kind_str(source.kind),
            batch.posts.len(),
            batch.page_finality,
            now,
        )?;
        transaction.commit().map_err(|_| AppError::internal())?;
        let changed = expected.len();
        Ok((
            changed,
            batch
                .posts
                .len()
                .saturating_sub(changed.min(batch.posts.len())),
        ))
    }

    pub fn source_sync_specs(
        &self,
        mode: SourceSelectionMode,
        now: i64,
        cap: usize,
    ) -> AppResult<(Vec<SourceSyncSpec>, bool)> {
        let due_clause = if mode == SourceSelectionMode::ResidentDue {
            " AND (next_poll_at IS NULL OR next_poll_at <= ?1)"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, connector_kind, generation, config_json, sync_cursor FROM sources
             WHERE connector_kind!='demo' AND status!='paused'{due_clause} ORDER BY id LIMIT ?2"
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|_| AppError::internal())?;
        let limit = i64::try_from(cap.saturating_add(1)).unwrap_or(i64::MAX);
        let rows = statement
            .query_map(params![now, limit], |row| {
                let kind: String = row.get(1)?;
                let kind = match kind.as_str() {
                    "rss" => SourceKind::Rss,
                    "mastodon" => SourceKind::Mastodon,
                    "bluesky" => SourceKind::Bluesky,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok(SourceSyncSpec {
                    id: row.get(0)?,
                    kind,
                    generation: row.get(2)?,
                    config_json: row.get(3)?,
                    cursor: row.get(4)?,
                })
            })
            .map_err(|_| AppError::internal())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AppError::internal())?;
        let capped = rows.len() > cap;
        Ok((rows.into_iter().take(cap).collect(), capped))
    }

    pub fn settings(&self) -> AppResult<Settings> {
        self.load_settings().map_err(|_| AppError::internal())
    }

    pub fn rss_sources(
        &self,
        mode: SourceSelectionMode,
        now: i64,
        cap: usize,
    ) -> AppResult<(Vec<RssSourceSpec>, bool)> {
        let due_clause = if mode == SourceSelectionMode::ResidentDue {
            " AND (next_poll_at IS NULL OR next_poll_at <= ?1)"
        } else {
            ""
        };
        let sql = format!(
            "SELECT id, generation, account_label, config_json, validator_url, etag, last_modified
             FROM sources WHERE connector_kind='rss' AND status!='paused'{due_clause}
             ORDER BY id LIMIT ?2"
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|_| AppError::internal())?;
        let limit = i64::try_from(cap.saturating_add(1)).unwrap_or(i64::MAX);
        let rows = statement
            .query_map(params![now, limit], |row| {
                let config: String = row.get(3)?;
                let requested_url = serde_json::from_str::<serde_json::Value>(&config)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("url")
                            .and_then(|url| url.as_str())
                            .map(ToOwned::to_owned)
                    })
                    .unwrap_or_default();
                Ok(RssSourceSpec {
                    id: row.get(0)?,
                    generation: row.get(1)?,
                    label: row.get(2)?,
                    requested_url,
                    effective_url: row.get(4)?,
                    etag: row.get(5)?,
                    last_modified: row.get(6)?,
                })
            })
            .map_err(|_| AppError::internal())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AppError::internal())?;
        let capped = rows.len() > cap;
        Ok((rows.into_iter().take(cap).collect(), capped))
    }

    pub fn complete_not_modified(
        &mut self,
        source: &RssSourceSpec,
        request_id: &str,
        page: &SyncPage,
    ) -> AppResult<()> {
        self.complete_not_modified_fenced(source, request_id, page, None)
    }

    pub fn complete_not_modified_fenced(
        &mut self,
        source: &RssSourceSpec,
        request_id: &str,
        page: &SyncPage,
        lease: Option<&RunnerLease>,
    ) -> AppResult<()> {
        let now = Utc::now().timestamp_millis();
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        assert_runner_authority(&transaction, lease)?;
        let changed = transaction.execute(
            "UPDATE sources SET status='healthy', validator_url=?1, etag=?2, last_modified=?3, last_success_at=?4, next_poll_at=?5, failure_count=0, updated_at=?4 WHERE id=?6 AND generation=?7",
            params![page.effective_url, page.etag, page.last_modified, now, now + Duration::hours(6).num_milliseconds(), source.id, source.generation],
        ).map_err(|_| AppError::internal())?;
        if changed != 1 {
            return Err(AppError::conflict(
                "The source was deleted or replaced while synchronization was running.",
            ));
        }
        transaction.execute(
            "INSERT INTO source_sync_metadata(source_id, health_state, safe_detail, comments_status, comments_truncated, retry_at, updated_at)
             VALUES(?1, 'healthy', 'RSS unchanged.', 'unavailable', 0, ?2, ?3)
             ON CONFLICT(source_id) DO UPDATE SET health_state='healthy', safe_detail='RSS unchanged.', comments_status='unavailable', comments_truncated=0, retry_at=excluded.retry_at, updated_at=excluded.updated_at",
            params![source.id, now + Duration::hours(6).num_milliseconds(), now],
        ).map_err(|_| AppError::internal())?;
        transaction.execute(
            "INSERT OR REPLACE INTO jobs(id, kind, dedupe_key, state, attempts, run_after, message, created_at, updated_at) VALUES(?1, 'sync', ?2, 'complete', 1, ?3, 'RSS source unchanged (HTTP 304)', ?3, ?3)",
            params![uuid::Uuid::new_v4().to_string(), format!("rss-sync:{request_id}"), now],
        ).map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())
    }

    pub fn record_sync_failure(
        &mut self,
        source: &RssSourceSpec,
        request_id: &str,
        message: &str,
    ) -> AppResult<()> {
        self.record_sync_failure_fenced(source, request_id, message, None)
    }

    pub fn record_sync_failure_fenced(
        &mut self,
        source: &RssSourceSpec,
        request_id: &str,
        message: &str,
        lease: Option<&RunnerLease>,
    ) -> AppResult<()> {
        let now = Utc::now().timestamp_millis();
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        assert_runner_authority(&transaction, lease)?;
        let failures: i64 = transaction
            .query_row(
                "SELECT failure_count + 1 FROM sources WHERE id=?1 AND generation=?2",
                params![source.id, source.generation],
                |row| row.get(0),
            )
            .map_err(|_| AppError::internal())?;
        let exponent = u32::try_from(failures.clamp(1, 6) - 1).unwrap_or(0);
        let backoff_minutes = 5_i64.saturating_mul(2_i64.pow(exponent));
        transaction.execute(
            "UPDATE sources SET status='attention', failure_count=?1, next_poll_at=?2, updated_at=?3 WHERE id=?4 AND generation=?5",
            params![failures, now + Duration::minutes(backoff_minutes).num_milliseconds(), now, source.id, source.generation],
        ).map_err(|_| AppError::internal())?;
        let retry_at = now + Duration::minutes(backoff_minutes).num_milliseconds();
        transaction.execute(
            "INSERT INTO source_sync_metadata(source_id, health_state, safe_detail, comments_status, comments_truncated, retry_at, updated_at)
             VALUES(?1, 'transient', 'RSS synchronization failed; retry is bounded.', 'unavailable', 0, ?2, ?3)
             ON CONFLICT(source_id) DO UPDATE SET health_state='transient', safe_detail='RSS synchronization failed; retry is bounded.', comments_status='unavailable', comments_truncated=0, retry_at=excluded.retry_at, updated_at=excluded.updated_at",
            params![source.id, retry_at, now],
        ).map_err(|_| AppError::internal())?;
        transaction.execute(
            "INSERT OR REPLACE INTO jobs(id, kind, dedupe_key, state, attempts, run_after, last_error_code, message, created_at, updated_at) VALUES(?1, 'sync', ?2, 'failed', ?3, ?4, 'SYNC_FAILED', ?5, ?6, ?6)",
            params![uuid::Uuid::new_v4().to_string(), format!("rss-sync:{request_id}"), failures, now + Duration::minutes(backoff_minutes).num_milliseconds(), message, now],
        ).map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())
    }

    pub fn acquire_runner_lease(
        &mut self,
        owner: &str,
        scheduled_for: i64,
        now: i64,
        lease_ms: i64,
    ) -> AppResult<Option<RunnerLease>> {
        validate_id(owner)?;
        if lease_ms <= 0 {
            return Err(AppError::validation(
                "The runner lease duration is invalid.",
            ));
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        transaction.execute(
            "UPDATE jobs SET state='failed', lease_owner=NULL, lease_expires_at=NULL,
             last_error_code=CASE WHEN attempts >= 2 THEN 'UNKNOWN_RECOVERY_EXHAUSTED' ELSE 'UNKNOWN_AFTER_LEASE_EXPIRY' END,
             message=CASE WHEN attempts >= 2 THEN 'Resident recovery also expired; this scheduled instant is terminal' ELSE 'Previous resident owner expired; outcome unknown and recoverable once' END,
             updated_at=?1 WHERE state='running' AND lease_expires_at <= ?1",
            [now],
        ).map_err(|_| AppError::internal())?;
        transaction.execute(
            "UPDATE runner_state SET lease_owner=NULL, lease_expires_at=NULL,
             last_outcome=CASE WHEN EXISTS(
                 SELECT 1 FROM jobs
                 WHERE dedupe_key='scheduled-edition:' || runner_state.last_scheduled_for
                   AND last_error_code='UNKNOWN_RECOVERY_EXHAUSTED'
             ) THEN 'failed' ELSE 'unknown' END,
             detail=CASE WHEN EXISTS(
                 SELECT 1 FROM jobs
                 WHERE dedupe_key='scheduled-edition:' || runner_state.last_scheduled_for
                   AND last_error_code='UNKNOWN_RECOVERY_EXHAUSTED'
             ) THEN 'The one permitted resident recovery also expired; this scheduled instant is terminal.'
             ELSE 'A previous resident owner expired. The same scheduled instant may be recovered once.' END
             WHERE lease_expires_at <= ?1",
            [now],
        ).map_err(|_| AppError::internal())?;
        let dedupe_key = format!("scheduled-edition:{scheduled_for}");
        let terminal: Option<String> = transaction
            .query_row(
                "SELECT id FROM jobs WHERE dedupe_key=?1 AND (state IN ('running','complete') OR (state='failed' AND (COALESCE(last_error_code, '') != 'UNKNOWN_AFTER_LEASE_EXPIRY' OR attempts >= 2)))",
                [&dedupe_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        if terminal.is_some() {
            // Expiry cleanup is itself durable state. In particular, an exhausted second owner
            // must remain terminal after this transaction is dropped or the database is reopened.
            transaction.commit().map_err(|_| AppError::internal())?;
            return Ok(None);
        }
        let token = transaction
            .query_row(
                "SELECT lease_token + 1 FROM runner_state WHERE singleton=1 AND lease_owner IS NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        let Some(token) = token else {
            transaction.commit().map_err(|_| AppError::internal())?;
            return Ok(None);
        };
        let expires_at = now.saturating_add(lease_ms);
        let changed = transaction.execute(
            "UPDATE runner_state SET lease_owner=?1, lease_token=?2, lease_expires_at=?3, last_attempt_at=?4, last_scheduled_for=?5, last_outcome='running', detail='Synchronizing due sources and preparing one finite edition.' WHERE singleton=1 AND lease_owner IS NULL",
            params![owner, token, expires_at, now, scheduled_for],
        ).map_err(|_| AppError::internal())?;
        if changed != 1 {
            transaction.commit().map_err(|_| AppError::internal())?;
            return Ok(None);
        }
        transaction.execute(
            "INSERT INTO jobs(id, kind, dedupe_key, state, attempts, run_after, lease_expires_at, lease_owner, lease_token, message, created_at, updated_at) VALUES(?1, 'scheduled_digest', ?2, 'running', 1, ?3, ?4, ?5, ?6, 'Scheduled edition running while Web is open', ?3, ?3)
             ON CONFLICT(dedupe_key) DO UPDATE SET state='running', attempts=jobs.attempts+1, run_after=excluded.run_after, lease_expires_at=excluded.lease_expires_at, lease_owner=excluded.lease_owner, lease_token=excluded.lease_token, last_error_code=NULL, message=excluded.message, updated_at=excluded.updated_at",
            params![uuid::Uuid::new_v4().to_string(), dedupe_key, now, expires_at, owner, token],
        ).map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())?;
        Ok(Some(RunnerLease {
            owner: owner.to_owned(),
            token,
            scheduled_for,
            expires_at,
        }))
    }

    pub fn heartbeat_runner_lease(
        &mut self,
        lease: &RunnerLease,
        now: i64,
        lease_ms: i64,
    ) -> AppResult<RunnerLease> {
        let expires_at = now.saturating_add(lease_ms);
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        let runner_changed = transaction.execute(
            "UPDATE runner_state SET lease_expires_at=?1 WHERE singleton=1 AND lease_owner=?2 AND lease_token=?3 AND lease_expires_at > ?4",
            params![expires_at, lease.owner, lease.token, now],
        ).map_err(|_| AppError::internal())?;
        let job_changed = transaction.execute(
            "UPDATE jobs SET lease_expires_at=?1, updated_at=?4 WHERE dedupe_key=?2 AND lease_owner=?3 AND lease_token=?5 AND state='running'",
            params![expires_at, format!("scheduled-edition:{}", lease.scheduled_for), lease.owner, now, lease.token],
        ).map_err(|_| AppError::internal())?;
        if runner_changed != 1 || job_changed != 1 {
            return Err(AppError::conflict(
                "This resident worker no longer owns the scheduled lease.",
            ));
        }
        transaction.commit().map_err(|_| AppError::internal())?;
        Ok(RunnerLease {
            expires_at,
            ..lease.clone()
        })
    }

    pub fn finish_runner_lease(
        &mut self,
        lease: &RunnerLease,
        outcome: RunnerOutcome,
        detail: &str,
        next_scheduled_at: Option<i64>,
        now: i64,
    ) -> AppResult<()> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        let attempts: i64 = transaction
            .query_row(
                "SELECT jobs.attempts FROM jobs JOIN runner_state ON runner_state.singleton=1
                 WHERE jobs.dedupe_key=?1 AND jobs.lease_owner=?2 AND jobs.lease_token=?3
                   AND jobs.state='running' AND jobs.lease_expires_at > ?4
                   AND runner_state.lease_owner=?2 AND runner_state.lease_token=?3
                   AND runner_state.lease_expires_at > ?4",
                params![
                    format!("scheduled-edition:{}", lease.scheduled_for),
                    lease.owner,
                    lease.token,
                    now
                ],
                |row| row.get(0),
            )
            .map_err(|_| {
                AppError::conflict(
                    "This resident worker cannot finish after its authority expired or changed.",
                )
            })?;
        let exhausted = outcome == RunnerOutcome::Unknown && attempts >= 2;
        let runner_outcome = if exhausted {
            RunnerOutcome::Failed
        } else {
            outcome
        };
        let runner_detail = if exhausted {
            "The one permitted recovery also ended with an unknown outcome; this scheduled instant is terminal."
        } else {
            detail
        };
        let success = matches!(outcome, RunnerOutcome::Complete | RunnerOutcome::Partial);
        let runner_changed = transaction.execute(
            "UPDATE runner_state SET lease_owner=NULL, lease_expires_at=NULL, last_success_at=CASE WHEN ?1 THEN ?2 ELSE last_success_at END, next_scheduled_at=?3, last_outcome=?4, detail=?5 WHERE singleton=1 AND lease_owner=?6 AND lease_token=?7 AND lease_expires_at > ?2",
            params![success, now, next_scheduled_at, runner_outcome.as_str(), runner_detail, lease.owner, lease.token],
        ).map_err(|_| AppError::internal())?;
        let (job_state, error_code) = match outcome {
            RunnerOutcome::Complete => ("complete", None),
            RunnerOutcome::Partial => ("complete", Some("PARTIAL")),
            RunnerOutcome::Failed => ("failed", Some("RUN_FAILED")),
            RunnerOutcome::Unknown if attempts >= 2 => {
                ("failed", Some("UNKNOWN_RECOVERY_EXHAUSTED"))
            }
            RunnerOutcome::Unknown => ("failed", Some("UNKNOWN_AFTER_LEASE_EXPIRY")),
        };
        let job_changed = transaction.execute(
            "UPDATE jobs SET state=?1, lease_owner=NULL, lease_expires_at=NULL, last_error_code=?2, message=?3, updated_at=?4 WHERE dedupe_key=?5 AND lease_owner=?6 AND lease_token=?7 AND state='running' AND lease_expires_at > ?4",
            params![job_state, error_code, runner_detail, now, format!("scheduled-edition:{}", lease.scheduled_for), lease.owner, lease.token],
        ).map_err(|_| AppError::internal())?;
        if runner_changed != 1 || job_changed != 1 {
            return Err(AppError::conflict(
                "A stale resident owner cannot finish a successor's lease.",
            ));
        }
        transaction.commit().map_err(|_| AppError::internal())
    }

    pub fn set_next_scheduled(&self, next: Option<i64>) -> AppResult<()> {
        self.connection
            .execute(
                "UPDATE runner_state SET next_scheduled_at=?1 WHERE singleton=1",
                [next],
            )
            .map_err(|_| AppError::internal())?;
        Ok(())
    }

    pub fn last_runner_success(&self) -> AppResult<Option<i64>> {
        self.connection
            .query_row(
                "SELECT last_success_at FROM runner_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| AppError::internal())
    }

    /// The scheduler advances after every terminal instant, not only a successful one. A first
    /// unknown remains recoverable; an exhausted second unknown is terminal.
    pub fn last_runner_handled(&self) -> AppResult<Option<i64>> {
        let scheduled: Option<i64> = self
            .connection
            .query_row(
                "SELECT last_scheduled_for FROM runner_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(|_| AppError::internal())?;
        let Some(scheduled) = scheduled else {
            return Ok(None);
        };
        let key = format!("scheduled-edition:{scheduled}");
        let terminal = self
            .connection
            .query_row(
                "SELECT 1 FROM jobs WHERE dedupe_key=?1 AND (state='complete' OR (state='failed' AND (COALESCE(last_error_code, '') != 'UNKNOWN_AFTER_LEASE_EXPIRY' OR attempts >= 2)))",
                [key],
                |_| Ok(()),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        Ok(terminal.map(|()| scheduled))
    }

    pub fn secret_ref_for_source(&self, source_id: &str) -> AppResult<Option<String>> {
        validate_id(source_id)?;
        self.connection
            .query_row(
                "SELECT secret_ref FROM sources WHERE id=?1",
                [source_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?
            .ok_or_else(|| AppError::not_found("That source is already disconnected."))
    }

    pub fn delete_source(&mut self, request_id: &str, source_id: &str) -> AppResult<()> {
        validate_id(request_id)?;
        validate_id(source_id)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| AppError::internal())?;
        let generation = transaction
            .query_row(
                "SELECT generation FROM sources WHERE id=?1",
                [source_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?
            .ok_or_else(|| AppError::not_found("That source is already disconnected."))?;
        let deleted_at = Utc::now().timestamp_millis();
        let replay_capability = transaction
            .query_row(
                "SELECT replay_capability FROM source_tombstones WHERE source_id=?1",
                [source_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?
            .flatten()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        transaction.execute(
            "INSERT INTO source_tombstones(source_id, generation, deleted_at, replay_capability) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(source_id) DO UPDATE SET generation=MAX(source_tombstones.generation, excluded.generation), deleted_at=excluded.deleted_at, replay_capability=COALESCE(source_tombstones.replay_capability, excluded.replay_capability)",
            params![source_id, generation, deleted_at, replay_capability],
        ).map_err(|_| AppError::internal())?;
        // Add/feedback payloads cannot be proven after their subject rows are erased. Convert them
        // to explicit Unknown tombstones, never false Complete. The delete command is rebound below
        // to the source tombstone's random replay capability.
        transaction.execute(
            "UPDATE request_receipts SET payload_hash='tombstone-unknown:' || command, state='complete', completed_at=COALESCE(completed_at, ?2)
             WHERE request_id IN (SELECT request_id FROM receipt_sources WHERE source_id=?1)
                OR request_id IN (SELECT request_id FROM feedback WHERE source_id=?1)",
            params![source_id, deleted_at],
        ).map_err(|_| AppError::internal())?;
        transaction.execute(
            "DELETE FROM trend_clusters WHERE id IN (
                SELECT DISTINCT tm.cluster_id FROM trend_members tm JOIN posts p ON p.id=tm.post_id WHERE p.source_id=?1
             )",
            [source_id],
        ).map_err(|_| AppError::internal())?;
        transaction.execute(
            "DELETE FROM digests WHERE id IN (
                SELECT DISTINCT di.digest_id FROM digest_items di JOIN posts p ON p.id=di.post_id WHERE p.source_id=?1
             )",
            [source_id],
        ).map_err(|_| AppError::internal())?;
        transaction
            .execute("DELETE FROM sources WHERE id=?1", [source_id])
            .map_err(|_| AppError::internal())?;
        transaction
            .execute(
                "UPDATE app_state SET privacy_epoch=privacy_epoch+1 WHERE singleton=1",
                [],
            )
            .map_err(|_| AppError::internal())?;
        transaction
            .execute(
                "DELETE FROM receipt_sources WHERE source_id=?1",
                [source_id],
            )
            .map_err(|_| AppError::internal())?;
        let now = Utc::now().timestamp_millis();
        ensure_ready_edition(&transaction, now).map_err(|_| AppError::internal())?;
        transaction.execute(
            "INSERT INTO audit_events(id, occurred_at, category, action, subject_id, outcome, detail_json) VALUES(?1, ?2, 'privacy', 'delete_source', NULL, 'complete', json_object('deleted', 1))",
            params![uuid::Uuid::new_v4().to_string(), now],
        ).map_err(|_| AppError::internal())?;
        // The receipt contains no subject identifier or deterministic subject digest. Payload-bound
        // replay is proven by resolving this random capability through the durable source tombstone.
        transaction.execute(
            "UPDATE request_receipts SET payload_hash=?1, state='complete', completed_at=?2 WHERE request_id=?3",
            params![format!("private-delete:{replay_capability}"), now, request_id],
        ).map_err(|_| AppError::internal())?;
        transaction.commit().map_err(|_| AppError::internal())?;
        Ok(())
    }

    fn load_edition(&self) -> rusqlite::Result<Edition> {
        self.connection.query_row(
            "SELECT id, label, generated_at, overview FROM digests WHERE status='ready' ORDER BY generated_at DESC, rowid DESC LIMIT 1",
            [], |row| Ok(Edition { id: row.get(0)?, label: row.get(1)?, generated_at: iso(row.get(2)?), next_edition_at: None, summary: row.get(3)? }),
        )
    }

    fn load_items(&self, digest_id: &str) -> rusqlite::Result<Vec<DigestItem>> {
        let mut statement = self.connection.prepare(
            "SELECT p.id, p.source_id, so.account_label, COALESCE(a.display_name, so.account_label), p.title,
                    su.summary_text, su.comment_overview, su.summary_method, su.provider, su.uncertainty,
                    p.published_at, p.published_time_kind, di.reason, di.topic, di.importance,
                    p.body_text, p.canonical_http_url
             FROM digest_items di JOIN posts p ON p.id=di.post_id JOIN sources so ON so.id=p.source_id LEFT JOIN actors a ON a.id=p.actor_id
             LEFT JOIN post_comment_state pcs ON pcs.post_id=p.id
             JOIN summaries su ON su.id=(SELECT s2.id FROM summaries s2 WHERE s2.post_id=p.id AND s2.input_hash=COALESCE(NULLIF(pcs.summary_input_hash, ''), p.content_hash) ORDER BY s2.created_at DESC, s2.id DESC LIMIT 1)
             WHERE di.digest_id=?1 AND p.deleted_at IS NULL
             AND NOT EXISTS(SELECT 1 FROM feedback f WHERE f.post_id=p.id AND f.signal='not_relevant' AND f.retracted_at IS NULL)
             AND NOT EXISTS(SELECT 1 FROM feedback f WHERE f.source_id=p.source_id AND f.signal='mute_source' AND f.retracted_at IS NULL)
             ORDER BY di.rank LIMIT 12"
        )?;
        let rows = statement.query_map([digest_id], |row| {
            let source: String = row.get(2)?;
            let author: String = row.get(3)?;
            let published = iso(row.get(10)?);
            Ok(DigestItem {
                id: row.get(0)?,
                source_id: row.get(1)?,
                source: source.clone(),
                author: author.clone(),
                title: row.get(4)?,
                summary: row.get(5)?,
                comment_overview: row.get(6)?,
                summary_method: row.get(7)?,
                summary_provider: row.get(8)?,
                summary_uncertainty: row.get(9)?,
                published_at: published.clone(),
                published_time_kind: parse_timestamp_kind(row.get::<_, String>(11)?.as_str()),
                reason: row.get(12)?,
                topic: row.get(13)?,
                importance: row.get(14)?,
                evidence: vec![Evidence {
                    source,
                    author,
                    published_at: published,
                    timestamp_kind: parse_timestamp_kind(row.get::<_, String>(11)?.as_str()),
                    excerpt: row.get(15)?,
                    canonical_url: row.get(16)?,
                }],
            })
        })?;
        rows.collect()
    }

    fn load_trends(&self, digest_id: &str) -> rusqlite::Result<Vec<Trend>> {
        let mut statement = self.connection.prepare(
            "SELECT t.id, t.label, t.summary, t.confidence, t.cluster_method,
                    COUNT(DISTINCT p.source_id), GROUP_CONCAT(tm.post_id)
             FROM trend_clusters t JOIN trend_members tm ON tm.cluster_id=t.id JOIN posts p ON p.id=tm.post_id
             WHERE t.digest_id=?1
             AND NOT EXISTS (
                 SELECT 1 FROM trend_members hidden_tm
                 JOIN posts hidden_p ON hidden_p.id=hidden_tm.post_id
                 WHERE hidden_tm.cluster_id=t.id AND (
                     EXISTS(SELECT 1 FROM feedback f WHERE f.post_id=hidden_p.id AND f.signal='not_relevant' AND f.retracted_at IS NULL)
                     OR EXISTS(SELECT 1 FROM feedback f WHERE f.source_id=hidden_p.source_id AND f.signal='mute_source' AND f.retracted_at IS NULL)
                 )
             )
             GROUP BY t.id ORDER BY t.created_at LIMIT 5"
        )?;
        let rows = statement.query_map([digest_id], |row| {
            let ids: String = row.get(6)?;
            Ok(Trend {
                id: row.get(0)?,
                label: row.get(1)?,
                summary: row.get(2)?,
                confidence: row.get(3)?,
                method: parse_trend_method(row.get::<_, String>(4)?.as_str()),
                source_count: row.get(5)?,
                evidence_ids: ids.split(',').map(ToOwned::to_owned).collect(),
            })
        })?;
        rows.collect()
    }

    fn load_sources(&self) -> rusqlite::Result<Vec<Source>> {
        let mut statement = self.connection.prepare(
            "SELECT s.id, s.connector_kind, s.account_label, s.detail,
                    COALESCE(m.health_state, CASE s.status WHEN 'paused' THEN 'paused' WHEN 'attention' THEN 'transient' ELSE 'healthy' END),
                    COALESCE(m.safe_detail, ''), COALESCE(m.comments_status, 'unavailable'), COALESCE(m.comments_truncated, 0),
                    COALESCE(m.page_finality, 'complete'), s.last_success_at, COALESCE(m.retry_at, s.next_poll_at), COUNT(p.id)
             FROM sources s
             LEFT JOIN source_sync_metadata m ON m.source_id=s.id
             LEFT JOIN posts p ON p.source_id=s.id
             GROUP BY s.id ORDER BY s.account_label"
        )?;
        let rows = statement.query_map([], |row| {
            let last: Option<i64> = row.get(9)?;
            let next: Option<i64> = row.get(10)?;
            Ok(Source {
                id: row.get(0)?,
                kind: row.get(1)?,
                label: row.get(2)?,
                detail: row.get(3)?,
                status: row.get(4)?,
                health_detail: row.get(5)?,
                comments_status: row.get(6)?,
                comments_truncated: row.get::<_, i64>(7)? != 0,
                sync_finality: row.get(8)?,
                last_sync: last.map_or_else(|| "Not yet".to_owned(), iso),
                next_sync: next.map(iso),
                item_count: row.get(11)?,
            })
        })?;
        rows.collect()
    }

    fn load_activity(&self) -> rusqlite::Result<Vec<Activity>> {
        let mut statement = self.connection.prepare("SELECT id, kind, state, message, updated_at FROM jobs ORDER BY updated_at DESC LIMIT 20")?;
        let rows = statement.query_map([], |row| {
            Ok(Activity {
                id: row.get(0)?,
                kind: row.get(1)?,
                status: row.get(2)?,
                message: row.get(3)?,
                occurred_at: iso(row.get(4)?),
            })
        })?;
        rows.collect()
    }

    fn load_runner_status(&self, active: bool, in_flight: bool) -> rusqlite::Result<RunnerStatus> {
        self.connection.query_row(
            "SELECT last_attempt_at, last_success_at, next_scheduled_at, last_outcome, detail FROM runner_state WHERE singleton=1",
            [],
            |row| {
                let attempt: Option<i64> = row.get(0)?;
                let success: Option<i64> = row.get(1)?;
                let next: Option<i64> = row.get(2)?;
                Ok(RunnerStatus {
                    active,
                    in_flight,
                    last_attempt_at: attempt.map(iso),
                    last_success_at: success.map(iso),
                    next_scheduled_at: next.map(iso),
                    last_outcome: row.get(3)?,
                    detail: row.get(4)?,
                })
            },
        )
    }

    pub fn runner_status(&self, active: bool, in_flight: bool) -> AppResult<RunnerStatus> {
        self.load_runner_status(active, in_flight)
            .map_err(|_| AppError::internal())
    }

    fn load_settings(&self) -> rusqlite::Result<Settings> {
        let value: String = self.connection.query_row(
            "SELECT value_json FROM settings WHERE key='app'",
            [],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&value).unwrap_or_default())
    }
}

fn assert_runner_authority_connection(
    connection: &Connection,
    lease: &RunnerLease,
) -> AppResult<()> {
    let now = Utc::now().timestamp_millis();
    let current = connection
        .query_row(
            "SELECT 1 FROM runner_state WHERE singleton=1 AND lease_owner=?1 AND lease_token=?2 AND lease_expires_at > ?3",
            params![lease.owner, lease.token, now],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| AppError::internal())?;
    current.ok_or_else(|| {
        AppError::conflict("This resident worker no longer owns the scheduled lease.")
    })
}

fn assert_runner_authority(
    transaction: &Transaction<'_>,
    lease: Option<&RunnerLease>,
) -> AppResult<()> {
    let Some(lease) = lease else {
        return Ok(());
    };
    let now = Utc::now().timestamp_millis();
    let current = transaction
        .query_row(
            "SELECT 1 FROM runner_state WHERE singleton=1 AND lease_owner=?1 AND lease_token=?2 AND lease_expires_at > ?3",
            params![lease.owner, lease.token, now],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| AppError::internal())?;
    current.ok_or_else(|| {
        AppError::conflict("This resident worker no longer owns the scheduled lease.")
    })
}

fn validate_source_label(label: &str) -> AppResult<()> {
    if label.trim().is_empty() || label.chars().count() > 100 {
        return Err(AppError::validation(
            "The source label must be between 1 and 100 characters.",
        ));
    }
    Ok(())
}

fn post_content_hash(post: &NormalizedPost) -> String {
    content_hash(&format!("{}\n{}", post.title, post.body_text))
}

fn canonical_comment_order(
    left: &NormalizedComment,
    right: &NormalizedComment,
) -> std::cmp::Ordering {
    left.position
        .cmp(&right.position)
        .then(left.published_at.cmp(&right.published_at))
        .then(left.remote_id.cmp(&right.remote_id))
}

pub(crate) fn canonical_comments(comments: &[NormalizedComment]) -> Vec<NormalizedComment> {
    let mut ordered = comments.to_vec();
    ordered.sort_by(canonical_comment_order);
    ordered
}

pub(crate) fn comment_evidence_hash(
    comments: &[NormalizedComment],
    status: CommentCompleteness,
    truncated: bool,
) -> String {
    if status == CommentCompleteness::Unavailable {
        return "unavailable".to_owned();
    }
    let ordered = canonical_comments(comments);
    let evidence = ordered
        .iter()
        .map(|comment| {
            serde_json::json!([
                comment.remote_id,
                comment.parent_remote_id,
                comment.author,
                comment.body_text,
                comment.published_at,
                comment.depth,
                comment.position
            ])
        })
        .collect::<Vec<_>>();
    content_hash(
        &serde_json::json!({
            "status": status.as_str(),
            "truncated": truncated,
            "comments": evidence
        })
        .to_string(),
    )
}

pub(crate) fn summary_input_hash_for(
    post: &NormalizedPost,
    comments: &[NormalizedComment],
    status: CommentCompleteness,
    truncated: bool,
) -> String {
    let post_hash = post_content_hash(post);
    if status == CommentCompleteness::Unavailable {
        post_hash
    } else {
        content_hash(
            &serde_json::json!({
                "post": post_hash,
                "comment_evidence": comment_evidence_hash(comments, status, truncated)
            })
            .to_string(),
        )
    }
}

fn source_kind_str(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Rss => "rss",
        SourceKind::Mastodon => "mastodon",
        SourceKind::Bluesky => "bluesky",
    }
}

fn bounded_safe_detail(value: &str) -> String {
    value.trim().chars().take(240).collect()
}

fn load_stored_post(
    connection: &Connection,
    source_id: &str,
    remote_id: &str,
) -> AppResult<NormalizedPost> {
    connection
        .query_row(
            "SELECT p.remote_id, p.canonical_http_url, COALESCE(a.display_name, ''), p.title, p.body_text,
                    p.published_at, p.published_time_kind
             FROM posts p LEFT JOIN actors a ON a.id=p.actor_id
             WHERE p.source_id=?1 AND p.remote_id=?2 AND p.deleted_at IS NULL",
            params![source_id, remote_id],
            |row| {
                Ok(NormalizedPost {
                    remote_id: row.get(0)?,
                    canonical_url: row.get(1)?,
                    author: row.get(2)?,
                    title: row.get(3)?,
                    body_text: row.get(4)?,
                    published_at: row.get(5)?,
                    timestamp_kind: match row.get::<_, String>(6)?.as_str() {
                        "published" => crate::connectors::TimestampKind::Published,
                        "updated" => crate::connectors::TimestampKind::Updated,
                        _ => crate::connectors::TimestampKind::Fetched,
                    },
                })
            },
        )
        .optional()
        .map_err(|_| AppError::internal())?
        .ok_or_else(|| AppError::validation("A comment snapshot referenced an unknown post."))
}

fn load_stored_comments(
    connection: &Connection,
    source_id: &str,
    post_remote_id: &str,
) -> AppResult<Vec<NormalizedComment>> {
    let mut statement = connection
        .prepare(
            "SELECT c.remote_id, c.parent_remote_id, COALESCE(a.display_name, ''), c.body_text,
                    c.published_at, c.depth, c.position
             FROM comments c JOIN posts p ON p.id=c.post_id LEFT JOIN actors a ON a.id=c.actor_id
             WHERE c.source_id=?1 AND p.remote_id=?2 AND c.deleted_at IS NULL
             ORDER BY c.position, c.published_at, c.remote_id",
        )
        .map_err(|_| AppError::internal())?;
    statement
        .query_map(params![source_id, post_remote_id], |row| {
            Ok(NormalizedComment {
                post_remote_id: post_remote_id.to_owned(),
                remote_id: row.get(0)?,
                parent_remote_id: row.get(1)?,
                author: row.get(2)?,
                body_text: row.get(3)?,
                published_at: row.get(4)?,
                depth: row.get(5)?,
                position: row.get(6)?,
            })
        })
        .map_err(|_| AppError::internal())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::internal())
}

fn assert_comment_id_assignments(
    connection: &Connection,
    source_id: &str,
    batch: &SyncBatch,
) -> AppResult<()> {
    for comment in &batch.comments {
        let existing_post = connection
            .query_row(
                "SELECT post_remote_id FROM comment_identity_ledger
                 WHERE source_id=?1 AND remote_id=?2",
                params![source_id, comment.remote_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        if existing_post
            .as_deref()
            .is_some_and(|post_remote_id| post_remote_id != comment.post_remote_id)
        {
            return Err(AppError::conflict(
                "A provider comment identifier cannot move between posts.",
            ));
        }
    }
    Ok(())
}

fn insert_comment_identities(
    transaction: &Transaction<'_>,
    source: &SourceSyncSpec,
    batch: &SyncBatch,
) -> AppResult<()> {
    for comment in &batch.comments {
        transaction
            .execute(
                "INSERT OR IGNORE INTO comment_identity_ledger
                 (source_id, remote_id, post_remote_id, first_seen_generation)
                 SELECT id, ?1, ?2, generation FROM sources
                 WHERE id=?3 AND generation=?4 AND connector_kind=?5",
                params![
                    comment.remote_id,
                    comment.post_remote_id,
                    source.id,
                    source.generation,
                    source_kind_str(source.kind)
                ],
            )
            .map_err(|_| AppError::internal())?;
    }
    assert_comment_id_assignments(transaction, &source.id, batch)
}

fn prospective_comments(
    connection: &Connection,
    source_id: &str,
    batch: &SyncBatch,
    post_remote_id: &str,
) -> AppResult<Vec<NormalizedComment>> {
    let observed = batch
        .comments
        .iter()
        .filter(|comment| comment.post_remote_id == post_remote_id)
        .cloned()
        .collect::<Vec<_>>();
    if batch.comment_completeness == CommentCompleteness::Complete {
        return Ok(canonical_comments(&observed));
    }
    if batch.comment_completeness == CommentCompleteness::Unavailable {
        return Ok(Vec::new());
    }
    let mut merged = load_stored_comments(connection, source_id, post_remote_id)?
        .into_iter()
        .map(|comment| (comment.remote_id.clone(), comment))
        .collect::<HashMap<_, _>>();
    for comment in observed {
        merged.insert(comment.remote_id.clone(), comment);
    }
    let comments = merged.into_values().collect::<Vec<_>>();
    Ok(canonical_comments(&comments))
}

fn changed_posts_for_batch(
    connection: &Connection,
    source: &SourceSyncSpec,
    batch: &SyncBatch,
) -> AppResult<Vec<InferenceCandidate>> {
    let batch_posts = batch
        .posts
        .iter()
        .map(|post| (post.remote_id.as_str(), post))
        .collect::<HashMap<_, _>>();
    let mut remote_ids = batch
        .posts
        .iter()
        .map(|post| post.remote_id.clone())
        .collect::<BTreeSet<_>>();
    remote_ids.extend(batch.comment_scope_post_ids.iter().cloned());
    let mut changed = Vec::new();
    for remote_id in remote_ids {
        let post = batch_posts
            .get(remote_id.as_str())
            .map(|post| (*post).clone())
            .map(Ok)
            .unwrap_or_else(|| load_stored_post(connection, &source.id, &remote_id))?;
        let (status, truncated, comments) =
            if batch.comment_completeness == CommentCompleteness::Unavailable {
                (CommentCompleteness::Unavailable, false, Vec::new())
            } else {
                (
                    batch.comment_completeness,
                    batch.comments_truncated,
                    prospective_comments(connection, &source.id, batch, &remote_id)?,
                )
            };
        let evidence_hash = comment_evidence_hash(&comments, status, truncated);
        let expected = summary_input_hash_for(&post, &comments, status, truncated);
        let previous = connection
            .query_row(
                "SELECT COALESCE(NULLIF(pcs.summary_input_hash, ''), p.content_hash)
                 FROM posts p LEFT JOIN post_comment_state pcs ON pcs.post_id=p.id
                 WHERE p.source_id=?1 AND p.remote_id=?2 AND p.deleted_at IS NULL",
                params![source.id, remote_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        if previous.as_deref() != Some(expected.as_str()) {
            changed.push(InferenceCandidate {
                post,
                comments,
                comment_completeness: status,
                comments_truncated: truncated,
                evidence_hash,
                input_hash: expected,
            });
        }
    }
    Ok(changed)
}

fn validate_prepared_posts(
    expected: &[InferenceCandidate],
    prepared: &[PreparedPost],
) -> AppResult<()> {
    let expected_keys = expected
        .iter()
        .map(|candidate| {
            (
                candidate.post.remote_id.as_str(),
                candidate.input_hash.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    let prepared_keys = prepared
        .iter()
        .map(|item| (item.post.remote_id.as_str(), item.input_hash.as_str()))
        .collect::<BTreeSet<_>>();
    let prepared_remote_ids = prepared
        .iter()
        .map(|item| item.post.remote_id.as_str())
        .collect::<BTreeSet<_>>();
    if expected_keys.len() != expected.len()
        || prepared_keys.len() != prepared.len()
        || prepared_remote_ids.len() != prepared.len()
        || prepared_keys != expected_keys
    {
        return Err(AppError::conflict(
            "Prepared summaries do not match the current post and comment evidence.",
        ));
    }
    Ok(())
}

fn upsert_posts_only(
    transaction: &Transaction<'_>,
    source_id: &str,
    posts: &[NormalizedPost],
    now: i64,
) -> AppResult<()> {
    for post in posts {
        let actor_id = format!(
            "actor-{}",
            &content_hash(&format!("{source_id}:{}", post.author))[..20]
        );
        transaction
            .execute(
                "INSERT INTO actors(id, source_id, remote_id, display_name) VALUES(?1, ?2, ?3, ?3)
             ON CONFLICT(source_id, remote_id) DO UPDATE SET display_name=excluded.display_name",
                params![actor_id, source_id, post.author],
            )
            .map_err(|_| AppError::internal())?;
        let post_id = format!(
            "post-{}",
            &content_hash(&format!("{source_id}:{}", post.remote_id))[..20]
        );
        let hash = post_content_hash(post);
        transaction.execute(
            "INSERT INTO posts(id, source_id, remote_id, canonical_url, canonical_http_url, actor_id, title, body_text, published_at, published_time_kind, fetched_at, content_hash)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(source_id, remote_id) DO UPDATE SET canonical_url=excluded.canonical_url, canonical_http_url=excluded.canonical_http_url, actor_id=excluded.actor_id, title=excluded.title, body_text=excluded.body_text, published_at=excluded.published_at, published_time_kind=excluded.published_time_kind, fetched_at=excluded.fetched_at, content_hash=excluded.content_hash, deleted_at=NULL",
            params![post_id, source_id, post.remote_id, post.canonical_url.clone().unwrap_or_default(), post.canonical_url, actor_id, post.title, post.body_text, post.published_at, post.timestamp_kind.as_str(), now, hash],
        ).map_err(|_| AppError::internal())?;
    }
    Ok(())
}

fn reconcile_comments(
    transaction: &Transaction<'_>,
    source_id: &str,
    batch: &SyncBatch,
    now: i64,
) -> AppResult<bool> {
    let mut removed = false;
    if batch.comment_completeness == CommentCompleteness::Complete {
        for remote_id in &batch.comment_scope_post_ids {
            let post_id = transaction
                .query_row(
                    "SELECT id FROM posts WHERE source_id=?1 AND remote_id=?2 AND deleted_at IS NULL",
                    params![source_id, remote_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| AppError::internal())?
                .ok_or_else(|| AppError::validation("A comment snapshot referenced an unknown post."))?;
            let observed = batch
                .comments
                .iter()
                .filter(|comment| comment.post_remote_id == *remote_id)
                .map(|comment| comment.remote_id.as_str())
                .collect::<std::collections::HashSet<_>>();
            let existing = {
                let mut statement = transaction
                    .prepare(
                        "SELECT remote_id FROM comments WHERE post_id=?1 AND deleted_at IS NULL",
                    )
                    .map_err(|_| AppError::internal())?;
                statement
                    .query_map([&post_id], |row| row.get::<_, String>(0))
                    .map_err(|_| AppError::internal())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| AppError::internal())?
            };
            for existing_id in existing {
                if !observed.contains(existing_id.as_str()) {
                    transaction
                        .execute(
                            "DELETE FROM comments WHERE post_id=?1 AND remote_id=?2",
                            params![post_id, existing_id],
                        )
                        .map_err(|_| AppError::internal())?;
                    removed = true;
                }
            }
        }
    }
    ingest_comments(transaction, source_id, batch, now)?;
    if removed {
        transaction
            .execute(
                "DELETE FROM actors WHERE source_id=?1
             AND NOT EXISTS (SELECT 1 FROM posts WHERE posts.actor_id=actors.id)
             AND NOT EXISTS (SELECT 1 FROM comments WHERE comments.actor_id=actors.id)",
                [source_id],
            )
            .map_err(|_| AppError::internal())?;
    }
    Ok(removed)
}

fn update_comment_state(
    transaction: &Transaction<'_>,
    source_id: &str,
    batch: &SyncBatch,
    now: i64,
) -> AppResult<()> {
    let targets = if batch.comment_completeness == CommentCompleteness::Unavailable {
        batch
            .posts
            .iter()
            .map(|post| post.remote_id.clone())
            .collect::<Vec<_>>()
    } else {
        batch.comment_scope_post_ids.clone()
    };
    for remote_id in targets {
        let post = load_stored_post(transaction, source_id, &remote_id)?;
        let comments = load_stored_comments(transaction, source_id, &remote_id)?;
        let evidence_hash = comment_evidence_hash(
            &comments,
            batch.comment_completeness,
            batch.comments_truncated,
        );
        let summary_hash = summary_input_hash_for(
            &post,
            &comments,
            batch.comment_completeness,
            batch.comments_truncated,
        );
        transaction.execute(
            "INSERT INTO post_comment_state(post_id, status, truncated, fetched_at, evidence_hash, summary_input_hash)
             SELECT id, ?1, ?2, ?3, ?4, ?5 FROM posts WHERE source_id=?6 AND remote_id=?7
             ON CONFLICT(post_id) DO UPDATE SET status=excluded.status, truncated=excluded.truncated, fetched_at=excluded.fetched_at, evidence_hash=excluded.evidence_hash, summary_input_hash=excluded.summary_input_hash",
            params![batch.comment_completeness.as_str(), i64::from(batch.comments_truncated), now, evidence_hash, summary_hash, source_id, remote_id],
        ).map_err(|_| AppError::internal())?;
    }
    Ok(())
}

fn persist_prepared_summaries(
    transaction: &Transaction<'_>,
    source_id: &str,
    prepared: Vec<PreparedPost>,
    now: i64,
) -> AppResult<()> {
    for prepared_post in prepared {
        let (post_id, expected, evidence_hash, status, truncated): (String, String, String, String, i64) = transaction
            .query_row(
                "SELECT p.id, COALESCE(NULLIF(pcs.summary_input_hash, ''), p.content_hash),
                        COALESCE(pcs.evidence_hash, 'unavailable'), COALESCE(pcs.status, 'unavailable'),
                        COALESCE(pcs.truncated, 0)
                 FROM posts p LEFT JOIN post_comment_state pcs ON pcs.post_id=p.id
                 WHERE p.source_id=?1 AND p.remote_id=?2 AND p.deleted_at IS NULL",
                params![source_id, prepared_post.post.remote_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .map_err(|_| AppError::internal())?;
        if expected != prepared_post.input_hash {
            return Err(AppError::conflict(
                "Comment evidence changed before its summary could be committed.",
            ));
        }
        transaction
            .execute("DELETE FROM summaries WHERE post_id=?1", [&post_id])
            .map_err(|_| AppError::internal())?;
        transaction.execute(
            "INSERT INTO summaries(id, post_id, summary_text, comment_overview, provenance_json, provider, model_id, prompt_version, input_hash, created_at, summary_method, uncertainty)
             VALUES(?1, ?2, ?3, ?4, json_object('post_id', ?2, 'input_hash', ?8, 'comment_evidence_hash', ?12, 'comment_status', ?13, 'comments_truncated', ?14, 'prompt_version', ?7, 'provider', ?5, 'model_id', ?6), ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![uuid::Uuid::new_v4().to_string(), post_id, prepared_post.summary.summary, prepared_post.summary.comment_overview, prepared_post.provider, prepared_post.model_id, prepared_post.prompt_version, prepared_post.input_hash, now, prepared_post.summary_method, prepared_post.summary.uncertainty, evidence_hash, status, truncated],
        ).map_err(|_| AppError::internal())?;
    }
    Ok(())
}

fn ingest_comments(
    transaction: &Transaction<'_>,
    source_id: &str,
    batch: &SyncBatch,
    now: i64,
) -> AppResult<()> {
    let mut comments = batch.comments.clone();
    comments.sort_by(|left, right| {
        left.post_remote_id
            .cmp(&right.post_remote_id)
            .then_with(|| canonical_comment_order(left, right))
    });
    for comment in comments {
        let post_id = transaction
            .query_row(
                "SELECT id FROM posts WHERE source_id=?1 AND remote_id=?2 AND deleted_at IS NULL",
                params![source_id, comment.post_remote_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?
            .ok_or_else(|| AppError::validation("A comment referenced an unknown post."))?;
        let actor_id = format!(
            "actor-{}",
            &content_hash(&format!("{source_id}:{}", comment.author))[..20]
        );
        transaction
            .execute(
                "INSERT INTO actors(id, source_id, remote_id, display_name) VALUES(?1, ?2, ?3, ?3)
             ON CONFLICT(source_id, remote_id) DO UPDATE SET display_name=excluded.display_name",
                params![actor_id, source_id, comment.author],
            )
            .map_err(|_| AppError::internal())?;
        let comment_id = format!(
            "comment-{}",
            &content_hash(&format!("{source_id}:{}", comment.remote_id))[..20]
        );
        let hash = content_hash(&comment.body_text);
        transaction.execute(
            "INSERT INTO comments(id, post_id, source_id, remote_id, parent_remote_id, actor_id, body_text, published_at, fetched_at, content_hash, depth, position)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(source_id, remote_id) DO UPDATE SET post_id=excluded.post_id, parent_remote_id=excluded.parent_remote_id, actor_id=excluded.actor_id, body_text=excluded.body_text, published_at=excluded.published_at, fetched_at=excluded.fetched_at, content_hash=excluded.content_hash, depth=excluded.depth, position=excluded.position, deleted_at=NULL",
            params![comment_id, post_id, source_id, comment.remote_id, comment.parent_remote_id, actor_id, comment.body_text, comment.published_at, now, hash, comment.depth, comment.position],
        ).map_err(|_| AppError::internal())?;
    }
    Ok(())
}

fn set_comment_state(
    transaction: &Transaction<'_>,
    source_id: &str,
    posts: &[NormalizedPost],
    status: CommentCompleteness,
    truncated: bool,
    now: i64,
) -> AppResult<()> {
    for post in posts {
        let evidence_hash = comment_evidence_hash(&[], status, truncated);
        let summary_hash = summary_input_hash_for(post, &[], status, truncated);
        transaction.execute(
            "INSERT INTO post_comment_state(post_id, status, truncated, fetched_at, evidence_hash, summary_input_hash)
             SELECT id, ?1, ?2, ?3, ?4, ?5 FROM posts WHERE source_id=?6 AND remote_id=?7
             ON CONFLICT(post_id) DO UPDATE SET status=excluded.status, truncated=excluded.truncated, fetched_at=excluded.fetched_at, evidence_hash=excluded.evidence_hash, summary_input_hash=excluded.summary_input_hash",
            params![status.as_str(), i64::from(truncated), now, evidence_hash, summary_hash, source_id, post.remote_id],
        ).map_err(|_| AppError::internal())?;
    }
    Ok(())
}

fn ingest_posts(
    transaction: &Transaction<'_>,
    source_id: &str,
    posts: &[NormalizedPost],
    prepared: Vec<PreparedPost>,
    now: i64,
) -> AppResult<usize> {
    let mut prepared = prepared
        .into_iter()
        .map(|item| (item.post.remote_id.clone(), item))
        .collect::<HashMap<_, _>>();
    let mut changed_count = 0_usize;
    for post in posts {
        let hash = post_content_hash(post);
        let previous_hash = transaction
            .query_row(
                "SELECT content_hash FROM posts WHERE source_id=?1 AND remote_id=?2 AND deleted_at IS NULL",
                params![source_id, post.remote_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| AppError::internal())?;
        let content_changed = previous_hash.as_deref() != Some(hash.as_str());
        let actor_id = format!(
            "actor-{}",
            &content_hash(&format!("{source_id}:{}", post.author))[..20]
        );
        transaction.execute(
            "INSERT INTO actors(id, source_id, remote_id, display_name) VALUES(?1, ?2, ?3, ?3) ON CONFLICT(source_id, remote_id) DO UPDATE SET display_name=excluded.display_name",
            params![actor_id, source_id, post.author],
        ).map_err(|_| AppError::internal())?;
        let post_id = format!(
            "post-{}",
            &content_hash(&format!("{source_id}:{}", post.remote_id))[..20]
        );
        transaction.execute(
            "INSERT INTO posts(id, source_id, remote_id, canonical_url, canonical_http_url, actor_id, title, body_text, published_at, published_time_kind, fetched_at, content_hash)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(source_id, remote_id) DO UPDATE SET canonical_url=excluded.canonical_url, canonical_http_url=excluded.canonical_http_url, actor_id=excluded.actor_id, title=excluded.title, body_text=excluded.body_text, published_at=excluded.published_at, published_time_kind=excluded.published_time_kind, fetched_at=excluded.fetched_at, content_hash=excluded.content_hash, deleted_at=NULL",
            params![post_id, source_id, post.remote_id, post.canonical_url.clone().unwrap_or_default(), post.canonical_url, actor_id, post.title, post.body_text, post.published_at, post.timestamp_kind.as_str(), now, hash],
        ).map_err(|_| AppError::internal())?;
        if content_changed {
            let prepared_post = prepared
                .remove(&post.remote_id)
                .ok_or_else(AppError::internal)?;
            if prepared_post.input_hash != hash {
                return Err(AppError::conflict(
                    "Prepared RSS summary did not match the post evidence.",
                ));
            }
            transaction
                .execute("DELETE FROM summaries WHERE post_id=?1", [&post_id])
                .map_err(|_| AppError::internal())?;
            transaction.execute(
                "INSERT INTO summaries(id, post_id, summary_text, comment_overview, provenance_json, provider, model_id, prompt_version, input_hash, created_at, summary_method, uncertainty)
                 VALUES(?1, ?2, ?3, ?4, json_object('post_id', ?2, 'input_hash', ?8, 'comment_evidence_hash', 'unavailable', 'comment_status', 'unavailable', 'comments_truncated', 0, 'prompt_version', ?7, 'provider', ?5, 'model_id', ?6), ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![uuid::Uuid::new_v4().to_string(), post_id, prepared_post.summary.summary, prepared_post.summary.comment_overview, prepared_post.provider, prepared_post.model_id, prepared_post.prompt_version, prepared_post.input_hash, now, prepared_post.summary_method, prepared_post.summary.uncertainty],
            ).map_err(|_| AppError::internal())?;
            changed_count += 1;
        }
    }
    if !prepared.is_empty() {
        return Err(AppError::internal());
    }
    Ok(changed_count)
}

fn record_sync_job(
    transaction: &Transaction<'_>,
    request_id: &str,
    item_count: usize,
    now: i64,
) -> AppResult<()> {
    record_connector_sync_job(
        transaction,
        request_id,
        "rss",
        item_count,
        PageFinality::Complete,
        now,
    )
}

fn record_connector_sync_job(
    transaction: &Transaction<'_>,
    request_id: &str,
    kind: &str,
    item_count: usize,
    finality: PageFinality,
    now: i64,
) -> AppResult<()> {
    let label = match kind {
        "rss" => "RSS",
        "mastodon" => "Mastodon",
        "bluesky" => "Bluesky",
        _ => return Err(AppError::internal()),
    };
    let (state, message) = match finality {
        PageFinality::Complete => (
            "complete",
            format!("{label} source synchronized ({item_count} items)"),
        ),
        PageFinality::Partial => (
            "partial",
            format!("{label} source synchronized a bounded partial page ({item_count} items)"),
        ),
    };
    transaction.execute(
        "INSERT OR REPLACE INTO jobs(id, kind, dedupe_key, state, attempts, run_after, message, created_at, updated_at) VALUES(?1, 'sync', ?2, ?3, 1, ?4, ?5, ?4, ?4)",
        params![uuid::Uuid::new_v4().to_string(), format!("{kind}-sync:{request_id}"), state, now, message],
    ).map_err(|_| AppError::internal())?;
    Ok(())
}

pub(crate) fn validate_id(value: &str) -> AppResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':'))
    {
        return Err(AppError::validation("The request identifier is invalid."));
    }
    Ok(())
}

fn sanitize_legacy_links(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    let values = {
        let mut statement = transaction.prepare("SELECT id, canonical_url FROM posts")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (id, value) in values {
        let canonical = sanitize_legacy_canonical_url(&value);
        transaction.execute(
            "UPDATE posts SET canonical_http_url=?1 WHERE id=?2",
            params![canonical, id],
        )?;
    }
    Ok(())
}

fn sanitize_legacy_canonical_url(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 2_048 || value.trim() != value {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host().is_none()
    {
        return None;
    }
    Some(url.into())
}

fn apply_retention_in_transaction(
    transaction: &Transaction<'_>,
    retention_days: u16,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let cutoff = now_ms - Duration::days(i64::from(retention_days)).num_milliseconds();
    let expired_posts: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM posts WHERE fetched_at < ?1",
        [cutoff],
        |row| row.get(0),
    )?;
    transaction.execute(
        "DELETE FROM trend_clusters WHERE id IN (
            SELECT DISTINCT tm.cluster_id FROM trend_members tm JOIN posts p ON p.id=tm.post_id WHERE p.fetched_at < ?1
         )",
        [cutoff],
    )?;
    transaction.execute(
        "DELETE FROM digests WHERE id IN (
            SELECT DISTINCT di.digest_id FROM digest_items di JOIN posts p ON p.id=di.post_id WHERE p.fetched_at < ?1
         )",
        [cutoff],
    )?;
    let comment_affected_posts = {
        let mut statement = transaction.prepare(
            "SELECT DISTINCT c.post_id FROM comments c JOIN posts p ON p.id=c.post_id
             WHERE c.fetched_at < ?1 AND p.fetched_at >= ?1",
        )?;
        statement
            .query_map([cutoff], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    transaction.execute("DELETE FROM comments WHERE fetched_at < ?1", [cutoff])?;
    for post_id in &comment_affected_posts {
        transaction.execute("DELETE FROM summaries WHERE post_id=?1", [post_id])?;
        let (source_id, remote_id): (String, String) = transaction.query_row(
            "SELECT source_id, remote_id FROM posts WHERE id=?1",
            [post_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let post = load_stored_post(transaction, &source_id, &remote_id)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let comments = load_stored_comments(transaction, &source_id, &remote_id)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let evidence_hash = comment_evidence_hash(&comments, CommentCompleteness::Partial, true);
        let summary_hash =
            summary_input_hash_for(&post, &comments, CommentCompleteness::Partial, true);
        transaction.execute(
            "UPDATE post_comment_state SET status='partial', truncated=1, fetched_at=?1,
                    evidence_hash=?2, summary_input_hash=?3 WHERE post_id=?4",
            params![now_ms, evidence_hash, summary_hash, post_id],
        )?;
        transaction.execute(
            "UPDATE source_sync_metadata SET comments_status='partial', comments_truncated=1,
                    safe_detail='Some retained comment evidence expired; refresh is required.', updated_at=?1
             WHERE source_id=?2",
            params![now_ms, source_id],
        )?;
    }
    transaction.execute("DELETE FROM posts WHERE fetched_at < ?1", [cutoff])?;
    transaction.execute(
        "DELETE FROM actors WHERE NOT EXISTS (SELECT 1 FROM posts WHERE posts.actor_id=actors.id)
         AND NOT EXISTS (SELECT 1 FROM comments WHERE comments.actor_id=actors.id)",
        [],
    )?;
    if expired_posts > 0 || !comment_affected_posts.is_empty() {
        transaction.execute(
            "UPDATE app_state SET privacy_epoch=privacy_epoch+1 WHERE singleton=1",
            [],
        )?;
    }
    ensure_ready_edition(transaction, now_ms)
}

fn ensure_ready_edition(transaction: &Transaction<'_>, now_ms: i64) -> rusqlite::Result<()> {
    if transaction
        .query_row(
            "SELECT 1 FROM digests WHERE status='ready' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_none()
    {
        transaction.execute(
            "INSERT INTO digests(id, label, period_start, period_end, generated_at, next_edition_at, status, overview)
             VALUES(?1, 'Current edition', ?2, ?2, ?2, 0, 'ready', 'No retained items are ready. Add or synchronize a source to prepare a new finite edition.')",
            params![uuid::Uuid::new_v4().to_string(), now_ms],
        )?;
    }
    Ok(())
}

fn parse_timestamp_kind(value: &str) -> TimestampKind {
    match value {
        "published" => TimestampKind::Published,
        "updated" => TimestampKind::Updated,
        _ => TimestampKind::Fetched,
    }
}

fn parse_trend_method(value: &str) -> TrendMethod {
    match value {
        "lexical" => TrendMethod::Lexical,
        "embedding" => TrendMethod::Embedding,
        _ => TrendMethod::Fixture,
    }
}

pub(crate) fn content_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn iso(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp)
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ModelState;

    fn model() -> ModelStatus {
        ModelStatus {
            provider: "test".into(),
            state: ModelState::RuntimeUnavailable,
            model: None,
            digest: None,
            size_bytes: None,
            parameter_size: None,
            quantization: None,
            runtime_version: None,
            structured_output: false,
            endpoint: "http://127.0.0.1:11434".into(),
            fallback_available: true,
            detail: "test".into(),
        }
    }

    fn host() -> HostCapabilities {
        crate::capabilities::detect_host(&model())
    }

    fn source_spec(database: &Database, source_id: &str) -> RssSourceSpec {
        let (generation, label): (i64, String) = database
            .connection
            .query_row(
                "SELECT generation, account_label FROM sources WHERE id=?1",
                [source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("source spec");
        RssSourceSpec {
            id: source_id.into(),
            generation,
            label,
            requested_url: "https://example.test/feed".into(),
            effective_url: Some("https://cdn.example.test/feed".into()),
            etag: Some("\"old\"".into()),
            last_modified: None,
        }
    }

    fn normalized(remote_id: &str, title: &str, body: &str) -> NormalizedPost {
        NormalizedPost {
            remote_id: remote_id.into(),
            canonical_url: Some(format!("https://example.test/{remote_id}")),
            author: "Author".into(),
            title: title.into(),
            body_text: body.into(),
            published_at: Utc::now().timestamp_millis(),
            timestamp_kind: crate::connectors::TimestampKind::Published,
        }
    }

    fn fallback_prepared(post: NormalizedPost) -> PreparedPost {
        let input_hash =
            summary_input_hash_for(&post, &[], CommentCompleteness::Unavailable, false);
        PreparedPost {
            input_hash,
            summary: GroundedSummary {
                summary: post.body_text.clone(),
                comment_overview: "No comments.".into(),
                uncertainty: "Fallback.".into(),
            },
            post,
            provider: "deterministic-fallback".into(),
            model_id: None,
            prompt_version: "extractive-v1".into(),
            summary_method: "extractive".into(),
        }
    }

    fn seed_post(database: &mut Database, source_id: &str, post_id: &str) {
        let now = Utc::now().timestamp_millis();
        database.connection.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at)
             VALUES(?1, 'rss', ?1, 'test', 'healthy', '{}', ?2, ?2)",
            params![source_id, now],
        ).expect("source");
        database.connection.execute(
            "INSERT INTO posts(id, source_id, remote_id, canonical_url, canonical_http_url, title, body_text, published_at, published_time_kind, fetched_at, content_hash)
             VALUES(?1, ?2, ?1, 'https://example.test/item', 'https://example.test/item', 'Test item', 'Current evidence.', ?3, 'published', ?3, ?4)",
            params![post_id, source_id, now, content_hash("Test item\nCurrent evidence.")],
        ).expect("post");
        database.connection.execute(
            "INSERT INTO summaries(id, post_id, summary_text, comment_overview, provenance_json, provider, prompt_version, input_hash, created_at, summary_method, uncertainty)
             SELECT ?1, ?2, 'Current evidence.', 'No comments.', json_array(?2), 'deterministic-fallback', 'extractive-v1', content_hash, ?3, 'extractive', 'Fallback.' FROM posts WHERE id=?2",
            params![format!("summary-{post_id}"), post_id, now],
        ).expect("summary");
    }

    fn seed_trend(database: &Database, cluster_id: &str, member_ids: &[&str]) {
        let edition_id = database.load_edition().expect("edition").id;
        database
            .connection
            .execute(
                "INSERT INTO trend_clusters(id, digest_id, label, summary, confidence, created_at, cluster_method)
                 VALUES(?1, ?2, 'Derived from source A', 'Sensitive derivative from source A.', 'supported', ?3, 'lexical')",
                params![cluster_id, edition_id, Utc::now().timestamp_millis()],
            )
            .expect("trend");
        for post_id in member_ids {
            database
                .connection
                .execute(
                    "INSERT INTO trend_members(cluster_id, post_id) VALUES(?1, ?2)",
                    params![cluster_id, post_id],
                )
                .expect("trend member");
        }
    }

    fn privacy_epoch(database: &Database) -> u64 {
        database
            .dashboard(model(), host())
            .expect("dashboard")
            .privacy_epoch
    }

    #[test]
    fn fresh_and_reopened_desktop_database_is_empty_and_valid() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("web.sqlite3");
        for _ in 0..2 {
            let database = Database::open(&path).expect("database");
            let dashboard = database.dashboard(model(), host()).expect("dashboard");
            assert!(dashboard.sources.is_empty());
            assert!(dashboard.items.is_empty());
            assert!(dashboard.trends.is_empty());
            assert!(dashboard.activity.is_empty());
        }
    }

    #[test]
    fn v1_upgrade_removes_legacy_fixture_rows_and_rejects_future_schema() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("upgrade.sqlite3");
        {
            let connection = Connection::open(&path).expect("connection");
            connection.execute_batch(MIGRATION_1).expect("v1");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(1, 1)",
                    [],
                )
                .expect("version");
            connection.execute(
                "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at)
                 VALUES('source-rss-ai', 'rss', 'fixture', '', 'healthy', '{}', 1, 1)",
                [],
            ).expect("legacy fixture");
        }
        let database = Database::open(&path).expect("upgrade");
        assert!(
            database
                .dashboard(model(), host())
                .expect("dashboard")
                .sources
                .is_empty()
        );
        drop(database);

        let future = directory.path().join("future.sqlite3");
        let connection = Connection::open(&future).expect("future");
        connection.execute_batch(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
             INSERT INTO schema_migrations VALUES(99, 1);",
        ).expect("future schema");
        drop(connection);
        assert!(Database::open(&future).is_err());
    }

    #[test]
    fn v2_upgrade_sanitizes_legacy_links_and_defaults_time_provenance() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("v2.sqlite3");
        {
            let connection = Connection::open(&path).expect("connection");
            connection.execute_batch(MIGRATION_1).expect("v1");
            connection.execute_batch(MIGRATION_2).expect("v2");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(1, 1), (2, 2)",
                    [],
                )
                .expect("versions");
            connection.execute("INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at) VALUES('legacy-source', 'rss', 'legacy', '', 'healthy', '{}', 1, 1)", []).expect("source");
            for (id, link) in [
                ("valid", "https://example.test/post"),
                ("malformed", "http://"),
                ("credentials", "https://user:pass@example.test/post"),
                ("fragment", "https://example.test/post#secret"),
                ("scheme", "javascript:alert(1)"),
            ] {
                connection.execute(
                    "INSERT INTO posts(id, source_id, remote_id, canonical_url, title, body_text, published_at, fetched_at, content_hash) VALUES(?1, 'legacy-source', ?1, ?2, 'title', 'body', ?3, ?3, 'hash')",
                    params![id, link, Utc::now().timestamp_millis()],
                ).expect("post");
            }
        }
        let database = Database::open(&path).expect("upgrade");
        let valid: Option<String> = database
            .connection
            .query_row(
                "SELECT canonical_http_url FROM posts WHERE id='valid'",
                [],
                |row| row.get(0),
            )
            .expect("valid");
        assert_eq!(valid.as_deref(), Some("https://example.test/post"));
        let unsafe_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM posts WHERE id!='valid' AND canonical_http_url IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("unsafe");
        assert_eq!(unsafe_count, 0);
        let kinds: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM posts WHERE published_time_kind='fetched'",
                [],
                |row| row.get(0),
            )
            .expect("kinds");
        assert_eq!(kinds, 5);
        let placeholders: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM digests WHERE next_edition_at != 0",
                [],
                |row| row.get(0),
            )
            .expect("placeholder");
        assert_eq!(placeholders, 0);
    }

    #[test]
    fn failed_migration_rolls_back_and_future_schema_fails_closed() {
        let connection = Connection::open_in_memory().expect("connection");
        connection.execute_batch(MIGRATION_1).expect("v1");
        connection.execute_batch(MIGRATION_2).expect("v2");
        connection
            .execute("ALTER TABLE posts ADD COLUMN canonical_http_url TEXT", [])
            .expect("conflict column");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(1,1),(2,2)",
                [],
            )
            .expect("versions");
        assert!(Database::from_connection(connection).is_err());
    }

    #[test]
    fn feedback_is_payload_bound_idempotent_and_reset_blocks_delayed_replay() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        database.run_digest("digest-a").expect("digest");
        database
            .record_feedback("feedback-a", "post-a", &FeedbackSignal::NotRelevant)
            .expect("feedback");
        database
            .record_feedback("feedback-a", "post-a", &FeedbackSignal::NotRelevant)
            .expect("replay");
        assert!(
            database
                .record_feedback("feedback-a", "post-a", &FeedbackSignal::MoreLikeThis)
                .is_err()
        );
        assert!(
            database
                .dashboard(model(), host())
                .expect("dashboard")
                .items
                .is_empty()
        );
        database.undo_feedback("feedback-a").expect("undo");
        database.undo_feedback("feedback-a").expect("undo replay");
        assert_eq!(
            database
                .dashboard(model(), host())
                .expect("dashboard")
                .items
                .len(),
            1
        );
        database.reset_learning("reset-a").expect("reset");
        database
            .record_feedback("feedback-a", "post-a", &FeedbackSignal::NotRelevant)
            .expect("delayed replay");
        assert_eq!(
            database
                .dashboard(model(), host())
                .expect("dashboard")
                .items
                .len(),
            1
        );
    }

    #[test]
    fn privacy_feedback_suppresses_the_whole_derived_trend_until_undo_or_reset() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        seed_post(&mut database, "source-b", "post-b");
        seed_post(&mut database, "source-c", "post-c");
        seed_trend(&database, "trend-ab", &["post-a", "post-b"]);

        let initial = database.dashboard(model(), host()).expect("dashboard");
        assert_eq!(initial.trends.len(), 1);
        assert_eq!(initial.trends[0].source_count, 2);
        assert_eq!(initial.trends[0].evidence_ids.len(), 2);

        // Active feedback on evidence outside this cluster must not suppress its derivative.
        database
            .record_feedback("unrelated-hidden", "post-c", &FeedbackSignal::NotRelevant)
            .expect("unrelated feedback");
        assert_eq!(
            database
                .dashboard(model(), host())
                .expect("unrelated dashboard")
                .trends
                .len(),
            1
        );

        database
            .record_feedback("mute-a", "post-a", &FeedbackSignal::MuteSource)
            .expect("mute member source");
        assert!(
            database
                .dashboard(model(), host())
                .expect("muted dashboard")
                .trends
                .is_empty()
        );
        let muted_epoch = privacy_epoch(&database);
        database.undo_feedback("mute-a").expect("undo mute");
        let after_undo = database.dashboard(model(), host()).expect("undo dashboard");
        assert_eq!(after_undo.trends.len(), 1);
        assert_eq!(after_undo.privacy_epoch, muted_epoch);

        // Inactive/retracted privacy feedback and active ranking-only feedback do not suppress.
        database
            .record_feedback("ranking-a", "post-a", &FeedbackSignal::LessLikeThis)
            .expect("ranking feedback");
        assert_eq!(
            database
                .dashboard(model(), host())
                .expect("ranking dashboard")
                .trends
                .len(),
            1
        );

        database
            .record_feedback("hide-a", "post-a", &FeedbackSignal::NotRelevant)
            .expect("hide member");
        assert!(
            database
                .dashboard(model(), host())
                .expect("hidden dashboard")
                .trends
                .is_empty()
        );
        let hidden_epoch = privacy_epoch(&database);
        database.reset_learning("reset-trends").expect("reset");
        let after_reset = database
            .dashboard(model(), host())
            .expect("reset dashboard");
        assert_eq!(after_reset.trends.len(), 1);
        assert_eq!(after_reset.privacy_epoch, hidden_epoch);
    }

    #[test]
    fn v4_upgrade_clears_unbound_validators_before_first_v5_sync() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("fk");
        connection
            .execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);")
            .expect("migration table");
        for (version, sql) in [
            (1_i64, MIGRATION_1),
            (2_i64, MIGRATION_2),
            (3_i64, MIGRATION_3),
            (4_i64, MIGRATION_4),
        ] {
            connection.execute_batch(sql).expect("legacy migration");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, 0)",
                    [version],
                )
                .expect("migration row");
        }
        connection.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, etag, last_modified, created_at, updated_at)
             VALUES('legacy-rss','rss','Legacy','RSS','healthy','{\"url\":\"https://origin.example.test/feed\"}','\"secret-cdn-tag\"','Sun, 06 Nov 1994 08:49:37 GMT',0,0)",
            [],
        ).expect("legacy source");
        let database = Database::from_connection(connection).expect("upgrade");
        let (sources, _) = database
            .rss_sources(SourceSelectionMode::ManualOverride, 0, 20)
            .expect("first v5 source selection");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].sync_url(), "https://origin.example.test/feed");
        assert!(sources[0].etag.is_none());
        assert!(sources[0].last_modified.is_none());
        assert!(sources[0].effective_url.is_none());
    }

    #[test]
    fn v5_upgrade_repairs_only_unbound_validators() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("fk");
        connection
            .execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);")
            .expect("migration table");
        for (version, sql) in [
            (1_i64, MIGRATION_1),
            (2_i64, MIGRATION_2),
            (3_i64, MIGRATION_3),
            (4_i64, MIGRATION_4),
            (5_i64, MIGRATION_5),
        ] {
            connection.execute_batch(sql).expect("legacy migration");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, 0)",
                    [version],
                )
                .expect("migration row");
        }
        connection.execute_batch(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, etag, last_modified, validator_url, created_at, updated_at)
             VALUES('unbound','rss','Unbound','RSS','healthy','{\"url\":\"https://origin.example.test/feed\"}','\"legacy\"','Sun, 06 Nov 1994 08:49:37 GMT',NULL,0,0);
             INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, etag, last_modified, validator_url, created_at, updated_at)
             VALUES('bound','rss','Bound','RSS','healthy','{\"url\":\"https://origin.example.test/bound\"}','\"current\"','Sun, 06 Nov 1994 08:49:37 GMT','https://cdn.example.test/bound',0,0);",
        ).expect("v5 sources");
        let database = Database::from_connection(connection).expect("upgrade");
        let unbound: (Option<String>, Option<String>) = database
            .connection
            .query_row(
                "SELECT etag, last_modified FROM sources WHERE id='unbound'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("unbound");
        assert_eq!(unbound, (None, None));
        let bound: (Option<String>, Option<String>, Option<String>) = database
            .connection
            .query_row(
                "SELECT etag, last_modified, validator_url FROM sources WHERE id='bound'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("bound");
        assert_eq!(bound.0.as_deref(), Some("\"current\""));
        assert_eq!(bound.1.as_deref(), Some("Sun, 06 Nov 1994 08:49:37 GMT"));
        assert_eq!(bound.2.as_deref(), Some("https://cdn.example.test/bound"));
        let version: i64 = database
            .connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("version");
        assert_eq!(version, LATEST_SCHEMA_VERSION);
    }

    #[test]
    fn v7_upgrade_marks_unprovable_command_tombstones_unknown() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("fk");
        connection
            .execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);")
            .expect("migration table");
        for (version, sql) in [
            (1_i64, MIGRATION_1),
            (2_i64, MIGRATION_2),
            (3_i64, MIGRATION_3),
            (4_i64, MIGRATION_4),
            (5_i64, MIGRATION_5),
            (6_i64, MIGRATION_6),
            (7_i64, MIGRATION_7),
        ] {
            connection.execute_batch(sql).expect("legacy migration");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, 0)",
                    [version],
                )
                .expect("migration row");
        }
        connection
            .execute(
                "INSERT INTO source_tombstones(source_id, generation, deleted_at) VALUES('gone-source', 1, 0)",
                [],
            )
            .expect("legacy tombstone");
        connection
            .execute(
                "INSERT INTO request_receipts(request_id, command, payload_hash, state, created_at, completed_at)
                 VALUES('legacy-delete','delete_source','tombstone:delete_source','complete',0,0)",
                [],
            )
            .expect("legacy receipt");
        let database = Database::from_connection(connection).expect("upgrade");
        assert_eq!(
            database
                .begin_delete_request("legacy-delete", "gone-source")
                .expect("legacy replay"),
            RequestDisposition::Unknown
        );
        let capability: Option<String> = database
            .connection
            .query_row(
                "SELECT replay_capability FROM source_tombstones WHERE source_id='gone-source'",
                [],
                |row| row.get(0),
            )
            .expect("new nullable capability");
        assert!(capability.is_none());
    }

    #[test]
    fn feedback_epoch_and_receipt_are_atomic_at_failure_boundaries() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        database
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_privacy_epoch BEFORE UPDATE OF privacy_epoch ON app_state
             BEGIN SELECT RAISE(ABORT, 'injected epoch failure'); END;",
            )
            .expect("epoch trigger");
        assert!(
            database
                .record_feedback("atomic-epoch", "post-a", &FeedbackSignal::NotRelevant)
                .is_err()
        );
        let counts: (i64, i64, i64) = database
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM feedback),
                    (SELECT privacy_epoch FROM app_state WHERE singleton=1),
                    (SELECT COUNT(*) FROM request_receipts WHERE request_id='atomic-epoch')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("rolled back epoch boundary");
        assert_eq!(counts, (0, 0, 0));
        database
            .connection
            .execute_batch("DROP TRIGGER fail_privacy_epoch;")
            .expect("drop epoch trigger");
        database
            .record_feedback("atomic-epoch", "post-a", &FeedbackSignal::NotRelevant)
            .expect("safe retry");
        let committed: (i64, i64, String) = database
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM feedback),
                    (SELECT privacy_epoch FROM app_state WHERE singleton=1),
                    (SELECT state FROM request_receipts WHERE request_id='atomic-epoch')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("atomic commit");
        assert_eq!(committed, (1, 1, "complete".into()));

        seed_post(&mut database, "source-b", "post-b");
        database
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_feedback_receipt BEFORE UPDATE OF state ON request_receipts
             WHEN NEW.request_id='atomic-receipt' AND NEW.state='complete'
             BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;",
            )
            .expect("receipt trigger");
        assert!(
            database
                .record_feedback("atomic-receipt", "post-b", &FeedbackSignal::MuteSource)
                .is_err()
        );
        let rolled_back: (i64, i64) = database
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM feedback WHERE request_id='atomic-receipt'),
                    (SELECT COUNT(*) FROM request_receipts WHERE request_id='atomic-receipt')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("rolled back receipt boundary");
        assert_eq!(rolled_back, (0, 0));
    }

    #[test]
    fn digest_and_delete_are_idempotent_without_fixture_dependencies() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        database.run_digest("digest-a").expect("digest");
        let first = database
            .dashboard(model(), host())
            .expect("dashboard")
            .edition
            .id;
        database.run_digest("digest-a").expect("replay");
        assert_eq!(
            database
                .dashboard(model(), host())
                .expect("dashboard")
                .edition
                .id,
            first
        );
        database
            .begin_delete_request("delete-a", "source-a")
            .expect("delete receipt");
        database
            .delete_source("delete-a", "source-a")
            .expect("delete");
        let dashboard = database.dashboard(model(), host()).expect("dashboard");
        assert!(dashboard.sources.is_empty());
        assert!(dashboard.items.is_empty());
        let receipt: String = database
            .connection
            .query_row(
                "SELECT payload_hash FROM request_receipts WHERE request_id='delete-a'",
                [],
                |row| row.get(0),
            )
            .expect("receipt tombstone");
        assert!(receipt.starts_with("private-delete:"));
        assert!(!receipt.contains("source-a"));
        assert_eq!(
            database
                .begin_delete_request("delete-a", "source-a")
                .expect("same deletion replay"),
            RequestDisposition::Complete
        );
        assert!(
            database
                .begin_delete_request("delete-a", "source-b")
                .is_err()
        );
    }

    #[test]
    fn settings_and_retention_remove_posts_and_independently_old_comments() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        let old = (Utc::now() - Duration::days(3)).timestamp_millis();
        database.connection.execute(
            "INSERT INTO comments(id, post_id, source_id, remote_id, body_text, published_at, fetched_at, content_hash)
             VALUES('comment-old', 'post-a', 'source-a', 'remote-comment', 'old', ?1, ?1, 'hash')",
            [old],
        ).expect("comment");
        let settings = Settings {
            retention_days: 1,
            ..Settings::default()
        };
        database
            .update_settings("retention-a", &settings)
            .expect("settings");
        let comment_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM comments WHERE id='comment-old'",
                [],
                |row| row.get(0),
            )
            .expect("comments");
        assert_eq!(comment_count, 0);
        assert_eq!(database.load_settings().expect("stored").retention_days, 1);
    }

    #[test]
    fn deletion_privacy_keeps_private_payload_binding_across_reopen() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("private-delete.sqlite3");
        {
            let mut database = Database::open(&path).expect("database");
            seed_post(&mut database, "source-a", "post-a");
            seed_post(&mut database, "source-b", "post-b");
            database
                .begin_request("add-a", "add_rss_source", "guessable-url-hash")
                .expect("add receipt");
            database.complete_request("add-a").expect("complete add");
            database
                .connection
                .execute(
                    "INSERT INTO receipt_sources(request_id, source_id) VALUES('add-a','source-a')",
                    [],
                )
                .expect("link");
            database
                .record_feedback("feedback-a", "post-a", &FeedbackSignal::MoreLikeThis)
                .expect("feedback");
            assert_eq!(
                database
                    .begin_delete_request("delete-a", "source-a")
                    .expect("delete receipt"),
                RequestDisposition::New
            );
            database
                .delete_source("delete-a", "source-a")
                .expect("delete");
        }

        let mut database = Database::open(&path).expect("reopen database");
        assert_eq!(
            database
                .begin_delete_request("delete-a", "source-a")
                .expect("same-payload replay"),
            RequestDisposition::Complete
        );
        assert!(
            database
                .begin_delete_request("delete-a", "source-b")
                .is_err()
        );
        assert_eq!(
            database
                .begin_request("add-a", "add_rss_source", "guessable-url-hash")
                .expect("erased add finality"),
            RequestDisposition::Unknown
        );
        assert!(
            database
                .record_feedback("feedback-a", "post-a", &FeedbackSignal::MoreLikeThis)
                .is_err()
        );
        let receipts = database
            .connection
            .prepare("SELECT request_id, payload_hash FROM request_receipts ORDER BY request_id")
            .expect("statement")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        assert_eq!(
            receipts[0],
            ("add-a".into(), "tombstone-unknown:add_rss_source".into())
        );
        assert_eq!(
            receipts[2],
            ("feedback-a".into(), "tombstone-unknown:feedback".into())
        );
        assert!(receipts[1].0 == "delete-a" && receipts[1].1.starts_with("private-delete:"));
        assert!(
            receipts
                .iter()
                .all(|(_, binding)| !binding.contains("source-a"))
        );
        let audit_disclosure: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM audit_events WHERE subject_id='source-a' OR detail_json LIKE '%source-a%'",
                [],
                |row| row.get(0),
            )
            .expect("audit privacy");
        assert_eq!(audit_disclosure, 0);
        let remaining = database
            .dashboard(model(), host())
            .expect("dashboard")
            .sources;
        assert!(!remaining.iter().any(|source| source.id == "source-a"));
        assert!(remaining.iter().any(|source| source.id == "source-b"));
        let source_bindings: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM receipt_sources", [], |row| row.get(0))
            .expect("bindings");
        assert_eq!(source_bindings, 0);
    }

    #[test]
    fn paused_fetch_delete_resume_cannot_resurrect_and_readd_advances_generation() {
        let mut database = Database::memory().expect("database");
        database
            .begin_request("add-old", "add_rss_source", "url-hash")
            .expect("add receipt");
        let old_post = normalized("old", "Old", "Old body.");
        let first_page = SyncPage {
            posts: vec![old_post.clone()],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"v1\"".into()),
            last_modified: None,
            not_modified: false,
        };
        let (source_id, _) = database
            .add_rss_source(
                "add-old",
                "Feed",
                "https://example.test/feed",
                &first_page,
                vec![fallback_prepared(old_post)],
            )
            .expect("initial add");
        database.complete_request("add-old").expect("complete add");
        let stale_fetch_identity = source_spec(&database, &source_id);
        let old_generation = stale_fetch_identity.generation;
        database
            .record_feedback(
                "feedback-old",
                &format!("post-{}", &content_hash(&format!("{source_id}:old"))[..20]),
                &FeedbackSignal::MoreLikeThis,
            )
            .expect("feedback");
        database
            .begin_delete_request("delete-old", &source_id)
            .expect("delete receipt");
        database
            .delete_source("delete-old", &source_id)
            .expect("delete while fetch paused");

        let resumed = normalized("resumed", "Resumed", "Must not return.");
        let resumed_page = SyncPage {
            posts: vec![resumed.clone()],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"v2\"".into()),
            last_modified: None,
            not_modified: false,
        };
        assert!(
            database
                .ingest_existing_rss(
                    &stale_fetch_identity,
                    "stale-resume",
                    &resumed_page,
                    vec![fallback_prepared(resumed)],
                )
                .is_err()
        );
        let resurrected: i64 = database
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sources) + (SELECT COUNT(*) FROM posts) + (SELECT COUNT(*) FROM receipt_sources)",
                [],
                |row| row.get(0),
            )
            .expect("no resurrection");
        assert_eq!(resurrected, 0);
        let receipt_hashes = database
            .connection
            .prepare("SELECT payload_hash FROM request_receipts ORDER BY request_id")
            .expect("receipts")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("hashes");
        assert!(receipt_hashes.iter().all(|value| {
            value.starts_with("tombstone-unknown:") || value.starts_with("private-delete:")
        }));

        database
            .begin_request("add-new", "add_rss_source", "new-hash")
            .expect("readd receipt");
        let new_post = normalized("new", "New", "New body.");
        let new_page = SyncPage {
            posts: vec![new_post.clone()],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"v3\"".into()),
            last_modified: None,
            not_modified: false,
        };
        let (new_source_id, _) = database
            .add_rss_source(
                "add-new",
                "Feed",
                "https://example.test/feed",
                &new_page,
                vec![fallback_prepared(new_post)],
            )
            .expect("explicit readd");
        assert_eq!(new_source_id, source_id);
        assert!(source_spec(&database, &source_id).generation > old_generation);
    }

    #[test]
    fn unchanged_old_entries_preserve_summary_and_do_not_starve_new_candidates() {
        let mut database = Database::memory().expect("database");
        database
            .begin_request("add-budget", "add_rss_source", "hash")
            .expect("receipt");
        let old = normalized("old", "Old", "Stable body.");
        let initial = SyncPage {
            posts: vec![old.clone()],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"one\"".into()),
            last_modified: None,
            not_modified: false,
        };
        let (source_id, _) = database
            .add_rss_source(
                "add-budget",
                "Budget feed",
                "https://example.test/budget",
                &initial,
                vec![fallback_prepared(old.clone())],
            )
            .expect("add");
        let source = source_spec(&database, &source_id);
        let summary_id: String = database
            .connection
            .query_row("SELECT id FROM summaries", [], |row| row.get(0))
            .expect("summary");
        let mut posts = vec![old];
        for index in 0..5 {
            posts.push(normalized(
                &format!("new-{index}"),
                &format!("New {index}"),
                "New content.",
            ));
        }
        let changed = database.changed_posts(&source, &posts).expect("classify");
        assert_eq!(changed.len(), 5);
        assert_eq!(changed[0].remote_id, "new-0");
        assert_eq!(
            changed
                .iter()
                .take(4)
                .map(|post| post.remote_id.as_str())
                .collect::<Vec<_>>(),
            vec!["new-0", "new-1", "new-2", "new-3"]
        );
        let page = SyncPage {
            posts: posts.clone(),
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"two\"".into()),
            last_modified: None,
            not_modified: false,
        };
        database
            .ingest_existing_rss(
                &source,
                "sync-budget",
                &page,
                changed.into_iter().map(fallback_prepared).collect(),
            )
            .expect("ingest");
        let retained: String = database
            .connection
            .query_row(
                "SELECT id FROM summaries WHERE post_id=(SELECT id FROM posts WHERE remote_id='old')",
                [],
                |row| row.get(0),
            )
            .expect("retained summary");
        assert_eq!(retained, summary_id);
    }

    #[test]
    fn conditional_checkpoint_backoff_and_owner_fenced_recovery() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        let source = source_spec(&database, "source-a");
        let page = SyncPage {
            posts: vec![],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"rotated\"".into()),
            last_modified: Some("Sun, 06 Nov 1994 08:49:37 GMT".into()),
            not_modified: true,
        };
        database
            .complete_not_modified(&source, "sync-304", &page)
            .expect("304");
        let (etag, validator_url): (Option<String>, Option<String>) = database
            .connection
            .query_row(
                "SELECT etag, validator_url FROM sources WHERE id='source-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("source");
        assert_eq!(etag.as_deref(), Some("\"rotated\""));
        assert_eq!(
            validator_url.as_deref(),
            Some("https://cdn.example.test/feed")
        );
        let (resync, _) = database
            .rss_sources(SourceSelectionMode::ManualOverride, i64::MAX, 20)
            .expect("resync identity");
        assert_eq!(resync[0].sync_url(), "https://cdn.example.test/feed");
        assert_eq!(resync[0].etag.as_deref(), Some("\"rotated\""));
        database
            .record_sync_failure(&source, "sync-fail", "Bounded sync failed")
            .expect("failure");
        let now = Utc::now().timestamp_millis();
        let (due, _) = database
            .rss_sources(SourceSelectionMode::ResidentDue, now, 20)
            .expect("due selection");
        assert!(due.is_empty(), "resident honors persisted backoff");
        let (manual, _) = database
            .rss_sources(SourceSelectionMode::ManualOverride, now, 20)
            .expect("manual override");
        assert_eq!(manual.len(), 1);

        let lease_ms = 600_000;
        let lease_a = database
            .acquire_runner_lease("runner-a", 100, 100, lease_ms)
            .expect("lease")
            .expect("owner A");
        assert!(
            database
                .acquire_runner_lease("runner-b", 100, 101, lease_ms)
                .expect("contention")
                .is_none()
        );
        let renewed = database
            .heartbeat_runner_lease(&lease_a, 500_000, lease_ms)
            .expect("heartbeat");
        assert_eq!(renewed.expires_at, 1_100_000);
        let lease_b = database
            .acquire_runner_lease("runner-b", 100, 1_100_001, lease_ms)
            .expect("recover at former 11 minute boundary")
            .expect("owner B");
        assert!(
            database
                .finish_runner_lease(
                    &lease_a,
                    RunnerOutcome::Complete,
                    "stale",
                    Some(200),
                    1_100_002
                )
                .is_err()
        );
        database
            .finish_runner_lease(
                &lease_b,
                RunnerOutcome::Partial,
                "partial",
                Some(200),
                1_100_003,
            )
            .expect("owner B finish");
        assert!(
            database
                .acquire_runner_lease("runner-c", 100, 1_600_000, lease_ms)
                .expect("former 16 minute boundary")
                .is_none()
        );
        let receipt_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM request_receipts WHERE request_id LIKE 'scheduled-%' OR request_id LIKE 'resident-%'",
                [],
                |row| row.get(0),
            )
            .expect("no resident receipts");
        assert_eq!(receipt_count, 0);
    }

    #[test]
    fn manual_timeout_after_first_commit_seals_replay_as_unknown() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        let payload = content_hash("sync-all-rss-manual-override-v2");
        assert_eq!(
            database
                .begin_request("manual-timeout", "sync_sources", &payload)
                .expect("begin"),
            RequestDisposition::New
        );
        let source = source_spec(&database, "source-a");
        let page = SyncPage {
            posts: vec![],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"committed\"".into()),
            last_modified: None,
            not_modified: true,
        };
        database
            .complete_not_modified(&source, "manual-timeout:0", &page)
            .expect("first source commit");
        database
            .seal_request_unknown("manual-timeout", "sync_sources")
            .expect("timeout seal");
        assert_eq!(
            database
                .begin_request("manual-timeout", "sync_sources", &payload)
                .expect("same-id replay"),
            RequestDisposition::Unknown
        );
        let jobs: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE dedupe_key='rss-sync:manual-timeout:0'",
                [],
                |row| row.get(0),
            )
            .expect("single committed effect");
        assert_eq!(jobs, 1);
    }

    #[test]
    fn unknown_recovery_is_limited_and_failed_instants_are_handled() {
        let mut database = Database::memory().expect("database");
        let lease_a = database
            .acquire_runner_lease("owner-a", 500, 100, 10)
            .expect("acquire A")
            .expect("owner A");
        database
            .finish_runner_lease(&lease_a, RunnerOutcome::Unknown, "unknown A", None, 101)
            .expect("unknown A");
        assert_eq!(database.last_runner_handled().expect("handled"), None);
        let lease_b = database
            .acquire_runner_lease("owner-b", 500, 102, 10)
            .expect("recover")
            .expect("owner B");
        database
            .finish_runner_lease(&lease_b, RunnerOutcome::Unknown, "unknown B", None, 103)
            .expect("exhaust recovery");
        assert_eq!(database.last_runner_handled().expect("handled"), Some(500));
        assert!(
            database
                .acquire_runner_lease("owner-c", 500, 200, 10)
                .expect("terminal lookup")
                .is_none()
        );

        let failed = database
            .acquire_runner_lease("owner-f", 600, 201, 10)
            .expect("failed instant")
            .expect("owner F");
        database
            .finish_runner_lease(&failed, RunnerOutcome::Failed, "failed", None, 202)
            .expect("finish failed");
        assert_eq!(database.last_runner_handled().expect("handled"), Some(600));
    }

    #[test]
    fn expired_matching_owner_cannot_finish_and_can_be_recovered() {
        let mut database = Database::memory().expect("database");
        let lease_a = database
            .acquire_runner_lease("expired-owner-a", 700, 100, 10)
            .expect("acquire")
            .expect("owner A");
        let before: String = database
            .connection
            .query_row(
                "SELECT json_object(
                    'runner_owner', runner_state.lease_owner,
                    'runner_token', runner_state.lease_token,
                    'runner_expiry', runner_state.lease_expires_at,
                    'runner_outcome', runner_state.last_outcome,
                    'job_state', jobs.state,
                    'job_attempts', jobs.attempts,
                    'job_error', jobs.last_error_code)
                 FROM runner_state JOIN jobs ON jobs.dedupe_key='scheduled-edition:700'
                 WHERE runner_state.singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("before");
        assert!(
            database
                .finish_runner_lease(
                    &lease_a,
                    RunnerOutcome::Complete,
                    "too late",
                    Some(800),
                    111,
                )
                .is_err()
        );
        let after: String = database
            .connection
            .query_row(
                "SELECT json_object(
                    'runner_owner', runner_state.lease_owner,
                    'runner_token', runner_state.lease_token,
                    'runner_expiry', runner_state.lease_expires_at,
                    'runner_outcome', runner_state.last_outcome,
                    'job_state', jobs.state,
                    'job_attempts', jobs.attempts,
                    'job_error', jobs.last_error_code)
                 FROM runner_state JOIN jobs ON jobs.dedupe_key='scheduled-edition:700'
                 WHERE runner_state.singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("after");
        assert_eq!(after, before, "expired finish has zero side effects");
        assert!(
            database
                .acquire_runner_lease("expired-owner-b", 700, 111, 10)
                .expect("recover")
                .is_some()
        );
    }

    #[test]
    fn two_expired_owners_terminalize_durably_and_next_instant_runs() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("double-expiry.sqlite3");
        {
            let mut database = Database::open(&path).expect("database");
            database
                .acquire_runner_lease("expiry-owner-a", 800, 100, 10)
                .expect("A")
                .expect("owner A");
            database
                .acquire_runner_lease("expiry-owner-b", 800, 111, 10)
                .expect("B")
                .expect("owner B");
        }
        {
            let mut reopened = Database::open(&path).expect("reopen before exhaustion");
            assert!(
                reopened
                    .acquire_runner_lease("expiry-owner-c", 800, 122, 10)
                    .expect("terminalize")
                    .is_none()
            );
        }
        let mut reopened = Database::open(&path).expect("reopen terminal state");
        let job: (String, i64, Option<String>, Option<i64>, Option<String>) = reopened
            .connection
            .query_row(
                "SELECT state, attempts, lease_owner, lease_expires_at, last_error_code
                 FROM jobs WHERE dedupe_key='scheduled-edition:800'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("job");
        assert_eq!(
            job,
            (
                "failed".into(),
                2,
                None,
                None,
                Some("UNKNOWN_RECOVERY_EXHAUSTED".into())
            )
        );
        let runner: (Option<String>, Option<i64>, String) = reopened
            .connection
            .query_row(
                "SELECT lease_owner, lease_expires_at, last_outcome FROM runner_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("runner");
        assert_eq!(runner, (None, None, "failed".into()));
        assert_eq!(reopened.last_runner_handled().expect("handled"), Some(800));
        assert!(
            reopened
                .acquire_runner_lease("expiry-owner-d", 800, 123, 10)
                .expect("no third owner")
                .is_none()
        );
        assert!(
            reopened
                .acquire_runner_lease("next-owner", 900, 124, 10)
                .expect("next instant")
                .is_some()
        );
    }

    #[test]
    fn stale_resident_cannot_commit_source_or_digest_side_effects() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        let source = source_spec(&database, "source-a");
        let now = Utc::now().timestamp_millis();
        let lease_a = database
            .acquire_runner_lease("live-owner-a", 700, now, 10)
            .expect("A")
            .expect("owner A");
        let lease_b = database
            .acquire_runner_lease("live-owner-b", 700, now + 11, 600_000)
            .expect("B recovery")
            .expect("owner B");
        database.connection.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at, generation)
             VALUES('mastodon-stale', 'mastodon', 'Stale social', '', 'healthy', '{}', ?1, ?1, 1)",
            [now],
        ).expect("social source");
        let social_post = normalized("social", "Social", "Evidence");
        let social_batch = SyncBatch {
            posts: vec![social_post.clone()],
            comments: vec![crate::connectors::NormalizedComment {
                post_remote_id: social_post.remote_id.clone(),
                remote_id: "stale-comment".into(),
                parent_remote_id: None,
                author: "Reader".into(),
                body_text: "Must not commit".into(),
                published_at: now,
                depth: 1,
                position: 0,
            }],
            comment_scope_post_ids: vec![social_post.remote_id.clone()],
            cursor: Some("stale-cursor".into()),
            page_finality: crate::connectors::PageFinality::Complete,
            comment_completeness: CommentCompleteness::Complete,
            comments_truncated: false,
            health: crate::connectors::ConnectorHealth {
                state: crate::connectors::ConnectorHealthState::Healthy,
                safe_detail: "stale".into(),
                retry_at: None,
            },
            rss: None,
        };
        let social_source = SourceSyncSpec {
            id: "mastodon-stale".into(),
            kind: SourceKind::Mastodon,
            generation: 1,
            config_json: "{}".into(),
            cursor: None,
        };
        assert!(
            database
                .ingest_sync_batch_fenced(
                    &social_source,
                    "stale-social",
                    &social_batch,
                    vec![fallback_prepared(social_post)],
                    Some(&lease_a),
                )
                .is_err()
        );
        let stale_effects: (Option<String>, i64) = database.connection.query_row(
            "SELECT sync_cursor, (SELECT COUNT(*) FROM comments WHERE source_id='mastodon-stale') FROM sources WHERE id='mastodon-stale'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("stale effects");
        assert_eq!(stale_effects, (None, 0));
        let page = SyncPage {
            posts: vec![],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"new\"".into()),
            last_modified: None,
            not_modified: true,
        };
        assert!(
            database
                .complete_not_modified_fenced(&source, "stale-a", &page, Some(&lease_a))
                .is_err()
        );
        database
            .complete_not_modified_fenced(&source, "current-b", &page, Some(&lease_b))
            .expect("current owner commit");
        assert!(
            database
                .run_digest_fenced("stale-digest", Some(&lease_a))
                .is_err()
        );
        assert!(
            database
                .finish_runner_lease(
                    &lease_a,
                    RunnerOutcome::Complete,
                    "stale finish",
                    None,
                    now + 12,
                )
                .is_err()
        );
    }

    #[test]
    fn v8_upgrade_adds_neutral_health_and_comment_completeness_without_social_activation() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("fk");
        connection.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);"
        ).expect("migration table");
        for (version, sql) in [
            (1_i64, MIGRATION_1),
            (2, MIGRATION_2),
            (3, MIGRATION_3),
            (4, MIGRATION_4),
            (5, MIGRATION_5),
            (6, MIGRATION_6),
            (7, MIGRATION_7),
            (8, MIGRATION_8),
        ] {
            connection.execute_batch(sql).expect("legacy migration");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, 0)",
                    [version],
                )
                .expect("version");
        }
        connection.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at, generation)
             VALUES('legacy-rss-nine', 'rss', 'Legacy', 'RSS', 'healthy', '{}', 1, 1, 1)", []
        ).expect("source");
        connection.execute(
            "INSERT INTO posts(id, source_id, remote_id, canonical_url, title, body_text, published_at, fetched_at, content_hash, published_time_kind)
             VALUES('legacy-post-nine', 'legacy-rss-nine', 'remote', '', 'Title', 'Body', 9999999999999, 9999999999999, 'hash', 'fetched')", []
        ).expect("post");
        connection.execute(
            "INSERT INTO comments(id, post_id, source_id, remote_id, body_text, published_at, fetched_at, content_hash)
             VALUES('legacy-comment-nine', 'legacy-post-nine', 'legacy-rss-nine', 'comment', 'Body', 9999999999999, 9999999999999, 'hash')", []
        ).expect("comment");
        let database = Database::from_connection(connection).expect("upgrade");
        let metadata: (String, String, i64, i64, String, String) = database
            .connection
            .query_row(
                "SELECT m.health_state, m.comments_status, c.depth, c.position,
                        pcs.evidence_hash, pcs.summary_input_hash
             FROM source_sync_metadata m JOIN comments c ON c.source_id=m.source_id
             JOIN post_comment_state pcs ON pcs.post_id=c.post_id
             WHERE m.source_id='legacy-rss-nine'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("metadata");
        assert_eq!(
            metadata,
            (
                "healthy".into(),
                "unavailable".into(),
                0,
                0,
                "unavailable".into(),
                "hash".into()
            )
        );
        assert!(
            crate::connectors::connector_descriptors()[1..].iter().all(
                |item| item.availability != crate::connectors::ConnectorAvailability::Available
            )
        );
    }

    #[test]
    fn v9_upgrade_invalidates_unbound_social_comment_summaries_and_preserves_rss() {
        let connection = Connection::open_in_memory().expect("connection");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("fk");
        for (version, sql) in [
            (1_i64, MIGRATION_1),
            (2, MIGRATION_2),
            (3, MIGRATION_3),
            (4, MIGRATION_4),
            (5, MIGRATION_5),
            (6, MIGRATION_6),
            (7, MIGRATION_7),
            (8, MIGRATION_8),
            (9, MIGRATION_9),
        ] {
            connection.execute_batch(sql).expect("legacy migration");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, 0)",
                    [version],
                )
                .expect("version");
        }
        for (source_id, kind) in [("social-v9", "mastodon"), ("rss-v9", "rss")] {
            connection.execute(
                "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at, generation)
                 VALUES(?1, ?2, ?1, '', 'healthy', '{}', 1, 1, 1)",
                params![source_id, kind],
            ).expect("source");
            connection.execute(
                "INSERT INTO posts(id, source_id, remote_id, canonical_url, title, body_text, published_at, fetched_at, content_hash, published_time_kind)
                 VALUES(?1, ?2, 'remote', '', 'Title', 'Body', 9999999999999, 9999999999999, ?3, 'fetched')",
                params![format!("post-{source_id}"), source_id, format!("hash-{source_id}")],
            ).expect("post");
            connection.execute(
                "INSERT INTO source_sync_metadata(source_id, health_state, safe_detail, comments_status, comments_truncated, updated_at)
                 VALUES(?1, 'healthy', '', ?2, 0, 1)",
                params![source_id, if kind == "rss" { "unavailable" } else { "complete" }],
            ).expect("metadata");
            connection.execute(
                "INSERT INTO post_comment_state(post_id, status, truncated, fetched_at) VALUES(?1, ?2, 0, 1)",
                params![format!("post-{source_id}"), if kind == "rss" { "unavailable" } else { "complete" }],
            ).expect("comment state");
            connection.execute(
                "INSERT INTO summaries(id, post_id, summary_text, comment_overview, provenance_json, provider, prompt_version, input_hash, created_at, summary_method, uncertainty)
                 VALUES(?1, ?2, 'Legacy', 'Legacy comments', '{}', 'legacy', 'legacy', ?3, 1, 'extractive', 'legacy')",
                params![format!("summary-{source_id}"), format!("post-{source_id}"), format!("hash-{source_id}")],
            ).expect("summary");
        }
        connection.execute(
            "INSERT INTO comments(id, post_id, source_id, remote_id, body_text, published_at, fetched_at, content_hash, depth, position)
             VALUES('comment-social-v9', 'post-social-v9', 'social-v9', 'comment', 'Evidence', 9999999999999, 9999999999999, 'comment-hash', 1, 0)", []
        ).expect("comment");
        for (index, state) in ["queued", "running", "complete", "failed", "scheduled"]
            .iter()
            .enumerate()
        {
            connection.execute(
                "INSERT INTO jobs(id, kind, dedupe_key, state, run_after, message, created_at, updated_at)
                 VALUES(?1, 'sync', ?2, ?3, 1, 'legacy', 1, 1)",
                params![format!("job-{index}"), format!("dedupe-{index}"), state],
            ).expect("job");
        }

        let database = Database::from_connection(connection).expect("upgrade");
        let version: i64 = database
            .connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("version");
        assert_eq!(version, 12);
        let social_state: (String, i64, String, String) = database.connection.query_row(
            "SELECT status, truncated, evidence_hash, summary_input_hash FROM post_comment_state WHERE post_id='post-social-v9'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).expect("social state");
        assert_eq!(social_state.0, "partial");
        assert_eq!(social_state.1, 1);
        assert_eq!(social_state.2, "migration-unverified");
        assert!(social_state.3.starts_with("migration-unverified:"));
        let social_summaries: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM summaries WHERE post_id='post-social-v9'",
                [],
                |row| row.get(0),
            )
            .expect("social summaries");
        assert_eq!(social_summaries, 0);
        let rss: (String, String, i64) = database
            .connection
            .query_row(
                "SELECT pcs.status, pcs.summary_input_hash, COUNT(s.id)
             FROM post_comment_state pcs LEFT JOIN summaries s ON s.post_id=pcs.post_id
             WHERE pcs.post_id='post-rss-v9' GROUP BY pcs.post_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("rss state");
        assert_eq!(rss, ("unavailable".into(), "hash-rss-v9".into(), 1));
        let jobs: i64 = database.connection.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('queued','running','complete','failed','scheduled')",
            [], |row| row.get(0)
        ).expect("jobs");
        assert_eq!(jobs, 5);
    }

    #[test]
    fn comment_finality_migration_failure_rolls_back_jobs_columns_repair_and_version() {
        let mut connection = Connection::open_in_memory().expect("connection");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("fk");
        for (version, sql) in [
            (1_i64, MIGRATION_1),
            (2, MIGRATION_2),
            (3, MIGRATION_3),
            (4, MIGRATION_4),
            (5, MIGRATION_5),
            (6, MIGRATION_6),
            (7, MIGRATION_7),
            (8, MIGRATION_8),
            (9, MIGRATION_9),
        ] {
            connection.execute_batch(sql).expect("legacy migration");
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, 0)",
                    [version],
                )
                .expect("version");
        }
        {
            let transaction = connection.transaction().expect("transaction");
            transaction
                .execute_batch(MIGRATION_10)
                .expect("migration 10");
            transaction
                .execute_batch(MIGRATION_11)
                .expect("migration 11");
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(10, 0),(11, 0)",
                    [],
                )
                .expect("versions");
            assert!(
                transaction
                    .execute_batch("ALTER TABLE source_sync_metadata ADD COLUMN page_finality TEXT")
                    .is_err()
            );
            // Drop without commit: every schema/data/version change must roll back together.
        }
        let max_version: i64 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("version");
        assert_eq!(max_version, 9);
        let has_page_finality = connection
            .prepare("PRAGMA table_info(source_sync_metadata)")
            .expect("table info")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("columns")
            .collect::<Result<Vec<_>, _>>()
            .expect("columns")
            .iter()
            .any(|column| column == "page_finality");
        assert!(!has_page_finality);
        let jobs_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='jobs'",
                [],
                |row| row.get(0),
            )
            .expect("jobs sql");
        assert!(!jobs_sql.contains("'partial'"));
    }

    #[test]
    fn neutral_comment_batch_is_atomic_ordered_and_generation_fenced() {
        let mut database = Database::memory().expect("database");
        let now = Utc::now().timestamp_millis();
        database.connection.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at, generation)
             VALUES('mastodon-a', 'mastodon', 'Example account', 'disabled fixture seam', 'healthy', '{}', ?1, ?1, 1)",
            [now],
        ).expect("source");
        let post = normalized("social-post", "A post", "Bounded post evidence.");
        let comment = |id: &str, position: u32| crate::connectors::NormalizedComment {
            post_remote_id: post.remote_id.clone(),
            remote_id: id.into(),
            parent_remote_id: None,
            author: "Commenter".into(),
            body_text: format!("Comment {id}"),
            published_at: now,
            depth: 1,
            position,
        };
        let batch = SyncBatch {
            posts: vec![post.clone()],
            comments: vec![comment("later", 2), comment("first", 1)],
            comment_scope_post_ids: vec![post.remote_id.clone()],
            cursor: Some("opaque-cursor".into()),
            page_finality: crate::connectors::PageFinality::Complete,
            comment_completeness: CommentCompleteness::Complete,
            comments_truncated: false,
            health: crate::connectors::ConnectorHealth {
                state: crate::connectors::ConnectorHealthState::Healthy,
                safe_detail: "Official API fixture synchronized.".into(),
                retry_at: Some(now + 60_000),
            },
            rss: None,
        };
        let prepared = vec![PreparedPost {
            input_hash: summary_input_hash_for(
                &post,
                &batch.comments,
                CommentCompleteness::Complete,
                false,
            ),
            summary: GroundedSummary {
                summary: post.body_text.clone(),
                comment_overview: "2 comments in complete snapshot.".into(),
                uncertainty: "Fallback.".into(),
            },
            post: post.clone(),
            provider: "deterministic-fallback".into(),
            model_id: None,
            prompt_version: "extractive-v1".into(),
            summary_method: "extractive".into(),
        }];
        let source = SourceSyncSpec {
            id: "mastodon-a".into(),
            kind: SourceKind::Mastodon,
            generation: 1,
            config_json: "{}".into(),
            cursor: None,
        };
        database
            .ingest_sync_batch_fenced(&source, "social-sync", &batch, prepared, None)
            .expect("ingest");
        let ordered: Vec<String> = database.connection.prepare(
            "SELECT remote_id FROM comments WHERE source_id='mastodon-a' ORDER BY position, published_at, remote_id"
        ).expect("statement").query_map([], |row| row.get(0)).expect("query")
          .collect::<Result<_, _>>().expect("comments");
        assert_eq!(ordered, vec!["first", "later"]);
        let state: (String, i64) = database
            .connection
            .query_row(
                "SELECT status, truncated FROM post_comment_state",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("state");
        assert_eq!(state, ("complete".into(), 0));

        let mut oversized = batch.clone();
        oversized.comments[0].body_text = "x".repeat(crate::connectors::MAX_COMMENT_BODY_BYTES + 1);
        assert!(
            database
                .ingest_sync_batch_fenced(&source, "oversized-sync", &oversized, Vec::new(), None)
                .is_err()
        );
        let count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM comments WHERE source_id='mastodon-a'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(count, 2, "oversize batch has no partial effects");
        let invalid_source = SourceSyncSpec {
            config_json: serde_json::json!({"unsafe": "line\u{0000}break"}).to_string(),
            ..source.clone()
        };
        assert!(
            database
                .ingest_sync_batch_fenced(
                    &invalid_source,
                    "invalid-spec-sync",
                    &batch,
                    Vec::new(),
                    None,
                )
                .is_err()
        );
        let cursor: Option<String> = database
            .connection
            .query_row(
                "SELECT sync_cursor FROM sources WHERE id='mastodon-a'",
                [],
                |row| row.get(0),
            )
            .expect("cursor");
        assert_eq!(cursor.as_deref(), Some("opaque-cursor"));

        database
            .connection
            .execute("UPDATE sources SET generation=2 WHERE id='mastodon-a'", [])
            .expect("replace generation");
        let mut stale = batch;
        stale.comments.push(comment("stale", 3));
        assert!(
            database
                .ingest_sync_batch_fenced(&source, "stale-sync", &stale, Vec::new(), None)
                .is_err()
        );
        let stale_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM comments WHERE remote_id='stale'",
                [],
                |row| row.get(0),
            )
            .expect("stale count");
        assert_eq!(stale_count, 0);
    }

    #[test]
    fn valid_finality_matrix_persists_matching_source_job_and_cursor_truth() {
        let cases = [
            (
                PageFinality::Complete,
                CommentCompleteness::Unavailable,
                false,
            ),
            (
                PageFinality::Partial,
                CommentCompleteness::Unavailable,
                false,
            ),
            (PageFinality::Complete, CommentCompleteness::Complete, false),
            (PageFinality::Partial, CommentCompleteness::Complete, false),
            (PageFinality::Partial, CommentCompleteness::Partial, false),
            (PageFinality::Partial, CommentCompleteness::Partial, true),
        ];
        for (index, (page_finality, completeness, truncated)) in cases.into_iter().enumerate() {
            let mut database = Database::memory().expect("database");
            let now = Utc::now().timestamp_millis();
            let source_id = format!("matrix-{index}");
            let post = normalized("matrix-post", "Matrix", "Matrix body");
            database.connection.execute(
                "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, sync_cursor, created_at, updated_at, generation)
                 VALUES(?1, 'mastodon', ?1, '', 'healthy', '{}', 'cursor-before', ?2, ?2, 1)",
                params![source_id, now],
            ).expect("source");
            database.connection.execute(
                "INSERT INTO posts(id, source_id, remote_id, canonical_url, title, body_text, published_at, fetched_at, content_hash, published_time_kind)
                 VALUES(?1, ?2, ?3, '', ?4, ?5, ?6, ?6, ?7, 'fetched')",
                params![format!("post-{index}"), source_id, post.remote_id, post.title, post.body_text, post.published_at, post_content_hash(&post)],
            ).expect("post");
            let scoped = completeness != CommentCompleteness::Unavailable;
            let batch = SyncBatch {
                posts: if scoped {
                    Vec::new()
                } else {
                    vec![post.clone()]
                },
                comments: Vec::new(),
                comment_scope_post_ids: if scoped {
                    vec![post.remote_id.clone()]
                } else {
                    Vec::new()
                },
                cursor: Some(format!("cursor-after-{index}")),
                page_finality,
                comment_completeness: completeness,
                comments_truncated: truncated,
                health: crate::connectors::ConnectorHealth {
                    state: crate::connectors::ConnectorHealthState::Healthy,
                    safe_detail: "Matrix".into(),
                    retry_at: None,
                },
                rss: None,
            };
            let source = SourceSyncSpec {
                id: source_id.clone(),
                kind: SourceKind::Mastodon,
                generation: 1,
                config_json: "{}".into(),
                cursor: Some("cursor-before".into()),
            };
            let candidates = database
                .changed_posts_for_sync_batch_fenced(&source, &batch, None)
                .expect("classify");
            let prepared = candidates
                .into_iter()
                .map(|candidate| PreparedPost {
                    post: candidate.post,
                    input_hash: candidate.input_hash,
                    summary: GroundedSummary {
                        summary: "Matrix".into(),
                        comment_overview: "Matrix".into(),
                        uncertainty: "Matrix".into(),
                    },
                    provider: "deterministic-fallback".into(),
                    model_id: None,
                    prompt_version: "extractive-v1".into(),
                    summary_method: "extractive".into(),
                })
                .collect();
            database
                .ingest_sync_batch_fenced(
                    &source,
                    &format!("matrix-request-{index}"),
                    &batch,
                    prepared,
                    None,
                )
                .expect("ingest");
            let truth: (String, String, String, String) = database
                .connection
                .query_row(
                    "SELECT s.sync_cursor, m.page_finality, m.comments_status, j.state
                 FROM sources s JOIN source_sync_metadata m ON m.source_id=s.id
                 JOIN jobs j ON j.dedupe_key=?2 WHERE s.id=?1",
                    params![source_id, format!("mastodon-sync:matrix-request-{index}")],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("truth");
            let finality = if page_finality == PageFinality::Complete {
                "complete"
            } else {
                "partial"
            };
            assert_eq!(
                truth,
                (
                    format!("cursor-after-{index}"),
                    finality.into(),
                    completeness.as_str().into(),
                    finality.into()
                )
            );
        }
    }

    #[test]
    fn comment_remote_identity_cannot_move_between_posts_in_partial_or_complete_batches() {
        let mut database = Database::memory().expect("database");
        let now = Utc::now().timestamp_millis();
        database.connection.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at, generation)
             VALUES('identity-source', 'mastodon', 'Identity', '', 'healthy', '{}', ?1, ?1, 1)",
            [now],
        ).expect("source");
        let post_a = normalized("post-a", "A", "A body");
        let post_b = normalized("post-b", "B", "B body");
        let original = NormalizedComment {
            post_remote_id: post_a.remote_id.clone(),
            remote_id: "stable-comment".into(),
            parent_remote_id: None,
            author: "Reader".into(),
            body_text: "Original evidence".into(),
            published_at: now,
            depth: 1,
            position: 0,
        };
        let initial = SyncBatch {
            posts: vec![post_a.clone(), post_b.clone()],
            comments: vec![original.clone()],
            comment_scope_post_ids: vec![post_a.remote_id.clone(), post_b.remote_id.clone()],
            cursor: None,
            page_finality: PageFinality::Complete,
            comment_completeness: CommentCompleteness::Complete,
            comments_truncated: false,
            health: crate::connectors::ConnectorHealth {
                state: crate::connectors::ConnectorHealthState::Healthy,
                safe_detail: "Complete".into(),
                retry_at: None,
            },
            rss: None,
        };
        let prepared = [
            (post_a.clone(), vec![original.clone()]),
            (post_b.clone(), Vec::new()),
        ]
        .into_iter()
        .map(|(post, comments)| PreparedPost {
            input_hash: summary_input_hash_for(
                &post,
                &comments,
                CommentCompleteness::Complete,
                false,
            ),
            summary: GroundedSummary {
                summary: post.body_text.clone(),
                comment_overview: format!("{} comments", comments.len()),
                uncertainty: "Bounded".into(),
            },
            post,
            provider: "deterministic-fallback".into(),
            model_id: None,
            prompt_version: "extractive-v1".into(),
            summary_method: "extractive".into(),
        })
        .collect();
        let source = SourceSyncSpec {
            id: "identity-source".into(),
            kind: SourceKind::Mastodon,
            generation: 1,
            config_json: "{}".into(),
            cursor: None,
        };
        database
            .ingest_sync_batch_fenced(&source, "identity-initial", &initial, prepared, None)
            .expect("initial");
        let before: (String, String, String) = database
            .connection
            .query_row(
                "SELECT p.remote_id, pcs.summary_input_hash, s.comment_overview
             FROM comments c JOIN posts p ON p.id=c.post_id
             JOIN post_comment_state pcs ON pcs.post_id=p.id
             JOIN summaries s ON s.post_id=p.id WHERE c.remote_id='stable-comment'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("before");
        for (index, (page_finality, completeness)) in [
            (PageFinality::Partial, CommentCompleteness::Partial),
            (PageFinality::Complete, CommentCompleteness::Complete),
        ]
        .into_iter()
        .enumerate()
        {
            let moved = SyncBatch {
                posts: Vec::new(),
                comments: vec![NormalizedComment {
                    post_remote_id: post_b.remote_id.clone(),
                    body_text: "Moved evidence".into(),
                    ..original.clone()
                }],
                comment_scope_post_ids: vec![post_b.remote_id.clone()],
                cursor: None,
                page_finality,
                comment_completeness: completeness,
                comments_truncated: false,
                health: initial.health.clone(),
                rss: None,
            };
            assert!(
                database
                    .changed_posts_for_sync_batch_fenced(&source, &moved, None)
                    .is_err()
            );
            assert!(
                database
                    .ingest_sync_batch_fenced(
                        &source,
                        &format!("identity-move-{index}"),
                        &moved,
                        Vec::new(),
                        None
                    )
                    .is_err()
            );
        }
        let after: (String, String, String) = database
            .connection
            .query_row(
                "SELECT p.remote_id, pcs.summary_input_hash, s.comment_overview
             FROM comments c JOIN posts p ON p.id=c.post_id
             JOIN post_comment_state pcs ON pcs.post_id=p.id
             JOIN summaries s ON s.post_id=p.id WHERE c.remote_id='stable-comment'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("after");
        assert_eq!(after, before);
    }

    #[test]
    fn comment_snapshot_finality_reconciles_provenance_and_retention_privacy() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("comment-finality.sqlite3");
        let mut database = Database::open(&path).expect("database");
        let now = Utc::now().timestamp_millis();
        database.connection.execute(
            "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at, generation)
             VALUES('mastodon-finality', 'mastodon', 'Example', 'disabled seam', 'healthy', '{}', ?1, ?1, 1)",
            [now],
        ).expect("source");
        let post = normalized("post-finality", "Post", "Post evidence.");
        let make_comment = |id: &str, body: &str, position: u32| NormalizedComment {
            post_remote_id: post.remote_id.clone(),
            remote_id: id.into(),
            parent_remote_id: None,
            author: "Reader".into(),
            body_text: body.into(),
            published_at: now + i64::from(position),
            depth: 1,
            position,
        };
        let mut source = SourceSyncSpec {
            id: "mastodon-finality".into(),
            kind: SourceKind::Mastodon,
            generation: 1,
            config_json: "{}".into(),
            cursor: None,
        };
        let prepared = |changed: Vec<InferenceCandidate>, overview: &str| {
            changed
                .into_iter()
                .map(|candidate| PreparedPost {
                    input_hash: candidate.input_hash,
                    summary: GroundedSummary {
                        summary: candidate.post.body_text.clone(),
                        comment_overview: overview.into(),
                        uncertainty: "Verify bounded evidence.".into(),
                    },
                    post: candidate.post,
                    provider: "deterministic-fallback".into(),
                    model_id: None,
                    prompt_version: "extractive-v1".into(),
                    summary_method: "extractive".into(),
                })
                .collect::<Vec<_>>()
        };

        let initial = SyncBatch {
            posts: vec![post.clone()],
            comments: vec![
                make_comment("one", "First", 1),
                make_comment("two", "Second", 2),
            ],
            comment_scope_post_ids: vec![post.remote_id.clone()],
            cursor: Some("cursor-1".into()),
            page_finality: PageFinality::Complete,
            comment_completeness: CommentCompleteness::Complete,
            comments_truncated: false,
            health: crate::connectors::ConnectorHealth {
                state: crate::connectors::ConnectorHealthState::Healthy,
                safe_detail: "Complete context.".into(),
                retry_at: None,
            },
            rss: None,
        };
        let changed = database
            .changed_posts_for_sync_batch_fenced(&source, &initial, None)
            .expect("classify initial");
        assert_eq!(
            changed.len(),
            1,
            "one changed post consumes one attempt slot"
        );
        database
            .ingest_sync_batch_fenced(
                &source,
                "complete-one",
                &initial,
                prepared(changed, "2 comments in complete snapshot."),
                None,
            )
            .expect("initial ingest");
        source.cursor = Some("cursor-1".into());
        assert!(
            database
                .changed_posts_for_sync_batch_fenced(&source, &initial, None)
                .expect("unchanged")
                .is_empty(),
            "unchanged comment evidence consumes no model budget"
        );

        let partial = SyncBatch {
            posts: Vec::new(),
            comments: vec![make_comment("one", "First changed", 1)],
            comment_scope_post_ids: vec![post.remote_id.clone()],
            cursor: Some("cursor-2".into()),
            page_finality: PageFinality::Partial,
            comment_completeness: CommentCompleteness::Partial,
            comments_truncated: true,
            health: initial.health.clone(),
            rss: None,
        };
        let changed = database
            .changed_posts_for_sync_batch_fenced(&source, &partial, None)
            .expect("classify partial");
        assert_eq!(changed.len(), 1, "comment-only change returns stored post");
        database
            .ingest_sync_batch_fenced(
                &source,
                "partial-page",
                &partial,
                prepared(
                    changed,
                    "Partial and truncated evidence; omitted discussion may differ.",
                ),
                None,
            )
            .expect("partial ingest");
        source.cursor = Some("cursor-2".into());
        let partial_state: String = database
            .connection
            .query_row(
                "SELECT state FROM jobs WHERE dedupe_key='mastodon-sync:partial-page'",
                [],
                |row| row.get(0),
            )
            .expect("partial job");
        assert_eq!(partial_state, "partial");
        let partial_source: (Option<String>, String) = database.connection.query_row(
            "SELECT s.sync_cursor, m.page_finality FROM sources s JOIN source_sync_metadata m ON m.source_id=s.id
             WHERE s.id='mastodon-finality'",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("partial source finality");
        assert_eq!(partial_source, (Some("cursor-2".into()), "partial".into()));
        let comment_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM comments WHERE source_id='mastodon-finality'",
                [],
                |row| row.get(0),
            )
            .expect("comments");
        assert_eq!(comment_count, 2, "partial snapshot retains omitted comment");

        let complete_delete = SyncBatch {
            posts: Vec::new(),
            comments: vec![make_comment("one", "First changed", 1)],
            comment_scope_post_ids: vec![post.remote_id.clone()],
            cursor: None,
            page_finality: PageFinality::Complete,
            comment_completeness: CommentCompleteness::Complete,
            comments_truncated: false,
            health: initial.health.clone(),
            rss: None,
        };
        let epoch_before: i64 = database
            .connection
            .query_row(
                "SELECT privacy_epoch FROM app_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("epoch");
        let changed = database
            .changed_posts_for_sync_batch_fenced(&source, &complete_delete, None)
            .expect("classify deletion");
        database
            .ingest_sync_batch_fenced(
                &source,
                "complete-delete",
                &complete_delete,
                prepared(changed, "1 comment in complete snapshot."),
                None,
            )
            .expect("delete reconciliation");
        let remaining: Vec<String> = database.connection.prepare(
            "SELECT remote_id FROM comments WHERE source_id='mastodon-finality' ORDER BY position, published_at, remote_id"
        ).expect("statement").query_map([], |row| row.get(0)).expect("query")
          .collect::<Result<_, _>>().expect("remaining");
        assert_eq!(remaining, vec!["one"]);
        let epoch_after: i64 = database
            .connection
            .query_row(
                "SELECT privacy_epoch FROM app_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("epoch");
        assert_eq!(epoch_after, epoch_before + 1);

        drop(database);
        let mut database = Database::open(&path).expect("reopen");
        let (input_hash, provenance): (String, String) = database.connection.query_row(
            "SELECT s.input_hash, s.provenance_json FROM summaries s JOIN posts p ON p.id=s.post_id
             WHERE p.source_id='mastodon-finality'",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("summary provenance");
        let expected_hash: String = database.connection.query_row(
            "SELECT summary_input_hash FROM post_comment_state pcs JOIN posts p ON p.id=pcs.post_id
             WHERE p.source_id='mastodon-finality'",
            [], |row| row.get(0),
        ).expect("expected hash");
        assert_eq!(input_hash, expected_hash);
        assert!(provenance.contains("comment_evidence_hash"));

        let old = (Utc::now() - Duration::days(40)).timestamp_millis();
        database
            .connection
            .execute(
                "UPDATE comments SET fetched_at=?1 WHERE source_id='mastodon-finality'",
                [old],
            )
            .expect("age comment");
        let epoch_before_retention: i64 = database
            .connection
            .query_row(
                "SELECT privacy_epoch FROM app_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("epoch");
        database.apply_retention().expect("retention");
        let summary_count: i64 = database.connection.query_row(
            "SELECT COUNT(*) FROM summaries s JOIN posts p ON p.id=s.post_id WHERE p.source_id='mastodon-finality'",
            [], |row| row.get(0),
        ).expect("summary count");
        assert_eq!(summary_count, 0, "retention removes stale derived overview");
        let epoch_after_retention: i64 = database
            .connection
            .query_row(
                "SELECT privacy_epoch FROM app_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("epoch");
        assert_eq!(epoch_after_retention, epoch_before_retention + 1);
        let retained_state: (String, i64) = database.connection.query_row(
            "SELECT pcs.status, pcs.truncated FROM post_comment_state pcs JOIN posts p ON p.id=pcs.post_id
             WHERE p.source_id='mastodon-finality'",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("retained state");
        assert_eq!(retained_state, ("partial".into(), 1));
    }

    #[test]
    fn model_summary_persists_exact_identity_and_grounded_provenance() {
        let mut database = Database::memory().expect("database");
        database
            .begin_request("add-model", "add_rss_source", "hash")
            .expect("receipt");
        let post = NormalizedPost {
            remote_id: "remote-model".into(),
            canonical_url: Some("https://example.test/model".into()),
            author: "Author".into(),
            title: "Model item".into(),
            body_text: "Grounded evidence.".into(),
            published_at: Utc::now().timestamp_millis(),
            timestamp_kind: crate::connectors::TimestampKind::Published,
        };
        let page = SyncPage {
            posts: vec![post.clone()],
            effective_url: "https://cdn.example.test/feed".into(),
            etag: Some("\"etag\"".into()),
            last_modified: None,
            not_modified: false,
        };
        let prepared = vec![PreparedPost {
            input_hash: summary_input_hash_for(&post, &[], CommentCompleteness::Unavailable, false),
            post,
            summary: GroundedSummary {
                summary: "Grounded evidence.".into(),
                comment_overview: "No comments.".into(),
                uncertainty: "Verify.".into(),
            },
            provider: "Ollama-compatible".into(),
            model_id: Some("chosen:7b@sha256:abc".into()),
            prompt_version: crate::inference::PROMPT_VERSION.into(),
            summary_method: "model".into(),
        }];
        database
            .add_rss_source(
                "add-model",
                "Model feed",
                "https://example.test/feed",
                &page,
                prepared,
            )
            .expect("ingest");
        let (provider, model_id, method, provenance): (String, Option<String>, String, String) =
            database
                .connection
                .query_row(
                    "SELECT provider, model_id, summary_method, provenance_json FROM summaries",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .expect("summary");
        assert_eq!(provider, "Ollama-compatible");
        assert_eq!(model_id.as_deref(), Some("chosen:7b@sha256:abc"));
        assert_eq!(method, "model");
        assert!(provenance.contains("input_hash") && provenance.contains("social-summary-v2"));
    }

    #[test]
    fn dashboard_requires_exact_current_summary_hash() {
        let mut database = Database::memory().expect("database");
        seed_post(&mut database, "source-a", "post-a");
        database.run_digest("digest-a").expect("digest");
        database
            .connection
            .execute(
                "UPDATE summaries SET input_hash='stale' WHERE post_id='post-a'",
                [],
            )
            .expect("stale");
        assert!(
            database
                .dashboard(model(), host())
                .expect("dashboard")
                .items
                .is_empty()
        );
    }
}
