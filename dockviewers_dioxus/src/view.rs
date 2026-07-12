//! The Dioxus view over [`PackedState`]: one root-scope `Signal<PackedState>` replacing the old
//! ~14 separate signals, three `rsx!` blocks (root / frame / content) rendered straight from the
//! core view-model getters, and event handlers that each read one DOM primitive then forward it to
//! a reducer method. No layout logic lives here — only the reactive cell, the templates, and the
//! DOM reads the core can't do itself.

use std::collections::HashMap;

use dioxus::{html::input_data::MouseButton, prelude::*};
use dockviewers_core::{Breakpoint, CSS, Config, Group, GroupId, MinSize, PackedState, PanelId};

use crate::panel::DockPanel;

/// Imperative handle over the packed layout, handed to the host via `on_ready`: a thin wrapper
/// over the root-scope `Signal<PackedState>` so the host can script tiles, save/load, and read the
/// live band from *outside* [`PackedArea`]'s subtree. Every method is a `read`/`write` of the one
/// cell; the reducer does the work.
#[derive(Clone, Copy)]
pub struct PackedApi {
	state: Signal<PackedState>,
}

impl PackedApi {
	pub fn place(&mut self, group: Group, w: u32, h: u32, min: MinSize) {
		self.state.write().place(group, w, h, min);
	}

	pub fn add_tab(&mut self, group: GroupId, panel: PanelId) {
		self.state.write().add_tab(group, panel);
	}

	pub fn close_active(&mut self, group: GroupId) {
		self.state.write().close_active(group);
	}

	pub fn resize(&mut self, idx: usize, new_w: u32, new_h: u32) {
		self.state.write().resize(idx, new_w, new_h);
	}

	pub fn mint_group_id(&mut self) -> GroupId {
		self.state.write().mint_group_id()
	}

	pub fn cols(&self) -> u32 {
		self.state.read().cols()
	}

	pub fn breakpoint(&self) -> Breakpoint {
		self.state.read().breakpoint()
	}

	/// Wipe the layout back to empty (a host re-seeding a fresh band).
	pub fn reset(&mut self) {
		self.state.write().reset();
	}

	/// Every panel id the live layout hosts, in cell/tab order — for a host rebuilding its panel
	/// list from a restored layout.
	pub fn tab_ids(&self) -> Vec<PanelId> {
		self.state.read().grid().cells.iter().flat_map(|c| c.group.tabs.iter().cloned()).collect()
	}

	/// Serialize the current layout.
	pub fn save(&self) -> String {
		self.state.read().save()
	}

	/// Replace the layout with a previously [`save`](Self::save)d one; errors on a corrupt or
	/// future-version payload.
	pub fn load(&mut self, json: &str) -> Result<(), dockviewers_core::model::serial::LoadError> {
		self.state.write().load(json)
	}
}

