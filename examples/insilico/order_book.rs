use dioxus::prelude::*;

use crate::market::{DOWN, Market, UP, xorshift};

// ponytail: this is a *simulated* L2 ladder derived from `px`+`tape`, not a matched book.
// Base level sizes come from a slowly-changing xorshift seed (so the shape is stable across
// ticks) and are then thinned by recent same-side tape aggression, so the side that just got
// hit visibly drains as prints land. Nothing here clears against a real order queue.

const LEVELS: usize = 12;

struct Row {
	px: f64,
	size: f64,
	total: f64,
}

#[component]
pub fn OrderBook(market: Signal<Market>) -> Element {
	let m = market.read();
	let mid = m.px;
	let step = (mid * 0.0004).max(0.01);

	// Recent aggression per side, weighted by recency (newest prints hit hardest) and qty.
	let (mut hit_ask, mut hit_bid) = (0.0f64, 0.0f64);
	for (i, tr) in m.tape.iter().take(16).enumerate() {
		let w = 1.0 - i as f64 / 18.0;
		if tr.buy {
			hit_ask += tr.qty * w;
		} else {
			hit_bid += tr.qty * w;
		}
	}

	// Seed changes slowly so the ladder shape is coherent between ticks rather than reshuffling.
	let key = mid.to_bits() ^ (m.t / 4);

	let build = |up: bool, drain: f64| -> Vec<Row> {
		let mut s = key ^ if up { 0xa5a5_a5a5 } else { 0x5a5a_5a5a };
		let mut total = 0.0;
		(1..=LEVELS)
			.map(|i| {
				let base = 4.0 + (xorshift(&mut s) % 9000) as f64 / 100.0;
				// near-touch levels thin first under aggression; deep levels barely move.
				let depth_factor = 1.0 - (i as f64 - 1.0) / LEVELS as f64;
				let size = (base - drain * depth_factor * 0.5).max(0.4);
				total += size;
				let px = if up { mid + i as f64 * step } else { mid - i as f64 * step };
				Row { px, size, total }
			})
			.collect()
	};

	let asks = build(true, hit_ask);
	let bids = build(false, hit_bid);

	let max_total = asks.last().map(|r| r.total).unwrap_or(1.0).max(bids.last().map(|r| r.total).unwrap_or(1.0)).max(1.0);
	let max_size = asks.iter().chain(bids.iter()).map(|r| r.size).fold(1.0f64, f64::max);

	let best_ask = asks.first().map(|r| r.px).unwrap_or(mid);
	let best_bid = bids.first().map(|r| r.px).unwrap_or(mid);
	let spread = (best_ask - best_bid).max(0.0);
	let spread_bps = if mid > 0.0 { spread / mid * 10_000.0 } else { 0.0 };

	let ask_vol: f64 = asks.iter().map(|r| r.size).sum();
	let bid_vol: f64 = bids.iter().map(|r| r.size).sum();
	let imb = if ask_vol + bid_vol > 0.0 { bid_vol / (ask_vol + bid_vol) } else { 0.5 };
	let bid_pct = imb * 100.0;
	let ask_pct = 100.0 - bid_pct;

	let last = m.tape.first();
	let last_up = last.map(|t| t.buy).unwrap_or(true);
	let mid_col = if last_up { UP } else { DOWN };
	let arrow = if last_up { "\u{25B2}" } else { "\u{25BC}" };

	rsx! {
		div { style: "display:flex; flex-direction:column; height:100%; box-sizing:border-box; font:11px ui-monospace, monospace; color:#c6d2cf; background:#0c1311; overflow:hidden;",
			style { ".ob-row:hover{{background:rgba(99,233,205,.07);}}" }

			div { style: "display:flex; padding:4px 8px 3px; color:#5a6b66; letter-spacing:.04em; border-bottom:1px solid #1a2522; flex:0 0 auto;",
				span { style: "flex:1;", "Price" }
				span { style: "flex:1; text-align:right;", "Size" }
				span { style: "flex:1; text-align:right;", "Total" }
			}

			// Asks: rendered far-from-mid at top, best ask at the bottom of the block (descending toward mid).
			div { style: "flex:1 1 0; display:flex; flex-direction:column; justify-content:flex-end; overflow:hidden;",
				for r in asks.iter().rev() {
					{level_row(r, true, r.px <= best_ask + 1e-9, max_total, max_size)}
				}
			}

			// Mid: best bid/ask emphasis + spread readout.
			div { style: "flex:0 0 auto; display:flex; align-items:center; justify-content:space-between; padding:4px 8px; border-top:1px solid #1a2522; border-bottom:1px solid #1a2522; background:#0f1916;",
				span { style: "color:{mid_col}; font-size:13px; font-weight:700; letter-spacing:.02em;", "{arrow} {mid:.2}" }
				div { style: "text-align:right; line-height:1.25;",
					div { style: "color:#63e9cd;", "spread {spread:.2}" }
					div { style: "color:#5a6b66; font-size:10px;", "{spread_bps:.1} bps" }
				}
			}

			// Bids: best bid at top (descending away from mid).
			div { style: "flex:1 1 0; display:flex; flex-direction:column; justify-content:flex-start; overflow:hidden;",
				for r in bids.iter() {
					{level_row(r, false, r.px >= best_bid - 1e-9, max_total, max_size)}
				}
			}

			// Imbalance bar: bid volume vs ask volume.
			div { style: "flex:0 0 auto; padding:4px 8px 5px; border-top:1px solid #1a2522;",
				div { style: "display:flex; height:5px; border-radius:2px; overflow:hidden; background:#1a2522;",
					div { style: "width:{bid_pct}%; background:{UP};" }
					div { style: "width:{ask_pct}%; background:{DOWN};" }
				}
				div { style: "display:flex; justify-content:space-between; padding-top:2px; color:#5a6b66; font-size:10px;",
					{
						let bl = format!("{:.0}% B", bid_pct);
						let al = format!("A {:.0}%", ask_pct);
						rsx! {
							span { style: "color:#5fe39a;", "{bl}" }
							span { style: "color:#ff8a96;", "{al}" }
						}
					}
				}
			}
		}
	}
}

