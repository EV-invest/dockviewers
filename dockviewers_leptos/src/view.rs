//! The Leptos view over [`PackedState`]. One `RwSignal<PackedState, LocalStorage>` (thread-local:
//! the state holds `Rc`s, so it is `!Send`) drives everything. The tile skeleton is a coarse
//! reactive block — it holds no user content, so rebuilding it on every state change is harmless —
//! while the content overlay is a keyed `<For>` whose per-panel position is a fine-grained reactive
//! style, so a panel's node survives layout restructuring while still tracking its tile. Every
//! `on:*` reads one DOM primitive and forwards it to the identical core reducer method.

use std::{collections::HashMap, sync::Arc};

use dockviewers_core::{Band, CSS, Config, Group, GroupId, MinSize, PackedState, PanelId, model::serial::LoadError};
use leptos::{ev, prelude::*};

use crate::panel::DockPanel;

/// Imperative handle over the packed layout, handed to the host via `on_band`: a thin wrapper over
/// the thread-local `RwSignal<PackedState>` so the host can script tiles, save/load, and read the
/// live band from outside the component. Every method is a `with`/`update` of the one cell.
#[derive(Clone, Copy)]
pub struct PackedApi {
	state: RwSignal<PackedState, LocalStorage>,
}

impl PackedApi {
	pub fn place(&self, group: Group, w: u32, h: u32, min: MinSize) {
		self.state.update(|s| s.place(group, w, h, min));
	}

	pub fn add_tab(&self, group: GroupId, panel: PanelId) {
		self.state.update(|s| s.add_tab(group, panel));
	}

	pub fn close_active(&self, group: GroupId) {
		self.state.update(|s| s.close_active(group));
	}

	pub fn resize(&self, idx: usize, new_w: u32, new_h: u32) {
		self.state.update(|s| s.resize(idx, new_w, new_h));
	}

	pub fn mint_group_id(&self) -> GroupId {
		self.state.try_update(|s| s.mint_group_id()).expect("state signal alive")
	}

	pub fn cols(&self) -> u32 {
		self.state.with(|s| s.cols())
	}

	/// The band the layout is stored under.
	pub fn band(&self) -> Band {
		self.state.with(|s| s.band())
	}

	/// Whether `on_band`'s invocation follows a restore from the seed cache. Only meaningful inside
	/// `on_band` — a fresh band arrives with an empty grid, and anything the host places fills it.
	pub fn restored(&self) -> bool {
		self.state.with(|s| !s.grid().cells.is_empty())
	}

	pub fn reset(&self) {
		self.state.update(|s| s.reset());
	}

	/// Every panel id the live layout hosts, in cell/tab order — for a host rebuilding its panel list.
	pub fn tab_ids(&self) -> Vec<PanelId> {
		self.state.with(|s| s.grid().cells.iter().flat_map(|c| c.group.tabs.iter().cloned()).collect())
	}

	pub fn save(&self) -> String {
		self.state.with(|s| s.save())
	}

	pub fn load(&self, json: &str) -> Result<(), LoadError> {
		self.state.try_update(|s| s.load(json)).expect("state signal alive")
	}
}

