// Games against the engine.
const { test, expect } = require("@playwright/test");
const { enter, tapMove, dragMove, cleanup, moveCount } = require("./helpers");

test.afterEach(async ({ page }) => cleanup(page));

async function startEngineGame(page, level, color) {
  await page.click(`#level-picker button[data-level="${level}"]`);
  await page.click(`#engine-color button[data-color="${color}"]`);
  await page.click("#play-engine-btn");
  await expect(page.locator("#view-game")).toBeVisible();
  await expect(page).toHaveURL(/#\/game\/\d+/);
}

test("play white against the engine: move, engine replies, take back", async ({ page }) => {
  await enter(page, "E");
  await startEngineGame(page, 1, "w");
  await expect(page.locator("#status-line")).toHaveText("Your turn");
  await expect(page.locator("#bar-bottom .player-name")).toContainText("(you)");
  await expect(page.locator("#bar-top .player-name")).toContainText("level 1");

  // Legal-move dots after selecting a pawn.
  await page.click(".sq[data-sq=e2]");
  await expect(page.locator(".sq.dest")).toHaveCount(2);
  await page.click(".sq[data-sq=e4]");

  // The engine replies.
  await expect.poll(() => moveCount(page), { timeout: 15000 }).toBe(2);
  await expect(page.locator("#moves .mv").first()).toHaveText("e4");
  await expect(page.locator("#status-line")).toHaveText("Your turn");

  // Second move by dragging.
  await dragMove(page, "g1", "f3");
  await expect.poll(() => moveCount(page), { timeout: 15000 }).toBe(4);
  await expect(page.locator("#moves .mv").nth(2)).toHaveText("Nf3");

  // Takeback: undoes my move and the engine's reply.
  await page.click("#takeback-btn");
  await expect.poll(() => moveCount(page)).toBe(2);
});

test("play black: engine opens, hint shows squares, illegal move is refused", async ({ page }) => {
  await enter(page, "B");
  await startEngineGame(page, 2, "b");
  // Black at the bottom.
  await expect(page.locator("#bar-bottom")).toHaveAttribute("data-color", "b");
  await expect.poll(() => moveCount(page), { timeout: 15000 }).toBe(1);

  // An illegal move (pawn three squares) is not played.
  await tapMove(page, "e7", "e4");
  await expect.poll(() => moveCount(page)).toBe(1);

  await page.click("#hint-btn");
  await expect(page.locator(".sq.hint-sq")).toHaveCount(2, { timeout: 15000 });
});

test("history navigation shows older positions", async ({ page }) => {
  await enter(page, "H");
  await startEngineGame(page, 1, "w");
  await tapMove(page, "d2", "d4");
  await expect.poll(() => moveCount(page), { timeout: 15000 }).toBe(2);
  await page.click("#nav-first");
  // Starting position: pawn on d2.
  await expect(page.locator(".piece.wP")).toHaveCount(8);
  await expect(page.locator("#moves .mv.active")).toHaveCount(0);
  await page.click("#nav-next");
  await expect(page.locator("#moves .mv.active")).toHaveText("d4");
  await page.click("#nav-last");
  await expect(page.locator("#moves .mv.active")).toHaveCount(1);
});

test("resigning ends the game and rematch starts a new one", async ({ page }) => {
  await enter(page, "X");
  await startEngineGame(page, 1, "w");
  await tapMove(page, "e2", "e4");
  await expect.poll(() => moveCount(page), { timeout: 15000 }).toBe(2);
  // No dialogs (captive portals block them) – confirmation is a second click.
  page.on("dialog", () => { throw new Error("the app must not use dialogs"); });
  await page.click("#resign-btn");
  await expect(page.locator("#resign-btn")).toContainText("Tap again");
  await expect(page.locator("#result-box")).toBeHidden();
  // The armed state expires after 3 s → the first click only arms again.
  await page.waitForTimeout(3300);
  await expect(page.locator("#resign-btn")).toHaveText("Resign");
  await page.click("#resign-btn");
  await page.click("#resign-btn");
  await expect(page.locator("#result-box")).toBeVisible();
  await expect(page.locator("#result-score")).toHaveText("0-1");
  await expect(page.locator("#result-reason")).toContainText("Resignation");
  const oldUrl = page.url();
  await page.click("#rematch-btn");
  await expect(page).not.toHaveURL(oldUrl);
  // Rematch: colours swapped → now we play black.
  await expect(page.locator("#bar-bottom")).toHaveAttribute("data-color", "b");
});

test("no horizontal scrolling on the game screen", async ({ page }) => {
  await enter(page, "O");
  await startEngineGame(page, 1, "w");
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  // The board is square and fits the width.
  const box = await page.locator("#board").boundingBox();
  expect(Math.abs(box.width - box.height)).toBeLessThan(2);
  expect(box.width).toBeLessThanOrEqual(page.viewportSize().width);
});
