//! The run loop: build the world from `(seed, size)`, then generate → apply → assert the full
//! oracle after every step. A run is fully reproducible from its `(seed, size)`, so `minimize`
//! can re-run it freely.

use dockviewers_core::{
	GroupId, PanelId,
	model::{Group, packed::PackedGrid},
};

use crate::{actions, frng::Frng, oracle};

/// View-width bounds for the fuzzer (grid columns). The initial width and every [`Refit`] draw
/// from this range, so a run exercises shrinking onto a phone-narrow grid and growing back.
pub const MIN_COLS: u32 = 2;
pub const MAX_COLS: u32 = 16;

/// The fuzzed world: the grid under test, a monotonic panel-id source (the grid mints its own
/// group ids), the current view width, and the resize-history baseline backing the round-trip
/// invariant ([`refit`](World::refit)).
pub struct World {
	pub grid: PackedGrid,
	pub next_panel: u64,
	pub cols: u32,
	/// Grid snapshots keyed by the view width they settled at, since the last *structural* edit.
	/// A viewport resize is pure reflow, so returning to a previously-seen width must reproduce
	/// that width's exact arrangement; any structural edit (place/drop/close/tile-resize/add-tab)
	/// changes the layout for non-viewport reasons and clears this baseline.
	pub resize_log: Vec<(u32, PackedGrid)>,
}

impl World {
	pub fn mint_panel(&mut self) -> PanelId {
		let id = PanelId(format!("p{}", self.next_panel));
		self.next_panel += 1;
		id
	}

	/// A container resize: reflow into `cols`, then check/extend the round-trip baseline. Returns
	/// `Err` if a width seen since the last structural edit fails to reproduce its arrangement —
	/// i.e. resizing away and back left a lasting impact, which is invalid.
	pub fn refit(&mut self, cols: u32) -> Result<(), String> {
		self.grid.refit(cols);
		self.cols = cols;
		match self.resize_log.iter().find(|(c, _)| *c == cols) {
			Some((_, prev)) if prev != &self.grid => {
				return Err(format!("resize round-trip changed the arrangement at cols={cols}: {:?} vs {:?}", self.grid.cells, prev.cells));
			}
			Some(_) => {}
			None => self.resize_log.push((cols, self.grid.clone())),
		}
		Ok(())
	}

	/// A non-viewport mutation: invalidate the resize round-trip baseline.
	pub fn structural_edit(&mut self) {
		self.resize_log.clear();
	}
}

/// An oracle violation. The reproducing trace is the `(seed, size)` itself — `replay` re-runs
/// verbosely to print the steps — so we carry only what the report prints.
pub struct Failure {
	pub step: usize,
	pub what: String,
}

pub fn run(seed: u64, size: usize, verbose: bool) -> Result<(), Failure> {
	let mut frng = Frng::new(seed, size);
	// Swarm: per-run action weights come from the buffer head, not hand-tuned constants.
	let mut weights = [0u32; actions::N_KINDS];
	for w in &mut weights {
		*w = frng.byte() as u32 % 8;
	}
	// Seed-derived initial view width, so different runs start packed at different scales.
	let cols0 = MIN_COLS + frng.below(MAX_COLS - MIN_COLS + 1);

	let mut world = seed_world(cols0);

	let mut step = 0;
	while frng.remaining() > 0 {
		let Some(action) = actions::generate(&mut frng, &world, &weights) else { break };
		if verbose {
			eprintln!("step {step} (cols={}): {action:?}", world.cols);
		}
		if let Err(what) = actions::apply(&action, &mut world) {
			return Err(Failure { step, what });
		}
		if let Err(what) = oracle::check(&world.grid, world.cols) {
			return Err(Failure { step, what });
		}
		step += 1;
	}
	Ok(())
}

/// A small starting layout (a few packed tiles, one with a second tab), built through the real
/// `place`/`add_tab` path so the fuzzer starts from realistic non-trivial state.
fn seed_world(cols: u32) -> World {
	let mut world = World {
		grid: PackedGrid::default(),
		next_panel: 0,
		cols,
		resize_log: Vec::new(),
	};
	for (w, h) in [(3, 4), (4, 3), (3, 5), (5, 4)] {
		let gid = world.grid.mint_group_id();
		let panel = world.mint_panel();
		world.grid.place(Group::new(gid, panel), w, h, (1, 1), cols);
	}
	// give the first tile a second tab so tab-tears have something to bite on from the start.
	let extra = world.mint_panel();
	world.grid.add_tab(GroupId(0), extra);
	world
}
