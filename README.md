# dockviewers
![Minimum Supported Rust Version](https://img.shields.io/badge/nightly-1.92+-ab6000.svg)
[<img alt="crates.io" src="https://img.shields.io/crates/v/dockviewers.svg?color=fc8d62&logo=rust" height="20" style=flat-square>](https://crates.io/crates/dockviewers)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs&style=flat-square" height="20">](https://docs.rs/dockviewers)
![Lines Of Code](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/dockviewers-loc.json)
<br>
[<img alt="ci errors" src="https://img.shields.io/github/actions/workflow/status/EV-invest/dockviewers/errors.yml?branch=main&style=for-the-badge&style=flat-square&label=errors&labelColor=420d09" height="20">](https://github.com/EV-invest/dockviewers/actions?query=branch%3Amain) <!--NB: Won't find it if repo is private-->
[<img alt="ci warnings" src="https://img.shields.io/github/actions/workflow/status/EV-invest/dockviewers/warnings.yml?branch=main&style=for-the-badge&style=flat-square&label=warnings&labelColor=d16002" height="20">](https://github.com/EV-invest/dockviewers/actions?query=branch%3Amain) <!--NB: Won't find it if repo is private-->

🌐 **[Live demo](https://ev-invest.github.io/dockviewers/)** — no setup, runs in the browser.

A tiling/docking layout for [Dioxus](https://dioxuslabs.com/) — the IDE/trading-terminal kind: panes split, resize, tab together, float, and maximize, with the arrangement saved to JSON and restored on reload.

![fuzz trace, seed 172](docs/.readme_assets/fuzz.svg)

A random fuzz run (seed 172), not a demo. For the real thing, [play with it](https://ev-invest.github.io/dockviewers/).
// generate it yourself: `cargo r --example fuzz_film -p dockviewers_core -- --seed 172`.
<!-- markdownlint-disable -->
<details>
<summary>
<h2>Installation</h2>
</summary>

TODO

</details>
<!-- markdownlint-restore -->

## Usage
Hand `DockArea` a list of `DockPanel`s (id + title + the `Element` to render); the library owns layout, you own content.

```rust
use dioxus::prelude::*;
use dockviewers_dioxus::{DockArea, DockPanel, PanelId};

fn app() -> Element {
    let panels = vec![
        DockPanel { id: PanelId("chart".into()),  title: "Chart".into(),  content: rsx! { Chart {} } },
        DockPanel { id: PanelId("orders".into()), title: "Orders".into(), content: rsx! { Orders {} } },
    ];
    rsx! {
        // Needs a sized parent; height:100% collapses to 0 otherwise.
        div { style: "position:fixed; inset:0;",
            DockArea { panels, storage_key: Some("my-app-layout".to_string()), on_ready: None }
        }
    }
}
```

Runnable demo: `dx serve --example insilico --package dockviewers_dioxus --platform web` — or open the [hosted demo](https://ev-invest.github.io/dockviewers/), no local setup needed.

**Props:** `panels` (order = stable overlay render order — don't reorder it, that remounts panels), `storage_key` (`localStorage` key for autosave/restore; `None` to disable), `on_ready` (`Option<Callback<DockApi>>`, fires once only on a fresh default layout — use it to script the initial split).

**Scripting** — grab `DockApi` via `use_context::<DockApi>()` or from `on_ready`; every method mutates the model:

```rust
api.add_panel(id, title, target);  // Option<(Location, Position)>
api.move_panel(id, location, pos); // Location = path of child indices; vec![] is root
api.remove_panel(id);
api.maximize(gid); api.exit_maximized();
api.float(gid, rect);
let json = api.save(); api.load(&json); // load panics on corrupt JSON
```

`Position`: `Top/Bottom/Left/Right` split into a new branch, `Center` docks as a tab.

**Persistence** is wasm-only (no-op natively). With `storage_key` set: no data → default layout + `on_ready`; valid → restored, `on_ready` skipped; corrupt → error watermark, never a silent reset.

**Theming** — only structural CSS ships; set `--dv-*` custom properties on any ancestor for colors/sizes (e.g. `--dv-group-bg`, `--dv-tab-active-bg`, `--dv-splitter-size`, `--dv-drop-bg`).



<br>

<sup>
	This repository follows <a href="https://github.com/valeratrades/.github/tree/master/best_practices">my best practices</a> and <a href="https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md">Tiger Style</a> (except "proper capitalization for acronyms": (VsrState, not VSRState) and formatting). For project's architecture, see <a href="./docs/ARCHITECTURE.md">ARCHITECTURE.md</a>.
</sup>

#### License

<sup>
	Licensed under <a href="LICENSE">Blue Oak 1.0.0</a>
</sup>

<br>

<sub>
	Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be licensed as above, without any additional terms or conditions.
</sub>

