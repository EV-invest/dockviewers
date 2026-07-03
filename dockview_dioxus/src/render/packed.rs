//! Render layer for the packed-grid model ([`model::packed`](crate::model::packed)) —
//! fixed-size tiles that snap to a step grid and leave whitespace instead of filling
//! the view. InsilicoTerminal's look.
//!
//! The skeleton ([`PackedFrame`]) and the content overlay ([`PackedContent`]) are both
//! positioned directly from the model's integer grid rects (`x·STEP, y·STEP, …`), in the
//! packed root's own coordinate space. There is no DOM measuring: a tile and its content
//! share the exact same math, so content can never drift off its tile when the layout
//! settles (a measured slot only re-measures on *resize*, never on a pure move — the source
//! of the old "content nested in a neighbour" bug).
//!
//! Drag-to-reposition is a pure grid op: pick up a tile (titlebar) or tear a tab, and a
//! cloned [`PackedGrid::drop`] previews the result live (`view`) — other tiles settling under
//! gravity, the source's landing cell drawn as a detail-less grey shadow — committed verbatim on
//! release. The picked-up pane itself rides a floating ghost under the pointer (it does not snap);
//! only its shadow snaps ahead to where it will land. One `drop` path for preview and commit.

use std::{
	collections::{HashMap, HashSet},
	rc::Rc,
};

use dioxus::{html::input_data::MouseButton, prelude::*};

use super::CSS;
use crate::{
	config::{Breakpoint, Config},
	math::Rect,
	model::{
		Group, GroupId, PanelId,
		packed::{DragSource, DropTarget, GridState, MinSize, PackedGrid},
		serial,
	},
	panel::DockPanel,
};

/// Approx root font size; bridges rem ⇄ px so a type's rem-expressed [`MinSize`] resolves to whole
/// steps against the live step size.
const REM_PX: f64 = 16.0;
/// Fixed header-bar height (CSS pins it); content starts below it.
const CHROME_H: f64 = 32.0;
/// A press promotes to a drag only past this many px, so a click still activates a tab.
const DRAG_THRESHOLD: f64 = 4.0;

/// Imperative handle over the packed grid: thin `Signal` writes over [`PackedGrid`],
/// with `place` resolving [`MinSize`] → steps first. `cols` is the live column count
/// the render root measures.
#[derive(Clone, Copy)]
pub struct PackedApi {
	pub grid: Signal<PackedGrid>,
	pub cols: Signal<u32>,
	/// Live px-per-step on each axis (`width / cols`, `height / rows`), the units [`MinSize::Rem`]
	/// resolves against.
	pub step_px: Signal<(f64, f64)>,
	/// Live responsive band (classified from the container width). `cols`/`rows` are this band's
	/// scaling of the config's desktop counts, so a host persists the view separately per band by
	/// keying storage on it (see [`Breakpoint`]).
	pub breakpoint: Signal<Breakpoint>,
}

impl PackedApi {
	pub fn place(&mut self, group: Group, w: u32, h: u32, min: MinSize) {
		let cols = (self.cols)();
		let (step_w, step_h) = (self.step_px)();
		assert!(step_w > 0.0 && step_h > 0.0, "place before the first measure: no step size yet");
		self.grid.write().place(group, w, h, min.resolve(step_w, step_h, REM_PX), cols);
	}

	pub fn add_tab(&mut self, group: GroupId, panel: PanelId) {
		self.grid.write().add_tab(group, panel);
	}

	pub fn close_active(&mut self, group: GroupId) {
		self.grid.write().close_active(group);
	}

	pub fn resize(&mut self, idx: usize, new_w: u32, new_h: u32) {
		let cols = (self.cols)();
		self.grid.write().resize(idx, new_w, new_h, cols);
	}

	/// Serialize the current layout (see [`serial`](crate::model::serial)).
	pub fn save(&self) -> String {
		serial::save(&self.grid.read())
	}

	/// Replace the layout with a previously [`save`](Self::save)d one; errors (not a silent
	/// reset) on a corrupt or future-version payload.
	pub fn load(&mut self, json: &str) -> Result<(), serial::LoadError> {
		*self.grid.write() = serial::load(json)?;
		// The file may have been saved in another band: settle it into the live geometry.
		let cols = *self.cols.peek();
		self.grid.write().refit(cols);
		Ok(())
	}
}

