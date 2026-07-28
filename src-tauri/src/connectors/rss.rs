use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use async_trait::async_trait;
use feed_rs::parser;
use futures_util::StreamExt;
use reqwest::header::{ETAG, HeaderMap, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, LOCATION};
use reqwest::{Client, StatusCode};
use tokio::net::lookup_host;
use url::{Host, Url};

use super::{
    CommentCompleteness, Connector, ConnectorAvailability, ConnectorDescriptor, ConnectorError,
    ConnectorHealth, ConnectorHealthState, ConnectorSyncRequest, ConnectorTransport,
    MAX_ITEMS_PER_SYNC, NormalizedPost, PageFinality, RssRepresentation, SourceKind, SyncBatch,
    SyncPage, SyncRequest, TimestampKind,
};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub struct RssConnector;

impl RssConnector {
    pub fn new() -> Result<Self, ConnectorError> {
        Ok(Self)
    }

    async fn pinned_client(url: &Url) -> Result<Client, ConnectorError> {
        validate_public_feed_url(url.as_str())?;
        let mut builder = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(Duration::from_secs(5))
            .user_agent("WebSocialDigest/0.1 (+local read-only feed client)");
        if let Some(Host::Domain(host)) = url.host() {
            let port = url
                .port_or_known_default()
                .ok_or(ConnectorError::UnsafeUrl)?;
            let addresses = lookup_host((host, port))
                .await
                .map_err(|_| ConnectorError::UnsafeUrl)?
                .collect::<Vec<_>>();
            validate_resolved_addresses(&addresses)?;
            builder = builder.resolve_to_addrs(host, &addresses);
        }
        builder.build().map_err(|_| ConnectorError::Transient)
    }

    async fn fetch(&self, request: &SyncRequest) -> Result<SyncPage, ConnectorError> {
        validate_sync_request(request)?;
        let mut current = Url::parse(&request.url).map_err(|_| ConnectorError::UnsafeUrl)?;
        for redirect_count in 0..=MAX_REDIRECTS {
            let client = Self::pinned_client(&current).await?;
            let mut builder = client.get(current.clone());
            if redirect_count == 0 {
                if let Some(etag) = &request.etag {
                    builder = builder.header(IF_NONE_MATCH, etag);
                }
                if let Some(last_modified) = &request.last_modified {
                    builder = builder.header(IF_MODIFIED_SINCE, last_modified);
                }
            }
            let response = builder
                .send()
                .await
                .map_err(|_| ConnectorError::Transient)?;
            if response.status() == StatusCode::NOT_MODIFIED {
                // Validators are intentionally never forwarded across a redirect boundary.
                // A redirected 304 therefore cannot prove the requested representation unchanged.
                if redirect_count != 0 {
                    return Err(ConnectorError::InvalidFeed);
                }
                let (etag, last_modified) = response_validators(
                    response.headers(),
                    request.etag.as_deref(),
                    request.last_modified.as_deref(),
                );
                return Ok(SyncPage {
                    posts: Vec::new(),
                    effective_url: current.to_string(),
                    etag,
                    last_modified,
                    not_modified: true,
                });
            }
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                return Err(ConnectorError::RateLimited);
            }
            if response.status().is_redirection() {
                if redirect_count == MAX_REDIRECTS {
                    return Err(ConnectorError::UnsafeUrl);
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(ConnectorError::UnsafeUrl)?;
                let next = current
                    .join(location)
                    .map_err(|_| ConnectorError::UnsafeUrl)?;
                if !redirect_allowed(&current, &next) {
                    return Err(ConnectorError::UnsafeUrl);
                }
                current = next;
                continue;
            }
            if !response.status().is_success() {
                return Err(ConnectorError::Transient);
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
            {
                return Err(ConnectorError::ResponseTooLarge);
            }
            let (etag, last_modified) = response_validators(response.headers(), None, None);
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| ConnectorError::Transient)?;
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(ConnectorError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            let posts = parse_feed(
                &bytes,
                current.as_str(),
                chrono::Utc::now().timestamp_millis(),
            )?;
            return Ok(SyncPage {
                posts,
                effective_url: current.to_string(),
                etag,
                last_modified,
                not_modified: false,
            });
        }
        Err(ConnectorError::UnsafeUrl)
    }
}

#[async_trait]
impl Connector for RssConnector {
    fn descriptor(&self) -> ConnectorDescriptor {
        ConnectorDescriptor {
            kind: "rss".into(),
            label: "RSS / Atom".into(),
            availability: ConnectorAvailability::Available,
            detail: "Official feed URLs; read-only and available now.".into(),
            unmet_prerequisite: None,
            read_only: true,
            supports_comments: false,
            requires_oauth: false,
        }
    }

