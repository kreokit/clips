//! KreoKit Clips: long video → vertical short clips with captions, offline.

mod analysis;
mod ass;
mod audio;
mod awake;
mod batch;
mod captions;
mod chunks;
mod cpu;
mod export;
mod faces;
mod ffmpeg;
mod frames;
mod layout;
mod models;
mod moments;
mod probe;
mod projects;
mod reframe;
mod sentences;
mod style;
mod subtitles;
mod tracks;
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

/// The face analysis job (one at a time, never beside a transcription): whether it runs,
/// and its own Cancel.
#[derive(Default)]
struct FacesJob {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

/// Export all (one at a time): whether it runs, and its Stop flag.
#[derive(Default)]
struct ExportJob {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

/// What `analyze_faces` returns while a transcription runs (the frontend tries later).
const TRANSCRIBING: &str = "transcribing";

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
    faces: State<'_, FacesJob>,
    id: String,
    settings: Settings,
) -> Result<Transcript, String> {
    if busy.0.swap(true, Ordering::SeqCst) {
        return Err("A transcription is already running.".into());
    }
    let busy = busy.0.clone();
    // Transcription comes first: a running face analysis stops (it resumes later from its
    // finished blocks) and is waited for, so the two never run at the same time.
    faces.cancel.store(true, Ordering::SeqCst);
    let faces_running = faces.running.clone();
    for _ in 0..600 {
        if !faces_running.load(Ordering::SeqCst) {
            break;
        }
        tokio_sleep_ms(50).await;
    }
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

async fn tokio_sleep_ms(ms: u64) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms))
    })
    .await;
}

/// A project's face cache folder and the video facts it is checked against.
fn faces_job(app: &AppHandle, id: &str) -> Result<(PathBuf, analysis::Video), String> {
    let root = projects_root(app)?;
    let project = projects::load(&root, id)?;
    let dir = projects::project_dir(&root, id)?.join(projects::FACES_DIR);
    let video = analysis::Video {
        path: PathBuf::from(&project.video_path),
        width: project.video.width,
        height: project.video.height,
        duration_ms: project.video.duration_ms,
        size: project.video_size,
        mtime_ms: project.video_mtime_ms,
    };
    Ok((dir, video))
}

#[derive(Clone, serde::Serialize)]
struct FacesProgress {
    id: String,
    #[serde(flatten)]
    progress: analysis::Progress,
}

/// Finds the faces in `ranges` of the project's video (what isn't cached yet), emitting
/// `faces-progress`. Downloads the face model first if needed (2 MB). Fails with
/// "transcribing" while a transcription runs, and with "cancelled" when stopped.
#[tauri::command]
async fn analyze_faces(
    app: AppHandle,
    busy: State<'_, Busy>,
    faces: State<'_, FacesJob>,
    id: String,
    ranges: Vec<(u64, u64)>,
) -> Result<(), String> {
    if busy.0.load(Ordering::SeqCst) {
        return Err(TRANSCRIBING.into());
    }
    if faces.running.swap(true, Ordering::SeqCst) {
        return Err("A face analysis is already running.".into());
    }
    let running = faces.running.clone();
    // A transcription that started meanwhile wins.
    if busy.0.load(Ordering::SeqCst) {
        running.store(false, Ordering::SeqCst);
        return Err(TRANSCRIBING.into());
    }
    let cancel = faces.cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    let result = blocking(move || {
        let (dir, video) = faces_job(&app, &id)?;
        let models_dir = model_dirs(&app)?.ours;
        let _awake = awake::KeepAwake::new("finding faces");
        let load = || {
            let path = models::download(models::FACE_MODEL, &models_dir, &cancel, |_| {})?;
            faces::Detector::load(&path)
        };
        let emitter = app.clone();
        analysis::run(&dir, &video, &ranges, load, &cancel, move |progress| {
            let _ = emitter.emit(
                "faces-progress",
                FacesProgress {
                    id: id.clone(),
                    progress,
                },
            );
        })
    })
    .await;
    running.store(false, Ordering::SeqCst);
    result
}

/// Stops the running face analysis (finished blocks are kept).
#[tauri::command]
fn cancel_faces(faces: State<'_, FacesJob>) {
    faces.cancel.store(true, Ordering::SeqCst);
}

