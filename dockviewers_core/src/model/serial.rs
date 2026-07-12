//! JSON (de)serialization of the packed layout. [`PackedGrid`] already derives serde, so a
//! layout round-trips to JSON directly — `PackedGrid` *is* the serialized value. This module
//! is the seam for keeping that JSON **stable across versions** (schema tag + migrations);
//! `load` errors on a younger payload rather than silently wiping the user's workspace.

use crate::model::packed::PackedGrid;

/// Bump when the on-disk shape changes; `load` would migrate older payloads.
pub const SCHEMA_VERSION: u32 = 1;

/// Versioned wrapper actually written to storage.
#[derive(serde::Deserialize, serde::Serialize)]
pub struct Serialized {
	pub version: u32,
	pub grid: PackedGrid,
}

pub fn save(grid: &PackedGrid) -> String {
	serde_json::to_string(&Serialized {
		version: SCHEMA_VERSION,
		grid: grid.clone(),
	})
	.expect("PackedGrid is always serializable")
}

/// Parse a saved layout. Errors (not a silent default) on malformed/younger JSON so a corrupt
/// or future-version layout surfaces instead of silently resetting the workspace.
pub fn load(json: &str) -> Result<PackedGrid, LoadError> {
	let serialized: Serialized = serde_json::from_str(json).map_err(LoadError::Parse)?;
	if serialized.version > SCHEMA_VERSION {
		return Err(LoadError::FutureVersion(serialized.version));
	}
	Ok(serialized.grid)
}

#[derive(Debug)]
pub enum LoadError {
	Parse(serde_json::Error),
	/// Payload is newer than this build understands.
	FutureVersion(u32),
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::model::{Group, GroupId, PanelId};

	/// A non-trivial grid driven through the real API so the round-trip proves `next_group_id`
	/// (private, but observable via id collisions) and cell geometry survive.
	fn sample() -> PackedGrid {
		let mut g = PackedGrid::default();
		let gid = g.mint_group_id();
		g.place(Group::new(gid, PanelId("a".into())), 3, 2, (1, 1), 6);
		let gid = g.mint_group_id();
		g.place(Group::new(gid, PanelId("b".into())), 2, 4, (1, 1), 6);
		g.add_tab(GroupId(0), PanelId("c".into()));
		g
	}

	#[test]
	fn round_trips() {
		let g = sample();
		assert_eq!(load(&save(&g)).expect("loads"), g, "round-trip must preserve the grid (incl. next_group_id)");
	}

	#[test]
	fn future_version_errors_not_resets() {
		let json = save(&sample()).replace("\"version\":1", &format!("\"version\":{}", SCHEMA_VERSION + 1));
		assert!(matches!(load(&json), Err(LoadError::FutureVersion(v)) if v == SCHEMA_VERSION + 1));
	}

	#[test]
	fn garbage_errors() {
		assert!(matches!(load("not json"), Err(LoadError::Parse(_))));
	}
}
