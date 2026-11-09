//! Blocking HTTP with ETag caching, pagination, typed errors and token scrubbing.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use macrunnerbar_core::{ApiError, Secret};
use serde_json::Value;
use ureq::Agent;
use ureq::http::HeaderMap;

const MAX_PAGES: usize = 20;
const CACHE_MAX_ENTRIES: usize = 1024;
const CACHE_TTL: Duration = Duration::from_secs(3600);
const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: u64 = 8 * 1024;
const MAX_MESSAGE_CHARS: usize = 160;
const API_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub(crate) const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug)]
struct Cached {
    etag: String,
    body: Arc<Value>,
    next: Option<String>,
    last_used: Instant,
}

/// One response page: parsed JSON and the `rel="next"` link, if any.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    pub(crate) body: Arc<Value>,
    pub(crate) next: Option<String>,
}

pub(crate) struct Http {
    api: Agent,
    download: Agent,
    base: String,
    token: Secret<String>,
    cache: Mutex<HashMap<String, Cached>>,
}

impl Http {
    pub(crate) fn new(token: Secret<String>, base: &str, use_env_proxy: bool) -> Self {
        let agent = |timeout: Duration, redirects: u32| {
            let mut b = Agent::config_builder()
                .timeout_global(Some(timeout))
                .http_status_as_error(false)
                .max_redirects(redirects)
                .max_redirects_will_error(false)
                .user_agent("macrunnerbar");
            if !use_env_proxy {
                b = b.proxy(None);
            }
            Agent::new_with_config(b.build())
        };
        Self {
            // The API origin is fixed: never follow a redirect with the token attached.
            api: agent(API_TIMEOUT, 0),
            // Release assets redirect to a CDN; that agent never carries the token.
            download: agent(DOWNLOAD_TIMEOUT, 5),
            base: base.trim_end_matches('/').to_string(),
            token,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /// Whether plain `http://` downloads are acceptable (only for a local test stub).
    pub(crate) fn allows_insecure(&self) -> bool {
        self.base.starts_with("http://")
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<String, Cached>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replace every occurrence of the token in `s`.
    pub(crate) fn scrub(&self, s: &str) -> String {
        let t = self.token.expose();
        if t.is_empty() {
            s.to_string()
        } else {
            s.replace(t.as_str(), "[redacted]")
        }
    }

    fn url(&self, path_or_url: &str) -> Result<String, ApiError> {
        if path_or_url.starts_with('/') {
            return Ok(format!("{}{path_or_url}", self.base));
        }
        match path_or_url.strip_prefix(self.base.as_str()) {
            Some(rest) if rest.starts_with('/') || rest.is_empty() => Ok(path_or_url.to_string()),
            _ => Err(ApiError::Invalid("link leaves the API origin".into())),
        }
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token.expose())
    }

    /// GET one page. With `cache`, sends `If-None-Match` and replays the cached body on a 304.
    pub(crate) fn get(&self, path_or_url: &str, cache: bool) -> Result<Page, ApiError> {
        let url = self.url(path_or_url)?;
        let etag = if cache {
            self.cache().get(&url).map(|c| c.etag.clone())
        } else {
            None
        };
        let mut req = self
            .api
            .get(&url)
            .header("Authorization", self.auth())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(e) = &etag {
            req = req.header("If-None-Match", e.as_str());
        }
        let mut resp = req.call().map_err(|e| self.transport_error(&e))?;
        let status = resp.status().as_u16();
        match status {
            304 => {
                let mut c = self.cache();
                match c.get_mut(&url) {
                    Some(hit) => {
                        hit.last_used = Instant::now();
                        Ok(Page {
                            body: hit.body.clone(),
                            next: hit.next.clone(),
                        })
                    }
                    None => Err(ApiError::Invalid("304 without a cached response".into())),
                }
            }
            200..=299 => {
                let headers = resp.headers().clone();
                let bytes = resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_JSON_BYTES)
                    .read_to_vec()
                    .map_err(|e| self.transport_error(&e))?;
                let body: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| ApiError::Invalid("response is not valid JSON".into()))?;
                let body = Arc::new(body);
                let next = next_link(&headers);
                if cache && let Some(tag) = header_str(&headers, "etag") {
                    self.store(url, tag.to_string(), body.clone(), next.clone());
                }
                Ok(Page { body, next })
            }
            _ => Err(self.status_error(status, resp.headers().clone(), resp)),
        }
    }

