//! Platform profile: the only place that knows whether we run on an RK device or
//! on the Intel two-machine setup.

mod profile;

pub use profile::{current, Capabilities, Profile, ProfileKind, ServiceProbe};

/// Exposed to the frontend so one UI bundle can adapt to both products.
#[tauri::command]
pub fn get_platform_profile() -> serde_json::Value {
    current().to_frontend()
}
