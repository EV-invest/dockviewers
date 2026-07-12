//! The consumer-facing content seam. Equivalent of `dockview-core/src/framwork.ts`
//! + the framework adapters (`docs/refs/dockview-react`, `dockview-vue`): the point
//! where an app injects its own widget for each panel id while the library owns layout.

use dioxus::prelude::*;
use dockviewers_core::PanelId;

/// One widget the consumer hands to [`PackedArea`](crate::view::PackedArea). The `content`
/// `Element` is rendered once per panel in a stable, id-keyed list inside the content-overlay
/// layer — never re-parented when the layout changes — so the component instance (and any inner
/// JS state, e.g. a live map) is preserved. Core only ever needs the `id` + `title`; this Dioxus
/// VNode is placed by the binding's overlay loop.
///
/// `content` is an eagerly-built `VNode`; that is fine because the overlay diffs by key, not by
/// tree position. If lazy construction is ever needed, swap to `Callback<PanelId, Element>` — the
/// overlay's keying contract stays identical.
#[derive(Clone, PartialEq, Props)]
pub struct DockPanel {
	pub id: PanelId,
	pub title: String,
	pub content: Element,
}
