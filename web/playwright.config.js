// Playwright tests for the web UI. Server = the board simulator (sim/) in fast mode:
// the same HTML, API and engine as on the ESP32, just running on a PC.
const { defineConfig, devices } = require("@playwright/test");

const PORT = 8765;

module.exports = defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  // Tests share one server (and thus one hub) → they run sequentially.
  workers: 1,
  fullyParallel: false,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"] } },
    { name: "mobile", use: { ...devices["Pixel 7"] } },
  ],
  webServer: {
    // --fast = the engine thinks 20× shorter; --no-demo = the engine is free for tests.
    command: `cargo run -q -p chess-sim -- --port ${PORT} --fast --no-demo`,
    cwd: "..",
    url: `http://localhost:${PORT}/`,
    timeout: 300_000,
    reuseExistingServer: false,
  },
});
