//! Thin Tauri shell: commands delegate to `alst-core`.

use alst_core::{NewWork, Store, Work};
use std::sync::Mutex;
use tauri::Manager;

struct AppState(Mutex<Store>);

#[tauri::command]
fn create_work(state: tauri::State<AppState>, title: String, text: String) -> Result<Work, String> {
    let mut store = state.0.lock().map_err(|e| e.to_string())?;
    store
        .create_work(NewWork { language: "und".into(), title, text })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_works(state: tauri::State<AppState>) -> Result<Vec<Work>, String> {
    let store = state.0.lock().map_err(|e| e.to_string())?;
    store.list_works().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let store = Store::open(&dir.join("alstroemeria.db"))?;
            app.manage(AppState(Mutex::new(store)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![create_work, list_works])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
