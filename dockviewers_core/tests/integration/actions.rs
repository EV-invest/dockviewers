//! State-aware action enum + generator. Every action is an *input event* — a measure, a pointer
//! path, a keystroke — driven at the same entry points a binding calls, so `resolve_target`, the
//! drag threshold, the preview, the ghost, the breakpoint math and the undo history are all on the
//! fuzzed path. The generator enumerates only valid targets each step (matklad's "non-crashed
//! replicas only") so no entropy is spent on impossible picks.
//!
//! Pointer paths are *aimed*, not uniform: uniform pixels would essentially never land on a 32px
//! header, so a drag would only ever resolve to `Pack`. The generator picks a live aim point (a
//! body centre, a header, open space, off-grid) and interpolates to it, always with one
//! sub-threshold move first so the arming branch runs on every drag.

use std::collections::HashMap;

use dockviewers_core::{
	Group, GroupId, Keybind, PackedState, PanelId, Step,
	model::packed::{DragSource, DropTarget, MinSize},
};

use crate::{
	frng::Frng,
	oracle,
	sim::{EDGES, MAX_H, MAX_W, MIN_H, MIN_W, World},
};

/// Height of the tile chrome: `Config::title_h_rem`'s default resolved against the 16px root font a
/// non-DOM target assumes. Mirrored here only to aim at headers and to re-run the pure
/// `resolve_target` for the coverage tally.
const CHROME_H: f64 = 32.0;

/// Action kinds, in the order their swarm weights are drawn.
pub const N_KINDS: usize = 8;
const MEASURE: usize = 0;
const PLACE: usize = 1;
const ADD_TAB: usize = 2;
const FOCUS: usize = 3;
const DRAG: usize = 4;
const RESIZE: usize = 5;
const KEY: usize = 6;
const SAVE_LOAD: usize = 7;

/// Every chord, so the generator can't silently drop one as the set grows.
pub const KEY_KINDS: [KeyKind; 7] = [KeyKind::Close, KeyKind::Maximize, KeyKind::Undo, KeyKind::Redo, KeyKind::Help, KeyKind::Escape, KeyKind::Inspect];
#[derive(Clone, Debug)]
pub enum Action {
	/// A container resize, through `set_measure` → `Breakpoint` → `scale_cols` → `refit`.
	Measure {
		w: f64,
		h: f64,
	},
	Place {
		w: u32,
		h: u32,
		min: MinSize,
	},
	AddTab {
		group: GroupId,
	},
	Focus {
		group: GroupId,
	},
	/// press → N pointer moves → release (`commit`) or cancel.
	Drag {
		grab: Grab,
		path: Vec<(f64, f64)>,
		commit: bool,
	},
	/// Corner-grip resize; `commit: false` leaves the gesture in flight, which is a state a user can
	/// genuinely rest in.
	Resize {
		idx: usize,
		start: (f64, f64),
		path: Vec<(f64, f64)>,
		commit: bool,
	},
	Key(KeyKind),
	/// `save()` → a fresh state measured the same → `load()` → must reproduce the layout.
	SaveLoad,
}

/// What a press picks up, with the press point and the pointer's offset within the pressed element.
#[derive(Clone, Debug)]
pub enum Grab {
	Tile { gid: GroupId, start: (f64, f64), grab: (f64, f64) },
	Tab { panel: PanelId, from: GroupId, start: (f64, f64), grab: (f64, f64) },
}

#[derive(Clone, Copy, Debug)]
pub enum KeyKind {
	Close,
	Maximize,
	Undo,
	Redo,
	Help,
	Escape,
	Inspect,
}

/// Apply `action`, returning the invariant it violated. Several invariants are only expressible
/// *around* an action (a cancel is a no-op, an unarmed release only activates), so they live here
/// rather than in the oracle. `mid` fires after every individual pointer move that passed the
/// in-flight oracle, so an observer sees a gesture as the several events it is.
pub fn apply(action: &Action, world: &mut World, mid: &mut dyn FnMut(&World)) -> Result<(), String> {
	match action {
		Action::Measure { w, h } => return world.measure(*w, *h),
		Action::Place { w, h, min } => {
			world.structural_edit();
			let gid = world.state.mint_group_id();
			let panel = world.mint_panel();
			world.state.place(Group::new(gid, panel), *w, *h, *min);
		}
		Action::AddTab { group } => {
			world.structural_edit();
			let panel = world.mint_panel();
			world.state.add_tab(*group, panel);
		}
		Action::Focus { group } => world.state.focus(*group),
		Action::Drag { grab, path, commit } => return drag(grab, path, *commit, world, mid),
		Action::Resize { idx, start, path, commit } => {
			world.structural_edit();
			world.state.resize_start(*idx, start.0, start.1);
			for p in path {
				world.cursor = *p;
				world.state.resize_move(*p);
				oracle::check_in_flight(&world.state)?;
				mid(world);
			}
			if *commit {
				world.state.resize_end();
			}
		}
		Action::Key(k) => return key(*k, world),
		Action::SaveLoad => return save_load(world),
	}
	Ok(())
}

