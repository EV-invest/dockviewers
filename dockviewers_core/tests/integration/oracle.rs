//! The contract checked after every *settled* step — independent of the implementation.
//! The packed model's no-overlap guarantee holds only while [`GridState::Settled`], so the
//! oracle asserts: the grid is settled, overlap-free and gap-free (no tile floats above the
//! skyline), every cell honours its per-type
//! minimum and stays within `cols`, every group is non-empty with `active` in range, ids and
//! panels are unique, and the layout survives a serde round-trip. Returns `Err(reason)` rather
//! than panicking so the minimizer survives a violation (production `expect`/`unreachable`
//! blowups are caught separately by `catch_unwind`).

use std::collections::HashSet;

use dockviewers_core::model::{
	packed::{GridState, PackedGrid},
	serial,
};

pub fn check(grid: &PackedGrid, cols: u32) -> Result<(), String> {
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
	Ok(())
}