    /// GET every page of a list, following same-origin `next` links, handing each body to `f`.
    pub(crate) fn get_all(
        &self,
        path: &str,
        mut f: impl FnMut(&Value) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        let mut next = Some(path.to_string());
        for _ in 0..MAX_PAGES {
            let Some(url) = next.take() else {
                return Ok(());
            };
            let page = self.get(&url, true)?;
            f(&page.body)?;
            next = page.next;
        }
        if next.is_some() {
            return Err(ApiError::Invalid("too many pages".into()));
        }
        Ok(())
    }

    /// POST a JSON body; returns the parsed JSON response.
    pub(crate) fn post_json(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        let url = self.url(path)?;
        let payload = body.to_string();
        let mut resp = self
            .api
            .post(&url)
            .header("Authorization", self.auth())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Content-Type", "application/json")
            .send(payload)
            .map_err(|e| self.transport_error(&e))?;
        let status = resp.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(self.status_error(status, resp.headers().clone(), resp));
        }
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_JSON_BYTES)
            .read_to_vec()
            .map_err(|e| self.transport_error(&e))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::Invalid("response is not valid JSON".into()))
    }

    /// DELETE; success is any 2xx.
    pub(crate) fn delete(&self, path: &str) -> Result<(), ApiError> {
        let url = self.url(path)?;
        let resp = self
            .api
            .delete(&url)
            .header("Authorization", self.auth())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(|e| self.transport_error(&e))?;
        let status = resp.status().as_u16();
        if (200..=299).contains(&status) {
            Ok(())
        } else {
            Err(self.status_error(status, resp.headers().clone(), resp))
        }
    }

    /// Open an unauthenticated streaming download (release asset). Returns a reader over the body.
    pub(crate) fn open_download(&self, url: &str) -> Result<Box<dyn Read + Send>, ApiError> {
        if !(url.starts_with("https://") || (self.allows_insecure() && url.starts_with("http://")))
        {
            return Err(ApiError::Invalid("download url is not https".into()));
        }
        let resp = self
            .download
            .get(url)
            .call()
            .map_err(|e| self.transport_error(&e))?;
        let status = resp.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(ApiError::Http { status });
        }
        Ok(Box::new(
            resp.into_body()
                .into_with_config()
                .limit(MAX_DOWNLOAD_BYTES)
                .reader(),
        ))
    }

    fn store(&self, url: String, etag: String, body: Arc<Value>, next: Option<String>) {
        let mut c = self.cache();
        let now = Instant::now();
        c.retain(|_, v| now.duration_since(v.last_used) < CACHE_TTL);
        if c.len() >= CACHE_MAX_ENTRIES
            && let Some(oldest) = c
                .iter()
                .min_by_key(|(_, v)| v.last_used)
                .map(|(k, _)| k.clone())
        {
            c.remove(&oldest);
        }
        c.insert(
            url,
            Cached {
                etag,
                body,
                next,
                last_used: now,
            },
        );
    }

    /// Map a transport failure to a fixed, token-free message (never `Display` of the ureq error,
    /// which can carry URLs).
    pub(crate) fn transport_error(&self, e: &ureq::Error) -> ApiError {
        let what = match e {
            ureq::Error::Timeout(_) => "timed out",
            ureq::Error::HostNotFound => "host not found",
            ureq::Error::ConnectionFailed => "connection failed",
            ureq::Error::Io(_) => "connection error",
            ureq::Error::Tls(_) => "TLS error",
            ureq::Error::BodyExceedsLimit(_) => "response too large",
            ureq::Error::Protocol(_) => "protocol error",
            _ => "request failed",
        };
        ApiError::Network(what.into())
    }

    fn status_error(
        &self,
        status: u16,
        headers: HeaderMap,
        mut resp: ureq::http::Response<ureq::Body>,
    ) -> ApiError {
        match status {
            401 => ApiError::Unauthorized,
            404 => ApiError::NotFound,
            403 | 429 => {
                let message = resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_ERROR_BODY_BYTES)
                    .read_to_vec()
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                let retry_after = retry_after(&headers);
                let exhausted = header_str(&headers, "x-ratelimit-remaining") == Some("0");
                let limited_text = message.to_ascii_lowercase().contains("rate limit");
                if exhausted || retry_after.is_some() || status == 429 || limited_text {
                    ApiError::RateLimited { retry_after }
                } else {
                    let short: String = message.chars().take(MAX_MESSAGE_CHARS).collect();
                    ApiError::Forbidden(self.scrub(&short))
                }
            }
            s => ApiError::Http { status: s },
        }
    }
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// `Retry-After` seconds, else time until `X-RateLimit-Reset` (+1s) when the quota is exhausted.
fn retry_after(h: &HeaderMap) -> Option<Duration> {
    if let Some(s) = header_str(h, "retry-after").and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(Duration::from_secs(s));
    }
    if header_str(h, "x-ratelimit-remaining") == Some("0") {
        let reset = header_str(h, "x-ratelimit-reset")?
            .trim()
            .parse::<u64>()
            .ok()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        return Some(Duration::from_secs(reset.saturating_sub(now) + 1));
    }
    None
}

