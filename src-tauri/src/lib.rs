pub mod capabilities;
pub mod clustering;
pub mod connectors;
pub mod db;
pub mod domain;
pub mod inference;
pub mod ranking;
pub mod redaction;
pub mod scheduler;
pub mod secrets;

use std::fs::File;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use chrono::{Local, Utc};
use connectors::{
    Connector, ConnectorError, ConnectorSyncRequest, ConnectorTransport, MastodonConnector,
    MastodonLoopbackCallback, RssConnector, SecretValue, SourceKind, SourceSyncSpec, SyncPage,
    SyncRequest, exchange_mastodon_authorization_code,
    export_import::{
        ImportError, ImportPlatform, MAX_IMPORT_FILE_BYTES, MAX_IMPORT_ITEMS, parse_export_file,
    },
    mastodon_authorization_url, mastodon_registration_request, new_mastodon_pkce,
    probe_mastodon_instance as probe_mastodon_oauth_metadata, register_mastodon_client,
    validate_sync_request,
};
use db::{
    Database, InferenceCandidate, PreparedPost, RequestDisposition, RssSourceSpec,
    SourceSelectionMode, content_hash, validate_id, validate_source_label,
};
use domain::{
    AddRssSourceRequest, AppError, AppResult, ConnectMastodonRequest, Dashboard,
    DeleteSourceRequest, DiscoverFeedsRequest, FeedbackRequest, ImportArchiveRequest,
    ImportArchiveResult, ImportArchiveStatus, MastodonProbeResult, ModelState, OpenOriginalRequest,
    OpmlCandidate, ProbeMastodonInstanceRequest, RenameSourceRequest, ResetLearningRequest,
    RestoreBackupRequest, RunDigestRequest, SearchLibraryRequest, SetSavedRequest,
    SetSourcePausedRequest, SyncSourceRequest, SyncSourcesRequest, SyncSourcesResult,
    UndoFeedbackRequest, UpdateSettingsRequest,
};
use inference::{
    DeterministicFallback, InferenceProvider, OllamaProvider, PROMPT_VERSION, SummaryRequest,
    fallback_status,
};
use secrets::{OsSecretStore, SecretStore};
use tauri::{
    AppHandle, Manager, State, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;

const OLLAMA_ENDPOINT: &str = "http://127.0.0.1:11434";
const MAX_ORIGINAL_URL_BYTES: usize = 2 * 1024;
// Whole-run model-summarization budget. Bounded by the existing envelope this
// runner already promises: MAX_SOURCES_PER_RUN (20) sources at the RSS
// transport's worst-case REQUEST_TIMEOUT (15s, connectors/rss.rs) can consume
// up to 20*15s = 300s of the RUNNER_DEADLINE's 480s (8 minutes) before any
// model call happens. Each model item costs at most one OllamaProvider
// generation call at its default 30s timeout (inference.rs). The remaining
// 480s - 300s = 180s headroom therefore allows at most floor(180/30) = 6
// model items per whole run without risking the existing 8-minute deadline
// even in the pathological case where every source's fetch times out before
// any model call starts. Raised from the previous placeholder of 4, which
// left most 8-item editions mostly extractive with no headroom analysis.
const MAX_MODEL_ITEMS_PER_BATCH: usize = 6;
// "Finite by design" is a core product invariant: this compiles into a build
// failure (not just a test) if the cap is ever widened past a small hard
// bound or made unlimited.
const _: () = assert!(MAX_MODEL_ITEMS_PER_BATCH > 0 && MAX_MODEL_ITEMS_PER_BATCH <= 10);
const MAX_ARCHIVE_MODEL_ITEMS_PER_IMPORT: usize = 1;
const _: () = assert!(
    MAX_ARCHIVE_MODEL_ITEMS_PER_IMPORT > 0
        && MAX_ARCHIVE_MODEL_ITEMS_PER_IMPORT <= MAX_MODEL_ITEMS_PER_BATCH
);
const MAX_SOURCES_PER_RUN: usize = 20;
const RUNNER_LEASE_MS: i64 = 10 * 60 * 1_000;
const RUNNER_DEADLINE: Duration = Duration::from_secs(8 * 60);
const MAX_OPML_FILE_BYTES: u64 = 1024 * 1024;
const MAX_OPML_CANDIDATES: usize = 500;

async fn bounded_deadline<F: std::future::Future>(
    duration: Duration,
    work: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    tokio::time::timeout(duration, work).await
}

struct SourceSyncResult {
    attempted_model_items: usize,
    changed_items: usize,
    changed: bool,
}

fn empty_sync_outcome(mode: domain::SyncMode) -> domain::SyncOutcome {
    domain::SyncOutcome {
        mode,
        finality: domain::SyncFinality::Complete,
        changed_sources: 0,
        unchanged_sources: 0,
        failed_sources: 0,
        changed_items: 0,
        source_limit_reached: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalCommandAdmission {
    Execute,
    ReplayComplete,
}

/// Add and delete cross the network/vault boundary, so an unknown crash outcome must never be
/// retried. This admission check stays before construction/use of either external adapter.
fn admit_external_command(disposition: RequestDisposition) -> AppResult<ExternalCommandAdmission> {
    match disposition {
        RequestDisposition::New => Ok(ExternalCommandAdmission::Execute),
        RequestDisposition::Complete => Ok(ExternalCommandAdmission::ReplayComplete),
        RequestDisposition::Unknown => Err(AppError::conflict(
            "That earlier request has unknown finality and was not repeated. Refresh before starting a new request.",
        )),
    }
}

/// Local mutations also fail closed on stale Unknown finality. Complete remains an idempotent
/// replay; only New may execute a durable effect.
fn admit_local_command(disposition: RequestDisposition) -> AppResult<ExternalCommandAdmission> {
    match disposition {
        RequestDisposition::New => Ok(ExternalCommandAdmission::Execute),
        RequestDisposition::Complete => Ok(ExternalCommandAdmission::ReplayComplete),
        RequestDisposition::Unknown => Err(AppError::conflict(
            "That earlier local request has unknown finality and was not reported as complete. Refresh before choosing again.",
        )),
    }
}

pub struct AppState {
    database: Mutex<Database>,
    database_path: PathBuf,
    model: tokio::sync::Mutex<Option<OllamaProvider>>,
    sync_gate: tokio::sync::Mutex<()>,
    runner_active: AtomicBool,
    in_flight: AtomicBool,
    secrets: OsSecretStore,
}

impl AppState {
    fn new(database_path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let database = Database::open(&database_path)?;
        let secrets = OsSecretStore;
        // A credential provider can fail after accepting a write. These rows contain only the
        // opaque credential reference, never a token; retrying deletion is safe and makes a
        // interrupted connection recoverable on every supported desktop platform.
        let pending_cleanups = database
            .pending_vault_cleanups()
            .map_err(|_| std::io::Error::other("could not read pending vault cleanup records"))?;
        for (request_id, secret_ref) in pending_cleanups {
            if secrets.delete(&secret_ref).is_ok() {
                database
                    .clear_pending_vault_cleanup(&request_id)
                    .map_err(|_| std::io::Error::other("could not clear pending vault cleanup"))?;
            }
        }
        Ok(Self {
            database: Mutex::new(database),
            database_path,
            model: tokio::sync::Mutex::new(None),
            sync_gate: tokio::sync::Mutex::new(()),
            runner_active: AtomicBool::new(false),
            in_flight: AtomicBool::new(false),
            secrets,
        })
    }

    fn database(&self) -> AppResult<MutexGuard<'_, Database>> {
        self.database.lock().map_err(|_| AppError::internal())
    }

    async fn model_status(&self, selected_model: &str) -> domain::ModelStatus {
        if selected_model.is_empty() {
            return fallback_status(
                "No installed Ollama model is selected. Deterministic local extraction is active.",
            );
        }
        let mut slot = self.model.lock().await;
        if slot
            .as_ref()
            .is_none_or(|provider| provider.model() != selected_model)
        {
            *slot = OllamaProvider::new(OLLAMA_ENDPOINT, selected_model).ok();
        }
        match slot.as_ref() {
            Some(provider) => provider.health().await,
            None => fallback_status(
                "The selected model name is invalid; deterministic fallback remains active.",
            ),
        }
    }

    async fn installed_models(&self) -> Vec<String> {
        OllamaProvider::installed_models(OLLAMA_ENDPOINT)
            .await
            .unwrap_or_default()
    }

    async fn dashboard(&self) -> AppResult<Dashboard> {
        let settings = self.database()?.settings()?;
        let model = self.model_status(&settings.selected_model).await;
        let host = capabilities::detect_host(&model);
        let mut database = self.database()?;
        database
            .apply_retention()
            .map_err(|_| AppError::internal())?;
        let mut dashboard = database.dashboard(model, host)?;
        dashboard.runner = database.runner_status(
            self.runner_active.load(Ordering::SeqCst),
            self.in_flight.load(Ordering::SeqCst),
        )?;
        dashboard.edition.next_edition_at = dashboard.runner.next_scheduled_at.clone();
        Ok(dashboard)
    }

    async fn prepare_posts(
        &self,
        candidates: &[InferenceCandidate],
        selected_model: &str,
        model_item_budget: usize,
    ) -> AppResult<(Vec<PreparedPost>, usize)> {
        let fallback = DeterministicFallback;
        let status = self.model_status(selected_model).await;
        let mut prepared = Vec::with_capacity(candidates.len());
        let mut model_slot = self.model.lock().await;
        let model_provider = (status.state == ModelState::Ready)
            .then(|| model_slot.as_mut())
            .flatten();
        let expected_digest = status.digest.as_deref();
        let attempted_model_items = if model_provider.is_some() && expected_digest.is_some() {
            candidates.len().min(model_item_budget)
        } else {
            0
        };
        for (index, candidate) in candidates.iter().cloned().enumerate() {
            let request = summary_request_for_candidate(&candidate);
            let post = candidate.post;
            let model_result = if index < model_item_budget {
                if let (Some(provider), Some(digest)) = (model_provider.as_deref(), expected_digest)
                {
                    tokio::time::timeout(
                        Duration::from_secs(30),
                        provider.summarize_attested(&request, digest),
                    )
                    .await
                    .ok()
                    .and_then(Result::ok)
                } else {
                    None
                }
            } else {
                None
            }
            .filter(|summary| {
                if candidate.comment_completeness == connectors::CommentCompleteness::Partial
                    || candidate.comments_truncated
                {
                    let overview = summary.comment_overview.to_ascii_lowercase();
                    overview.contains("partial") || overview.contains("truncat")
                } else {
                    true
                }
            });
            let (summary, provider, model_id, prompt_version, summary_method) =
                if let Some(summary) = model_result {
                    let exact_id = status
                        .model
                        .as_ref()
                        .zip(status.digest.as_ref())
                        .map(|(name, digest)| format!("{name}@{digest}"));
                    let provider_label = exact_id.as_ref().map_or_else(
                        || "Ollama-compatible".to_owned(),
                        |identity| format!("Ollama-compatible · {identity}"),
                    );
                    (
                        summary,
                        provider_label,
                        exact_id,
                        PROMPT_VERSION.to_owned(),
                        "model".to_owned(),
                    )
                } else {
                    let summary = fallback
                        .summarize(&request)
                        .await
                        .map_err(|_| AppError::internal())?;
                    (
                        summary,
                        "deterministic-fallback".to_owned(),
                        None,
                        "extractive-v1".to_owned(),
                        "extractive".to_owned(),
                    )
                };
            prepared.push(PreparedPost {
                post,
                input_hash: candidate.input_hash,
                summary,
                provider,
                model_id,
                prompt_version,
                summary_method,
            });
        }
        Ok((prepared, attempted_model_items))
    }

    async fn sync_one(
        &self,
        source: &RssSourceSpec,
        request_id: &str,
        selected_model: &str,
        model_item_budget: usize,
        lease: Option<&db::RunnerLease>,
    ) -> Result<SourceSyncResult, ConnectorError> {
        let sync_request = SyncRequest {
            url: source.sync_url().to_owned(),
            etag: source.etag.clone(),
            last_modified: source.last_modified.clone(),
        };
        validate_sync_request(&sync_request)?;
        let connector = RssConnector::new()?;
        let batch = connector
            .sync(&ConnectorSyncRequest {
                source: SourceSyncSpec {
                    id: source.id.clone(),
                    kind: SourceKind::Rss,
                    generation: source.generation,
                    config_json: serde_json::json!({ "url": source.requested_url }).to_string(),
                    cursor: None,
                },
                auth: None,
                transport: ConnectorTransport::Rss(sync_request),
            })
            .await?;
        let page = SyncPage::try_from(batch)?;
        if page.not_modified {
            self.database()
                .map_err(|_| ConnectorError::Transient)?
                .complete_not_modified_fenced(source, request_id, &page, lease)
                .map_err(|_| ConnectorError::Transient)?;
            return Ok(SourceSyncResult {
                attempted_model_items: 0,
                changed_items: 0,
                changed: false,
            });
        }
        let changed_posts = self
            .database()
            .map_err(|_| ConnectorError::Transient)?
            .changed_posts_fenced(source, &page.posts, lease)
            .map_err(|_| ConnectorError::Transient)?;
        let candidates = changed_posts
            .into_iter()
            .map(InferenceCandidate::unavailable)
            .collect::<Vec<_>>();
        let (prepared, attempted_model_items) = self
            .prepare_posts(&candidates, selected_model, model_item_budget)
            .await
            .map_err(|_| ConnectorError::Transient)?;
        let (changed_items, _) = self
            .database()
            .map_err(|_| ConnectorError::Transient)?
            .ingest_existing_rss_fenced(source, request_id, &page, prepared, lease)
            .map_err(|_| ConnectorError::Transient)?;
        Ok(SourceSyncResult {
            attempted_model_items,
            changed_items,
            changed: changed_items > 0,
        })
    }

    async fn sync_mastodon_one(
        &self,
        source: &SourceSyncSpec,
        request_id: &str,
        selected_model: &str,
        model_item_budget: usize,
        lease: Option<&db::RunnerLease>,
    ) -> Result<SourceSyncResult, ConnectorError> {
        if source.kind != SourceKind::Mastodon {
            return Err(ConnectorError::InvalidFeed);
        }
        let secret_ref = self
            .database()
            .map_err(|_| ConnectorError::Transient)?
            .secret_ref_for_source(&source.id)
            .map_err(|_| ConnectorError::Transient)?
            .ok_or(ConnectorError::AuthRequired)?;
        let access_token = self
            .secrets
            .get(&secret_ref)
            .map_err(|_| ConnectorError::Transient)?;
        let access_token = SecretValue::new(access_token)?;
        let connector = MastodonConnector::new()?;
        let batch = connector
            .sync(&ConnectorSyncRequest {
                source: source.clone(),
                auth: Some(connectors::ConnectorAuth { access_token }),
                transport: ConnectorTransport::OfficialApi,
            })
            .await?;
        let candidates = self
            .database()
            .map_err(|_| ConnectorError::Transient)?
            .changed_posts_for_sync_batch_fenced(source, &batch, lease)
            .map_err(|_| ConnectorError::Transient)?;
        let (prepared, attempted_model_items) = self
            .prepare_posts(&candidates, selected_model, model_item_budget)
            .await
            .map_err(|_| ConnectorError::Transient)?;
        let (changed_items, _) = self
            .database()
            .map_err(|_| ConnectorError::Transient)?
            .ingest_sync_batch_fenced(source, request_id, &batch, prepared, lease)
            .map_err(|_| ConnectorError::Transient)?;
        Ok(SourceSyncResult {
            attempted_model_items,
            changed_items,
            changed: changed_items > 0,
        })
    }

    async fn sync_and_prepare(
        &self,
        mode: SourceSelectionMode,
        request_id: &str,
        mut lease: Option<db::RunnerLease>,
    ) -> AppResult<domain::SyncOutcome> {
        validate_id(request_id)?;
        let _guard = self
            .sync_gate
            .try_lock()
            .map_err(|_| AppError::conflict("A source sync or deletion is already running."))?;
        self.in_flight.store(true, Ordering::SeqCst);
        struct Flight<'a>(&'a AtomicBool);
        impl Drop for Flight<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _flight = Flight(&self.in_flight);
        if mode == SourceSelectionMode::ManualOverride {
            let payload_hash = content_hash("sync-all-rss-manual-override-v2");
            match self
                .database()?
                .begin_request(request_id, "sync_sources", &payload_hash)?
            {
                RequestDisposition::Complete => {
                    return Ok(empty_sync_outcome(domain::SyncMode::ManualOverride));
                }
                RequestDisposition::Unknown => {
                    let mut outcome = empty_sync_outcome(domain::SyncMode::ManualOverride);
                    outcome.finality = domain::SyncFinality::Unknown;
                    return Ok(outcome);
                }
                RequestDisposition::New => {}
            }
        }
        let now = Utc::now().timestamp_millis();
        let (sources, source_limit_reached, settings) = {
            let database = self.database()?;
            let (sources, capped) = database.source_sync_specs(mode, now, MAX_SOURCES_PER_RUN)?;
            (sources, capped, database.settings()?)
        };
        let mut outcome = domain::SyncOutcome {
            mode: if mode == SourceSelectionMode::ManualOverride {
                domain::SyncMode::ManualOverride
            } else {
                domain::SyncMode::ResidentDue
            },
            finality: domain::SyncFinality::Complete,
            changed_sources: 0,
            unchanged_sources: 0,
            failed_sources: 0,
            changed_items: 0,
            source_limit_reached,
        };
        let mut model_item_budget = MAX_MODEL_ITEMS_PER_BATCH;
        for (index, source) in sources.iter().enumerate() {
            if let Some(current) = lease.as_ref() {
                lease = Some(self.database()?.heartbeat_runner_lease(
                    current,
                    Utc::now().timestamp_millis(),
                    RUNNER_LEASE_MS,
                )?);
            }
            let child_request = format!("{request_id}:{index}");
            let result = match source.kind {
                SourceKind::Rss => {
                    let rss_source = self.database()?.rss_source(&source.id)?;
                    self.sync_one(
                        &rss_source,
                        &child_request,
                        &settings.selected_model,
                        model_item_budget,
                        lease.as_ref(),
                    )
                    .await
                }
                SourceKind::Mastodon => {
                    self.sync_mastodon_one(
                        source,
                        &child_request,
                        &settings.selected_model,
                        model_item_budget,
                        lease.as_ref(),
                    )
                    .await
                }
                SourceKind::Bluesky => Err(ConnectorError::AuthRequired),
            };
            match result {
                Ok(result) => {
                    model_item_budget =
                        model_item_budget.saturating_sub(result.attempted_model_items);
                    outcome.changed_items += result.changed_items;
                    if result.changed {
                        outcome.changed_sources += 1;
                    } else {
                        outcome.unchanged_sources += 1;
                    }
                }
                Err(error) => {
                    outcome.failed_sources += 1;
                    let message = match error {
                        ConnectorError::RateLimited => {
                            "RSS source rate-limited; bounded backoff scheduled"
                        }
                        ConnectorError::UnsafeUrl => {
                            "RSS source URL no longer passes the public-network policy"
                        }
                        ConnectorError::ResponseTooLarge => "RSS response exceeded the 2 MB limit",
                        ConnectorError::InvalidFeed => "RSS response was not a valid bounded feed",
                        ConnectorError::AuthRequired => {
                            "Source authorization is required; automatic retry is paused"
                        }
                        ConnectorError::Transient => {
                            "RSS source could not be reached within the bounded request"
                        }
                    };
                    if source.kind == SourceKind::Rss {
                        let rss_source = self.database()?.rss_source(&source.id)?;
                        let _ = self.database()?.record_sync_failure_fenced(
                            &rss_source,
                            &child_request,
                            message,
                            lease.as_ref(),
                        );
                    }
                }
            }
        }
        if let Some(current) = lease.as_ref() {
            let _ = self.database()?.heartbeat_runner_lease(
                current,
                Utc::now().timestamp_millis(),
                RUNNER_LEASE_MS,
            )?;
        }
        outcome.finality = if outcome.failed_sources > 0 || outcome.source_limit_reached {
            domain::SyncFinality::Partial
        } else {
            domain::SyncFinality::Complete
        };
        self.database()?
            .run_digest_fenced(request_id, lease.as_ref())?;
        if mode == SourceSelectionMode::ManualOverride {
            self.database()?.complete_request(request_id)?;
        }
        Ok(outcome)
    }

    async fn sync_source_and_prepare(&self, request_id: &str, source_id: &str) -> AppResult<()> {
        validate_id(request_id)?;
        validate_id(source_id)?;
        let _guard = self
            .sync_gate
            .try_lock()
            .map_err(|_| AppError::conflict("A source sync or deletion is already running."))?;
        if admit_external_command(self.database()?.begin_request(
            request_id,
            "sync_source",
            &content_hash(source_id),
        )?)? == ExternalCommandAdmission::ReplayComplete
        {
            return Ok(());
        }
        let (source, selected_model) = {
            let database = self.database()?;
            (
                database.connector_source(source_id)?,
                database.settings()?.selected_model,
            )
        };
        let result = match source.kind {
            SourceKind::Rss => {
                let source = self.database()?.rss_source(source_id)?;
                self.sync_one(
                    &source,
                    request_id,
                    &selected_model,
                    MAX_MODEL_ITEMS_PER_BATCH,
                    None,
                )
                .await
            }
            SourceKind::Mastodon => {
                self.sync_mastodon_one(
                    &source,
                    request_id,
                    &selected_model,
                    MAX_MODEL_ITEMS_PER_BATCH,
                    None,
                )
                .await
            }
            SourceKind::Bluesky => Err(ConnectorError::AuthRequired),
        };
        match result {
            Ok(_) => {
                self.database()?.run_digest(request_id)?;
                self.database()?.complete_request(request_id)?;
                Ok(())
            }
            Err(error) => {
                self.database()?.abort_request(request_id);
                Err(map_connector_error(error))
            }
        }
    }

    async fn resident_tick(&self) -> AppResult<()> {
        self.database()?
            .apply_retention()
            .map_err(|_| AppError::internal())?;
        let settings = self.database()?.settings()?;
        let now = Local::now();
        let next = scheduler::next_eligible_run(
            now,
            settings.schedule_enabled,
            settings.schedule_hour,
            settings.quiet_hours_start,
            settings.quiet_hours_end,
            self.database()?.last_runner_handled()?,
        )
        .map(|value| value.timestamp_millis());
        self.database()?.set_next_scheduled(next)?;
        let last_handled = self.database()?.last_runner_handled()?;
        let Some(scheduled_for) = scheduler::scheduled_due(
            now,
            settings.schedule_enabled,
            settings.schedule_hour,
            settings.quiet_hours_start,
            settings.quiet_hours_end,
            last_handled,
        ) else {
            return Ok(());
        };
        let owner = format!("runner-{}", uuid::Uuid::new_v4().simple());
        let Some(lease) = self.database()?.acquire_runner_lease(
            &owner,
            scheduled_for,
            Utc::now().timestamp_millis(),
            RUNNER_LEASE_MS,
        )?
        else {
            return Ok(());
        };
        let request_id = format!("resident-{scheduled_for}-{}", lease.token);
        let result = bounded_deadline(
            RUNNER_DEADLINE,
            self.sync_and_prepare(
                SourceSelectionMode::ResidentDue,
                &request_id,
                Some(lease.clone()),
            ),
        )
        .await;
        let next = scheduler::next_eligible_run(
            Local::now(),
            settings.schedule_enabled,
            settings.schedule_hour,
            settings.quiet_hours_start,
            settings.quiet_hours_end,
            Some(scheduled_for),
        )
        .map(|value| value.timestamp_millis());
        let (outcome, detail, error) = match result {
            Ok(Ok(batch)) if batch.failed_sources == 0 && !batch.source_limit_reached => (
                db::RunnerOutcome::Complete,
                "Scheduled due-source sync and finite edition completed while Web was open.",
                None,
            ),
            Ok(Ok(_)) => (
                db::RunnerOutcome::Partial,
                "Scheduled edition completed partially; successful sources were retained and failed or capped sources remain eligible under bounded policy.",
                None,
            ),
            Ok(Err(error)) if error.code == "CONFLICT" => (
                db::RunnerOutcome::Unknown,
                "Scheduled work was deferred by another local source operation and remains recoverable for this nearest instant.",
                None,
            ),
            Ok(Err(error)) => (
                db::RunnerOutcome::Failed,
                "Scheduled work failed safely; the prior edition remains available.",
                Some(error),
            ),
            Err(_) => (
                db::RunnerOutcome::Unknown,
                "Scheduled work reached its eight-minute deadline; its partial outcome is unknown and recoverable once for this instant.",
                None,
            ),
        };
        self.database()?.finish_runner_lease(
            &lease,
            outcome,
            detail,
            next,
            Utc::now().timestamp_millis(),
        )?;
        error.map_or(Ok(()), Err)
    }
}

fn summary_request_for_candidate(candidate: &InferenceCandidate) -> SummaryRequest {
    let comments = db::canonical_comments(&candidate.comments);
    SummaryRequest {
        title: candidate.post.title.clone(),
        body: candidate.post.body_text.clone(),
        comments: comments
            .iter()
            .take(connectors::MAX_COMMENTS_PER_POST)
            .map(|comment| comment.body_text.clone())
            .collect(),
        comment_completeness: candidate.comment_completeness,
        comments_truncated: candidate.comments_truncated,
    }
}

fn map_import_error(error: ImportError) -> AppError {
    match error {
        ImportError::FileTooLarge => {
            AppError::validation("That archive file is larger than the 20 MiB import limit.")
        }
        ImportError::TooManyItems => AppError::validation(format!(
            "That archive contains more than {MAX_IMPORT_ITEMS} entries. Import a smaller archive part."
        )),
        ImportError::ConflictingDuplicate { .. } => {
            AppError::validation("That archive contains conflicting entries for the same post.")
        }
        ImportError::UnreadableFile => AppError::validation("That archive file could not be read."),
        ImportError::UnrecognizedFormat => {
            AppError::validation("That file is not a recognized archive export.")
        }
        ImportError::NoItemsFound => {
            AppError::validation("No importable posts were found in that archive file.")
        }
    }
}

fn read_import_file_bounded(path: &Path) -> Result<Vec<u8>, ImportError> {
    let metadata = std::fs::metadata(path).map_err(|_| ImportError::UnreadableFile)?;
    if !metadata.is_file() {
        return Err(ImportError::UnreadableFile);
    }
    if metadata.len() > MAX_IMPORT_FILE_BYTES {
        return Err(ImportError::FileTooLarge);
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| ImportError::FileTooLarge)?;
    let file = File::open(path).map_err(|_| ImportError::UnreadableFile)?;
    let mut reader = file.take(MAX_IMPORT_FILE_BYTES.saturating_add(1));
    let mut bytes = Vec::with_capacity(capacity);
    reader
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError::UnreadableFile)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IMPORT_FILE_BYTES {
        return Err(ImportError::FileTooLarge);
    }
    Ok(bytes)
}

