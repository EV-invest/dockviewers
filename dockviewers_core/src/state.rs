//! The gesture reducer: every field the old Signal-driven renderer split across ~14 reactive
//! cells lives here on one [`PackedState`] as a plain field, and every event handler / effect is
//! a `&mut self` method whose inputs are already-extracted primitives (the binding does the DOM
//! read, passes plain numbers/strings in). So the whole drag/resize/undo/fit machine is ordinary
//! Rust — unit-testable without a DOM, and shared verbatim by the Dioxus and Leptos bindings.
//!
//! The imperative host API the old `PackedApi` exposed ([`place`](PackedState::place) /
//! [`save`](PackedState::save) / …) folds into methods here too; a binding wraps a mutable
//! accessor to its reactive cell to hand the host something to drive.
//!
//! View-model getters ([`frames`](PackedState::frames), [`content_slots`](PackedState::content_slots),
//! [`ghost`](PackedState::ghost), …) return plain data — ids and inline-style strings — that a
//! template renders 1:1, so all the `rsx!`/`view!`-adjacent computation lives here once.

use std::collections::{HashMap, HashSet};

use crate::{
	config::{Band, Breakpoint, Config, Saved},
	model::{
		Group, GroupId, PanelId,
		packed::{DragSource, DropTarget, GridState, MinSize, PackedGrid},
		serial,
	},
	persist,
};

/// Root font size assumed before the first measure, and on a non-DOM target (the fuzzer). Every
/// browser's default.
const REM_PX: f64 = 16.0;
/// A press promotes to a drag only past this many px, so a click still activates a tab.
const DRAG_THRESHOLD: f64 = 4.0;

/// Everything the packed layout needs to render and mutate itself, on one struct. The engine's
/// grid plus the live view geometry (`cols`/`step_px`/`breakpoint`/`root_*`, set by the measure),
/// the in-flight gestures (`drag`/`resize`), the keyboard-driven pane state
/// (`focused`/`maximized`/`help`/`popups`), the undo history, the `shown` band latch, and the host
/// `cfg`.
pub struct PackedState {
	grid: PackedGrid,
	cols: u32,
	/// Live px-per-step on each axis (`width / cols`, `height / rows`); `(0, 0)` until first measure.
	step_px: (f64, f64),
	/// The document's actual root font size, re-read at every measure. Bridges rem ⇄ px for the
	/// chrome height and a type's rem-expressed [`MinSize`], so both track browser zoom / a reader's
	/// font preference instead of assuming 16.
	rem_px: f64,
	breakpoint: Breakpoint,
	drag: Option<Drag>,
	resize: Option<ResizeStart>,
	root_origin: (f64, f64),
	root_size: (f64, f64),
	/// The pane keyboard ops act on: set when a tile's header/tab/body is pressed. `maximized` is a
	/// pure view toggle (no model mutation) — the focused tile fills the container, the rest hide.
	focused: Option<GroupId>,
	maximized: Option<GroupId>,
	help: bool,
	popups: HashSet<GroupId>,
	undo: UndoHistory,
	/// The [`Band`] whose layout is currently on screen — `None` until the first real measure, and
	/// reset to the new band by [`take_band`](PackedState::take_band) on every crossing.
	shown: Option<Band>,
	cfg: Config,
}

impl PackedState {
	pub fn new(cfg: Config) -> Self {
		let cols = cfg.steps.max(1);
		Self {
			grid: PackedGrid::default(),
			cols,
			step_px: (0.0, 0.0),
			rem_px: REM_PX,
			breakpoint: Breakpoint::default(),
			drag: None,
			resize: None,
			root_origin: (0.0, 0.0),
			root_size: (0.0, 0.0),
			focused: None,
			maximized: None,
			help: false,
			popups: HashSet::new(),
			undo: UndoHistory::default(),
			shown: None,
			cfg,
		}
	}

	// ------- imperative host API (folded from the old `PackedApi`) -------

	/// Auto-pack `group`, resolving its per-type [`MinSize`] against the live step size first.
	pub fn place(&mut self, group: Group, w: u32, h: u32, min: MinSize) {
		let (sw, sh) = self.step_px;
		assert!(sw > 0.0 && sh > 0.0, "place before the first measure: no step size yet");
		self.grid.place(group, w, h, min.resolve(sw, sh, self.rem_px), self.cols);
	}

