//! The packed-grid paradigm in action — InsilicoTerminal's tiled trading layout, the visual
//! proof of [`PackedArea`]. **Web-only**: run with
//! `dx serve --example insilico --package dockview_dioxus` (a native `cargo run` panics inside
//! the dioxus-web renderer — it needs a wasm host).
//!
//! Every pane is a different projection of one shared [`market::Market`] sim the ticker advances:
//! price walks, prints a trade tape, fills your resting limit orders, and books PnL — so a fill
//! placed in the order ticket ripples through Positions, Balances, Orders, Trades and Console at
//! once. Tiles pack left-to-right; the top-bar `+` spawns a random window, each tile's `+` opens
//! the catalog as a new tab, `x` closes the active tab, and the grip resizes in `STEP` increments.

mod balances;
mod chart;
mod chat;
mod console;
mod market;
mod news;
mod order_book;
mod orders;
mod place_order;
mod positions;
mod trades;
mod watchlist;

use std::rc::Rc;

use dioxus::prelude::*;
use dockviewers_dioxus::{Action, Breakpoint, Config, DockPanel, Group, GroupId, Keybind, MinSize, PackedApi, PackedArea, PackedState, PanelId, Step, persist};
use market::{Market, xorshift};

/// localStorage key prefix the layout round-trips through (see `dockview_dioxus::persist`/`serial`).
/// The live [`Breakpoint`] is appended so each screen band keeps its own arrangement of the view.
const STORAGE_KEY: &str = "insilico-layout";

fn key(bp: Breakpoint) -> String {
	format!("{STORAGE_KEY}-{bp}")
}

fn main() {
	// The only renderer wired in is `dioxus/web`, so a launch needs a wasm host. A native
	// `cargo run` has nothing to drive — point the user at the real entrypoint instead of
	// letting dioxus-web panic deep in `spawn_local`.
	#[cfg(target_arch = "wasm32")]
	dioxus::launch(app);
	#[cfg(not(target_arch = "wasm32"))]
	{
		std::hint::black_box(app as fn() -> Element);
		eprintln!("insilico is a web example — run it with:\n  dx serve --example insilico --package dockview_dioxus");
	}
}

/// The catalog of insilico-style windows. Each declares its title, default size, and a per-type
/// [`MinSize`] (in whichever unit reads naturally).
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
	Chart,
	OrderBook,
	Trades,
	Positions,
	Orders,
	Balances,
	Watchlist,
	PlaceOrder,
	Console,
	Chat,
	News,
}

impl Kind {
	const ALL: [Kind; 11] = [
		Kind::Chart,
		Kind::OrderBook,
		Kind::Trades,
		Kind::Positions,
		Kind::Orders,
		Kind::Balances,
		Kind::Watchlist,
		Kind::PlaceOrder,
		Kind::Console,
		Kind::Chat,
		Kind::News,
	];

	fn id(self) -> &'static str {
		match self {
			Kind::Chart => "chart",
			Kind::OrderBook => "orderbook",
			Kind::Trades => "trades",
			Kind::Positions => "positions",
			Kind::Orders => "orders",
			Kind::Balances => "balances",
			Kind::Watchlist => "watchlist",
			Kind::PlaceOrder => "place",
			Kind::Console => "console",
			Kind::Chat => "chat",
			Kind::News => "news",
		}
	}

	fn title(self) -> &'static str {
		match self {
			Kind::Chart => "Chart",
			Kind::OrderBook => "Order Book",
			Kind::Trades => "Trades",
			Kind::Positions => "Positions",
			Kind::Orders => "Orders",
			Kind::Balances => "Balances",
			Kind::Watchlist => "Watchlist",
			Kind::PlaceOrder => "Place Order",
			Kind::Console => "Console",
			Kind::Chat => "#general",
			Kind::News => "News",
		}
	}

	fn min(self) -> MinSize {
		match self {
			Kind::Chart => MinSize::Rem { w: 12.0, h: 8.0 },
			Kind::OrderBook => MinSize::Rem { w: 8.0, h: 8.0 },
			Kind::Trades => MinSize::Rem { w: 9.0, h: 5.0 },
			Kind::Positions => MinSize::Rem { w: 10.0, h: 5.0 },
			Kind::Orders => MinSize::Steps { w: Step(3), h: Step(2) },
			Kind::Balances => MinSize::Steps { w: Step(2), h: Step(2) },
			Kind::Watchlist => MinSize::Steps { w: Step(2), h: Step(2) },
			Kind::PlaceOrder => MinSize::Rem { w: 10.0, h: 9.0 },
			Kind::Console => MinSize::Rem { w: 12.0, h: 6.0 },
			Kind::Chat => MinSize::Rem { w: 9.0, h: 8.0 },
			Kind::News => MinSize::Rem { w: 11.0, h: 6.0 },
		}
	}
}

