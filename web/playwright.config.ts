import { existsSync } from "node:fs";
import { defineConfig } from "@playwright/test";

const port = Number(process.env.QWENPAW_BROWSER_PORT || 4175);
const baseURL = `http://127.0.0.1:${port}/cgi/ThirdParty/qwenpaw/index.cgi/`;
const localChromium = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
  || (process.env.CI ? undefined : "/usr/bin/chromium");

if (!Number.isInteger(port) || port < 1024 || port > 65535) {
  throw new Error(`Invalid QWENPAW_BROWSER_PORT: ${process.env.QWENPAW_BROWSER_PORT}`);
}
if (localChromium && !existsSync(localChromium)) {
  throw new Error(`PLAYWRIGHT_CHROMIUM_EXECUTABLE does not exist: ${localChromium}`);
}

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: "list",
  outputDir: "/tmp/qwenpaw-playwright-results",
  use: {
    baseURL,
    browserName: "chromium",
    headless: true,
    launchOptions: localChromium ? { executablePath: localChromium } : {},
    trace: "off",
    screenshot: "off",
    video: "off",
  },
  webServer: {
    command: "python3 ../scripts/browser-cgi-gateway.py",
    url: baseURL,
    timeout: 120_000,
    gracefulShutdown: { signal: "SIGTERM", timeout: 90_000 },
    reuseExistingServer: false,
    env: {
      QWENPAW_BROWSER_PORT: String(port),
    },
  },
});
