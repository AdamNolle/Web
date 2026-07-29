//! Archive import: turns a user-selected official personal-data-export file (X, Instagram) into
//! bounded `NormalizedPost`s. This module makes **no network call whatsoever** -- it only parses
//! bytes that Rust itself already read from a local path the user picked via the OS file dialog.
//! It is deliberately not a `Connector`: there is no live `sync()`, no cursor, no health/backoff,
//! because an archive import is a one-time (or user-repeated) local file read, never a recurring
//! connection. Never call this a "connector" in user-facing copy for that same reason.
//!
//! Every parser here is defensive by construction: untrusted, unverified-schema file content is
//! validated field-by-field before any value is trusted, malformed/missing fields cause that one
//! entry to be skipped (not the whole file) unless nothing at all could be recovered, and nothing
//! ever panics on attacker- or corruption-controlled bytes.
//!
//! Format assumptions (current as of 2026, not contractually stable -- platforms change export
//! formats without notice; this is why every parser degrades to a typed error rather than trusting
//! unrecognized structure):
//! - X ("Download an archive of your data"): `data/tweets.js` (or `tweet.js`) contains a single
//!   JavaScript assignment, e.g. `window.YTD.tweets.part0 = [ ... ];`, wrapping a JSON array. Each
//!   element is either `{"tweet": {...}}` or, in some tool output, a bare tweet object. Relevant
//!   fields: `id_str`/`id`, `full_text`/`text`, `created_at` (format `"%a %b %d %H:%M:%S %z %Y"`,
//!   e.g. `"Wed Oct 10 20:19:24 +0000 2018"`).
//! - Instagram ("Download your information", JSON format): `your_instagram_activity/.../posts_1.json`
//!   (path varies by export tool version) contains a top-level JSON array of post objects. Each
//!   post has an optional top-level `title` (caption) and `creation_timestamp`, and a `media` array
//!   whose first element may carry its own `title`/`creation_timestamp` for single-media posts.
//!   Instagram does not include a stable post ID or permalink in this file, so imported posts have
//!   no `canonical_url` -- an honest, documented gap, not a bug.
//! - Facebook is not implemented in this pass; `ImportPlatform` has no Facebook variant yet.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use serde::de::{DeserializeSeed, Deserializer as _, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{NormalizedPost, TimestampKind};

/// Real archives can be several MB of JSON text even excluding media. This is a fixed, generous
/// multiple of the 2 MB RSS response cap (`rss::MAX_RESPONSE_BYTES`) chosen to bound memory use for
/// a local file read, not to match any particular platform's typical export size.
pub const MAX_IMPORT_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// Archive files are already bounded by `MAX_IMPORT_FILE_BYTES`, so they can safely have a much
/// larger item envelope than one live connector page. Keeping a dedicated ceiling prevents
/// pathological tiny-entry arrays from creating unbounded per-row/database work while still
/// covering realistic personal archives. Files above this ceiling fail explicitly instead of
/// silently dropping their tail.
pub const MAX_IMPORT_ITEMS: usize = 25_000;

const MAX_BODY_CHARS: usize = 20_000;
const MAX_TITLE_CHARS: usize = 200;
const MAX_MEDIA_URI_CHARS: usize = 4_096;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportPlatform {
    X,
    Instagram,
}

impl ImportPlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Instagram => "instagram",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "x" => Some(Self::X),
            "instagram" => Some(Self::Instagram),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("the selected file is larger than the archive import limit")]
    FileTooLarge,
    #[error("the selected file contains more entries than one archive import can safely process")]
    TooManyItems,
    #[error("the selected file contains conflicting entries with the same post identity")]
    ConflictingDuplicate { remote_id: String },
    #[error("the selected file could not be read")]
    UnreadableFile,
    #[error("the selected file is not a recognized archive export")]
    UnrecognizedFormat,
    #[error("no importable entries were found in that file")]
    NoItemsFound,
}

