use crate::{download, network::SafeClient, sources};
use anyhow::{Context, Result};
use futures_util::FutureExt;
use serde::Serialize;
use std::{sync::Mutex, time::Duration};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
struct Update {
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}
impl Update {
    fn new(phase: &'static str) -> Self {
        Self {
            phase,
            filename: None,
            bytes: None,
            total: None,
            message: None,
        }
    }
}

struct Session {
    cancel: CancellationToken,
    confirmation: Option<oneshot::Sender<bool>>,
}
#[derive(Default)]
struct Controller(Mutex<Option<Session>>);

fn emit(app: &tauri::AppHandle, update: Update) -> Result<()> {
    app.emit("download-state", update)
        .context("Cannot update the application window")
}

#[tauri::command]
fn start_download(
    app: tauri::AppHandle,
    state: tauri::State<'_, Controller>,
) -> Result<(), String> {
    let cancel = CancellationToken::new();
    {
        let mut active = state
            .0
            .lock()
            .map_err(|_| "Download controller unavailable")?;
        if active.is_some() {
            return Err("A download is already running".into());
        }
        *active = Some(Session {
            cancel: cancel.clone(),
            confirmation: None,
        });
    }
    tauri::async_runtime::spawn(async move {
        let outcome = std::panic::AssertUnwindSafe(async {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Ok(None),
                result = perform(&app, &cancel) => result.map(Some),
            }
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("The download task stopped unexpectedly")));
        // Clear the lock before reporting completion so a fast next activation
        // cannot race with cleanup from the preceding operation.
        if let Ok(mut active) = app.state::<Controller>().0.lock() {
            *active = None;
        }
        let update = match outcome {
            Ok(Some(receipt)) if !cancel.is_cancelled() => {
                let mut update = Update::new("complete");
                update.filename = receipt
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                update.bytes = Some(receipt.bytes);
                update
            }
            Ok(_) => Update::new("idle"),
            Err(_) if cancel.is_cancelled() => Update::new("idle"),
            Err(error) => {
                // Diagnostic details stay in the native console. Never put an
                // untrusted URL/response body into the interface.
                eprintln!("BOTÓN download failed: {error:#}");
                let mut update = Update::new("error");
                update.message = Some("Nothing arrived this time. Try again.".into());
                update
            }
        };
        if let Err(error) = emit(&app, update) {
            eprintln!("{error:#}");
        }
    });
    Ok(())
}

async fn perform(app: &tauri::AppHandle, cancel: &CancellationToken) -> Result<download::Receipt> {
    emit(app, Update::new("discovering"))?;
    let client = SafeClient::new()?;
    let candidate = tokio::time::timeout(Duration::from_secs(90), sources::discover(&client))
        .await
        .context("Discovery timed out")??;
    let prepared = download::prepare(&client, candidate).await?;
    if download::requires_confirmation(prepared.total) {
        let (sender, receiver) = oneshot::channel();
        {
            let state = app.state::<Controller>();
            let mut active = state
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Download controller unavailable"))?;
            active.as_mut().context("Download cancelled")?.confirmation = Some(sender);
        }
        let mut update = Update::new("confirming");
        update.total = prepared.total;
        emit(app, update)?;
        let accepted = tokio::time::timeout(Duration::from_secs(120), receiver).await;
        if !matches!(accepted, Ok(Ok(true))) {
            cancel.cancel();
            anyhow::bail!("Download cancelled");
        }
    }
    let directory = app
        .path()
        .download_dir()
        .context("The system Downloads directory is unavailable")?;
    let mut update = Update::new("downloading");
    update.total = prepared.total;
    update.bytes = Some(0);
    emit(app, update)?;
    download::download(prepared, &directory, cancel, |bytes, total| {
        let mut update = Update::new("downloading");
        update.bytes = Some(bytes);
        update.total = total;
        if let Err(error) = emit(app, update) {
            eprintln!("{error:#}");
            cancel.cancel();
        }
    })
    .await
}

#[tauri::command]
fn confirm_download(accept: bool, state: tauri::State<'_, Controller>) -> Result<(), String> {
    let mut active = state
        .0
        .lock()
        .map_err(|_| "Download controller unavailable")?;
    let session = active.as_mut().ok_or("No active download")?;
    let sender = session
        .confirmation
        .take()
        .ok_or("No confirmation is pending")?;
    sender
        .send(accept)
        .map_err(|_| "Confirmation expired".to_owned())
}

#[tauri::command]
fn cancel_download(state: tauri::State<'_, Controller>) -> Result<(), String> {
    let active = state
        .0
        .lock()
        .map_err(|_| "Download controller unavailable")?;
    if let Some(session) = active.as_ref() {
        session.cancel.cancel();
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .manage(Controller::default())
        .invoke_handler(tauri::generate_handler![
            start_download,
            confirm_download,
            cancel_download
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<Controller>();
                let running = if let Ok(active) = state.0.lock() {
                    if let Some(session) = active.as_ref() {
                        session.cancel.cancel();
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if running {
                    // Keep the runtime alive until the task drops its tempfile.
                    api.prevent_close();
                    let window = window.clone();
                    tauri::async_runtime::spawn(async move {
                        loop {
                            let busy = window
                                .state::<Controller>()
                                .0
                                .lock()
                                .map(|s| s.is_some())
                                .unwrap_or(false);
                            if !busy {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        if let Err(error) = window.destroy() {
                            eprintln!("Cannot close BOTÓN: {error}");
                        }
                    });
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Unable to start BOTÓN");
}
