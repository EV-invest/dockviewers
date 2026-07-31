//! The contract checked after every step, independent of the implementation. Split by gesture
//! state, because the packed model's no-overlap guarantee is only claimed while
//! [`GridState::Settled`] — that single requirement is why the gesture layer used to be unfuzzable.
//!
//! [`check_settled`] is the model contract: settled, overlap-free and gap-free (no tile floats
//! above the skyline), every cell honours its per-type minimum and stays within `cols`, every group
//! is non-empty with `active` in range, ids and panels are unique, and the layout survives a serde
//! round-trip. [`check_in_flight`] is the *render* contract, and runs in both states: it reads only
//! the view-model getters a binding renders, so it catches drift between them without needing any
//! new `pub` on [`PackedState`].
//!
//! Not checkable from out here: a dangling `focused` id has no getter. It surfaces instead as a
//! panic the moment the close chord fires (`close_active: group exists`), which `catch_unwind`
//! reports — so the fuzzer still finds it, just as a crash rather than an oracle violation.

use std::collections::{HashMap, HashSet};

use dockviewers_core::{
	PackedState,
	model::{packed::GridState, serial},
};

pub fn check_settled(state: &PackedState) -> Result<(), String> {
	let grid = state.grid();
	let cols = state.cols();
	if grid.state != GridState::Settled {
		return Err(format!("grid not settled: {:?}", grid.state));
	}
	if let Some((i, j)) = grid.overlaps() {
		return Err(format!("overlap: {:?} vs {:?}", grid.cells[i], grid.cells[j]));
	}
	if let Some(i) = grid.unsupported() {
		return Err(format!("floating tile (gap above): {:?}", grid.cells[i]));
	}

	let mut groups = HashSet::new();
	let mut panels = HashSet::new();
	for c in &grid.cells {
		// A tile must meet its min, unless the view itself is narrower than that min — then filling
		// the view (w == cols) is the best it can do, so a clamp to `cols` is allowed.
		if c.w < c.min_w.min(cols) || c.h < c.min_h {
			return Err(format!("cell below its min: {c:?} (min {}x{}, cols {cols})", c.min_w, c.min_h));
		}
		if c.x + c.w > cols {
			return Err(format!("cell spills past cols={cols}: {c:?}"));
		}
		if c.group.tabs.is_empty() {
			return Err(format!("empty group {:?}", c.group.id));
		}
		if c.group.active >= c.group.tabs.len() {
			return Err(format!("active {} out of range (len {})", c.group.active, c.group.tabs.len()));
		}
		if !groups.insert(c.group.id.0) {
			return Err(format!("duplicate group id {}", c.group.id.0));
		}
		for p in &c.group.tabs {
			if !panels.insert(p.0.clone()) {
				return Err(format!("duplicate panel id {}", p.0));
			}
		}
	}

	let back = serial::load(&serial::save(grid)).map_err(|e| format!("round-trip load failed: {e:?}"))?;
	if &back != grid {
		return Err("round-trip changed the grid".to_string());
	}
	// `maximized` is a pure view override that renders only its own tile, so naming a group the
	// layout no longer holds renders nothing at all — which is how a dangling id shows up out here.
	if !grid.cells.is_empty() && state.frames(&HashMap::new()).is_empty() {
		return Err("a non-empty grid rendered no frames (maximized names a dead group?)".to_string());
	}
	check_in_flight(state)
}

/// The render contract, checked after every pointer move as well as at rest: the rects a binding
/// would actually paint don't overlap, the skeleton and the content overlay agree about where each
/// panel goes, and the drop feedback matches whether the drag has armed.
pub fn check_in_flight(state: &PackedState) -> Result<(), String> {
	let titles = HashMap::new();
	let frames = state.frames(&titles);

	let rects: Vec<(usize, [f64; 4])> = frames.iter().enumerate().filter_map(|(i, f)| rect(&f.style).map(|r| (i, r))).collect();
	// Abutting edges are the common case and `x·step + w·step != (x+w)·step` in binary floating point,
	// so a hairline counts as touching. A real overlap is a whole step wide, orders of magnitude above.
	const HAIRLINE: f64 = 1e-6;
	for (n, &(i, a)) in rects.iter().enumerate() {
		for &(j, b) in &rects[n + 1..] {
			let over = |al: f64, aw: f64, bl: f64, bw: f64| (al + aw).min(bl + bw) - al.max(bl) > HAIRLINE;
			if over(a[0], a[2], b[0], b[2]) && over(a[1], a[3], b[1], b[3]) {
				return Err(format!("rendered frames overlap: {:?} at {a:?} vs {:?} at {b:?}", frames[i].gid, frames[j].gid));
			}
		}
	}

	// The skeleton and the content overlay position a panel from the same cell rect, the overlay then
	// insetting the chrome band in `calc` — so their grid-derived terms must be identical, and a
	// visible slot with no frame under it means the two disagree about what the layout even holds.
	for (panel, slot) in state.content_slots() {
		let Some(s) = rect(&slot.style) else { continue };
		let Some(f) = frames.iter().find(|f| f.gid == slot.group).and_then(|f| rect(&f.style)) else {
			return Err(format!("panel {} renders at {s:?} with no frame for its group {:?}", panel.0, slot.group));
		};
		if s != f {
			return Err(format!("panel {} slot {s:?} drifted from its frame {f:?}", panel.0));
		}
	}

	// The ghost exists exactly while the drag is armed, so it is the public witness of arming; an
	// armed drag paints exactly one landing hint — a grey shadow, or a header highlight for a join.
	// A maximized pane is the exception: it renders alone (`%`-sized, or not at all once the drag's
	// preview tears its last tab away), so a hint belonging to any other tile has nowhere to land and
	// only the upper bound survives.
	let maximized = frames.is_empty() || frames.iter().any(|f| f.style.contains('%'));
	let shadows = frames.iter().filter(|f| f.shadow).count();
	let tab_drops = frames.iter().filter(|f| f.tab_drop).count();
	let hints = shadows + tab_drops;
	match state.ghost(&titles) {
		Some(_) if hints > 1 || (hints != 1 && !maximized) => Err(format!("armed drag wants exactly one landing hint, got {shadows} shadow(s) and {tab_drops} tab-drop(s)")),
		None if hints != 0 => Err(format!("no ghost, yet {shadows} shadow(s) and {tab_drops} tab-drop(s) are painted")),
		_ => Ok(()),
	}
}

/// `left`/`top`/`width`/`height` in px out of an inline style, or `None` for a hidden or
/// percentage-sized (maximized) one — those carry no comparable rect. `calc(120px + 32px)` yields
/// its leading term, which is the grid-derived part both getters compute.
fn rect(style: &str) -> Option<[f64; 4]> {
	if style.contains("display:none") || style.contains('%') {
		return None;
	}
	let mut out = [0.0; 4];
	for (i, key) in ["left:", "top:", "width:", "height:"].iter().enumerate() {
		let at = style.find(key)? + key.len();
		let end = at + style[at..].find(';')?;
		let num: String = style[at..end]
			.chars()
			.skip_while(|c| !c.is_ascii_digit() && *c != '-')
			.take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
			.collect();
		out[i] = num.parse().ok()?;
	}
	Some(out)
}
