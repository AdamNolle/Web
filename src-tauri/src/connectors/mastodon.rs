//! Mastodon authorization-server metadata validation.
//!
//! This module deliberately does not register a client, launch a browser, or exchange a token.
//! It establishes the fail-closed compatibility policy those user-initiated steps must satisfy:
//! a public HTTPS instance must advertise same-origin OAuth endpoints, authorization-code + PKCE
//! S256 support, and only the minimum read capability needed for a calm home-timeline digest.

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;
use zeroize::{Zeroize, Zeroizing};

use super::{
    CommentCompleteness, Connector, ConnectorAvailability, ConnectorDescriptor, ConnectorError,
    ConnectorHealth, ConnectorHealthState, ConnectorSyncRequest, ConnectorTransport,
    NormalizedComment, NormalizedPost, PageFinality, SourceKind, SyncBatch, TimestampKind,
    rss::pinned_public_client,
};

pub const MASTODON_REQUIRED_SCOPES: [&str; 2] = ["read:accounts", "read:statuses"];
const OAUTH_METADATA_PATH: &str = ".well-known/oauth-authorization-server";
const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_METADATA_SCOPES: usize = 100;
const WEB_CLIENT_NAME: &str = "Web Social Digest";
const LOOPBACK_CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const LOOPBACK_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_LOOPBACK_REQUEST_BYTES: usize = 8 * 1024;
const MAX_TIMELINE_BYTES: usize = 1024 * 1024;
const TIMELINE_LIMIT: usize = 40;
const MAX_CONTEXT_POSTS: usize = 5;
const MAX_CONTEXT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MastodonOAuthEndpoints {
    pub instance: Url,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub app_registration_endpoint: Url,
    pub scopes: BTreeSet<String>,
}

/// Read-only official Mastodon home-timeline client. It accepts only a privileged Rust auth
/// value, has no renderer-facing transport, and intentionally returns no discussion evidence
/// until bounded `/context` fetch semantics are implemented.
pub struct MastodonConnector;

impl MastodonConnector {
    pub fn new() -> Result<Self, ConnectorError> {
        Ok(Self)
    }

    async fn fetch_timeline(
        &self,
        request: &ConnectorSyncRequest,
    ) -> Result<SyncBatch, ConnectorError> {
        request.validate()?;
        if request.source.kind != SourceKind::Mastodon
            || !matches!(request.transport, ConnectorTransport::OfficialApi)
        {
            return Err(ConnectorError::InvalidFeed);
        }
        let token = request
            .auth
            .as_ref()
            .ok_or(ConnectorError::AuthRequired)?
            .access_token
            .expose();
        let instance = mastodon_instance_from_config(&request.source.config_json)?;
        let mut endpoint = instance
            .join("api/v1/timelines/home")
            .map_err(|_| ConnectorError::UnsafeUrl)?;
        endpoint
            .query_pairs_mut()
            .append_pair("limit", &TIMELINE_LIMIT.to_string());
        let client = pinned_public_client(&endpoint).await?;
        let response = client
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| ConnectorError::Transient)?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(ConnectorError::RateLimited);
        }
        if response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::FORBIDDEN
        {
            return Err(ConnectorError::AuthRequired);
        }
        if response.status().is_server_error() {
            return Err(ConnectorError::Transient);
        }
        if !response.status().is_success() {
            return Err(ConnectorError::InvalidFeed);
        }
        let body = bounded_timeline_body(response).await?;
        let posts = parse_mastodon_timeline(&body)?;
        let mut comments = Vec::new();
        let mut comment_scope_post_ids = Vec::new();
        for post in posts.iter().take(MAX_CONTEXT_POSTS) {
            if let Ok(mut context) = self.fetch_context(&instance, token, &post.remote_id).await {
                comment_scope_post_ids.push(post.remote_id.clone());
                comments.append(&mut context);
            }
        }
        let partial_context = !comment_scope_post_ids.is_empty();
        let batch = SyncBatch {
            posts,
            comments,
            comment_scope_post_ids,
            cursor: None,
            page_finality: if partial_context {
                PageFinality::Partial
            } else {
                PageFinality::Complete
            },
            comment_completeness: if partial_context {
                CommentCompleteness::Partial
            } else {
                CommentCompleteness::Unavailable
            },
            comments_truncated: false,
            health: ConnectorHealth {
                state: ConnectorHealthState::Healthy,
                safe_detail: if partial_context {
                    "Mastodon home timeline synchronized with bounded partial discussion context."
                } else {
                    "Mastodon home timeline synchronized; discussion context was unavailable."
                }
                .into(),
                retry_at: Some(chrono::Utc::now().timestamp_millis() + 6 * 60 * 60 * 1_000),
            },
            rss: None,
        };
        batch.validate_for(SourceKind::Mastodon)?;
        Ok(batch)
    }

    async fn fetch_context(
        &self,
        instance: &Url,
        token: &str,
        root: &str,
    ) -> Result<Vec<NormalizedComment>, ConnectorError> {
        if root.is_empty() || root.len() > 512 {
            return Err(ConnectorError::InvalidFeed);
        }
        let endpoint = instance
            .join(&format!("api/v1/statuses/{root}/context"))
            .map_err(|_| ConnectorError::UnsafeUrl)?;
        let response = pinned_public_client(&endpoint)
            .await?
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| ConnectorError::Transient)?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(ConnectorError::RateLimited);
        }
        if response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::FORBIDDEN
        {
            return Err(ConnectorError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ConnectorError::Transient);
        }
        parse_mastodon_context(root, &bounded_json_body(response, MAX_CONTEXT_BYTES).await?)
    }
}

#[async_trait::async_trait]
impl Connector for MastodonConnector {
    fn descriptor(&self) -> ConnectorDescriptor {
        ConnectorDescriptor {
            kind: "mastodon".into(),
            label: "Mastodon".into(),
            availability: ConnectorAvailability::ValidationRequired,
            detail: "Read-only home timeline ingestion is implemented but remains behind provider acceptance and native lifecycle gates.".into(),
            unmet_prerequisite: Some("Authorization activation remains unavailable until provider acceptance and packaged lifecycle evidence are complete.".into()),
            read_only: true,
            supports_comments: true,
            requires_oauth: true,
        }
    }

