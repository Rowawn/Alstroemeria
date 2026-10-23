//! Extension commands and the `alstext://` protocol. Logic lives in `alst-ext`; this only wires it to Tauri.

use crate::{AppState, Res, err};
use alst_ext::host::{self, ExtInfo, ExtensionList, ResolvedView};
use alst_ext::{Registry, Source, install};
use serde::Serialize;
use serde_json::Value;
use tauri::http::{Request as HttpRequest, Response, StatusCode};
use tauri::ipc::Request;
use tauri::{Manager, State, UriSchemeContext, Wry};

/// Reload the registry and language definitions after the set of extensions or their state changed.
fn reload(state: &AppState) -> Res<()> {
    let mut reg = state.registry.write().map_err(err)?;
    *reg = Registry::load(&alst_ext::registry::BUNDLED, Some(&state.ext_root));
    let store = state.store()?;
    *state.languages.write().map_err(err)? = host::language_defs(&reg, &store).map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn list_extensions(state: State<AppState>) -> Res<ExtensionList> {
    let reg = state.registry()?;
    let store = state.store()?;
    host::list(&reg, &store).map_err(err)
}

#[tauri::command]
pub fn resolve_extensions(state: State<AppState>, language: String) -> Res<ResolvedView> {
    let reg = state.registry()?;
    let store = state.store()?;
    Ok(host::resolve_language(&reg, &store, &language, host::platform()).map_err(err)?.1)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    #[serde(flatten)]
    info: ExtInfo,
    /// Version already installed or bundled under this id, if any.
    existing_version: Option<String>,
    file_count: usize,
}

/// Validate a `.alext` without installing it; the UI shows its permissions before `install_extension`.
#[tauri::command]
pub fn inspect_extension(state: State<AppState>, request: Request<'_>) -> Res<PackageInfo> {
    let pkg = install::read_package(&crate::raw_body(&request)?).map_err(err)?;
    let reg = state.registry()?;
    Ok(PackageInfo {
        existing_version: reg.get(&pkg.manifest.id).map(|e| e.manifest.version.clone()),
        file_count: pkg.files.len(),
        info: host::ext_info(&pkg.manifest, Source::Installed, None),
    })
}

#[tauri::command]
pub fn install_extension(state: State<AppState>, request: Request<'_>) -> Res<ExtInfo> {
    let pkg = install::read_package(&crate::raw_body(&request)?).map_err(err)?;
    {
        let reg = state.registry()?;
        if let Some(b) = reg.get(&pkg.manifest.id).filter(|e| e.source == Source::Bundled) {
            if b.manifest.semver() >= pkg.manifest.semver() {
                return Err(format!(
                    "'{}' is bundled with the app at version {}; install a newer version to replace it",
                    pkg.manifest.id, b.manifest.version
                ));
            }
        }
    }
    install::install(&state.ext_root, &pkg).map_err(err)?;
    // Installing is the user's explicit approval: enable it, keeping any settings from an earlier install.
    state.store()?.set_ext_state(&pkg.manifest.id, Some(true), None, None).map_err(err)?;
    reload(&state)?;
    let store = state.store()?;
    let states = host::user_states(&store).map_err(err)?;
    Ok(host::ext_info(&pkg.manifest, Source::Installed, states.get(&pkg.manifest.id)))
}

#[tauri::command]
pub fn uninstall_extension(state: State<AppState>, id: String) -> Res<()> {
    {
        let reg = state.registry()?;
        match reg.get(&id) {
            None => return Err(format!("no extension '{id}'")),
            Some(e) if e.source == Source::Bundled => return Err("bundled extensions can be disabled, not uninstalled".into()),
            Some(_) => {}
        }
    }
    install::uninstall(&state.ext_root, &id).map_err(err)?;
    // Keep the row (disabled) so settings survive a reinstall (SPEC §6.9.2).
    state.store()?.set_ext_state(&id, Some(false), None, None).map_err(err)?;
    reload(&state)
}

fn known(state: &AppState, id: &str) -> Res<()> {
    state.registry()?.get(id).map(|_| ()).ok_or_else(|| format!("no extension '{id}'"))
}

#[tauri::command]
pub fn set_extension_enabled(state: State<AppState>, id: String, enabled: bool) -> Res<()> {
    known(&state, &id)?;
    state.store()?.set_ext_state(&id, Some(enabled), None, None).map_err(err)?;
    reload(&state)
}

#[tauri::command]
pub fn set_extension_languages(state: State<AppState>, id: String, languages: Vec<String>) -> Res<()> {
    known(&state, &id)?;
    state.store()?.set_ext_state(&id, None, None, Some(&languages)).map_err(err)
}

#[tauri::command]
pub fn set_extension_settings(state: State<AppState>, id: String, settings: Value) -> Res<()> {
    let reg = state.registry()?;
    let ext = reg.get(&id).ok_or_else(|| format!("no extension '{id}'"))?;
    alst_ext::validate_settings(&ext.manifest, &settings).map_err(err)?;
    state.store()?.set_ext_state(&id, None, Some(&settings), None).map_err(err)
}

#[tauri::command]
pub fn set_provider(state: State<AppState>, language: String, capability: String, ext_id: String) -> Res<()> {
    let reg = state.registry()?;
    let mut store = state.store()?;
    host::set_provider(&reg, &mut store, &language, &capability, &ext_id).map_err(err)
}

#[tauri::command]
pub fn set_decorator_order(state: State<AppState>, language: String, ids: Vec<String>) -> Res<()> {
    let mut store = state.store()?;
    host::set_decorator_order(&mut store, &language, &ids).map_err(err)
}

/// Host-API entry point for extensions. The UI host passes the id of the worker (or panel) the call
/// came from; permissions are checked here (SPEC §6.9.4).
#[tauri::command]
pub async fn ext_call(app: tauri::AppHandle, state: State<'_, AppState>, ext_id: String, method: String, params: Value) -> Res<Value> {
    if host::is_net_call(&method) {
        let manifest = {
            let reg = state.registry()?;
            let store = state.store()?;
            host::manifest_for_call(&reg, &store, &ext_id).map_err(err)?
        };
        return tauri::async_runtime::spawn_blocking(move || host::net_fetch(&manifest, &params))
            .await
            .map_err(err)?
            .map_err(err);
    }
    let reg = state.registry()?;
    let languages = state.languages.read().map_err(err)?;
    let mut store = state.store()?;
    let result = host::call(&mut host::HostCtx { registry: &reg, store: &mut store, languages: &languages }, &ext_id, &method, &params)
        .map_err(err);
    state.forward_events(&app, &store);
    result
}

// ---- alstext:// ------------------------------------------------------------------------------

/// CSP for extension documents (panels). They can't reach the network, the app, or each other.
const PANEL_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' alstext: http://alstext.localhost; \
style-src 'unsafe-inline' alstext: http://alstext.localhost; img-src data: alstext: http://alstext.localhost; \
connect-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'";

fn respond(status: StatusCode, mime: &str, body: Vec<u8>, html: bool) -> Response<Vec<u8>> {
    let mut b = Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Access-Control-Allow-Origin", "*")
        .header("Cross-Origin-Resource-Policy", "cross-origin")
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff");
    if html {
        b = b.header("Content-Security-Policy", PANEL_CSP);
    }
    b.body(body).expect("static response")
}

/// `alstext://<id>/<path>` on macOS/Linux/iOS, `http://alstext.localhost/<id>/<path>` on Windows/Android.
fn split(uri: &tauri::http::Uri) -> Option<(String, String)> {
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    // Android rewrites `http://alstext.localhost/<id>/<path>` to `alstext://localhost/<id>/<path>`.
    if uri.scheme_str() == Some("alstext") && uri.host() != Some("localhost") {
        Some((uri.host()?.to_string(), decode(uri.path().trim_start_matches('/'))))
    } else {
        let path = uri.path().trim_start_matches('/');
        let (id, rest) = path.split_once('/')?;
        Some((decode(id), decode(rest)))
    }
}

pub fn serve(ctx: UriSchemeContext<'_, Wry>, request: HttpRequest<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() == tauri::http::Method::OPTIONS {
        return respond(StatusCode::NO_CONTENT, "text/plain", vec![], false);
    }
    let Some(state) = ctx.app_handle().try_state::<AppState>() else {
        return respond(StatusCode::SERVICE_UNAVAILABLE, "text/plain", b"starting".to_vec(), false);
    };
    let Some((id, path)) = split(request.uri()) else {
        return respond(StatusCode::BAD_REQUEST, "text/plain", b"bad extension URL".to_vec(), false);
    };
    let Ok(reg) = state.registry.read() else {
        return respond(StatusCode::INTERNAL_SERVER_ERROR, "text/plain", vec![], false);
    };
    match reg.read_file(&id, &path) {
        Ok((bytes, mime)) => respond(StatusCode::OK, mime, bytes, mime.starts_with("text/html")),
        Err(e) => respond(StatusCode::NOT_FOUND, "text/plain", e.to_string().into_bytes(), false),
    }
}
