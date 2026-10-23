//! Permissions (SPEC §6.9.4): parsing, host matching and the checker the shell calls on every host-API call.

use crate::Error;
use crate::manifest::Manifest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permission {
    TermsRead,
    TermsWrite,
    EventsRead,
    DictRead,
    /// Exact host (`example.org`) or subdomain wildcard (`*.example.org`).
    Net(String),
}

/// What a host call needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need<'a> {
    TermsRead,
    TermsWrite,
    EventsRead,
    DictRead,
    Net(&'a str),
}

impl Permission {
    pub fn parse(s: &str) -> std::result::Result<Self, Error> {
        match s {
            "terms:read" => Ok(Self::TermsRead),
            "terms:write" => Ok(Self::TermsWrite),
            "events:read" => Ok(Self::EventsRead),
            "dict:read" => Ok(Self::DictRead),
            _ => match s.strip_prefix("net:") {
                Some(host) if valid_pattern(host) => Ok(Self::Net(host.to_ascii_lowercase())),
                Some(host) => Err(Error::Manifest(format!("invalid net permission host '{host}'"))),
                None => Err(Error::Manifest(format!("unknown permission '{s}'"))),
            },
        }
    }

    /// One-line explanation for the install prompt.
    pub fn describe(s: &str) -> String {
        match s {
            "terms:read" => "Read your vocabulary (terms, statuses, translations)".into(),
            "terms:write" => "Change your vocabulary (statuses, translations, notes)".into(),
            "events:read" => "Read your activity history (reading and lookup events)".into(),
            "dict:read" => "Look words up in your dictionaries".into(),
            _ => match s.strip_prefix("net:") {
                Some(h) if h.starts_with("*.") => format!("Connect to any subdomain of {} over the internet", &h[2..]),
                Some(h) => format!("Connect to {h} over the internet"),
                None => s.to_string(),
            },
        }
    }

    pub fn is_warning(s: &str) -> bool {
        s == "terms:write" || s.starts_with("net:")
    }
}

fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn valid_pattern(p: &str) -> bool {
    match p.strip_prefix("*.") {
        Some(rest) => rest.contains('.') && valid_host(rest),
        None => valid_host(p),
    }
}

/// Does `pattern` (`example.org` or `*.example.org`) cover `host`? The wildcard doesn't cover the apex.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let (pattern, host) = (pattern.to_ascii_lowercase(), host.to_ascii_lowercase());
    match pattern.strip_prefix("*.") {
        Some(rest) => host.len() > rest.len() + 1 && host.ends_with(&format!(".{rest}")),
        None => host == pattern,
    }
}

pub fn allows(manifest: &Manifest, need: &Need) -> bool {
    manifest.permissions().iter().any(|p| match (p, need) {
        (Permission::TermsRead, Need::TermsRead)
        | (Permission::TermsWrite, Need::TermsWrite)
        | (Permission::EventsRead, Need::EventsRead)
        | (Permission::DictRead, Need::DictRead) => true,
        // Writing implies the ability to read what you're allowed to change.
        (Permission::TermsWrite, Need::TermsRead) => true,
        (Permission::Net(pat), Need::Net(host)) => host_matches(pat, host),
        _ => false,
    })
}

/// Extract the host of an `https://` URL, rejecting anything that could smuggle another host
/// (userinfo, backslashes, whitespace, IP literals).
pub fn https_host(url: &str) -> std::result::Result<String, Error> {
    let bad = |m: &str| Err(Error::Permission(format!("{m}: {url}")));
    let Some(rest) = url.strip_prefix("https://") else { return bad("only https:// URLs are allowed") };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains(['@', '\\', ' ', '\t', '\n', '[', ']']) {
        return bad("invalid URL authority");
    }
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => h,
        Some(_) => return bad("invalid port"),
        None => authority,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if !valid_host(&host) || host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return bad("invalid host");
    }
    Ok(host)
}

/// Check that `manifest` may fetch `url`; returns the host.
pub fn check_net(manifest: &Manifest, url: &str) -> std::result::Result<String, Error> {
    let host = https_host(url)?;
    if allows(manifest, &Need::Net(&host)) {
        Ok(host)
    } else {
        Err(Error::Permission(format!("'{}' has no net permission for {host}", manifest.id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(perms: &[&str]) -> Manifest {
        Manifest::parse(
            &serde_json::json!({
                "id": "addon.t", "name": "T", "version": "1.0.0", "kind": "addon", "license": "MIT",
                "apiVersion": "^1.0", "targets": ["*"], "runtime": { "type": "js", "entry": "m.js" },
                "permissions": perms
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn wildcard_covers_subdomains_not_apex() {
        assert!(host_matches("*.example.org", "a.example.org"));
        assert!(host_matches("*.example.org", "a.b.example.org"));
        assert!(!host_matches("*.example.org", "example.org"));
        assert!(!host_matches("*.example.org", "badexample.org"));
        assert!(!host_matches("*.example.org", "example.org.evil.com"));
        assert!(host_matches("Example.org", "EXAMPLE.org"));
        assert!(!host_matches("example.org", "www.example.org"));
    }

    #[test]
    fn parse_permissions() {
        assert_eq!(Permission::parse("terms:read").unwrap(), Permission::TermsRead);
        assert_eq!(Permission::parse("net:*.Example.org").unwrap(), Permission::Net("*.example.org".into()));
        for bad in ["net:*", "net:", "net:a b.com", "net:*.com", "net:https://x.org", "audio", "net:x.org/path"] {
            assert!(Permission::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn checker_matches_needs() {
        let m = manifest(&["terms:read", "net:api.example.org", "net:*.cdn.example.org"]);
        assert!(allows(&m, &Need::TermsRead));
        assert!(!allows(&m, &Need::TermsWrite));
        assert!(!allows(&m, &Need::EventsRead));
        assert!(!allows(&m, &Need::DictRead));
        assert!(allows(&m, &Need::Net("api.example.org")));
        assert!(allows(&m, &Need::Net("x.cdn.example.org")));
        assert!(!allows(&m, &Need::Net("example.org")));
        let w = manifest(&["terms:write"]);
        assert!(allows(&w, &Need::TermsRead) && allows(&w, &Need::TermsWrite));
        assert!(!allows(&manifest(&[]), &Need::TermsRead));
    }

    #[test]
    fn net_urls_are_https_only_and_unambiguous() {
        let m = manifest(&["net:api.example.org"]);
        assert_eq!(check_net(&m, "https://api.example.org/v1?q=1").unwrap(), "api.example.org");
        assert_eq!(check_net(&m, "https://API.example.org:8443/").unwrap(), "api.example.org");
        for bad in [
            "http://api.example.org/",
            "https://evil.org/",
            "https://api.example.org@evil.org/",
            "https://evil.org\\@api.example.org/",
            "https://api.example.org.evil.org/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "ftp://api.example.org",
            "//api.example.org",
            "https:///x",
        ] {
            assert!(check_net(&m, bad).is_err(), "{bad}");
        }
    }
}
