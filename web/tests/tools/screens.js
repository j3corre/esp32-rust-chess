// Development helper: takes screenshots of the main screens.
// Usage: node tests/tools/screens.js http://localhost:8123 <output dir>
const { chromium, devices } = require("@playwright/test");

(async () => {
  const base = process.argv[2] || "http://localhost:8080";
  const out = process.argv[3] || ".";
  const browser = await chromium.launch();
  for (const [name, device] of [["mobile", devices["Pixel 7"]], ["desktop", { viewport: { width: 1280, height: 800 } }]]) {
    const context = await browser.newContext(device);
    const page = await context.newPage();
    page.on("console", (msg) => { if (msg.type() === "error") console.log(name, "console:", msg.text()); });
    page.on("pageerror", (err) => console.log(name, "pageerror:", err.message));
    await page.goto(base + "/");
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${out}/${name}-1-welcome.png` });
    await page.fill("#nick-input", name + " " + (Date.now() % 1000));
    await page.click("#nick-form button[type=submit]");
    await page.waitForTimeout(1200);
    await page.screenshot({ path: `${out}/${name}-2-lobby.png`, fullPage: true });
    await page.click("#play-engine-btn");
    await page.waitForTimeout(1500);
    // Play e2e4 by tapping squares.
    const move = async (from, to) => {
      await page.click(`.sq[data-sq=${from}]`);
      await page.click(`.sq[data-sq=${to}]`);
    };
    const you = await page.evaluate(() => document.querySelector("#bar-bottom").dataset.color);
    if (you === "w") await move("e2", "e4"); else { await page.waitForTimeout(1000); await move("e7", "e5"); }
    await page.waitForTimeout(1500);
    await page.click(`.sq[data-sq=${you === "w" ? "g1" : "g8"}]`);
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${out}/${name}-3-game.png`, fullPage: true });
    await page.goto(base + "/#/settings");
    await page.waitForTimeout(800);
    await page.screenshot({ path: `${out}/${name}-4-settings.png`, fullPage: true });
    await context.close();
  }
  await browser.close();
})();