/// The bundled caption fonts (tauri.conf.json `bundle.resources`); in debug builds also
/// the source folder, in case the resources haven't been copied yet.
fn caption_fonts_dir(app: &AppHandle) -> Option<PathBuf> {
    let bundled = app
        .path()
        .resolve("fonts", tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|p| p.is_dir());
    bundled.or_else(|| {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/fonts");
        (cfg!(debug_assertions) && source.is_dir()).then_some(source)
    })
}

/// What Export all would do: file names, files already there, size and free space.
#[tauri::command]
async fn export_plan(
    app: AppHandle,
    id: String,
    folder: String,
    items: Vec<(u32, u64)>,
    srt: bool,
) -> Result<batch::Plan, String> {
    let root = projects_root(&app)?;
    blocking(move || {
        let project = projects::load(&root, &id)?;
        Ok(batch::plan(Path::new(&folder), &project.name, &items, srt))
    })
    .await
}

#[derive(Clone, serde::Serialize)]
struct ExportProgress {
    /// The clip being exported (0-based) of `total`, and its progress 0–100.
    index: usize,
    total: usize,
    percent: u32,
}

/// Exports `items` into `folder`, one after the other (`export-progress` events). A clip
/// that fails is reported and the next one goes on; Stop skips the rest.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn export_clips(
    app: AppHandle,
    busy: State<'_, Busy>,
    job: State<'_, ExportJob>,
    id: String,
    settings: Settings,
    folder: String,
    items: Vec<batch::Item>,
    srt: bool,
    replace: bool,
) -> Result<Vec<batch::ItemResult>, String> {
    if busy.0.load(Ordering::SeqCst) {
        return Err("Wait for the transcription to finish.".into());
    }
    if job.running.swap(true, Ordering::SeqCst) {
        return Err("An export is already running.".into());
    }
    let running = job.running.clone();
    let cancel = job.cancel.clone();
    cancel.store(false, Ordering::SeqCst);
    let fonts = caption_fonts_dir(&app);
    let result = blocking(move || {
        let (project, output) = finished(&app, &id, &settings)?;
        let text = sentences::split(&output.transcript);
        let _awake = awake::KeepAwake::new("exporting clips");
        let tiktok = project.finder.platform == Some(moments::Platform::TiktokRewards);
        let largest = items.iter().map(|i| i.number).max().unwrap_or(1);
        let folder = PathBuf::from(&folder);
        let batch = batch::Batch {
            video: Path::new(&project.video_path),
            info: &project.video,
            video_name: &project.name,
            text: &text,
            fixes: &project.word_fixes,
            folder: &folder,
            largest_number: largest,
            srt,
            replace,
            tiktok,
            fonts: fonts.as_deref(),
        };
        let total = items.len();
        let mut results = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                results.push(batch::ItemResult::Skipped {
                    clip_id: item.clip_id,
                });
                continue;
            }
            let emitter = app.clone();
            let _ = emitter.emit(
                "export-progress",
                ExportProgress {
                    index,
                    total,
                    percent: 0,
                },
            );
            let result = batch::export_one(&batch, item, &cancel, |percent| {
                let _ = emitter.emit(
                    "export-progress",
                    ExportProgress {
                        index,
                        total,
                        percent,
                    },
                );
            });
            results.push(match result {
                Ok((path, srt)) => batch::ItemResult::Saved {
                    clip_id: item.clip_id,
                    path: path.to_string_lossy().into_owned(),
                    srt: srt.map(|p| p.to_string_lossy().into_owned()),
                },
                Err(e) if e == models::CANCELLED => batch::ItemResult::Skipped {
                    clip_id: item.clip_id,
                },
                Err(error) => batch::ItemResult::Failed {
                    clip_id: item.clip_id,
                    error,
                },
            });
        }
        Ok(results)
    })
    .await;
    running.store(false, Ordering::SeqCst);
    result
}

/// Stops Export all: the clip being written is dropped, the rest skipped.
#[tauri::command]
fn cancel_export(job: State<'_, ExportJob>) {
    job.cancel.store(true, Ordering::SeqCst);
}

/// Shows an exported clip in the file manager (only .mp4 / .srt files).
#[tauri::command]
fn reveal_export(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = PathBuf::from(path);
    let exported = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_lowercase().as_str(), "mp4" | "srt"));
    if !exported || !path.is_file() {
        return Err("not an exported clip".into());
    }
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| e.to_string())
}

