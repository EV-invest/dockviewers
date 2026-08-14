//! Animates a real fuzz trace into one self-contained SMIL SVG, and prints the variant-coverage
//! table the film exists to make visible. The fuzzer is pure Rust and headless, so every rendered
//! frame is already known here — no browser, no rasterizer, no ffmpeg: the browser tweens between
//! keyframes, so smooth motion costs nothing and the file embeds in markdown with a plain `![](…)`.
//!
//! `cargo r --example fuzz_film -p dockviewers_core -- --help`
//!
//! Frames come from the same `frames()`/`ghost()` view-model a binding renders, and one keyframe is
//! one *input event*, so a drag reads as a drag: the ghost tracks the pointer, the grey landing
//! shadow snaps to its cell, a join highlights the target header. The coverage tally is over *all*
//! scanned seeds, not the filmed one — tallying only the winner would hide exactly the gaps the
//! table exists to expose.

use std::{
	collections::{BTreeMap, BTreeSet, HashMap},
	fmt::Write as _,
	path::{Path, PathBuf},
};

use clap::Parser;
use dockviewers_core::model::packed::{DropTarget, MinSize};

// An example can't `use` a test binary's modules, but these are plain files and `crate::` resolves
// the same at an example root.
#[path = "../tests/integration/actions.rs"]
mod actions;
#[path = "../tests/integration/oracle.rs"]
mod oracle;
#[path = "../tests/integration/sim.rs"]
mod sim;

/// Freeze on the last frame before looping, so the end reads as an end.
const HOLD_S: f64 = 1.0;
/// Canvas in SVG user units. The container is drawn at its true px size (never magnified) and
/// centred, so a measure into a narrower band visibly shrinks the window rather than restretching it.
const W: f64 = 960.0;
const H: f64 = 540.0;
const PAD: f64 = 8.0;
const CAP_H: f64 = 24.0;
/// Tile chrome, in container px — `.dv-header`'s pinned height. The per-tile clip hides it when a
/// tile is shorter than this.
const HEADER_H: f64 = 32.0;
const PIP_W: f64 = 26.0;
const MAX_PIPS: usize = 6;
const C_ROOT: &str = "#141414";
const C_TILE: &str = "#1e1e1e";
const C_BORDER: &str = "#333";
const C_HEADER: &str = "#2d2d2d";
const C_TAB: &str = "#2d2d2d";
const C_TAB_BORDER: &str = "#1e1e1e";
const C_TAB_ACTIVE: &str = "#1e1e1e";
const C_ACCENT: &str = "#63e9cd";
const C_FG: &str = "#ddd";
/// `--dv-shadow-bg` is `rgba(160,160,160,0.18)`; flattened over the tile bg so the landing cell is
/// legible at film scale.
const C_SHADOW: &str = "#3d3d3d";
const BUCKETS: [&str; 27] = [
	"Measure",
	"Place",
	"AddTab",
	"Focus",
	"Drag",
	"Resize",
	"Key",
	"SaveLoad",
	"drag: arms",
	"drag: stays a click",
	"drag: released",
	"drag: cancelled",
	"grab Tile",
	"grab Tab",
	"resolved Tab",
	"resolved Displace",
	"resolved Pack",
	"key Close",
	"key Maximize",
	"key Undo",
	"key Redo",
	"key Help",
	"key Escape",
	"key Inspect",
	"band sm",
	"band md",
	"band xl",
];
/// Every flag also reads a `FILM_*` variable, so the film is scriptable from an env without a
/// second code path; clap's precedence puts the flag first.
#[derive(Parser)]
#[command(about = "Animate a fuzz trace into an SMIL SVG, and report which interactions it reaches")]
struct Args {
	/// Seed to film. Default: whichever scanned seed reaches the most distinct interactions.
	#[arg(long, env = "FILM_SEED")]
	seed: Option<u64>,
	/// Entropy budget per run, in bytes — the same knob as the fuzzer's `FUZZ_SIZE`. Longer traces.
	#[arg(long, env = "FILM_SIZE", default_value_t = 256)]
	size: usize,
	/// How many seeds to scan for the coverage table (and to pick the filmed one from).
	#[arg(long, env = "FILM_SEEDS", default_value_t = 256)]
	seeds: u64,
	/// Wall-clock per input event. One event is one pointer move, not one whole edit, so this is
	/// lower than it would be for edit-granular keyframes. Raise it to read the interactions.
	#[arg(long, env = "FILM_STEP_S", default_value_t = 0.1)]
	step_s: f64,
	/// Hard ceiling on the film. Events past it are simply not rendered — the actions are random, so
	/// a prefix is as representative as the whole. The oracle still runs the full trace.
	#[arg(long, env = "FILM_MAX_S", default_value_t = 20.0)]
	max_s: f64,
	/// Where to write the SVG. Default: the committed README asset.
	#[arg(long, env = "FILM_OUT")]
	out: Option<PathBuf>,
}