/// Pick a valid action for the current `world`, weighting kinds by the per-run `weights` (swarm).
pub fn generate(frng: &mut Frng, world: &World, weights: &[u32; N_KINDS]) -> Option<Action> {
	let cells = &world.state.grid().cells;

	let mut avail = vec![MEASURE, PLACE, KEY];
	if !cells.is_empty() {
		avail.push(ADD_TAB);
		avail.push(FOCUS);
		avail.push(DRAG);
		// `resize_start` indexes the cell list, so an empty grid has no grip to take.
		avail.push(RESIZE);
		// A mid-resize grid is deliberately unsettled and `load` refits, so the two can't agree yet.
		if !world.state.resizing() {
			avail.push(SAVE_LOAD);
		}
	}

	let ws: Vec<u32> = avail.iter().map(|&k| weights[k].max(1)).collect();
	let kind = avail[frng.weighted(&ws)];
	let (sw, sh) = world.step_px();
	let (ox, oy) = world.origin;

	Some(match kind {
		MEASURE => {
			// Half the draws land on a band boundary, half anywhere — so `Breakpoint::of`'s edges are hit
			// exactly instead of being left to a 1-in-1600 chance.
			let w = if frng.below(2) == 0 {
				EDGES[frng.below(EDGES.len() as u32) as usize]
			} else {
				frng.span(MIN_W, MAX_W)
			};
			Action::Measure { w, h: frng.span(MIN_H, MAX_H) }
		}
		PLACE => {
			// Sized as a fraction of the grid, so a tile stays a pane rather than a sliver — both counts
			// scale with the band, and a fixed 1..8 would be a sliver on the 36-row default.
			let w = 1 + frng.below(world.state.cols() / 2);
			let h = 1 + frng.below(world.cfg.rows / 2);
			Action::Place {
				w,
				h,
				// Both `MinSize` arms: `Rem` is the one that resolves against the live step size.
				min: if frng.below(2) == 0 {
					MinSize::Steps {
						w: Step(1 + frng.below(w)),
						h: Step(1 + frng.below(h)),
					}
				} else {
					MinSize::Rem {
						w: (1 + frng.below(20)) as f64,
						h: (1 + frng.below(20)) as f64,
					}
				},
			}
		}
		ADD_TAB => Action::AddTab { group: pick_group(frng, world) },
		FOCUS => Action::Focus { group: pick_group(frng, world) },
		DRAG => {
			let c = &cells[frng.below(cells.len() as u32) as usize];
			let (left, top) = (ox + c.x as f64 * sw, oy + c.y as f64 * sh);
			let width = c.w as f64 * sw;
			// Half the time tear a tab (aim at its slot in the strip), else grab the header's empty area.
			let grab = if frng.below(2) == 0 {
				let i = frng.below(c.group.tabs.len() as u32);
				let x = (i as f64 * 40.0 + 12.0).min((width - 2.0).max(0.0));
				Grab::Tab {
					panel: c.group.tabs[i as usize].clone(),
					from: c.group.id,
					start: (left + x, top + 8.0),
					grab: (x, 8.0),
				}
			} else {
				let x = width * 0.8;
				Grab::Tile {
					gid: c.group.id,
					start: (left + x, top + 8.0),
					grab: (x, 8.0),
				}
			};
			let start = match &grab {
				Grab::Tile { start, .. } | Grab::Tab { start, .. } => *start,
			};
			let to = aim(frng, world);
			// One drag in four never leaves the threshold, so the click path (activate, don't move) runs.
			let arm = frng.below(4) != 0;
			Action::Drag {
				path: path(frng, start, to, arm),
				grab,
				commit: frng.below(4) != 0,
			}
		}
		RESIZE => {
			let idx = frng.below(cells.len() as u32) as usize;
			let c = &cells[idx];
			// The grip is the tile's bottom-right corner, where `.dv-resize-handle` sits.
			let start = (ox + (c.x + c.w) as f64 * sw, oy + (c.y + c.h) as f64 * sh);
			let to = aim(frng, world);
			Action::Resize {
				idx,
				start,
				path: path(frng, start, to, true),
				// A gesture left in flight is legal; when one already is, always close it out so a run can't
				// spend its whole tail unsettled.
				commit: world.state.resizing() || frng.below(4) != 0,
			}
		}
		KEY => Action::Key(KEY_KINDS[frng.below(KEY_KINDS.len() as u32) as usize]),
		SAVE_LOAD => Action::SaveLoad,
		_ => unreachable!("kind is one of the constants pushed into `avail`"),
	})
}
/// One whole pointer gesture, plus the three invariants that only exist around it: a press never
/// touches the model, a cancel is a no-op, and an unarmed release is pure tab activation.
fn drag(grab: &Grab, path: &[(f64, f64)], commit: bool, world: &mut World, mid: &mut dyn FnMut(&World)) -> Result<(), String> {
	let titles = HashMap::new();
	let before = world.state.grid().clone();
	let source = match grab {
		Grab::Tile { gid, start, grab } => {
			world.state.press_tile(*gid, *start, *grab);
			DragSource::Tile(*gid)
		}
		Grab::Tab { panel, from, start, grab } => {
			world.state.press_tab(panel.clone(), *from, *start, *grab);
			DragSource::Tab { panel: panel.clone(), from: *from }
		}
	};
	if world.state.grid() != &before {
		return Err("a press mutated the layout".to_string());
	}

	world.resolved.clear();
	let mut armed = false;
	for p in path {
		world.cursor = *p;
		world.state.drag_move(*p, 0.0);
		oracle::check_in_flight(&world.state)?;
		// The ghost is the public witness of arming, so the fuzzer never has to know the threshold.
		if world.state.ghost(&titles).is_some() {
			armed = true;
			let t = resolve(world, &source, grab_offset(grab), *p);
			world.resolved.push(t);
		}
		mid(world);
	}

	if !commit {
		world.state.drag_cancel();
		return if world.state.grid() == &before {
			Ok(())
		} else {
			Err(format!("drag_cancel changed the layout: {:?} vs {:?}", world.state.grid().cells, before.cells))
		};
	}

	world.structural_edit();
	world.state.drag_release();
	if !armed {
		// A press that never armed is a click: at most it activates the tab it landed on.
		let mut expect = before.clone();
		for (e, a) in expect.cells.iter_mut().zip(&world.state.grid().cells) {
			e.group.active = a.group.active;
		}
		if world.state.grid() != &expect {
			return Err(format!("unarmed release changed more than the active tab: {:?} vs {:?}", world.state.grid().cells, before.cells));
		}
	}
	Ok(())
}