    async fn sync(&self, request: &ConnectorSyncRequest) -> Result<SyncBatch, ConnectorError> {
        self.fetch_timeline(request).await
    }
}

fn mastodon_instance_from_config(config_json: &str) -> Result<Url, ConnectorError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        instance_url: String,
        activation: String,
    }
    let config: Config =
        serde_json::from_str(config_json).map_err(|_| ConnectorError::InvalidFeed)?;
    if config.activation != "foundation_only" {
        return Err(ConnectorError::AuthRequired);
    }
    validate_mastodon_instance_url(&config.instance_url)
}

async fn bounded_timeline_body(response: reqwest::Response) -> Result<String, ConnectorError> {
    bounded_json_body(response, MAX_TIMELINE_BYTES).await
}

async fn bounded_json_body(
    response: reqwest::Response,
    maximum: usize,
) -> Result<String, ConnectorError> {
    if response
        .content_length()
        .is_some_and(|size| size > maximum as u64)
    {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ConnectorError::Transient)?;
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(ConnectorError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| ConnectorError::InvalidFeed)
}

#[derive(Clone, Deserialize)]
struct MastodonTimelineStatus {
    id: String,
    url: Option<String>,
    created_at: String,
    content: String,
    spoiler_text: String,
    account: MastodonTimelineAccount,
    #[serde(default)]
    in_reply_to_id: Option<String>,
    #[serde(default)]
    reblog: Option<Box<MastodonTimelineStatus>>,
}

#[derive(Clone, Deserialize)]
struct MastodonTimelineAccount {
    acct: String,
    display_name: String,
}

#[derive(Deserialize)]
struct MastodonStatusContext {
    #[serde(default)]
    descendants: Vec<MastodonTimelineStatus>,
}

fn parse_mastodon_context(
    root: &str,
    body: &str,
) -> Result<Vec<NormalizedComment>, ConnectorError> {
    let context: MastodonStatusContext =
        serde_json::from_str(body).map_err(|_| ConnectorError::InvalidFeed)?;
    if context.descendants.len() > 50 {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let mut seen = BTreeSet::new();
    let mut comments = Vec::with_capacity(context.descendants.len());
    for status in context.descendants {
        if !seen.insert(status.id.clone()) {
            return Err(ConnectorError::InvalidFeed);
        }
        let published_at = chrono::DateTime::parse_from_rfc3339(&status.created_at)
            .map_err(|_| ConnectorError::InvalidFeed)?
            .timestamp_millis();
        let author = if status.account.display_name.trim().is_empty() {
            status.account.acct.trim().to_owned()
        } else {
            mastodon_plain_text(&status.account.display_name)
        };
        comments.push(NormalizedComment {
            post_remote_id: root.into(),
            remote_id: status.id,
            parent_remote_id: status.in_reply_to_id,
            author,
            body_text: mastodon_plain_text(&status.content),
            published_at,
            depth: 0,
            position: 0,
        });
    }
    comments.sort_by(|a, b| {
        a.published_at
            .cmp(&b.published_at)
            .then(a.remote_id.cmp(&b.remote_id))
    });
    for (position, comment) in comments.iter_mut().enumerate() {
        comment.position = u32::try_from(position).map_err(|_| ConnectorError::ResponseTooLarge)?;
    }
    Ok(comments)
}

fn parse_mastodon_timeline(body: &str) -> Result<Vec<NormalizedPost>, ConnectorError> {
    let statuses: Vec<MastodonTimelineStatus> =
        serde_json::from_str(body).map_err(|_| ConnectorError::InvalidFeed)?;
    if statuses.len() > TIMELINE_LIMIT {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let mut posts = Vec::with_capacity(statuses.len());
    let mut ids = BTreeSet::new();
    for status in statuses {
        let status = status.reblog.as_deref().cloned().unwrap_or(status);
        if !ids.insert(status.id.clone()) {
            return Err(ConnectorError::InvalidFeed);
        }
        let body_text = mastodon_plain_text(&status.content);
        let spoiler = mastodon_plain_text(&status.spoiler_text);
        let title = if spoiler.is_empty() {
            body_text.chars().take(240).collect()
        } else {
            spoiler
        };
        let author = if status.account.display_name.trim().is_empty() {
            status.account.acct.trim().to_owned()
        } else {
            mastodon_plain_text(&status.account.display_name)
        };
        let published_at = chrono::DateTime::parse_from_rfc3339(&status.created_at)
            .map_err(|_| ConnectorError::InvalidFeed)?
            .timestamp_millis();
        posts.push(NormalizedPost {
            remote_id: status.id.clone(),
            canonical_url: status.url.clone().filter(|url| canonical_mastodon_url(url)),
            author,
            title,
            body_text,
            published_at,
            timestamp_kind: TimestampKind::Published,
        });
    }
    Ok(posts)
}

fn canonical_mastodon_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.as_str() == value
    })
}

fn mastodon_plain_text(value: &str) -> String {
    let mut output = String::new();
    let mut tag = false;
    let mut entity = String::new();
    let mut in_entity = false;
    for character in value.chars() {
        match character {
            '<' if !in_entity => tag = true,
            '>' if tag => {
                tag = false;
                output.push(' ');
            }
            '&' if !tag => {
                in_entity = true;
                entity.clear();
            }
            ';' if in_entity => {
                output.push_str(match entity.as_str() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    "nbsp" => " ",
                    _ => " ",
                });
                in_entity = false;
            }
            _ if in_entity => {
                if entity.len() >= 16 {
                    in_entity = false;
                    output.push(' ');
                } else {
                    entity.push(character);
                }
            }
            _ if !tag && !character.is_control() => output.push(character),
            _ => {}
        }
    }
    if in_entity {
        output.push(' ');
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Ephemeral PKCE material for one future native authorization attempt. It is intentionally not
/// serializable, and Debug cannot disclose the verifier. The caller must keep it only for the
/// short-lived callback exchange and zero it when that attempt ends.
#[derive(Clone)]
pub struct MastodonPkce {
    pub state: String,
    verifier: Zeroizing<String>,
    pub challenge: String,
}

impl std::fmt::Debug for MastodonPkce {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MastodonPkce")
            .field("state", &"[REDACTED]")
            .field("verifier", &"[REDACTED]")
            .field("challenge", &"[REDACTED]")
            .finish()
    }
}

