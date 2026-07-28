use url::Url;

pub fn redact_url(value: &str) -> String {
    Url::parse(value).map_or_else(
        |_| "[invalid-url]".to_owned(),
        |mut url| {
            url.set_query(None);
            url.set_fragment(None);
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        },
    )
}

pub fn safe_activity_detail(kind: &str, count: usize) -> String {
    match kind {
        "sync" => format!("Source sync processed {count} items"),
        "digest" => format!("Edition prepared with {count} items"),
        _ => "Local job completed".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_remove_credentials_queries_and_fragments() {
        assert_eq!(
            redact_url("https://user:secret@example.com/path?token=secret#private"),
            "https://example.com/path"
        );
        let detail = safe_activity_detail("sync", 4);
        assert!(!detail.contains("secret"));
    }
}