async fn pick_archive_file(
    app: &AppHandle,
    platform: ImportPlatform,
) -> AppResult<Option<PathBuf>> {
    let picker = app
        .dialog()
        .file()
        .set_title("Choose an official social archive export");
    let picker = match platform {
        ImportPlatform::X => picker.add_filter("X data export", &["js", "json"]),
        ImportPlatform::Instagram => picker.add_filter("Instagram data export", &["json"]),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    picker.pick_file(move |selection| {
        let _ = sender.send(selection);
    });
    match receiver.await.map_err(|_| AppError::internal())? {
        Some(file) => file
            .into_path()
            .map(Some)
            .map_err(|_| AppError::validation("The selected archive is not a local file.")),
        None => Ok(None),
    }
}

async fn pick_opml_file(app: &AppHandle) -> AppResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose an OPML feed list")
        .add_filter("OPML feed list", &["opml", "xml"])
        .pick_file(move |selection| {
            let _ = sender.send(selection);
        });
    match receiver.await.map_err(|_| AppError::internal())? {
        Some(file) => file
            .into_path()
            .map(Some)
            .map_err(|_| AppError::validation("The selected OPML file is not a local file.")),
        None => Ok(None),
    }
}

async fn pick_opml_save_path(app: &AppHandle) -> AppResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export Web RSS subscriptions")
        .set_file_name("web-rss-sources.opml")
        .add_filter("OPML feed list", &["opml"])
        .save_file(move |selection| {
            let _ = sender.send(selection);
        });
    match receiver.await.map_err(|_| AppError::internal())? {
        Some(file) => file
            .into_path()
            .map(Some)
            .map_err(|_| AppError::validation("The chosen OPML location is not a local file.")),
        None => Ok(None),
    }
}