#[derive(Debug, Clone)]
pub struct ParsedExport {
    pub posts: Vec<NormalizedPost>,
    /// Entries present in the file that were not imported because a required field was
    /// missing or malformed, or because they exactly duplicated an earlier entry. Exceeding the
    /// dedicated item bound rejects the file explicitly.
    pub skipped: usize,
}

const ABORT_MARKER: &str = "archive import aborted";

struct ExportArrayVisitor<'a> {
    platform: ImportPlatform,
    author: &'a str,
    abort: &'a mut Option<ImportError>,
}

struct ExportEntrySeed<'a> {
    entries_seen: &'a mut usize,
    abort: &'a mut Option<ImportError>,
}

impl<'de> DeserializeSeed<'de> for ExportEntrySeed<'_> {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if *self.entries_seen >= MAX_IMPORT_ITEMS {
            *self.abort = Some(ImportError::TooManyItems);
            return Err(serde::de::Error::custom(ABORT_MARKER));
        }
        *self.entries_seen = (*self.entries_seen).saturating_add(1);
        serde_json::Value::deserialize(deserializer)
    }
}

impl<'de> Visitor<'de> for ExportArrayVisitor<'_> {
    type Value = ParsedExport;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a top-level archive entry array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut posts = Vec::new();
        let mut positions = HashMap::<String, usize>::new();
        let mut skipped = 0_usize;
        let mut entry_count = 0_usize;
        while let Some(entry) = sequence.next_element_seed(ExportEntrySeed {
            entries_seen: &mut entry_count,
            abort: &mut *self.abort,
        })? {
            let normalized = match self.platform {
                ImportPlatform::X => {
                    let tweet = entry.get("tweet").unwrap_or(&entry);
                    normalize_tweet(tweet, self.author)
                }
                ImportPlatform::Instagram => normalize_instagram_post(&entry, self.author),
            };
            let Some(post) = normalized else {
                skipped = skipped.saturating_add(1);
                continue;
            };
            if let Some(&position) = positions.get(&post.remote_id) {
                if normalized_posts_equal(&posts[position], &post) {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
                *self.abort = Some(ImportError::ConflictingDuplicate {
                    remote_id: post.remote_id,
                });
                return Err(serde::de::Error::custom(ABORT_MARKER));
            }
            positions.insert(post.remote_id.clone(), posts.len());
            posts.push(post);
        }
        Ok(ParsedExport { posts, skipped })
    }
}

fn normalized_posts_equal(left: &NormalizedPost, right: &NormalizedPost) -> bool {
    left == right
}

fn parse_json_array(
    platform: ImportPlatform,
    bytes: &[u8],
    author: &str,
) -> Result<ParsedExport, ImportError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let mut abort = None;
    let parsed = deserializer.deserialize_seq(ExportArrayVisitor {
        platform,
        author,
        abort: &mut abort,
    });
    if let Some(error) = abort {
        return Err(error);
    }
    let parsed = parsed.map_err(|_| ImportError::UnrecognizedFormat)?;
    deserializer
        .end()
        .map_err(|_| ImportError::UnrecognizedFormat)?;
    if parsed.posts.is_empty() {
        return Err(ImportError::NoItemsFound);
    }
    Ok(parsed)
}

/// Parses raw archive-export bytes already read by Rust. `default_author` is shown as the post
/// author for platforms whose export does not carry a stable per-post author field (both X and
/// Instagram archives describe exactly one account, so the caller supplies that account's label).
pub fn parse_export_file(
    platform: ImportPlatform,
    bytes: &[u8],
    default_author: &str,
) -> Result<ParsedExport, ImportError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IMPORT_FILE_BYTES {
        return Err(ImportError::FileTooLarge);
    }
    if bytes.is_empty() {
        return Err(ImportError::UnreadableFile);
    }
    let author = sanitize_text(default_author, 200);
    let author = if author.is_empty() {
        "Archive owner".to_owned()
    } else {
        author
    };
    match platform {
        ImportPlatform::X => parse_x_export(bytes, &author),
        ImportPlatform::Instagram => parse_instagram_export(bytes, &author),
    }
}

