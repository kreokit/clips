//! KreoKit Clips: long video → vertical short clips with captions, offline.

mod audio;
mod cpu;
mod ffmpeg;
mod models;
mod probe;
mod projects;
mod transcribe;
mod vad;
mod wer;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use models::{ModelDirs, ModelInfo, WhisperModel};
use probe::VideoInfo;
use projects::{Project, ProjectEntry};
use tauri::{AppHandle, Emitter, Manager, State};

/// Set by `cancel`; checked by the running model download (and, later, transcription).
#[derive(Default)]
struct CancelFlag(Arc<AtomicBool>);

/// Runs blocking work (ffprobe, later whisper and FFmpeg) off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn probe_video(path: String) -> Result<VideoInfo, String> {
    blocking(move || probe::probe_video(&PathBuf::from(path))).await
}

/// `<app data>/projects`, where every project lives (CLAUDE.md "Projects").
fn projects_root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("projects"))
        .map_err(|e| format!("no app data folder: {e}"))
}

#[tauri::command]
async fn list_projects(app: AppHandle) -> Result<Vec<ProjectEntry>, String> {
    let root = projects_root(&app)?;
    blocking(move || Ok(projects::list(&root))).await
}

/// Opens the project of the video at `path`, creating it on first import.
#[tauri::command]
async fn open_video(app: AppHandle, path: String) -> Result<Project, String> {
    let root = projects_root(&app)?;
    blocking(move || {
        projects::open_video(
            &root,
            &PathBuf::from(path),
            probe::probe_video,
            projects::now_ms(),
        )
    })
    .await
}

/// Opens a project from Home's list.
#[tauri::command]
async fn open_project(app: AppHandle, id: String) -> Result<Project, String> {
    let root = projects_root(&app)?;
    blocking(move || projects::open(&root, &id, projects::now_ms())).await
}

/// The autosave: writes the project and returns it with `updated_at` set.
#[tauri::command]
async fn save_project(app: AppHandle, project: Project) -> Result<Project, String> {
    let root = projects_root(&app)?;
    blocking(move || projects::save(&root, &project, projects::now_ms())).await
}

#[tauri::command]
async fn locate_video(app: AppHandle, id: String, path: String) -> Result<Project, String> {
    let root = projects_root(&app)?;
    blocking(move || {
        projects::locate_video(
            &root,
            &id,
            &PathBuf::from(path),
            probe::probe_video,
            projects::now_ms(),
        )
    })
    .await
}

#[tauri::command]
async fn remove_project(app: AppHandle, id: String) -> Result<(), String> {
    let root = projects_root(&app)?;
    blocking(move || projects::remove(&root, &id)).await
}

/// Path of the project's thumbnail (made on first request), for `convertFileSrc`.
#[tauri::command]
async fn project_thumbnail(app: AppHandle, id: String) -> Result<String, String> {
    let root = projects_root(&app)?;
    blocking(move || projects::thumbnail(&root, &id).map(|p| p.to_string_lossy().into_owned()))
        .await
}

fn model_dirs(app: &AppHandle) -> Result<ModelDirs, String> {
    app.path()
        .app_data_dir()
        .map(|dir| ModelDirs::new(&dir))
        .map_err(|e| format!("no app data folder: {e}"))
}

#[tauri::command]
async fn list_models(app: AppHandle) -> Result<Vec<ModelInfo>, String> {
    let dirs = model_dirs(&app)?;
    blocking(move || Ok(models::list(&dirs))).await
}

/// What `prepare_model` is doing, for its progress events.
#[derive(Clone, serde::Serialize)]
struct ModelProgress {
    /// "checking" (a model from Captions' folder) or "downloading".
    stage: &'static str,
    #[serde(flatten)]
    progress: models::Progress,
}

/// Makes `model` ready to use: a matching file from Captions' folder after its checksum,
/// else a download from our mirror. Emits `model-progress`.
#[tauri::command]
async fn prepare_model(
    app: AppHandle,
    cancel: State<'_, CancelFlag>,
    model: WhisperModel,
) -> Result<(), String> {
    let dirs = model_dirs(&app)?;
    let cancel = cancel.0.clone();
    cancel.store(false, Ordering::Relaxed);
    blocking(move || {
        let emit = |stage| {
            let app = app.clone();
            move |progress| {
                let _ = app.emit("model-progress", ModelProgress { stage, progress });
            }
        };
        if models::resolve(model, &dirs, &cancel, emit("checking"))?.is_none() {
            models::download(model, &dirs, &cancel, emit("downloading"))?;
        }
        Ok(())
    })
    .await
}

/// Stops the running model check or download.
#[tauri::command]
fn cancel(cancel: State<'_, CancelFlag>) {
    cancel.0.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn cpu_support() -> cpu::CpuSupport {
    cpu::check()
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Where testers report problems: the bug-report form, with the app version filled in.
fn report_problem_url() -> String {
    format!(
        "https://github.com/kreokit/clips/issues/new?template=bug_report.yml&version={}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Opens the bug-report page in the default browser. Only this fixed page can be opened;
/// the webview gets no general permission to open links.
#[tauri::command]
fn report_problem(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(report_problem_url(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Development aid: a video to open at startup, from `KREOKIT_DEV_OPEN`. Always `None`
/// in release builds.
#[tauri::command]
fn dev_startup_video() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("KREOKIT_DEV_OPEN")
        .ok()
        .filter(|p| !p.is_empty())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(CancelFlag::default())
        .invoke_handler(tauri::generate_handler![
            probe_video,
            list_projects,
            open_video,
            open_project,
            save_project,
            locate_video,
            remove_project,
            project_thumbnail,
            list_models,
            prepare_model,
            cancel,
            cpu_support,
            quit_app,
            report_problem,
            dev_startup_video
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn report_problem_opens_the_bug_form() {
        assert_eq!(
            super::report_problem_url(),
            format!(
                "https://github.com/kreokit/clips/issues/new?template=bug_report.yml&version={}",
                env!("CARGO_PKG_VERSION")
            )
        );
    }
}