async fn pick_backup_save_path(app: &AppHandle) -> AppResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Save a local Web backup")
        .set_file_name("web-backup.sqlite3")
        .add_filter("Web SQLite backup", &["sqlite3"])
        .save_file(move |selection| {
            let _ = sender.send(selection);
        });
    match receiver.await.map_err(|_| AppError::internal())? {
        Some(file) => file
            .into_path()
            .map(Some)
            .map_err(|_| AppError::validation("The chosen backup location is not a local file.")),
        None => Ok(None),
    }
}

async fn pick_saved_items_export_path(app: &AppHandle) -> AppResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export saved Web items")
        .set_file_name("web-saved-items.json")
        .add_filter("Web saved items", &["json"])
        .save_file(move |selection| {
            let _ = sender.send(selection);
        });
    match receiver.await.map_err(|_| AppError::internal())? {
        Some(file) => file
            .into_path()
            .map(Some)
            .map_err(|_| AppError::validation("The chosen export location is not a local file.")),
        None => Ok(None),
    }
}

async fn pick_backup_restore_path(app: &AppHandle) -> AppResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Choose a Web SQLite backup")
        .add_filter("Web SQLite backup", &["sqlite3", "db"])
        .pick_file(move |selection| {
            let _ = sender.send(selection);
        });
    match receiver.await.map_err(|_| AppError::internal())? {
        Some(file) => file
            .into_path()
            .map(Some)
            .map_err(|_| AppError::validation("The selected backup is not a local file.")),
        None => Ok(None),
    }
}

fn read_opml_file_bounded(path: &Path) -> AppResult<Vec<u8>> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| AppError::validation("That OPML file could not be read."))?;
    if !metadata.is_file() || metadata.len() > MAX_OPML_FILE_BYTES {
        return Err(AppError::validation(
            "Choose a local OPML file up to 1 MiB.",
        ));
    }
    let mut reader = File::open(path)
        .map_err(|_| AppError::validation("That OPML file could not be read."))?
        .take(MAX_OPML_FILE_BYTES + 1);
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    reader
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::validation("That OPML file could not be read."))?;
    if bytes.len() > usize::try_from(MAX_OPML_FILE_BYTES).unwrap_or(usize::MAX) {
        return Err(AppError::validation(
            "Choose a local OPML file up to 1 MiB.",
        ));
    }
    Ok(bytes)
}

fn attribute_value(tag: &str, attribute: &str) -> Option<String> {
    for quote in ['\"', '\''] {
        let needle = format!("{attribute}={quote}");
        if let Some(start) = tag.find(&needle) {
            let tail = &tag[start + needle.len()..];
            if let Some(end) = tail.find(quote) {
                return Some(
                    tail[..end]
                        .replace("&quot;", "\"")
                        .replace("&apos;", "'")
                        .replace("&lt;", "<")
                        .replace("&gt;", ">")
                        .replace("&amp;", "&"),
                );
            }
        }
    }
    None
}

fn parse_opml(bytes: &[u8]) -> AppResult<Vec<OpmlCandidate>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| AppError::validation("That OPML file is not valid UTF-8."))?;
    if !text.to_ascii_lowercase().contains("<opml") {
        return Err(AppError::validation("That file is not an OPML feed list."));
    }
    let mut candidates = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for fragment in text
        .split('<')
        .filter(|fragment| fragment.trim_start().starts_with("outline"))
    {
        let tag = fragment.split('>').next().unwrap_or_default();
        let Some(url) = attribute_value(tag, "xmlUrl").or_else(|| attribute_value(tag, "xmlurl"))
        else {
            continue;
        };
        let Ok(parsed) = Url::parse(&url) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host().is_none()
            || !seen.insert(url.clone())
        {
            continue;
        }
        let label = attribute_value(tag, "title")
            .or_else(|| attribute_value(tag, "text"))
            .unwrap_or_else(|| parsed.host_str().unwrap_or("Feed").to_owned());
        candidates.push(OpmlCandidate {
            label: label.chars().take(100).collect(),
            url,
        });
        if candidates.len() > MAX_OPML_CANDIDATES {
            return Err(AppError::validation(
                "This OPML list has more than 500 feeds. Split it into smaller lists.",
            ));
        }
    }
    if candidates.is_empty() {
        return Err(AppError::validation(
            "No public RSS or Atom URLs were found in that OPML file.",
        ));
    }
    Ok(candidates)
}

fn escape_opml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

fn render_opml_export(sources: &[(String, String)]) -> AppResult<Vec<u8>> {
    if sources.is_empty() {
        return Err(AppError::validation(
            "Add an RSS source before exporting an OPML list.",
        ));
    }
    if sources.len() > MAX_OPML_CANDIDATES {
        return Err(AppError::validation(
            "This library has more than 500 RSS sources. Export smaller groups first.",
        ));
    }
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head><title>Web RSS subscriptions</title></head>\n  <body>\n",
    );
    for (label, url) in sources {
        xml.push_str(&format!(
            "    <outline text=\"{}\" title=\"{}\" type=\"rss\" xmlUrl=\"{}\" />\n",
            escape_opml_attribute(label),
            escape_opml_attribute(label),
            escape_opml_attribute(url)
        ));
    }
    xml.push_str("  </body>\n</opml>\n");
    if xml.len() > usize::try_from(MAX_OPML_FILE_BYTES).unwrap_or(usize::MAX) {
        return Err(AppError::validation(
            "The OPML export would exceed 1 MiB. Export smaller source groups first.",
        ));
    }
    Ok(xml.into_bytes())
}

async fn import_archive_with_loader<F, Fut>(
    state: &AppState,
    request: &ImportArchiveRequest,
    loader: F,
) -> AppResult<ImportArchiveResult>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = AppResult<Option<Vec<u8>>>>,
{
    validate_id(&request.request_id)?;
    validate_source_label(&request.label)?;
    let _sync_guard = state
        .sync_gate
        .try_lock()
        .map_err(|_| AppError::conflict("A source sync or deletion is already running."))?;
    let payload_hash = content_hash(&format!(
        "{}\n{}",
        request.platform.as_str(),
        request.label.trim()
    ));
    let admission = admit_external_command(state.database()?.begin_request(
        &request.request_id,
        "import_archive",
        &payload_hash,
    )?)?;
    if admission == ExternalCommandAdmission::ReplayComplete {
        let source_id = state
            .database()?
            .source_id_for_request(&request.request_id)?
            .ok_or_else(AppError::internal)?;
        return Ok(ImportArchiveResult {
            status: ImportArchiveStatus::Replayed,
            source_id: Some(source_id),
            imported_items: 0,
            skipped_items: 0,
            changed_items: 0,
            dashboard: state.dashboard().await?,
        });
    }

    let bytes = match loader().await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            state.database()?.abort_request(&request.request_id);
            return Ok(ImportArchiveResult {
                status: ImportArchiveStatus::Canceled,
                source_id: None,
                imported_items: 0,
                skipped_items: 0,
                changed_items: 0,
                dashboard: state.dashboard().await?,
            });
        }
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(error);
        }
    };
    let parsed = match parse_export_file(request.platform, &bytes, request.label.trim()) {
        Ok(parsed) => parsed,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_import_error(error));
        }
    };
    let selected_model = state.database()?.settings()?.selected_model;
    let changed_posts = match state.database()?.changed_export_import_posts(
        &request.label,
        request.platform.as_str(),
        &parsed.posts,
    ) {
        Ok(posts) => posts,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(error);
        }
    };
    let candidates = changed_posts
        .into_iter()
        .map(InferenceCandidate::unavailable)
        .collect::<Vec<_>>();
    let (prepared, _) = match state
        .prepare_posts(
            &candidates,
            &selected_model,
            MAX_ARCHIVE_MODEL_ITEMS_PER_IMPORT,
        )
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(error);
        }
    };
    let imported_items = parsed.posts.len();
    let skipped_items = parsed.skipped;
    let stored = state.database()?.add_export_import_source(
        &request.request_id,
        &request.label,
        request.platform.as_str(),
        &parsed.posts,
        skipped_items,
        prepared,
    );
    let (source_id, changed_items) = match stored {
        Ok(result) => result,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(error);
        }
    };
    Ok(ImportArchiveResult {
        status: ImportArchiveStatus::Imported,
        source_id: Some(source_id),
        imported_items,
        skipped_items,
        changed_items,
        dashboard: state.dashboard().await?,
    })
}

fn validate_original_url(value: &str) -> AppResult<Url> {
    const MESSAGE: &str =
        "Only credential-free HTTPS source URLs up to 2 KiB can be opened externally.";

    if value.is_empty() || value.len() > MAX_ORIGINAL_URL_BYTES {
        return Err(AppError::validation(MESSAGE));
    }
    let url = Url::parse(value).map_err(|_| AppError::validation(MESSAGE))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::validation(MESSAGE));
    }
    Ok(url)
}

fn open_original_with<F, E>(request: &OpenOriginalRequest, open: F) -> AppResult<()>
where
    F: FnOnce(&str) -> Result<(), E>,
{
    let url = validate_original_url(&request.url)?;
    open(url.as_str()).map_err(|_| AppError::internal())
}

#[tauri::command]
fn open_original(app: AppHandle, request: OpenOriginalRequest) -> AppResult<()> {
    open_original_with(&request, |url| app.opener().open_url(url, None::<&str>))
}

#[tauri::command]
async fn get_dashboard(state: State<'_, AppState>) -> AppResult<Dashboard> {
    state.dashboard().await
}

#[tauri::command]
async fn get_edition(
    state: State<'_, AppState>,
    edition_id: String,
) -> AppResult<domain::EditionDetail> {
    state.database()?.edition_detail(&edition_id)
}

#[tauri::command]
async fn installed_models(state: State<'_, AppState>) -> AppResult<Vec<String>> {
    Ok(state.installed_models().await)
}