/// Re-run the pure `resolve_target` the way `drag_move` does, purely to tally what the pointer
/// actually reached. It mirrors the reducer's block-centre math; drift makes the *tally* wrong, not
/// the run — no invariant reads it.
fn resolve(world: &World, source: &DragSource, grab: (f64, f64), cursor: (f64, f64)) -> DropTarget {
	let grid = world.state.grid();
	let (sw, sh) = world.step_px();
	let (ox, oy) = world.origin;
	let c = grid
		.cells
		.iter()
		.find(|c| c.group.id == source_group(source))
		.expect("the drag source is live for the whole gesture");
	let (cx, cy) = (cursor.0 - grab.0 + c.w as f64 * sw / 2.0, cursor.1 - grab.1 + c.h as f64 * sh / 2.0);
	grid.resolve_target(source, cx - ox, cy - oy, cursor.0 - ox, cursor.1 - oy, sw, sh, CHROME_H, world.state.cols(), c.w, c.h)
}

fn source_group(source: &DragSource) -> GroupId {
	match source {
		DragSource::Tile(g) => *g,
		DragSource::Tab { from, .. } => *from,
	}
}

fn grab_offset(grab: &Grab) -> (f64, f64) {
	match grab {
		Grab::Tile { grab, .. } | Grab::Tab { grab, .. } => *grab,
	}
}