/// The catalog kind a panel id was minted from (ids are `"{kind.id}-{n}"`), used to rebuild a
/// panel's content after a layout is restored from storage.
fn kind_from_id(id: &str) -> Option<Kind> {
	let prefix = id.rsplit_once('-')?.0;
	Kind::ALL.into_iter().find(|k| k.id() == prefix)
}

/// A panel's content for `kind` under a given id.
fn panel_of(kind: Kind, id: PanelId) -> DockPanel {
	DockPanel {
		id,
		title: kind.title().into(),
		content: rsx! { KindView { kind } },
	}
}

/// A unique panel id + its content for `kind`, minted from the running counter.
fn make_panel(kind: Kind, counter: &mut Signal<u64>) -> DockPanel {
	let n = counter();
	counter.set(n + 1);
	panel_of(kind, PanelId(format!("{}-{n}", kind.id())))
}

/// Mint a window of `kind` and place it as a fresh tile.
fn spawn(kind: Kind, w: u32, h: u32, mut panels: Signal<Vec<DockPanel>>, mut counter: Signal<u64>, mut api: PackedApi) {
	let panel = make_panel(kind, &mut counter);
	let id = panel.id.clone();
	panels.write().push(panel);
	let gid = api.mint_group_id();
	// A desktop-tuned width can exceed a phone band's columns; clamp so it doesn't spill the grid.
	let w = w.min(api.cols().max(1));
	api.place(Group::new(gid, id), w, h, kind.min());
}

/// Rebuild the panel list from a freshly loaded grid, decoding each tab id back to its kind so a
/// restored layout's content comes back to life. `counter` resumes past the highest id seen.
fn rebuild_panels(a: PackedApi, mut panels: Signal<Vec<DockPanel>>, mut counter: Signal<u64>) {
	let mut rebuilt = Vec::new();
	let mut next = 0u64;
	for pid in a.tab_ids() {
		if let Some(kind) = kind_from_id(&pid.0) {
			if let Some(n) = pid.0.rsplit_once('-').and_then(|(_, s)| s.parse::<u64>().ok()) {
				next = next.max(n + 1);
			}
			rebuilt.push(panel_of(kind, pid));
		}
	}
	panels.set(rebuilt);
	counter.set(next);
}

/// The screenshot-like starting spread, used for any band with no saved layout yet.
fn seed_fresh(panels: Signal<Vec<DockPanel>>, counter: Signal<u64>, a: PackedApi) {
	spawn(Kind::Chart, 22, 16, panels, counter, a);
	spawn(Kind::OrderBook, 11, 20, panels, counter, a);
	spawn(Kind::PlaceOrder, 12, 18, panels, counter, a);
	spawn(Kind::Trades, 12, 12, panels, counter, a);
	spawn(Kind::Console, 18, 10, panels, counter, a);
	spawn(Kind::Chat, 12, 14, panels, counter, a);
}

/// Host-registered chord: `Alt+S` notifies the user then stubs the "POST layout to server" pipe
/// by reading the live layout JSON the closure's [`PackedApi`] hands it. Swap the `alert` for a
/// real `fetch` to ship it.
fn insilico_config() -> Config {
	let save: Action = Rc::new(|st: &mut PackedState| {
		let msg = format!("Alt+S: would POST {} bytes of layout", st.save().len());
		#[cfg(target_arch = "wasm32")]
		web_sys::window().expect("a browser window").alert_with_message(&msg).expect("alert");
		#[cfg(not(target_arch = "wasm32"))]
		println!("{msg}");
	});
	Config {
		actions: vec![(Keybind { key: "s", alt: true, ctrl: false }, save)],
		..Default::default()
	}
}