#[tauri::command]
async fn run_digest(state: State<'_, AppState>, request: RunDigestRequest) -> AppResult<Dashboard> {
    state.database()?.run_digest(&request.request_id)?;
    state.dashboard().await
}

#[tauri::command]
async fn sync_sources(
    state: State<'_, AppState>,
    request: SyncSourcesRequest,
) -> AppResult<SyncSourcesResult> {
    let outcome = match bounded_deadline(
        RUNNER_DEADLINE,
        state.sync_and_prepare(
            SourceSelectionMode::ManualOverride,
            &request.request_id,
            None,
        ),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            state
                .database()?
                .seal_request_unknown(&request.request_id, "sync_sources")?;
            let mut outcome = empty_sync_outcome(domain::SyncMode::ManualOverride);
            outcome.finality = domain::SyncFinality::Unknown;
            outcome
        }
    };
    Ok(SyncSourcesResult {
        dashboard: state.dashboard().await?,
        outcome,
    })
}

#[tauri::command]
async fn sync_source(
    state: State<'_, AppState>,
    request: SyncSourceRequest,
) -> AppResult<Dashboard> {
    state
        .sync_source_and_prepare(&request.request_id, &request.source_id)
        .await?;
    state.dashboard().await
}

#[tauri::command]
async fn record_feedback(
    state: State<'_, AppState>,
    request: FeedbackRequest,
) -> AppResult<Dashboard> {
    state
        .database()?
        .record_feedback(&request.request_id, &request.item_id, &request.signal)?;
    state.dashboard().await
}

#[tauri::command]
async fn undo_feedback(
    state: State<'_, AppState>,
    request: UndoFeedbackRequest,
) -> AppResult<Dashboard> {
    state.database()?.undo_feedback(&request.request_id)?;
    state.dashboard().await
}

fn update_settings_core(database: &mut Database, request: &UpdateSettingsRequest) -> AppResult<()> {
    let payload = serde_json::to_string(&request.settings).map_err(|_| AppError::internal())?;
    let payload_hash = content_hash(&payload);
    if admit_local_command(database.begin_request(
        &request.request_id,
        "update_settings",
        &payload_hash,
    )?)? == ExternalCommandAdmission::Execute
    {
        if let Err(error) = database.update_settings(&request.request_id, &request.settings) {
            database.abort_request(&request.request_id);
            return Err(error);
        }
        database.complete_request(&request.request_id)?;
    }
    Ok(())
}

#[tauri::command]
async fn update_settings(
    state: State<'_, AppState>,
    request: UpdateSettingsRequest,
) -> AppResult<Dashboard> {
    {
        let mut database = state.database()?;
        update_settings_core(&mut database, &request)?;
    }
    state.dashboard().await
}

#[tauri::command]
async fn add_rss_source(
    state: State<'_, AppState>,
    request: AddRssSourceRequest,
) -> AppResult<Dashboard> {
    validate_id(&request.request_id)?;
    if request.label.trim().is_empty() || request.label.chars().count() > 100 {
        return Err(AppError::validation(
            "The source label must be between 1 and 100 characters.",
        ));
    }
    let sync_request = SyncRequest {
        url: request.url.clone(),
        etag: None,
        last_modified: None,
    };
    validate_sync_request(&sync_request).map_err(map_connector_error)?;
    let _sync_guard = state
        .sync_gate
        .try_lock()
        .map_err(|_| AppError::conflict("A source sync or deletion is already running."))?;
    let payload_hash = content_hash(&format!("{}\n{}", request.label.trim(), request.url));
    let admission = admit_external_command(state.database()?.begin_request(
        &request.request_id,
        "add_rss_source",
        &payload_hash,
    )?)?;
    if admission == ExternalCommandAdmission::ReplayComplete {
        return state.dashboard().await;
    }
    let connector = RssConnector::new().map_err(map_connector_error)?;
    let provisional_source_id = format!("rss-{}", &content_hash(&request.url)[..20]);
    let page = match connector
        .sync(&ConnectorSyncRequest {
            source: SourceSyncSpec {
                id: provisional_source_id,
                kind: SourceKind::Rss,
                generation: 1,
                config_json: serde_json::json!({ "url": request.url }).to_string(),
                cursor: None,
            },
            auth: None,
            transport: ConnectorTransport::Rss(sync_request),
        })
        .await
        .and_then(SyncPage::try_from)
    {
        Ok(page) => page,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_connector_error(error));
        }
    };
    let selected_model = state.database()?.settings()?.selected_model;
    let candidates = page
        .posts
        .iter()
        .cloned()
        .map(InferenceCandidate::unavailable)
        .collect::<Vec<_>>();
    let (prepared, _) = match state
        .prepare_posts(&candidates, &selected_model, MAX_MODEL_ITEMS_PER_BATCH)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(error);
        }
    };
    {
        let mut database = state.database()?;
        if let Err(error) = database.add_rss_source(
            &request.request_id,
            &request.label,
            &request.url,
            &page,
            prepared,
        ) {
            database.abort_request(&request.request_id);
            return Err(error);
        }
        database.complete_request(&request.request_id)?;
    }
    state.dashboard().await
}

fn map_connector_error(error: ConnectorError) -> AppError {
    match error {
        ConnectorError::UnsafeUrl => {
            AppError::validation("That feed URL is unsafe or resolves to a private network.")
        }
        ConnectorError::ResponseTooLarge => {
            AppError::validation("That feed is larger than Web's 2 MB safety limit.")
        }
        ConnectorError::InvalidFeed => {
            AppError::validation("That address did not return a valid RSS or Atom feed.")
        }
        ConnectorError::AuthRequired => AppError::conflict(
            "This source requires renewed authorization before it can synchronize.",
        ),
        ConnectorError::RateLimited | ConnectorError::Transient => AppError::internal(),
    }
}

#[tauri::command]
async fn import_archive(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ImportArchiveRequest,
) -> AppResult<ImportArchiveResult> {
    let platform = request.platform;
    import_archive_with_loader(&state, &request, move || async move {
        let Some(path) = pick_archive_file(&app, platform).await? else {
            return Ok(None);
        };
        let bytes = tokio::task::spawn_blocking(move || read_import_file_bounded(&path))
            .await
            .map_err(|_| AppError::internal())?
            .map_err(map_import_error)?;
        Ok(Some(bytes))
    })
    .await
}

#[tauri::command]
async fn delete_source(
    state: State<'_, AppState>,
    request: DeleteSourceRequest,
) -> AppResult<Dashboard> {
    validate_id(&request.request_id)?;
    validate_id(&request.source_id)?;
    let _sync_guard = state.sync_gate.lock().await;
    let admission = admit_external_command(
        state
            .database()?
            .begin_delete_request(&request.request_id, &request.source_id)?,
    )?;
    if admission == ExternalCommandAdmission::ReplayComplete {
        return state.dashboard().await;
    }
    let secret_ref = match state.database()?.secret_ref_for_source(&request.source_id) {
        Ok(secret_ref) => secret_ref,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(error);
        }
    };
    if let Some(secret_ref) = secret_ref
        && state.secrets.delete(&secret_ref).is_err()
    {
        state.database()?.abort_request(&request.request_id);
        return Err(AppError::new_secure_store_failure());
    }
    if let Err(error) = state
        .database()?
        .delete_source(&request.request_id, &request.source_id)
    {
        state.database()?.abort_request(&request.request_id);
        return Err(error);
    }
    state.dashboard().await
}

fn reset_learning_core(database: &mut Database, request: &ResetLearningRequest) -> AppResult<()> {
    let payload_hash = content_hash("reset-learning-v1");
    if admit_local_command(database.begin_request(
        &request.request_id,
        "reset_learning",
        &payload_hash,
    )?)? == ExternalCommandAdmission::Execute
    {
        if let Err(error) = database.reset_learning(&request.request_id) {
            database.abort_request(&request.request_id);
            return Err(error);
        }
        database.complete_request(&request.request_id)?;
    }
    Ok(())
}

#[tauri::command]
async fn reset_learning(
    state: State<'_, AppState>,
    request: ResetLearningRequest,
) -> AppResult<Dashboard> {
    {
        let mut database = state.database()?;
        reset_learning_core(&mut database, &request)?;
    }
    state.dashboard().await
}

#[tauri::command]
async fn search_library(
    state: State<'_, AppState>,
    request: SearchLibraryRequest,
) -> AppResult<Vec<domain::LibraryItem>> {
    state.database()?.search_library(&request.query)
}

#[tauri::command]
async fn saved_library(state: State<'_, AppState>) -> AppResult<Vec<domain::LibraryItem>> {
    state.database()?.saved_library()
}

#[tauri::command]
async fn set_saved(state: State<'_, AppState>, request: SetSavedRequest) -> AppResult<Dashboard> {
    let payload = format!("{}:{}", request.post_id, request.saved);
    {
        let mut database = state.database()?;
        if admit_local_command(database.begin_request(
            &request.request_id,
            "set_saved",
            &content_hash(&payload),
        )?)? == ExternalCommandAdmission::Execute
        {
            if let Err(error) =
                database.set_saved(&request.request_id, &request.post_id, request.saved)
            {
                database.abort_request(&request.request_id);
                return Err(error);
            }
            database.complete_request(&request.request_id)?;
        }
    }
    state.dashboard().await
}

#[tauri::command]
async fn rename_source(
    state: State<'_, AppState>,
    request: RenameSourceRequest,
) -> AppResult<Dashboard> {
    let payload = format!("{}:{}", request.source_id, request.label.trim());
    {
        let mut database = state.database()?;
        if admit_local_command(database.begin_request(
            &request.request_id,
            "rename_source",
            &content_hash(&payload),
        )?)? == ExternalCommandAdmission::Execute
        {
            if let Err(error) = database.rename_source(&request.source_id, &request.label) {
                database.abort_request(&request.request_id);
                return Err(error);
            }
            database.complete_request(&request.request_id)?;
        }
    }
    state.dashboard().await
}

#[tauri::command]
async fn set_source_paused(
    state: State<'_, AppState>,
    request: SetSourcePausedRequest,
) -> AppResult<Dashboard> {
    let payload = format!("{}:{}", request.source_id, request.paused);
    {
        let mut database = state.database()?;
        if admit_local_command(database.begin_request(
            &request.request_id,
            "set_source_paused",
            &content_hash(&payload),
        )?)? == ExternalCommandAdmission::Execute
        {
            if let Err(error) = database.set_source_paused(&request.source_id, request.paused) {
                database.abort_request(&request.request_id);
                return Err(error);
            }
            database.complete_request(&request.request_id)?;
        }
    }
    state.dashboard().await
}

#[tauri::command]
async fn pick_opml(app: AppHandle) -> AppResult<Vec<OpmlCandidate>> {
    let Some(path) = pick_opml_file(&app).await? else {
        return Ok(Vec::new());
    };
    let bytes = tokio::task::spawn_blocking(move || read_opml_file_bounded(&path))
        .await
        .map_err(|_| AppError::internal())??;
    parse_opml(&bytes)
}

#[tauri::command]
async fn discover_feeds(request: DiscoverFeedsRequest) -> AppResult<Vec<OpmlCandidate>> {
    let connector = RssConnector::new().map_err(map_connector_error)?;
    connector
        .discover_feeds(&request.url)
        .await
        .map(|feeds| {
            feeds
                .into_iter()
                .map(|(label, url)| OpmlCandidate { label, url })
                .collect()
        })
        .map_err(map_connector_error)
}

fn map_mastodon_probe_error(error: ConnectorError) -> AppError {
    match error {
        ConnectorError::UnsafeUrl => AppError::validation(
            "Use a public HTTPS Mastodon instance root, without a path, query, or credentials.",
        ),
        ConnectorError::ResponseTooLarge => AppError::validation(
            "That instance's OAuth metadata exceeds Web's 64 KiB safety limit.",
        ),
        ConnectorError::InvalidFeed => AppError::validation(
            "That instance did not provide compatible OAuth authorization-server metadata.",
        ),
        ConnectorError::AuthRequired => AppError::validation(
            "That instance does not advertise authorization-code PKCE S256 with the minimum read scopes.",
        ),
        ConnectorError::RateLimited => AppError::conflict(
            "That instance rate-limited the compatibility check. No account was connected; try again later.",
        ),
        ConnectorError::Transient => AppError::unavailable(
            "The instance could not be safely checked. No account was connected or stored.",
        ),
    }
}

#[tauri::command]
async fn probe_mastodon_instance(
    request: ProbeMastodonInstanceRequest,
) -> AppResult<MastodonProbeResult> {
    let endpoints = probe_mastodon_oauth_metadata(&request.instance_url)
        .await
        .map_err(map_mastodon_probe_error)?;
    Ok(MastodonProbeResult {
        instance_url: endpoints.instance.to_string(),
        supported_scopes: endpoints.scopes.into_iter().collect(),
        connection_enabled: false,
    })
}

