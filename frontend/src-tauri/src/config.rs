/// Application configuration constants
///
/// Centralized definitions for default models and settings.
/// Used across database initialization, import, and retranscription.

/// Default Whisper model for transcription when no preference is configured.
/// large-v3: on a 37-min Ukrainian meeting turbo dropped whole sentences that
/// large-v3 kept; large-v3 is ~2.5x slower (6 min instead of 2.4 on an M5).
pub const DEFAULT_WHISPER_MODEL: &str = "large-v3";

/// Default Parakeet model for transcription when no preference is configured.
/// This is the quantized version optimized for speed.
pub const DEFAULT_PARAKEET_MODEL: &str = "parakeet-tdt-0.6b-v3-int8";

/// Whisper model catalog with metadata for all supported models.
/// Used by both WhisperEngine::discover_models() and discover_models_standalone().
///
/// Format: (name, filename, size_mb, accuracy, speed, description)
pub const WHISPER_MODEL_CATALOG: &[(&str, &str, u32, &str, &str, &str)] = &[
    // Only multilingual large models: they handle Ukrainian, surzhyk and English loanwords,
    // and accept the forced "uk" language plus the glossary initial prompt.
    // tiny/base/small/medium, English-only and distil models were removed.
    ("large-v3", "ggml-large-v3.bin", 2951, "High", "Medium", "Рекомендовано. Найточніша для української: не губить фраз; розпізнає 37 хв запису приблизно за 6 хв"),
    ("large-v3-turbo-q8_0", "ggml-large-v3-turbo-q8_0.bin", 834, "Medium", "Fast", "Швидша й менша, але може пропускати цілі фрази"),
];

/// Where an offered Whisper model comes from: the whisper.cpp repository on
/// Hugging Face at one fixed revision, with the file's SHA-256 (the value
/// Hugging Face lists for that revision, also checked against copies
/// downloaded earlier). A file that differs is refused.
pub struct WhisperModelSource {
    pub file: &'static str,
    pub sha256: &'static str,
}

const WHISPER_REPO_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

impl WhisperModelSource {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/{}/{}",
            WHISPER_REPO_REVISION, self.file
        )
    }
}

pub fn whisper_model_source(name: &str) -> Option<WhisperModelSource> {
    let (file, sha256) = match name {
        "large-v3" => ("ggml-large-v3.bin", "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2"),
        "large-v3-turbo-q8_0" => ("ggml-large-v3-turbo-q8_0.bin", "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1"),
        _ => return None,
    };
    Some(WhisperModelSource { file, sha256 })
}

/// A model fine-tuned on this organisation's own checked meetings, used in
/// place of the original: the version in use from `learning::versions`
/// (trained on this Mac), or without one the file put by hand next to the
/// original as `ggml-large-v3-tuned.bin` (scripts/finetune). It heard real
/// meetings, so it is never downloaded, published or sent anywhere; deleting
/// it in Settings brings the original back.
pub fn tuned_model_file(models_dir: &std::path::Path, filename: &str) -> Option<std::path::PathBuf> {
    crate::learning::versions::model_file(models_dir, filename)
}

/// Whisper models this app supports; a saved model outside this list is migrated to the default.
pub fn is_supported_whisper_model(name: &str) -> bool {
    WHISPER_MODEL_CATALOG.iter().any(|(model, ..)| *model == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_whisper_model_has_a_pinned_checked_source() {
        for (name, file, ..) in WHISPER_MODEL_CATALOG {
            let source = whisper_model_source(name).expect(name);
            assert_eq!(source.file, *file);
            assert_eq!(source.sha256.len(), 64);
            assert!(source.url().contains(WHISPER_REPO_REVISION));
            assert!(source.url().ends_with(file));
        }
        assert!(whisper_model_source("tiny").is_none());
    }
}
