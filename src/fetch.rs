use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue, IF_NONE_MATCH, USER_AGENT};
use reqwest::redirect::Policy;

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
        let metadata = fs::metadata(&path)
            .with_context(|| format!("reading local spec {}", path.display()))?;
        if !metadata.is_file() {
            bail!("local spec {} is not a file", path.display());
        }
        if metadata.len() > MAX_BODY_BYTES as u64 {
            bail!(
                "local spec {} is too large ({:.1} MB, limit is {} MB)",
                path.display(),
                metadata.len() as f64 / 1_048_576.0,
                MAX_BODY_BYTES / 1_048_576
            );
        }
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

    // Custom headers often carry credentials. Never forward them to a
    // different origin via a redirect; the default reqwest policy only strips
    // a fixed set of sensitive header names.
    let has_custom_headers = !headers.is_empty();
    let redirect_policy = Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        if has_custom_headers {
            if let Some(previous) = attempt.previous().last() {
                if !same_origin(previous, attempt.url()) {
                    return attempt.error(
                        "refusing to follow a cross-origin redirect while custom headers are set",
                    );
                }
            }
        }
        attempt.follow()
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECONDS))
        .user_agent(format!("apiwatch/{}", env!("CARGO_PKG_VERSION")))
        .redirect(redirect_policy)
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
    Some(expand_home(url))
}

fn same_origin(previous: &Url, next: &Url) -> bool {
    next.host_str() == previous.host_str()
        && next.port_or_known_default() == previous.port_or_known_default()
        && next.scheme() == previous.scheme()
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
    fn missing_local_files_report_a_file_error() {
        let path = std::env::temp_dir().join("apiwatch-missing-spec.yaml");
        fs::remove_file(&path).ok();
        let error = fetch(path.to_str().unwrap(), &BTreeMap::new(), None).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("reading local spec"), "{message}");
        assert!(message.contains("apiwatch-missing-spec.yaml"), "{message}");
    }

    #[test]
    fn rejects_oversized_local_files_before_reading_them() {
        let path = std::env::temp_dir().join("apiwatch-oversized-spec.json");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_BODY_BYTES as u64 + 1)
            .unwrap();
        let error = fetch(path.to_str().unwrap(), &BTreeMap::new(), None).unwrap_err();
        fs::remove_file(&path).ok();
        assert!(error.to_string().contains("too large"), "{error:#}");
    }

    #[test]
    fn compares_redirect_origins() {
        let base = Url::parse("https://example.com/spec.json").unwrap();
        assert!(same_origin(
            &base,
            &Url::parse("https://example.com/other.json").unwrap()
        ));
        assert!(!same_origin(
            &base,
            &Url::parse("http://example.com/spec.json").unwrap()
        ));
        assert!(!same_origin(
            &base,
            &Url::parse("https://example.com:8443/spec.json").unwrap()
        ));
        assert!(!same_origin(
            &base,
            &Url::parse("https://api.example.com/spec.json").unwrap()
        ));
    }

    #[test]
    fn refuses_cross_origin_redirects_when_custom_headers_are_set() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/moved\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_string(), "Bearer secret".to_string());
        let error = fetch(
            &format!("http://127.0.0.1:{port}/spec.json"),
            &headers,
            None,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        server.join().unwrap();
        assert!(message.contains("cross-origin redirect"), "{message}");
    }

    #[test]
    fn follows_same_origin_redirects_when_custom_headers_are_set() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let response = if attempt == 0 {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}"
                        .to_string()
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_string(), "Bearer secret".to_string());
        let result = fetch(
            &format!("http://127.0.0.1:{port}/spec.json"),
            &headers,
            None,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(result.body.as_deref(), Some("{\"ok\":true}"));
    }

    #[test]
    fn rejects_unsupported_urls() {
        let error = fetch("ftp://example.com/spec.json", &BTreeMap::new(), None).unwrap_err();
        assert!(error.to_string().contains("unsupported URL"));
    }
}
