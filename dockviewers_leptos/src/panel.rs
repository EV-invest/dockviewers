//! The consumer-facing content seam for Leptos — the point where an app injects its own widget
//! for each panel id while the library owns layout. Mirrors the Dioxus binding's `DockPanel`, but
//! the content handle is a Leptos view factory instead of a Dioxus `Element`.

use std::sync::Arc;

use dockviewers_core::PanelId;
use leptos::prelude::AnyView;

/// A view factory: called once per panel when its key first appears in the overlay's keyed `<For>`,
/// so the produced node (and any inner JS state) is kept alive across layout restructuring. `Arc<…
/// + Send + Sync>` because Leptos's keyed list keeps its items in the send-sync reactive arena (the
/// produced `AnyView` need not itself be `Send`).
pub type PanelView = Arc<dyn Fn() -> AnyView + Send + Sync>;

/// One widget the consumer hands to [`PackedArea`](crate::view::PackedArea): a stable id, its tab
/// title, and its content factory.
#[derive(Clone)]
pub struct DockPanel {
	pub id: PanelId,
	pub title: String,
	pub content: PanelView,
}