// The `--dv-*` custom-property defaults from `css.rs`, so the film matches the real component.

fn main() {
	let args = Args::parse();
	let (step_s, size) = (args.step_s, args.size);

	let mut counts = [0u64; BUCKETS.len()];
	let mut best = (0u64, 0u32); // (seed, distinct buckets)
	for seed in 0..args.seeds {
		let mut mask = 0u32;
		let mut observe = |action: Option<&actions::Action>, world: &sim::World| {
			let Some(a) = action else { return };
			let hits = hit(a, world);
			mask |= hits;
			for (i, c) in counts.iter_mut().enumerate() {
				*c += ((hits >> i) & 1) as u64;
			}
		};
		if let Err(f) = sim::run(seed, size, false, &mut observe) {
			eprintln!("seed {seed} failed at step {}: {} — the film assumes a green fuzzer", f.step, f.what);
		}
		if mask.count_ones() > best.1 {
			best = (seed, mask.count_ones());
		}
	}

	eprintln!("coverage over seeds 0..{} (size {size}), counted per input event:", args.seeds);
	for (label, n) in BUCKETS.iter().zip(counts) {
		eprintln!("  {label:<22} {n:>9}{}", if n == 0 { "   <-- NEVER REACHED" } else { "" });
	}

	let seed = args.seed.unwrap_or(best.0);
	let mut trace = Vec::new();
	let mut step = 0usize;
	let mut observe = |action: Option<&actions::Action>, world: &sim::World| {
		let caption = match action {
			None => format!("seed {seed} \u{b7} start"),
			Some(a) => format!("step {step} \u{b7} {} \u{b7} cols={} \u{b7} {}", world.state.band(), world.state.cols(), summary(a, world)),
		};
		if action.is_some() {
			step += 1;
		}
		trace.push(keyframe(world, caption));
	};
	sim::run(seed, size, false, &mut observe).ok();

	let full = trace.len();
	trace.truncate(full.min((args.max_s / step_s).floor() as usize).max(2));
	let dur = (trace.len() - 1) as f64 * step_s + HOLD_S;

	let out = args.out.unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/.readme_assets/fuzz.svg"));
	std::fs::write(&out, render(&trace, step_s, dur)).expect("write the SVG");
	eprintln!(
		"\nfilmed seed {seed} ({} of {} buckets): {full} events, {} rendered, {dur:.1}s -> {}",
		best.1,
		BUCKETS.len(),
		trace.len(),
		out.display()
	);
}

