pub mod card;
pub mod pdf;
mod raw;
pub mod render;
mod stamp;

use std::sync::Mutex;

use serde::Serialize;
use tauri::async_runtime::spawn_blocking;
use tauri::ipc::Response;
use tauri::{Emitter, Manager, RunEvent, State};

/// A PDF opened from Finder (Open With, or dropped on the app icon), kept until the window asks
/// for it in case it arrives before the window is ready.
#[derive(Default)]
struct Opened(Mutex<Option<String>>);

#[derive(Serialize)]
struct Signed {
    output: String,
    signed_at: String,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    spawn_blocking(work).await.map_err(|e| e.to_string())?
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("Could not open {path}: {e}"))
}

/// Writes next to the target and renames, so a failed write never leaves half a PDF.
fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    let partial = format!("{path}.partial");
    std::fs::write(&partial, bytes)
        .and_then(|_| std::fs::rename(&partial, path))
        .map_err(|e| format!("Could not save {path}: {e}"))
}

#[tauri::command]
async fn list_identities() -> Result<Vec<card::Identity>, String> {
    blocking(card::list_identities).await
}

#[tauri::command]
async fn inspect_pdf(path: String) -> Result<pdf::Info, String> {
    blocking(move || pdf::inspect(&read(&path)?)).await
}

#[tauri::command]
async fn render_page(path: String, page: usize, width: usize) -> Result<Response, String> {
    blocking(move || render::page_png(&read(&path)?, page, width.clamp(100, 4000)).map(Response::new)).await
}

#[tauri::command]
async fn sign_pdf(
    input: String,
    output: String,
    field: String,
    identity: String,
    new_fields: Vec<pdf::NewField>,
) -> Result<Signed, String> {
    blocking(move || {
        let bytes = read(&input)?;
        let (key, info) = card::find(&identity)?;
        let (signed, signed_at) = pdf::sign(
            &bytes,
            &new_fields,
            &field,
            &info.name,
            || card::unlock(&key),
            |data| card::sign_detached(&key, data),
        )?;
        write(&output, &signed)?;
        Ok(Signed { output, signed_at })
    })
    .await
}

#[tauri::command]
async fn save_fields(input: String, output: String, new_fields: Vec<pdf::NewField>) -> Result<String, String> {
    blocking(move || {
        let updated = pdf::add_fields(&read(&input)?, &new_fields)?;
        write(&output, &updated)?;
        Ok(output)
    })
    .await
}

#[tauri::command]
fn opened_file(opened: State<Opened>) -> Option<String> {
    opened.0.lock().unwrap().take()
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Opened::default())
        .invoke_handler(tauri::generate_handler![
            list_identities,
            inspect_pdf,
            render_page,
            sign_pdf,
            save_fields,
            opened_file,
            reveal
        ])
        .build(tauri::generate_context!())
        .expect("error while starting CAC Signer");
    app.run(|app, event| {
        if let RunEvent::Opened { urls } = event {
            if let Some(path) = urls.iter().find_map(|url| url.to_file_path().ok()) {
                let path = path.to_string_lossy().into_owned();
                *app.state::<Opened>().0.lock().unwrap() = Some(path.clone());
                let _ = app.emit("open-pdf", path);
            }
        }
    });
}