	pub fn add_tab(&mut self, group: GroupId, panel: PanelId) {
		self.grid.add_tab(group, panel);
	}

	pub fn close_active(&mut self, group: GroupId) {
		self.grid.close_active(group);
	}

	pub fn resize(&mut self, idx: usize, new_w: u32, new_h: u32) {
		self.grid.resize(idx, new_w, new_h, self.cols);
	}

	/// Serialize the current layout (see [`serial`]).
	pub fn save(&self) -> String {
		serial::save(&self.grid)
	}

	/// Replace the layout with a previously [`save`](Self::save)d one; errors (not a silent reset)
	/// on a corrupt or future-version payload. The file may have been saved in another band, so it
	/// is settled into the live geometry.
	pub fn load(&mut self, json: &str) -> Result<(), serial::LoadError> {
		self.grid = serial::load(json)?;
		self.grid.refit(self.cols);
		Ok(())
	}

	pub fn mint_group_id(&mut self) -> GroupId {
		self.grid.mint_group_id()
	}

	pub fn cols(&self) -> u32 {
		self.cols
	}

	/// The band the layout is stored under — coarser than the [`Breakpoint`] driving `cols`.
	pub fn band(&self) -> Band {
		self.breakpoint.band()
	}

	/// Title-bar height in px — the one resolution of [`Config::title_h_rem`], used for the content
	/// overlay's chrome offset and the header's drop hit-test. The stylesheet sizes the same bar from
	/// the `--dv-title-h` the binding emits, so they agree by construction.
	pub fn title_h(&self) -> f64 {
		self.cfg.title_h_rem * self.rem_px
	}

	/// Read access to the live grid — for a host rebuilding its panel list from a restored layout.
	pub fn grid(&self) -> &PackedGrid {
		&self.grid
	}

	/// Wipe the layout back to empty (a host re-seeding a fresh band). Any gesture in flight goes
	/// with it: `drag`/`resize` hold cell *indices*, which address nothing once the cells are gone.
	pub fn reset(&mut self) {
		self.grid = PackedGrid::default();
		self.drag = None;
		self.resize = None;
	}

	// ------- effects, folded to methods -------

	/// Store the root's origin (to map pointer→grid space) and its content box, then reflow the
	/// grid to fill it: classify the width into a responsive band, stretch `cols × rows` across the
	/// container (a horizontal step is `width / cols`, cols scaled per band; a vertical one
	/// `height / rows`, rows fixed), and refit tiles into any new column count.
	pub fn set_measure(&mut self, origin: (f64, f64), size: (f64, f64)) {
		self.root_origin = origin;
		self.root_size = size;
		self.rem_px = root_font_px();
		let (width, height) = size;
		if width <= 0.0 || height <= 0.0 {
			return;
		}
		let steps = self.cfg.steps.max(1);
		let rows = self.cfg.rows.max(1);
		let bp = Breakpoint::of(width);
		let c = bp.scale_cols(steps);
		self.breakpoint = bp;
		if self.cols != c {
			self.cols = c;
			// Reflow tiles into the new band so a layout built wide never spills past a narrower view.
			self.grid.refit(c);
		}
		self.step_px = (width / c as f64, height / rows as f64);
	}

	/// Whether [`capture_undo`](Self::capture_undo) would actually push. A binding that captures via
	/// a single-signal effect reads this first (to subscribe) and only writes when it's `true`, so
	/// the resulting write doesn't re-fire the effect into a loop.
	pub fn wants_undo_snapshot(&self) -> bool {
		self.drag.is_none() && self.grid.state == GridState::Settled && !self.grid.cells.is_empty() && self.undo.states.get(self.undo.cursor) != Some(&self.grid)
	}

	/// Snapshot a *solid* layout into the undo history: only when the grid is at rest — no drag in
	/// flight, not mid-resize — and differs from the cursor's snapshot. `cells.is_empty()` skips
	/// the default grid before the host's seed lands, so undo can't walk back to a blank layout.
	pub fn capture_undo(&mut self) {
		if self.drag.is_some() {
			return;
		}
		if self.grid.state != GridState::Settled || self.grid.cells.is_empty() {
			return;
		}
		if self.undo.states.get(self.undo.cursor) == Some(&self.grid) {
			return;
		}
		self.undo.push(self.grid.clone());
	}