/// Root of the packed layout. Owns the `Signal<PackedState>` (root-scope, so the [`PackedApi`] it
/// hands the host outlives this component), installs the window keydown/pointer listeners, measures
/// its own box, and stacks the tile skeleton over the content overlay.
///
/// - `panels` is a `Signal` so windows spawned at runtime appear in the overlay.
/// - `on_ready`: invoked once with the [`PackedApi`] after the first measure (so seeds can `place`
///   against a real step size), letting a host script the initial tiles.
/// - the host must provide a `Callback<GroupId>` context — the per-tile `+` button calls it to ask
///   the host to open a new tab.
#[component]
pub fn PackedArea(panels: Signal<Vec<DockPanel>>, on_ready: Option<Callback<PackedApi>>, config: Option<Config>) -> Element {
	let cfg = config.unwrap_or_default();
	// Root-scope, not this scope: `PackedApi` is driven from outside `PackedArea`'s subtree, so the
	// cell must outlive this component (this root component never unmounts).
	let mut state = use_hook(move || Signal::new_in_scope(PackedState::new(cfg), ScopeId::ROOT));
	let api = PackedApi { state };
	let request_tab = use_context::<Callback<GroupId>>();

	// Keybinds are app-level: a `window` keydown listener (not an element `onkeydown`, which fires
	// only while the dock subtree holds DOM focus — it usually doesn't). A sibling `pointermove`
	// tracks the raw cursor for the `d` hit-test. `forget` leaks both so they live for the whole app.
	#[cfg(target_arch = "wasm32")]
	use_hook(move || {
		use wasm_bindgen::{JsCast, closure::Closure};
		let cursor = std::rc::Rc::new(std::cell::Cell::new((0.0_f64, 0.0_f64)));
		let track = cursor.clone();
		let mv = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
			track.set((e.client_x() as f64, e.client_y() as f64));
		});
		let window = web_sys::window().expect("a browser window");
		window
			.add_event_listener_with_callback("pointermove", mv.as_ref().unchecked_ref())
			.expect("add pointermove listener");
		mv.forget();
		let mut state = state;
		let handler = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
			// Don't hijack typing: ignore keys aimed at a form field / editable content, so a bare
			// `u`/`f`/`Backspace` bind only acts on the layout, never on text the user is entering.
			if let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
				if matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT") || el.dyn_ref::<web_sys::HtmlElement>().is_some_and(web_sys::HtmlElement::is_content_editable) {
					return;
				}
			}
			let (cx, cy) = cursor.get();
			let out = state.write().on_key(&e.key(), e.alt_key(), e.ctrl_key(), (cx, cy), packed_scroll_y());
			if out.prevent_default {
				e.prevent_default();
			}
		});
		// Capture phase: fire before any descendant (Google Maps, Plotly, …) can `stopPropagation` a
		// keydown on its way up to `window`, which would otherwise silence every bind.
		window
			.add_event_listener_with_callback_and_bool("keydown", handler.as_ref().unchecked_ref(), true)
			.expect("add keydown listener");
		handler.forget();
	});

	// Undo history: snapshot a settled layout after each structural edit. `wants_undo_snapshot` is
	// read (to subscribe) before any write, so the resulting push doesn't re-fire this effect.
	use_effect(move || {
		if state.read().wants_undo_snapshot() {
			state.write().capture_undo();
		}
	});

	let titles: HashMap<PanelId, String> = panels.read().iter().map(|p| (p.id.clone(), p.title.clone())).collect();
	let frames = state.read().frames(&titles);
	let content = state.read().content_slots();
	let ghost = state.read().ghost(&titles);
	let inspect = state.read().inspect();
	let help_rows = state.read().help_rows();
	let dragging = state.read().dragging();
	let resizing = state.read().resizing();
	let help_open = state.read().help_open();
	let panel_list = panels.read().clone();

	rsx! {
		style { dangerous_inner_html: CSS }
		div {
			class: "dv-packed",
			onmounted: move |e| measure_and_seed(e, state, api, on_ready),
			onresize: move |_| remeasure(state),

			for frame in frames.iter() {
				div {
					key: "{frame.gid.0}",
					class: if frame.shadow { "dv-tile dv-shadow" } else { "dv-tile" },
					style: "{frame.style}",
					if !frame.shadow {
						div { class: "dv-group",
							div {
								class: if frame.tab_drop { "dv-header dv-tab-drop" } else { "dv-header" },
								"data-dvg": "{frame.gid.0}",
								onpointerdown: {
									let gid = frame.gid;
									move |e: PointerEvent| {
										if e.trigger_button() != Some(MouseButton::Primary) {
											return;
										}
										e.stop_propagation();
										let c = e.client_coordinates();
										let g = e.element_coordinates();
										state.write().press_tile(gid, (c.x, c.y), (g.x, g.y));
									}
								},
								for tab in frame.tabs.iter() {
									div {
										key: "{tab.id.0}",
										"data-panel": "{tab.id.0}",
										class: if tab.active { "dv-tab dv-active" } else { "dv-tab" },
										onpointerdown: {
											let (id, from) = (tab.id.clone(), frame.gid);
											move |e: PointerEvent| {
												if e.trigger_button() != Some(MouseButton::Primary) {
													return;
												}
												e.stop_propagation();
												let c = e.client_coordinates();
												let g = e.element_coordinates();
												state.write().press_tab(id.clone(), from, (c.x, c.y), (g.x, g.y));
											}
										},
										"{tab.title}"
									}
								}
								div { class: "dv-actions",
									button {
										class: "dv-action",
										title: "Add window as a tab",
										onpointerdown: |e: PointerEvent| e.stop_propagation(),
										onclick: {
											let gid = frame.gid;
											move |_| request_tab.call(gid)
										},
										"+"
									}
									button {
										class: "dv-action",
										title: "Close",
										onpointerdown: |e: PointerEvent| e.stop_propagation(),
										onclick: {
											let gid = frame.gid;
											move |_| {
												state.write().close_active(gid);
											}
										},
										"✕"
									}
								}
							}
							div { class: "dv-content-slot" }
						}
						div {
							class: "dv-resize-handle",
							onpointerdown: {
								let idx = frame.idx;
								move |e: PointerEvent| {
									if e.trigger_button() != Some(MouseButton::Primary) {
										return;
									}
									e.stop_propagation();
									// Capture the pointer: moves keep streaming past the window bottom while the button is
									// held, so the tile stretches below the fold (the root's `overflow-y: auto` then scrolls).
									capture_pointer(&e);
									let c = e.client_coordinates();
									state.write().resize_start(idx, c.x, c.y);
								}
							},
							onpointermove: move |e: PointerEvent| {
								let c = e.client_coordinates();
								state.write().resize_move((c.x, c.y));
							},
							onpointerup: move |_| state.write().resize_end(),
							onpointercancel: move |_| state.write().resize_end(),
						}
					}
				}
			}

			div { class: "dv-overlay",
				for panel in panel_list.iter() {
					div {
						key: "{panel.id.0}",
						class: "dv-render-overlay",
						style: content.get(&panel.id).map(|s| s.style.clone()).unwrap_or_else(|| "display:none;".to_string()),
						// Clicking a pane's body focuses it too (not just its header/tab), so pane keybinds act
						// on whichever pane you last interacted with.
						onpointerdown: {
							let group = content.get(&panel.id).map(|s| s.group);
							move |_| {
								if let Some(g) = group {
									state.write().focus(g);
								}
							}
						},
						{panel.content.clone()}
					}
				}
			}

			for (style, label) in inspect.iter() {
				div { class: "dv-inspect", style: "{style}", span { "{label}" } }
			}

			// Drag capture: a fixed surface (the Dioxus-web stand-in for `setPointerCapture`) that owns
			// pointermove/up for the whole gesture, so moves over child tiles don't leak.
			if dragging {
				div {
					style: "position:fixed; inset:0; z-index:1000; cursor:grabbing;",
					onpointermove: move |e: PointerEvent| {
						let c = e.client_coordinates();
						state.write().drag_move((c.x, c.y), packed_scroll_y());
					},
					onpointerup: move |_| state.write().drag_release(),
					onpointercancel: move |_| state.write().drag_cancel(),
				}
			}
			// Cursor-only scrim while resizing: the captured pointer routes moves to the handle; this just
			// keeps the resize cursor steady over whatever the pointer crosses.
			if resizing {
				div { style: "position:fixed; inset:0; z-index:1000; cursor:nwse-resize;" }
			}
			if let Some(g) = ghost {
				div { class: "dv-ghost", style: "left:{g.left}px; top:{g.top}px; width:{g.w}px; height:{g.h}px;",
					div { class: "dv-header", div { class: "dv-tab dv-active", "{g.title}" } }
				}
			}
			if help_open {
				div { class: "dv-help-scrim", onclick: move |_| state.write().close_help(),
					div { class: "dv-help",
						div { class: "dv-help-title", "Keybinds" }
						for (label, chord) in help_rows.iter() {
							div { class: "dv-help-row",
								span { class: "dv-help-label", "{label}" }
								span { class: "dv-help-key", "{chord}" }
							}
						}
						div { class: "dv-help-foot", "click anywhere or press ? to close" }
					}
				}
			}
		}
	}
}