    async fn sync(&self, request: &ConnectorSyncRequest) -> Result<SyncBatch, ConnectorError> {
        request.validate()?;
        if request.auth.is_some() {
            return Err(ConnectorError::InvalidFeed);
        }
        let ConnectorTransport::Rss(rss_request) = &request.transport else {
            return Err(ConnectorError::InvalidFeed);
        };
        let page = self.fetch(rss_request).await?;
        let batch = SyncBatch {
            posts: page.posts,
            comments: Vec::new(),
            comment_scope_post_ids: Vec::new(),
            cursor: request.source.cursor.clone(),
            page_finality: PageFinality::Complete,
            comment_completeness: CommentCompleteness::Unavailable,
            comments_truncated: false,
            health: ConnectorHealth {
                state: ConnectorHealthState::Healthy,
                safe_detail: "RSS synchronized.".into(),
                retry_at: None,
            },
            rss: Some(RssRepresentation {
                effective_url: page.effective_url,
                etag: page.etag,
                last_modified: page.last_modified,
                not_modified: page.not_modified,
            }),
        };
        batch.validate_for(SourceKind::Rss)?;
        Ok(batch)
    }
}

pub fn validate_sync_request(request: &SyncRequest) -> Result<(), ConnectorError> {
    if request.url.len() > 2_048
        || request
            .etag
            .as_deref()
            .is_some_and(|value| !valid_etag(value))
        || request
            .last_modified
            .as_deref()
            .is_some_and(|value| !valid_last_modified(value))
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    validate_public_feed_url(&request.url)
}

fn response_validators(
    headers: &HeaderMap,
    previous_etag: Option<&str>,
    previous_last_modified: Option<&str>,
) -> (Option<String>, Option<String>) {
    let etag = headers
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_etag(value))
        .map(ToOwned::to_owned)
        .or_else(|| {
            previous_etag
                .filter(|value| valid_etag(value))
                .map(ToOwned::to_owned)
        });
    let last_modified = headers
        .get(LAST_MODIFIED)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_last_modified(value))
        .map(ToOwned::to_owned)
        .or_else(|| {
            previous_last_modified
                .filter(|value| valid_last_modified(value))
                .map(ToOwned::to_owned)
        });
    (etag, last_modified)
}

fn valid_etag(value: &str) -> bool {
    if value.is_empty() || value.len() > 1_024 {
        return false;
    }
    let opaque = value.strip_prefix("W/").unwrap_or(value);
    opaque.len() >= 2
        && opaque.starts_with('"')
        && opaque.ends_with('"')
        && opaque[1..opaque.len() - 1]
            .bytes()
            .all(|byte| byte == 0x21 || (0x23..=0x7e).contains(&byte))
}

fn valid_last_modified(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && httpdate::parse_http_date(value).is_ok()
}

