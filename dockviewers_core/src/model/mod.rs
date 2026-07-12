//! The pure layout model — no DOM, no Dioxus: just data + operations, so it is
//! unit-testable in isolation and `cargo check`s on any target.
//!
//! - [`group`]  — a tile's tab-group (many panels, one active).
//! - [`packed`] — the packed grid: fixed-size tiles that snap to a step grid and
//!   leave whitespace instead of filling the view (InsilicoTerminal's look).

pub mod group;
pub mod packed;
pub mod serial;

pub use group::Group;

/// Stable identity of a panel (a single widget). Provided by the consumer; used as
/// the render key that keeps a panel's component instance alive across restructuring.
#[derive(Clone, Debug, serde::Deserialize, Eq, Hash, PartialEq, serde::Serialize)]
pub struct PanelId(pub String);

/// Stable identity of a group (a tab-strip leaf holding 1+ panels).
#[derive(Clone, Copy, Debug, serde::Deserialize, Eq, Hash, PartialEq, serde::Serialize)]
pub struct GroupId(pub u64);