/// One depth row. `ask` picks the side's palette and the aggression bar's edge (asks stack upward
/// from the mid, bids downward), `touch` marks the level at the touch.
fn level_row(r: &Row, ask: bool, touch: bool, max_total: f64, max_size: f64) -> Element {
	let bw = (r.total / max_total * 100.0).min(100.0);
	let wfac = (r.size / max_size * 100.0).min(100.0);
	let (rgb, text) = if ask { ("255,93,108", "#ff8a96") } else { ("61,220,132", "#5fe39a") };
	let bg = format!("rgba({rgb},{})", if touch { ".20" } else { ".14" });
	let edge = if ask { "bottom:0;" } else { "top:0;" };
	let wcol = format!("color:{text};{}", if touch { " font-weight:600;" } else { "" });
	let px = format!("{:.2}", r.px);
	let size = format!("{:.2}", r.size);
	let total = format!("{:.1}", r.total);
	rsx! {
		div { class: "ob-row", style: "position:relative; display:flex; padding:1.5px 8px; line-height:1.35;",
			div { style: "position:absolute; right:0; top:0; bottom:0; width:{bw}%; background:{bg};" }
			div { style: "position:absolute; left:0; {edge} height:2px; width:{wfac}%; background:rgba({rgb},.5);" }
			span { style: "flex:1; position:relative; {wcol}", "{px}" }
			span { style: "flex:1; position:relative; text-align:right; color:#9fb0ac;", "{size}" }
			span { style: "flex:1; position:relative; text-align:right; color:#6f827d;", "{total}" }
		}
	}
}