/// The `.dv-packed` root's scroll offset — the tiles live in this scrolled content space, the
/// pointer in client space, so a drag/keybind hit-test bridges the two with it.
fn packed_scroll_y() -> f64 {
	#[cfg(target_arch = "wasm32")]
	{
		web_sys::window()
			.expect("a browser window")
			.document()
			.expect("a document")
			.query_selector(".dv-packed")
			.expect("static selector")
			.expect("packed root in DOM while a gesture is live")
			.scroll_top() as f64
	}
	#[cfg(not(target_arch = "wasm32"))]
	0.0
}

/// Read the root's `getBoundingClientRect` origin + content box (`clientWidth/Height`, which
/// exclude the scrollbar gutter the tiles must not spill into) and hand them to the core measure;
/// then seed once past the first real step size.
fn measure_and_seed(e: MountedEvent, mut state: Signal<PackedState>, api: PackedApi, on_ready: Option<Callback<PackedApi>>) {
	#[cfg(target_arch = "wasm32")]
	{
		use dioxus::web::WebEventExt;
		if let Some(el) = e.data().try_as_web_event() {
			let rect = el.get_bounding_client_rect();
			state.write().set_measure((rect.x(), rect.y()), (el.client_width() as f64, el.client_height() as f64));
			if state.write().take_ready() {
				if let Some(cb) = on_ready {
					cb.call(api);
				}
			}
		}
	}
	#[cfg(not(target_arch = "wasm32"))]
	let _ = (e, &mut state, api, on_ready);
}

/// Re-measure on a container resize.
fn remeasure(mut state: Signal<PackedState>) {
	#[cfg(target_arch = "wasm32")]
	{
		use wasm_bindgen::JsCast;
		if let Some(el) = web_sys::window()
			.and_then(|w| w.document())
			.and_then(|d| d.query_selector(".dv-packed").ok().flatten())
			.and_then(|el| el.dyn_into::<web_sys::Element>().ok())
		{
			let rect = el.get_bounding_client_rect();
			state.write().set_measure((rect.x(), rect.y()), (el.client_width() as f64, el.client_height() as f64));
		}
	}
	#[cfg(not(target_arch = "wasm32"))]
	let _ = &mut state;
}

/// `setPointerCapture` on the resize grip so its pointermove keeps firing past the viewport edge.
#[cfg(target_arch = "wasm32")]
fn capture_pointer(e: &PointerEvent) {
	use dioxus::web::WebEventExt;
	use wasm_bindgen::JsCast;
	let ev = e.data().as_web_event();
	ev.target()
		.and_then(|t| t.dyn_into::<web_sys::Element>().ok())
		.expect("pointerdown target is the handle element")
		.set_pointer_capture(ev.pointer_id())
		.expect("capture on a live element with an active pointer");
}
#[cfg(not(target_arch = "wasm32"))]
fn capture_pointer(_: &PointerEvent) {}
