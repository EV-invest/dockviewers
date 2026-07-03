import { expect, test } from '@playwright/test'

// A corner-resize must not pin at the viewport: the grip captures the pointer, so moves keep
// streaming with client y past the window bottom and the tile keeps stretching below the fold
// (insilico's behavior). The root's `overflow-y: auto` then scrolls to the grown content.

test.beforeEach(async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 720 })
	await page.goto('/')
	await page.waitForSelector('.dv-resize-handle', { timeout: 30_000 })
})

test('pulling a resize grip past the bottom edge keeps stretching the tile', async ({ page, browserName }) => {
	// Firefox itself captures fine (gotpointercapture fires, in-viewport moves resize), but
	// Playwright's juggler mangles out-of-viewport pointer coords (y=1200 arrives as a negative),
	// so the past-the-fold leg is only verifiable in Chromium.
	test.skip(browserName === 'firefox', 'juggler mangles out-of-viewport pointer coordinates')
	const root = page.locator('.dv-packed')
	const before = await root.evaluate((el) => ({ scrollHeight: el.scrollHeight, clientHeight: el.clientHeight }))

	// The grip lowest on screen belongs to the tile whose bottom is nearest the fold.
	const grips = await page.locator('.dv-resize-handle').all()
	let grip = null
	let gripBox = null
	for (const g of grips) {
		const b = await g.boundingBox()
		if (b && (!gripBox || b.y > gripBox.y)) {
			grip = g
			gripBox = b
		}
	}
	const tile = grip.locator('..')
	const tileBefore = await tile.boundingBox()

	const gx = gripBox.x + gripBox.width / 2
	await page.mouse.move(gx, gripBox.y + gripBox.height / 2)
	await page.mouse.down()
	// Straight down to well past the window bottom — with the pointer captured these moves still
	// reach the grip, client y > viewport height and all.
	await page.mouse.move(gx, before.clientHeight + 400, { steps: 20 })
	await page.mouse.up()

	const after = await root.evaluate((el) => ({ scrollHeight: el.scrollHeight, clientHeight: el.clientHeight }))
	expect(after.scrollHeight, 'the grid content should have outgrown the viewport').toBeGreaterThan(after.clientHeight)
	expect(after.scrollHeight).toBeGreaterThan(before.scrollHeight)

	const tileAfter = await tile.boundingBox()
	expect(tileAfter.height, 'the tile itself should keep stretching past the fold').toBeGreaterThan(tileBefore.height + 300)

	// And the overgrown content is reachable: the root actually scrolls down.
	const scrolled = await root.evaluate((el) => {
		el.scrollTop = 10_000
		return el.scrollTop
	})
	expect(scrolled).toBeGreaterThan(0)
})
