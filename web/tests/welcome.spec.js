// Welcome screen, nicknames, languages, settings, layout.
const { test, expect } = require("@playwright/test");
const { enter, uniqueNick, api, cleanup } = require("./helpers");

test.afterEach(async ({ page }) => cleanup(page));

test("invalid nickname shows an error, valid one enters the lobby", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#view-welcome")).toBeVisible();
  // A random nickname is prefilled.
  await expect(page.locator("#nick-input")).not.toHaveValue("");
  await page.fill("#nick-input", "<b>");
  await page.click("#nick-form button[type=submit]");
  await expect(page.locator("#nick-error")).toBeVisible();

  const nick = uniqueNick("W");
  await page.fill("#nick-input", nick);
  await page.click("#nick-form button[type=submit]");
  await expect(page.locator("#view-lobby")).toBeVisible();
  await expect(page.locator("#player-list")).toContainText(nick);
});

test("session survives a page reload", async ({ page }) => {
  const nick = await enter(page, "R");
  await page.reload();
  await expect(page.locator("#view-lobby")).toBeVisible();
  await expect(page.locator("#nick-chip")).toHaveText(nick);
});

test("duplicate nickname is rejected", async ({ page, browser }) => {
  const nick = await enter(page, "D");
  const other = await browser.newContext();
  const page2 = await other.newPage();
  await page2.goto("/");
  await page2.fill("#nick-input", nick.toUpperCase());
  await page2.click("#nick-form button[type=submit]");
  await expect(page2.locator("#nick-error")).toContainText("taken");
  await other.close();
});

test("English is the default and Czech can be selected and persists", async ({ page }) => {
  await enter(page, "L");
  await expect(page.locator("#play-engine-btn")).toHaveText("Start game");
  await page.goto("/#/settings");
  await page.click("#lang-switch-settings button[data-lang=cs]");
  await expect(page.locator("#view-settings h2").first()).toHaveText("Nastavení");
  await page.goto("/#/");
  await page.reload();
  await expect(page.locator("#play-engine-btn")).toHaveText("Začít partii");
  await expect(page.locator("html")).toHaveAttribute("lang", "cs");
  // Back to English.
  await page.goto("/#/settings");
  await page.click("#lang-switch-settings button[data-lang=en]");
  await expect(page.locator("#view-settings h2").first()).toHaveText("Settings");
});

test("theme, board colours and coordinates can be changed", async ({ page }) => {
  await enter(page, "T");
  await page.goto("/#/settings");
  await page.click("#theme-picker button[data-theme-choice=light]");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.click("#board-picker button[data-board=blue]");
  await expect(page.locator("html")).toHaveAttribute("data-board", "blue");
  await page.uncheck("#coords-toggle");
  await expect(page.locator("body")).toHaveClass(/hide-coords/);
  await page.click("#theme-picker button[data-theme-choice=dark]");
  await page.check("#coords-toggle");
});

test("nickname can be changed in settings", async ({ page }) => {
  await enter(page, "N");
  await page.goto("/#/settings");
  const renamed = uniqueNick("Renamed");
  await page.fill("#rename-input", renamed);
  await page.click("#rename-form button[type=submit]");
  await expect(page.locator("#nick-chip")).toHaveText(renamed);
});

test("Wi-Fi settings: only the owner sees the form", async ({ page }) => {
  await enter(page, "F");
  await page.goto("/#/settings");
  const lobby = await api(page, "lobby");
  if (lobby.net.owner) {
    await expect(page.locator("#wifi-owner")).toBeVisible();
    await page.click("#wifi-scan-btn");
    await expect(page.locator("#wifi-list li")).toHaveCount(3);
    await page.click("#wifi-list li >> nth=0");
    await expect(page.locator("#wifi-ssid")).toHaveValue("Home-WiFi");
  } else {
    await expect(page.locator("#wifi-not-owner")).toBeVisible();
    await expect(page.locator("#wifi-owner")).toBeHidden();
    // An unauthorised attempt to change WiFi is rejected.
    const r = await api(page, "wifi", { ssid: "Evil", pass: "12345678" });
    expect(r.ok).toBe(false);
  }
  await expect(page.locator("#wifi-status")).not.toBeEmpty();
});

test("logging out returns to the welcome screen and frees the nickname", async ({ page }) => {
  const nick = await enter(page, "Q");
  await page.goto("/#/settings");
  await page.click("#logout-btn");
  await expect(page.locator("#view-welcome")).toBeVisible();
  await page.fill("#nick-input", nick);
  await page.click("#nick-form button[type=submit]");
  await expect(page.locator("#view-lobby")).toBeVisible();
});

test("captive portal probes redirect to the app", async ({ request }) => {
  const res = await request.get("/generate_204", { maxRedirects: 0 });
  expect(res.status()).toBe(302);
  expect(res.headers()["location"]).toContain("http://");
});

test("no horizontal scrolling in lobby", async ({ page }) => {
  await enter(page, "S");
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
});
