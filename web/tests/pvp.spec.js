// Two humans playing each other (two browsers).
const { test, expect } = require("@playwright/test");
const { enter, api, tapMove, dragMove, cleanup, moveCount } = require("./helpers");

// Two players: A creates a challenge (white), B accepts it.
async function setupGame(browser, tc) {
  const ctxA = await browser.newContext();
  const ctxB = await browser.newContext();
  const a = await ctxA.newPage();
  const b = await ctxB.newPage();
  const nickA = await enter(a, "A");
  await enter(b, "B");

  await a.click('#seek-color button[data-color="w"]');
  await a.click(`#tc-grid button[data-tc="${tc}"]`);
  await expect(a.locator("#seeking-box")).toBeVisible();

  // B sees the challenge and accepts it.
  const seek = b.locator("#seek-list li", { hasText: nickA });
  await expect(seek).toBeVisible();
  await seek.locator("button").click();
  await expect(b.locator("#view-game")).toBeVisible();
  // A is taken to the game automatically.
  await expect(a.locator("#view-game")).toBeVisible({ timeout: 10000 });
  return { a, b, ctxA, ctxB };
}

test("two players: challenge, moves, clocks, draw offer, rematch", async ({ browser }) => {
  const { a, b, ctxA, ctxB } = await setupGame(browser, "5+3");
  try {
    await expect(a.locator("#bar-bottom")).toHaveAttribute("data-color", "w");
    await expect(b.locator("#bar-bottom")).toHaveAttribute("data-color", "b");
    await expect(a.locator("#bar-bottom .clock")).toHaveText("5:00");

    await dragMove(a, "e2", "e4");
    await expect(b.locator("#moves .mv").first()).toHaveText("e4", { timeout: 10000 });
    await expect(b.locator("#status-line")).toHaveText("Your turn");
    await tapMove(b, "e7", "e5");
    await expect.poll(() => moveCount(a), { timeout: 10000 }).toBe(2);

    // White's clock runs after the second ply.
    await expect(a.locator("#bar-bottom .clock")).toHaveClass(/running/, { timeout: 5000 });

    // B offers a draw, A accepts it.
    await b.click("#draw-btn");
    await expect(a.locator("#draw-banner")).toBeVisible({ timeout: 10000 });
    await a.click("#draw-accept");
    await expect(a.locator("#result-score")).toHaveText("½-½");
    await expect(b.locator("#result-reason")).toContainText("Draw agreed", { timeout: 10000 });

    // Rematch once both agree.
    await a.click("#rematch-btn");
    await expect(b.locator("#rematch-btn")).toContainText("Accept", { timeout: 10000 });
    await b.click("#rematch-btn");
    await expect(b.locator("#bar-bottom")).toHaveAttribute("data-color", "w");
    await expect(a.locator("#bar-bottom")).toHaveAttribute("data-color", "b", { timeout: 10000 });
  } finally {
    await cleanup(a, b);
    await ctxA.close();
    await ctxB.close();
  }
});

test("promotion chooser and spectators", async ({ browser }) => {
  const { a, b, ctxA, ctxB } = await setupGame(browser, "0+0");
  const ctxS = await browser.newContext();
  const spectator = await ctxS.newPage();
  try {
    const id = a.url().match(/game\/(\d+)/)[1];
    // Quickly reach a promotion via the API: 1.e4 d5 2.exd5 c6 3.dxc6 Nf6 4.cxb7 Nbd7
    const moves = ["e2e4", "d7d5", "e4d5", "c7c6", "d5c6", "g8f6", "c6b7", "b8d7"];
    for (let i = 0; i < moves.length; i++) {
      const r = await api(i % 2 === 0 ? a : b, "move", { id, m: moves[i] });
      expect(r.ok).toBe(true);
    }
    await expect.poll(() => moveCount(a), { timeout: 10000 }).toBe(8);

    // Spectator.
    await enter(spectator, "S");
    await spectator.goto("/#/game/" + id);
    await expect(spectator.locator("#status-line")).toContainText("Spectating");
    await expect(spectator.locator("#actions")).toBeHidden();

    // Promotion: pawn b7 takes the rook on a8 → piece chooser → knight.
    await tapMove(a, "b7", "a8");
    await expect(a.locator("#promo")).toBeVisible();
    await expect(a.locator("#promo button")).toHaveCount(4);
    await a.click('#promo button[data-promo="n"]');
    await expect(a.locator("#moves .mv").last()).toHaveText("bxa8=N", { timeout: 10000 });
    await expect(spectator.locator("#moves .mv").last()).toHaveText("bxa8=N", { timeout: 10000 });
    await expect(a.locator("#viewers")).toContainText("1");
  } finally {
    await cleanup(a, b);
    await ctxA.close();
    await ctxB.close();
    await ctxS.close();
  }
});

test("premove is played automatically when it becomes legal", async ({ browser }) => {
  const { a, b, ctxA, ctxB } = await setupGame(browser, "0+0");
  try {
    await tapMove(a, "e2", "e4");
    await expect.poll(() => moveCount(b), { timeout: 10000 }).toBe(1);
    // A enters a premove while it is B's turn.
    await tapMove(a, "g1", "f3");
    await expect(a.locator(".sq.pre")).toHaveCount(2);
    await tapMove(b, "e7", "e5");
    await expect.poll(() => moveCount(b), { timeout: 10000 }).toBe(3);
    await expect(b.locator("#moves .mv").nth(2)).toHaveText("Nf3");
  } finally {
    await cleanup(a, b);
    await ctxA.close();
    await ctxB.close();
  }
});