/// The `rel="next"` URL from a `Link` header.
pub(crate) fn next_link(h: &HeaderMap) -> Option<String> {
    let link = header_str(h, "link")?;
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        let is_next = params.split(';').any(|p| {
            let p = p.trim().replace(' ', "");
            p.eq_ignore_ascii_case("rel=\"next\"") || p.eq_ignore_ascii_case("rel=next")
        });
        let url = url.trim().strip_prefix('<')?.strip_suffix('>')?;
        is_next.then(|| url.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ureq::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).expect("header value"));
        }
        h
    }

    #[test]
    fn next_link_found_among_several() {
        let h = headers(&[(
            "link",
            "<https://api.github.com/x?page=1>; rel=\"prev\", <https://api.github.com/x?page=3>; rel=\"next\", <https://api.github.com/x?page=9>; rel=\"last\"",
        )]);
        assert_eq!(
            next_link(&h).as_deref(),
            Some("https://api.github.com/x?page=3")
        );
        assert_eq!(next_link(&headers(&[("link", "<u>; rel=\"last\"")])), None);
        assert_eq!(next_link(&HeaderMap::new()), None);
    }

    #[test]
    fn retry_after_prefers_header_then_reset() {
        assert_eq!(
            retry_after(&headers(&[("retry-after", "42")])),
            Some(Duration::from_secs(42))
        );
        assert_eq!(
            retry_after(&headers(&[("x-ratelimit-remaining", "5")])),
            None
        );
        let far = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs()
            + 100;
        let d = retry_after(&headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", &far.to_string()),
        ]))
        .expect("some");
        assert!((99..=101).contains(&d.as_secs()));
    }

    #[test]
    fn url_rejects_other_origins() {
        let h = Http::new(Secret::new("t".into()), "https://api.github.com/", false);
        assert_eq!(
            h.url("/user/repos").as_deref(),
            Ok("https://api.github.com/user/repos")
        );
        assert!(h.url("https://api.github.com/x?page=2").is_ok());
        assert!(h.url("https://evil.example/x").is_err());
        assert!(h.url("https://api.github.com.evil.example/x").is_err());
    }

    #[test]
    fn scrub_removes_token() {
        let h = Http::new(
            Secret::new("SECRET123".into()),
            "https://api.github.com",
            false,
        );
        assert_eq!(
            h.scrub("Bearer SECRET123 and SECRET123"),
            "Bearer [redacted] and [redacted]"
        );
    }
}
