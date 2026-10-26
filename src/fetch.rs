use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue, IF_NONE_MATCH, USER_AGENT};

const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;
const TIMEOUT_SECONDS: u64 = 30;

#[derive(Debug)]
pub struct FetchResult {
    pub body: Option<String>,
    pub http_status: Option<u16>,
    pub etag: Option<String>,
    pub not_modified: bool,
}

/// Fetch a spec over HTTP(S) or read it from a local path.
///
/// Sending the previous ETag lets well-behaved servers answer `304 Not Modified`
/// instead of transferring an unchanged spec again.
pub fn fetch(
    url: &str,
    headers: &BTreeMap<String, String>,
    previous_etag: Option<&str>,
) -> Result<FetchResult> {
    if let Some(path) = local_path(url) {
        let body = fs::read_to_string(&path)
            .with_context(|| format!("reading local spec {}", path.display()))?;
        if body.trim().is_empty() {
            bail!("local spec {} is empty", path.display());
        }
        return Ok(FetchResult {
            body: Some(body),
            http_status: None,
            etag: None,
            not_modified: false,
        });
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        bail!("unsupported URL '{url}': use http(s):// or a local file path");
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECONDS))
        .user_agent(format!("apiwatch/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building HTTP client")?;

    let mut request_headers = HeaderMap::new();
    request_headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, application/yaml, text/yaml, */*"),
    );
    request_headers.insert(
        USER_AGENT,
        HeaderValue::from_str(&format!("apiwatch/{}", env!("CARGO_PKG_VERSION")))
            .expect("static user agent is valid"),
    );

    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name '{name}'"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid value for header '{name}'"))?;
        request_headers.insert(header_name, header_value);
    }

    if let Some(etag) = previous_etag {
        let header_value = HeaderValue::from_str(etag).context("invalid stored ETag")?;
        request_headers.insert(IF_NONE_MATCH, header_value);
    }

    let response = client
        .get(url)
        .headers(request_headers)
        .send()
        .with_context(|| format!("requesting {url}"))?;

    let status = response.status();
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(FetchResult {
            body: None,
            http_status: Some(status.as_u16()),
            etag: etag.or_else(|| previous_etag.map(str::to_string)),
            not_modified: true,
        });
    }

    if !status.is_success() {
        let mut snippet = String::new();
        let _ = response.take(2_000).read_to_string(&mut snippet);
        let snippet = snippet.trim().replace('\n', " ");
        let snippet: String = snippet.chars().take(300).collect();
        if snippet.is_empty() {
            bail!("HTTP {status} from {url}");
        }
        bail!("HTTP {status} from {url}: {snippet}");
    }

    if let Some(length) = response.content_length() {
        if length > MAX_BODY_BYTES as u64 {
            bail!(
                "response is too large ({:.1} MB, limit is {} MB)",
                length as f64 / 1_048_576.0,
                MAX_BODY_BYTES / 1_048_576
            );
        }
    }

    let mut buffer = Vec::new();
    response
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut buffer)
        .with_context(|| format!("reading response body from {url}"))?;
    if buffer.len() > MAX_BODY_BYTES {
        bail!(
            "response is too large (limit is {} MB)",
            MAX_BODY_BYTES / 1_048_576
        );
    }

    let body = String::from_utf8(buffer)
        .with_context(|| format!("response from {url} is not valid UTF-8"))?;

    Ok(FetchResult {
        body: Some(body),
        http_status: Some(status.as_u16()),
        etag,
        not_modified: false,
    })
}

fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(path) = url.strip_prefix("file://") {
        return Some(expand_home(path));
    }
    if url.contains("://") {
        return None;
    }
    let path = expand_home(url);
    path.is_file().then_some(path)
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_local_paths() {
        let path = std::env::temp_dir().join("apiwatch-fetch-test.json");
        fs::write(&path, "{\"ok\":true}").unwrap();
        let result = fetch(path.to_str().unwrap(), &BTreeMap::new(), None).unwrap();
        assert_eq!(result.body.as_deref(), Some("{\"ok\":true}"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_unsupported_urls() {
        let error = fetch("ftp://example.com/spec.json", &BTreeMap::new(), None).unwrap_err();
        assert!(error.to_string().contains("unsupported URL"));
    }
}