	/// Band-resolution latch: `Some(restored)` the first time a real step size lands, and again on
	/// every band change. `true` ⇒ this band's cached layout was restored from `localStorage`;
	/// `false` ⇒ nothing usable was cached and the grid has been reset — the host must place its
	/// tiles (or load its own default). Called by the binding after every measure; it then invokes
	/// the host's `on_band` hook.
	pub fn take_band(&mut self) -> Option<bool> {
		let band = self.band();
		if self.step_px.0 <= 0.0 || self.shown == Some(band) {
			return None;
		}
		self.shown = Some(band);
		self.reset();
		let Some(key) = self.cfg.storage_key.as_ref().map(|k| format!("{k}-{band}")) else {
			return Some(false);
		};
		let Some(json) = persist::read(&key) else { return Some(false) };
		match serial::load(&json) {
			// An empty payload parses fine and renders a blank dock, which reads as a broken app rather
			// than as a cache to fall past — so it is a miss, like a corrupt one.
			Ok(grid) if grid.cells.is_empty() => {
				report(&format!("dockview: cached layout at {key:?} is empty; re-seeding"));
				Some(false)
			}
			Ok(grid) => {
				self.grid = grid;
				self.grid.refit(self.cols);
				Some(true)
			}
			Err(e) => {
				report(&format!("dockview: cached layout at {key:?} is unusable ({e:?}); re-seeding"));
				Some(false)
			}
		}
	}

	// ------- gestures -------

	/// Pick up a whole tile by its header (empty area): arm a `Drag` and focus it. `start`/`grab`
	/// are client px + the pointer's offset within the header.
	pub fn press_tile(&mut self, gid: GroupId, start: (f64, f64), grab: (f64, f64)) {
		self.focused = Some(gid);
		let (w, h) = self.grid.cells.iter().find(|c| c.group.id == gid).map(|c| (c.w, c.h)).expect("press_tile: group exists");
		self.drag = Some(Drag {
			source: DragSource::Tile(gid),
			src_w: w,
			src_h: h,
			start,
			grab,
			cursor: start,
			armed: false,
			target: None,
		});
	}

	/// Tear a tab out of its group: arm a `Drag` and focus the origin.
	pub fn press_tab(&mut self, panel: PanelId, from: GroupId, start: (f64, f64), grab: (f64, f64)) {
		self.focused = Some(from);
		let (w, h) = self.grid.cells.iter().find(|c| c.group.id == from).map(|c| (c.w, c.h)).expect("press_tab: group exists");
		self.drag = Some(Drag {
			source: DragSource::Tab { panel, from },
			src_w: w,
			src_h: h,
			start,
			grab,
			cursor: start,
			armed: false,
			target: None,
		});
	}

	/// Advance an in-flight drag: arm past the `DRAG_THRESHOLD`, then resolve the live
	/// [`DropTarget`], refining a `Tab` slot from the preview's tab geometry (via `tab_drop_index`,
	/// a no-op without a DOM). `scroll_y` bridges client space to the root's scrolled content space.
	pub fn drag_move(&mut self, cursor: (f64, f64), scroll_y: f64) {
		let Some(mut d) = self.drag.clone() else { return };
		d.cursor = cursor;
		if !d.armed {
			let (dx, dy) = (cursor.0 - d.start.0, cursor.1 - d.start.1);
			// Below the threshold the ghost isn't shown yet, so the discarded cursor update is moot.
			if (dx * dx + dy * dy).sqrt() <= DRAG_THRESHOLD {
				return;
			}
			d.armed = true;
		}
		let (ox, oy) = self.root_origin;
		let (sw, sh) = self.step_px;
		// Reference the moving block's center (ghost top-left + half its size), not the raw pointer —
		// the cell the block visibly covers is where it lands.
		let cx = cursor.0 - d.grab.0 + d.src_w as f64 * sw / 2.0;
		let cy = cursor.1 - d.grab.1 + d.src_h as f64 * sh / 2.0;
		let mut t = self.grid.resolve_target(
			&d.source,
			cx - ox,
			cy - oy + scroll_y,
			cursor.0 - ox,
			cursor.1 - oy + scroll_y,
			sw,
			sh,
			self.title_h(),
			self.cols,
			d.src_w,
			d.src_h,
		);
		// The model can only append (it has no tab geometry); refine the slot from the live preview's
		// tab rects, skipping the ghost's own tab(s) so we read the source-free order.
		if let DropTarget::Tab { group, index } = t {
			let dragged: Vec<String> = match &d.source {
				DragSource::Tab { panel, .. } => vec![panel.0.clone()],
				DragSource::Tile(g) => self
					.grid
					.cells
					.iter()
					.find(|c| c.group.id == *g)
					.expect("drag source group exists")
					.group
					.tabs
					.iter()
					.map(|p| p.0.clone())
					.collect(),
			};
			t = DropTarget::Tab {
				group,
				index: tab_drop_index(cursor.0, group.0, &dragged).unwrap_or(index),
			};
		}
		d.target = Some(t);
		self.drag = Some(d);
	}

