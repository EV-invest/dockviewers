import { expect, test } from '@playwright/test'

// The framework's built-in client-side layout cache, exercised through the insilico example (which
// carries zero persistence code of its own — that is the point). `Alt+S` writes the live layout to
// `localStorage` under `<storage_key>-<band>`; a reload restores it instead of re-seeding; a
// viewport change into another band uses a different key; `Alt+Shift+S` only calls the host hook.

const KEY = 'insilico-layout'

/** Fresh profile every test: wipe the cache, then boot and wait for the dock. */
const boot = async (page) => {
	await page.addInitScript(() => localStorage.clear())
	await page.goto('/')
	await page.waitForSelector('.dv-header', { timeout: 30_000 })
}

const cache = (page, band) => page.evaluate((k) => localStorage.getItem(k), `${KEY}-${band}`)
const keys = (page) => page.evaluate(() => Object.keys(localStorage).sort())
/** Tile geometry, in the skeleton's own order — the layout's observable signature. */
const geometry = (page) => page.$$eval('.dv-tile', (els) => els.map((e) => e.getAttribute('style')))

test.beforeEach(async ({ page }) => await boot(page))

test('Alt+S caches the current band under its own key', async ({ page }) => {
	expect(await cache(page, 'xl'), 'a fresh profile starts with no cache').toBe(null)
	await page.keyboard.press('Alt+s')
	const json = await cache(page, 'xl')
	expect(json, 'Alt+S must write the xl entry').not.toBe(null)
	expect(JSON.parse(json), 'the cached payload is the versioned layout').toHaveProperty('grid')
})

test('a cached layout survives a reload', async ({ page }) => {
	// Grow the first tile so the restored layout is distinguishable from a fresh seed.
	const grip = page.locator('.dv-tile .dv-resize-handle').first()
	const box = await grip.boundingBox()
	await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
	await page.mouse.down()
	await page.mouse.move(box.x + 160, box.y + 120, { steps: 8 })
	await page.mouse.up()

	const edited = await geometry(page)
	await page.keyboard.press('Alt+s')
	await page.reload()
	await page.waitForSelector('.dv-header', { timeout: 30_000 })
	expect(await geometry(page), 'the restored layout must match what was cached').toEqual(edited)
})

test('each band caches under its own key', async ({ page }) => {
	await page.keyboard.press('Alt+s')
	const xl = await cache(page, 'xl')
	expect(xl).not.toBe(null)

	await page.setViewportSize({ width: 700, height: 800 })
	// The band change re-seeds; wait for the dock to settle into the narrow arrangement.
	await page.waitForFunction(() => document.querySelectorAll('.dv-tile').length > 0)
	await page.keyboard.press('Alt+s')

	expect(await cache(page, 'sm'), 'the narrow band writes the sm entry').not.toBe(null)
	expect(await cache(page, 'xl'), 'the xl entry must be untouched by an sm save').toBe(xl)
})

test('Alt+Shift+S calls the host hook without touching the cache', async ({ page }) => {
	const before = await keys(page)
	const dialog = new Promise((resolve) => page.once('dialog', (d) => resolve(d.message()) || d.dismiss()))
	await page.keyboard.press('Alt+Shift+S')
	expect(await dialog, "the example's publish hook alerts").toContain('publish')
	expect(await keys(page), 'publish must not write localStorage').toEqual(before)
})
