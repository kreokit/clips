//! Locating the FFmpeg and ffprobe binaries.
//!
//! Lookup order:
//! 1. `KREOKIT_FFMPEG` / `KREOKIT_FFPROBE` environment variables (explicit override).
//! 2. A bundled sidecar next to the app executable. Release builds declare the binaries in
//!    `tauri.release.conf.json` (`bundle.externalBin`); Tauri copies them next to the
//!    executable with the target-triple suffix stripped.
//! 3. The system install: `PATH`, then common install dirs. Apps launched from Finder
//!    don't inherit the shell `PATH`, so those dirs are checked explicitly.
//!
//! In development (step 3) the system FFmpeg is used. Release builds (milestone 6) bundle
//! an LGPL FFmpeg build (no libx264/GPL parts) as the sidecar, as KreoKit Captions does.

use std::env;
use std::path::{Path, PathBuf};

/// Directories checked after `PATH`.
const FALLBACK_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "~/.local/bin"];

/// Returns the path of `tool` ("ffmpeg" or "ffprobe").
pub fn locate(tool: &str) -> Result<PathBuf, String> {
    let env_var = format!("KREOKIT_{}", tool.to_uppercase());
    if let Some(path) = env::var_os(&env_var).map(PathBuf::from) {
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "{env_var} points to {}, which does not exist",
                path.display()
            ))
        };
    }

    let file_name = format!("{tool}{}", env::consts::EXE_SUFFIX);

    let sidecar_dir = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let path_dirs = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    let fallback_dirs = FALLBACK_DIRS.iter().filter_map(|d| expand_home(d));

    sidecar_dir
        .into_iter()
        .chain(path_dirs)
        .chain(fallback_dirs)
        .map(|dir| dir.join(&file_name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            format!("{tool} not found. Install FFmpeg or set {env_var} to the {tool} binary.")
        })
}

fn expand_home(dir: &str) -> Option<PathBuf> {
    match dir.strip_prefix("~/") {
        Some(rest) => env::var_os("HOME").map(|home| PathBuf::from(home).join(rest)),
        None => Some(PathBuf::from(dir)),
    }
}