fn parse_x_export(bytes: &[u8], author: &str) -> Result<ParsedExport, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ImportError::UnrecognizedFormat)?;
    let json_slice = strip_js_assignment_prefix(text)?;
    parse_json_array(ImportPlatform::X, json_slice.as_bytes(), author)
}

/// Strips a leading `identifier.chain = ` JavaScript assignment (the documented X archive format,
/// e.g. `window.YTD.tweets.part0 = `) before a JSON array, tolerating a bare JSON array with no
/// prefix and a single trailing `;`. Anything else is rejected as an unrecognized format rather
/// than guessed at.
fn strip_js_assignment_prefix(text: &str) -> Result<&str, ImportError> {
    let trimmed = text.trim();
    let body = if trimmed.starts_with('[') {
        trimmed
    } else {
        let bracket_pos = trimmed.find('[').ok_or(ImportError::UnrecognizedFormat)?;
        let prefix = trimmed[..bracket_pos].trim_end();
        let assignment = prefix
            .strip_suffix('=')
            .map(str::trim_end)
            .ok_or(ImportError::UnrecognizedFormat)?;
        if assignment.is_empty()
            || assignment.len() > 200
            || !assignment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_'))
        {
            return Err(ImportError::UnrecognizedFormat);
        }
        &trimmed[bracket_pos..]
    };
    Ok(body.trim_end().strip_suffix(';').unwrap_or(body).trim())
}

fn normalize_tweet(tweet: &serde_json::Value, author: &str) -> Option<NormalizedPost> {
    let remote_id = tweet
        .get("id_str")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| tweet.get("id").and_then(json_id_as_string))?;
    let remote_id = sanitize_text(&remote_id, 512);
    if remote_id.is_empty() {
        return None;
    }
    let raw_text = tweet
        .get("full_text")
        .and_then(serde_json::Value::as_str)
        .or_else(|| tweet.get("text").and_then(serde_json::Value::as_str))?;
    let body_text = sanitize_text(raw_text, MAX_BODY_CHARS);
    if body_text.is_empty() {
        return None;
    }
    let (published_at, timestamp_kind) = tweet
        .get("created_at")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_x_created_at)
        .map_or((0, TimestampKind::Fetched), |millis| {
            (millis, TimestampKind::Published)
        });
    Some(NormalizedPost {
        remote_id,
        canonical_url: None,
        author: author.to_owned(),
        title: derive_title(&body_text),
        body_text,
        published_at,
        timestamp_kind,
    })
}

fn json_id_as_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

fn parse_x_created_at(value: &str) -> Option<i64> {
    if value.len() > 64 {
        return None;
    }
    chrono::DateTime::parse_from_str(value, "%a %b %d %H:%M:%S %z %Y")
        .ok()
        .map(|parsed| parsed.timestamp_millis())
}

fn parse_instagram_export(bytes: &[u8], author: &str) -> Result<ParsedExport, ImportError> {
    parse_json_array(ImportPlatform::Instagram, bytes, author)
}

