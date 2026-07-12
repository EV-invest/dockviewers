//! Minimal structural stylesheet. Layout (positioning/sizing) ships with the lib; all
//! colors/sizes read from `--dv-*` custom properties so a host can re-theme without us
//! hardcoding a palette. Both bindings inject this verbatim into a `<style>`.

pub const CSS: &str = r#"
.dv-group { display: flex; flex-direction: column; width: 100%; height: 100%;
	background: var(--dv-group-bg, #1e1e1e); }
/* One header bar holds the tabs and the actions (insilico's elevated tab strip); the active
   tab is the title, so there's no separate titlebar. Height is pinned (box-sizing: border-box)
   so the content overlay's fixed chrome offset (CHROME_H in state) matches the skeleton.
   Its empty area is the tile's move-handle; tabs/actions stop propagation for their own gestures. */
.dv-header { flex: 0 0 auto; height: 32px; box-sizing: border-box; display: flex;
	align-items: stretch; overflow: hidden; cursor: grab; background: var(--dv-tabstrip-bg, #2d2d2d); }
.dv-actions { flex: 0 0 auto; margin-left: auto; display: flex; align-items: center; gap: 2px; padding: 0 4px; }
.dv-action { cursor: pointer; border: 0; background: transparent; color: var(--dv-fg, #ddd);
	opacity: 0.55; padding: 0 5px; font: inherit; line-height: 1; }
.dv-action:hover { opacity: 1; background: var(--dv-tab-bg, #2d2d2d); }
.dv-tab { display: flex; align-items: center; padding: 0 14px; font-size: 13px;
	white-space: nowrap; cursor: pointer; background: var(--dv-tab-bg, #2d2d2d);
	border-right: 1px solid var(--dv-tab-border, #1e1e1e); }
.dv-tab.dv-active { background: var(--dv-tab-active-bg, #1e1e1e);
	color: var(--dv-tab-active-fg, #fff); }
.dv-content-slot { flex: 1 1 auto; overflow: hidden; }
.dv-overlay { position: absolute; inset: 0; pointer-events: none; }
.dv-render-overlay { position: absolute; overflow: hidden; pointer-events: auto; }
.dv-resize-handle { position: absolute; right: 0; bottom: 0; width: 14px; height: 14px;
	cursor: nwse-resize; z-index: 101; background: var(--dv-resize-bg, #555); }
.dv-resize-handle::after { content: "⌟"; position: absolute; right: 1px; bottom: -3px;
	font-size: 13px; line-height: 1; color: var(--dv-fg, #ddd); }
/* Horizontal is fully model-bounded (every tile satisfies x + w ≤ cols), so a horizontal
   scrollbar is never legitimate — clip it. Only the vertical axis (whitespace/stack below) scrolls. */
.dv-packed { position: relative; width: 100%; height: 100%; overflow-x: hidden; overflow-y: auto;
	color: var(--dv-fg, #ddd); font: 13px/1.4 system-ui, sans-serif; }
.dv-tile { position: absolute; overflow: hidden; box-sizing: border-box;
	background: var(--dv-group-bg, #1e1e1e); border: 1px solid var(--dv-tab-border, #333); }
/* Drop feedback: the landing cell drawn as a plain greyed-out area (no chrome, no content),
   the floating ghost that tracks the pointer, and a Tab target's drop site. */
.dv-shadow { background: var(--dv-shadow-bg, rgba(160, 160, 160, 0.18)); border-style: dashed; }
.dv-ghost { position: fixed; z-index: 1001; pointer-events: none; opacity: 0.8; overflow: hidden;
	background: var(--dv-group-bg, #1e1e1e); border: 1px solid var(--dv-accent, #63e9cd);
	box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45); }
.dv-tab-drop { box-shadow: inset 0 0 0 2px var(--dv-accent, #63e9cd); }
/* `?` hint: a dim scrim over the whole root with a centered card listing the active binds. */
.dv-help-scrim { position: absolute; inset: 0; z-index: 1100; display: flex; align-items: center;
	justify-content: center; background: rgba(0, 0, 0, 0.45); cursor: pointer; }
.dv-help { min-width: 240px; padding: 14px 18px; background: var(--dv-group-bg, #1e1e1e);
	border: 1px solid var(--dv-accent, #63e9cd); border-radius: 6px; cursor: default;
	box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5); }
.dv-help-title { font-weight: 600; margin-bottom: 8px; }
.dv-help-row { display: flex; justify-content: space-between; gap: 24px; padding: 3px 0; }
.dv-help-key { font-family: ui-monospace, monospace; background: var(--dv-tab-bg, #2d2d2d);
	padding: 0 6px; border-radius: 3px; color: var(--dv-tab-active-fg, #fff); }
.dv-help-foot { margin-top: 10px; opacity: 0.5; font-size: 11px; }
/* `d` inspect popup: translucent dimensions box over a container, content shows through. */
.dv-inspect { position: absolute; z-index: 1050; pointer-events: none;
	display: flex; align-items: center; justify-content: center;
	background: var(--dv-group-bg, rgba(30, 30, 30, 0.25));
	border: 1px solid var(--dv-accent, #63e9cd);
	color: var(--dv-tab-active-fg, #fff); font: 600 18px ui-monospace, monospace; }
"#;