/// Root of the packed layout. Owns the thread-local `RwSignal<PackedState>`, installs the window
/// keydown/pointer listeners, measures its own box, and stacks the tile skeleton over the content
/// overlay.
///
/// - `panels` is a signal so windows spawned at runtime appear in the overlay.
/// - `on_band`: invoked with the [`PackedApi`] once per [`Band`] entry — after the first measure and
///   again on every band crossing — *after* the framework has resolved that band's seed cache.
///   [`PackedApi::restored`] says whether it found one; if not, the grid is empty and the host is
///   expected to place its tiles (or load its own default) against the now-real step size.
/// - `request_tab`: the per-tile `+` button calls it to ask the host to open a new tab.
#[component]
pub fn PackedArea(
	panels: RwSignal<Vec<DockPanel>>,
	#[prop(optional)] config: Option<Config>,
	#[prop(optional)] on_band: Option<Arc<dyn Fn(PackedApi) + Send + Sync>>,
	#[prop(optional)] request_tab: Option<Arc<dyn Fn(GroupId) + Send + Sync>>,
) -> impl IntoView {
	let cfg = config.unwrap_or_default();
	let title_h_rem = cfg.title_h_rem;
	let state = RwSignal::new_local(PackedState::new(cfg));
	let api = PackedApi { state };
	let root = NodeRef::<leptos::html::Div>::new();

	// The first measure, once the root mounts (its content box lands a real step size).
	root.on_load({
		let on_band = on_band.clone();
		move |el| measure(el.into(), state, api, on_band.as_ref())
	});

	// A container resize (window is the only source) may cross a band, so it runs the latch too.
	window_event_listener(ev::resize, move |_| {
		if let Some(el) = root.get_untracked() {
			measure(el.into(), state, api, on_band.as_ref());
		}
	});

	// Undo history: snapshot a settled layout after each structural edit. The `wants_undo_snapshot`
	// read subscribes this effect; the guarded update doesn't re-fire it into a loop.
	Effect::new(move |_| {
		if state.with(|s| s.wants_undo_snapshot()) {
			state.update(|s| s.capture_undo());
		}
	});

	// App-level keybinds: a window keydown listener plus a sibling pointermove tracking the raw
	// cursor for the `d` hit-test. `on_key` is web-only (it logs to the console), so gate the block.
	#[cfg(target_arch = "wasm32")]
	{
		use std::{cell::Cell, rc::Rc};

		use wasm_bindgen::JsCast;
		let cursor = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
		let track = cursor.clone();
		window_event_listener(ev::pointermove, move |e| track.set((e.client_x() as f64, e.client_y() as f64)));
		window_event_listener(ev::keydown, move |e| {
			// Don't hijack typing: ignore keys aimed at an editable field.
			if let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
				if matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT") || el.dyn_ref::<web_sys::HtmlElement>().is_some_and(web_sys::HtmlElement::is_content_editable) {
					return;
				}
			}
			let (cx, cy) = cursor.get();
			let out = state.try_update(|s| s.on_key(&e.key(), e.alt_key(), e.ctrl_key(), (cx, cy), packed_scroll_y()));
			if out.map(|o| o.prevent_default).unwrap_or(false) {
				e.prevent_default();
			}
		});
	}

	view! {
		<style>{CSS}</style>
		// `--dv-title-h` is the stylesheet's one knob for the chrome band; the state resolves the same
		// rem to px for the content overlay's offset.
		<div class="dv-packed" node_ref=root style=format!("--dv-title-h: {title_h_rem}rem;")>
			// Tile skeleton — coarse re-render on any state change (no user content lives here).
			{move || {
				let titles = panel_titles(panels);
				state
					.with(|s| s.frames(&titles))
					.into_iter()
					.map(|f| render_frame(f, state, request_tab.clone()))
					.collect_view()
			}}

			// Content overlay — keyed so a panel's node survives; position is a fine-grained reactive style.
			<div class="dv-overlay">
				<For
					each=move || panels.get()
					key=|p| p.id.0.clone()
					children=move |panel| {
						let pid = panel.id.clone();
						let pid_click = panel.id.clone();
						let content = panel.content.clone();
						view! {
							<div
								class="dv-render-overlay"
								style=move || {
									state
										.with(|s| {
											s.content_slots()
												.get(&pid)
												.map(|sl| sl.style.clone())
												.unwrap_or_else(|| "display:none;".to_string())
										})
								}
								on:pointerdown=move |_| {
									if let Some(g) = state
										.with_untracked(|s| s.content_slots().get(&pid_click).map(|sl| sl.group))
									{
										state.update(|s| s.focus(g));
									}
								}
							>
								{content()}
							</div>
						}
					}
				/>
			</div>

			// Inspect popups.
			{move || {
				state
					.with(|s| s.inspect())
					.into_iter()
					.map(|(style, label)| {
						view! {
							<div class="dv-inspect" style=style>
								<span>{label}</span>
							</div>
						}
					})
					.collect_view()
			}}

			// Drag capture surface: owns pointermove/up for the whole gesture so moves over tiles don't leak.
			{move || {
				state
					.with(|s| s.dragging())
					.then(|| {
						view! {
							<div
								style="position:fixed; inset:0; z-index:1000; cursor:grabbing;"
								on:pointermove=move |e: web_sys::PointerEvent| {
									state
										.update(|s| {
											s.drag_move((e.client_x() as f64, e.client_y() as f64), packed_scroll_y())
										});
								}
								on:pointerup=move |_| state.update(|s| s.drag_release())
								on:pointercancel=move |_| state.update(|s| s.drag_cancel())
							></div>
						}
					})
			}}

			// Cursor-only scrim while resizing.
			{move || {
				state
					.with(|s| s.resizing())
					.then(|| {
						view! { <div style="position:fixed; inset:0; z-index:1000; cursor:nwse-resize;"></div> }
					})
			}}

			// Floating drag ghost.
			{move || {
				let titles = panel_titles(panels);
				state
					.with(|s| s.ghost(&titles))
					.map(|g| {
						view! {
							<div
								class="dv-ghost"
								style=format!(
									"left:{}px; top:{}px; width:{}px; height:{}px;",
									g.left,
									g.top,
									g.w,
									g.h,
								)
							>
								<div class="dv-header">
									<div class="dv-tab dv-active">{g.title}</div>
								</div>
							</div>
						}
					})
			}}

			// `?` keybind hint.
			{move || {
				state
					.with(|s| s.help_open())
					.then(|| {
						let rows = state.with(|s| s.help_rows());
						view! {
							<div class="dv-help-scrim" on:click=move |_| state.update(|s| s.close_help())>
								<div class="dv-help">
									<div class="dv-help-title">"Keybinds"</div>
									{rows
										.into_iter()
										.map(|(label, chord)| {
											view! {
												<div class="dv-help-row">
													<span class="dv-help-label">{label}</span>
													<span class="dv-help-key">{chord}</span>
												</div>
											}
										})
										.collect_view()}
									<div class="dv-help-foot">"click anywhere or press ? to close"</div>
								</div>
							</div>
						}
					})
			}}
		</div>
	}
}