fn normalize_instagram_post(entry: &serde_json::Value, author: &str) -> Option<NormalizedPost> {
    let entry = entry.as_object()?;
    let media = entry.get("media").and_then(serde_json::Value::as_array);
    let first_media = media.and_then(|items| items.first());
    // Single-media posts carry `creation_timestamp`/`title` on the media item; multi-photo posts
    // carry them on the post itself. Prefer the post-level value, falling back to the first media
    // item, matching Instagram's own documented export shape.
    let creation_timestamp = entry
        .get("creation_timestamp")
        .and_then(serde_json::Value::as_i64)
        .or_else(|| {
            first_media
                .and_then(|item| item.get("creation_timestamp"))
                .and_then(serde_json::Value::as_i64)
        })?;
    if !(0..=32_503_680_000).contains(&creation_timestamp) {
        // Reject implausible (negative, or post-year-3000) timestamps rather than trusting them.
        return None;
    }
    let caption = entry
        .get("title")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            first_media
                .and_then(|item| item.get("title"))
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("");
    let body_text = sanitize_text(caption, MAX_BODY_CHARS);

    let mut media_uris = BTreeSet::new();
    if let Some(uri) = entry
        .get("uri")
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_media_uri)
    {
        media_uris.insert(uri);
    }
    if let Some(media) = media {
        for item in media {
            if let Some(uri) = item
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .and_then(normalize_media_uri)
            {
                media_uris.insert(uri);
            }
        }
    }
    if media_uris.is_empty() {
        // Instagram exports do not provide a stable post ID. Without at least one media path,
        // caption-only identity would create a new row whenever a user edits their caption.
        return None;
    }
    let remote_id = instagram_remote_id(&media_uris, creation_timestamp);
    let title = if body_text.is_empty() {
        "Archived Instagram post".to_owned()
    } else {
        derive_title(&body_text)
    };
    Some(NormalizedPost {
        remote_id,
        // Instagram's export does not include a permalink/shortcode for posts, so no honest
        // canonical URL can be constructed.
        canonical_url: None,
        author: author.to_owned(),
        title,
        body_text,
        published_at: creation_timestamp.saturating_mul(1_000),
        timestamp_kind: TimestampKind::Published,
    })
}

/// Canonicalizes the path-like media URIs used by official Instagram exports.
///
/// Empty and current-directory segments collapse, parent segments resolve only within the URI,
/// and control characters or oversized values are rejected.
fn normalize_media_uri(value: &str) -> Option<String> {
    if value.chars().count() > MAX_MEDIA_URI_CHARS {
        return None;
    }
    let normalized = value.trim().replace('\\', "/");
    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        let segment = segment.trim();
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            _ if segment.chars().any(char::is_control) => return None,
            _ => segments.push(segment),
        }
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

