import { existsSync } from "node:fs";
import { defineConfig } from "@playwright/test";

const browserEngine = process.env.QWENPAW_BROWSER_ENGINE || "chromium";
if (browserEngine !== "chromium" && browserEngine !== "webkit") {
  throw new Error(`Invalid QWENPAW_BROWSER_ENGINE: ${browserEngine}`);
}
const port = Number(process.env.QWENPAW_BROWSER_PORT || 4176);
const secureWebKitGateway = browserEngine === "webkit";
const baseURL = `${secureWebKitGateway ? "https" : "http"}://127.0.0.1:${port}/app/qwenpaw/`;
const localChromium = browserEngine === "chromium"
  ? process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || (process.env.CI ? undefined : "/usr/bin/chromium")
  : undefined;

if (!Number.isInteger(port) || port < 1024 || port > 65535) {
  throw new Error(`Invalid QWENPAW_BROWSER_PORT: ${process.env.QWENPAW_BROWSER_PORT}`);
}
if (localChromium && !existsSync(localChromium)) {
  throw new Error(`PLAYWRIGHT_CHROMIUM_EXECUTABLE does not exist: ${localChromium}`);
}

export default defineConfig({
  testDir: "./e2e",
  testMatch: "gateway.spec.ts",
  fullyParallel: false,
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  reporter: "list",
  outputDir: process.env.QWENPAW_BROWSER_RESULTS || "/tmp/qwenpaw-playwright-gateway-results",
  use: {
    baseURL,
    browserName: browserEngine,
    headless: true,
    ignoreHTTPSErrors: secureWebKitGateway,
    launchOptions: localChromium ? { executablePath: localChromium } : {},
    trace: "off",
    screenshot: "off",
    video: "off",
  },
  webServer: {
    command: "python3 ../scripts/browser-gateway.py",
    url: baseURL,
    timeout: 120_000,
    gracefulShutdown: { signal: "SIGTERM", timeout: 90_000 },
    ignoreHTTPSErrors: secureWebKitGateway,
    reuseExistingServer: false,
    env: {
      QWENPAW_BROWSER_PORT: String(port),
      QWENPAW_BROWSER_TLS: secureWebKitGateway ? "1" : "0",
    },
  },
});
