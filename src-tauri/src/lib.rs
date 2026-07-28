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

use std::path::PathBuf;
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use chrono::{Local, Utc};
use connectors::{
    Connector, ConnectorError, ConnectorSyncRequest, ConnectorTransport, RssConnector, SourceKind,
    SourceSyncSpec, SyncPage, SyncRequest, validate_sync_request,
};
use db::{
    Database, InferenceCandidate, PreparedPost, RequestDisposition, RssSourceSpec,
    SourceSelectionMode, content_hash, validate_id,
};
use domain::{
    AddRssSourceRequest, AppError, AppResult, Dashboard, DeleteSourceRequest, FeedbackRequest,
    ModelState, ResetLearningRequest, RunDigestRequest, SyncSourcesRequest, SyncSourcesResult,
    UndoFeedbackRequest, UpdateSettingsRequest,
};
use inference::{
    DeterministicFallback, InferenceProvider, OllamaProvider, PROMPT_VERSION, SummaryRequest,
    fallback_status,
};
use secrets::{OsSecretStore, SecretStore};
use tauri::{Manager, State};

const OLLAMA_ENDPOINT: &str = "http://127.0.0.1:11434";
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
const MAX_SOURCES_PER_RUN: usize = 20;
const RUNNER_LEASE_MS: i64 = 10 * 60 * 1_000;
const RUNNER_DEADLINE: Duration = Duration::from_secs(8 * 60);

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
    model: tokio::sync::Mutex<Option<OllamaProvider>>,
    sync_gate: tokio::sync::Mutex<()>,
    runner_active: AtomicBool,
    in_flight: AtomicBool,
    secrets: OsSecretStore,
}

impl AppState {
    fn new(database_path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            database: Mutex::new(Database::open(&database_path)?),
            model: tokio::sync::Mutex::new(None),
            sync_gate: tokio::sync::Mutex::new(()),
            runner_active: AtomicBool::new(false),
            in_flight: AtomicBool::new(false),
            secrets: OsSecretStore,
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
            let (sources, capped) = database.rss_sources(mode, now, MAX_SOURCES_PER_RUN)?;
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
            match self
                .sync_one(
                    source,
                    &child_request,
                    &settings.selected_model,
                    model_item_budget,
                    lease.as_ref(),
                )
                .await
            {
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
                    let _ = self.database()?.record_sync_failure_fenced(
                        source,
                        &child_request,
                        message,
                        lease.as_ref(),
                    );
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

#[tauri::command]
async fn get_dashboard(state: State<'_, AppState>) -> AppResult<Dashboard> {
    state.dashboard().await
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

impl AppError {
    fn new_secure_store_failure() -> Self {
        Self::validation(
            "The operating-system credential vault is unavailable, so the source was not deleted. Try again after unlocking the vault.",
        )
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_dashboard,
            run_digest,
            sync_sources,
            record_feedback,
            undo_feedback,
            update_settings,
            add_rss_source,
            delete_source,
            reset_learning
        ])
        .run(tauri::generate_context!())
        .expect("Web desktop runtime failed to start");
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::connectors::SyncPage;

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
}
