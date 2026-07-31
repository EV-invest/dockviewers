//! The run loop: build the world from `(seed, size)`, then generate → apply → assert the oracle
//! after every step. A run is fully reproducible from its `(seed, size)`, so `minimize` can re-run
//! it freely.
//!
//! The world is a [`PackedState`], not a bare [`PackedGrid`]: a user cannot produce a `DropTarget`,
//! only a cursor position, so fuzzing `PackedGrid::drop` directly would exercise an interface
//! nobody drives. Everything the grid used to be poked with is still covered — through the gesture
//! layer that actually produces it.

use dockviewers_core::{
	Config, Group, GroupId, PackedState, PanelId, Step,
	model::packed::{DropTarget, MinSize, PackedGrid},
};

use crate::{actions, frng::Frng, oracle};

/// Container widths the fuzzer measures at: both sides of every [`Breakpoint`] boundary, so band
/// transitions are hit exactly rather than by luck, plus a uniform draw in between.
pub const EDGES: [f64; 8] = [575.0, 576.0, 767.0, 768.0, 991.0, 992.0, 1199.0, 1200.0];
pub const MIN_W: f64 = 320.0;
pub const MAX_W: f64 = 1920.0;
pub const MIN_H: f64 = 400.0;
pub const MAX_H: f64 = 1200.0;

/// The fuzzed world: the reducer under test, a monotonic panel-id source (the state mints its own
/// group ids), the live container geometry, the last pointer position (what the inspect chord
/// hit-tests against), and the two histories backing the round-trip invariants.
pub struct World {
	pub state: PackedState,
	pub cfg: Config,
	pub next_panel: u64,
	pub origin: (f64, f64),
	pub size: (f64, f64),
	pub cursor: (f64, f64),
	/// Grid snapshots keyed by the container size they settled at, since the last *structural* edit.
	/// A container resize is pure reflow, so returning to a previously-seen size must reproduce that
	/// size's exact arrangement; any structural edit changes the layout for non-viewport reasons and
	/// clears this baseline.
	pub measure_log: Vec<((f64, f64), PackedGrid)>,
	/// Every grid [`capture_undo`](PackedState::capture_undo) actually pushed. Undo/redo may only
	/// ever land on one of these — checking membership rather than mirroring the cursor keeps this
	/// from re-implementing the history it is testing.
	pub undo_log: Vec<PackedGrid>,
	/// The [`DropTarget`]s `resolve_target` returned during the most recent drag — what the coverage
	/// tally reports, so it says which interactions were *reached*, not which were aimed for.
	pub resolved: Vec<DropTarget>,
}

impl World {
	pub fn mint_panel(&mut self) -> PanelId {
		let id = PanelId(format!("p{}", self.next_panel));
		self.next_panel += 1;
		id
	}

	/// Live px per grid step, the same `(width / cols, height / rows)` [`PackedState::set_measure`]
	/// derives — the generator needs it to aim a pointer at a tile.
	pub fn step_px(&self) -> (f64, f64) {
		(self.size.0 / self.state.cols() as f64, self.size.1 / self.cfg.rows.max(1) as f64)
	}

	/// A container resize, through the real breakpoint path. Returns `Err` if a size seen since the
	/// last structural edit fails to reproduce its arrangement — i.e. resizing away and back left a
	/// lasting impact, which is invalid.
	pub fn measure(&mut self, w: f64, h: f64) -> Result<(), String> {
		self.size = (w, h);
		self.state.set_measure(self.origin, self.size);
		match self.measure_log.iter().find(|(s, _)| *s == self.size) {
			Some((_, prev)) if prev != self.state.grid() => Err(format!(
				"measure round-trip changed the arrangement at {w}x{h} (cols={}): {:?} vs {:?}",
				self.state.cols(),
				self.state.grid().cells,
				prev.cells
			)),
			Some(_) => Ok(()),
			None => {
				let settled = self.state.grid().clone();
				self.measure_log.push((self.size, settled));
				Ok(())
			}
		}
	}

	/// A non-viewport mutation: invalidate the measure round-trip baseline.
	pub fn structural_edit(&mut self) {
		self.measure_log.clear();
	}

