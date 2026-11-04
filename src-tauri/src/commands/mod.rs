//! Tauri commands – thin wrappers over the modules, one file per work stream:
//!
//! | file | owner | contents |
//! |---|---|---|
//! | `geo.rs` | R4 geocode | geocode_search / reverse, poi_layer |
//! | `route.rs` | R2 routing | route (hybrid), optimize_order, elevation_profile |
//! | `nav.rs` | R2 routing | nav_*, speak, voices_list, locate |
//! | `transit.rs` | R3 transit | transit_* |
//! | `data.rs` | R1 data | data_*, archive paths, minyanim, regions, save_text_file |
//! | `app.rs` | R1 data | settings, history, favorites, program updates, app_info, logs |
//!
//! Every command returns [`R<T>`] = `Result<T, AppError>`; the webview receives
//! `{ code, message, details? }` (see `crate::error` for the code list). Register new commands in
//! `lib.rs` under the stream's marker comment.
pub mod app;
pub mod data;
pub mod geo;
pub mod nav;
pub mod route;
pub mod transit;

pub use crate::error::{guard, guard_async, AppError, R};

/// Imports shared by the command files (`use super::prelude::*`).
pub(crate) mod prelude {
    pub use std::sync::atomic::Ordering;
    pub use std::sync::Arc;

    pub use tauri::{AppHandle, Emitter, State};

    pub use super::{err, AppError, R};
    pub use crate::state::AppState;
    pub use crate::types::*;
}

/// Any error (anyhow, io, rusqlite, tauri join…) → `AppError`, classified by its cause chain.
pub fn err<E: Into<anyhow::Error>>(e: E) -> AppError {
    AppError::from_anyhow(&e.into())
}

// helpers used by more than one command file (routing is called from data_preload's warm-up etc.)
pub use route::{route_hybrid, route_offline};
