//! Update checks (SPEC §17.4). Desktop uses `tauri-plugin-updater` (signature-checked install, then restart);
//! Android has no self-update and only learns that a newer version exists. Version and feed logic is in
//! `alst_core::update`.

use crate::{AppState, Res, err};
use alst_core::update as core_update;
use serde::Serialize;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    auto_check: bool,
    can_install: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    version: String,
    notes: String,
    can_install: bool,
    page_url: Option<String>,
}

const CAN_INSTALL: bool = cfg!(not(any(target_os = "android", target_os = "ios")));

/// The feed URL from `plugins.updater.endpoints` in tauri.conf.json.
fn feed_url(app: &tauri::AppHandle) -> Option<String> {
    app.config().plugins.0.get("updater")?.get("endpoints")?.get(0)?.as_str().map(String::from)
}

#[tauri::command]
pub fn update_info(app: tauri::AppHandle, state: State<AppState>) -> Res<UpdateInfo> {
    let auto_check = core_update::auto_check(state.store()?.local_conn());
    Ok(UpdateInfo { version: app.package_info().version.to_string(), auto_check, can_install: CAN_INSTALL })
}

#[tauri::command]
pub fn update_set_auto(state: State<AppState>, on: bool) -> Res<()> {
    core_update::set_auto_check(state.store()?.local_conn(), on).map_err(err)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
pub async fn update_check(app: tauri::AppHandle) -> Res<Option<Available>> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app.updater().map_err(err)?.check().await.map_err(err)?;
    let feed = feed_url(&app);
    Ok(update.map(|u| Available {
        page_url: feed.as_deref().and_then(|f| core_update::release_page(f, &u.version)),
        version: u.version,
        notes: u.body.unwrap_or_default(),
        can_install: true,
    }))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
pub async fn update_check(app: tauri::AppHandle) -> Res<Option<Available>> {
    let feed = feed_url(&app).ok_or("no update feed is configured")?;
    let current = app.package_info().version.to_string();
    let url = feed.clone();
    let json = tauri::async_runtime::spawn_blocking(move || {
        alst_text::import::rss::fetch_bytes(&url, 1 << 20, std::time::Duration::from_secs(20)).map(|b| String::from_utf8_lossy(&b).into_owned())
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    Ok(core_update::newer_than(&current, &json).map_err(err)?.map(|l| Available {
        page_url: core_update::release_page(&feed, &l.version),
        version: l.version,
        notes: l.notes,
        can_install: false,
    }))
}

/// Download the newer version, verify its signature against the key in the app, install it and restart.
/// Never runs without the user pressing the button.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tauri::command]
pub async fn update_install(app: tauri::AppHandle) -> Res<()> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app.updater().map_err(err)?.check().await.map_err(err)?.ok_or("already up to date")?;
    update.download_and_install(|_, _| {}, || {}).await.map_err(err)?;
    app.restart()
}

#[cfg(any(target_os = "android", target_os = "ios"))]
#[tauri::command]
pub async fn update_install() -> Res<()> {
    Err("this device updates by downloading the new version".into())
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use tauri_plugin_updater::UpdaterExt;

    const PUBKEY: &str = include_str!("../tests/fixtures/update-test.pub");
    const PAYLOAD: &[u8] = include_bytes!("../tests/fixtures/payload.bin");
    const PAYLOAD_SIG: &str = include_str!("../tests/fixtures/payload.bin.sig");
    const OTHER_SIG: &str = include_str!("../tests/fixtures/other.bin.sig");

    /// Serve `routes` (path → body) on `l` in the background.
    fn serve(l: TcpListener, routes: HashMap<&'static str, Vec<u8>>) {
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut line = String::new();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let _ = r.read_line(&mut line);
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/");
                let (status, body) = match routes.get(path) {
                    Some(b) => ("200 OK", b.clone()),
                    None => ("404 Not Found", vec![]),
                };
                let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(&body);
            }
        });
    }

    /// Run the real updater plugin against a feed whose artifact carries `signature`; returns the downloaded bytes.
    fn run(current: &str, feed_version: &str, signature: &str) -> Result<Option<Vec<u8>>, String> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let feed = serde_json::json!({
            "version": feed_version,
            "notes": "n",
            "platforms": { "test-target": { "signature": signature, "url": format!("{base}/payload") } }
        });
        let routes = HashMap::from([("/payload", PAYLOAD.to_vec()), ("/latest.json", feed.to_string().into_bytes())]);
        serve(listener, routes);

        let mut ctx = tauri::test::mock_context(tauri::test::noop_assets());
        ctx.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({ "pubkey": PUBKEY.trim(), "endpoints": [format!("{base}/latest.json")], "dangerousInsecureTransportProtocol": true }),
        );
        let app = tauri::test::mock_builder().plugin(tauri_plugin_updater::Builder::new().build()).build(ctx).map_err(|e| e.to_string())?;
        tauri::async_runtime::block_on(async {
            let updater = app
                .handle()
                .updater_builder()
                .version_comparator({
                    let current = current.to_string();
                    move |_, remote| alst_core::update::is_newer(&current, &remote.version.to_string())
                })
                .target("test-target")
                .build()
                .map_err(|e| e.to_string())?;
            match updater.check().await.map_err(|e| e.to_string())? {
                None => Ok(None),
                Some(u) => u.download(|_, _| {}, || {}).await.map(Some).map_err(|e| e.to_string()),
            }
        })
    }

    #[test]
    fn a_correctly_signed_update_downloads() {
        assert_eq!(run("0.1.0", "0.2.0", PAYLOAD_SIG).unwrap().as_deref(), Some(PAYLOAD));
    }

    #[test]
    fn a_feed_with_a_bad_signature_is_refused() {
        // The signature belongs to a different file.
        let err = run("0.1.0", "0.2.0", OTHER_SIG).unwrap_err();
        assert!(err.to_lowercase().contains("signature") || err.to_lowercase().contains("minisign") || err.to_lowercase().contains("verif"), "{err}");
        // Not a signature at all.
        assert!(run("0.1.0", "0.2.0", "AAAA").is_err());
    }

    #[test]
    fn the_same_or_an_older_version_offers_nothing() {
        assert_eq!(run("0.2.0", "0.2.0", PAYLOAD_SIG).unwrap(), None);
        assert_eq!(run("0.3.0", "0.2.0", PAYLOAD_SIG).unwrap(), None);
    }
}
