# Usage

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

Runnable demo: `dx serve --example insilico --package dockviewers_dioxus --platform web`.

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