	/// Commit a drag: an armed release runs the same [`drop`](PackedGrid::drop) the preview used; an
	/// unarmed tab release is a plain click that just activates that tab.
	pub fn drag_release(&mut self) {
		let Some(d) = self.drag.take() else { return };
		if d.armed {
			if let Some(t) = d.target {
				self.grid.drop(d.source, t, self.cols);
			}
		} else if let DragSource::Tab { panel, from } = &d.source
			&& let Some(c) = self.grid.cells.iter_mut().find(|c| c.group.id == *from)
			&& let Some(i) = c.group.tabs.iter().position(|p| p == panel)
		{
			c.group.active = i;
		}
	}

	pub fn drag_cancel(&mut self) {
		self.drag = None;
	}

	/// Begin a corner-resize on tile `idx`: capture the pointer start + the tile's size then.
	pub fn resize_start(&mut self, idx: usize, px: f64, py: f64) {
		let (w, h) = {
			let c = &self.grid.cells[idx];
			(c.w, c.h)
		};
		self.resize = Some(ResizeStart { idx, px, py, w, h });
		self.grid.state = GridState::Resizing;
	}

	pub fn resize_move(&mut self, cursor: (f64, f64)) {
		let Some(s) = self.resize else { return };
		let (sw, sh) = self.step_px;
		let dw = ((cursor.0 - s.px) / sw).round() as i64;
		let dh = ((cursor.1 - s.py) / sh).round() as i64;
		let nw = (s.w as i64 + dw).max(1) as u32;
		let nh = (s.h as i64 + dh).max(1) as u32;
		self.grid.resize(s.idx, nw, nh, self.cols);
	}

	pub fn resize_end(&mut self) {
		if self.resize.is_none() {
			return;
		}
		self.resize = None;
		self.grid.state = GridState::Settled;
	}

	/// A body/tab press focuses the pane so keybinds act on whichever pane you last touched.
	pub fn focus(&mut self, gid: GroupId) {
		self.focused = Some(gid);
	}

	/// `focused`/`maximized` filtered to a group the layout still holds. They're set by a press and
	/// never unset by one, but a group can vanish without any event to hear it on — a drag can tear
	/// the focused pane's last tab into a fresh group, or re-home it into another. Resolving liveness
	/// at every read is why nothing downstream has to remember to clear them.
	fn live(&self, gid: Option<GroupId>) -> Option<GroupId> {
		gid.filter(|g| self.grid.cells.iter().any(|c| c.group.id == *g))
	}

	/// Walk the undo history by `dir` and settle whatever comes back into the live geometry. A
	/// snapshot may have been taken in another responsive band, so it needs the same refit a
	/// [`load`](Self::load)ed layout does — restoring one verbatim would spill it past `cols`.
	fn restore(&mut self, dir: i32) {
		if let Some(mut g) = self.undo.step(dir) {
			g.refit(self.cols);
			self.grid = g;
		}
	}

	pub fn close_help(&mut self) {
		self.help = false;
	}

	// ------- view flags -------

	pub fn dragging(&self) -> bool {
		self.drag.is_some()
	}

	pub fn resizing(&self) -> bool {
		self.resize.is_some()
	}

	pub fn help_open(&self) -> bool {
		self.help
	}

	// ------- keyboard -------

