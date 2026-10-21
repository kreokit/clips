//! KreoKit Clips: long video → vertical short clips with captions, offline.

mod audio;
mod awake;
mod chunks;
mod cpu;
mod faces;
mod ffmpeg;
mod models;
mod moments;
mod probe;
mod projects;
mod sentences;
mod transcribe;
mod transcription;
mod vad;
mod waveform;
// Accuracy checks; tests only until the release self-test (milestone 6) uses it.
#[cfg(test)]
mod wer;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use models::{ModelDirs, ModelInfo, WhisperModel};
use probe::VideoInfo;
use projects::{Project, ProjectEntry};
use tauri::{AppHandle, Emitter, Manager, State};
use transcribe::Transcript;
use transcription::Settings;

/// Set by `cancel`; checked by the running model check/download and transcription.
#[derive(Default)]
struct CancelFlag(Arc<AtomicBool>);

/// Set while a transcription runs; there is only ever one.
#[derive(Default)]
struct Busy(Arc<AtomicBool>);

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

/// Brings a project's transcript made before audio padding onto the video's clock, with
/// its clips (`transcription::upgrade`); the video is probed only for such a transcript.
fn upgrade(root: &Path, project: Project) -> Result<Project, String> {
    let dir = projects::project_dir(root, &project.id)?.join(projects::TRANSCRIPT_DIR);
    let video = PathBuf::from(&project.video_path);
    match transcription::upgrade(&dir, || probe::audio_start_ms(&video))? {
        Some(ms) => projects::shift_clips(root, project, ms),
        None => Ok(project),
    }
}

/// Lets the webview play the project's video (the asset protocol serves only files
/// allowed this way, besides the thumbnails).
fn allow_video(app: &AppHandle, project: Project) -> Result<Project, String> {
    app.asset_protocol_scope()
        .allow_file(&project.video_path)
        .map_err(|e| e.to_string())?;
    Ok(project)
}

/// Opens the project of the video at `path`, creating it on first import.
#[tauri::command]
async fn open_video(app: AppHandle, path: String) -> Result<Project, String> {
    let root = projects_root(&app)?;
    let project = blocking(move || {
        projects::open_video(
            &root,
            &PathBuf::from(path),
            probe::probe_video,
            projects::now_ms(),
        )
        .and_then(|p| upgrade(&root, p))
    })
    .await?;
    allow_video(&app, project)
}

/// Opens a project from Home's list.
#[tauri::command]
async fn open_project(app: AppHandle, id: String) -> Result<Project, String> {
    let root = projects_root(&app)?;
    let project = blocking(move || {
        projects::open(&root, &id, projects::now_ms()).and_then(|p| upgrade(&root, p))
    })
    .await?;
    allow_video(&app, project)
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
    let project = blocking(move || {
        projects::locate_video(
            &root,
            &id,
            &PathBuf::from(path),
            probe::probe_video,
            projects::now_ms(),
        )
        .and_then(|p| upgrade(&root, p))
    })
    .await?;
    allow_video(&app, project)
}

#[tauri::command]
async fn remove_project(app: AppHandle, id: String) -> Result<(), String> {
    let root = projects_root(&app)?;
    blocking(move || projects::remove(&root, &id)).await
}

/// "Transcribe again": deletes the project's transcript; its clips stay.
#[tauri::command]
async fn discard_transcript(app: AppHandle, id: String) -> Result<(), String> {
    let root = projects_root(&app)?;
    blocking(move || projects::discard_transcript(&root, &id)).await
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
            models::download(model.file(), &dirs.ours, &cancel, emit("downloading"))?;
        }
        Ok(())
    })
    .await
}

/// Stops the running model check, download or transcription.
#[tauri::command]
fn cancel(cancel: State<'_, CancelFlag>) {
    cancel.0.store(true, Ordering::Relaxed);
}

/// A project's transcript cache folder and the facts the cache is checked against.
fn transcript_job(
    app: &AppHandle,
    id: &str,
) -> Result<(Project, PathBuf, transcription::Video), String> {
    let root = projects_root(app)?;
    let project = projects::load(&root, id)?;
    let dir = projects::project_dir(&root, id)?.join(projects::TRANSCRIPT_DIR);
    let video = transcription::Video {
        size: project.video_size,
        mtime_ms: project.video_mtime_ms,
        duration_ms: project.video.duration_ms,
    };
    Ok((project, dir, video))
}

/// What the Transcribe screen shows for these settings: a finished transcript, or how far
/// a cancelled run got.
#[derive(serde::Serialize)]
struct TranscriptStatus {
    transcript: Option<Transcript>,
    /// Saved work reaches this far into the video (0: nothing saved).
    saved_until_ms: u64,
    /// How many parts the video has (estimated until the audio has been read once).
    parts: usize,
}

#[tauri::command]
async fn transcript_status(
    app: AppHandle,
    id: String,
    settings: Settings,
) -> Result<TranscriptStatus, String> {
    blocking(move || {
        let (_, dir, video) = transcript_job(&app, &id)?;
        let limits = &chunks::LIMITS;
        let (saved_until_ms, parts) = transcription::saved_until(&dir, &settings, &video, limits);
        Ok(TranscriptStatus {
            transcript: transcription::cached(&dir, &settings, &video, limits)
                .map(|o| o.transcript),
            saved_until_ms,
            parts: parts
                .unwrap_or_else(|| video.duration_ms.div_ceil(limits.target_ms).max(1) as usize),
        })
    })
    .await
}

