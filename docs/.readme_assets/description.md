🌐 **[Live demo](https://ev-invest.github.io/dockviewers/)** — no setup, runs in the browser.

A tiling/docking layout for [Dioxus](https://dioxuslabs.com/) — the IDE/trading-terminal kind: panes split, resize, tab together, float, and maximize, with the arrangement saved to JSON and restored on reload. It's a Dioxus-idiomatic port of [`dockview-core`](https://github.com/mathuo/dockview): one pure `DockModel` in a `Signal` is the only source of truth, and the UI is declarative `rsx!` derived from it. User content lives in a stable, id-keyed overlay layer separate from the split-tree skeleton, so a panel keeps its component instance and inner state (a live chart, scroll, an unsaved textarea) while it's dragged across the grid.

![fuzz trace](docs/.readme_assets/fuzz.svg)

Not a hand-authored demo: that's an actual trace from the model fuzzer (`dockviewers_core/tests/integration/`), replayed frame-for-frame — every move you see is one the oracle then checked. Regenerate with `cargo r --example fuzz_film -p dockviewers_core`, which also prints which interactions the fuzzer is and isn't reaching.