	/// Handle a keydown, already stripped to primitives by the binding (the binding also drops keys
	/// aimed at an editable field before calling). `cursor`/`scroll_y` position the `d` inspect
	/// hit-test. Returns whether the binding should `preventDefault`.
	pub fn on_key(&mut self, key: &str, alt: bool, ctrl: bool, cursor: (f64, f64), scroll_y: f64) -> KeyOutcome {
		// Logs the moment a chord is recognized (a configured bind matched), before the action runs —
		// so a console with this line but no visible effect means the action was a no-op, not that the
		// keypress went unseen (the likeliest "works in Chrome, dead in Firefox" failure). The console is
		// the only web-specific thing in here; natively (the fuzzer drives this) it's a no-op.
		let recognized = |action: &str| {
			debug_assert!(!action.is_empty(), "every recognized chord names its action");
			#[cfg(target_arch = "wasm32")]
			web_sys::console::debug_1(&format!("dockview keybind: {key:?} (alt={alt}, ctrl={ctrl}) → {action}").into());
		};
		let prevent = |b: bool| KeyOutcome { prevent_default: b };
		let kb = self.cfg.keybinds;
		// Esc always dismisses the hint and any open inspect popups, regardless of the binds.
		if key == "Escape" && (self.help || !self.popups.is_empty()) {
			recognized("Escape");
			self.help = false;
			self.popups.clear();
			return prevent(true);
		}
		if kb.undo.matches(key, alt, ctrl) {
			recognized("undo");
			self.restore(-1);
			return prevent(true);
		}
		if kb.redo.matches(key, alt, ctrl) {
			recognized("redo");
			self.restore(1);
			return prevent(true);
		}
		if kb.close.matches(key, alt, ctrl) {
			recognized("close");
			if let Some(g) = self.live(self.focused) {
				self.close_active(g);
				return prevent(true);
			}
			return prevent(false);
		}
		if kb.maximize.matches(key, alt, ctrl) {
			recognized("maximize");
			if let Some(f) = self.live(self.focused) {
				self.maximized = if self.maximized == Some(f) { None } else { Some(f) };
				return prevent(true);
			}
			return prevent(false);
		}
		if kb.help.matches(key, alt, ctrl) {
			recognized("help");
			self.help = !self.help;
			return prevent(true);
		}
		if key == "d" && !alt && !ctrl {
			recognized("inspect");
			// maximized tile fills the view; else hit-test the cursor against the grid rects (tiles live
			// in the root's scrolled content space, the cursor in client space).
			let (cx, cy) = cursor;
			let (ox, oy) = self.root_origin;
			let (sw, sh) = self.step_px;
			let hit = self.live(self.maximized).or_else(|| {
				self.grid
					.cells
					.iter()
					.find(|c| {
						let (gx, gy) = (cx - ox, cy - oy + scroll_y);
						gx >= c.x as f64 * sw && gx < (c.x + c.w) as f64 * sw && gy >= c.y as f64 * sh && gy < (c.y + c.h) as f64 * sh
					})
					.map(|c| c.group.id)
			});
			if let Some(gid) = hit {
				if !self.popups.remove(&gid) {
					self.popups.insert(gid);
				}
				return prevent(true);
			}
			return prevent(false);
		}
		// The two save chords, ahead of the host's own: a host that rebinds `s` would otherwise shadow
		// the built-in cache without any way to notice.
		if kb.save.matches(key, alt, ctrl) {
			recognized("save");
			let band = self.band();
			if let Some(k) = self.cfg.storage_key.as_ref() {
				persist::write(&format!("{k}-{band}"), &self.save());
			}
			// Cloning the `Rc` rather than taking it: the hook is `Fn(Saved)` and never touches `self`,
			// so there is nothing to hand back.
			if let Some(hook) = self.cfg.on_save.clone() {
				hook(Saved::Cached { band });
			}
			return prevent(true);
		}
		if kb.publish.matches(key, alt, ctrl)
			&& let Some(hook) = self.cfg.on_save.clone()
		{
			recognized("publish");
			hook(Saved::Published {
				band: self.band(),
				json: self.save(),
			});
			return prevent(true);
		}
		// Host-registered chords: taken out so the boxed action can borrow `self` mutably, restored after.
		let actions = std::mem::take(&mut self.cfg.actions);
		let mut prevent_default = false;
		for (bind, run) in &actions {
			if bind.matches(key, alt, ctrl) {
				recognized("custom-action");
				run(self);
				prevent_default = true;
				break;
			}
		}
		self.cfg.actions = actions;
		prevent(prevent_default)
	}