fn abort_mastodon_connection(database: &Database, request_id: &str) {
    // Deleting the pending receipt cascades the non-secret cleanup reference. This is safe only
    // before a vault write has reported an indeterminate outcome.
    database.abort_request(request_id);
}

fn persist_mastodon_access_token<S: SecretStore>(
    database: &mut Database,
    secrets: &S,
    request: &ConnectMastodonRequest,
    source_id: &str,
    secret_ref: &str,
    access_token: &str,
) -> AppResult<()> {
    if secrets.put(secret_ref, access_token).is_err() {
        // A secure-store failure does not prove that no write occurred. Preserve its cleanup
        // reference and fail closed rather than allowing this request to replay.
        database.seal_request_unknown(&request.request_id, "connect_mastodon")?;
        return Err(AppError::new_secure_store_failure());
    }
    if let Err(error) = database.add_mastodon_source(
        &request.request_id,
        source_id,
        &request.label,
        &request.instance_url,
        secret_ref,
    ) {
        if secrets.delete(secret_ref).is_ok() {
            abort_mastodon_connection(database, &request.request_id);
            return Err(error);
        }
        database.seal_request_unknown(&request.request_id, "connect_mastodon")?;
        return Err(AppError::new_secure_store_failure());
    }
    Ok(())
}

/// The native connection orchestration is deliberately not registered in the invoke handler.
/// It is complete enough to make vault/database failure boundaries testable, but must stay out of
/// the renderer until the bounded read-only timeline connector can activate and remove sources
/// without exposing an authorization that cannot yet be consumed.
async fn connect_mastodon_natively<F>(
    state: &AppState,
    request: ConnectMastodonRequest,
    open_authorization: F,
) -> AppResult<()>
where
    F: FnOnce(&Url) -> AppResult<()>,
{
    validate_id(&request.request_id)?;
    validate_source_label(&request.label)?;
    connectors::validate_mastodon_instance_url(&request.instance_url).map_err(|_| {
        AppError::validation(
            "Use a public HTTPS Mastodon instance root, without a path, query, or credentials.",
        )
    })?;
    let payload_hash = content_hash(&format!(
        "{}\n{}",
        request.label.trim(),
        request.instance_url
    ));
    let admission = admit_external_command(state.database()?.begin_request(
        &request.request_id,
        "connect_mastodon",
        &payload_hash,
    )?)?;
    if admission == ExternalCommandAdmission::ReplayComplete {
        return Ok(());
    }
    let source_id = format!("mastodon-{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("mastodon-token-{}", uuid::Uuid::new_v4().simple());
    if let Err(error) = state
        .database()?
        .record_pending_vault_cleanup(&request.request_id, &secret_ref)
    {
        state.database()?.abort_request(&request.request_id);
        return Err(error);
    }
    let endpoints = match probe_mastodon_oauth_metadata(&request.instance_url).await {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_mastodon_probe_error(error));
        }
    };
    let listener = match MastodonLoopbackCallback::bind().await {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_mastodon_probe_error(error));
        }
    };
    let registration = match mastodon_registration_request(&endpoints, listener.redirect_uri()) {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_mastodon_probe_error(error));
        }
    };
    let client = match register_mastodon_client(&registration).await {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_mastodon_probe_error(error));
        }
    };
    let pkce = new_mastodon_pkce();
    let authorization = match mastodon_authorization_url(
        &endpoints,
        &client.client_id,
        listener.redirect_uri(),
        &pkce,
    ) {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_mastodon_probe_error(error));
        }
    };
    if let Err(error) = open_authorization(&authorization) {
        state.database()?.abort_request(&request.request_id);
        return Err(error);
    }
    let code = match listener.receive(&pkce).await {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_mastodon_probe_error(error));
        }
    };
    let token = match exchange_mastodon_authorization_code(&endpoints, &client, code, &pkce).await {
        Ok(value) => value,
        Err(error) => {
            state.database()?.abort_request(&request.request_id);
            return Err(map_mastodon_probe_error(error));
        }
    };
    let mut database = state.database()?;
    persist_mastodon_access_token(
        &mut database,
        &state.secrets,
        &request,
        &source_id,
        &secret_ref,
        token.expose(),
    )
}

#[tauri::command]
async fn connect_mastodon(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ConnectMastodonRequest,
) -> AppResult<Dashboard> {
    connect_mastodon_natively(&state, request, |authorization| {
        app.opener()
            .open_url(authorization.as_str(), None::<&str>)
            .map_err(|_| {
                AppError::unavailable("The system browser could not be opened for authorization.")
            })
    })
    .await?;
    state.dashboard().await
}

#[tauri::command]
async fn export_opml(app: AppHandle, state: State<'_, AppState>) -> AppResult<bool> {
    let bytes = {
        let database = state.database()?;
        render_opml_export(&database.opml_export_sources()?)?
    };
    let Some(path) = pick_opml_save_path(&app).await? else {
        return Ok(false);
    };
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temporary, bytes).map_err(|_| AppError::internal())?;
    replace_export_backup(&temporary, &path)?;
    Ok(true)
}

#[tauri::command]
async fn export_backup(app: AppHandle, state: State<'_, AppState>) -> AppResult<bool> {
    let Some(path) = pick_backup_save_path(&app).await? else {
        return Ok(false);
    };
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    {
        let database = state.database()?;
        database.backup_to(&temporary)?;
    }
    replace_export_backup(&temporary, &path)?;
    Ok(true)
}

fn render_saved_items_export(
    items: Vec<domain::LibraryItem>,
    exported_at: chrono::DateTime<Utc>,
) -> AppResult<Vec<u8>> {
    serde_json::to_vec_pretty(&domain::SavedLibraryExport::new(
        exported_at.to_rfc3339(),
        items,
    ))
    .map_err(|_| AppError::internal())
}

#[tauri::command]
async fn export_saved_items(app: AppHandle, state: State<'_, AppState>) -> AppResult<bool> {
    let bytes = {
        let database = state.database()?;
        let items = database.saved_library_export()?;
        if items.is_empty() {
            return Err(AppError::validation(
                "Save at least one item before creating a portable export.",
            ));
        }
        render_saved_items_export(items, Utc::now())?
    };
    let Some(path) = pick_saved_items_export_path(&app).await? else {
        return Ok(false);
    };
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temporary, bytes).map_err(|_| AppError::internal())?;
    replace_export_backup(&temporary, &path)?;
    Ok(true)
}