impl Drop for MastodonPkce {
    fn drop(&mut self) {
        // Zeroizing already protects this field; make the ownership boundary explicit so no future
        // callback/session refactor can accidentally turn this into ordinary retained text.
        self.verifier.zeroize();
    }
}

/// Public portions of one registration request. This does not contain a secret and is safe to
/// pass to the future pinned HTTP adapter, but is never sent from the renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MastodonRegistrationRequest {
    pub endpoint: Url,
    pub client_name: String,
    pub redirect_uri: String,
    pub scopes: String,
}

/// Result of a successful dynamic registration. The client secret stays in Rust-only zeroizing
/// memory until a future vault handoff; it is intentionally absent from Debug and serialization.
#[derive(Clone)]
pub struct MastodonRegisteredClient {
    pub client_id: String,
    client_secret: Zeroizing<String>,
    pub redirect_uri: Url,
    pub scopes: String,
}

impl std::fmt::Debug for MastodonRegisteredClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MastodonRegisteredClient")
            .field("client_id", &"[REDACTED]")
            .field("client_secret", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .finish()
    }
}

impl Drop for MastodonRegisteredClient {
    fn drop(&mut self) {
        self.client_secret.zeroize();
    }
}

/// A single authorization code accepted by the future loopback listener. It never crosses IPC,
/// diagnostics, or SQLite, and is zeroized after the immediate token exchange attempt.
#[derive(Clone)]
pub struct MastodonAuthorizationCode(Zeroizing<String>);

impl std::fmt::Debug for MastodonAuthorizationCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MastodonAuthorizationCode([REDACTED])")
    }
}

impl Drop for MastodonAuthorizationCode {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A bearer token awaiting the future OS-vault handoff. It cannot be serialized or displayed, and
/// is zeroized if the connection transaction cannot complete.
#[derive(Clone)]
pub struct MastodonAccessToken(Zeroizing<String>);

impl std::fmt::Debug for MastodonAccessToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MastodonAccessToken([REDACTED])")
    }
}

impl Drop for MastodonAccessToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl MastodonAccessToken {
    /// Crate-visible solely for the immediate OS-vault handoff. It must not be serialized,
    /// logged, or retained after the handoff returns.
    #[allow(dead_code)] // The unregistered native orchestration is intentionally not yet callable.
    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

/// A one-shot numeric-loopback callback listener for one future authorization attempt. It binds to
/// an OS-selected port, has no public interface, accepts one bounded HTTP request, and is consumed
/// whether that request is valid or not.
pub struct MastodonLoopbackCallback {
    listener: TcpListener,
    redirect_uri: String,
}

impl std::fmt::Debug for MastodonLoopbackCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MastodonLoopbackCallback")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

impl MastodonLoopbackCallback {
    pub async fn bind() -> Result<Self, ConnectorError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| ConnectorError::Transient)?;
        let port = listener
            .local_addr()
            .map_err(|_| ConnectorError::Transient)?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/oauth/callback");
        validate_loopback_redirect_uri(&redirect_uri)?;
        Ok(Self {
            listener,
            redirect_uri,
        })
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub async fn receive(
        self,
        pkce: &MastodonPkce,
    ) -> Result<MastodonAuthorizationCode, ConnectorError> {
        let (mut stream, _) =
            tokio::time::timeout(LOOPBACK_CALLBACK_TIMEOUT, self.listener.accept())
                .await
                .map_err(|_| ConnectorError::Transient)?
                .map_err(|_| ConnectorError::Transient)?;
        let target = read_loopback_request_target(&mut stream).await?;
        let callback_uri = Url::parse(&self.redirect_uri)
            .and_then(|redirect| redirect.join(&target))
            .map_err(|_| ConnectorError::UnsafeUrl)?;
        let result =
            parse_mastodon_authorization_callback(callback_uri.as_str(), &self.redirect_uri, pkce);
        let response = if result.is_ok() {
            loopback_http_response(
                "200 OK",
                "Web received the authorization response. You can return to it.",
            )
        } else {
            loopback_http_response(
                "400 Bad Request",
                "Web could not accept this authorization response. Return to it.",
            )
        };
        let _ = stream.write_all(response.as_bytes()).await;
        result
    }
}

fn loopback_http_response(status: &str, message: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    )
}

async fn read_loopback_request_target(
    stream: &mut tokio::net::TcpStream,
) -> Result<String, ConnectorError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1_024];
    loop {
        let read = tokio::time::timeout(LOOPBACK_REQUEST_TIMEOUT, stream.read(&mut buffer))
            .await
            .map_err(|_| ConnectorError::Transient)?
            .map_err(|_| ConnectorError::Transient)?;
        if read == 0 {
            return Err(ConnectorError::InvalidFeed);
        }
        if bytes.len().saturating_add(read) > MAX_LOOPBACK_REQUEST_BYTES {
            return Err(ConnectorError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&buffer[..read]);
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let request =
            std::str::from_utf8(&bytes[..header_end]).map_err(|_| ConnectorError::InvalidFeed)?;
        let request_line = request.lines().next().ok_or(ConnectorError::InvalidFeed)?;
        let mut fields = request_line.split_ascii_whitespace();
        let method = fields.next();
        let target = fields.next();
        let version = fields.next();
        if method != Some("GET")
            || version != Some("HTTP/1.1")
            || fields.next().is_some()
            || target.is_none_or(|value| !value.starts_with('/') || value.starts_with("//"))
        {
            return Err(ConnectorError::InvalidFeed);
        }
        return Ok(target.expect("validated target").to_owned());
    }
}

#[derive(Debug, Deserialize)]
struct OAuthAuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    app_registration_endpoint: String,
    #[serde(default)]
    scopes_supported: Vec<String>,
    #[serde(default)]
    response_types_supported: Vec<String>,
    #[serde(default)]
    grant_types_supported: Vec<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

/// Accept only a root HTTPS instance URL. A future network client must additionally perform the
/// same DNS/IP validation and pinning as the RSS transport before any request is sent.
pub fn validate_mastodon_instance_url(value: &str) -> Result<Url, ConnectorError> {
    if value.len() > 2_048 {
        return Err(ConnectorError::UnsafeUrl);
    }
    let url = Url::parse(value.trim()).map_err(|_| ConnectorError::UnsafeUrl)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    Ok(url)
}

