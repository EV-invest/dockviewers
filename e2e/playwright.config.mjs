import { defineConfig, devices } from '@playwright/test'

// On NixOS the browser binaries are nix-patched, so Playwright's glibc/ldd preflight would
// wrongly fail; the nix store path is already correct. Harmless elsewhere.
process.env.PLAYWRIGHT_SKIP_VALIDATE_HOST_REQUIREMENTS ??= '1'

// Two browsers, one spec: the keybind listener regression must hold in both engines (it was the
// Firefox-only break that motivated this test). Point at a running insilico dev server:
//   dx serve --example insilico --package dockviewers_dioxus --platform web --port 8111
// Override with DOCKVIEW_URL to run against any host that mounts PackedArea (e.g. the REA MFE).
export default defineConfig({
	testDir: '.',
	use: { baseURL: process.env.DOCKVIEW_URL ?? 'http://127.0.0.1:8111/' },
	projects: [
		{ name: 'chromium', use: { ...devices['Desktop Chrome'] } },
		{ name: 'firefox', use: { ...devices['Desktop Firefox'] } },
	],
})