/// Move an existing user backup aside before publishing the fresh snapshot. This avoids deleting
/// the prior backup before the new file has a durable destination (notably on Windows, where
/// rename does not replace an existing file).
fn replace_export_backup(temporary: &Path, destination: &Path) -> AppResult<()> {
    if !destination.exists() {
        return std::fs::rename(temporary, destination).map_err(|_| AppError::internal());
    }
    let previous = destination.with_file_name(format!(
        ".web-backup-replace-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::rename(destination, &previous)
        .map_err(|_| AppError::validation("The existing backup could not be staged safely."))?;
    if std::fs::rename(temporary, destination).is_err() {
        let _ = std::fs::rename(&previous, destination);
        return Err(AppError::validation(
            "The new backup could not be saved; the previous backup was restored.",
        ));
    }
    let _ = std::fs::remove_file(previous);
    Ok(())
}

enum RestoreReplacement {
    Replaced(Database),
    RolledBack(Database),
}

fn cleanup_stale_restore_artifacts(directory: &Path) -> AppResult<()> {
    let entries = std::fs::read_dir(directory).map_err(|_| AppError::internal())?;
    for entry in entries {
        let entry = entry.map_err(|_| AppError::internal())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // Restore files are created only beside the application database. Keep cleanup deliberately
        // narrow so an interrupted restore can never sweep up user-selected backups or other data.
        if name == "web.restore-rollback.sqlite3"
            || (name.starts_with("web.restore-") && name.ends_with(".sqlite3"))
        {
            let path = entry.path();
            if path.is_file() {
                std::fs::remove_file(path).map_err(|_| AppError::internal())?;
            }
        }
    }
    Ok(())
}

fn remove_database_files(path: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let target = PathBuf::from(format!("{}{suffix}", path.display()));
        if target.exists() {
            std::fs::remove_file(target)?;
        }
    }
    Ok(())
}

fn reopen_rollback(main: &Path, rollback: &Path) -> AppResult<Database> {
    remove_database_files(main).map_err(|_| AppError::internal())?;
    std::fs::rename(rollback, main).map_err(|_| AppError::internal())?;
    Database::open(main).map_err(|_| AppError::internal())
}

/// Replacing SQLite files requires closing the active handle on Windows. Every failure after that
/// point must therefore reopen the independently-created rollback snapshot before the caller
/// releases the mutex back to the application.
fn replace_database_with_candidate<F>(
    main: &Path,
    candidate: &Path,
    rollback: &Path,
    reopen_candidate: F,
) -> AppResult<RestoreReplacement>
where
    F: FnOnce(&Path) -> Result<Database, rusqlite::Error>,
{
    let result = (|| -> Result<Database, ()> {
        remove_database_files(main).map_err(|_| ())?;
        std::fs::rename(candidate, main).map_err(|_| ())?;
        reopen_candidate(main).map_err(|_| ())
    })();

    match result {
        Ok(database) => {
            // A stale rollback is harmless but misleading, and it is not needed after the new
            // database has successfully reopened.
            let _ = std::fs::remove_file(rollback);
            Ok(RestoreReplacement::Replaced(database))
        }
        Err(()) => {
            let _ = std::fs::remove_file(candidate);
            reopen_rollback(main, rollback).map(RestoreReplacement::RolledBack)
        }
    }
}

fn validate_restore_candidate(candidate: &Path) -> AppResult<()> {
    // Opening validates SQLite integrity and runs every migration while the existing library is
    // still open and untouched.
    Database::open(candidate)
        .map(drop)
        .map_err(|_| AppError::validation("That file is not a compatible Web backup."))
}

#[tauri::command]
async fn restore_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RestoreBackupRequest,
) -> AppResult<Dashboard> {
    validate_id(&request.request_id)?;
    let Some(source) = pick_backup_restore_path(&app).await? else {
        return state.dashboard().await;
    };
    let metadata = std::fs::metadata(&source)
        .map_err(|_| AppError::validation("That backup could not be read."))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 * 1024 {
        return Err(AppError::validation(
            "Choose a non-empty local SQLite backup smaller than 2 GiB.",
        ));
    }
    let _sync_guard = state.sync_gate.lock().await;
    let restore_directory = state
        .database_path
        .parent()
        .ok_or_else(AppError::internal)?
        .join(".web-restore");
    std::fs::create_dir_all(&restore_directory).map_err(|_| AppError::internal())?;
    if source.starts_with(&restore_directory) {
        return Err(AppError::validation(
            "Choose a backup outside Web's temporary restore workspace.",
        ));
    }
    cleanup_stale_restore_artifacts(&restore_directory)?;
    let candidate = restore_directory.join(format!(
        "web.restore-{}.sqlite3",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::copy(&source, &candidate)
        .map_err(|_| AppError::validation("That backup could not be copied safely."))?;
    if validate_restore_candidate(&candidate).is_err() {
        let _ = std::fs::remove_file(&candidate);
        return Err(AppError::validation(
            "That file is not a compatible Web backup.",
        ));
    }
    let rollback = restore_directory.join("web.restore-rollback.sqlite3");
    {
        let database = state.database()?;
        if rollback.exists() {
            let _ = std::fs::remove_file(&rollback);
        }
        database.backup_to(&rollback)?;
    }
    {
        let mut database = state.database()?;
        let replacement = Database::memory().map_err(|_| AppError::internal())?;
        let prior = std::mem::replace(&mut *database, replacement);
        drop(prior);
    }
    let main = state.database_path.clone();
    match replace_database_with_candidate(&main, &candidate, &rollback, Database::open)? {
        RestoreReplacement::Replaced(restored) => {
            *state.database()? = restored;
            state.dashboard().await
        }
        RestoreReplacement::RolledBack(restored) => {
            *state.database()? = restored;
            Err(AppError::validation(
                "The backup could not replace your library. Your previous library was restored.",
            ))
        }
    }
}

impl AppError {
    fn new_secure_store_failure() -> Self {
        Self::validation(
            "The operating-system credential vault is unavailable, so the source was not deleted. Try again after unlocking the vault.",
        )
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            // A second launch must never create another resident scheduler against the same local
            // database. Bring the existing calm digest forward instead.
            show_main_window(app);
        }))
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;
            app.manage(AppState::new(app_data.join("web.sqlite3"))?);
            let handle = app.handle().clone();
            app.state::<AppState>()
                .runner_active
                .store(true, Ordering::SeqCst);
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    let state = handle.state::<AppState>();
                    let _ = state.resident_tick().await;
                }
            });
            let show = MenuItem::with_id(app, "show", "Show Web", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Web", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let _tray = TrayIconBuilder::with_id("web-tray")
                .tooltip("Web — your calm digest")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        show_main_window(app);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        let app = tray.app_handle();
                        show_main_window(app);
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let close_to_tray = window
                    .state::<AppState>()
                    .database()
                    .and_then(|database| database.settings())
                    .map(|settings| settings.close_to_tray)
                    .unwrap_or(false);
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_original,
            get_dashboard,
            get_edition,
            installed_models,
            run_digest,
            sync_sources,
            sync_source,
            record_feedback,
            undo_feedback,
            update_settings,
            add_rss_source,
            import_archive,
            delete_source,
            reset_learning,
            search_library,
            saved_library,
            set_saved,
            rename_source,
            set_source_paused,
            pick_opml,
            discover_feeds,
            probe_mastodon_instance,
            connect_mastodon,
            export_opml,
            export_backup,
            export_saved_items,
            restore_backup
        ])
        .run(tauri::generate_context!())
        .expect("Web desktop runtime failed to start");
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::connectors::SyncPage;
    use std::sync::Arc;

    fn seed_restore_database(path: &Path, marker: &str) {
        let database = Database::open(path).expect("database");
        database
            .connection_for_test()
            .execute_batch("CREATE TABLE restore_test_marker (value TEXT NOT NULL);")
            .expect("marker table");
        database
            .connection_for_test()
            .execute(
                "INSERT INTO restore_test_marker(value) VALUES(?1)",
                [marker],
            )
            .expect("marker value");
    }

    fn restore_marker(path: &Path) -> String {
        rusqlite::Connection::open(path)
            .expect("open marker database")
            .query_row("SELECT value FROM restore_test_marker", [], |row| {
                row.get(0)
            })
            .expect("marker value")
    }

    fn create_restore_rollback(main: &Path, rollback: &Path) {
        Database::open(main)
            .expect("main database")
            .backup_to(rollback)
            .expect("rollback backup");
    }

    #[test]
    fn original_url_validation_accepts_only_bounded_credential_free_https() {
        let valid = validate_original_url("https://example.com/source?id=7#discussion")
            .expect("credential-free HTTPS URL");
        assert_eq!(valid.as_str(), "https://example.com/source?id=7#discussion");

        for invalid in [
            "",
            "not a URL",
            "http://example.com/source",
            "https://",
            "https://reader@example.com/source",
            "https://reader:secret@example.com/source",
        ] {
            let error = validate_original_url(invalid).expect_err("unsafe URL must be rejected");
            assert_eq!(error.code, "VALIDATION");
        }

        let oversized = format!("https://example.com/{}", "a".repeat(MAX_ORIGINAL_URL_BYTES));
        let error = validate_original_url(&oversized).expect_err("oversized URL must be rejected");
        assert_eq!(error.code, "VALIDATION");
    }

    #[test]
    fn original_url_dispatch_uses_validated_url_and_maps_launcher_failures() {
        let request = OpenOriginalRequest {
            url: "https://example.com/original".into(),
        };
        let mut opened = None;
        open_original_with(&request, |url| {
            opened = Some(url.to_owned());
            Ok::<(), ()>(())
        })
        .expect("open dispatch");
        assert_eq!(opened.as_deref(), Some("https://example.com/original"));

        let error = open_original_with(&request, |_url| Err::<(), _>("launcher unavailable"))
            .expect_err("launcher failure");
        assert_eq!(error.code, "INTERNAL");
    }

    #[test]
    fn renderer_capability_does_not_expose_opener_commands() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/main.json")).expect("capability");
        let permissions = capability["permissions"]
            .as_array()
            .expect("permissions array");
        assert!(permissions.iter().all(|permission| {
            !permission
                .as_str()
                .is_some_and(|name| name.starts_with("opener:"))
        }));
    }

    #[test]
    fn opml_export_is_bounded_escaped_and_round_trips_through_the_local_importer() {
        let rendered = render_opml_export(&[
            (
                "Ada & Bob's feed".into(),
                "https://example.com/feed?a=1&b=2".into(),
            ),
            ("<News>".into(), "https://example.net/atom.xml".into()),
        ])
        .expect("render OPML");
        let text = std::str::from_utf8(&rendered).expect("UTF-8 OPML");
        assert!(text.contains("Ada &amp; Bob&apos;s feed"));
        assert!(text.contains("a=1&amp;b=2"));
        let imported = parse_opml(&rendered).expect("round trip");
        assert_eq!(
            imported
                .into_iter()
                .map(|candidate| (candidate.label, candidate.url))
                .collect::<Vec<_>>(),
            vec![
                (
                    "Ada & Bob's feed".into(),
                    "https://example.com/feed?a=1&b=2".into()
                ),
                ("<News>".into(), "https://example.net/atom.xml".into()),
            ]
        );
        assert!(render_opml_export(&[]).is_err());
    }

    #[test]
    fn saved_item_export_is_versioned_portable_and_omits_database_identifiers() {
        let rendered = render_saved_items_export(
            vec![domain::LibraryItem {
                id: "post-internal-id".into(),
                source_id: "source-internal-id".into(),
                source: "Practical AI Notes".into(),
                author: "Ada".into(),
                title: "A remembered idea".into(),
                excerpt: "A local excerpt with useful context.".into(),
                published_at: "2026-01-02T03:04:05Z".into(),
                canonical_url: Some("https://example.test/idea".into()),
                saved: true,
                source_status: "healthy".into(),
                source_health_detail: "Current runtime state.".into(),
                summary_method: Some("extractive".into()),
                summary_provider: Some("local fallback".into()),
                summary_uncertainty: Some("Source excerpt only.".into()),
            }],
            "2026-01-03T04:05:06Z"
                .parse::<chrono::DateTime<Utc>>()
                .expect("fixed timestamp"),
        )
        .expect("render saved export");
        let value: serde_json::Value = serde_json::from_slice(&rendered).expect("valid JSON");

        assert_eq!(value["format"], "web-saved-items");
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["exportedAt"], "2026-01-03T04:05:06+00:00");
        assert_eq!(value["items"][0]["title"], "A remembered idea");
        assert_eq!(
            value["items"][0]["canonicalUrl"],
            "https://example.test/idea"
        );
        assert!(value["items"][0].get("id").is_none());
        assert!(value["items"][0].get("sourceId").is_none());
        assert!(value["items"][0].get("sourceStatus").is_none());
    }

    #[test]
    fn export_replacement_publishes_a_new_backup_without_an_existing_destination() {
        let directory = tempfile::tempdir().expect("tempdir");
        let temporary = directory.path().join("backup.tmp");
        let destination = directory.path().join("backup.sqlite3");
        std::fs::write(&temporary, b"new backup").expect("temporary backup");

        replace_export_backup(&temporary, &destination).expect("publish backup");

        assert_eq!(
            std::fs::read(&destination).expect("backup contents"),
            b"new backup"
        );
        assert!(!temporary.exists());
    }

    #[test]
    fn export_replacement_swaps_an_existing_backup_only_after_staging_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let temporary = directory.path().join("backup.tmp");
        let destination = directory.path().join("backup.sqlite3");
        std::fs::write(&temporary, b"new backup").expect("temporary backup");
        std::fs::write(&destination, b"previous backup").expect("previous backup");

        replace_export_backup(&temporary, &destination).expect("replace backup");

        assert_eq!(
            std::fs::read(&destination).expect("backup contents"),
            b"new backup"
        );
        assert!(!temporary.exists());
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("directory")
                .count(),
            1,
            "staged prior backup is cleaned after a successful replacement"
        );
    }

    #[test]
    fn failed_export_replacement_restores_the_existing_backup() {
        let directory = tempfile::tempdir().expect("tempdir");
        let temporary = directory.path().join("missing.tmp");
        let destination = directory.path().join("backup.sqlite3");
        std::fs::write(&destination, b"previous backup").expect("previous backup");

        let error = replace_export_backup(&temporary, &destination).expect_err("publish fails");

        assert_eq!(error.code, "VALIDATION");
        assert_eq!(
            std::fs::read(&destination).expect("restored backup"),
            b"previous backup"
        );
    }

    #[test]
    fn malformed_restore_candidate_leaves_main_database_untouched() {
        let directory = tempfile::tempdir().expect("tempdir");
        let main = directory.path().join("web.sqlite3");
        let candidate = directory.path().join("web.restore-malformed.sqlite3");
        seed_restore_database(&main, "original library");
        std::fs::write(&candidate, b"not a SQLite database").expect("malformed candidate");

        let error = validate_restore_candidate(&candidate).expect_err("candidate must fail");
        assert_eq!(error.code, "VALIDATION");
        assert_eq!(restore_marker(&main), "original library");
    }

    #[test]
    fn unsupported_restore_migration_leaves_main_database_untouched() {
        let directory = tempfile::tempdir().expect("tempdir");
        let main = directory.path().join("web.sqlite3");
        let candidate = directory.path().join("web.restore-future.sqlite3");
        seed_restore_database(&main, "original library");
        let future = rusqlite::Connection::open(&candidate).expect("future candidate");
        future
            .execute_batch(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
                 INSERT INTO schema_migrations(version, applied_at) VALUES(999, 0);",
            )
            .expect("future migration");
        drop(future);

        let error = validate_restore_candidate(&candidate).expect_err("future schema must fail");
        assert_eq!(error.code, "VALIDATION");
        assert_eq!(restore_marker(&main), "original library");
    }

    #[test]
    fn failed_restore_rename_reopens_the_rollback_library() {
        let directory = tempfile::tempdir().expect("tempdir");
        let main = directory.path().join("web.sqlite3");
        let candidate = directory.path().join("web.restore-candidate.sqlite3");
        let rollback = directory.path().join("web.restore-rollback.sqlite3");
        seed_restore_database(&main, "original library");
        create_restore_rollback(&main, &rollback);
        seed_restore_database(&candidate, "replacement library");
        // Simulate an external delete after validation but before the replacement rename.
        std::fs::remove_file(&candidate).expect("remove candidate");

        match replace_database_with_candidate(&main, &candidate, &rollback, Database::open)
            .expect("rollback recovery")
        {
            RestoreReplacement::RolledBack(database) => drop(database),
            RestoreReplacement::Replaced(_) => {
                panic!("rename failure must not replace the library")
            }
        }
        assert_eq!(restore_marker(&main), "original library");
        assert!(!rollback.exists());
    }

    #[test]
    fn failed_restore_reopen_reopens_the_rollback_library() {
        let directory = tempfile::tempdir().expect("tempdir");
        let main = directory.path().join("web.sqlite3");
        let candidate = directory.path().join("web.restore-candidate.sqlite3");
        let rollback = directory.path().join("web.restore-rollback.sqlite3");
        seed_restore_database(&main, "original library");
        create_restore_rollback(&main, &rollback);
        seed_restore_database(&candidate, "replacement library");

        match replace_database_with_candidate(&main, &candidate, &rollback, |_| {
            Err(rusqlite::Error::InvalidQuery)
        })
        .expect("rollback recovery")
        {
            RestoreReplacement::RolledBack(database) => drop(database),
            RestoreReplacement::Replaced(_) => {
                panic!("reopen failure must not replace the library")
            }
        }
        assert_eq!(restore_marker(&main), "original library");
        assert!(!rollback.exists());
    }

    #[test]
    fn stale_restore_artifacts_are_cleaned_without_touching_other_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        for name in [
            "web.restore-previous.sqlite3",
            "web.restore-rollback.sqlite3",
        ] {
            std::fs::write(directory.path().join(name), b"stale").expect("stale artifact");
        }
        let unrelated = directory.path().join("my-precious-backup.sqlite3");
        std::fs::write(&unrelated, b"keep").expect("unrelated backup");

        cleanup_stale_restore_artifacts(directory.path()).expect("cleanup");

        assert!(
            !directory
                .path()
                .join("web.restore-previous.sqlite3")
                .exists()
        );
        assert!(
            !directory
                .path()
                .join("web.restore-rollback.sqlite3")
                .exists()
        );
        assert!(unrelated.exists());
    }

    #[test]
    fn successful_restore_replaces_the_library_and_cleans_rollback() {
        let directory = tempfile::tempdir().expect("tempdir");
        let main = directory.path().join("web.sqlite3");
        let candidate = directory.path().join("web.restore-candidate.sqlite3");
        let rollback = directory.path().join("web.restore-rollback.sqlite3");
        seed_restore_database(&main, "original library");
        create_restore_rollback(&main, &rollback);
        seed_restore_database(&candidate, "replacement library");

        match replace_database_with_candidate(&main, &candidate, &rollback, Database::open)
            .expect("replacement")
        {
            RestoreReplacement::Replaced(database) => drop(database),
            RestoreReplacement::RolledBack(_) => panic!("valid replacement must win"),
        }
        assert_eq!(restore_marker(&main), "replacement library");
        assert!(!candidate.exists());
        assert!(!rollback.exists());
    }

    #[test]
    fn bounded_archive_reader_rejects_non_files_and_oversized_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            read_import_file_bounded(directory.path()),
            Err(ImportError::UnreadableFile)
        ));
        let oversized = directory.path().join("oversized.json");
        let file = File::create(&oversized).expect("file");
        file.set_len(MAX_IMPORT_FILE_BYTES + 1)
            .expect("sparse oversized file");
        assert!(matches!(
            read_import_file_bounded(&oversized),
            Err(ImportError::FileTooLarge)
        ));
    }

    #[tokio::test]
    async fn archive_import_core_handles_cancel_replay_reimport_and_sync_gate() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state = AppState::new(directory.path().join("archive-command.sqlite3")).expect("state");
        let canceled_request = ImportArchiveRequest {
            request_id: "archive-cancel".into(),
            platform: ImportPlatform::X,
            label: "Ada archive".into(),
        };
        let canceled = import_archive_with_loader(&state, &canceled_request, || async {
            Ok::<Option<Vec<u8>>, AppError>(None)
        })
        .await
        .expect("cancel is not an error");
        assert_eq!(canceled.status, ImportArchiveStatus::Canceled);
        assert!(canceled.source_id.is_none());
        let canceled_receipts: i64 = state
            .database()
            .expect("database")
            .connection_for_test()
            .query_row(
                "SELECT COUNT(*) FROM request_receipts WHERE request_id='archive-cancel'",
                [],
                |row| row.get(0),
            )
            .expect("receipt count");
        assert_eq!(canceled_receipts, 0);

        let request = ImportArchiveRequest {
            request_id: "archive-import".into(),
            platform: ImportPlatform::X,
            label: "Ada archive".into(),
        };
        let bytes = include_bytes!("../../tests/fixtures/x_tweets_sample.fixture").to_vec();
        let imported = import_archive_with_loader(&state, &request, move || async move {
            Ok::<Option<Vec<u8>>, AppError>(Some(bytes))
        })
        .await
        .expect("import");
        assert_eq!(imported.status, ImportArchiveStatus::Imported);
        assert_eq!(imported.imported_items, 2);
        let source_id = imported.source_id.clone().expect("source id");

        let loader_called = Arc::new(AtomicBool::new(false));
        let replay_called = Arc::clone(&loader_called);
        let replayed = import_archive_with_loader(&state, &request, move || {
            replay_called.store(true, Ordering::SeqCst);
            async { Ok::<Option<Vec<u8>>, AppError>(None) }
        })
        .await
        .expect("replay");
        assert_eq!(replayed.status, ImportArchiveStatus::Replayed);
        assert_eq!(replayed.source_id.as_deref(), Some(source_id.as_str()));
        assert!(!loader_called.load(Ordering::SeqCst));

        let reimport_request = ImportArchiveRequest {
            request_id: "archive-reimport".into(),
            platform: ImportPlatform::X,
            label: "Ada archive".into(),
        };
        let bytes = include_bytes!("../../tests/fixtures/x_tweets_sample.fixture").to_vec();
        let reimported =
            import_archive_with_loader(&state, &reimport_request, move || async move {
                Ok::<Option<Vec<u8>>, AppError>(Some(bytes))
            })
            .await
            .expect("reimport");
        assert_eq!(reimported.source_id.as_deref(), Some(source_id.as_str()));
        assert_eq!(reimported.changed_items, 0);

        let gate = state.sync_gate.lock().await;
        let blocked_called = Arc::new(AtomicBool::new(false));
        let blocked_probe = Arc::clone(&blocked_called);
        let blocked_request = ImportArchiveRequest {
            request_id: "archive-blocked".into(),
            platform: ImportPlatform::Instagram,
            label: "Blocked archive".into(),
        };
        let blocked = import_archive_with_loader(&state, &blocked_request, move || {
            blocked_probe.store(true, Ordering::SeqCst);
            async { Ok::<Option<Vec<u8>>, AppError>(None) }
        })
        .await
        .expect_err("sync gate blocks import");
        assert_eq!(blocked.code, "CONFLICT");
        assert!(!blocked_called.load(Ordering::SeqCst));
        drop(gate);
    }

    #[tokio::test]
    async fn instagram_reimport_reuses_media_identity_and_updates_the_existing_post() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state =
            AppState::new(directory.path().join("instagram-reimport.sqlite3")).expect("state");
        let first_request = ImportArchiveRequest {
            request_id: "instagram-import-first".into(),
            platform: ImportPlatform::Instagram,
            label: "Ada Instagram archive".into(),
        };
        let first_bytes = br#"[
            {
                "title":"Original caption",
                "creation_timestamp":1784000000,
                "media":[
                    {"uri":"media/posts/2026/a.jpg"},
                    {"uri":"media/posts/2026/b.jpg"}
                ]
            }
        ]"#
        .to_vec();
        let first = import_archive_with_loader(&state, &first_request, move || async move {
            Ok::<Option<Vec<u8>>, AppError>(Some(first_bytes))
        })
        .await
        .expect("first Instagram import");
        assert_eq!(first.changed_items, 1);
        assert_eq!(first.imported_items, 1);
        let source_id = first.source_id.expect("source id");

        let edited_request = ImportArchiveRequest {
            request_id: "instagram-import-edited".into(),
            platform: ImportPlatform::Instagram,
            label: "Ada Instagram archive".into(),
        };
        let edited_bytes = br#"[
            {
                "title":"Edited caption",
                "creation_timestamp":1784000000,
                "media":[
                    {"uri":"media\\posts\\2026\\b.jpg"},
                    {"uri":"media/posts/2026/./a.jpg"},
                    {"uri":"media/posts/2026/a.jpg"}
                ]
            }
        ]"#
        .to_vec();
        let edited = import_archive_with_loader(&state, &edited_request, move || async move {
            Ok::<Option<Vec<u8>>, AppError>(Some(edited_bytes))
        })
        .await
        .expect("edited Instagram re-import");
        assert_eq!(edited.source_id.as_deref(), Some(source_id.as_str()));
        assert_eq!(edited.changed_items, 1);

        let stored: (i64, String) = state
            .database()
            .expect("database")
            .connection_for_test()
            .query_row(
                "SELECT COUNT(*), MAX(body_text) FROM posts WHERE source_id=?1",
                [source_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("stored Instagram post");
        assert_eq!(stored, (1, "Edited caption".to_owned()));
    }

    fn stale_disposition(
        database: &Database,
        request_id: &str,
        command: &str,
        payload_hash: &str,
    ) -> RequestDisposition {
        assert_eq!(
            database
                .begin_request(request_id, command, payload_hash)
                .expect("begin request"),
            RequestDisposition::New
        );
        database.age_pending_request_for_test(request_id);
        database
            .begin_request(request_id, command, payload_hash)
            .expect("stale replay")
    }

    #[test]
    fn stale_unknown_add_and_delete_admission_blocks_external_and_database_effects() {
        let mut database = Database::memory().expect("database");
        let add_hash = content_hash("Example\nhttps://example.com/feed");
        let add_disposition =
            stale_disposition(&database, "stale-add", "add_rss_source", &add_hash);
        let sources_before_add = database
            .dashboard(
                fallback_status("test"),
                capabilities::detect_host(&fallback_status("test")),
            )
            .expect("dashboard")
            .sources
            .len();
        let mut feed_requests = 0;
        let mut database_mutations = 0;
        let add_result = (|| -> AppResult<()> {
            if admit_external_command(add_disposition)? == ExternalCommandAdmission::Execute {
                feed_requests += 1;
                database_mutations += 1;
            }
            Ok(())
        })();
        assert_eq!(
            add_result.expect_err("unknown add must fail closed").code,
            "CONFLICT"
        );
        assert_eq!(feed_requests, 0);
        assert_eq!(database_mutations, 0);
        assert_eq!(
            database
                .dashboard(
                    fallback_status("test"),
                    capabilities::detect_host(&fallback_status("test")),
                )
                .expect("dashboard")
                .sources
                .len(),
            sources_before_add
        );

        database
            .begin_request("seed-delete", "add_rss_source", "seed")
            .expect("seed receipt");
        database
            .add_rss_source(
                "seed-delete",
                "Delete me",
                "https://example.com/delete-feed",
                &SyncPage {
                    posts: vec![],
                    effective_url: "https://example.com/delete-feed".into(),
                    etag: None,
                    last_modified: None,
                    not_modified: false,
                },
                vec![],
            )
            .expect("seed source");
        database
            .complete_request("seed-delete")
            .expect("complete seed");
        let source_id = database
            .dashboard(
                fallback_status("test"),
                capabilities::detect_host(&fallback_status("test")),
            )
            .expect("dashboard")
            .sources[0]
            .id
            .clone();
        let delete_hash = content_hash(&source_id);
        let delete_disposition =
            stale_disposition(&database, "stale-delete", "delete_source", &delete_hash);
        let mut vault_deletions = 0;
        let delete_result = (|| -> AppResult<()> {
            if admit_external_command(delete_disposition)? == ExternalCommandAdmission::Execute {
                vault_deletions += 1;
                database_mutations += 1;
            }
            Ok(())
        })();
        assert_eq!(
            delete_result
                .expect_err("unknown delete must fail closed")
                .code,
            "CONFLICT"
        );
        assert_eq!(vault_deletions, 0);
        assert_eq!(database_mutations, 0);
        assert!(
            database
                .dashboard(
                    fallback_status("test"),
                    capabilities::detect_host(&fallback_status("test")),
                )
                .expect("dashboard")
                .sources
                .iter()
                .any(|source| source.id == source_id)
        );
    }

    #[test]
    fn stale_unknown_feedback_settings_and_reset_are_truthful_and_effect_free() {
        let mut database = Database::memory().expect("database");
        assert_eq!(
            database
                .begin_request("seed-local", "add_rss_source", "seed")
                .expect("seed receipt"),
            RequestDisposition::New
        );
        let post = connectors::NormalizedPost {
            remote_id: "remote-local".into(),
            canonical_url: Some("https://example.com/local".into()),
            author: "Local".into(),
            title: "Local".into(),
            body_text: "Local body".into(),
            published_at: Utc::now().timestamp_millis(),
            timestamp_kind: connectors::TimestampKind::Published,
        };
        database
            .add_rss_source(
                "seed-local",
                "Local source",
                "https://example.com/feed",
                &SyncPage {
                    posts: vec![post.clone()],
                    effective_url: "https://example.com/feed".into(),
                    etag: None,
                    last_modified: None,
                    not_modified: false,
                },
                vec![PreparedPost {
                    input_hash: db::summary_input_hash_for(
                        &post,
                        &[],
                        connectors::CommentCompleteness::Unavailable,
                        false,
                    ),
                    post,
                    summary: crate::inference::GroundedSummary {
                        summary: "Summary".into(),
                        comment_overview: "No comments".into(),
                        uncertainty: "Deterministic".into(),
                    },
                    provider: "deterministic".into(),
                    model_id: None,
                    prompt_version: PROMPT_VERSION.into(),
                    summary_method: "extractive_fallback".into(),
                }],
            )
            .expect("seed source");
        database
            .complete_request("seed-local")
            .expect("complete seed");
        database.run_digest("local-digest").expect("digest");
        let item_id = database
            .dashboard(
                fallback_status("test"),
                capabilities::detect_host(&fallback_status("test")),
            )
            .expect("dashboard")
            .items[0]
            .id
            .clone();
        database
            .record_feedback(
                "existing-feedback",
                &item_id,
                &domain::FeedbackSignal::MoreLikeThis,
            )
            .expect("existing feedback");

        let feedback_hash = content_hash(&format!("{item_id}:more_like_this"));
        let feedback_disposition =
            stale_disposition(&database, "stale-feedback", "feedback", &feedback_hash);
        assert_eq!(feedback_disposition, RequestDisposition::Unknown);
        let feedback_error = database
            .record_feedback(
                "stale-feedback",
                &item_id,
                &domain::FeedbackSignal::MoreLikeThis,
            )
            .expect_err("unknown feedback must not report success");
        assert_eq!(feedback_error.code, "CONFLICT");

        let original_settings = database.settings().expect("settings");
        let mut changed_settings = original_settings.clone();
        changed_settings.retention_days = 7;
        let settings_request = UpdateSettingsRequest {
            request_id: "stale-settings".into(),
            settings: changed_settings.clone(),
        };
        let settings_payload = serde_json::to_string(&changed_settings).expect("settings payload");
        assert_eq!(
            stale_disposition(
                &database,
                "stale-settings",
                "update_settings",
                &content_hash(&settings_payload),
            ),
            RequestDisposition::Unknown
        );
        let settings_error = update_settings_core(&mut database, &settings_request)
            .expect_err("unknown settings must not report success");
        assert_eq!(settings_error.code, "CONFLICT");
        assert_eq!(
            database
                .settings()
                .expect("unchanged settings")
                .retention_days,
            original_settings.retention_days
        );

        assert_eq!(
            stale_disposition(
                &database,
                "stale-reset",
                "reset_learning",
                &content_hash("reset-learning-v1"),
            ),
            RequestDisposition::Unknown
        );
        let reset_error = reset_learning_core(
            &mut database,
            &ResetLearningRequest {
                request_id: "stale-reset".into(),
            },
        )
        .expect_err("unknown reset must not report success");
        assert_eq!(reset_error.code, "CONFLICT");
        assert_eq!(
            database
                .dashboard(
                    fallback_status("test"),
                    capabilities::detect_host(&fallback_status("test")),
                )
                .expect("final dashboard")
                .settings
                .feedback_count,
            1
        );
    }

    #[test]
    fn request_admission_policies_are_exhaustive_and_preserve_complete_replays() {
        assert_eq!(
            admit_external_command(RequestDisposition::New).expect("new"),
            ExternalCommandAdmission::Execute
        );
        assert_eq!(
            admit_external_command(RequestDisposition::Complete).expect("complete"),
            ExternalCommandAdmission::ReplayComplete
        );
        assert!(admit_external_command(RequestDisposition::Unknown).is_err());
        assert_eq!(
            admit_local_command(RequestDisposition::New).expect("local new"),
            ExternalCommandAdmission::Execute
        );
        assert_eq!(
            admit_local_command(RequestDisposition::Complete).expect("local complete"),
            ExternalCommandAdmission::ReplayComplete
        );
        assert!(admit_local_command(RequestDisposition::Unknown).is_err());

        let database = Database::memory().expect("database");
        for (request_id, command, payload_hash) in [
            ("complete-add", "add_rss_source", "add-hash"),
            ("complete-delete", "delete_source", "delete-hash"),
        ] {
            assert_eq!(
                database
                    .begin_request(request_id, command, payload_hash)
                    .expect("begin"),
                RequestDisposition::New
            );
            database.complete_request(request_id).expect("complete");
            assert_eq!(
                admit_external_command(
                    database
                        .begin_request(request_id, command, payload_hash)
                        .expect("same-payload replay")
                )
                .expect("known complete"),
                ExternalCommandAdmission::ReplayComplete
            );
            assert!(
                database
                    .begin_request(request_id, command, "different-payload")
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn partial_comment_preparation_uses_the_exact_merged_candidate_end_to_end() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("partial-candidate.sqlite3");
        let state = AppState::new(path.clone()).expect("state");
        let now = Utc::now().timestamp_millis();
        {
            let database = state.database().expect("database");
            database.connection_for_test().execute(
                "INSERT INTO sources(id, connector_kind, account_label, detail, status, config_json, created_at, updated_at, generation)
                 VALUES('candidate-source', 'mastodon', 'Candidate', '', 'healthy', '{}', ?1, ?1, 1)",
                [now],
            ).expect("source");
        }
        let post = connectors::NormalizedPost {
            remote_id: "candidate-post".into(),
            canonical_url: Some("https://example.test/post".into()),
            author: "Author".into(),
            title: "Candidate".into(),
            body_text: "Candidate body.".into(),
            published_at: now,
            timestamp_kind: connectors::TimestampKind::Published,
        };
        let comment = |id: &str, body: &str, position: u32| connectors::NormalizedComment {
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
            id: "candidate-source".into(),
            kind: SourceKind::Mastodon,
            generation: 1,
            config_json: "{}".into(),
            cursor: None,
        };
        let initial = connectors::SyncBatch {
            posts: vec![post.clone()],
            comments: vec![comment("one", "First", 1), comment("two", "Second", 2)],
            comment_scope_post_ids: vec![post.remote_id.clone()],
            cursor: Some("cursor-one".into()),
            page_finality: connectors::PageFinality::Complete,
            comment_completeness: connectors::CommentCompleteness::Complete,
            comments_truncated: false,
            health: connectors::ConnectorHealth {
                state: connectors::ConnectorHealthState::Healthy,
                safe_detail: "Complete".into(),
                retry_at: None,
            },
            rss: None,
        };
        let initial_candidates = state
            .database()
            .expect("database")
            .changed_posts_for_sync_batch_fenced(&source, &initial, None)
            .expect("initial candidates");
        let (initial_prepared, _) = state
            .prepare_posts(&initial_candidates, "", 4)
            .await
            .expect("initial preparation");
        state
            .database()
            .expect("database")
            .ingest_sync_batch_fenced(
                &source,
                "candidate-initial",
                &initial,
                initial_prepared,
                None,
            )
            .expect("initial commit");

        source.cursor = Some("cursor-one".into());
        let partial = connectors::SyncBatch {
            posts: Vec::new(),
            comments: vec![comment("one", "First changed", 1)],
            comment_scope_post_ids: vec![post.remote_id.clone()],
            cursor: Some("cursor-two".into()),
            page_finality: connectors::PageFinality::Partial,
            comment_completeness: connectors::CommentCompleteness::Partial,
            comments_truncated: true,
            health: initial.health.clone(),
            rss: None,
        };
        let candidates = state
            .database()
            .expect("database")
            .changed_posts_for_sync_batch_fenced(&source, &partial, None)
            .expect("partial candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0]
                .comments
                .iter()
                .map(|item| item.body_text.as_str())
                .collect::<Vec<_>>(),
            vec!["First changed", "Second"],
            "the immutable inference candidate contains observed and retained evidence in order"
        );
        let candidate_hash = candidates[0].input_hash.clone();
        let (prepared, attempted_model_items) = state
            .prepare_posts(&candidates, "", 4)
            .await
            .expect("partial preparation");
        assert_eq!(prepared.len(), 1, "one fallback summary was prepared");
        assert_eq!(
            attempted_model_items, 0,
            "fallback does not consume a model slot"
        );
        assert!(prepared[0].summary.comment_overview.contains("2 comments"));
        let stale_prepared = prepared.clone();
        state
            .database()
            .expect("database")
            .ingest_sync_batch_fenced(&source, "candidate-partial", &partial, prepared, None)
            .expect("partial commit");

        source.cursor = Some("cursor-two".into());
        let unchanged = state
            .database()
            .expect("database")
            .changed_posts_for_sync_batch_fenced(&source, &partial, None)
            .expect("unchanged candidates");
        assert!(
            unchanged.is_empty(),
            "unchanged evidence consumes zero preparation budget"
        );
        let (none_prepared, none_attempted) = state
            .prepare_posts(&unchanged, "", 4)
            .await
            .expect("empty preparation");
        assert!(none_prepared.is_empty());
        assert_eq!(none_attempted, 0);
        assert!(
            state
                .database()
                .expect("database")
                .ingest_sync_batch_fenced(
                    &source,
                    "candidate-stale",
                    &partial,
                    stale_prepared,
                    None
                )
                .is_err(),
            "an extra stale prepared candidate is rejected"
        );
        drop(state);

        let reopened = Database::open(&path).expect("reopen");
        let stored_hash: String = reopened.connection_for_test().query_row(
            "SELECT pcs.summary_input_hash FROM post_comment_state pcs JOIN posts p ON p.id=pcs.post_id
             WHERE p.source_id='candidate-source'", [], |row| row.get(0)
        ).expect("stored identity");
        assert_eq!(stored_hash, candidate_hash);
    }

    #[tokio::test(start_paused = true)]
    async fn actual_runner_deadline_uses_injectable_monotonic_time() {
        let task = tokio::spawn(bounded_deadline(
            RUNNER_DEADLINE,
            std::future::pending::<()>(),
        ));
        tokio::task::yield_now().await;
        tokio::time::advance(RUNNER_DEADLINE - Duration::from_millis(1)).await;
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(task.await.expect("deadline task").is_err());
    }

    #[test]
    fn source_and_model_attempt_budgets_fit_inside_renewable_lease() {
        // Four validated HTTP hops at 15 seconds plus four serial model attempts at 30 seconds.
        let bounded_source_ms = (4 * 15 + 4 * 30) * 1_000;
        assert!(bounded_source_ms < RUNNER_LEASE_MS);
        assert!(RUNNER_DEADLINE.as_millis() < RUNNER_LEASE_MS as u128);
    }

    #[test]
    fn model_item_budget_cannot_exceed_the_runner_deadline_envelope() {
        // Mirrors the arithmetic justified in the MAX_MODEL_ITEMS_PER_BATCH doc
        // comment: MAX_SOURCES_PER_RUN sources each worst-casing the RSS
        // transport's REQUEST_TIMEOUT (15s, connectors/rss.rs) before any model
        // call happens, plus MAX_MODEL_ITEMS_PER_BATCH model attempts at
        // OllamaProvider's default per-item timeout (30s, inference.rs), must
        // never exceed RUNNER_DEADLINE. This is a regression test: if either
        // constant changes without re-deriving this budget, it fails loudly
        // instead of silently risking the 8-minute whole-run envelope.
        const RSS_REQUEST_TIMEOUT_SECS: u64 = 15;
        const MODEL_ITEM_TIMEOUT_SECS: u64 = 30;
        let worst_case_ms = (MAX_SOURCES_PER_RUN as u64 * RSS_REQUEST_TIMEOUT_SECS
            + MAX_MODEL_ITEMS_PER_BATCH as u64 * MODEL_ITEM_TIMEOUT_SECS)
            * 1_000;
        assert!(worst_case_ms <= RUNNER_DEADLINE.as_millis() as u64);
    }

    #[derive(Default)]
    struct TestSecretStore {
        values: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
        fail_put: bool,
        fail_delete: bool,
    }

    impl SecretStore for TestSecretStore {
        fn put(&self, reference: &str, secret: &str) -> Result<(), secrets::SecretStoreError> {
            if self.fail_put {
                return Err(secrets::SecretStoreError::Unavailable);
            }
            self.values
                .lock()
                .expect("test vault lock")
                .insert(reference.to_owned(), secret.to_owned());
            Ok(())
        }

        fn get(&self, reference: &str) -> Result<String, secrets::SecretStoreError> {
            self.values
                .lock()
                .expect("test vault lock")
                .get(reference)
                .cloned()
                .ok_or(secrets::SecretStoreError::Unavailable)
        }

        fn delete(&self, reference: &str) -> Result<(), secrets::SecretStoreError> {
            if self.fail_delete {
                return Err(secrets::SecretStoreError::Unavailable);
            }
            self.values
                .lock()
                .expect("test vault lock")
                .remove(reference);
            Ok(())
        }
    }

    fn pending_mastodon_request(
        database: &Database,
        request: &ConnectMastodonRequest,
        secret_ref: &str,
    ) {
        let payload = content_hash(&format!("{}\n{}", request.label, request.instance_url));
        assert_eq!(
            database
                .begin_request(&request.request_id, "connect_mastodon", &payload)
                .expect("begin request"),
            RequestDisposition::New
        );
        database
            .record_pending_vault_cleanup(&request.request_id, secret_ref)
            .expect("pending cleanup");
    }

    #[test]
    fn mastodon_vault_and_source_commit_as_one_recoverable_handoff() {
        let mut database = Database::memory().expect("database");
        let vault = TestSecretStore::default();
        let request = ConnectMastodonRequest {
            request_id: "mastodon-connect-one".into(),
            label: "My Mastodon".into(),
            instance_url: "https://mastodon.social".into(),
        };
        pending_mastodon_request(&database, &request, "mastodon-token-one");

        persist_mastodon_access_token(
            &mut database,
            &vault,
            &request,
            "mastodon-one",
            "mastodon-token-one",
            "token-value",
        )
        .expect("commit connection");

        assert_eq!(
            database
                .connection_for_test()
                .query_row(
                    "SELECT status FROM sources WHERE id='mastodon-one'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("source status"),
            "healthy"
        );
        assert_eq!(
            database
                .connection_for_test()
                .query_row(
                    "SELECT state FROM request_receipts WHERE request_id='mastodon-connect-one'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("completed receipt"),
            "complete"
        );
        assert!(
            database
                .pending_vault_cleanups()
                .expect("pending rows")
                .is_empty()
        );
        assert_eq!(
            vault.get("mastodon-token-one").expect("stored token"),
            "token-value"
        );
    }

    #[test]
    fn indeterminate_vault_write_stays_sealed_with_a_non_secret_cleanup_reference() {
        let mut database = Database::memory().expect("database");
        let vault = TestSecretStore {
            fail_put: true,
            ..Default::default()
        };
        let request = ConnectMastodonRequest {
            request_id: "mastodon-connect-two".into(),
            label: "My Mastodon".into(),
            instance_url: "https://mastodon.social".into(),
        };
        pending_mastodon_request(&database, &request, "mastodon-token-two");

        assert!(
            persist_mastodon_access_token(
                &mut database,
                &vault,
                &request,
                "mastodon-two",
                "mastodon-token-two",
                "token-value",
            )
            .is_err()
        );
        assert_eq!(
            database.pending_vault_cleanups().expect("pending rows"),
            vec![("mastodon-connect-two".into(), "mastodon-token-two".into())]
        );
        assert_eq!(
            database
                .begin_request(
                    "mastodon-connect-two",
                    "connect_mastodon",
                    "anything-different",
                )
                .expect("sealed request"),
            RequestDisposition::Unknown
        );
    }
}