	// ------- view-model getters -------

	/// The grid the skeleton/content render: the live preview while an armed drag is in flight
	/// (other tiles settled under a cloned `drop`), else the real grid. One `drop` path.
	fn preview(&self) -> PackedGrid {
		let g = self.grid.clone();
		match &self.drag {
			Some(d) if d.armed => match &d.target {
				Some(t) => {
					let mut p = g;
					p.drop(d.source.clone(), t.clone(), self.cols);
					p
				}
				None => g,
			},
			_ => g,
		}
	}

	/// The tile skeleton: one [`Frame`] per rendered cell (a maximized pane hides the rest), in the
	/// preview's order. `titles` maps panel ids to their tab labels (owned by the binding).
	pub fn frames(&self, titles: &HashMap<PanelId, String>) -> Vec<Frame> {
		let g = self.preview();
		let (sw, sh) = self.step_px;
		let maximized = self.live(self.maximized);
		let mut out = Vec::new();
		for (idx, cell) in g.cells.iter().enumerate() {
			let gid = cell.group.id;
			// Maximize is a view-only override: the focused tile fills the container, others omitted.
			if let Some(mg) = maximized
				&& mg != gid
			{
				continue;
			}
			let style = if maximized == Some(gid) {
				"left:0; top:0; width:100%; height:100%;".to_string()
			} else {
				format!(
					"left:{}px; top:{}px; width:{}px; height:{}px;",
					cell.x as f64 * sw,
					cell.y as f64 * sh,
					cell.w as f64 * sw,
					cell.h as f64 * sh
				)
			};
			let (shadow, tab_drop) = self.drop_flags(gid, cell);
			if shadow {
				// The drag's landing cell: a detail-less grey placeholder, no chrome, no content.
				out.push(Frame {
					gid,
					idx,
					style,
					shadow: true,
					tab_drop: false,
					tabs: Vec::new(),
				});
				continue;
			}
			let tabs = cell
				.group
				.tabs
				.iter()
				.enumerate()
				.map(|(i, id)| Tab {
					id: id.clone(),
					title: titles.get(id).cloned().unwrap_or_default(),
					active: i == cell.group.active,
				})
				.collect();
			out.push(Frame {
				gid,
				idx,
				style,
				shadow: false,
				tab_drop,
				tabs,
			});
		}
		out
	}

	/// While a drag is armed, does this preview cell render as the source's grey landing shadow, and
	/// is its header the `Tab` drop site?
	fn drop_flags(&self, gid: GroupId, cell: &crate::model::packed::Cell) -> (bool, bool) {
		match &self.drag {
			Some(d) if d.armed => match &d.target {
				Some(DropTarget::Tab { group, .. }) => (false, *group == gid),
				Some(DropTarget::Displace { .. }) | Some(DropTarget::Pack { .. }) => {
					let src = match &d.source {
						DragSource::Tile(g) => *g == gid,
						DragSource::Tab { panel, .. } => cell.group.tabs.iter().any(|p| p == panel),
					};
					(src, false)
				}
				None => (false, false),
			},
			_ => (false, false),
		}
	}

	/// The flat, id-keyed content overlay: for every panel the layout hosts, its inline style
	/// (positioned over its tile's body from the same grid rect, `display:none` unless active /
	/// hidden while carried by the ghost) and its host group (a body click focuses it). Panels the
	/// binding knows but the layout doesn't simply aren't in the map — the binding hides them.
	pub fn content_slots(&self) -> HashMap<PanelId, ContentSlot> {
		let g = self.preview();
		let (sw, sh) = self.step_px;
		let hidden = self.ghost_hidden(&g);
		let maximized = self.live(self.maximized);
		let mut map = HashMap::new();
		for cell in &g.cells {
			let active = cell.group.active_panel().clone();
			for id in &cell.group.tabs {
				let style = if hidden.contains(id) {
					"display:none;".to_string()
				} else {
					slot_style(maximized, cell.group.id, cell.x, cell.y, cell.w, cell.h, id == &active, sw, sh, self.title_h())
				};
				map.insert(id.clone(), ContentSlot { style, group: cell.group.id });
			}
		}
		map
	}

