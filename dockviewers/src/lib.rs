//! `dockviewers` — a packed-grid tiling/docking layout: fixed-size tiles that snap to a step grid,
//! never overlap, and leave whitespace below (InsilicoTerminal's look). Panes split, resize, tab
//! together, float, and maximize, with the arrangement saved to JSON and restored on reload.
//!
//! This crate is a thin **facade** over the ecosystem — pick a UI binding with a feature flag:
//!
//! ```toml
//! dockviewers = { version = "0.1", features = ["dioxus"] }   # or ["leptos"]
//! ```
//!
//! - Always available: [`core`] re-exports [`dockviewers_core`] — the framework-agnostic engine
//!   (grid model, gesture reducer, persistence, CSS). A bare `dockviewers` (no features) is just this.
//! - `dioxus` feature → [`dioxus`] re-exports [`dockviewers_dioxus`] (the Dioxus binding).
//! - `leptos` feature → [`leptos`] re-exports [`dockviewers_leptos`] (the Leptos binding).
//!
//! Depending on this facade vs. the individual crates is purely ergonomic — the code is identical
//! either way.

pub use dockviewers_core as core;
#[cfg(feature = "dioxus")]
pub use dockviewers_dioxus as dioxus;
#[cfg(feature = "leptos")]
pub use dockviewers_leptos as leptos;