/// Instagram's export has no stable post ID, so derive one from the complete normalized media set
/// and timestamp. Sorting and deduplicating makes export-order changes harmless; captions are
/// deliberately excluded so an edit updates the existing row on re-import.
fn instagram_remote_id(media_uris: &BTreeSet<String>, creation_timestamp: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"web-instagram-media-v2\0");
    hasher.update(creation_timestamp.to_le_bytes());
    for uri in media_uris {
        let uri_bytes = uri.as_bytes();
        hasher.update(
            u64::try_from(uri_bytes.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        hasher.update(uri_bytes);
    }
    format!("ig-{:x}", hasher.finalize())[..40].to_owned()
}

fn derive_title(body_text: &str) -> String {
    let truncated: String = body_text.chars().take(MAX_TITLE_CHARS).collect();
    if truncated.is_empty() {
        "Archived post".to_owned()
    } else if truncated.chars().count() < body_text.chars().count() {
        format!("{truncated}\u{2026}")
    } else {
        truncated
    }
}

/// Strips control characters (keeping newline/tab) and bounds length. Untrusted archive file text
/// is not HTML, so no tag-stripping is needed the way RSS content requires.
fn sanitize_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    const TWEETS_FIXTURE: &str = include_str!("../../../tests/fixtures/x_tweets_sample.fixture");
    const INSTAGRAM_FIXTURE: &str =
        include_str!("../../../tests/fixtures/instagram_posts_sample.json");

    fn realistic_x_archive(item_count: usize) -> String {
        let mut archive = String::with_capacity(item_count.saturating_mul(180));
        archive.push_str("window.YTD.tweets.part0 = [");
        for index in 0..item_count {
            if index > 0 {
                archive.push(',');
            }
            write!(
                &mut archive,
                r#"{{"tweet":{{"id_str":"{index}","full_text":"Archived tweet {index} with a realistic caption and link https://example.com/{index}","created_at":"Wed Oct 10 20:19:24 +0000 2018"}}}}"#
            )
            .expect("write archive entry");
        }
        archive.push_str("];");
        archive
    }

    #[test]
    fn parses_bounded_x_fixture_with_published_provenance() {
        let result = parse_export_file(ImportPlatform::X, TWEETS_FIXTURE.as_bytes(), "Ada")
            .expect("valid fixture parses");
        assert_eq!(result.posts.len(), 2);
        assert_eq!(result.skipped, 1); // the fixture's third entry is deliberately malformed.
        assert_eq!(result.posts[0].remote_id, "1000000000000000001");
        assert_eq!(result.posts[0].author, "Ada");
        assert!(result.posts[0].body_text.contains("first archived tweet"));
        assert_eq!(result.posts[0].timestamp_kind, TimestampKind::Published);
        assert!(result.posts[0].published_at > 0);
        assert_eq!(result.posts[0].canonical_url, None);
        // Missing created_at falls back to Fetched with a zero timestamp, never a panic/guess.
        assert_eq!(result.posts[1].timestamp_kind, TimestampKind::Fetched);
    }

    #[test]
    fn parses_bounded_instagram_fixture_with_published_provenance() {
        let result = parse_export_file(
            ImportPlatform::Instagram,
            INSTAGRAM_FIXTURE.as_bytes(),
            "Ada",
        )
        .expect("valid fixture parses");
        assert_eq!(result.posts.len(), 2);
        assert_eq!(result.skipped, 1); // the fixture's third entry has no timestamp anywhere.
        assert!(result.posts[0].body_text.contains("Multi-photo caption"));
        assert_eq!(result.posts[0].timestamp_kind, TimestampKind::Published);
        assert_eq!(result.posts[0].canonical_url, None);
        // Single-media post: creation_timestamp/title resolved from the media item, not the post.
        assert!(result.posts[1].body_text.contains("Single photo caption"));
    }

    #[test]
    fn rejects_oversized_and_wrong_type_and_empty_files_without_panicking() {
        assert!(matches!(
            parse_export_file(ImportPlatform::X, b"", "Ada"),
            Err(ImportError::UnreadableFile)
        ));
        assert!(matches!(
            parse_export_file(ImportPlatform::X, b"not json at all {{{", "Ada"),
            Err(ImportError::UnrecognizedFormat)
        ));
        assert!(matches!(
            parse_export_file(ImportPlatform::X, &[0xff, 0xfe, 0x00, 0x01], "Ada"),
            Err(ImportError::UnrecognizedFormat)
        ));
        assert!(matches!(
            parse_export_file(ImportPlatform::Instagram, b"{\"not\":\"an array\"}", "Ada"),
            Err(ImportError::UnrecognizedFormat)
        ));
        assert!(matches!(
            parse_export_file(ImportPlatform::X, b"window.YTD.tweets.part0 = []", "Ada"),
            Err(ImportError::NoItemsFound)
        ));
        assert!(matches!(
            parse_export_file(ImportPlatform::Instagram, b"[]", "Ada"),
            Err(ImportError::NoItemsFound)
        ));
        assert!(matches!(
            parse_export_file(ImportPlatform::Instagram, b"[] trailing", "Ada"),
            Err(ImportError::UnrecognizedFormat)
        ));
    }

    #[test]
    fn parser_accepts_the_exact_file_limit_and_rejects_one_byte_more() {
        let limit = usize::try_from(MAX_IMPORT_FILE_BYTES).expect("portable import limit");
        let mut bytes = vec![b'x'; limit];
        assert!(matches!(
            parse_export_file(ImportPlatform::X, &bytes, "Ada"),
            Err(ImportError::UnrecognizedFormat)
        ));
        bytes.push(b'x');
        assert!(matches!(
            parse_export_file(ImportPlatform::X, &bytes, "Ada"),
            Err(ImportError::FileTooLarge)
        ));
    }

    #[test]
    fn accepts_more_than_one_live_page_and_rejects_the_dedicated_archive_bound() {
        let realistic_count = crate::connectors::MAX_ITEMS_PER_SYNC + 150;
        let file = realistic_x_archive(realistic_count);
        let result = parse_export_file(ImportPlatform::X, file.as_bytes(), "Ada")
            .expect("archive larger than a live connector page");
        assert_eq!(result.posts.len(), realistic_count);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn enforces_exact_item_bound_without_consuming_the_overflow_tail() {
        let mut file = realistic_x_archive(MAX_IMPORT_ITEMS);
        let exact = parse_export_file(ImportPlatform::X, file.as_bytes(), "Ada")
            .expect("exact item bound is accepted");
        assert_eq!(exact.posts.len(), MAX_IMPORT_ITEMS);

        file.truncate(file.len() - 2);
        file.push_str(
            r#",{"tweet":{"id_str":"overflow","full_text":"overflow"}} THIS IS NOT JSON"#,
        );
        assert!(matches!(
            parse_export_file(ImportPlatform::X, file.as_bytes(), "Ada"),
            Err(ImportError::TooManyItems)
        ));

        let invalid_entries = format!(
            "[{}{{\"tweet\":{{\"id_str\":\"overflow\",\"full_text\":\"overflow\"}}}}]",
            "null,".repeat(MAX_IMPORT_ITEMS)
        );
        assert!(matches!(
            parse_export_file(ImportPlatform::X, invalid_entries.as_bytes(), "Ada"),
            Err(ImportError::TooManyItems)
        ));
    }

    #[test]
    fn realistic_large_archive_stays_within_file_and_time_bounds() {
        let file = realistic_x_archive(15_000);
        assert!(u64::try_from(file.len()).expect("file length") < MAX_IMPORT_FILE_BYTES);

        let started = Instant::now();
        let parsed = parse_export_file(ImportPlatform::X, file.as_bytes(), "Ada")
            .expect("large realistic archive");
        assert_eq!(parsed.posts.len(), 15_000);
        assert_eq!(parsed.skipped, 0);
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "debug parser exceeded generous boundedness threshold: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn identical_x_duplicates_collapse_but_conflicting_duplicates_fail() {
        let identical = br#"[
            {"tweet":{"id_str":"same","full_text":"same body","unknown":"ignored"}},
            {"tweet":{"id_str":"same","full_text":"same body"}},
            {"tweet":{"id_str":"second","full_text":"second body"}}
        ]"#;
        let parsed =
            parse_export_file(ImportPlatform::X, identical, "Ada").expect("identical duplicate");
        assert_eq!(parsed.posts.len(), 2);
        assert_eq!(parsed.posts[0].remote_id, "same");
        assert_eq!(parsed.posts[1].remote_id, "second");
        assert_eq!(parsed.skipped, 1);

        let conflicting = br#"[
            {"tweet":{"id_str":"same","full_text":"first body"}},
            {"tweet":{"id_str":"same","full_text":"different body"}},
            THIS TRAILING DATA MUST NOT BE READ
        ]"#;
        match parse_export_file(ImportPlatform::X, conflicting, "Ada") {
            Err(ImportError::ConflictingDuplicate { remote_id }) => {
                assert_eq!(remote_id, "same");
            }
            other => panic!("expected typed duplicate conflict, got {other:?}"),
        }
    }

    #[test]
    fn instagram_identity_uses_the_normalized_complete_media_set_not_caption_or_order() {
        let original = br#"[
            {
                "title":"Original caption",
                "creation_timestamp":1784000000,
                "media":[
                    {"uri":" media\\posts\\2026\\b.jpg "},
                    {"uri":"media/posts/2026/./a.jpg"},
                    {"uri":"media/posts/2026/a.jpg"}
                ]
            }
        ]"#;
        let edited = br#"[
            {
                "title":"Edited caption",
                "creation_timestamp":1784000000,
                "media":[
                    {"uri":"media/posts/2026/a.jpg"},
                    {"uri":"media/posts/2026/b.jpg"}
                ]
            }
        ]"#;
        let original = parse_export_file(ImportPlatform::Instagram, original, "Ada")
            .expect("original archive");
        let edited =
            parse_export_file(ImportPlatform::Instagram, edited, "Ada").expect("edited archive");
        assert_eq!(original.posts[0].remote_id, edited.posts[0].remote_id);
        assert_ne!(original.posts[0].body_text, edited.posts[0].body_text);
    }

    #[test]
    fn instagram_duplicate_media_sets_collapse_or_conflict_before_persistence() {
        let identical = br#"[
            {
                "title":"Same caption",
                "creation_timestamp":1784000000,
                "media":[{"uri":"media/b.jpg"},{"uri":"media/a.jpg"}]
            },
            {
                "title":"Same caption",
                "creation_timestamp":1784000000,
                "media":[{"uri":"media/a.jpg"},{"uri":"media/b.jpg"}]
            }
        ]"#;
        let parsed = parse_export_file(ImportPlatform::Instagram, identical, "Ada")
            .expect("identical normalized Instagram entries");
        assert_eq!(parsed.posts.len(), 1);
        assert_eq!(parsed.skipped, 1);

        let conflicting = br#"[
            {
                "title":"Original caption",
                "creation_timestamp":1784000000,
                "media":[{"uri":"media/a.jpg"}]
            },
            {
                "title":"Edited caption",
                "creation_timestamp":1784000000,
                "media":[{"uri":"media/a.jpg"}]
            }
        ]"#;
        assert!(matches!(
            parse_export_file(ImportPlatform::Instagram, conflicting, "Ada"),
            Err(ImportError::ConflictingDuplicate { .. })
        ));
    }

    #[test]
    fn instagram_pathless_entries_are_skipped_even_when_they_have_captions() {
        let pathless = br#"[
            {"title":"Mutable caption","creation_timestamp":1784000000},
            {
                "title":"Stable post",
                "creation_timestamp":1784000001,
                "media":[{"uri":"media/posts/2026/stable.jpg"}]
            }
        ]"#;
        let parsed =
            parse_export_file(ImportPlatform::Instagram, pathless, "Ada").expect("one stable post");
        assert_eq!(parsed.posts.len(), 1);
        assert_eq!(parsed.skipped, 1);

        let only_pathless = br#"[{"title":"Mutable caption","creation_timestamp":1784000000}]"#;
        assert!(matches!(
            parse_export_file(ImportPlatform::Instagram, only_pathless, "Ada"),
            Err(ImportError::NoItemsFound)
        ));
    }

    #[test]
    fn strips_known_and_bare_js_prefixes_and_rejects_garbage_prefixes() {
        assert_eq!(strip_js_assignment_prefix("[1,2]").unwrap(), "[1,2]");
        assert_eq!(
            strip_js_assignment_prefix("window.YTD.tweet.part0 = [1,2];").unwrap(),
            "[1,2]"
        );
        assert!(strip_js_assignment_prefix("<script>alert(1)</script>[1]").is_err());
        assert!(strip_js_assignment_prefix("no array here").is_err());
    }

    #[test]
    fn platform_parsing_is_exact_and_case_sensitive() {
        assert_eq!(ImportPlatform::parse("x"), Some(ImportPlatform::X));
        assert_eq!(
            ImportPlatform::parse("instagram"),
            Some(ImportPlatform::Instagram)
        );
        assert_eq!(ImportPlatform::parse("X"), None);
        assert_eq!(ImportPlatform::parse("facebook"), None);
        assert_eq!(ImportPlatform::parse(""), None);
    }
}
