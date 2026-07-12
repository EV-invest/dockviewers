//! A minimal Leptos smoke example proving one [`dockviewers_core`] engine drives the Leptos binding
//! exactly as it drives the Dioxus one: a handful of static panes seeded through the same
//! [`PackedApi`], laid out by the same reducer. **Web-only** — run with `trunk serve` (or any CSR
//! host); a native `cargo run` has no DOM to mount into.

use std::sync::Arc;

use dockviewers_leptos::{Config, DockPanel, Group, GroupId, MinSize, PackedApi, PackedArea, PanelId, Step};
use leptos::prelude::*;

fn main() {
	#[cfg(target_arch = "wasm32")]
	leptos::mount::mount_to_body(|| view! { <App /> });
	#[cfg(not(target_arch = "wasm32"))]
	{
		let _ = App;
		eprintln!("insilico_leptos is a web example — run it with a CSR host (e.g. `trunk serve`).");
	}
}

/// One pane's widget: its title on a dark card. `content` is a factory so the overlay can re-key
/// without consuming the view.
fn make_panel(kind: &str, n: u64) -> DockPanel {
	let title = kind.to_string();
	let label = title.clone();
	DockPanel {
		id: PanelId(format!("{kind}-{n}")),
		title,
		content: Arc::new(move || view! {
			<div style="padding:10px; color:#ddd; font:13px system-ui;">{format!("{label} pane")}</div>
		}.into_any()),
	}
}

/// Seed a small starting spread through the shared [`PackedApi`], after the first measure.
fn seed(api: PackedApi, panels: RwSignal<Vec<DockPanel>>) {
	for (i, kind) in ["Chart", "Order Book", "Trades", "Console"].iter().enumerate() {
		let panel = make_panel(kind, i as u64);
		let id = panel.id.clone();
		panels.update(|v| v.push(panel));
		let gid = api.mint_group_id();
		api.place(Group::new(gid, id), 10 + i as u32 * 3, 8, MinSize::Steps { w: Step(3), h: Step(2) });
	}
}

#[component]
fn App() -> impl IntoView {
	let panels = RwSignal::new(Vec::<DockPanel>::new());
	let on_ready: Arc<dyn Fn(PackedApi) + Send + Sync> = Arc::new(move |api: PackedApi| seed(api, panels));
	// The `+` button target: this smoke has no catalog, so it's a no-op.
	let request_tab: Arc<dyn Fn(GroupId) + Send + Sync> = Arc::new(|_gid| {});

	view! {
		<div style="position:fixed; inset:0; display:flex; flex-direction:column; background:#0b0f0e;">
			<div style="flex:0 0 auto; height:44px; display:flex; align-items:center; gap:14px; padding:0 14px;
			background:#0f1b18; border-bottom:2px solid #63e9cd; color:#63e9cd; font:700 15px system-ui;">
				"InSilico (Leptos)"
			</div>
			<div style="flex:1 1 auto; position:relative;">
				<PackedArea
					panels=panels
					config=Config::default()
					on_ready=on_ready
					request_tab=request_tab
				/>
			</div>
		</div>
	}
}