pub fn validate_public_feed_url(value: &str) -> Result<(), ConnectorError> {
    if value.is_empty() || value.len() > 2_048 {
        return Err(ConnectorError::UnsafeUrl);
    }
    let url = Url::parse(value).map_err(|_| ConnectorError::UnsafeUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_some_and(|host| host.len() > 253)
        || url.path().len() > 1_500
        || url.query().is_some_and(|query| query.len() > 512)
    {
        return Err(ConnectorError::UnsafeUrl);
    }
    let host = url.host().ok_or(ConnectorError::UnsafeUrl)?;
    match host {
        Host::Ipv4(address) => is_public_ip(IpAddr::V4(address))
            .then_some(())
            .ok_or(ConnectorError::UnsafeUrl),
        Host::Ipv6(address) => is_public_ip(IpAddr::V6(address))
            .then_some(())
            .ok_or(ConnectorError::UnsafeUrl),
        Host::Domain(domain) => {
            let normalized = domain.trim_end_matches('.').to_ascii_lowercase();
            if normalized == "localhost"
                || normalized.ends_with(".localhost")
                || normalized.ends_with(".local")
                || normalized.ends_with(".internal")
            {
                Err(ConnectorError::UnsafeUrl)
            } else {
                Ok(())
            }
        }
    }
}

fn validate_resolved_addresses(addresses: &[SocketAddr]) -> Result<(), ConnectorError> {
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(ConnectorError::UnsafeUrl);
    }
    Ok(())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !is_special_v4(ip),
        IpAddr::V6(ip) => is_global_unicast_v6(ip),
    }
}

/// Conservative, registry-aligned public IPv4 policy. Every special-purpose block is denied;
/// future additions must be reviewed against the IANA registry before they are admitted.
fn is_special_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
}

/// Current globally routable IPv6 space is conservatively limited to 2000::/3, with the
/// special-purpose ranges inside it denied explicitly. This also rejects IPv4-compatible,
/// IPv4-mapped, NAT64 (64:ff9b::/96 and 64:ff9b:1::/48), discard-only (100::/64), dummy
/// (100:0:0:1::/64), ULA, link-local, and multicast space because they are outside 2000::/3.
fn is_global_unicast_v6(ip: Ipv6Addr) -> bool {
    if ip.to_ipv4_mapped().is_some() {
        return false;
    }
    let value = u128::from(ip);
    in_v6_prefix(
        value,
        u128::from(Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0)),
        3,
    ) && !in_v6_prefix(
        value,
        u128::from(Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0)),
        23,
    ) && !in_v6_prefix(
        value,
        u128::from(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0)),
        32,
    ) && !in_v6_prefix(
        value,
        u128::from(Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0)),
        16,
    ) && !in_v6_prefix(
        value,
        u128::from(Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0)),
        20,
    )
}

fn in_v6_prefix(value: u128, network: u128, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    value & mask == network & mask
}

fn redirect_allowed(current: &Url, next: &Url) -> bool {
    matches!(next.scheme(), "http" | "https")
        && !(current.scheme() == "https" && next.scheme() != "https")
}

fn parse_feed(
    bytes: &[u8],
    effective_feed_url: &str,
    fetched_at: i64,
) -> Result<Vec<NormalizedPost>, ConnectorError> {
    let feed = parser::Builder::new()
        .base_uri(Some(effective_feed_url))
        .build()
        .parse(bytes)
        .map_err(|_| ConnectorError::InvalidFeed)?;
    let base = Url::parse(effective_feed_url).map_err(|_| ConnectorError::InvalidFeed)?;
    let mut posts = Vec::new();
    for entry in feed.entries.into_iter().take(MAX_ITEMS_PER_SYNC) {
        let canonical_url = entry
            .links
            .iter()
            .find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
            .or_else(|| entry.links.first())
            .and_then(|link| canonical_http_url(&base, &link.href));
        let remote_id = clean_text(&entry.id, 1_000);
        if remote_id.is_empty() {
            continue;
        }
        let title = clean_text(
            entry
                .title
                .as_ref()
                .map_or("Untitled", |title| title.content.as_str()),
            500,
        );
        let body = entry
            .summary
            .as_ref()
            .map(|summary| summary.content.as_str())
            .or_else(|| {
                entry
                    .content
                    .as_ref()
                    .and_then(|content| content.body.as_deref())
            })
            .unwrap_or("");
        let author = entry.authors.first().map_or_else(
            || "Unknown author".to_owned(),
            |person| clean_text(&person.name, 200),
        );
        let (published_at, timestamp_kind) = if let Some(date) = entry.published {
            (date.timestamp_millis(), TimestampKind::Published)
        } else if let Some(date) = entry.updated {
            (date.timestamp_millis(), TimestampKind::Updated)
        } else {
            (fetched_at, TimestampKind::Fetched)
        };
        posts.push(NormalizedPost {
            remote_id,
            canonical_url,
            author,
            title,
            body_text: clean_text(body, 20_000),
            published_at,
            timestamp_kind,
        });
    }
    Ok(posts)
}