/// The automatic crop path for a clip, or `None` until its faces are analysed.
#[tauri::command]
async fn reframe_path(
    app: AppHandle,
    id: String,
    in_ms: u64,
    out_ms: u64,
) -> Result<Option<reframe::Path>, String> {
    blocking(move || {
        let (dir, video) = faces_job(&app, &id)?;
        let Some(samples) = analysis::samples(&dir, &video, in_ms, out_ms) else {
            return Ok(None);
        };
        let (w, h) = frames::sample_size(video.width, video.height);
        let frame = (w as f32, h as f32);
        let tracks = tracks::track(&samples);
        let subjects = tracks::subjects(&samples, &tracks, frame);
        Ok(Some(reframe::auto_path(
            &samples,
            &subjects,
            (video.width, video.height),
            frame,
        )))
    })
    .await
}

/// The faces found in a clip, per face-analysis sample, as boxes in shares of the video
/// frame (left, top, right, bottom), or `None` until its faces are analysed. For the
/// editor's warning when captions cover a face.
#[tauri::command]
async fn clip_faces(
    app: AppHandle,
    id: String,
    in_ms: u64,
    out_ms: u64,
) -> Result<Option<Vec<(u64, Vec<[f32; 4]>)>>, String> {
    blocking(move || {
        let (dir, video) = faces_job(&app, &id)?;
        let Some(samples) = analysis::samples(&dir, &video, in_ms, out_ms) else {
            return Ok(None);
        };
        let (w, h) = frames::sample_size(video.width, video.height);
        let (w, h) = (w as f32, h as f32);
        Ok(Some(
            samples
                .iter()
                .map(|s| {
                    let boxes = s
                        .faces
                        .iter()
                        .map(|f| [f.x0 / w, f.y0 / h, f.x1 / w, f.y1 / h])
                        .collect();
                    (s.t_ms, boxes)
                })
                .collect(),
        ))
    })
    .await
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

/// Remembers the caption style for new projects.
#[tauri::command]
async fn remember_style(app: AppHandle, style: style::Style) -> Result<(), String> {
    let root = projects_root(&app)?;
    blocking(move || projects::remember_style(&root, &style)).await
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

/// A clip's captions on its own clock, with the project's word fixes (see captions.rs).
#[tauri::command]
async fn clip_captions(
    app: AppHandle,
    id: String,
    settings: Settings,
    fixes: Vec<captions::WordFix>,
    in_ms: u64,
    out_ms: u64,
) -> Result<Vec<style::Segment>, String> {
    blocking(move || {
        let text = sentences::split(&finished(&app, &id, &settings)?.1.transcript);
        Ok(captions::clip_captions(&text, &fixes, in_ms, out_ms))
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

/// Development aid: a clip to open in the editor at startup (with `KREOKIT_DEV_OPEN`),
/// from `KREOKIT_DEV_CLIP`. Always `None` in release builds.
#[tauri::command]
fn dev_startup_clip() -> Option<u32> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("KREOKIT_DEV_CLIP").ok()?.trim().parse().ok()
}

/// Development aid: Export all uses this folder instead of asking (`KREOKIT_DEV_EXPORT_DIR`),
/// so the export can be checked without the system's folder picker. Always `None` in
/// release builds.
#[tauri::command]
fn dev_export_folder() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("KREOKIT_DEV_EXPORT_DIR")
        .ok()
        .filter(|d| Path::new(d).is_dir())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(CancelFlag::default())
        .manage(Busy::default())
        .manage(FacesJob::default())
        .manage(ExportJob::default())
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
            analyze_faces,
            cancel_faces,
            reframe_path,
            clip_faces,
            export_plan,
            export_clips,
            cancel_export,
            reveal_export,
            transcript_text,
            clip_captions,
            find_moments,
            score_clips,
            remember_finder,
            remember_style,
            cpu_support,
            quit_app,
            report_problem,
            dev_startup_video,
            dev_startup_clip,
            dev_export_folder
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Quitting stops the jobs, so their FFmpeg processes end with the app.
            if let tauri::RunEvent::Exit = event {
                app.state::<FacesJob>().cancel.store(true, Ordering::SeqCst);
                app.state::<ExportJob>()
                    .cancel
                    .store(true, Ordering::SeqCst);
                app.state::<CancelFlag>().0.store(true, Ordering::SeqCst);
            }
        });
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