	/// Panels carried by the floating ghost: hidden in the overlay so their live content rides the
	/// cursor's ghost, not the snapped grey shadow at the landing cell.
	fn ghost_hidden(&self, preview: &PackedGrid) -> HashSet<PanelId> {
		match &self.drag {
			Some(d) if d.armed => match &d.source {
				DragSource::Tile(g) => preview
					.cells
					.iter()
					.find(|c| c.group.id == *g)
					.map(|c| c.group.tabs.iter().cloned().collect())
					.unwrap_or_default(),
				DragSource::Tab { panel, .. } => std::iter::once(panel.clone()).collect(),
			},
			_ => HashSet::new(),
		}
	}

	/// The floating ghost of whatever is being dragged: it tracks the pointer 1:1 (`cursor − grab`),
	/// keeping the grabbed point under the cursor, while its shadow snaps to the landing cell.
	pub fn ghost(&self, titles: &HashMap<PanelId, String>) -> Option<Ghost> {
		let d = self.drag.as_ref().filter(|d| d.armed)?;
		let title = match &d.source {
			DragSource::Tile(g) => self
				.grid
				.cells
				.iter()
				.find(|c| c.group.id == *g)
				.map(|c| titles.get(c.group.active_panel()).cloned().unwrap_or_default())
				.unwrap_or_default(),
			DragSource::Tab { panel, .. } => titles.get(panel).cloned().unwrap_or_default(),
		};
		let (sw, sh) = self.step_px;
		Some(Ghost {
			title,
			left: d.cursor.0 - d.grab.0,
			top: d.cursor.1 - d.grab.1,
			w: d.src_w as f64 * sw,
			h: d.src_h as f64 * sh,
		})
	}

	/// One translucent `WxH` box per open inspect popup, placed from the same grid rect the tile
	/// uses. Dead/closed groups drop out of the filter (the set may keep stale ids).
	pub fn inspect(&self) -> Vec<(String, String)> {
		let g = self.preview();
		let (sw, sh) = self.step_px;
		g.cells
			.iter()
			.filter(|c| self.popups.contains(&c.group.id))
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
	}

	/// The rows of the `?` keybind hint: label + rendered chord.
	pub fn help_rows(&self) -> Vec<(&'static str, String)> {
		let kb = self.cfg.keybinds;
		[
			("Undo", kb.undo),
			("Redo", kb.redo),
			("Close pane", kb.close),
			("Maximize pane", kb.maximize),
			("Toggle this hint", kb.help),
			("Save layout", kb.save),
		]
		.into_iter()
		.chain(self.cfg.on_save.is_some().then_some(("Publish layout", kb.publish)))
		.map(|(label, b)| (label, format!("{}{}{}", if b.ctrl { "Ctrl+" } else { "" }, if b.alt { "Alt+" } else { "" }, b.key)))
		.chain(std::iter::once(("Inspect pane", "d".into())))
		.collect()
	}
}

/// One tile in the skeleton view-model: its group id, position style, the drop-feedback flags
/// (`shadow` ⇒ render the grey landing placeholder, `tab_drop` ⇒ highlight the header), and its
/// tabs (empty when `shadow`).
pub struct Frame {
	pub gid: GroupId,
	/// Index into the (preview) grid's cells — a resize gesture never overlaps a drag, so the
	/// preview equals the real grid then and this addresses the right cell for [`resize`](PackedState::resize).
	pub idx: usize,
	pub style: String,
	pub shadow: bool,
	pub tab_drop: bool,
	pub tabs: Vec<Tab>,
}
/// One tab in a [`Frame`]'s header: its panel id (render key + `data-panel`), label, and whether
/// it's the active tab.
pub struct Tab {
	pub id: PanelId,
	pub title: String,
	pub active: bool,
}
/// A panel's slot in the content overlay: its inline style and host group.
pub struct ContentSlot {
	pub style: String,
	pub group: GroupId,
}
/// The floating drag ghost: title to show, top-left in client px, and its size in px.
pub struct Ghost {
	pub title: String,
	pub left: f64,
	pub top: f64,
	pub w: f64,
	pub h: f64,
}
/// What a binding must do after a handled keydown.
pub struct KeyOutcome {
	pub prevent_default: bool,
}
/// A cached layout that couldn't be used. Loud on purpose: a seed cache that silently wipes itself
/// is indistinguishable from one that was never written.
fn report(msg: &str) {
	#[cfg(target_arch = "wasm32")]
	web_sys::console::error_1(&msg.into());
	#[cfg(not(target_arch = "wasm32"))]
	eprintln!("{msg}");
}

