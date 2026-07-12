//! Finite RNG over a fixed, seed-derived entropy buffer (matklad's FRNG). A run is
//! `(seed, size)`: the buffer is `size` bytes deterministically streamed from `seed`
//! via SplitMix64, so a smaller `size` is a *strict prefix* of a larger one — identical
//! early draws, just fewer. That prefix property is what turns minimization into a
//! binary search on `size`. Drawing past the end returns 0, so a run stops cleanly and
//! no draw ever panics.

pub struct Frng {
	buf: Vec<u8>,
	pos: usize,
}

impl Frng {
	pub fn new(seed: u64, size: usize) -> Self {
		let mut state = seed;
		let mut buf = Vec::with_capacity(size + 8);
		while buf.len() < size {
			state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
			let mut z = state;
			z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
			z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
			z ^= z >> 31;
			buf.extend_from_slice(&z.to_le_bytes());
		}
		buf.truncate(size);
		Frng { buf, pos: 0 }
	}

	pub fn remaining(&self) -> usize {
		self.buf.len() - self.pos
	}

	/// Next byte, or 0 once the buffer is exhausted (the deterministic stop).
	pub fn byte(&mut self) -> u8 {
		let b = self.buf.get(self.pos).copied().unwrap_or(0);
		if self.pos < self.buf.len() {
			self.pos += 1;
		}
		b
	}

	/// Uniform pick in `0..n` (one byte of entropy; `n <= 1` → 0). All call sites use a
	/// tiny `n` (live target counts), so single-byte modulo bias is irrelevant.
	pub fn below(&mut self, n: u32) -> u32 {
		if n <= 1 {
			return 0;
		}
		self.byte() as u32 % n
	}

	/// Weighted index into `weights` (sum must be > 0) — the swarm-testing knob.
	pub fn weighted(&mut self, weights: &[u32]) -> usize {
		let total: u32 = weights.iter().sum();
		assert!(total > 0, "weighted: weights must have positive sum");
		let mut pick = self.below(total);
		for (i, &w) in weights.iter().enumerate() {
			if pick < w {
				return i;
			}
			pick -= w;
		}
		unreachable!("weighted: pick < total always lands in a bucket")
	}
}