/// Panel id → tab label, read from the panels signal (tracks it).
fn panel_titles(panels: RwSignal<Vec<DockPanel>>) -> HashMap<PanelId, String> {
	panels.with(|ps| ps.iter().map(|p| (p.id.clone(), p.title.clone())).collect())
}

/// One tile of the skeleton. A shadow frame is a bare grey placeholder; a full frame renders its
/// header (move-handle + tabs + actions), a body spacer, and the resize grip — each wired to a core
/// reducer method.
fn render_frame(frame: dockviewers_core::Frame, state: RwSignal<PackedState, LocalStorage>, request_tab: Option<Arc<dyn Fn(GroupId) + Send + Sync>>) -> AnyView {
	if frame.shadow {
		return view! { <div class="dv-tile dv-shadow" style=frame.style></div> }.into_any();
	}
	let gid = frame.gid;
	let idx = frame.idx;
	let header_class = if frame.tab_drop { "dv-header dv-tab-drop" } else { "dv-header" };
	let tabs = frame
		.tabs
		.into_iter()
		.map(move |tab| {
			let id = tab.id.clone();
			view! {
				<div
					data-panel=tab.id.0.clone()
					class=if tab.active { "dv-tab dv-active" } else { "dv-tab" }
					on:pointerdown=move |e: web_sys::PointerEvent| {
						if e.button() != 0 {
							return;
						}
						e.stop_propagation();
						state
							.update(|s| {
								s.press_tab(
									id.clone(),
									gid,
									(e.client_x() as f64, e.client_y() as f64),
									(e.offset_x() as f64, e.offset_y() as f64),
								)
							});
					}
				>
					{tab.title}
				</div>
			}
		})
		.collect_view();
	view! {
		<div class="dv-tile" style=frame.style>
			<div class="dv-group">
				<div
					class=header_class
					data-dvg=gid.0
					on:pointerdown=move |e: web_sys::PointerEvent| {
						if e.button() != 0 {
							return;
						}
						e.stop_propagation();
						state
							.update(|s| {
								s.press_tile(
									gid,
									(e.client_x() as f64, e.client_y() as f64),
									(e.offset_x() as f64, e.offset_y() as f64),
								)
							});
					}
				>
					{tabs}
					<div class="dv-actions">
						<button
							class="dv-action"
							title="Add window as a tab"
							on:pointerdown=move |e: web_sys::PointerEvent| e.stop_propagation()
							on:click=move |_| {
								if let Some(rt) = request_tab.clone() {
									rt(gid);
								}
							}
						>
							"+"
						</button>
						<button
							class="dv-action"
							title="Close"
							on:pointerdown=move |e: web_sys::PointerEvent| e.stop_propagation()
							on:click=move |_| state.update(|s| s.close_active(gid))
						>
							"✕"
						</button>
					</div>
				</div>
				<div class="dv-content-slot"></div>
			</div>
			<div
				class="dv-resize-handle"
				on:pointerdown=move |e: web_sys::PointerEvent| {
					if e.button() != 0 {
						return;
					}
					e.stop_propagation();
					capture_pointer(&e);
					state.update(|s| s.resize_start(idx, e.client_x() as f64, e.client_y() as f64));
				}
				on:pointermove=move |e: web_sys::PointerEvent| {
					state.update(|s| s.resize_move((e.client_x() as f64, e.client_y() as f64)));
				}
				on:pointerup=move |_| state.update(|s| s.resize_end())
				on:pointercancel=move |_| state.update(|s| s.resize_end())
			></div>
		</div>
	}
	.into_any()
}

/// Hand the root's origin + content box to the core measure, then resolve the band: the latch has
/// already restored-or-reset the grid by the time `on_band` runs, so the host only fills in what it
/// left behind.
fn measure(el: web_sys::Element, state: RwSignal<PackedState, LocalStorage>, api: PackedApi, on_band: Option<&Arc<dyn Fn(PackedApi) + Send + Sync>>) {
	let rect = el.get_bounding_client_rect();
	state.update(|s| s.set_measure((rect.x(), rect.y()), (el.client_width() as f64, el.client_height() as f64)));
	if state.try_update(|s| s.take_band()).expect("state signal alive").is_some()
		&& let Some(cb) = on_band
	{
		cb(api);
	}
}

/// The `.dv-packed` root's scroll offset — bridges the tiles' scrolled content space and the
/// pointer's client space for a drag/keybind hit-test.
fn packed_scroll_y() -> f64 {
	web_sys::window()
		.and_then(|w| w.document())
		.and_then(|d| d.query_selector(".dv-packed").ok().flatten())
		.map(|el| el.scroll_top() as f64)
		.unwrap_or(0.0)
}

/// `setPointerCapture` on the resize grip so its pointermove keeps firing past the viewport edge.
fn capture_pointer(e: &web_sys::PointerEvent) {
	use wasm_bindgen::JsCast;
	if let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
		// A live element with an active pointer — a failed capture just means a non-captured resize.
		let _ = el.set_pointer_capture(e.pointer_id());
	}
}