/// The bucket mask one applied input event lands in. Drop targets are the ones `resolve_target`
/// actually returned during the gesture, not the ones the generator aimed for.
fn hit(action: &actions::Action, world: &sim::World) -> u32 {
	use actions::{Action as A, Grab, KeyKind as K};
	let mut m = 1
		<< match action {
			A::Measure { .. } => 0,
			A::Place { .. } => 1,
			A::AddTab { .. } => 2,
			A::Focus { .. } => 3,
			A::Drag { .. } => 4,
			A::Resize { .. } => 5,
			A::Key(_) => 6,
			A::SaveLoad => 7,
		};
	if let A::Drag { grab, commit, .. } = action {
		m |= 1 << if world.resolved.is_empty() { 9 } else { 8 };
		m |= 1 << if *commit { 10 } else { 11 };
		m |= 1
			<< match grab {
				Grab::Tile { .. } => 12,
				Grab::Tab { .. } => 13,
			};
		for t in &world.resolved {
			m |= 1
				<< match t {
					DropTarget::Tab { .. } => 14,
					DropTarget::Displace { .. } => 15,
					DropTarget::Pack { .. } => 16,
				};
		}
	}
	if let A::Key(k) = action {
		m |= 1
			<< match k {
				K::Close => 17,
				K::Maximize => 18,
				K::Undo => 19,
				K::Redo => 20,
				K::Help => 21,
				K::Escape => 22,
				K::Inspect => 23,
			};
	}
	m |= 1
		<< (24
			+ ["sm", "md", "xl"]
				.iter()
				.position(|b| *b == world.state.band().to_string())
				.expect("Band renders as one of its three names"));
	m
}

/// A caption-length gloss of an action — `{:?}` on a pointer path is a wall of coordinates.
fn summary(action: &actions::Action, world: &sim::World) -> String {
	use actions::{Action as A, Grab};
	match action {
		A::Measure { w, h } => format!("Measure {w:.0}x{h:.0}"),
		A::Place { w, h, min } => format!(
			"Place {w}x{h} min={}",
			match min {
				MinSize::Steps { w, h } => format!("{}x{} steps", w.0, h.0),
				MinSize::Rem { w, h } => format!("{w}x{h} rem"),
			}
		),
		A::AddTab { group } => format!("AddTab into g{}", group.0),
		A::Focus { group } => format!("Focus g{}", group.0),
		A::Drag { grab, commit, .. } => {
			let what = match grab {
				Grab::Tile { gid, .. } => format!("tile g{}", gid.0),
				Grab::Tab { panel, from, .. } => format!("tab {} out of g{}", panel.0, from.0),
			};
			let to = match world.resolved.last() {
				None => "click (never armed)".to_string(),
				Some(DropTarget::Tab { group, index }) => format!("join g{} at {index}", group.0),
				Some(DropTarget::Displace { x, y }) => format!("displace at {x},{y}"),
				Some(DropTarget::Pack { x }) => format!("pack at column {x}"),
			};
			format!("Drag {what} -> {to}{}", if *commit { "" } else { " [cancelled]" })
		}
		A::Resize { idx, commit, .. } => format!("Resize tile #{idx}{}", if *commit { "" } else { " [still held]" }),
		A::Key(k) => format!("Key {k:?}"),
		A::SaveLoad => "SaveLoad".to_string(),
	}
}

/// One rendered instant, read from the same view-model getters a binding renders.
struct Keyframe {
	container: (f64, f64),
	cells: BTreeMap<u64, Snap>,
	/// The floating drag ghost, in container space.
	ghost: Option<[f64; 4]>,
	caption: String,
}
struct Snap {
	rect: [f64; 4],
	tabs: usize,
	active: usize,
	shadow: bool,
	tab_drop: bool,
}

fn keyframe(world: &sim::World, caption: String) -> Keyframe {
	let titles = HashMap::new();
	let (ow, oh) = world.size;
	let cells = world
		.state
		.frames(&titles)
		.into_iter()
		.map(|f| {
			// A maximized tile is `%`-sized against the container, which in container px is the whole of it.
			let rect = rect(&f.style).unwrap_or([0.0, 0.0, ow, oh]);
			(
				f.gid.0,
				Snap {
					rect,
					tabs: f.tabs.len(),
					active: f.tabs.iter().position(|t| t.active).unwrap_or(0),
					shadow: f.shadow,
					tab_drop: f.tab_drop,
				},
			)
		})
		.collect();
	Keyframe {
		container: world.size,
		cells,
		// `ghost` rides the raw client pointer; the rest of the film is in container space.
		ghost: world.state.ghost(&titles).map(|g| [g.left - world.origin.0, g.top - world.origin.1, g.w, g.h]),
		caption,
	}
}

