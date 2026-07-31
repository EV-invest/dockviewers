🌐 **[Live demo](https://ev-invest.github.io/dockviewers/)** — no setup, runs in the browser.

A tiling/docking layout for [Dioxus](https://dioxuslabs.com/) — the IDE/trading-terminal kind: panes split, resize, tab together, float, and maximize, with the arrangement saved to JSON and restored on reload. It's a Dioxus-idiomatic port of [`dockview-core`](https://github.com/mathuo/dockview): one pure `DockModel` in a `Signal` is the only source of truth, and the UI is declarative `rsx!` derived from it. User content lives in a stable, id-keyed overlay layer separate from the split-tree skeleton, so a panel keeps its component instance and inner state (a live chart, scroll, an unsaved textarea) while it's dragged across the grid.

![fuzz trace, seed 172](docs/.readme_assets/fuzz.svg)

That's one **random** run of the model fuzzer (`dockviewers_core/tests/integration/`, seed 172), replayed one input event at a time — the drag ghost, the grey landing shadow and the header highlight are the real view-model, and every frame in between passed the oracle. Nobody arranged any of it: it is the layout being *stress-tested*, not shown off. **[Open the demo](https://ev-invest.github.io/dockviewers/)** and drag the panes around yourself to see what it actually looks like in use.

Regenerate with `cargo r --example fuzz_film -p dockviewers_core -- --seed 172`. Drop `--seed` and it scans for the run that reaches the most distinct interactions, printing a table of which ones the fuzzer is and isn't reaching.
