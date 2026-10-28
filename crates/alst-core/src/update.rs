//! Update checks, the parts that are pure logic (SPEC §17.4): version comparison, the `latest.json` feed and the
//! "check automatically" setting. Network and installation live in the app shell.

use crate::{Error, Result};
use rusqlite::Connection;
use serde::Deserialize;

/// `major.minor.patch` with an optional pre-release (`-rc1`); a leading `v` is accepted.
fn parse_version(s: &str) -> Option<([u64; 3], Option<&str>)> {
    let s = s.trim().trim_start_matches('v');
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let mut nums = core.split('.');
    let v = [nums.next()?.parse().ok()?, nums.next()?.parse().ok()?, nums.next()?.parse().ok()?];
    nums.next().is_none().then_some((v, pre))
}

/// Whether `candidate` is a newer version than `current`. A pre-release sorts before its release.
/// Anything that isn't a version is never newer.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    let (Some((c, cp)), Some((n, np))) = (parse_version(current), parse_version(candidate)) else { return false };
    match n.cmp(&c) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (cp, np) {
            (Some(_), None) => true,
            (Some(a), Some(b)) => b > a,
            _ => false,
        },
    }
}

/// The parts of `latest.json` the app reads (Tauri's updater feed; the desktop updater verifies signatures itself).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Latest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: Option<String>,
}

pub fn parse_latest(json: &str) -> Result<Latest> {
    let l: Latest = serde_json::from_str(json).map_err(|e| Error::Invalid(format!("update feed: {e}")))?;
    if parse_version(&l.version).is_none() {
        return Err(Error::Invalid(format!("update feed has a bad version: {}", l.version)));
    }
    Ok(l)
}

/// The feed's newer release, if any.
pub fn newer_than(current: &str, feed_json: &str) -> Result<Option<Latest>> {
    let l = parse_latest(feed_json)?;
    Ok(is_newer(current, &l.version).then_some(l))
}

/// `https://github.com/o/r/releases/latest/download/latest.json` → the release page of `version`.
pub fn release_page(feed_url: &str, version: &str) -> Option<String> {
    let base = feed_url.split("/releases/").next().filter(|b| b.starts_with("https://github.com/"))?;
    Some(format!("{base}/releases/tag/v{}", version.trim_start_matches('v')))
}

const KEY: &str = "update_check";

/// "Check automatically" (default on), kept per device.
pub fn auto_check(conn: &Connection) -> bool {
    conn.query_row("SELECT value FROM local_settings WHERE key = ?1", [KEY], |r| r.get::<_, String>(0)).map_or(true, |v| v != "off")
}

pub fn set_auto_check(conn: &Connection, on: bool) -> Result<()> {
    conn.execute("INSERT OR REPLACE INTO local_settings (key, value) VALUES (?1, ?2)", [KEY, if on { "on" } else { "off" }])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(is_newer("0.1.0", "v0.1.1"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.2.0", "0.1.9"));
        assert!(!is_newer("0.10.0", "0.9.0"), "numeric, not lexical");
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("0.2.0-rc1", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0-rc1"));
        assert!(is_newer("0.2.0-rc1", "0.2.0-rc2"));
        assert!(!is_newer("0.1.0", "garbage"));
        assert!(!is_newer("0.1.0", "1.0"));
        assert!(!is_newer("0.1.0", ""));
    }

    #[test]
    fn feed_parsing() {
        let feed = r#"{"version":"0.2.0","notes":"- faster","pub_date":"2026-11-01T00:00:00Z","platforms":{"darwin-x86_64":{"signature":"x","url":"https://u"}}}"#;
        assert_eq!(newer_than("0.1.0", feed).unwrap().unwrap().notes, "- faster");
        assert_eq!(newer_than("0.2.0", feed).unwrap(), None);
        assert!(parse_latest("not json").is_err());
        assert!(parse_latest(r#"{"version":"soon"}"#).is_err());
    }

    #[test]
    fn release_page_from_feed_url() {
        let feed = "https://github.com/me/alst/releases/latest/download/latest.json";
        assert_eq!(release_page(feed, "0.2.0").as_deref(), Some("https://github.com/me/alst/releases/tag/v0.2.0"));
        assert_eq!(release_page("https://example.com/latest.json", "0.2.0"), None);
    }

    #[test]
    fn auto_check_defaults_on() {
        let c = Connection::open_in_memory().unwrap();
        c.execute("CREATE TABLE local_settings (key TEXT PRIMARY KEY, value TEXT)", []).unwrap();
        assert!(auto_check(&c));
        set_auto_check(&c, false).unwrap();
        assert!(!auto_check(&c));
        set_auto_check(&c, true).unwrap();
        assert!(auto_check(&c));
    }
}
