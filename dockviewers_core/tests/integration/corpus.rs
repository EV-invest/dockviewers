//! The growing set of previously-failing `(seed, size)` cases, kept as a plain git-tracked
//! data file (`CORPUS.txt`) rather than a source `const` so `fuzz` can append a freshly
//! minimized repro on its own: find → fix → the new line is already recorded, just commit
//! it. `regressions` replays every entry.

use std::{fs::OpenOptions, io::Write, path::PathBuf};

/// Parse the corpus: one `seed size` per line; `#` starts a comment; blank lines ignored.
pub fn load() -> Vec<(u64, usize)> {
	let text = std::fs::read_to_string(path()).unwrap_or_default();
	let mut out = Vec::new();
	for line in text.lines() {
		let data = line.split('#').next().unwrap_or("").trim();
		if data.is_empty() {
			continue;
		}
		let mut it = data.split_whitespace();
		let seed = it.next().expect("corpus line has a seed").parse().expect("corpus seed is a u64");
		let size = it.next().expect("corpus line has a size").parse().expect("corpus size is a usize");
		out.push((seed, size));
	}
	out
}
/// Append a newly-found minimal repro unless it's already recorded. `reason` is written as
/// a trailing comment so a reviewer of the diff sees what broke.
pub fn record(seed: u64, size: usize, reason: &str) {
	if load().contains(&(seed, size)) {
		return;
	}
	let line = format!("{seed} {size}  # {}\n", reason.replace('\n', " "));
	// One short append on the rare failure path; the file is only ever appended to.
	let mut f = OpenOptions::new().create(true).append(true).open(path()).expect("open CORPUS.txt for append");
	f.write_all(line.as_bytes()).expect("append to CORPUS.txt");
}
/// `CARGO_MANIFEST_DIR` is fixed at compile time to the crate dir, so the path resolves
/// regardless of the test runner's working directory.
fn path() -> PathBuf {
	PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/integration/CORPUS.txt")
}
