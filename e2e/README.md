# Cross-browser keybind regression test

Guards the one thing that silently broke in Firefox: `PackedArea`'s `window` keydown listener
must *recognize* its configured binds in **both** Chromium and Firefox. It asserts recognition
(the lib calls `preventDefault()` + logs `dockview keybind:` the instant a bind matches), not the
downstream action — so it's stable regardless of layout state.

## Run

Needs a host that mounts `PackedArea`. Serve the bundled example:

```sh
dx serve --example insilico --package dockviewers_dioxus --platform web --port 8111
```

then, from this dir:

```sh
npm install            # pinned to @playwright/test 1.60.0 (matches the nix browser set)
npm test
```

Point it at any other host (e.g. the real REA dashboard) with `DOCKVIEW_URL`:

```sh
DOCKVIEW_URL=http://127.0.0.1:8122/ npm test
```

## NixOS

Playwright's own browser downloads don't run on NixOS; use the nix-provided set instead. The pin
`@playwright/test@1.60.0` matches `chromium-1223`/`firefox-1522`, which `playwright-driver.browsers`
ships. Export its store path before running:

```sh
export PLAYWRIGHT_BROWSERS_PATH=$(nix eval --raw nixpkgs#playwright-driver.browsers)/…
```

(`PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS=1` is set automatically in `playwright.config.mjs`.)