fn parse_same_origin_endpoint(instance: &Url, value: &str) -> Result<Url, ConnectorError> {
    let endpoint = Url::parse(value).map_err(|_| ConnectorError::InvalidFeed)?;
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.origin() != instance.origin()
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    Ok(endpoint)
}

fn supported_scopes(scopes: &[String]) -> Result<BTreeSet<String>, ConnectorError> {
    if scopes.is_empty()
        || scopes.len() > MAX_METADATA_SCOPES
        || scopes.iter().any(|scope| {
            scope.is_empty()
                || scope.len() > 100
                || !scope
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
        })
    {
        return Err(ConnectorError::InvalidFeed);
    }
    let scopes = scopes.iter().cloned().collect::<BTreeSet<_>>();
    let supports_minimum_read = scopes.contains("read")
        || MASTODON_REQUIRED_SCOPES
            .iter()
            .all(|required| scopes.contains(*required));
    if !supports_minimum_read {
        return Err(ConnectorError::AuthRequired);
    }
    Ok(scopes)
}

/// Generates a standards-compliant S256 verifier/challenge pair without keeping any provider or
/// account state. UUID v4 draws from the OS CSPRNG; three values provide 384 bits of verifier
/// entropy while the 96 hexadecimal characters remain within RFC 7636's unreserved alphabet.
pub fn new_mastodon_pkce() -> MastodonPkce {
    let verifier = (0..3)
        .map(|_| uuid::Uuid::new_v4().simple().to_string())
        .collect::<String>();
    let challenge = base64url_no_padding(&Sha256::digest(verifier.as_bytes()));
    MastodonPkce {
        state: uuid::Uuid::new_v4().simple().to_string(),
        verifier: Zeroizing::new(verifier),
        challenge,
    }
}

/// Produces one authorization-code request for a dynamically registered client. The redirect is
/// deliberately limited to a numeric IPv4 loopback callback; a future callback listener chooses
/// the actual ephemeral port and owns the matching PKCE state in memory.
pub fn mastodon_authorization_url(
    endpoints: &MastodonOAuthEndpoints,
    client_id: &str,
    redirect_uri: &str,
    pkce: &MastodonPkce,
) -> Result<Url, ConnectorError> {
    let client_id = validate_client_id(client_id)?;
    let redirect_uri = validate_loopback_redirect_uri(redirect_uri)?;
    let scope = selected_read_scope(endpoints)?;
    let mut authorization = endpoints.authorization_endpoint.clone();
    authorization
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri.as_str())
        .append_pair("scope", &scope)
        .append_pair("state", &pkce.state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(authorization)
}

fn validate_client_id(value: &str) -> Result<&str, ConnectorError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 1_024
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ConnectorError::InvalidFeed);
    }
    Ok(value)
}

fn validate_loopback_redirect_uri(value: &str) -> Result<Url, ConnectorError> {
    let redirect = Url::parse(value).map_err(|_| ConnectorError::UnsafeUrl)?;
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("127.0.0.1")
        || redirect.port().is_none()
        || !redirect.username().is_empty()
        || redirect.password().is_some()
        || redirect.path() != "/oauth/callback"
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    Ok(redirect)
}

/// Accepts only one authorization-code callback whose origin, path, and CSRF state exactly match
/// the in-memory authorization attempt. Unknown or duplicated parameters are rejected rather than
/// interpreted, so a future loopback HTTP parser has one narrow and testable success condition.
pub fn parse_mastodon_authorization_callback(
    callback_uri: &str,
    expected_redirect_uri: &str,
    pkce: &MastodonPkce,
) -> Result<MastodonAuthorizationCode, ConnectorError> {
    let expected = validate_loopback_redirect_uri(expected_redirect_uri)?;
    let callback = Url::parse(callback_uri).map_err(|_| ConnectorError::UnsafeUrl)?;
    if callback.scheme() != expected.scheme()
        || callback.host_str() != expected.host_str()
        || callback.port() != expected.port()
        || callback.path() != expected.path()
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.fragment().is_some()
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    let mut code = None;
    let mut state = None;
    for (name, value) in callback.query_pairs() {
        let target = match name.as_ref() {
            "code" => &mut code,
            "state" => &mut state,
            _ => return Err(ConnectorError::InvalidFeed),
        };
        if target.replace(value.into_owned()).is_some() {
            return Err(ConnectorError::InvalidFeed);
        }
    }
    let code = code.ok_or(ConnectorError::AuthRequired)?;
    let state = state.ok_or(ConnectorError::AuthRequired)?;
    if code.is_empty()
        || code.len() > 2_048
        || code
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || !constant_time_equal(&state, &pkce.state)
    {
        return Err(ConnectorError::AuthRequired);
    }
    Ok(MastodonAuthorizationCode(Zeroizing::new(code)))
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn selected_read_scope(endpoints: &MastodonOAuthEndpoints) -> Result<String, ConnectorError> {
    if endpoints.scopes.contains("read") {
        Ok("read".to_owned())
    } else if MASTODON_REQUIRED_SCOPES
        .iter()
        .all(|required| endpoints.scopes.contains(*required))
    {
        Ok(MASTODON_REQUIRED_SCOPES.join(" "))
    } else {
        Err(ConnectorError::AuthRequired)
    }
}

/// Builds the exact dynamic-client-registration form for a compatible instance. The caller must
/// later submit it only through the same public-DNS/IP-pinned transport used for metadata.
pub fn mastodon_registration_request(
    endpoints: &MastodonOAuthEndpoints,
    redirect_uri: &str,
) -> Result<MastodonRegistrationRequest, ConnectorError> {
    let redirect_uri = validate_loopback_redirect_uri(redirect_uri)?;
    Ok(MastodonRegistrationRequest {
        endpoint: endpoints.app_registration_endpoint.clone(),
        client_name: WEB_CLIENT_NAME.to_owned(),
        redirect_uri: redirect_uri.to_string(),
        scopes: selected_read_scope(endpoints)?,
    })
}

#[derive(Deserialize)]
struct MastodonDynamicRegistrationResponse {
    client_id: String,
    client_secret: String,
}

/// Parses a registration response before any secret can reach the vault. This only accepts a
/// bounded JSON object with a portable vault-sized secret and a non-control client identifier.
pub fn parse_mastodon_registered_client(
    request: &MastodonRegistrationRequest,
    body: &str,
) -> Result<MastodonRegisteredClient, ConnectorError> {
    if body.len() > MAX_METADATA_BYTES {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let response: MastodonDynamicRegistrationResponse =
        serde_json::from_str(body).map_err(|_| ConnectorError::InvalidFeed)?;
    let client_id = validate_client_id(&response.client_id)?.to_owned();
    if response.client_secret.is_empty()
        || response.client_secret.len() > 2_048
        || response.client_secret.chars().any(char::is_control)
    {
        return Err(ConnectorError::InvalidFeed);
    }
    Ok(MastodonRegisteredClient {
        client_id,
        client_secret: Zeroizing::new(response.client_secret),
        redirect_uri: validate_loopback_redirect_uri(&request.redirect_uri)?,
        scopes: request.scopes.clone(),
    })
}

/// Sends a dynamically generated registration form only through the same proxy-free, DNS/IP
/// pinned transport that validated the instance metadata. This function has no Tauri command and
/// no UI call site yet; a future explicit connection flow must own its response, vault handoff,
/// and compensating cleanup on every later failure.
pub async fn register_mastodon_client(
    request: &MastodonRegistrationRequest,
) -> Result<MastodonRegisteredClient, ConnectorError> {
    validate_mastodon_registration_request(request)?;
    let client = pinned_public_client(&request.endpoint).await?;
    let response = client
        .post(request.endpoint.clone())
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(mastodon_registration_form_body(request))
        .send()
        .await
        .map_err(|_| ConnectorError::Transient)?;
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return Err(ConnectorError::RateLimited);
    }
    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return Err(ConnectorError::AuthRequired);
    }
    if response.status().is_server_error() {
        return Err(ConnectorError::Transient);
    }
    if !response.status().is_success() {
        return Err(ConnectorError::InvalidFeed);
    }
    let body = bounded_mastodon_response_body(response).await?;
    parse_mastodon_registered_client(request, &body)
}

