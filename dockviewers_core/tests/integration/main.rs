//! Deterministic model fuzzer for the dockview DnD tree reshapers (matklad FRNG +
//! TigerBeetle VOPR). One integration binary, per matklad's "delete cargo integration
//! tests": `fuzz` drives random seeds, auto-minimizes the first failure, and records its
//! minimal `(seed, size)` to `CORPUS.txt`; `regressions` replays every recorded case.
//! Env-var replay: `FUZZ_SEED=… FUZZ_SIZE=… cargo test --test integration -- --nocapture`
//! verbose-replays one case.

mod actions;
mod corpus;
mod frng;
mod minimize;
mod oracle;
mod sim;

use std::cell::RefCell;

/// Default fuzz budget. `FUZZ_RUNS` / `FUZZ_SIZE` override; bump locally for deeper runs.
const DEFAULT_RUNS: u64 = 512;
const DEFAULT_SIZE: usize = 256;

thread_local! {
	static LAST_PANIC: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Swallow the default panic backtrace (we catch and report ourselves), but stash the
/// message so a caught production blowup is still legible.
fn install_quiet_hook() {
	std::panic::set_hook(Box::new(|info| {
		LAST_PANIC.with(|c| *c.borrow_mut() = info.to_string());
	}));
}

fn env_usize(key: &str, default: usize) -> usize {
	std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[test]
fn fuzz() {
	install_quiet_hook();
	let size = env_usize("FUZZ_SIZE", DEFAULT_SIZE);

	if let Ok(s) = std::env::var("FUZZ_SEED") {
		let seed = s.parse().expect("FUZZ_SEED must be a u64");
		replay(seed, size);
		return;
	}

	let runs: u64 = std::env::var("FUZZ_RUNS").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_RUNS);
	for seed in 0..runs {
		if minimize::fails(seed, size) {
			let (ms, msz) = minimize::minimize(seed, size);
			eprintln!("\n=== FUZZ FAILURE (seed={seed}, size={size}) ===");
			eprintln!("minimal repro: (seed={ms}, size={msz})");
			let reason = replay(ms, msz).unwrap_or_else(|| "(no failure on replay)".to_string());
			corpus::record(ms, msz, &reason);
			eprintln!("recorded to CORPUS.txt — fix the bug, then commit the new line.");
			panic!("fuzz found a failure; minimal repro (seed={ms}, size={msz}): {reason}");
		}
	}
}

#[test]
fn regressions() {
	install_quiet_hook();
	let all = corpus::load();
	let live: Vec<_> = all.iter().filter(|(.., v)| *v == corpus::VERSION).collect();
	// Loud, because a corpus that has silently emptied itself still reports green.
	eprintln!(
		"corpus: replaying {} of {} cases at generator {:08x}; the rest were recorded under an older generator and are history, not tests",
		live.len(),
		all.len(),
		corpus::VERSION
	);
	for &&(seed, size, _) in &live {
		assert!(!minimize::fails(seed, size), "recorded regression (seed={seed}, size={size}) fails again");
	}
}

/// Verbose re-run of one case: prints each step before applying, then the failure (or
/// panic). Returns the failure reason, or `None` if it didn't reproduce.
fn replay(seed: u64, size: usize) -> Option<String> {
	eprintln!("--- replay (seed={seed}, size={size}) ---");
	match std::panic::catch_unwind(|| sim::run(seed, size, true, &mut |_, _| {})) {
		Ok(Ok(())) => {
			eprintln!("(no failure on replay)");
			None
		}
		Ok(Err(f)) => {
			eprintln!("FAILURE at step {}: {}", f.step, f.what);
			Some(f.what)
		}
		Err(_) => {
			let p = LAST_PANIC.with(|c| c.borrow().clone());
			eprintln!("PANIC: {p}");
			Some(format!("PANIC: {p}"))
		}
	}
}
