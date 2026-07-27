use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use crate::core::constants;

pub struct FileWatcher {
    _watcher: Option<RecommendedWatcher>,
    watched_path: Option<PathBuf>,
}

impl FileWatcher {
    pub fn new() -> Self {
        FileWatcher {
            _watcher: None,
            watched_path: None,
        }
    }

    pub fn start_watching(&mut self, app_handle: AppHandle, game_mods_path: &Path) -> Result<()> {
        self.stop_watching();

        let managed_path = game_mods_path.join(constants::MANAGED_FOLDER);
        if !managed_path.exists() {
            std::fs::create_dir_all(&managed_path)?;
        }

        let (tx, rx) = mpsc::channel();

        let mut watcher = notify::recommended_watcher(move |res| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })?;

        watcher.watch(&managed_path, RecursiveMode::Recursive)?;

        let managed_path_clone = managed_path.clone();
        let debounce_duration = Duration::from_millis(constants::FILE_WATCHER_DEBOUNCE_MS);
        std::thread::spawn(move || {
            let mut last_event_time: Option<Instant> = None;
            let mut pending = false;

            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        use notify::EventKind;
                        let should_trigger = matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(_)
                        );

                        if should_trigger {
                            let backup_suffix = format!(".{}", constants::BACKUP_EXTENSION);
                            let relevant = event.paths.iter().any(|p| {
                                let path_str = p.to_string_lossy();
                                !path_str.contains(".tmp") && !path_str.ends_with(&backup_suffix)
                            });
                            if relevant {
                                last_event_time = Some(Instant::now());
                                pending = true;
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(t) = last_event_time {
                            if pending && t.elapsed() >= debounce_duration {
                                let _ = app_handle.emit("managed-folder-changed", &managed_path_clone);
                                pending = false;
                                last_event_time = None;
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
        });

        self._watcher = Some(watcher);
        self.watched_path = Some(managed_path);

        Ok(())
    }

    pub fn stop_watching(&mut self) {
        self._watcher = None;
        self.watched_path = None;
    }
}

#[tauri::command]
pub fn start_file_watcher(
    app_handle: AppHandle,
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
    mods_path: String,
) -> Result<(), String> {
    let mut w = watcher.lock().map_err(|e| e.to_string())?;
    w.stop_watching();
    w.start_watching(app_handle, Path::new(&mods_path))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn stop_file_watcher(
    watcher: tauri::State<'_, Arc<Mutex<FileWatcher>>>,
) -> Result<(), String> {
    let mut w = watcher.lock().map_err(|e| e.to_string())?;
    w.stop_watching();
    Ok(())
}
