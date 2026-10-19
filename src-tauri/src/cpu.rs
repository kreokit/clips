//! CPU support check. Clips bundles two native engines with different baselines:
//! - whisper.cpp release builds target x86-64 with AVX2, FMA and F16C (GGML_NATIVE=OFF, as
//!   in KreoKit Captions);
//! - ONNX Runtime 1.22 (pyke's static build, face detection) needs x86-64-v2: SSSE3, SSE4.1,
//!   SSE4.2 and POPCNT outside its runtime-dispatched AVX/FMA kernels (KreoKit Cutout).
//!
//! On an older CPU either would crash with an illegal instruction, so the app checks first
//! and explains instead. Re-check after upgrading either engine.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CpuSupport {
    pub supported: bool,
    /// Missing instruction sets, e.g. ["AVX2"]; empty when supported.
    pub missing: Vec<&'static str>,
}

/// What this machine's CPU lacks for transcription and face detection (always supported
/// off x86-64).
pub fn check() -> CpuSupport {
    let mut missing = missing_features();
    // Development aid: KREOKIT_DEV_MISSING_CPU=AVX2 simulates an unsupported CPU.
    if cfg!(debug_assertions) && std::env::var("KREOKIT_DEV_MISSING_CPU").as_deref() == Ok("AVX2") {
        missing.push("AVX2");
    }
    CpuSupport {
        supported: missing.is_empty(),
        missing,
    }
}

#[cfg(target_arch = "x86_64")]
fn missing_features() -> Vec<&'static str> {
    let checks = [
        (std::arch::is_x86_feature_detected!("ssse3"), "SSSE3"),
        (std::arch::is_x86_feature_detected!("sse4.1"), "SSE4.1"),
        (std::arch::is_x86_feature_detected!("sse4.2"), "SSE4.2"),
        (std::arch::is_x86_feature_detected!("popcnt"), "POPCNT"),
        (std::arch::is_x86_feature_detected!("avx2"), "AVX2"),
        (std::arch::is_x86_feature_detected!("fma"), "FMA"),
        (std::arch::is_x86_feature_detected!("f16c"), "F16C"),
    ];
    checks
        .into_iter()
        .filter(|(present, _)| !present)
        .map(|(_, name)| name)
        .collect()
}

#[cfg(not(target_arch = "x86_64"))]
fn missing_features() -> Vec<&'static str> {
    Vec::new()
}

/// The message shown when transcription or face detection can't run on this CPU (for the
/// command-line `--self-test`, milestone 6).
#[allow(dead_code)]
pub fn unsupported_message(missing: &[&str]) -> String {
    format!(
        "This CPU isn't supported: transcription and speaker tracking need a processor with {} \
         (almost all Intel and AMD processors from 2013 on).",
        missing.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_what_is_missing() {
        assert!(unsupported_message(&["AVX2", "FMA"]).contains("need a processor with AVX2, FMA"));
    }

    #[test]
    fn reports_consistently() {
        let s = check();
        assert_eq!(s.supported, s.missing.is_empty());
    }
}