/// `left`/`top`/`width`/`height` px out of an inline style, or `None` when it is percentage-sized.
fn rect(style: &str) -> Option<[f64; 4]> {
	if style.contains('%') {
		return None;
	}
	let mut out = [0.0; 4];
	for (i, key) in ["left:", "top:", "width:", "height:"].iter().enumerate() {
		let at = style.find(key)? + key.len();
		let end = at + style[at..].find(';')?;
		let num: String = style[at..end]
			.chars()
			.skip_while(|c| !c.is_ascii_digit() && *c != '-')
			.take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
			.collect();
		out[i] = num.parse().ok()?;
	}
	Some(out)
}

fn render(trace: &[Keyframe], step_s: f64, dur: f64) -> String {
	// Container px -> canvas units at *one* scale for the whole film, set by the widest container the
	// trace ever measures. So the largest frame fills the canvas while every other one keeps its true
	// relative size: a measure into a narrower band visibly shrinks the window rather than
	// restretching it, which is the whole point of showing the breakpoint math.
	let scale = trace.iter().map(|k| (W / k.container.0).min(H / k.container.1)).fold(f64::INFINITY, f64::min);
	let place = |k: &Keyframe| (scale, PAD + (W - k.container.0 * scale) / 2.0, PAD + (H - k.container.1 * scale) / 2.0);

	// Shared time axis: one sample per keyframe plus a duplicate of the last, held out to `dur`.
	let key_times: String = (0..trace.len())
		.map(|i| format!("{:.6}", (i as f64 * step_s / dur).min(1.0)))
		.chain(std::iter::once("1".to_string()))
		.collect::<Vec<_>>()
		.join(";");

	let mut defs = String::new();
	let mut body = String::new();

	// The container outline, so a measure reads as the window itself resizing.
	let mut frame = Anim::new(&key_times, dur);
	let boxes: Vec<(f64, f64, f64, f64)> = trace
		.iter()
		.map(|k| {
			let (s, dx, dy) = place(k);
			(dx, dy, k.container.0 * s, k.container.1 * s)
		})
		.collect();
	frame.push("x", &fmt(boxes.iter().map(|b| b.0)), false);
	frame.push("y", &fmt(boxes.iter().map(|b| b.1)), false);
	frame.push("width", &fmt(boxes.iter().map(|b| b.2)), false);
	frame.push("height", &fmt(boxes.iter().map(|b| b.3)), false);
	body.push_str(&frame.rect(&format!(r#"fill="{C_ROOT}" stroke="{C_ACCENT}" stroke-opacity="0.3""#)));

	let gids: BTreeSet<u64> = trace.iter().flat_map(|k| k.cells.keys().copied()).collect();
	for gid in gids {
		// A group absent from a keyframe holds its last known rect (invisible), so a reappearance never
		// flies in from the origin.
		let mut last = [0.0; 4];
		let (mut tr, mut rw, mut rh, mut op, mut fill, mut head_op, mut head_stroke) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
		let mut pip_op: Vec<Vec<String>> = vec![Vec::new(); MAX_PIPS];
		let mut pip_fill: Vec<Vec<String>> = vec![Vec::new(); MAX_PIPS];
		for k in trace {
			let (s, dx, dy) = place(k);
			let c = k.cells.get(&gid);
			if let Some(c) = c {
				last = c.rect;
			}
			let (shadow, tab_drop, tabs, active) = c.map_or((false, false, 0, 0), |c| (c.shadow, c.tab_drop, c.tabs, c.active));
			op.push(if c.is_some() { "1" } else { "0" }.to_string());
			fill.push(if shadow { C_SHADOW } else { C_TILE }.to_string());
			head_op.push(if shadow { "0" } else { "1" }.to_string());
			head_stroke.push(if tab_drop { C_ACCENT } else { "none" }.to_string());
			for p in 0..MAX_PIPS {
				pip_op[p].push(if !shadow && p < tabs { "1" } else { "0" }.to_string());
				pip_fill[p].push(if p == active { C_TAB_ACTIVE } else { C_TAB }.to_string());
			}
			tr.push(format!("{:.2},{:.2}", dx + last[0] * s, dy + last[1] * s));
			rw.push(format!("{:.2}", last[2] * s));
			rh.push(format!("{:.2}", last[3] * s));
		}

		// The clip resolves in the group's own (post-`transform`) space, so it is local — and it rides
		// on plain `width`/`height`, the most portable thing SMIL has, keeping pips and the label from
		// bleeding onto a neighbouring tile.
		let clip = format!("dvclip{gid}");
		let mut r = Anim::new(&key_times, dur);
		r.push("width", &rw, false);
		r.push("height", &rh, false);
		let _ = write!(defs, r#"<clipPath id="{clip}">{}</clipPath>"#, r.rect(r#"x="0" y="0""#));

		let mut inner = String::new();
		let mut tile = Anim::new(&key_times, dur);
		tile.push("width", &rw, false);
		tile.push("height", &rh, false);
		tile.push("fill", &fill, true);
		inner.push_str(&tile.rect(&format!(r#"x="0" y="0" stroke="{C_BORDER}""#)));
		let mut head = Anim::new(&key_times, dur);
		head.push("width", &rw, false);
		head.push("opacity", &head_op, true);
		head.push("stroke", &head_stroke, true);
		inner.push_str(&head.rect(&format!(r#"x="0" y="0" height="{HEADER_H}" fill="{C_HEADER}" stroke-width="2""#)));
		for p in 0..MAX_PIPS {
			let mut pip = Anim::new(&key_times, dur);
			pip.push("opacity", &pip_op[p], true);
			pip.push("fill", &pip_fill[p], true);
			inner.push_str(&pip.rect(&format!(
				r#"x="{:.1}" y="3" width="{:.1}" height="{:.1}" stroke="{C_TAB_BORDER}""#,
				3.0 + p as f64 * PIP_W,
				PIP_W - 2.0,
				HEADER_H - 6.0
			)));
		}
		let mut label = Anim::new(&key_times, dur);
		label.push("opacity", &head_op, true);
		let _ = write!(
			inner,
			r#"<text x="5" y="{:.0}" fill="{C_FG}" font-family="ui-monospace, monospace" font-size="12"{}>g{gid}{}</text>"#,
			HEADER_H + 14.0,
			label.attrs,
			label.children
		);

		let mut g = Anim::new(&key_times, dur);
		g.push("transform", &tr, false);
		g.push("opacity", &op, true);
		let _ = write!(body, r#"<g clip-path="url(#{clip})"{}>{}{inner}</g>"#, g.attrs, g.children);
	}

	// The floating ghost: one box tracking the pointer, `.dv-ghost`'s accent border and 0.8 opacity.
	let mut last = [0.0; 4];
	let (mut tr, mut rw, mut rh, mut op) = (vec![], vec![], vec![], vec![]);
	for k in trace {
		let (s, dx, dy) = place(k);
		if let Some(g) = k.ghost {
			last = g;
		}
		op.push(if k.ghost.is_some() { "0.8" } else { "0" }.to_string());
		tr.push(format!("{:.2},{:.2}", dx + last[0] * s, dy + last[1] * s));
		rw.push(format!("{:.2}", last[2] * s));
		rh.push(format!("{:.2}", last[3] * s));
	}
	let mut ghost = Anim::new(&key_times, dur);
	ghost.push("width", &rw, false);
	ghost.push("height", &rh, false);
	let ghost_rect = ghost.rect(&format!(r#"x="0" y="0" fill="{C_TILE}" stroke="{C_ACCENT}""#));
	let mut gg = Anim::new(&key_times, dur);
	gg.push("transform", &tr, false);
	gg.push("opacity", &op, true);
	let _ = write!(body, r#"<g{}>{}{ghost_rect}</g>"#, gg.attrs, gg.children);

	// Caption strip: one `<text>` per keyframe, revealed for its own slot — short discrete
	// animations instead of one giant values list.
	let mut caps = String::new();
	for (i, k) in trace.iter().enumerate() {
		let (values, times) = if i == 0 {
			("1;0".to_string(), format!("0;{:.6}", (step_s / dur).min(1.0)))
		} else {
			("0;1;0".to_string(), format!("0;{:.6};{:.6}", i as f64 * step_s / dur, ((i + 1) as f64 * step_s / dur).min(1.0)))
		};
		let _ = write!(
			caps,
			r#"<text x="{PAD}" y="{:.0}" fill="{C_FG}" font-family="ui-monospace, monospace" font-size="13" opacity="0"><animate attributeName="opacity" values="{values}" keyTimes="{times}" calcMode="discrete" dur="{dur:.3}s" repeatCount="indefinite"/>{}</text>"#,
			PAD + H + CAP_H - 4.0,
			escape(&k.caption)
		);
	}

	let (cw, ch) = (W + 2.0 * PAD, H + 2.0 * PAD + CAP_H);
	format!(
		r#"<svg xmlns="http://www.w3.org/2000/svg" width="{cw:.0}" height="{ch:.0}" viewBox="0 0 {cw:.0} {ch:.0}">
<defs>{defs}</defs>
<rect width="100%" height="100%" fill="{C_ROOT}"/>
{body}
{caps}
</svg>
"#
	)
}

fn fmt(values: impl Iterator<Item = f64>) -> Vec<String> {
	values.map(|v| format!("{v:.2}")).collect()
}

/// Accumulates one element's animated attributes: each `push` writes the frame-0 value as a plain
/// attribute (so a still renderer shows a correct first frame) and, only when the value actually
/// changes over the trace, the `<animate>` child driving it.
struct Anim<'a> {
	key_times: &'a str,
	dur: f64,
	attrs: String,
	children: String,
}

impl<'a> Anim<'a> {
	fn new(key_times: &'a str, dur: f64) -> Self {
		Self {
			key_times,
			dur,
			attrs: String::new(),
			children: String::new(),
		}
	}

	fn push(&mut self, attr: &str, values: &[String], discrete: bool) {
		let first = values.first().expect("a keyframe list is never empty");
		let _ = match attr {
			"transform" => write!(self.attrs, r#" transform="translate({first})""#),
			a => write!(self.attrs, r#" {a}="{first}""#),
		};
		if values.iter().all(|v| v == first) {
			return;
		}
		// The trailing duplicate is the hold: it pins the last state out to `dur` before the loop.
		let joined = values.iter().chain(std::iter::once(values.last().expect("non-empty"))).cloned().collect::<Vec<_>>().join(";");
		let (key_times, dur, mode) = (self.key_times, self.dur, if discrete { r#" calcMode="discrete""# } else { "" });
		let _ = match attr {
			"transform" => write!(
				self.children,
				r#"<animateTransform attributeName="transform" type="translate" values="{joined}" keyTimes="{key_times}"{mode} dur="{dur:.3}s" repeatCount="indefinite"/>"#
			),
			a => write!(
				self.children,
				r#"<animate attributeName="{a}" values="{joined}" keyTimes="{key_times}"{mode} dur="{dur:.3}s" repeatCount="indefinite"/>"#
			),
		};
	}

	fn rect(&self, static_attrs: &str) -> String {
		format!(r#"<rect {static_attrs}{}>{}</rect>"#, self.attrs, self.children)
	}
}

fn escape(s: &str) -> String {
	s.replace('&', "&amp;").replace('<', "&lt;")
}
