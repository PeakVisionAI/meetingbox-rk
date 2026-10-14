#![allow(dead_code)]

// Compile the production modules directly; do not duplicate the adapter in tests.
#[path = "../../../frontend/src-tauri/src/audio/transcription/provider.rs"]
mod provider;
#[path = "../../../frontend/src-tauri/src/audio/transcription/rk_asr_provider.rs"]
mod rk_asr_provider;
#[path = "../../../frontend/src-tauri/src/audio/transcription/bossai_provider.rs"]
mod bossai_provider;
#[path = "../../../frontend/src-tauri/src/audio/transcription/task.rs"]
mod task;
