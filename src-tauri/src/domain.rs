use serde::{Deserialize, Serialize};

use crate::connectors::{ConnectorDescriptor, export_import::ImportPlatform};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub privacy_epoch: u64,
    pub edition: Edition,
    pub items: Vec<DigestItem>,
    pub trends: Vec<Trend>,
    pub sources: Vec<Source>,
    pub activity: Vec<Activity>,
    pub settings: Settings,
    pub model: ModelStatus,
    pub host: HostCapabilities,
    pub runner: RunnerStatus,
    pub connectors: Vec<ConnectorDescriptor>,
    pub history: Vec<EditionSummary>,
    pub library: LibraryStats,
    pub since_last_edition: SinceLastEdition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edition {
    pub id: String,
    pub label: String,
    pub generated_at: String,
    pub next_edition_at: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditionSummary {
    pub id: String,
    pub label: String,
    pub generated_at: String,
    pub summary: String,
    pub item_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditionDetail {
    pub edition: Edition,
    pub items: Vec<DigestItem>,
    pub trends: Vec<Trend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub saved_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SinceLastEdition {
    pub new_items: i64,
    pub new_sources: i64,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub id: String,
    pub source_id: String,
    pub source: String,
    pub author: String,
    pub title: String,
    pub excerpt: String,
    pub published_at: String,
    pub canonical_url: Option<String>,
    pub saved: bool,
    pub source_status: String,
    pub source_health_detail: String,
    pub summary_method: Option<String>,
    pub summary_provider: Option<String>,
    pub summary_uncertainty: Option<String>,
}

/// A portable, person-readable representation of explicitly saved items. Database identifiers and
/// transient source-health diagnostics intentionally stay local to avoid turning an export into an
/// internal database dump.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLibraryExport {
    pub format: String,
    pub schema_version: u8,
    pub exported_at: String,
    pub items: Vec<SavedLibraryExportItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLibraryExportItem {
    pub source: String,
    pub author: String,
    pub title: String,
    pub excerpt: String,
    pub published_at: String,
    pub canonical_url: Option<String>,
    pub summary_method: Option<String>,
    pub summary_provider: Option<String>,
    pub summary_uncertainty: Option<String>,
}

impl SavedLibraryExport {
    pub fn new(exported_at: String, items: Vec<LibraryItem>) -> Self {
        Self {
            format: "web-saved-items".to_owned(),
            schema_version: 1,
            exported_at,
            items: items
                .into_iter()
                .map(|item| SavedLibraryExportItem {
                    source: item.source,
                    author: item.author,
                    title: item.title,
                    excerpt: item.excerpt,
                    published_at: item.published_at,
                    canonical_url: item.canonical_url,
                    summary_method: item.summary_method,
                    summary_provider: item.summary_provider,
                    summary_uncertainty: item.summary_uncertainty,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestItem {
    pub id: String,
    pub source_id: String,
    pub source: String,
    pub author: String,
    pub title: String,
    pub summary: String,
    pub comment_overview: String,
    pub summary_method: String,
    pub summary_provider: String,
    pub summary_uncertainty: String,
    pub published_at: String,
    pub published_time_kind: TimestampKind,
    pub reason: String,
    pub topic: String,
    pub importance: f64,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub source: String,
    pub author: String,
    pub published_at: String,
    pub timestamp_kind: TimestampKind,
    pub excerpt: String,
    pub canonical_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimestampKind {
    Published,
    Updated,
    Fetched,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trend {
    pub id: String,
    pub label: String,
    pub summary: String,
    pub source_count: i64,
    pub confidence: String,
    pub method: TrendMethod,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrendMethod {
    Fixture,
    Lexical,
    Embedding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub detail: String,
    pub status: String,
    pub health_detail: String,
    pub comments_status: String,
    pub comments_truncated: bool,
    pub sync_finality: String,
    pub last_sync: String,
    pub next_sync: Option<String>,
    pub item_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub message: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub schedule_enabled: bool,
    pub schedule_hour: u8,
    pub quiet_hours_start: u8,
    pub quiet_hours_end: u8,
    pub retention_days: u16,
    pub remote_media: bool,
    pub local_only: bool,
    pub feedback_count: i64,
    #[serde(default)]
    pub selected_model: String,
    /// When true, disables learned ranking entirely: every digest item is
    /// ordered purely chronologically and labeled accordingly, regardless of
    /// accumulated feedback. `#[serde(default)]` so settings JSON persisted
    /// before this field existed deserializes to `false` (learned ranking
    /// active), matching the existing round-trip pattern used by
    /// `selected_model`.
    #[serde(default)]
    pub ranking_paused: bool,
    #[serde(default = "default_edition_size")]
    pub edition_size: u8,
    #[serde(default)]
    pub close_to_tray: bool,
}

const fn default_edition_size() -> u8 {
    12
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schedule_enabled: false,
            schedule_hour: 8,
            quiet_hours_start: 21,
            quiet_hours_end: 7,
            retention_days: 30,
            remote_media: false,
            local_only: true,
            feedback_count: 0,
            selected_model: String::new(),
            ranking_paused: false,
            edition_size: default_edition_size(),
            close_to_tray: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    pub os: String,
    pub arch: String,
    pub total_memory_gb: f64,
    pub available_memory_gb: f64,
    pub logical_cpu_count: usize,
    pub gpu: CapabilityStatus,
    pub battery: CapabilityStatus,
    pub metered_network: CapabilityStatus,
    pub local_runtime: CapabilityStatus,
    pub recommended_profile: AdaptiveModelProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityStatus {
    pub state: CapabilityState,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Unknown,
    Available,
    Unavailable,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptiveModelProfile {
    pub id: AdaptiveProfileId,
    pub title: String,
    pub generation_model: String,
    pub embedding_model: String,
    pub context_window: u32,
    pub max_concurrent_requests: u8,
    pub rationale: String,
    pub requires_explicit_download: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AdaptiveProfileId {
    CpuBasic,
    Balanced,
    Performance,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    Checking,
    Unknown,
    RuntimeUnavailable,
    ModelMissing,
    Incompatible,
    DetectedUnverified,
    Ready,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub provider: String,
    pub state: ModelState,
    pub model: Option<String>,
    pub digest: Option<String>,
    pub size_bytes: Option<u64>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    pub runtime_version: Option<String>,
    pub structured_output: bool,
    pub endpoint: String,
    pub fallback_available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerStatus {
    pub active: bool,
    pub in_flight: bool,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub next_scheduled_at: Option<String>,
    pub last_outcome: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    ManualOverride,
    ResidentDue,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncFinality {
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub mode: SyncMode,
    pub finality: SyncFinality,
    pub changed_sources: usize,
    pub unchanged_sources: usize,
    pub failed_sources: usize,
    pub changed_items: usize,
    pub source_limit_reached: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSourcesResult {
    pub dashboard: Dashboard,
    pub outcome: SyncOutcome,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunDigestRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncSourcesRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackRequest {
    pub request_id: String,
    pub item_id: String,
    pub signal: FeedbackSignal,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackSignal {
    MoreLikeThis,
    LessLikeThis,
    NotRelevant,
    MuteSource,
}

impl FeedbackSignal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MoreLikeThis => "more_like_this",
            Self::LessLikeThis => "less_like_this",
            Self::NotRelevant => "not_relevant",
            Self::MuteSource => "mute_source",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoFeedbackRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSettingsRequest {
    pub request_id: String,
    pub settings: Settings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteSourceRequest {
    pub request_id: String,
    pub source_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddRssSourceRequest {
    pub request_id: String,
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenOriginalRequest {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportArchiveRequest {
    pub request_id: String,
    pub platform: ImportPlatform,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportArchiveStatus {
    Canceled,
    Imported,
    Replayed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportArchiveResult {
    pub status: ImportArchiveStatus,
    pub source_id: Option<String>,
    pub imported_items: usize,
    pub skipped_items: usize,
    pub changed_items: usize,
    pub dashboard: Dashboard,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetLearningRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetSavedRequest {
    pub request_id: String,
    pub post_id: String,
    pub saved: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchLibraryRequest {
    pub query: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameSourceRequest {
    pub request_id: String,
    pub source_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetSourcePausedRequest {
    pub request_id: String,
    pub source_id: String,
    pub paused: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncSourceRequest {
    pub request_id: String,
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpmlCandidate {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverFeedsRequest {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeMastodonInstanceRequest {
    pub instance_url: String,
}

/// Native-only connection input. There is intentionally no renderer transport method or invoke
/// registration yet: a stored authorization remains paused until read-only timeline ingestion and
/// its deletion/rollback acceptance coverage are ready.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectMastodonRequest {
    pub request_id: String,
    pub label: String,
    pub instance_url: String,
}

/// A compatibility receipt, not an account connection. Keeping this separate from Source prevents
/// a successful preflight check from looking like social data or a stored authorization.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MastodonProbeResult {
    pub instance_url: String,
    pub supported_scopes: Vec<String>,
    pub connection_enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreBackupRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub correlation_id: String,
}

impl AppError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new("VALIDATION", message, false)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("NOT_FOUND", message, false)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new("CONFLICT", message, false)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new("UNAVAILABLE", message, true)
    }

    pub fn internal() -> Self {
        Self::new(
            "INTERNAL",
            "The local operation failed safely. Your existing edition is unchanged.",
            true,
        )
    }

    fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            correlation_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