	/// What a binding's undo effect does once the layout comes to rest, mirrored here so the history
	/// under test is fed the same way in the fuzzer as in the app.
	pub fn settle(&mut self) {
		if self.state.wants_undo_snapshot() {
			self.state.capture_undo();
			self.undo_log.push(self.state.grid().clone());
		}
	}
}

/// An oracle violation. The reproducing trace is the `(seed, size)` itself — `replay` re-runs
/// verbosely to print the steps — so we carry only what the report prints.
pub struct Failure {
	pub step: usize,
	pub what: String,
}

/// `observe` sees the seeded world (with `None`), then every world that passed the oracle paired
/// with the action that produced it — enough to replay a trace's geometry outside the test binary
/// (see `examples/fuzz_film.rs`).
pub fn run(seed: u64, size: usize, verbose: bool, observe: &mut dyn FnMut(Option<&actions::Action>, &World)) -> Result<(), Failure> {
	let mut frng = Frng::new(seed, size);
	// Swarm: per-run action weights come from the buffer head, not hand-tuned constants.
	let mut weights = [0u32; actions::N_KINDS];
	for w in &mut weights {
		*w = frng.byte() as u32 % 8;
	}

	let mut world = seed_world(&mut frng);
	if let Err(what) = oracle::check_settled(&world.state) {
		return Err(Failure { step: 0, what });
	}
	observe(None, &world);

	let mut step = 0;
	while frng.remaining() > 0 {
		let Some(action) = actions::generate(&mut frng, &world, &weights) else { break };
		if verbose {
			eprintln!("step {step} (cols={}, {}x{}): {action:?}", world.state.cols(), world.size.0, world.size.1);
		}
		if let Err(what) = actions::apply(&action, &mut world, &mut |w| observe(Some(&action), w)) {
			return Err(Failure { step, what });
		}
		// A resize left mid-gesture is a legitimate resting place (the user is still holding the grip),
		// and the packed invariant is only claimed once it settles.
		let checked = if world.state.resizing() {
			oracle::check_in_flight(&world.state)
		} else {
			world.settle();
			oracle::check_settled(&world.state)
		};
		if let Err(what) = checked {
			return Err(Failure { step, what });
		}
		observe(Some(&action), &world);
		step += 1;
	}
	Ok(())
}

/// A small starting layout (a few packed tiles, one with a second tab), built through the real
/// `set_measure`/`place`/`add_tab` path so the fuzzer starts from realistic non-trivial state.
fn seed_world(frng: &mut Frng) -> World {
	let cfg = Config::default();
	let mut world = World {
		state: PackedState::new(cfg.clone()),
		cfg,
		next_panel: 0,
		// A non-zero root origin, so a lost `- origin` in the pointer mapping can't pass unnoticed.
		origin: (17.0, 23.0),
		size: (0.0, 0.0),
		cursor: (0.0, 0.0),
		measure_log: Vec::new(),
		undo_log: Vec::new(),
		resolved: Vec::new(),
	};
	// Seed-derived initial container size, so different runs start packed in different bands.
	let (w, h) = (frng.span(MIN_W, MAX_W), frng.span(MIN_H, MAX_H));
	world.measure(w, h).expect("the first measure has no baseline to contradict");
	assert!(world.state.take_ready(), "the first real measure arms the seed latch");

	let (cols, rows) = (world.state.cols(), world.cfg.rows);
	for (fw, fh) in [(0.25, 0.3), (0.3, 0.25), (0.2, 0.4), (0.35, 0.3)] {
		let gid = world.state.mint_group_id();
		let panel = world.mint_panel();
		let (w, h) = (((cols as f64 * fw) as u32).max(1), ((rows as f64 * fh) as u32).max(1));
		world.state.place(Group::new(gid, panel), w, h, MinSize::Steps { w: Step(1), h: Step(1) });
	}
	// give the first tile a second tab so tab-tears have something to bite on from the start.
	let extra = world.mint_panel();
	world.state.add_tab(GroupId(0), extra);
	world.settle();
	world
}