/// Transcribes the project's video (resuming a cancelled run), emitting
/// `transcription-progress`. The model must be ready (`prepare_model`).
#[tauri::command]
async fn transcribe(
    app: AppHandle,
    cancel: State<'_, CancelFlag>,
    busy: State<'_, Busy>,
    id: String,
    settings: Settings,
) -> Result<Transcript, String> {
    if busy.0.swap(true, Ordering::SeqCst) {
        return Err("A transcription is already running.".into());
    }
    let busy = busy.0.clone();
    let cancel = cancel.0.clone();
    cancel.store(false, Ordering::Relaxed);
    let result = blocking(move || {
        let (project, dir, video) = transcript_job(&app, &id)?;
        let dirs = model_dirs(&app)?;
        let _awake = awake::KeepAwake::new("transcribing");
        let video_path = PathBuf::from(&project.video_path);
        let load = || {
            let path = models::resolve(settings.model, &dirs, &cancel, |_| {})?
                .ok_or("The model isn't downloaded yet.")?;
            transcribe::Engine::load(settings.model, &path)
        };
        let emitter = app.clone();
        transcription::run(
            &dir,
            &settings,
            &video,
            &chunks::LIMITS,
            |start_ms| audio::AudioStream::open_at(&video_path, start_ms),
            load,
            &cancel,
            move |p| {
                let _ = emitter.emit("transcription-progress", p);
            },
        )
        .map(|o| o.transcript)
    })
    .await;
    busy.store(false, Ordering::SeqCst);
    result
}

/// Path of a clip's thumbnail, the frame `ms` into the video (made on first request).
#[tauri::command]
async fn clip_thumbnail(app: AppHandle, id: String, ms: u64) -> Result<String, String> {
    let root = projects_root(&app)?;
    blocking(move || {
        projects::clip_thumbnail(&root, &id, ms).map(|p| p.to_string_lossy().into_owned())
    })
    .await
}

/// The timeline waveform from `from_ms` to `to_ms`: one byte per 10 ms (waveform.rs),
/// sent as raw bytes.
#[tauri::command]
async fn waveform_range(
    app: AppHandle,
    id: String,
    from_ms: u64,
    to_ms: u64,
) -> Result<tauri::ipc::Response, String> {
    let root = projects_root(&app)?;
    let peaks = blocking(move || {
        let project = projects::load(&root, &id)?;
        waveform::range(std::path::Path::new(&project.video_path), from_ms, to_ms)
    })
    .await?;
    Ok(tauri::ipc::Response::new(peaks))
}

/// Remembers the finder settings chosen last, for new projects.
#[tauri::command]
async fn remember_finder(app: AppHandle, finder: moments::FinderSettings) -> Result<(), String> {
    let root = projects_root(&app)?;
    blocking(move || projects::remember_finder(&root, &finder)).await
}

/// The project's finished transcript for these settings, and its loudness.
fn finished(
    app: &AppHandle,
    id: &str,
    settings: &Settings,
) -> Result<(Project, transcription::Output), String> {
    let (project, dir, video) = transcript_job(app, id)?;
    let output = transcription::cached(&dir, settings, &video, &chunks::LIMITS)
        .ok_or("Transcribe the video first.")?;
    Ok((project, output))
}

/// The transcript as spoken words and sentences (sound tags apart), for the clip editor.
#[tauri::command]
async fn transcript_text(
    app: AppHandle,
    id: String,
    settings: Settings,
) -> Result<sentences::Text, String> {
    blocking(move || {
        Ok(sentences::split(
            &finished(&app, &id, &settings)?.1.transcript,
        ))
    })
    .await
}

/// Proposes clips: as many as the finder settings ask for minus the `have` clips already
/// counted on the board, none overlapping `avoid`.
#[tauri::command]
async fn find_moments(
    app: AppHandle,
    id: String,
    settings: Settings,
    finder: moments::FinderSettings,
    avoid: Vec<(u64, u64)>,
    have: usize,
) -> Result<Vec<moments::Moment>, String> {
    blocking(move || {
        let (project, output) = finished(&app, &id, &settings)?;
        Ok(moments::find_in(
            &output.transcript,
            &output.loudness,
            project.video.duration_ms,
            &finder,
            &avoid,
            have,
        ))
    })
    .await
}

/// Scores edited or manual clips (`None` for a range without words).
#[tauri::command]
async fn score_clips(
    app: AppHandle,
    id: String,
    settings: Settings,
    finder: moments::FinderSettings,
    ranges: Vec<(u64, u64)>,
) -> Result<Vec<Option<moments::Moment>>, String> {
    blocking(move || {
        let (project, output) = finished(&app, &id, &settings)?;
        Ok(moments::score_in(
            &output.transcript,
            &output.loudness,
            project.video.duration_ms,
            &finder,
            &ranges,
        ))
    })
    .await
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
        .manage(Busy::default())
        .invoke_handler(tauri::generate_handler![
            probe_video,
            list_projects,
            open_video,
            open_project,
            save_project,
            locate_video,
            remove_project,
            discard_transcript,
            project_thumbnail,
            clip_thumbnail,
            waveform_range,
            list_models,
            prepare_model,
            cancel,
            transcript_status,
            transcribe,
            transcript_text,
            find_moments,
            score_clips,
            remember_finder,
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
