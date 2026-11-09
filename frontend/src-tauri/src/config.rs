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

/// Whisper models this app supports; a saved model outside this list is migrated to the default.
pub fn is_supported_whisper_model(name: &str) -> bool {
    WHISPER_MODEL_CATALOG.iter().any(|(model, ..)| *model == name)
}