/// Root of the packed layout. Owns the `Signal<PackedGrid>`, measures its own size (→ the per-axis
/// step px that stretches the fixed `cols × rows` grid to fill it) and top-left origin (to map
/// pointer→grid space), provides [`PackedApi`]/the panels signal/the drag signal/the preview `view`
/// via context, and stacks the tile skeleton over the content overlay.
///
/// - `panels` is a `Signal` so windows spawned at runtime appear in the overlay.
/// - `on_ready`: invoked once with the [`PackedApi`] after the first measure (so seeds can
///   `place` against a real step size), letting a host script the initial tiles.
#[component]
pub fn PackedArea(panels: Signal<Vec<DockPanel>>, on_ready: Option<Callback<PackedApi>>, config: Option<Config>) -> Element {
	let cfg = config.unwrap_or_default();
	let steps = cfg.steps.max(1);
	let rows = cfg.rows.max(1);
	// Owned by the root, not this scope: `PackedApi` is handed to the host via `on_ready` and
	// driven from outside `PackedArea`'s subtree, so the signals must outlive this component.
	let mut grid = use_hook(|| Signal::new_in_scope(PackedGrid::default(), ScopeId::ROOT));
	// Column count for the current band: the grid always spans these, stretching the step size. Set
	// by the fit effect below from the measured width; seeded at the desktop count.
	let mut cols = use_hook(|| Signal::new_in_scope(steps, ScopeId::ROOT));
	// Px-per-step on each axis the whole render uses; recomputed by the fit effect below. (0, 0)
	// until first measure.
	let mut step_px = use_hook(|| Signal::new_in_scope((0.0_f64, 0.0_f64), ScopeId::ROOT));
	let mut breakpoint = use_hook(|| Signal::new_in_scope(Breakpoint::default(), ScopeId::ROOT));
	let api = PackedApi { grid, cols, step_px, breakpoint };
	let mut drag = use_signal(|| None::<Drag>);
	let mut root_origin = use_signal(|| (0.0_f64, 0.0_f64));
	let mut root_size = use_signal(|| (0.0_f64, 0.0_f64));
	// The pane keyboard ops act on: set when a tile's header/tab is pressed. `maximized` is a pure
	// view toggle (no model mutation) — the focused tile fills the container, the rest are hidden.
	let focused = use_signal(|| None::<GroupId>);
	let maximized = use_signal(|| None::<GroupId>);
	// `?` toggles a small overlay listing the active keybinds.
	let mut help = use_signal(|| false);
	// `d` toggles a translucent dimensions popup over the container the mouse points at; `Esc`
	// clears them all. Multiple can be open at once (one per group).
	let popups = use_signal(HashSet::<GroupId>::new);

	// Undo history of *solid* layouts: a snapshot is captured (by the effect below) only when the
	// grid is at rest — no drag in flight, not mid-resize — and differs from the cursor's snapshot.
	// Because restoring sets `grid` back to exactly `states[cursor]`, an undo/redo write is a no-op
	// for that effect (it won't re-record itself). A normal edit truncates any redo branch.
	let mut undo = use_signal(UndoHistory::default);

	// Keybinds are app-level: a `window` keydown listener, not an element `onkeydown` (which would
	// only fire while the dock subtree holds DOM focus — it usually doesn't). `forget` leaks the
	// closure so the listener lives for the whole app (this root-scope component never unmounts).
	#[cfg(target_arch = "wasm32")]
	use_hook(|| {
		use wasm_bindgen::{JsCast, closure::Closure};
		let kb = cfg.keybinds;
		let actions = cfg.actions.clone();
		// Raw cursor, read only at `d`-press time to hit-test the grid. A plain Cell (not a Signal)
		// because per-move reactivity would churn the whole render for nothing.
		let cursor = std::rc::Rc::new(std::cell::Cell::new((0.0_f64, 0.0_f64)));
		let track = cursor.clone();
		let mv = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
			track.set((e.client_x() as f64, e.client_y() as f64));
		});
		web_sys::window()
			.expect("a browser window")
			.add_event_listener_with_callback("pointermove", mv.as_ref().unchecked_ref())
			.expect("add pointermove listener");
		mv.forget();
		let handler = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
			// Don't hijack typing: ignore keys aimed at a form field / editable content, so a bare
			// `u`/`f`/`Backspace` bind only acts on the layout, never on text the user is entering.
			if let Some(el) = e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
				if matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT") || el.dyn_ref::<web_sys::HtmlElement>().is_some_and(web_sys::HtmlElement::is_content_editable) {
					return;
				}
			}
			let (mut grid, mut undo, mut focused, mut maximized, mut help, mut popups) = (grid, undo, focused, maximized, help, popups);
			let mut api = api;
			// `key` is the produced character (layout-aware), not the physical position; shift is
			// already baked into it (`"u"` vs `"U"`), so `matches` only checks alt/ctrl.
			let (key, alt, ctrl) = (e.key(), e.alt_key(), e.ctrl_key());
			// Logs the moment a chord is *recognized* (a configured bind matched), before the action
			// runs — so a console with this line but no visible effect means the action was a no-op
			// (nothing focused, history empty), not that the keypress went unseen. The likeliest
			// "works in Chrome, dead in Firefox" failure is the listener never seeing the event at
			// all, in which case this line is simply absent for every key.
			let recognized = |action: &str| web_sys::console::debug_1(&format!("dockview keybind: {key:?} (alt={alt}, ctrl={ctrl}) → {action}").into());
			// Esc always dismisses the hint and any open inspect popups, regardless of the binds.
			if key == "Escape" && (help() || !popups.read().is_empty()) {
				recognized("Escape");
				e.prevent_default();
				help.set(false);
				popups.write().clear();
				return;
			}
			if kb.undo.matches(&key, alt, ctrl) {
				recognized("undo");
				e.prevent_default();
				if let Some(g) = undo.write().step(-1) {
					*grid.write() = g;
				}
			} else if kb.redo.matches(&key, alt, ctrl) {
				recognized("redo");
				e.prevent_default();
				if let Some(g) = undo.write().step(1) {
					*grid.write() = g;
				}
			} else if kb.close.matches(&key, alt, ctrl) {
				recognized("close");
				if let Some(g) = focused() {
					e.prevent_default();
					api.close_active(g);
					// Dropped the last tab → the group is gone; don't leave focus/maximize on a dead id.
					if !grid.read().cells.iter().any(|c| c.group.id == g) {
						focused.set(None);
						if maximized() == Some(g) {
							maximized.set(None);
						}
					}
				}
			} else if kb.maximize.matches(&key, alt, ctrl) {
				recognized("maximize");
				let f = focused();
				if f.is_some() {
					e.prevent_default();
					maximized.set(if maximized() == f { None } else { f });
				}
			} else if kb.help.matches(&key, alt, ctrl) {
				recognized("help");
				e.prevent_default();
				help.set(!help());
			} else if key == "d" && !alt && !ctrl {
				recognized("inspect");
				let (cx, cy) = cursor.get();
				let (ox, oy) = root_origin();
				let (sw, sh) = (api.step_px)();
				let g = grid.read();
				let hit = if let Some(mg) = maximized() {
					Some(mg) // maximized tile fills the view
				} else {
					// Tiles live in the root's scrolled content space; the cursor in client space.
					let scroll_y = web_sys::window()
						.expect("a browser window")
						.document()
						.expect("a document")
						.query_selector(".dv-packed")
						.expect("static selector")
						.expect("packed root in DOM while its keybinds fire")
						.scroll_top() as f64;
					g.cells
						.iter()
						.find(|c| {
							let (gx, gy) = (cx - ox, cy - oy + scroll_y);
							gx >= c.x as f64 * sw && gx < (c.x + c.w) as f64 * sw && gy >= c.y as f64 * sh && gy < (c.y + c.h) as f64 * sh
						})
						.map(|c| c.group.id)
				};
				if let Some(gid) = hit {
					e.prevent_default();
					let mut p = popups.write();
					if !p.remove(&gid) {
						p.insert(gid);
					}
				}
			} else {
				for (bind, run) in &actions {
					if bind.matches(&key, alt, ctrl) {
						recognized("custom-action");
						e.prevent_default();
						run.call(api);
						break;
					}
				}
			}
		});
		let window = web_sys::window().expect("a browser window");
		// Capture phase: fire before any descendant (Google Maps, Plotly, …) can `stopPropagation`
		// a keydown on its way up to `window`, which would otherwise silence every bind.
		window
			.add_event_listener_with_callback_and_bool("keydown", handler.as_ref().unchecked_ref(), true)
			.expect("add keydown listener");
		handler.forget();
	});

	use_effect(move || {
		if drag.read().is_some() {
			return;
		}
		let g = grid.read();
		// `cells.is_empty()` skips the default grid before the host's seed lands, so undo can't
		// walk back to a blank layout.
		if g.state != GridState::Settled || g.cells.is_empty() {
			return;
		}
		let mut h = undo.write();
		if h.states.get(h.cursor) == Some(&*g) {
			return;
		}
		h.push(g.clone());
	});

	// The grid the skeleton/content actually render: the live preview while an armed drag is in
	// flight (other tiles settled under a cloned `drop`), else the real grid. One `drop` path.
	let view = use_memo(move || {
		let g = grid.read().clone();
		match drag.read().clone() {
			Some(d) if d.armed => match d.target {
				Some(t) => {
					let mut p = g;
					p.drop(d.source, t, cols());
					p
				}
				None => g,
			},
			_ => g,
		}
	});

	use_context_provider(|| api);
	use_context_provider(|| panels);
	use_context_provider(|| drag);
	use_context_provider(|| view);
	use_context_provider(|| StepPx(step_px));
	use_context_provider(|| PaneView { focused, maximized });
	let mut root_handle = use_signal(|| None::<Rc<MountedData>>);
	let mut ready = use_signal(|| false);

	// Seed once, but only after the first measure lands a real step size — `place` resolves a
	// type's rem [`MinSize`] against it, so seeding before the measure would have no scale.
	use_effect(move || {
		if ready() || step_px().0 <= 0.0 {
			return;
		}
		if let Some(cb) = on_ready {
			cb.call(api);
		}
		ready.set(true);
	});

	let measure = move |h: Rc<MountedData>| async move {
		if let Ok(rect) = h.get_client_rect().await {
			let r: Rect = rect.into();
			root_origin.set((r.x, r.y));
			// Size the grid from the *content* box, not the bounding rect: getBoundingClientRect's
			// width/height include the vertical scrollbar's gutter, but the tiles live in
			// clientWidth/Height, which exclude it. Dividing the wider rect by `cols` sized the
			// rightmost column to spill under the scrollbar and grow a spurious horizontal one.
			#[cfg(target_arch = "wasm32")]
			let (w, ht) = {
				use dioxus::web::WebEventExt;
				h.try_as_web_event().map(|e| (e.client_width() as f64, e.client_height() as f64)).unwrap_or((r.width, r.height))
			};
			#[cfg(not(target_arch = "wasm32"))]
			let (w, ht) = (r.width, r.height);
			root_size.set((w, ht));
		}
	};

	// Classify the measured width into a responsive band, then stretch the grid across the container:
	// a horizontal step is `width / cols` (cols scaled per band), a vertical one `height / rows` (rows
	// fixed). Fewer columns on a narrower band keep the step's physical size ~constant, so tiles
	// reflow/stack down on a phone instead of shrinking. `peek`s (not reads) on the values we also
	// write, so the effect doesn't re-trigger itself.
	use_effect(move || {
		let (width, height) = root_size();
		if width <= 0.0 || height <= 0.0 {
			return;
		}
		let bp = Breakpoint::of(width);
		let c = bp.scale_cols(steps);
		if *breakpoint.peek() != bp {
			breakpoint.set(bp);
		}
		if *cols.peek() != c {
			cols.set(c);
			// Reflow tiles into the new band so a layout built wide never spills past a narrower view.
			grid.write().refit(c);
		}
		step_px.set((width / c as f64, height / rows as f64));
	});

	// The floating ghost of whatever is being dragged: it tracks the pointer 1:1 (`cursor − grab`),
	// keeping the grabbed point under the cursor, while its shadow snaps to the landing cell.
	let ghost = drag.read().clone().filter(|d| d.armed).map(|d| {
		let titles: HashMap<PanelId, String> = panels.read().iter().map(|p| (p.id.clone(), p.title.clone())).collect();
		let title = match &d.source {
			DragSource::Tile(g) => grid
				.read()
				.cells
				.iter()
				.find(|c| c.group.id == *g)
				.map(|c| titles.get(c.group.active_panel()).cloned().unwrap_or_default())
				.unwrap_or_default(),
			DragSource::Tab { panel, .. } => titles.get(panel).cloned().unwrap_or_default(),
		};
		let (sw, sh) = step_px();
		(title, d.cursor.0 - d.grab.0, d.cursor.1 - d.grab.1, d.src_w as f64 * sw, d.src_h as f64 * sh)
	});

	let ids: Vec<u64> = view.read().cells.iter().map(|c| c.group.id.0).collect();
	// Inspect popups: one translucent `WxH` box per open group, placed from the same grid rect the
	// tile uses. Dead/closed groups simply drop out of the filter (the set may keep stale ids).
	let inspect: Vec<(String, String)> = {
		let (sw, sh) = step_px();
		let p = popups.read();
		view.read()
			.cells
			.iter()
			.filter(|c| p.contains(&c.group.id))
			.map(|c| {
				(
					format!(
						"left:{}px; top:{}px; width:{}px; height:{}px;",
						c.x as f64 * sw,
						c.y as f64 * sh,
						c.w as f64 * sw,
						c.h as f64 * sh
					),
					format!("{}x{}", c.w, c.h),
				)
			})
			.collect()
	};
	let help_rows: Vec<(&str, String)> = [
		("Undo", cfg.keybinds.undo),
		("Redo", cfg.keybinds.redo),
		("Close pane", cfg.keybinds.close),
		("Maximize pane", cfg.keybinds.maximize),
		("Toggle this hint", cfg.keybinds.help),
	]
	.into_iter()
	.map(|(label, b)| (label, format!("{}{}{}", if b.ctrl { "Ctrl+" } else { "" }, if b.alt { "Alt+" } else { "" }, b.key)))
	.chain(std::iter::once(("Inspect pane", "d".into())))
	.collect();
	// Cross-target sink for `cfg`: on non-wasm the keydown listener is absent, so this is the only
	// consumer keeping `cfg` from orphaning into an unused-var warning.
	use_context_provider(|| cfg);
	rsx! {
		style { dangerous_inner_html: CSS }
		div {
			class: "dv-packed",
			onmounted: move |e| {
				let h = e.data();
				root_handle.set(Some(h.clone()));
				measure(h)
			},
			onresize: move |_| {
				let handle = root_handle();
				async move {
					if let Some(h) = handle {
						measure(h).await;
					}
				}
			},
			for (idx, id) in ids.iter().enumerate() {
				PackedFrame { key: "{id}", idx }
			}
			div { class: "dv-overlay", PackedContent {} }

			for (style , label) in inspect.iter() {
				div { class: "dv-inspect", style: "{style}", span { "{label}" } }
			}

			// Drag capture: a fixed surface (the Dioxus-web stand-in for `setPointerCapture`)
			// that owns pointermove/up for the whole gesture, so moves over child tiles don't
			// leak. Inlined (not a component) so it shares the root's signals without `PartialEq`
			// props. Arms past the threshold, resolves the live target each move, and on release
			// runs the *same* `drop` the preview used; an unarmed tab release is a plain click.
			if drag.read().is_some() {
				div {
					style: "position:fixed; inset:0; z-index:1000; cursor:grabbing;",
					onpointermove: move |e: PointerEvent| {
						let c = e.client_coordinates();
						async move {
							let Some(mut d) = drag() else { return };
							d.cursor = (c.x, c.y);
							if !d.armed {
								let (dx, dy) = (c.x - d.start.0, c.y - d.start.1);
								if (dx * dx + dy * dy).sqrt() <= DRAG_THRESHOLD {
									return;
								}
								d.armed = true;
							}
							let (ox, oy) = root_origin();
							// The tiles live in the root's scrolled content space; the pointer in client space.
							let scroll_y = root_handle.peek().clone().expect("root mounted before any gesture").get_scroll_offset().await.expect("root is a live element").y;
							// Reference the moving block's center (ghost top-left + half its size), not the raw
							// pointer — the cell the block visibly covers is where it lands.
							let (sw, sh) = step_px();
							let cx = c.x - d.grab.0 + d.src_w as f64 * sw / 2.0;
							let cy = c.y - d.grab.1 + d.src_h as f64 * sh / 2.0;
							let mut t = grid.read().resolve_target(&d.source, cx - ox, cy - oy + scroll_y, c.x - ox, c.y - oy + scroll_y, sw, sh, CHROME_H, cols(), d.src_w, d.src_h);
							// The model can only append (it has no tab geometry); refine the slot from the live
							// preview's tab rects, skipping the ghost's own tab(s) so we read the source-free order.
							if let DropTarget::Tab { group, index } = t {
								let dragged: Vec<String> = match &d.source {
									DragSource::Tab { panel, .. } => vec![panel.0.clone()],
									DragSource::Tile(g) => grid.read().cells.iter().find(|c| c.group.id == *g).expect("drag source group exists").group.tabs.iter().map(|p| p.0.clone()).collect(),
								};
								t = DropTarget::Tab { group, index: tab_drop_index(c.x, group.0, &dragged).unwrap_or(index) };
							}
							d.target = Some(t);
							drag.set(Some(d));
						}
					},
					onpointerup: move |_| {
						let Some(d) = drag() else { return };
						if d.armed {
							if let Some(t) = d.target {
								grid.write().drop(d.source, t, cols());
							}
						} else if let DragSource::Tab { panel, from } = &d.source {
							let mut g = grid.write();
							if let Some(c) = g.cells.iter_mut().find(|c| c.group.id == *from) {
								if let Some(i) = c.group.tabs.iter().position(|p| p == panel) {
									c.group.active = i;
								}
							}
						}
						drag.set(None);
					},
					onpointercancel: move |_| drag.set(None),
				}
			}
			if let Some((title, left, top, gw, gh)) = ghost {
				div { class: "dv-ghost", style: "left:{left}px; top:{top}px; width:{gw}px; height:{gh}px;",
					div { class: "dv-header", div { class: "dv-tab dv-active", "{title}" } }
				}
			}
			if help() {
				div { class: "dv-help-scrim", onclick: move |_| help.set(false),
					div { class: "dv-help",
						div { class: "dv-help-title", "Keybinds" }
						for (label , chord) in help_rows.iter() {
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
/// The live render unit (**px** per step on each axis: `(width/cols, height/rows)`) shared with every
/// tile, the content overlay and the drag math via context, so all three scale together.
#[derive(Clone, Copy)]
struct StepPx(Signal<(f64, f64)>);

/// The keyboard-driven pane state, shared with the tiles via context: which group the pane ops
/// target (`focused`), and which — if any — is maximized to fill the container (`maximized`, a
/// pure view override that never touches the model).
#[derive(Clone, Copy)]
struct PaneView {
	focused: Signal<Option<GroupId>>,
	maximized: Signal<Option<GroupId>>,
}

/// Linear undo history of settled layouts with a cursor into it. `push` appends a fresh edit,
/// dropping any redo branch ahead of the cursor; `step` walks the cursor by ±1 and returns the
/// snapshot to restore (or `None` at an end).
// ponytail: linear, not a branching tree — two keys (undo/redo) can only express a line. Promote
// to a real tree once there's UI to pick a branch.
#[derive(Default)]
struct UndoHistory {
	states: Vec<PackedGrid>,
	cursor: usize,
}

impl UndoHistory {
	fn push(&mut self, g: PackedGrid) {
		if !self.states.is_empty() {
			self.states.truncate(self.cursor + 1);
		}
		self.states.push(g);
		self.cursor = self.states.len() - 1;
	}

	// Only the (web-only) keybind listener walks the history; `push` runs on every target.
	#[cfg(target_arch = "wasm32")]
	fn step(&mut self, dir: i32) -> Option<PackedGrid> {
		let next = self.cursor.checked_add_signed(dir as isize)?;
		let g = self.states.get(next)?.clone();
		self.cursor = next;
		Some(g)
	}
}

/// An in-flight reposition: what was picked up, where the press began (client px, to measure
/// the [`DRAG_THRESHOLD`]), the live pointer (to drag the ghost naturally), and — once `armed`
/// — the live [`DropTarget`]. `src_w`/`src_h` size both the ghost and a `Pack` target's column
/// clamp; `grab` is the pointer's offset within the picked-up element, so the ghost rides under
/// the same point you grabbed.
#[derive(Clone)]
struct Drag {
	source: DragSource,
	src_w: u32,
	src_h: u32,
	start: (f64, f64),
	grab: (f64, f64),
	cursor: (f64, f64),
	armed: bool,
	target: Option<DropTarget>,
}

/// Insertion slot for cursor x `mx` among `group`'s tabs in the live preview DOM: the count of
/// *real* tabs whose horizontal center is left of `mx` (so left-half ⇒ before, right-half ⇒
/// after). `dragged` are the panel ids the source carries — the preview already inserts them as
/// the floating ghost's tab(s), so they're skipped, leaving the source-free order this index
/// addresses (identical math whether re-homing into another group or reordering within one). The
/// one place this layer measures, because the model has no tab widths; `None` ⇒ DOM not ready,
/// caller keeps the append default.
#[cfg(target_arch = "wasm32")]
fn tab_drop_index(mx: f64, group: u64, dragged: &[String]) -> Option<usize> {
	use wasm_bindgen::JsCast;
	let doc = web_sys::window()?.document()?;
	// Selector is numeric-only, so it can't be malformed — `.ok()?` only trips if the doc is absent.
	let nodes = doc.query_selector_all(&format!("[data-dvg=\"{group}\"] .dv-tab")).ok()?;
	let mut idx = 0;
	for i in 0..nodes.length() {
		let Some(el) = nodes.get(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) else {
			continue;
		};
		// Every `.dv-tab` carries `data-panel`; default "" just classes a (nonexistent) attr-less tab as real.
		if dragged.iter().any(|p| *p == el.get_attribute("data-panel").unwrap_or_default()) {
			continue;
		}
		let r = el.get_bounding_client_rect();
		if mx > r.x() + r.width() / 2.0 {
			idx += 1;
		}
	}
	Some(idx)
}
#[cfg(not(target_arch = "wasm32"))]
fn tab_drop_index(_: f64, _: u64, _: &[String]) -> Option<usize> {
	None
}

/// Corner-resize gesture captured at `pointerdown`: pointer start + the tile's size (in steps) then.
#[derive(Clone, Copy)]
struct ResizeStart {
	px: f64,
	py: f64,
	w: u32,
	h: u32,
}

/// One tile: absolutely positioned at `x*STEP, y*STEP, w*STEP, h*STEP`, with a single header bar
/// (its empty area drags to reposition the whole tile; a tab drags to tear it out — the active
/// tab is the title, so there's no separate titlebar), an empty body filler, and a bottom-right
/// resize grip that snaps the pointer delta to whole steps. The `+`/`x` chrome (right of the
/// tabs): `+` asks the host (via a [`Callback<GroupId>`] context) to open a tab; `x` closes the
/// active tab (and removes the now-empty tile). The body is just a spacer — content rides in the
/// overlay, positioned from the same grid rect. Layout reads come from the preview `view`;
/// gestures write the real grid through [`PackedApi`].
#[component]
fn PackedFrame(idx: usize) -> Element {
	let mut api = use_context::<PackedApi>();
	let panels = use_context::<Signal<Vec<DockPanel>>>();
	let view = use_context::<Memo<PackedGrid>>();
	let mut drag = use_context::<Signal<Option<Drag>>>();
	let PaneView { mut focused, maximized } = use_context::<PaneView>();
	let request_tab = use_context::<Callback<GroupId>>();
	let step = use_context::<StepPx>().0;
	let mut resize = use_signal(|| None::<ResizeStart>);

	let titles: HashMap<PanelId, String> = panels.read().iter().map(|p| (p.id.clone(), p.title.clone())).collect();
	let (gid, x, y, w, h, tabs, active) = {
		let g = view.read();
		let c = &g.cells[idx];
		let tabs: Vec<(PanelId, String)> = c.group.tabs.iter().map(|id| (id.clone(), titles.get(id).cloned().unwrap_or_default())).collect();
		(c.group.id, c.x, c.y, c.w, c.h, tabs, c.group.active)
	};
	// Maximize is a view-only override: the focused tile fills the container (its real grid rect is
	// untouched), every other tile is omitted from the skeleton.
	match *maximized.read() {
		Some(mg) if mg != gid => return rsx! {},
		_ => {}
	}
	let style = if *maximized.read() == Some(gid) {
		"left:0; top:0; width:100%; height:100%;".to_string()
	} else {
		let (sw, sh) = step();
		format!("left:{}px; top:{}px; width:{}px; height:{}px;", x as f64 * sw, y as f64 * sh, w as f64 * sw, h as f64 * sh)
	};

	// While a drag is armed, mark this cell if it's where the source lands (a grey shadow for
	// Displace/Pack) or, for a Tab target, the group whose header is the drop site.
	let (is_shadow, tab_highlight) = match drag.read().clone() {
		Some(d) if d.armed => match d.target {
			Some(DropTarget::Tab { group, .. }) => (false, group == gid),
			Some(DropTarget::Displace { .. }) | Some(DropTarget::Pack { .. }) => {
				let src = match &d.source {
					DragSource::Tile(g) => *g == gid,
					DragSource::Tab { panel, .. } => tabs.iter().any(|(p, _)| p == panel),
				};
				(src, false)
			}
			None => (false, false),
		},
		_ => (false, false),
	};

	// The drag's landing cell: a detail-less grey placeholder snapped to where the floating
	// ghost will commit. No chrome, no content — just the greyed area it points at.
	if is_shadow {
		return rsx! { div { class: "dv-tile dv-shadow", style: "{style}" } };
	}

	let header_class = if tab_highlight { "dv-header dv-tab-drop" } else { "dv-header" };

	rsx! {
		div { class: "dv-tile", style: "{style}",
			div { class: "dv-group",
				div {
					class: "{header_class}",
					"data-dvg": "{gid.0}",
					onpointerdown: move |e: PointerEvent| {
						if e.trigger_button() != Some(MouseButton::Primary) {
							return;
						}
						e.stop_propagation();
						focused.set(Some(gid));
						let c = e.client_coordinates();
						let g = e.element_coordinates();
						drag.set(Some(Drag { source: DragSource::Tile(gid), src_w: w, src_h: h, start: (c.x, c.y), grab: (g.x, g.y), cursor: (c.x, c.y), armed: false, target: None }));
					},
					for (i , (id , t)) in tabs.iter().enumerate() {
						div {
							key: "{id.0}",
							"data-panel": "{id.0}",
							class: if i == active { "dv-tab dv-active" } else { "dv-tab" },
							onpointerdown: {
								let id = id.clone();
								move |e: PointerEvent| {
									if e.trigger_button() != Some(MouseButton::Primary) {
										return;
									}
									e.stop_propagation();
									focused.set(Some(gid));
									let c = e.client_coordinates();
									let g = e.element_coordinates();
									drag.set(Some(Drag {
										source: DragSource::Tab { panel: id.clone(), from: gid },
										src_w: w,
										src_h: h,
										start: (c.x, c.y),
										grab: (g.x, g.y),
										cursor: (c.x, c.y),
										armed: false,
										target: None,
									}));
								}
							},
							"{t}"
						}
					}
					div { class: "dv-actions",
						button {
							class: "dv-action",
							title: "Add window as a tab",
							onpointerdown: |e: PointerEvent| e.stop_propagation(),
							onclick: move |_| request_tab.call(gid),
							"+"
						}
						button {
							class: "dv-action",
							title: "Close",
							onpointerdown: |e: PointerEvent| e.stop_propagation(),
							onclick: move |_| api.close_active(gid),
							"✕"
						}
					}
				}
				div { class: "dv-content-slot" }
			}
			div {
				class: "dv-resize-handle",
				onpointerdown: move |e: PointerEvent| {
					if e.trigger_button() != Some(MouseButton::Primary) {
						return;
					}
					e.stop_propagation();
					// Capture the pointer: moves keep streaming — with client y running past the window
					// bottom — while the button is held, so the tile stretches below the fold instead of
					// pinning at the viewport (the root's `overflow-y: auto` then scrolls to the grown
					// content). The deltas below are client-space diffs, so a constant scroll cancels out.
					#[cfg(target_arch = "wasm32")]
					{
						use dioxus::web::WebEventExt;
						use wasm_bindgen::JsCast;
						let ev = e.data().as_web_event();
						ev.target()
							.and_then(|t| t.dyn_into::<web_sys::Element>().ok())
							.expect("pointerdown target is the handle element")
							.set_pointer_capture(ev.pointer_id())
							.expect("capture on a live element with an active pointer");
					}
					let c = e.client_coordinates();
					resize.set(Some(ResizeStart { px: c.x, py: c.y, w, h }));
					api.grid.write().state = GridState::Resizing;
				},
				onpointermove: move |e: PointerEvent| {
					let Some(s) = resize() else { return };
					let c = e.client_coordinates();
					let (sw, sh) = step();
					let dw = ((c.x - s.px) / sw).round() as i64;
					let dh = ((c.y - s.py) / sh).round() as i64;
					let nw = (s.w as i64 + dw).max(1) as u32;
					let nh = (s.h as i64 + dh).max(1) as u32;
					api.resize(idx, nw, nh);
				},
				onpointerup: move |_| {
					if resize().is_none() {
						return;
					}
					resize.set(None);
					api.grid.write().state = GridState::Settled;
				},
				onpointercancel: move |_| {
					if resize().is_none() {
						return;
					}
					resize.set(None);
					api.grid.write().state = GridState::Settled;
				},
			}
			// Cursor-only scrim while resizing: captured pointer events all go to the handle (this
			// receives none), it just keeps the resize cursor steady over whatever the pointer crosses.
			if resize().is_some() {
				div { style: "position:fixed; inset:0; z-index:1000; cursor:nwse-resize;" }
			}
		}
	}
}

/// The flat, id-keyed content overlay. Each panel renders once in a stable list (so Dioxus
/// keeps its component instance — and any inner JS state — alive across restructuring) and is
/// positioned over its host tile's body straight from the (preview) model rect: tile rect minus
/// the fixed chrome, in the packed root's coordinate space. Hidden unless it is its group's
/// active tab.
#[component]
fn PackedContent() -> Element {
	let view = use_context::<Memo<PackedGrid>>();
	let panels = use_context::<Signal<Vec<DockPanel>>>();
	let drag = use_context::<Signal<Option<Drag>>>();
	let mut focused = use_context::<PaneView>().focused;
	let maximized = *use_context::<PaneView>().maximized.read();
	let sp = use_context::<StepPx>().0;

	// Panels carried by the floating ghost: hidden here so their live content rides the cursor's
	// ghost, not the snapped grey shadow at the landing cell.
	let hidden: HashSet<PanelId> = match drag.read().clone() {
		Some(d) if d.armed => match &d.source {
			DragSource::Tile(g) => view
				.read()
				.cells
				.iter()
				.find(|c| c.group.id == *g)
				.map(|c| c.group.tabs.iter().cloned().collect())
				.unwrap_or_default(),
			DragSource::Tab { panel, .. } => std::iter::once(panel.clone()).collect(),
		},
		_ => HashSet::new(),
	};

	let host: HashMap<PanelId, Slot> = {
		let g = view.read();
		let mut map = HashMap::new();
		for cell in &g.cells {
			let active = cell.group.active_panel();
			for id in &cell.group.tabs {
				map.insert(
					id.clone(),
					Slot {
						group: cell.group.id,
						x: cell.x,
						y: cell.y,
						w: cell.w,
						h: cell.h,
						active: id == active,
					},
				);
			}
		}
		map
	};

	let panels = panels.read();
	rsx! {
		for panel in panels.iter() {
			div {
				key: "{panel.id.0}",
				class: "dv-render-overlay",
				style: if hidden.contains(&panel.id) { "display:none;".to_string() } else { host.get(&panel.id).map(|s| s.style(maximized, sp())).unwrap_or_else(|| "display:none;".into()) },
				// Clicking a pane's body focuses it too (not just its header/tab), so pane keybinds act
				// on whichever pane you last interacted with.
				onpointerdown: {
					let group = host.get(&panel.id).map(|s| s.group);
					move |_| {
						if let Some(g) = group {
							focused.set(Some(g));
						}
					}
				},
				{panel.content.clone()}
			}
		}
	}
}

/// Where a panel's content sits: its host group, its host tile's grid rect, and whether it's the
/// active tab.
struct Slot {
	group: GroupId,
	x: u32,
	y: u32,
	w: u32,
	h: u32,
	active: bool,
}

impl Slot {
	/// Inline style placing the content over the tile's body, from the grid rect — the same
	/// math the skeleton uses, so the two cannot drift apart. Inactive tabs stay mounted but
	/// `display:none`. When a group is maximized, its active panel fills the container below the
	/// chrome (matching the skeleton's maximized tile) and every other panel is hidden.
	fn style(&self, maximized: Option<GroupId>, (step_w, step_h): (f64, f64)) -> String {
		if let Some(mg) = maximized {
			if self.group != mg || !self.active {
				return "display:none;".into();
			}
			return format!("display:block; left:0; top:{CHROME_H}px; width:100%; height:calc(100% - {CHROME_H}px);");
		}
		if !self.active {
			return "display:none;".into();
		}
		// Tiles scale with the step px; the chrome band stays a fixed px (the header CSS pins it), so
		// the content's top/height bridge the two with `calc`.
		let (left, top) = (self.x as f64 * step_w, self.y as f64 * step_h);
		let (width, height) = (self.w as f64 * step_w, self.h as f64 * step_h);
		format!("display:block; left:{left}px; top:calc({top}px + {CHROME_H}px); width:{width}px; height:calc({height}px - {CHROME_H}px);")
	}
}
