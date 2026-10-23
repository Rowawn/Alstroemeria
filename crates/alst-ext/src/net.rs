//! `alst.net.fetch` (SPEC §6.9.4): HTTPS only, hosts from the manifest, every redirect re-checked.

use crate::manifest::Manifest;
use crate::perm::{check_net, https_host};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

pub const MAX_RESPONSE_BYTES: u64 = 10 * 1024 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REDIRECTS: usize = 5;
const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];
const FORBIDDEN_HEADERS: &[&str] = &["cookie", "set-cookie", "host", "connection", "content-length", "transfer-encoding", "upgrade"];

#[derive(Debug, Clone, Deserialize)]
pub struct FetchRequest {
    pub url: String,
    pub method: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub text: String,
}

fn net_err(e: impl std::fmt::Display) -> Error {
    Error::Net(e.to_string())
}

/// Resolve a `Location` header against the URL it came from. Only absolute `https://` and
/// host-relative (`/path`) targets are followed.
fn resolve_location(current: &str, location: &str) -> Result<String> {
    if location.starts_with("https://") {
        return Ok(location.to_string());
    }
    if location.starts_with('/') && !location.starts_with("//") {
        let host = https_host(current)?;
        let rest = &current["https://".len()..];
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let _ = host;
        return Ok(format!("https://{}{location}", &rest[..authority_end]));
    }
    Err(Error::Permission(format!("redirect to a disallowed location: {location}")))
}

pub fn fetch(manifest: &Manifest, req: &FetchRequest) -> Result<FetchResponse> {
    let mut method = req.method.as_deref().unwrap_or("GET").to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return Err(Error::Invalid(format!("unsupported method {method}")));
    }
    for k in req.headers.keys() {
        if FORBIDDEN_HEADERS.contains(&k.to_ascii_lowercase().as_str()) {
            return Err(Error::Invalid(format!("header '{k}' is not allowed")));
        }
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let mut url = req.url.clone();
    let mut body = req.body.clone().unwrap_or_default();
    for _ in 0..=MAX_REDIRECTS {
        check_net(manifest, &url)?;
        let mut builder = ureq::http::Request::builder().method(method.as_str()).uri(&url);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        let request = builder.body(body.clone()).map_err(net_err)?;
        let mut resp = agent.run(request).map_err(net_err)?;
        let status = resp.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(String::from);
            let Some(loc) = loc else { return Err(net_err("redirect without a Location header")) };
            url = resolve_location(&url, &loc)?;
            if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                method = "GET".into();
                body.clear();
            }
            continue;
        }
        let headers = resp
            .headers()
            .iter()
            .filter(|(k, _)| k.as_str() != "set-cookie")
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string())))
            .collect();
        let bytes = resp.body_mut().with_config().limit(MAX_RESPONSE_BYTES).read_to_vec().map_err(net_err)?;
        return Ok(FetchResponse { status, headers, text: String::from_utf8_lossy(&bytes).into_owned() });
    }
    Err(net_err("too many redirects"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest::parse(
            &serde_json::json!({
                "id": "addon.net", "name": "N", "version": "1.0.0", "kind": "addon", "license": "MIT",
                "apiVersion": "^1.0", "targets": ["*"], "runtime": { "type": "js", "entry": "m.js" },
                "permissions": ["net:allowed.example.org"]
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn redirect_targets_are_restricted() {
        assert_eq!(resolve_location("https://a.org/x?y", "/z").unwrap(), "https://a.org/z");
        assert_eq!(resolve_location("https://a.org:8443/x", "/z").unwrap(), "https://a.org:8443/z");
        assert_eq!(resolve_location("https://a.org/x", "https://b.org/").unwrap(), "https://b.org/");
        assert!(resolve_location("https://a.org/x", "//evil.org/").is_err());
        assert!(resolve_location("https://a.org/x", "http://a.org/").is_err());
        assert!(resolve_location("https://a.org/x", "relative").is_err());
    }

    #[test]
    fn refuses_before_any_request_is_made() {
        let m = manifest();
        let req = |url: &str| FetchRequest { url: url.into(), method: None, headers: Default::default(), body: None };
        for url in ["http://allowed.example.org/", "https://other.example.org/", "https://allowed.example.org@evil.org/"] {
            assert!(matches!(fetch(&m, &req(url)), Err(Error::Permission(_))), "{url}");
        }
        let mut r = req("https://allowed.example.org/");
        r.method = Some("TRACE".into());
        assert!(matches!(fetch(&m, &r), Err(Error::Invalid(_))));
        let mut r = req("https://allowed.example.org/");
        r.headers.insert("Cookie".into(), "a=b".into());
        assert!(matches!(fetch(&m, &r), Err(Error::Invalid(_))));
    }
}