/// One keystroke, plus the bounds on what each chord is allowed to touch.
fn key(k: KeyKind, world: &mut World) -> Result<(), String> {
	let kb = world.cfg.keybinds;
	let bind = match k {
		KeyKind::Close => kb.close,
		KeyKind::Maximize => kb.maximize,
		KeyKind::Undo => kb.undo,
		KeyKind::Redo => kb.redo,
		KeyKind::Help => kb.help,
		KeyKind::Escape => Keybind {
			key: "Escape",
			alt: false,
			ctrl: false,
		},
		KeyKind::Inspect => Keybind { key: "d", alt: false, ctrl: false },
	};
	let before = world.state.grid().clone();
	world.state.on_key(bind.key, bind.alt, bind.ctrl, world.cursor, 0.0);
	match k {
		// Maximize, the hint and the inspect popups are pure view state: they may never touch the model.
		KeyKind::Maximize | KeyKind::Help | KeyKind::Escape | KeyKind::Inspect =>
			if world.state.grid() != &before {
				return Err(format!("{k:?} is view-only but changed the layout: {:?} vs {:?}", world.state.grid().cells, before.cells));
			},
		KeyKind::Undo | KeyKind::Redo => {
			world.structural_edit();
			// A restored snapshot is refit into the live band first, so the match is against each capture
			// as it would land here — not against the raw capture.
			let landed = world.undo_log.iter().any(|g| {
				let mut g = g.clone();
				g.refit(world.state.cols());
				&g == world.state.grid()
			});
			if world.state.grid() != &before && !landed {
				return Err(format!("{k:?} produced a layout that was never captured: {:?}", world.state.grid().cells));
			}
		}
		KeyKind::Close => world.structural_edit(),
	}
	Ok(())
}

fn save_load(world: &mut World) -> Result<(), String> {
	let json = world.state.save();
	let mut fresh = PackedState::new(world.cfg.clone());
	fresh.set_measure(world.origin, world.size);
	fresh.load(&json).map_err(|e| format!("load of our own save failed: {e:?}"))?;
	if fresh.grid() != world.state.grid() {
		return Err(format!("save/load changed the layout: {:?} vs {:?}", fresh.grid().cells, world.state.grid().cells));
	}
	Ok(())
}

/// A live client-px point worth aiming a gesture at. Uniform pixels would almost never hit a header
/// or a tile edge, so each interesting site is drawn with its own probability instead.
fn aim(frng: &mut Frng, world: &World) -> (f64, f64) {
	let cells = &world.state.grid().cells;
	let (sw, sh) = world.step_px();
	let (ox, oy) = world.origin;
	match frng.below(5) {
		// A tile's body: resolves to a displace at the row below it.
		0 if !cells.is_empty() => {
			let c = &cells[frng.below(cells.len() as u32) as usize];
			(ox + (c.x as f64 + c.w as f64 / 2.0) * sw, oy + (c.y as f64 + c.h as f64 / 2.0) * sh)
		}
		// A tile's header: resolves to a join.
		1 if !cells.is_empty() => {
			let c = &cells[frng.below(cells.len() as u32) as usize];
			(ox + (c.x as f64 + 0.5) * sw, oy + c.y as f64 * sh + CHROME_H / 2.0)
		}
		// Open space in-band: displace or pack, depending on the skyline under it.
		2 => {
			let col = frng.below(world.state.cols());
			(ox + (col as f64 + 0.5) * sw, oy + frng.below(24) as f64 * sh)
		}
		3 => (ox - (1 + frng.below(200)) as f64, oy + frng.below(400) as f64),
		_ => (ox + world.size.0 + frng.below(200) as f64, oy + world.size.1 + frng.below(200) as f64),
	}
}

/// A pointer path from `start` towards `to`. The first move is always sub-threshold, so the arming
/// branch runs on every drag; `arm: false` keeps the whole path there, so the release is a click.
fn path(frng: &mut Frng, start: (f64, f64), to: (f64, f64), arm: bool) -> Vec<(f64, f64)> {
	let mut out = vec![(start.0 + 0.5, start.1 + 0.5)];
	if !arm {
		out.push((start.0 + 1.0, start.1));
		return out;
	}
	let n = 2 + frng.below(3);
	for i in 1..=n {
		let t = i as f64 / n as f64;
		out.push((start.0 + (to.0 - start.0) * t, start.1 + (to.1 - start.1) * t));
	}
	out
}

fn pick_group(frng: &mut Frng, world: &World) -> GroupId {
	let cells = &world.state.grid().cells;
	cells[frng.below(cells.len() as u32) as usize].group.id
}