fn app() -> Element {
	let panels = use_signal(Vec::<DockPanel>::new);
	let counter = use_signal(|| 0u64);
	let mut market = use_context_provider(|| Signal::new(Market::new()));
	let mut api = use_signal(|| None::<PackedApi>);
	let mut pending = use_signal(|| None::<GroupId>);

	use_context_provider(|| Callback::new(move |gid: GroupId| pending.set(Some(gid))));

	use_future(move || async move {
		loop {
			gloo_timers::future::TimeoutFuture::new(600).await;
			market.write().tick();
		}
	});

	let seed = Callback::new(move |a: PackedApi| api.set(Some(a)));

	// One effect per (band, settled edit): re-arrange the view per screen band and persist it under
	// that band's own key. Staying in a band just checkpoints; crossing into a new one restores that
	// band's saved layout (or seeds a fresh spread), so the same view has a distinct signature per
	// device size. `shown` is `peek`ed, not read, so writing it here doesn't re-fire the effect.
	let mut shown = use_signal(|| None::<Breakpoint>);
	use_effect(move || {
		let Some(mut a) = api() else { return };
		let mut panels = panels;
		// `breakpoint()` reads the state cell, so this effect re-runs on every settled edit (to persist).
		let bp = a.breakpoint();
		if *shown.peek() == Some(bp) {
			persist::write(&key(bp), &a.save());
			return;
		}
		shown.set(Some(bp));
		if persist::read(&key(bp)).is_some_and(|json| a.load(&json).is_ok()) {
			rebuild_panels(a, panels, counter);
		} else {
			a.reset();
			panels.write().clear();
			seed_fresh(panels, counter, a);
		}
	});

	let add_random = move |_| {
		let Some(a) = api() else { return };
		let mut s = counter().wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(0xd1b5);
		let kind = Kind::ALL[xorshift(&mut s) as usize % Kind::ALL.len()];
		let w = 8 + (xorshift(&mut s) % 16) as u32;
		let h = 8 + (xorshift(&mut s) % 12) as u32;
		spawn(kind, w, h, panels, counter, a);
	};

	rsx! {
		div { style: "position:fixed; inset:0; display:flex; flex-direction:column; background:#0b0f0e;",
			div {
				style: "flex:0 0 auto; height:44px; display:flex; align-items:center; gap:14px;
					padding:0 14px; background:#0f1b18; border-bottom:2px solid #63e9cd;",
				span { style: "color:#63e9cd; font:700 15px system-ui;", "InSilico" }
				button {
					style: "cursor:pointer; border:1px solid #63e9cd; background:transparent; color:#63e9cd;
						width:26px; height:26px; border-radius:4px; font:600 16px system-ui; line-height:1;",
					title: "Add a random window",
					onclick: add_random,
					"+"
				}
			}
			div { style: "flex:1 1 auto; position:relative;",
				PackedArea { panels, on_ready: Some(seed), config: Some(insilico_config()) }
			}
			if let Some(gid) = pending() {
				CatalogPopup { gid, panels, counter, api, pending }
			}
		}
	}
}

/// The per-tile `+` catalog: pick a kind to open it as a new tab in `gid`'s tile.
#[component]
fn CatalogPopup(gid: GroupId, panels: Signal<Vec<DockPanel>>, counter: Signal<u64>, api: Signal<Option<PackedApi>>, pending: Signal<Option<GroupId>>) -> Element {
	let mut pending = pending;
	rsx! {
		div {
			style: "position:fixed; inset:0; z-index:2000; display:flex; align-items:center; justify-content:center;
				background:rgba(0,0,0,.45);",
			onclick: move |_| pending.set(None),
			div {
				style: "background:#0f1b18; border:1px solid #63e9cd; border-radius:6px; padding:10px; min-width:180px;",
				onclick: |e: MouseEvent| e.stop_propagation(),
				div { style: "color:#63e9cd; font:600 13px system-ui; margin-bottom:6px;", "Add window" }
				for kind in Kind::ALL {
					button {
						style: "display:block; width:100%; text-align:left; cursor:pointer; border:0; padding:6px 8px;
							background:transparent; color:#ddd; font:13px system-ui; border-radius:4px;",
						onclick: move |_| {
							let mut counter = counter;
							let mut panels = panels;
							let panel = make_panel(kind, &mut counter);
							let id = panel.id.clone();
							panels.write().push(panel);
							if let Some(mut a) = api() {
								a.add_tab(gid, id);
							}
							pending.set(None);
						},
						"{kind.title()}"
					}
				}
			}
		}
	}
}

/// Dispatch a panel to its pane component, all sharing the one [`Market`] from context.
#[component]
fn KindView(kind: Kind) -> Element {
	let market = use_context::<Signal<Market>>();
	match kind {
		Kind::Chart => rsx! { chart::Chart { market } },
		Kind::OrderBook => rsx! { order_book::OrderBook { market } },
		Kind::Trades => rsx! { trades::Trades { market } },
		Kind::Positions => rsx! { positions::Positions { market } },
		Kind::Orders => rsx! { orders::Orders { market } },
		Kind::Balances => rsx! { balances::Balances { market } },
		Kind::Watchlist => rsx! { watchlist::Watchlist { market } },
		Kind::PlaceOrder => rsx! { place_order::PlaceOrder { market } },
		Kind::Console => rsx! { console::Console { market } },
		Kind::Chat => rsx! { chat::Chat { market } },
		Kind::News => rsx! { news::News { market } },
	}
}
