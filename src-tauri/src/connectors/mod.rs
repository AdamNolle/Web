mod rss;

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::Zeroizing;

pub use rss::{RssConnector, validate_public_feed_url, validate_sync_request};

pub const MAX_ITEMS_PER_SYNC: usize = 100;
pub const MAX_COMMENTS_PER_POST: usize = 50;
pub const MAX_COMMENTS_PER_SYNC: usize = 500;
pub const MAX_COMMENT_BODY_BYTES: usize = 4_000;
pub const MAX_COMMENT_BYTES_PER_SYNC: usize = 256 * 1024;
pub const MAX_COMMENT_DEPTH: u16 = 8;
pub const MAX_CONFIG_BYTES: usize = 8 * 1024;
pub const MAX_CURSOR_BYTES: usize = 2 * 1024;
pub const MAX_POST_BODY_BYTES: usize = 32 * 1024;
pub const MAX_POST_TITLE_BYTES: usize = 1_000;
pub const MAX_REMOTE_ID_BYTES: usize = 512;
pub const MAX_AUTHOR_BYTES: usize = 512;
pub const MAX_HEALTH_DETAIL_BYTES: usize = 512;
pub const MAX_CANONICAL_URL_BYTES: usize = 2_048;
const RSS_MAX_REMOTE_ID_CHARS: usize = 1_000;
const RSS_MAX_TITLE_CHARS: usize = 500;
const RSS_MAX_BODY_CHARS: usize = 20_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAvailability {
    Available,
    ValidationRequired,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorDescriptor {
    pub kind: String,
    pub label: String,
    pub availability: ConnectorAvailability,
    pub detail: String,
    pub unmet_prerequisite: Option<String>,
    pub read_only: bool,
    pub supports_comments: bool,
    pub requires_oauth: bool,
}

pub fn connector_descriptors() -> Vec<ConnectorDescriptor> {
    vec![
        ConnectorDescriptor {
            kind: "rss".into(),
            label: "RSS / Atom".into(),
            availability: ConnectorAvailability::Available,
            detail: "Official feed URLs; read-only and available now.".into(),
            unmet_prerequisite: None,
            read_only: true,
            supports_comments: false,
            requires_oauth: false,
        },
        ConnectorDescriptor {
            kind: "mastodon".into(),
            label: "Mastodon".into(),
            availability: ConnectorAvailability::ValidationRequired,
            detail: "Official read-only home timeline and bounded context are planned.".into(),
            unmet_prerequisite: Some(
                "Instance OAuth compatibility and provider policy review are required before connection is enabled."
                    .into(),
            ),
            read_only: true,
            supports_comments: true,
            requires_oauth: true,
        },
        ConnectorDescriptor {
            kind: "bluesky".into(),
            label: "Bluesky".into(),
            availability: ConnectorAvailability::Blocked,
            detail: "Official read-only timeline and bounded thread support are planned.".into(),
            unmet_prerequisite: Some(
                "A public HTTPS client-metadata/policy origin, owned native callback, and exact permission validation are required."
                    .into(),
            ),
            read_only: true,
            supports_comments: true,
            requires_oauth: true,
        },
    ]
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Rss,
    Mastodon,
    Bluesky,
}

#[derive(Debug, Clone)]
pub struct SourceSyncSpec {
    pub id: String,
    pub kind: SourceKind,
    pub generation: i64,
    pub config_json: String,
    pub cursor: Option<String>,
}

/// Authentication is resolved by privileged Rust immediately before connector use. The secret is
/// deliberately non-serializable, redacted from Debug output, and zeroed when its last clone drops.
#[derive(Clone)]
pub struct SecretValue(Zeroizing<Vec<u8>>);

impl SecretValue {
    pub fn new(value: String) -> Result<Self, ConnectorError> {
        if value.is_empty() || value.len() > 2_048 {
            return Err(ConnectorError::InvalidFeed);
        }
        Ok(Self(Zeroizing::new(value.into_bytes())))
    }

    pub(crate) fn expose(&self) -> &str {
        std::str::from_utf8(self.0.as_slice()).expect("secret constructed from UTF-8")
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone)]
pub struct ConnectorAuth {
    pub access_token: SecretValue,
}

impl fmt::Debug for ConnectorAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectorAuth")
            .field("access_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum ConnectorTransport {
    Rss(SyncRequest),
    OfficialApi,
}

#[derive(Debug, Clone)]
pub struct ConnectorSyncRequest {
    pub source: SourceSyncSpec,
    pub auth: Option<ConnectorAuth>,
    pub transport: ConnectorTransport,
}

impl ConnectorSyncRequest {
    pub fn validate(&self) -> Result<(), ConnectorError> {
        validate_source_sync_spec(&self.source)?;
        if self
            .auth
            .as_ref()
            .is_some_and(|auth| auth.access_token.expose().is_empty())
        {
            return Err(ConnectorError::InvalidFeed);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SyncRequest {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimestampKind {
    Published,
    Updated,
    Fetched,
}

impl TimestampKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Updated => "updated",
            Self::Fetched => "fetched",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedPost {
    pub remote_id: String,
    pub canonical_url: Option<String>,
    pub author: String,
    pub title: String,
    pub body_text: String,
    pub published_at: i64,
    pub timestamp_kind: TimestampKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedComment {
    pub post_remote_id: String,
    pub remote_id: String,
    pub parent_remote_id: Option<String>,
    pub author: String,
    pub body_text: String,
    pub published_at: i64,
    pub depth: u16,
    pub position: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommentCompleteness {
    Unavailable,
    Complete,
    Partial,
}

impl CommentCompleteness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Complete => "complete",
            Self::Partial => "partial",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PageFinality {
    Complete,
    Partial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorHealthState {
    Healthy,
    RateLimited,
    AuthRequired,
    Transient,
    Paused,
}

impl ConnectorHealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::RateLimited => "rate_limited",
            Self::AuthRequired => "auth_required",
            Self::Transient => "transient",
            Self::Paused => "paused",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectorHealth {
    pub state: ConnectorHealthState,
    pub safe_detail: String,
    pub retry_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RssRepresentation {
    pub effective_url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub not_modified: bool,
}

#[derive(Debug, Clone)]
pub struct SyncBatch {
    pub posts: Vec<NormalizedPost>,
    pub comments: Vec<NormalizedComment>,
    /// Exact post remote IDs whose comment snapshot/status this page describes. Complete,
    /// untruncated batches replace comments only inside this scope; partial pages only upsert.
    pub comment_scope_post_ids: Vec<String>,
    pub cursor: Option<String>,
    pub page_finality: PageFinality,
    pub comment_completeness: CommentCompleteness,
    pub comments_truncated: bool,
    pub health: ConnectorHealth,
    pub rss: Option<RssRepresentation>,
}

impl SyncBatch {
    pub fn validate(&self) -> Result<(), ConnectorError> {
        self.validate_for(SourceKind::Mastodon)
    }

    pub fn validate_for(&self, kind: SourceKind) -> Result<(), ConnectorError> {
        if self.posts.len() > MAX_ITEMS_PER_SYNC
            || self.comments.len() > MAX_COMMENTS_PER_SYNC
            || self.comment_scope_post_ids.len() > MAX_ITEMS_PER_SYNC
            || self
                .cursor
                .as_ref()
                .is_some_and(|value| value.len() > MAX_CURSOR_BYTES || !plain_text(value, false))
            || self.health.safe_detail.len() > MAX_HEALTH_DETAIL_BYTES
            || !plain_text(&self.health.safe_detail, true)
        {
            return Err(ConnectorError::ResponseTooLarge);
        }
        let mut scope = std::collections::HashSet::new();
        for remote_id in &self.comment_scope_post_ids {
            if !valid_remote_id(remote_id, kind) || !scope.insert(remote_id.as_str()) {
                return Err(ConnectorError::InvalidFeed);
            }
        }
        for post in &self.posts {
            let size_ok = match kind {
                SourceKind::Rss => {
                    post.remote_id.chars().count() <= RSS_MAX_REMOTE_ID_CHARS
                        && post.title.chars().count() <= RSS_MAX_TITLE_CHARS
                        && post.body_text.chars().count() <= RSS_MAX_BODY_CHARS
                }
                SourceKind::Mastodon | SourceKind::Bluesky => {
                    post.remote_id.len() <= MAX_REMOTE_ID_BYTES
                        && post.title.len() <= MAX_POST_TITLE_BYTES
                        && post.body_text.len() <= MAX_POST_BODY_BYTES
                }
            };
            let author_size_ok = if kind == SourceKind::Rss {
                post.author.chars().count() <= 200
            } else {
                post.author.len() <= MAX_AUTHOR_BYTES
            };
            if !size_ok
                || !valid_remote_id(&post.remote_id, kind)
                || !author_size_ok
                || !plain_text(&post.author, false)
                || !plain_text(&post.title, true)
                || !plain_text(&post.body_text, true)
                || post
                    .canonical_url
                    .as_ref()
                    .is_some_and(|url| !valid_canonical_url(url))
            {
                return Err(ConnectorError::ResponseTooLarge);
            }
            if self.comment_completeness != CommentCompleteness::Unavailable
                && !scope.contains(post.remote_id.as_str())
            {
                return Err(ConnectorError::InvalidFeed);
            }
        }
        let mut per_post = std::collections::HashMap::<&str, usize>::new();
        let mut comment_ids = std::collections::HashSet::new();
        let mut body_bytes = 0_usize;
        for comment in &self.comments {
            body_bytes = body_bytes.saturating_add(comment.body_text.len());
            let count = per_post.entry(comment.post_remote_id.as_str()).or_default();
            *count += 1;
            if *count > MAX_COMMENTS_PER_POST
                || body_bytes > MAX_COMMENT_BYTES_PER_SYNC
                || comment.body_text.len() > MAX_COMMENT_BODY_BYTES
                || comment.depth > MAX_COMMENT_DEPTH
                || !valid_remote_id(&comment.remote_id, kind)
                || !comment_ids.insert(comment.remote_id.as_str())
                || !valid_remote_id(&comment.post_remote_id, kind)
                || comment
                    .parent_remote_id
                    .as_ref()
                    .is_some_and(|value| !valid_remote_id(value, kind))
                || comment.author.len() > MAX_AUTHOR_BYTES
                || !plain_text(&comment.author, false)
                || !bounded_plain_text(&comment.body_text)
                || !scope.contains(comment.post_remote_id.as_str())
            {
                return Err(ConnectorError::ResponseTooLarge);
            }
        }
        match self.comment_completeness {
            CommentCompleteness::Unavailable
                if !self.comments.is_empty()
                    || !self.comment_scope_post_ids.is_empty()
                    || self.comments_truncated =>
            {
                return Err(ConnectorError::InvalidFeed);
            }
            CommentCompleteness::Complete if self.comments_truncated => {
                return Err(ConnectorError::InvalidFeed);
            }
            CommentCompleteness::Complete | CommentCompleteness::Partial
                if self.comment_scope_post_ids.is_empty() =>
            {
                return Err(ConnectorError::InvalidFeed);
            }
            CommentCompleteness::Partial if self.page_finality != PageFinality::Partial => {
                return Err(ConnectorError::InvalidFeed);
            }
            _ if self.comments_truncated && self.page_finality != PageFinality::Partial => {
                return Err(ConnectorError::InvalidFeed);
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn validate_source_sync_spec(spec: &SourceSyncSpec) -> Result<(), ConnectorError> {
    let config = serde_json::from_str::<serde_json::Value>(&spec.config_json).ok();
    if spec.id.is_empty()
        || spec.id.len() > 128
        || !spec
            .id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':'))
        || spec.generation < 1
        || spec.config_json.len() > MAX_CONFIG_BYTES
        || spec
            .cursor
            .as_ref()
            .is_some_and(|value| value.len() > MAX_CURSOR_BYTES || !plain_text(value, false))
        || config
            .as_ref()
            .is_none_or(|value| !valid_config_value(value, 0))
    {
        return Err(ConnectorError::InvalidFeed);
    }
    Ok(())
}

fn valid_config_value(value: &serde_json::Value, depth: u8) -> bool {
    if depth > 12 {
        return false;
    }
    match value {
        serde_json::Value::String(value) => value.len() <= 2_048 && plain_text(value, true),
        serde_json::Value::Array(values) => values
            .iter()
            .all(|value| valid_config_value(value, depth + 1)),
        serde_json::Value::Object(values) => values.iter().all(|(key, value)| {
            key.len() <= 128 && plain_text(key, false) && valid_config_value(value, depth + 1)
        }),
        _ => true,
    }
}

fn valid_remote_id(value: &str, kind: SourceKind) -> bool {
    let max = if kind == SourceKind::Rss {
        RSS_MAX_REMOTE_ID_CHARS
    } else {
        MAX_REMOTE_ID_BYTES
    };
    !value.is_empty()
        && if kind == SourceKind::Rss {
            value.chars().count() <= max
        } else {
            value.len() <= max
        }
        && plain_text(value, false)
}

fn valid_canonical_url(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_CANONICAL_URL_BYTES || value.trim() != value {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.as_str() == value
}

fn plain_text(value: &str, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.trim() == value
        && value
            .chars()
            .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
}

fn bounded_plain_text(value: &str) -> bool {
    plain_text(value, false)
}

/// Frozen RSS persistence representation. Kept separate so HTTP validators remain RSS-only and
/// representation-bound while the connector trait itself is provider-neutral.
#[derive(Debug, Clone)]
pub struct SyncPage {
    pub posts: Vec<NormalizedPost>,
    pub effective_url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub not_modified: bool,
}

impl TryFrom<SyncBatch> for SyncPage {
    type Error = ConnectorError;

    fn try_from(batch: SyncBatch) -> Result<Self, Self::Error> {
        batch.validate_for(SourceKind::Rss)?;
        if batch.comment_completeness != CommentCompleteness::Unavailable
            || !batch.comments.is_empty()
            || batch.comments_truncated
        {
            return Err(ConnectorError::InvalidFeed);
        }
        let rss = batch.rss.ok_or(ConnectorError::InvalidFeed)?;
        Ok(Self {
            posts: batch.posts,
            effective_url: rss.effective_url,
            etag: rss.etag,
            last_modified: rss.last_modified,
            not_modified: rss.not_modified,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("the source URL is not allowed")]
    UnsafeUrl,
    #[error("the source returned too much data")]
    ResponseTooLarge,
    #[error("the source did not return a valid feed")]
    InvalidFeed,
    #[error("the source rate-limited this request")]
    RateLimited,
    #[error("the source requires renewed authorization")]
    AuthRequired,
    #[error("the source request timed out or failed")]
    Transient,
}

#[async_trait]
pub trait Connector: Send + Sync {
    fn descriptor(&self) -> ConnectorDescriptor;
    async fn sync(&self, request: &ConnectorSyncRequest) -> Result<SyncBatch, ConnectorError>;
}

/// Contract for any future browser-assisted connector. Implementations must use a separate,
/// sandboxed process and a visibly user-authorized profile. These policy fields are invariants,
/// not toggles. No such connector ships in the first release.
#[derive(Debug, Clone, Serialize)]
pub struct BrowserConnectorPolicy {
    pub user_initiated_auth_only: bool,
    pub read_only: bool,
    pub import_session_cookies: bool,
    pub evade_automation_controls: bool,
    pub rotate_network_identity: bool,
    pub bypass_captcha: bool,
}

impl Default for BrowserConnectorPolicy {
    fn default() -> Self {
        Self {
            user_initiated_auth_only: true,
            read_only: true,
            import_session_cookies: false,
            evade_automation_controls: false,
            rotate_network_identity: false,
            bypass_captcha: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(post: &str, id: &str, body: &str) -> NormalizedComment {
        NormalizedComment {
            post_remote_id: post.into(),
            remote_id: id.into(),
            parent_remote_id: None,
            author: "Reader".into(),
            body_text: body.into(),
            published_at: 1,
            depth: 1,
            position: 0,
        }
    }

    #[test]
    fn browser_policy_cannot_enable_evasion_by_default() {
        let policy = BrowserConnectorPolicy::default();
        assert!(policy.user_initiated_auth_only && policy.read_only);
        assert!(!policy.import_session_cookies);
        assert!(!policy.evade_automation_controls);
        assert!(!policy.rotate_network_identity);
        assert!(!policy.bypass_captcha);
    }

    #[test]
    fn descriptors_are_read_only_and_social_connectors_are_not_available() {
        let descriptors = connector_descriptors();
        assert_eq!(descriptors.len(), 3);
        assert!(descriptors.iter().all(|item| item.read_only));
        assert_eq!(
            descriptors[0].availability,
            ConnectorAvailability::Available
        );
        assert!(
            descriptors[1..]
                .iter()
                .all(|item| item.availability != ConnectorAvailability::Available)
        );
    }

    #[test]
    fn connector_secret_debug_is_redacted_and_never_serializable() {
        let canary = "token-canary-never-print";
        let auth = ConnectorAuth {
            access_token: SecretValue::new(canary.into()).expect("secret"),
        };
        let rendered = format!("{auth:?}");
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains(canary));
    }

    #[test]
    fn comment_and_batch_bounds_fail_closed() {
        let base = SyncBatch {
            posts: Vec::new(),
            comments: vec![comment("post", "comment", "plain")],
            comment_scope_post_ids: vec!["post".into()],
            cursor: None,
            page_finality: PageFinality::Complete,
            comment_completeness: CommentCompleteness::Complete,
            comments_truncated: false,
            health: ConnectorHealth {
                state: ConnectorHealthState::Healthy,
                safe_detail: "Ready".into(),
                retry_at: None,
            },
            rss: None,
        };
        assert!(base.validate().is_ok());
        let mut deep = base.clone();
        deep.comments[0].depth = MAX_COMMENT_DEPTH + 1;
        assert!(deep.validate().is_err());
        let mut oversized = base.clone();
        oversized.comments[0].body_text = "x".repeat(MAX_COMMENT_BODY_BYTES + 1);
        assert!(oversized.validate().is_err());
        let mut unavailable = base.clone();
        unavailable.comment_completeness = CommentCompleteness::Unavailable;
        assert!(unavailable.validate().is_err());
        let mut invalid_url = base.clone();
        invalid_url.posts = vec![NormalizedPost {
            remote_id: "post".into(),
            canonical_url: Some("javascript:alert(1)".into()),
            author: "Author".into(),
            title: "Title".into(),
            body_text: "Body".into(),
            published_at: 1,
            timestamp_kind: TimestampKind::Published,
        }];
        assert!(invalid_url.validate().is_err());
        let mut duplicate = base.clone();
        let mut duplicate_comment = duplicate.comments[0].clone();
        duplicate_comment.post_remote_id = "other".into();
        duplicate.comment_scope_post_ids.push("other".into());
        duplicate.comments.push(duplicate_comment);
        assert!(duplicate.validate().is_err());
        let mut parent = base;
        parent.comments[0].parent_remote_id = Some("x".repeat(MAX_REMOTE_ID_BYTES + 1));
        assert!(parent.validate().is_err());

        let invalid_spec = SourceSyncSpec {
            id: "bad id".into(),
            kind: SourceKind::Mastodon,
            generation: 1,
            config_json: "{}".into(),
            cursor: None,
        };
        assert!(validate_source_sync_spec(&invalid_spec).is_err());
    }

    #[test]
    fn page_and_comment_finality_matrix_is_exhaustive() {
        for page in [PageFinality::Complete, PageFinality::Partial] {
            for completeness in [
                CommentCompleteness::Unavailable,
                CommentCompleteness::Complete,
                CommentCompleteness::Partial,
            ] {
                for truncated in [false, true] {
                    for with_scope in [false, true] {
                        let batch = SyncBatch {
                            posts: Vec::new(),
                            comments: if with_scope
                                && completeness != CommentCompleteness::Unavailable
                            {
                                vec![comment("post", "comment", "plain")]
                            } else {
                                Vec::new()
                            },
                            comment_scope_post_ids: if with_scope {
                                vec!["post".into()]
                            } else {
                                Vec::new()
                            },
                            cursor: None,
                            page_finality: page,
                            comment_completeness: completeness,
                            comments_truncated: truncated,
                            health: ConnectorHealth {
                                state: ConnectorHealthState::Healthy,
                                safe_detail: "Ready".into(),
                                retry_at: None,
                            },
                            rss: None,
                        };
                        let expected = match completeness {
                            CommentCompleteness::Unavailable => !with_scope && !truncated,
                            CommentCompleteness::Complete => with_scope && !truncated,
                            CommentCompleteness::Partial => {
                                with_scope && page == PageFinality::Partial
                            }
                        } && (!truncated || page == PageFinality::Partial);
                        assert_eq!(
                            batch.validate().is_ok(),
                            expected,
                            "page={page:?} completeness={completeness:?} truncated={truncated} scope={with_scope}"
                        );
                    }
                }
            }
        }
    }
}