/// Exchanges one accepted authorization code through the verified token endpoint. It remains
/// Rust-only and uncalled until the loopback listener, browser flow, and all-or-nothing vault/source
/// transaction exist.
pub async fn exchange_mastodon_authorization_code(
    endpoints: &MastodonOAuthEndpoints,
    client: &MastodonRegisteredClient,
    code: MastodonAuthorizationCode,
    pkce: &MastodonPkce,
) -> Result<MastodonAccessToken, ConnectorError> {
    validate_https_provider_endpoint(&endpoints.token_endpoint)?;
    if client.client_id.is_empty()
        || client.client_id.len() > 1_024
        || client
            .client_id
            .chars()
            .any(|character| character.is_control())
        || client.client_secret.is_empty()
        || client.client_secret.len() > 2_048
        || client.client_secret.chars().any(char::is_control)
        || !matches!(
            client.scopes.as_str(),
            "read" | "read:accounts read:statuses"
        )
    {
        return Err(ConnectorError::InvalidFeed);
    }
    validate_loopback_redirect_uri(client.redirect_uri.as_str())?;
    let client_transport = pinned_public_client(&endpoints.token_endpoint).await?;
    let response = client_transport
        .post(endpoints.token_endpoint.clone())
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(mastodon_token_exchange_form_body(client, &code, pkce))
        .send()
        .await
        .map_err(|_| ConnectorError::Transient)?;
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return Err(ConnectorError::RateLimited);
    }
    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        return Err(ConnectorError::AuthRequired);
    }
    if response.status().is_server_error() {
        return Err(ConnectorError::Transient);
    }
    if !response.status().is_success() {
        return Err(ConnectorError::InvalidFeed);
    }
    let body = bounded_mastodon_response_body(response).await?;
    parse_mastodon_access_token(&body, &client.scopes)
}

fn mastodon_token_exchange_form_body(
    client: &MastodonRegisteredClient,
    code: &MastodonAuthorizationCode,
    pkce: &MastodonPkce,
) -> String {
    let mut encoded = url::form_urlencoded::Serializer::new(String::new());
    encoded.append_pair("grant_type", "authorization_code");
    encoded.append_pair("code", code.0.as_str());
    encoded.append_pair("client_id", &client.client_id);
    encoded.append_pair("client_secret", client.client_secret.as_str());
    encoded.append_pair("redirect_uri", client.redirect_uri.as_str());
    encoded.append_pair("code_verifier", pkce.verifier.as_str());
    encoded.finish()
}

#[derive(Deserialize)]
struct MastodonTokenResponse {
    access_token: String,
    token_type: String,
    #[serde(default)]
    scope: String,
}

fn parse_mastodon_access_token(
    body: &str,
    requested_scopes: &str,
) -> Result<MastodonAccessToken, ConnectorError> {
    if body.len() > MAX_METADATA_BYTES {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let response: MastodonTokenResponse =
        serde_json::from_str(body).map_err(|_| ConnectorError::InvalidFeed)?;
    if !response.token_type.eq_ignore_ascii_case("bearer")
        || response.access_token.is_empty()
        || response.access_token.len() > 2_048
        || response
            .access_token
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || (!response.scope.is_empty() && response.scope != requested_scopes)
    {
        return Err(ConnectorError::AuthRequired);
    }
    Ok(MastodonAccessToken(Zeroizing::new(response.access_token)))
}

async fn bounded_mastodon_response_body(
    response: reqwest::Response,
) -> Result<String, ConnectorError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_METADATA_BYTES as u64)
    {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ConnectorError::Transient)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return Err(ConnectorError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| ConnectorError::InvalidFeed)
}

fn mastodon_registration_form_body(request: &MastodonRegistrationRequest) -> String {
    let mut encoded = url::form_urlencoded::Serializer::new(String::new());
    encoded.append_pair("client_name", &request.client_name);
    encoded.append_pair("redirect_uris", &request.redirect_uri);
    encoded.append_pair("scopes", &request.scopes);
    encoded.finish()
}