fn canonical_http_url(base: &Url, value: &str) -> Option<String> {
    let mut url = Url::parse(value).or_else(|_| base.join(value)).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.username() != ""
        || url.password().is_some()
        || url.as_str().len() > 2_048
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.into())
}

fn clean_text(value: &str, max_chars: usize) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut previous_space = false;
    for character in value
        .chars()
        .filter(|character| !character.is_control() && !is_unsafe_format(*character))
        .take(max_chars * 2)
    {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            _ if character.is_whitespace() => {
                if !previous_space && !result.is_empty() {
                    result.push(' ');
                }
                previous_space = true;
            }
            _ => {
                if result.chars().count() >= max_chars {
                    break;
                }
                result.push(character);
                previous_space = false;
            }
        }
    }
    result.trim().to_owned()
}

fn is_unsafe_format(character: char) -> bool {
    matches!(character, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_credentialed_urls() {
        for value in [
            "http://127.0.0.1/feed",
            "http://[::1]/feed",
            "http://[::ffff:127.0.0.1]/feed",
            "http://[::ffff:10.0.0.1]/feed",
            "http://[::127.0.0.1]/feed",
            "http://[64:ff9b::7f00:1]/feed",
            "http://[64:ff9b:1::1]/feed",
            "http://[100::1]/feed",
            "http://[100:0:0:1::1]/feed",
            "http://[2002:7f00:1::1]/feed",
            "http://[2001:db8::1]/feed",
            "http://[3fff::1]/feed",
            "http://169.254.169.254/latest",
            "http://198.18.0.1/benchmark",
            "http://240.0.0.1/reserved",
            "https://user:pass@example.com/feed",
            "file:///tmp/feed",
            "https://service.local/feed",
        ] {
            assert!(validate_public_feed_url(value).is_err(), "{value}");
        }
        assert!(validate_public_feed_url("https://example.com/feed.xml").is_ok());
        assert!(validate_public_feed_url("https://[2606:4700:4700::1111]/feed").is_ok());
    }

    #[test]
    fn rejects_mixed_resolution_and_https_downgrade() {
        let addresses = [
            "93.184.216.34:443".parse().expect("public"),
            "127.0.0.1:443".parse().expect("private"),
        ];
        assert!(validate_resolved_addresses(&addresses).is_err());
        let https = Url::parse("https://example.com/feed").expect("url");
        let http = Url::parse("http://example.com/feed").expect("url");
        assert!(!redirect_allowed(&https, &http));
        assert!(redirect_allowed(&https, &https));
    }

    #[test]
    fn rejects_oversized_sync_input_before_network() {
        let request = SyncRequest {
            url: format!("https://example.com/{}", "a".repeat(2_100)),
            etag: None,
            last_modified: None,
        };
        assert!(validate_sync_request(&request).is_err());
        assert!(
            validate_sync_request(&SyncRequest {
                url: "https://example.com/feed".into(),
                etag: Some("not-an-etag".into()),
                last_modified: None,
            })
            .is_err()
        );
    }

    #[test]
    fn response_validators_are_bounded_valid_and_rotate_on_304() {
        let mut headers = HeaderMap::new();
        headers.insert(ETAG, "W/\"rotated\"".parse().expect("etag"));
        headers.insert(
            LAST_MODIFIED,
            "Sun, 06 Nov 1994 08:49:37 GMT".parse().expect("date"),
        );
        assert_eq!(
            response_validators(&headers, Some("\"old\""), None),
            (
                Some("W/\"rotated\"".into()),
                Some("Sun, 06 Nov 1994 08:49:37 GMT".into())
            )
        );

        let mut hostile = HeaderMap::new();
        hostile.insert(
            ETAG,
            format!("\"{}\"", "a".repeat(1_024))
                .parse()
                .expect("header"),
        );
        hostile.insert(LAST_MODIFIED, "not a date".parse().expect("header"));
        assert_eq!(
            response_validators(&hostile, Some("\"safe\""), None),
            (Some("\"safe\"".into()), None)
        );
    }

    #[test]
    fn parses_bounded_atom_fixture_as_plain_text() {
        let fixture = include_bytes!("../../../tests/fixtures/sample.atom");
        let posts = parse_feed(fixture, "https://example.com/feed.atom", 42).expect("feed");
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].title, "An important update");
        assert!(!posts[0].body_text.contains('<'));
        assert!(posts[0].body_text.contains("data, not instructions"));
    }

    #[test]
    fn normalizes_links_and_preserves_timestamp_provenance() {
        let fixture = br#"<?xml version="1.0"?>
          <feed xmlns="http://www.w3.org/2005/Atom">
            <title>Test</title><id>tag:example.test,2026:feed</id><updated>2026-01-01T00:00:00Z</updated>
            <entry><title>Relative</title><id>tag:example.test,2026:relative</id><link href="posts/1"/><updated>2026-01-02T00:00:00Z</updated></entry>
            <entry><title>Unsafe</title><id>urn:opaque:2</id><link href="javascript:alert(1)"/></entry>
            <entry><title>Guid only</title><id>urn:opaque:3</id></entry>
          </feed>"#;
        let posts = parse_feed(fixture, "https://example.test/news/feed.atom", 123).expect("feed");
        assert_eq!(
            posts[0].canonical_url.as_deref(),
            Some("https://example.test/news/posts/1")
        );
        assert_eq!(posts[0].timestamp_kind, TimestampKind::Updated);
        assert_eq!(posts[1].canonical_url, None);
        assert_eq!(posts[1].timestamp_kind, TimestampKind::Fetched);
        assert_eq!(posts[1].published_at, 123);
        assert_eq!(posts[2].canonical_url, None);
        assert_eq!(posts[2].remote_id, "urn:opaque:3");
    }

    #[test]
    fn neutral_adapter_preserves_frozen_rss_character_bounds_for_multibyte_text() {
        let guid = "界".repeat(700);
        let title = "界".repeat(500);
        let body = "界".repeat(20_000);
        let author = "界".repeat(200);
        let fixture = format!(
            r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title>Test</title><id>feed</id><updated>2026-01-01T00:00:00Z</updated><entry><id>{guid}</id><title>{title}</title><author><name>{author}</name></author><content>{body}</content></entry></feed>"#
        );
        let posts = parse_feed(fixture.as_bytes(), "https://example.test/feed", 1).expect("feed");
        assert_eq!(posts[0].remote_id.chars().count(), 700);
        assert_eq!(posts[0].title.chars().count(), 500);
        assert_eq!(posts[0].body_text.chars().count(), 20_000);
        let batch = SyncBatch {
            posts,
            comments: Vec::new(),
            comment_scope_post_ids: Vec::new(),
            cursor: None,
            page_finality: PageFinality::Complete,
            comment_completeness: CommentCompleteness::Unavailable,
            comments_truncated: false,
            health: ConnectorHealth {
                state: ConnectorHealthState::Healthy,
                safe_detail: "RSS synchronized.".into(),
                retry_at: None,
            },
            rss: Some(RssRepresentation {
                effective_url: "https://example.test/feed".into(),
                etag: None,
                last_modified: None,
                not_modified: false,
            }),
        };
        batch.validate_for(SourceKind::Rss).expect("RSS bounds");
        assert!(batch.validate_for(SourceKind::Mastodon).is_err());
    }

    #[test]
    fn browser_content_controls_are_removed_from_text() {
        assert_eq!(
            clean_text("<script>alert(1)</script><p>Hello\u{202e} world</p>", 100),
            "alert(1)Hello world"
        );
    }
}
