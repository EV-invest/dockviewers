//! Minimization: `fails(seed, size)` runs a case under `catch_unwind` (so a production
//! `expect`/`unreachable` blowup counts as a failure too). Given a failing `(seed, size)`
//! we binary-search the smallest still-failing `size` (the action prefix shrinks to the
//! shortest that reproduces — the bug usually fires early and the tail is noise), then do
//! a bounded statistical shrink over random seeds at smaller sizes (matklad: smaller
//! failures are easier to stumble into).

use crate::sim;

pub fn fails(seed: u64, size: usize) -> bool {
	std::panic::catch_unwind(|| sim::run(seed, size, false)).map(|r| r.is_err()).unwrap_or(true)
}

/// Reduce a failing `(seed, size0)` to a small reproducible `(seed, size)`.
pub fn minimize(seed: u64, size0: usize) -> (u64, usize) {
	let mut best = (seed, min_size(seed, size0));

	// Bounded statistical shrink: probe deterministically-derived seeds at smaller sizes.
	const ROUNDS: usize = 256;
	let mut s = seed;
	for _ in 0..ROUNDS {
		s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
		if best.1 <= 1 {
			break;
		}
		let candidate = 1 + (s as usize % best.1); // strictly below best.1
		if fails(s, candidate) {
			let reduced = min_size(s, candidate);
			if reduced < best.1 {
				best = (s, reduced);
			}
		}
	}
	best
}
/// Smallest failing `size` for a fixed `seed`, given `size` already fails. The action
/// prefix isn't strictly monotone (more actions can reshape the bug away), so we track
/// the smallest size we actually *observed* failing rather than trusting the bisection.
fn min_size(seed: u64, size: usize) -> usize {
	let mut best = size;
	let (mut lo, mut hi) = (1, size);
	while lo < hi {
		let mid = (lo + hi) / 2;
		if fails(seed, mid) {
			best = best.min(mid);
			hi = mid;
		} else {
			lo = mid + 1;
		}
	}
	best
}