fn validate_mastodon_registration_request(
    request: &MastodonRegistrationRequest,
) -> Result<(), ConnectorError> {
    if request.client_name != WEB_CLIENT_NAME
        || validate_https_provider_endpoint(&request.endpoint).is_err()
        || !matches!(
            request.scopes.as_str(),
            "read" | "read:accounts read:statuses"
        )
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    validate_loopback_redirect_uri(&request.redirect_uri)?;
    Ok(())
}

fn validate_https_provider_endpoint(endpoint: &Url) -> Result<(), ConnectorError> {
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.as_str().len() > 2_048
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    Ok(())
}

fn base64url_no_padding(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[((value >> 18) & 0x3f) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 0x3f) as usize] as char);
        if chunk.len() >= 2 {
            encoded.push(ALPHABET[((value >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() == 3 {
            encoded.push(ALPHABET[(value & 0x3f) as usize] as char);
        }
    }
    encoded
}

/// Validate the public metadata obtained from
/// `/.well-known/oauth-authorization-server` for a previously validated instance URL.
pub fn validate_mastodon_oauth_metadata(
    instance: &Url,
    body: &str,
) -> Result<MastodonOAuthEndpoints, ConnectorError> {
    let instance = validate_mastodon_instance_url(instance.as_str())?;
    if body.len() > MAX_METADATA_BYTES {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let metadata: OAuthAuthorizationServerMetadata =
        serde_json::from_str(body).map_err(|_| ConnectorError::InvalidFeed)?;
    let issuer = parse_same_origin_endpoint(&instance, &metadata.issuer)?;
    if issuer.path() != "/" {
        return Err(ConnectorError::UnsafeUrl);
    }
    if !metadata
        .response_types_supported
        .iter()
        .any(|value| value == "code")
        || !metadata
            .grant_types_supported
            .iter()
            .any(|value| value == "authorization_code")
        || !metadata
            .code_challenge_methods_supported
            .iter()
            .any(|value| value == "S256")
    {
        return Err(ConnectorError::AuthRequired);
    }
    Ok(MastodonOAuthEndpoints {
        instance,
        authorization_endpoint: parse_same_origin_endpoint(
            &issuer,
            &metadata.authorization_endpoint,
        )?,
        token_endpoint: parse_same_origin_endpoint(&issuer, &metadata.token_endpoint)?,
        app_registration_endpoint: parse_same_origin_endpoint(
            &issuer,
            &metadata.app_registration_endpoint,
        )?,
        scopes: supported_scopes(&metadata.scopes_supported)?,
    })
}

/// Checks one user-supplied instance's OAuth metadata through the shared public-DNS/IP-pinned
/// transport. It reads at most 64 KiB from the fixed same-origin discovery address, follows no
/// redirects, and returns only validated public endpoint information. It does not persist data,
/// register an OAuth client, launch a browser, or exchange/store a credential.
pub async fn probe_mastodon_instance(
    value: &str,
) -> Result<MastodonOAuthEndpoints, ConnectorError> {
    let instance = validate_mastodon_instance_url(value)?;
    let metadata_url = instance
        .join(OAUTH_METADATA_PATH)
        .map_err(|_| ConnectorError::UnsafeUrl)?;
    let client = pinned_public_client(&metadata_url).await?;
    let response = client
        .get(metadata_url)
        .send()
        .await
        .map_err(|_| ConnectorError::Transient)?;
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return Err(ConnectorError::RateLimited);
    }
    if !response.status().is_success() {
        return Err(ConnectorError::InvalidFeed);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_METADATA_BYTES as u64)
    {
        return Err(ConnectorError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ConnectorError::Transient)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return Err(ConnectorError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = std::str::from_utf8(&bytes).map_err(|_| ConnectorError::InvalidFeed)?;
    validate_mastodon_oauth_metadata(&instance, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    const INSTANCE: &str = "https://social.example/";
    const VALID_METADATA: &str = r#"{
      "issuer":"https://social.example/",
      "authorization_endpoint":"https://social.example/oauth/authorize",
      "token_endpoint":"https://social.example/oauth/token",
      "app_registration_endpoint":"https://social.example/api/v1/apps",
      "scopes_supported":["read:accounts","read:statuses"],
      "response_types_supported":["code"],
      "grant_types_supported":["authorization_code"],
      "code_challenge_methods_supported":["S256"]
    }"#;

    #[test]
    fn metadata_requires_same_origin_pkce_and_minimum_read_scopes() {
        let instance = validate_mastodon_instance_url(INSTANCE).expect("instance");
        let endpoints =
            validate_mastodon_oauth_metadata(&instance, VALID_METADATA).expect("metadata");
        assert_eq!(
            endpoints.authorization_endpoint.as_str(),
            "https://social.example/oauth/authorize"
        );
        assert!(
            MASTODON_REQUIRED_SCOPES
                .iter()
                .all(|scope| endpoints.scopes.contains(*scope))
        );

        let without_pkce = VALID_METADATA.replace("\"S256\"", "\"plain\"");
        assert!(validate_mastodon_oauth_metadata(&instance, &without_pkce).is_err());
        let foreign_token = VALID_METADATA.replace(
            "https://social.example/oauth/token",
            "https://attacker.example/oauth/token",
        );
        assert!(validate_mastodon_oauth_metadata(&instance, &foreign_token).is_err());
        let write_only =
            VALID_METADATA.replace("\"read:accounts\",\"read:statuses\"", "\"write:statuses\"");
        assert!(validate_mastodon_oauth_metadata(&instance, &write_only).is_err());
        let too_many_scopes = format!(
            r#"{{"issuer":"https://social.example/","authorization_endpoint":"https://social.example/oauth/authorize","token_endpoint":"https://social.example/oauth/token","app_registration_endpoint":"https://social.example/api/v1/apps","scopes_supported":[{}],"response_types_supported":["code"],"grant_types_supported":["authorization_code"],"code_challenge_methods_supported":["S256"]}}"#,
            (0..=MAX_METADATA_SCOPES)
                .map(|index| format!("\"scope-{index}\""))
                .collect::<Vec<_>>()
                .join(","),
        );
        assert!(validate_mastodon_oauth_metadata(&instance, &too_many_scopes).is_err());
    }

    #[test]
    fn instance_url_rejects_credentials_paths_and_non_https_origins() {
        for value in [
            "http://social.example/",
            "https://user:secret@social.example/",
            "https://social.example/tenant",
            "https://social.example/?next=https://attacker.example/",
        ] {
            assert!(validate_mastodon_instance_url(value).is_err(), "{value}");
        }
    }

    #[test]
    fn metadata_endpoint_is_fixed_under_the_validated_instance_origin() {
        let instance = validate_mastodon_instance_url(INSTANCE).expect("instance");
        let metadata = instance
            .join(OAUTH_METADATA_PATH)
            .expect("fixed metadata path");
        assert_eq!(
            metadata.as_str(),
            "https://social.example/.well-known/oauth-authorization-server"
        );
        assert_eq!(metadata.origin(), instance.origin());
    }

    #[test]
    fn pkce_uses_rfc7636_s256_encoding_and_redacts_the_verifier() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            base64url_no_padding(&Sha256::digest(verifier.as_bytes())),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let pkce = new_mastodon_pkce();
        assert_eq!(pkce.state.len(), 32);
        assert_eq!(pkce.verifier.len(), 96);
        assert_eq!(pkce.challenge.len(), 43);
        assert!(pkce.verifier.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let debug = format!("{pkce:?}");
        assert!(!debug.contains(pkce.verifier.as_str()));
        assert!(!debug.contains(&pkce.state));
        assert!(!debug.contains(&pkce.challenge));
    }

    #[test]
    fn authorization_url_uses_only_pkce_code_flow_and_numeric_loopback_callback() {
        let instance = validate_mastodon_instance_url(INSTANCE).expect("instance");
        let endpoints =
            validate_mastodon_oauth_metadata(&instance, VALID_METADATA).expect("metadata");
        let pkce = new_mastodon_pkce();
        let authorization = mastodon_authorization_url(
            &endpoints,
            "registered-client",
            "http://127.0.0.1:49271/oauth/callback",
            &pkce,
        )
        .expect("authorization url");
        assert_eq!(authorization.origin(), instance.origin());
        let pairs = authorization.query_pairs().collect::<BTreeSet<_>>();
        for (name, value) in [
            ("response_type", "code"),
            ("client_id", "registered-client"),
            ("redirect_uri", "http://127.0.0.1:49271/oauth/callback"),
            ("scope", "read:accounts read:statuses"),
            ("code_challenge_method", "S256"),
        ] {
            assert!(
                pairs.contains(&(name.into(), value.into())),
                "missing {name}"
            );
        }
        assert!(pairs.contains(&("state".into(), pkce.state.clone().into())));
        assert!(pairs.contains(&("code_challenge".into(), pkce.challenge.clone().into())));

        for invalid_redirect in [
            "https://127.0.0.1:49271/oauth/callback",
            "http://localhost:49271/oauth/callback",
            "http://127.0.0.1:49271/other",
            "http://127.0.0.1/oauth/callback",
        ] {
            assert!(
                mastodon_authorization_url(
                    &endpoints,
                    "registered-client",
                    invalid_redirect,
                    &pkce
                )
                .is_err(),
                "{invalid_redirect}"
            );
        }
    }

    #[test]
    fn dynamic_registration_contract_has_only_read_scope_and_redacts_client_secret() {
        let instance = validate_mastodon_instance_url(INSTANCE).expect("instance");
        let endpoints =
            validate_mastodon_oauth_metadata(&instance, VALID_METADATA).expect("metadata");
        let request =
            mastodon_registration_request(&endpoints, "http://127.0.0.1:49271/oauth/callback")
                .expect("registration request");
        assert_eq!(request.endpoint, endpoints.app_registration_endpoint);
        assert_eq!(request.client_name, WEB_CLIENT_NAME);
        assert_eq!(request.scopes, "read:accounts read:statuses");
        assert!(validate_mastodon_registration_request(&request).is_ok());
        let form_body = mastodon_registration_form_body(&request);
        let form = url::form_urlencoded::parse(form_body.as_bytes()).collect::<BTreeSet<_>>();
        assert!(form.contains(&("client_name".into(), WEB_CLIENT_NAME.into())));
        assert!(form.contains(&("redirect_uris".into(), request.redirect_uri.clone().into())));
        assert!(form.contains(&("scopes".into(), request.scopes.clone().into())));
        let mut malformed_request = request.clone();
        malformed_request.endpoint = Url::parse("http://social.example/api/v1/apps").expect("url");
        assert!(validate_mastodon_registration_request(&malformed_request).is_err());

        let client = parse_mastodon_registered_client(
            &request,
            r#"{"client_id":"opaque-client","client_secret":"sensitive-secret"}"#,
        )
        .expect("registered client");
        assert_eq!(client.client_id, "opaque-client");
        assert_eq!(client.redirect_uri.as_str(), request.redirect_uri);
        let debug = format!("{client:?}");
        assert!(!debug.contains("opaque-client"));
        assert!(!debug.contains("sensitive-secret"));

        for body in [
            r#"{}"#,
            r#"{"client_id":"client id","client_secret":"s"}"#,
            r#"{"client_id":"client","client_secret":""}"#,
        ] {
            assert!(
                parse_mastodon_registered_client(&request, body).is_err(),
                "{body}"
            );
        }
    }

    #[test]
    fn callback_accepts_exact_loopback_code_and_state_only() {
        let pkce = new_mastodon_pkce();
        let redirect = "http://127.0.0.1:49271/oauth/callback";
        let callback = format!("{redirect}?code=opaque-code&state={}", pkce.state);
        let code = parse_mastodon_authorization_callback(&callback, redirect, &pkce)
            .expect("exact callback");
        assert_eq!(code.0.as_str(), "opaque-code");
        assert!(!format!("{code:?}").contains("opaque-code"));

        for callback in [
            format!("{redirect}?code=opaque-code&state=wrong-state"),
            format!(
                "{redirect}?code=opaque-code&state={}&code=again",
                pkce.state
            ),
            format!(
                "{redirect}?code=opaque-code&state={}&error=access_denied",
                pkce.state
            ),
            format!(
                "http://127.0.0.1:49272/oauth/callback?code=opaque-code&state={}",
                pkce.state
            ),
            format!("{redirect}?state={}", pkce.state),
        ] {
            assert!(
                parse_mastodon_authorization_callback(&callback, redirect, &pkce).is_err(),
                "{callback}"
            );
        }
    }

    #[test]
    fn token_exchange_contract_binds_code_verifier_and_redacts_access_token() {
        let instance = validate_mastodon_instance_url(INSTANCE).expect("instance");
        let endpoints =
            validate_mastodon_oauth_metadata(&instance, VALID_METADATA).expect("metadata");
        let request =
            mastodon_registration_request(&endpoints, "http://127.0.0.1:49271/oauth/callback")
                .expect("request");
        let client = parse_mastodon_registered_client(
            &request,
            r#"{"client_id":"client","client_secret":"secret"}"#,
        )
        .expect("client");
        let code = MastodonAuthorizationCode(Zeroizing::new("authorization-code".into()));
        let pkce = new_mastodon_pkce();
        let form_body = mastodon_token_exchange_form_body(&client, &code, &pkce);
        let form = url::form_urlencoded::parse(form_body.as_bytes()).collect::<BTreeSet<_>>();
        for (name, value) in [
            ("grant_type", "authorization_code"),
            ("code", "authorization-code"),
            ("client_id", "client"),
            ("client_secret", "secret"),
            ("redirect_uri", "http://127.0.0.1:49271/oauth/callback"),
        ] {
            assert!(
                form.contains(&(name.into(), value.into())),
                "missing {name}"
            );
        }
        assert!(form.contains(&("code_verifier".into(), pkce.verifier.as_str().into())));

        let token = parse_mastodon_access_token(
            r#"{"access_token":"sensitive-token","token_type":"Bearer","scope":"read:accounts read:statuses"}"#,
            &client.scopes,
        )
        .expect("token");
        assert_eq!(token.0.as_str(), "sensitive-token");
        assert!(!format!("{token:?}").contains("sensitive-token"));
        assert!(parse_mastodon_access_token(
            r#"{"access_token":"token","token_type":"mac","scope":"read:accounts read:statuses"}"#,
            &client.scopes,
        )
        .is_err());
        assert!(
            parse_mastodon_access_token(
                r#"{"access_token":"token","token_type":"Bearer","scope":"write"}"#,
                &client.scopes,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn loopback_listener_accepts_one_valid_callback_and_returns_a_plain_response() {
        let callback = MastodonLoopbackCallback::bind()
            .await
            .expect("bind loopback");
        let redirect = callback.redirect_uri().to_owned();
        let port = Url::parse(&redirect)
            .expect("redirect url")
            .port()
            .expect("loopback port");
        let pkce = new_mastodon_pkce();
        let state = pkce.state.clone();
        let receive = tokio::spawn(async move { callback.receive(&pkce).await });
        let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect loopback");
        stream
            .write_all(
                format!(
                    "GET /oauth/callback?code=one-time-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .expect("write callback");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("read response");
        let code = receive
            .await
            .expect("listener task")
            .expect("valid callback");
        assert_eq!(code.0.as_str(), "one-time-code");
        assert!(
            std::str::from_utf8(&response)
                .expect("plain response")
                .starts_with("HTTP/1.1 200 OK\r\n")
        );
        let response = loopback_http_response("200 OK", "ok");
        assert!(response.contains("Content-Length: 2\r\n"));
    }

    #[test]
    fn timeline_parser_keeps_only_bounded_inert_post_text() {
        let posts = parse_mastodon_timeline(
            r#"[
              {"id":"timeline-one","url":"https://mastodon.social/@ada/1","created_at":"2026-08-08T12:00:00.000Z","content":"<p>Hello <strong>world</strong> &amp; friends<script>ignored tag</script></p>","spoiler_text":"","account":{"acct":"ada","display_name":"Ada"}},
              {"id":"boost-wrapper","url":"https://mastodon.social/@reader/2","created_at":"2026-08-08T12:01:00.000Z","content":"ignored","spoiler_text":"","account":{"acct":"reader","display_name":"Reader"},"reblog":{"id":"timeline-two","url":"https://mastodon.social/@grace/2","created_at":"2026-08-08T12:01:00.000Z","content":"<p>Second status</p>","spoiler_text":"Content warning","account":{"acct":"grace","display_name":"Grace"}}}
            ]"#,
        )
        .expect("timeline");
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].author, "Ada");
        assert_eq!(posts[0].body_text, "Hello world & friends ignored tag");
        assert_eq!(posts[1].remote_id, "timeline-two");
        assert_eq!(posts[1].title, "Content warning");
        assert_eq!(posts[1].timestamp_kind, TimestampKind::Published);
    }

    #[test]
    fn timeline_parser_rejects_duplicate_or_unbounded_statuses() {
        let duplicate = r#"[
          {"id":"same","created_at":"2026-08-08T12:00:00Z","content":"one","spoiler_text":"","account":{"acct":"a","display_name":""}},
          {"id":"same","created_at":"2026-08-08T12:01:00Z","content":"two","spoiler_text":"","account":{"acct":"b","display_name":""}}
        ]"#;
        assert!(parse_mastodon_timeline(duplicate).is_err());
        let many = (0..=TIMELINE_LIMIT)
            .map(|index| format!(r#"{{"id":"{index}","created_at":"2026-08-08T12:00:00Z","content":"one","spoiler_text":"","account":{{"acct":"a","display_name":""}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_mastodon_timeline(&format!("[{many}]")).is_err());
    }

    #[test]
    fn context_parser_keeps_only_descendants_as_partial_comment_evidence() {
        let comments = parse_mastodon_context(
            "root-status",
            r#"{"descendants":[
              {"id":"reply-later","created_at":"2026-08-08T12:02:00Z","content":"<p>Later</p>","spoiler_text":"","account":{"acct":"later","display_name":""},"in_reply_to_id":"reply-first"},
              {"id":"reply-first","created_at":"2026-08-08T12:01:00Z","content":"<p>First &amp; safe</p>","spoiler_text":"","account":{"acct":"first","display_name":"First"},"in_reply_to_id":"root-status"}
            ]}"#,
        )
        .expect("context");
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].remote_id, "reply-first");
        assert_eq!(comments[0].post_remote_id, "root-status");
        assert_eq!(comments[0].body_text, "First & safe");
        assert_eq!(comments[1].parent_remote_id.as_deref(), Some("reply-first"));
        assert_eq!(comments[0].position, 0);
        assert_eq!(comments[1].position, 1);
    }
}