/// Inline style placing a panel's content over its tile's body from the grid rect — the same math
/// the skeleton uses, so the two cannot drift apart. Inactive tabs stay mounted but `display:none`;
/// a maximized group's active panel fills the container below the chrome, all others hidden.
fn slot_style(maximized: Option<GroupId>, group: GroupId, x: u32, y: u32, w: u32, h: u32, active: bool, step_w: f64, step_h: f64, chrome_h: f64) -> String {
	if let Some(mg) = maximized {
		if group != mg || !active {
			return "display:none;".into();
		}
		return format!("display:block; left:0; top:{chrome_h}px; width:100%; height:calc(100% - {chrome_h}px);");
	}
	if !active {
		return "display:none;".into();
	}
	// Tiles scale with the step px; the chrome band is a font-relative constant, so the content's
	// top/height bridge the two with `calc`.
	let (left, top) = (x as f64 * step_w, y as f64 * step_h);
	let (width, height) = (w as f64 * step_w, h as f64 * step_h);
	format!("display:block; left:{left}px; top:calc({top}px + {chrome_h}px); width:{width}px; height:calc({height}px - {chrome_h}px);")
}

/// The document's root font size in px. Only ever reached from a live measure of a mounted element,
/// so every step here is guaranteed — a failure means the DOM isn't what it claims, which would
/// silently mis-scale every tile's chrome, so it panics instead.
#[cfg(target_arch = "wasm32")]
fn root_font_px() -> f64 {
	let win = web_sys::window().expect("a browser window");
	let root = win.document().expect("a document").document_element().expect("a document element");
	let style = win
		.get_computed_style(&root)
		.expect("our own document element is never cross-origin")
		.expect("a rendered root has a style declaration");
	// A *computed* font-size is always an absolute px length, whatever the author wrote.
	style
		.get_property_value("font-size")
		.expect("property lookup never throws")
		.trim_end_matches("px")
		.parse()
		.expect("computed font-size is an absolute px length")
}
#[cfg(not(target_arch = "wasm32"))]
fn root_font_px() -> f64 {
	REM_PX
}

/// An in-flight reposition: what was picked up, where the press began (client px, to measure the
/// `DRAG_THRESHOLD`), the live pointer (to drag the ghost naturally), and — once `armed` — the
/// live [`DropTarget`]. `src_w`/`src_h` size both the ghost and a `Pack` target's column clamp;
/// `grab` is the pointer's offset within the picked-up element, so the ghost rides under the same
/// point you grabbed.
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

/// Corner-resize gesture captured at press: which tile, the pointer start, and the tile's size (in
/// steps) then.
#[derive(Clone, Copy)]
struct ResizeStart {
	idx: usize,
	px: f64,
	py: f64,
	w: u32,
	h: u32,
}

/// Linear undo history of settled layouts with a cursor into it. `push` appends a fresh edit,
/// dropping any redo branch ahead of the cursor; `step` walks the cursor by ±1 and returns the
/// snapshot to restore (or `None` at an end).
// ponytail: linear, not a branching tree — two keys (undo/redo) can only express a line. Promote to
// a real tree once there's UI to pick a branch.
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

	fn step(&mut self, dir: i32) -> Option<PackedGrid> {
		let next = self.cursor.checked_add_signed(dir as isize)?;
		let g = self.states.get(next)?.clone();
		self.cursor = next;
		Some(g)
	}
}

/// Insertion slot for cursor x `mx` among `group`'s tabs in the live preview DOM: the count of
/// *real* tabs whose horizontal center is left of `mx` (so left-half ⇒ before, right-half ⇒ after).
/// `dragged` are the panel ids the source carries — the preview already inserts them as the floating
/// ghost's tab(s), so they're skipped, leaving the source-free order this index addresses (identical
/// math whether re-homing into another group or reordering within one). The one place this layer
/// measures, because the model has no tab widths; `None` ⇒ DOM not ready (or a non-wasm target),
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
