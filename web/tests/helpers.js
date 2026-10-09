// Shared helpers for the Playwright tests.
const { expect } = require("@playwright/test");

let counter = 0;

// Unique nickname (the server is shared by all tests and both projects).
function uniqueNick(prefix) {
  counter++;
  return (prefix + (Date.now() % 100000) + counter).slice(0, 16);
}

// Opens the app and joins with a nickname; waits for the lobby.
async function enter(page, prefix) {
  const nick = uniqueNick(prefix || "P");
  await page.goto("/");
  await expect(page.locator("#view-welcome")).toBeVisible();
  await page.fill("#nick-input", nick);
  await page.click("#nick-form button[type=submit]");
  await expect(page.locator("#view-lobby")).toBeVisible();
  await expect(page.locator("#nick-chip")).toHaveText(nick);
  return nick;
}

// API call from the page context (with the joined player's token).
async function api(page, path, params) {
  return page.evaluate(async ([path, params]) => {
    const token = JSON.parse(localStorage.getItem("chess.token"));
    const q = new URLSearchParams(Object.assign({}, params || {}, token ? { t: token } : {}));
    const res = await fetch("/api/" + path + "?" + q.toString(), { method: "POST" });
    return res.json();
  }, [path, params || {}]);
}

// Move by tapping two squares.
async function tapMove(page, from, to) {
  await page.click(`.sq[data-sq=${from}]`);
  await page.click(`.sq[data-sq=${to}]`);
}

// Move by dragging a piece with the mouse / finger.
async function dragMove(page, from, to) {
  const a = await page.locator(`.sq[data-sq=${from}]`).boundingBox();
  const b = await page.locator(`.sq[data-sq=${to}]`).boundingBox();
  await page.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await page.mouse.down();
  await page.mouse.move(a.x + a.width / 2 + 10, a.y + a.height / 2 + 10, { steps: 3 });
  await page.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 8 });
  await page.mouse.up();
}

// Post-test cleanup: leave (resigns a running game, cancels challenges and the session).
async function cleanup(...pages) {
  for (const page of pages) {
    try {
      await api(page, "bye");
    } catch (e) {
      // the page may already be closed
    }
  }
}

// Number of moves in the move list.
function moveCount(page) {
  return page.locator("#moves .mv").count();
}

module.exports = { uniqueNick, enter, api, tapMove, dragMove, cleanup, moveCount };
