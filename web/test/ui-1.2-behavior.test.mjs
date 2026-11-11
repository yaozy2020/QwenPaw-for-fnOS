import test, { before, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium, expect } from "@playwright/test";

const prefix = "/app/qwenpaw/";
const origin = "http://qwenpaw.test";
const chromiumExecutable = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
  || (existsSync("/usr/bin/chromium") ? "/usr/bin/chromium" : chromium.executablePath());
const browserUnavailable = existsSync(chromiumExecutable)
  ? null
  : "Playwright Chromium is not installed; run the frontend browser tests in an environment with Chromium";
if (browserUnavailable && (process.env.STRICT_CI === "1" || process.env.CI || process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE)) {
  throw new Error(`${browserUnavailable}: ${chromiumExecutable}`);
}
let html;
let browser;

const browserTest = (name, fn) => browserUnavailable
  ? test(name, { skip: browserUnavailable }, fn)
  : test(name, fn);

before(async () => {
  if (browserUnavailable) return;
  const [template, css, bundled] = await Promise.all([
    readFile(new URL("../src/index.html", import.meta.url), "utf8"),
    readFile(new URL("../src/style.css", import.meta.url), "utf8"),
    build({ entryPoints: [fileURLToPath(new URL("../src/main.ts", import.meta.url))], bundle: true,
      write: false, format: "iife", target: "es2020", jsx: "automatic", jsxImportSource: "preact" }),
  ]);
  html = template.replace("/*__INLINE_CSS__*/", () => css)
    .replace("//__INLINE_JS__", () => bundled.outputFiles[0].text.replace(/<\/script>/gi, "<\\/script>"));
  browser = await chromium.launch({ executablePath: chromiumExecutable, headless: true, args: ["--no-sandbox"] });
});

after(async () => { await browser?.close(); });

async function fixture(t, options = {}) {
  const state = {
    status: { success: true, installed: false, running: false, install: { running: false } },
    proxy: { success: true, port: 43126, public: false },
    proxyStatus: 200,
    versions: { success: true, latest_stable: "2.0.0", versions: [
      { version: "2.1.0b1", is_beta: true }, { version: "2.0.0", is_beta: false }, { version: "1.9.0", is_beta: false },
    ] },
    install: { success: true, upgrading: false, running: false, seq: 0, instance: "worker-a", phase: "idle" },
    logs: ["原始服务日志"],
    installLogs: ["本任务日志"],
    clearStatus: 200,
    posts: [], logQueries: [], requestPaths: [], statusPolls: 0,
    ...options,
  };
  const context = await browser.newContext({ viewport: options.viewport, javaScriptEnabled: options.javaScriptEnabled });
  t.after(() => context.close());
  await context.addCookies([{ name: "qwenpaw_csrf", value: "test-token", url: origin }]);
  const page = await context.newPage();
  if (options.initScript) await page.addInitScript(options.initScript);
  await page.route("**/*", async route => {
    const req = route.request();
    const url = new URL(req.url());
    if (url.pathname === prefix) return route.fulfill({ contentType: "text/html", body: html });
    if (!url.pathname.startsWith(`${prefix}api/`)) return route.abort();
    const path = url.pathname.slice(`${prefix}api/`.length);
    state.requestPaths.push({ method: req.method(), path });
    if (req.method() === "POST") {
      state.posts.push({ path, body: req.postDataJSON(), csrf: req.headers()["x-csrf-token"] });
      if (path === "kernel/install") {
        state.install = { success: true, upgrading: true, running: true, seq: 42, instance: "worker-a", phase: "installing" };
        return route.fulfill({ json: { success: true, seq: 42, instance: "worker-a" } });
      }
      if (path === "logs/clear") {
        if (state.clearStatus === 200) state.logs = [];
        return route.fulfill({ status: state.clearStatus, json: { success: state.clearStatus === 200, message: "删除失败" } });
      }
      if (path === "web-proxy/port") return route.fulfill({ json: { success: true, port: req.postDataJSON().port } });
      return route.fulfill({ json: { success: true } });
    }
    if (path === "kernel/versions" && state.versionsHeld) await state.versionsHeld;
    if (path === "status") return route.fulfill({ status: state.statusCode || 200, json: state.status });
    if (path === "web-proxy/port") return route.fulfill({ status: state.proxyStatus, json: state.proxy });
    if (path === "kernel/versions") return route.fulfill({ json: state.versions });
    if (path === "kernel/install-status") { state.statusPolls++; return route.fulfill({ json: state.install }); }
    if (path === "kernel/install-log") {
      state.logQueries.push({ seq: url.searchParams.get("seq"), instance: url.searchParams.get("instance") });
      return route.fulfill({ json: { success: true, logs: state.installLogs } });
    }
    if (path === "logs") {
      if (state.logsHeld) await state.logsHeld;
      return route.fulfill({ json: { success: true, logs: state.logs } });
    }
    if (path === "mirror") return route.fulfill({ json: { success: true, mirror: "" } });
    return route.fulfill({ status: 404, json: { success: false } });
  });
  return { page, state };
}

async function openInstall(page) {
  await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
  await page.locator("#btnUpgradeAction").click();
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
}

async function pollNow(page, state) {
  const before = state.statusPolls;
  await page.evaluate(() => window.dispatchEvent(new Event("pageshow")));
  await expect.poll(() => state.statusPolls).toBeGreaterThan(before);
}

async function box(locator) {
  const bounds = await locator.boundingBox();
  assert.ok(bounds, "element is rendered");
  return bounds;
}

const desktop = { width: 1440, height: 900 };
const runningStatus = {
  success: true, installed: true, running: true, pid: 421,
  startAt: Math.floor(Date.now() / 1000) - 3600, cpu: 1.5, memKB: 102400,
  version: "2.0.0", authStatus: "protected", install: { running: false },
};

browserTest("revealed notices keep the icon beside the message", async t => {
  const { page } = await fixture(t, { viewport: desktop, versions: { success: false, message: "mirror down" } });
  await page.goto(origin + prefix);
  for (const selector of ["#kernelInstallTip", "#versionSourceStatus"]) {
    await expect(page.locator(selector)).toBeVisible();
    await expect(page.locator(selector)).toHaveCSS("display", "flex");
  }
  const tip = page.locator("#kernelInstallTip");
  const icon = await box(tip.locator("svg"));
  const text = await box(tip.locator("span").first());
  assert.ok(icon.x + icon.width <= text.x, "icon sits to the left of the message");
  assert.ok(icon.y >= text.y && icon.y < text.y + 24, "icon shares the first line of the message");
});

browserTest("downgrade warning and kernel removal keep their layouts when revealed", async t => {
  const { page } = await fixture(t, { viewport: desktop, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#btnUninstallKernel")).toBeVisible();
  const uninstallDisplay = await page.locator("#btnUninstallKernel").evaluate(el => getComputedStyle(el).display);
  assert.equal(uninstallDisplay, await page.locator("#btnUpgradeAction").evaluate(el => getComputedStyle(el).display));
  assert.notEqual(uninstallDisplay, "block");
  await expect(page.locator("#versionCardsContainer")).toContainText("2.0.0");
  await expect(page.locator("#downgradeWarnBar")).toBeHidden();
  await page.evaluate(() => window.pickVersion("1.9.0"));
  await expect(page.locator("#btnUpgradeAction")).toHaveText("降级至 v1.9.0");
  await expect(page.locator("#downgradeWarnBar")).toBeVisible();
  await expect(page.locator("#downgradeWarnBar")).toHaveCSS("display", "flex");
  await page.evaluate(() => window.pickVersion("2.0.0"));
  await expect(page.locator("#downgradeWarnBar")).toBeHidden();
});

browserTest("desktop dashboard spans the overview and pairs version with configuration", async t => {
  const { page } = await fixture(t, { viewport: desktop, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  const grid = await box(page.locator(".grid"));
  const overview = await box(page.locator("#overview"));
  const version = await box(page.locator("#version"));
  const configuration = await box(page.locator("#configuration"));
  assert.ok(Math.abs(overview.width - grid.width) <= 1, "overview spans both columns");
  assert.ok(version.y >= overview.y + overview.height, "version starts below the overview");
  assert.ok(Math.abs(version.y - configuration.y) <= 1, "version and configuration share a row");
  assert.ok(Math.abs(version.height - configuration.height) <= 1, "paired cards end on the same line");
  assert.ok(version.x + version.width <= configuration.x, "version precedes configuration like the section navigation");
  const order = await page.evaluate(() => [...document.querySelectorAll("[data-mobile-panel]")].map(el => el.id));
  assert.deepEqual(order, ["overview", "version", "configuration", "logs"]);
});

browserTest("install steps follow only the owned task and mark where it stopped", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix);
  await openInstall(page);
  const step = name => page.locator(`#upSteps [data-step="${name}"]`);
  await expect(step("installing")).toHaveAttribute("aria-current", "step");
  await expect(step("downloading")).toHaveAttribute("data-status", "done");
  await expect(step("starting")).toHaveAttribute("data-status", "pending");
  await expect(page.locator("#btnUpMinimize")).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(page.locator("#btnUpMinimize")).toBeFocused();

  state.install = { ...state.install, instance: "worker-b", phase: "starting" };
  await pollNow(page, state);
  await expect(step("installing")).toHaveAttribute("aria-current", "step");

  state.install = { ...state.install, instance: "worker-a", phase: "starting" };
  await pollNow(page, state);
  await expect(step("starting")).toHaveAttribute("aria-current", "step");
  await expect(step("installing")).toHaveAttribute("data-status", "done");

  state.install = { ...state.install, upgrading: false, running: false, phase: "failed" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).toContainText("遇到问题");
  await expect(step("starting")).toHaveAttribute("data-status", "failed");
  await expect(step("installing")).toHaveAttribute("data-status", "done");
});

browserTest("long logs scroll inside a bounded terminal that follows the newest line", async t => {
  const { page } = await fixture(t, { viewport: desktop, logs: Array.from({ length: 100 }, (_, index) => `日志 ${index + 1}`) });
  await page.goto(origin + prefix);
  const terminal = page.locator("#logTerminal");
  await expect(terminal).toContainText("日志 100");
  const metrics = await terminal.evaluate(el => ({ client: el.clientHeight, scroll: el.scrollHeight, top: el.scrollTop }));
  assert.ok(metrics.client >= 320 && metrics.client <= 640, `terminal height ${metrics.client}`);
  assert.ok(metrics.scroll > metrics.client, "older lines stay reachable by scrolling");
  assert.ok(metrics.top + metrics.client >= metrics.scroll - 2, "terminal follows the newest line");
  assert.ok((await box(page.locator("#logs"))).height < desktop.height, "log card fits within one viewport");
});

browserTest("desktop secondary controls keep comfortable pointer targets", async t => {
  const { page } = await fixture(t, { viewport: desktop });
  await page.goto(origin + prefix);
  for (const name of ["修改端口", "修改反代端口", "检查更新"]) {
    const bounds = await box(page.getByRole("button", { name, exact: true }));
    assert.ok(bounds.height >= 28, `${name} height ${bounds.height}`);
  }
  const archive = await box(page.locator(".history-versions-toggle"));
  assert.ok(archive.height >= 28 && archive.width >= 64, `archive toggle ${archive.width}x${archive.height}`);
  for (const selector of ["#webProxyPublic", "#autoScrollCheck", "#autoRefreshCheck"]) {
    const bounds = await box(page.locator(selector));
    assert.ok(bounds.width >= 16 && bounds.height >= 16, `${selector} ${bounds.width}x${bounds.height}`);
  }
});

browserTest("theme toggle names the current mode and the action it performs", async t => {
  const { page } = await fixture(t, { initScript: () => localStorage.setItem("qwenpaw_theme_v2", "dark") });
  await page.goto(origin + prefix);
  const toggle = page.locator("#themeToggleBtn");
  await expect(toggle).toHaveAttribute("aria-label", "暗夜模式，切换到白天模式");
  await toggle.click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator("#themeText")).toHaveText("白天");
  await expect(toggle).toHaveAttribute("aria-label", "白天模式，切换到暗夜模式");
});

browserTest("mobile header keeps its actions on one compact row", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 }, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#webConsoleText")).toHaveText("进入 Web 对话");
  const header = await box(page.locator("header"));
  assert.ok(header.height <= 150, `header height ${header.height}`);
  const actions = await Promise.all(["#themeToggleBtn", "#refreshDashboardBtn", "#btnWebConsole"].map(selector => box(page.locator(selector))));
  const tops = actions.map(bounds => bounds.y);
  assert.ok(Math.max(...tops) - Math.min(...tops) <= 1, "actions share one row");
  assert.ok(actions.every(bounds => bounds.height >= 44 && bounds.width >= 44));
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
});

browserTest("uninstalled overview explains the next step and links to version selection", async t => {
  const { page } = await fixture(t);
  await page.goto(origin + prefix);
  await expect(page.locator("#statusCardMount .status-description")).toHaveText("先安装 QwenPaw 内核，即可开始使用 AI 对话。");
  await expect(page.locator("#statusInstallLink")).toBeVisible();
  await expect(page.locator("#statusInstallLink")).toHaveAttribute("href", "#version");
  await expect(page.locator("#btnWebConsole")).toHaveAttribute("href", "#version");
  await expect(page.locator("#webConsoleText")).toHaveText("安装并开始使用");
  await expect(page.locator("#statusCheckedAt")).toContainText("最近同步");
});

browserTest("running overview provides direct console access and hides installation actions", async t => {
  const { page } = await fixture(t, { status: {
    success: true, installed: true, running: true, pid: 421,
    startAt: Math.floor(Date.now() / 1000) - 3600, cpu: 1.5, memKB: 102400,
    version: "2.0.0", authStatus: "protected", install: { running: false },
  } });
  await page.goto(origin + prefix);
  await expect(page.locator("#statusCardMount .status-description")).toContainText("服务已就绪");
  await expect(page.locator("#btnWebConsole")).toHaveAttribute("href", "/app/qwenpaw/web/");
  await expect(page.locator("#webConsoleText")).toHaveText("进入 Web 对话");
  await expect(page.locator("#statusInstallLink")).toBeHidden();
  await expect(page.locator("#btnStartService")).toBeHidden();
  await expect(page.locator("#btnRestartService")).toBeVisible();
  await expect(page.locator("#statusCheckedAt")).toContainText("最近同步");
});

browserTest("manual refresh exposes busy state until status and logs finish", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix);
  await expect(page.locator("#logTerminal")).toContainText("原始服务日志");
  let release;
  state.logsHeld = new Promise(resolve => { release = resolve; });
  t.after(() => release());
  await page.locator("#refreshDashboardBtn").click();
  await expect(page.locator("#refreshDashboardBtn")).toBeDisabled();
  await expect(page.locator("#refreshDashboardBtn")).toHaveAttribute("aria-busy", "true");
  release();
  state.logsHeld = null;
  await expect(page.locator("#refreshDashboardBtn")).toBeEnabled();
  await expect(page.locator("#refreshDashboardBtn")).not.toHaveAttribute("aria-busy");
});

browserTest("overview shortcuts navigate mobile sections without horizontal overflow", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  const shortcuts = page.locator(".overview-shortcuts a");
  await expect(shortcuts).toHaveCount(3);
  await expect(shortcuts.nth(0)).toHaveAttribute("href", "#version");
  await expect(shortcuts.nth(1)).toHaveAttribute("href", "#configuration");
  await expect(shortcuts.nth(2)).toHaveAttribute("href", "#logs");
  for (const shortcut of await shortcuts.all()) {
    const size = await shortcut.boundingBox();
    assert.ok(size && size.width >= 44 && size.height >= 44);
  }
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await shortcuts.nth(1).click();
  await expect(page.locator("#configuration")).toBeVisible();
  await expect(page.locator('#mobileSectionNav [data-section="configuration"]')).toHaveAttribute("aria-current", "page");
});

browserTest("mobile bottom navigation defaults to overview and synchronizes hash and history", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  const nav = page.locator("#mobileSectionNav");
  const overview = nav.locator('[data-section="overview"]');
  const version = nav.locator('[data-section="version"]');
  await expect(nav).toHaveCSS("position", "fixed");
  const bounds = await nav.boundingBox();
  assert.ok(bounds && Math.abs(bounds.y + bounds.height - 844) <= 1);
  assert.equal(await nav.locator("button").count(), 4);
  for (const button of await nav.locator("button").all()) {
    const size = await button.boundingBox();
    assert.ok(size && size.width >= 44 && size.height >= 44);
  }
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  await expect(overview).toHaveAttribute("aria-current", "page");
  assert.equal(await page.evaluate(() => location.hash), "#overview");
  await expect(page.locator("#overview")).toBeVisible();
  await expect(page.locator("#configuration")).toBeHidden();
  await expect(page.locator("#version")).toBeHidden();
  await expect(page.locator("#logs")).toBeHidden();

  await version.click();
  await expect(version).toHaveAttribute("aria-current", "page");
  await expect(overview).not.toHaveAttribute("aria-current");
  await expect(page.locator("#version")).toBeVisible();
  await expect(page.locator("#overview")).toBeHidden();
  assert.equal(await page.evaluate(() => location.hash), "#version");

  await page.goBack();
  await expect(overview).toHaveAttribute("aria-current", "page");
  await expect(page.locator("#overview")).toBeVisible();
  assert.equal(await page.evaluate(() => location.hash), "#overview");
  await page.goForward();
  await expect(version).toHaveAttribute("aria-current", "page");
  await expect(page.locator("#version")).toBeVisible();
  const configuration = nav.locator('[data-section="configuration"]');
  await configuration.focus();
  await page.keyboard.press("Enter");
  await expect(configuration).toHaveAttribute("aria-current", "page");
  assert.equal(await page.evaluate(() => location.hash), "#configuration");
});

browserTest("mobile configuration is grouped as its own section", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  await page.locator('#mobileSectionNav [data-section="configuration"]').click();
  await expect(page.locator("#configuration")).toBeVisible();
  for (const selector of ["#valPort", "#valWebProxyPort", "#webProxyPublic", "#valAuthStatus", '[onclick="resetAuth()"]']) {
    await expect(page.locator(selector)).toBeVisible();
    assert.equal(await page.locator(`#configuration ${selector}`).count(), 1);
  }
  await expect(page.locator("#overview")).toBeHidden();
  await expect(page.locator("#version")).toBeHidden();
  await expect(page.locator("#logs")).toBeHidden();
  await expect(page.locator('#mobileSectionNav [data-section="configuration"]')).toHaveAttribute("aria-current", "page");
});

browserTest("mobile section deep links and hash changes select the matching panel", async t => {
  const { page } = await fixture(t, { viewport: { width: 375, height: 812 } });
  await page.goto(origin + prefix + "#configuration");
  await expect(page.locator("#configuration")).toBeVisible();
  await expect(page.locator('#mobileSectionNav [data-section="configuration"]')).toHaveAttribute("aria-current", "page");

  await page.evaluate(() => { location.hash = "#logs"; });
  await expect(page.locator("#logs")).toBeVisible();
  await expect(page.locator('#mobileSectionNav [data-section="logs"]')).toHaveAttribute("aria-current", "page");
  await page.evaluate(() => { location.hash = "#not-a-section"; });
  await expect(page.locator("#overview")).toBeVisible();
  await expect(page.locator('#mobileSectionNav [data-section="overview"]')).toHaveAttribute("aria-current", "page");
  assert.equal(await page.evaluate(() => location.hash), "#overview");
});

browserTest("section selection keeps desktop content available after resizing", async t => {
  const { page } = await fixture(t, { viewport: { width: 375, height: 812 } });
  await page.goto(origin + prefix + "#version");
  await page.setViewportSize({ width: 1200, height: 800 });
  await expect(page.locator("#mobileSectionNav")).toBeHidden();
  for (const section of ["overview", "version", "configuration", "logs"]) {
    await expect(page.locator(`#${section}`)).toBeVisible();
  }
  await expect(page.locator('.section-nav a[href="#version"]')).toHaveAttribute("aria-current", "page");
  await page.locator('.section-nav a[href="#configuration"]').click();
  await expect(page.locator('.section-nav a[href="#configuration"]')).toHaveAttribute("aria-current", "page");
  await page.setViewportSize({ width: 375, height: 812 });
  await expect(page.locator("#configuration")).toBeVisible();
  await expect(page.locator("#version")).toBeHidden();
  await expect(page.locator('#mobileSectionNav [data-section="configuration"]')).toHaveAttribute("aria-current", "page");
});

for (const [name, options] of [
  ["JavaScript is disabled", { javaScriptEnabled: false }],
  ["navigation initialization fails", { initScript: () => {
    history.replaceState = () => { throw new Error("blocked history"); };
  } }],
]) {
  browserTest(`all sections remain visible when ${name}`, async t => {
    const { page } = await fixture(t, { viewport: { width: 390, height: 844 }, ...options });
    await page.goto(origin + prefix);
    await expect(page.locator("html")).not.toHaveAttribute("data-mobile-section");
    await expect(page.locator("#mobileSectionNav")).toBeHidden();
    for (const section of ["overview", "version", "configuration", "logs"]) {
      await expect(page.locator(`#${section}`)).toBeVisible();
    }
    await page.locator('.section-nav a[href="#version"]').click();
    await expect(page.locator("html")).not.toHaveAttribute("data-mobile-section");
    for (const section of ["overview", "version", "configuration", "logs"]) {
      await expect(page.locator(`#${section}`)).toBeVisible();
    }
  });
}

browserTest("mobile section switches do not fetch or restart an active install poll", async t => {
  const { page, state } = await fixture(t, {
    viewport: { width: 390, height: 844 },
    initScript: () => {
      const audit = window.navigationAudit = { fetches: 0, timers: [] };
      const originalFetch = window.fetch;
      window.fetch = (...args) => { audit.fetches++; return originalFetch(...args); };
      const originalSetInterval = window.setInterval;
      const originalClearInterval = window.clearInterval;
      window.setInterval = (...args) => {
        const id = originalSetInterval(...args);
        audit.timers.push({ action: "set", id, delay: args[1] });
        return id;
      };
      window.clearInterval = id => { audit.timers.push({ action: "clear", id }); originalClearInterval(id); };
    },
  });
  await page.goto(origin + prefix);
  await page.locator('#mobileSectionNav [data-section="version"]').click();
  await openInstall(page);
  await page.locator("#btnUpMinimize").click();
  await expect.poll(() => state.statusPolls).toBeGreaterThan(0);

  const result = await page.evaluate(() => {
    const audit = window.navigationAudit;
    const before = { fetches: audit.fetches, timers: audit.timers.length, history: history.length };
    const activePoll = audit.timers.findLast(event => event.action === "set" && event.delay === 800);
    for (const section of ["overview", "configuration", "logs", "version", "version"]) {
      document.querySelector(`#mobileSectionNav [data-section="${section}"]`).click();
    }
    return {
      activePoll,
      fetches: audit.fetches - before.fetches,
      timers: audit.timers.slice(before.timers),
      historyEntries: history.length - before.history,
    };
  });
  assert.ok(result.activePoll);
  assert.equal(result.fetches, 0);
  assert.deepEqual(result.timers, []);
  assert.equal(result.historyEntries, 4);
  const polls = state.statusPolls;
  await expect.poll(() => state.statusPolls).toBeGreaterThan(polls);
  assert.equal(state.posts.filter(post => post.path === "kernel/install").length, 1);
});

browserTest("unreachable status stays disabled across delayed version loads and selections, then recovers", async t => {
  let release;
  const versionsHeld = new Promise(resolve => { release = resolve; });
  const { page, state } = await fixture(t, { statusCode: 503, versionsHeld });
  t.after(() => release());
  await page.goto(origin + prefix);
  await expect(page.locator("#statusBadge")).toContainText("状态不可达");
  release();
  await expect(page.locator("#versionCardsContainer")).toContainText("2.0.0");
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  await page.locator("#versionCardsContainer .ver-card-item").first().click();
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  state.statusCode = 200;
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
  await page.locator("#btnUpgradeAction").click();
  state.statusCode = 503;
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator("#statusBadge")).toContainText("状态不可达");
  await page.locator("#modalConfirmBtn").click();
  assert.equal(state.posts.filter(post => post.path === "kernel/install").length, 0);
});

browserTest("unknown proxy scope cannot be silently sent as private when changing the port", async t => {
  const { page, state } = await fixture(t, { proxy: { port: 43126 }, proxyStatus: 200 });
  await page.goto(origin + prefix);
  await expect(page.locator("#valWebProxyPort")).toHaveText("43126");
  await expect(page.locator("#webProxyPublic")).toBeDisabled();
  assert.equal(await page.locator("#webProxyPublic").evaluate(el => el.indeterminate), true);
  await page.evaluate(() => window.changeWebProxyPort());
  await expect(page.locator("#customModal")).not.toHaveClass(/active/);
  assert.equal(state.posts.length, 0);
  state.proxy = { port: 43126, public: true };
  state.proxyStatus = 200;
  await page.reload();
  await expect(page.locator("#webProxyPublic")).toBeEnabled();
  await expect(page.locator("#webProxyPublic")).toBeChecked();
  await page.getByRole("button", { name: "修改反代端口" }).click();
  await page.locator("#modalInput").fill("43127");
  await page.locator("#modalConfirmBtn").click();
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(() => state.posts.length).toBe(1);
  assert.deepEqual(state.posts[0].body, { port: 43127, public: true });
  assert.equal(state.posts[0].csrf, "test-token");
});

browserTest("unknown proxy port disables scope changes instead of requesting automatic reassignment", async t => {
  const { page, state } = await fixture(t, { proxy: { success: true, port: 0, public: false } });
  await page.goto(origin + prefix);
  await expect(page.locator("#valWebProxyPort")).toHaveText("自动分配");
  await expect(page.locator("#webProxyPublic")).toBeDisabled();
  await expect(page.locator("#webProxyPublic")).toHaveJSProperty("indeterminate", true);
  assert.equal(state.posts.length, 0);
});

browserTest("owned task remains reopenable in background and terminal without adopting other identities", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix);
  await expect(page.locator("#taskCenterButton")).toBeHidden();
  await openInstall(page);
  await expect(page.locator("#taskCenterButton")).toBeVisible();
  await expect(page.locator("#btnUpMinimize")).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(page.locator("#btnUpMinimize")).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(page.locator("#btnUpMinimize")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(page.locator("#upgradeModal")).not.toHaveClass(/active/);
  await expect(page.locator("#btnUpgradeAction")).toBeFocused();
  await page.locator("#taskCenterButton").click();
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
  await page.locator("#btnUpMinimize").click();
  state.install = { success: true, upgrading: false, running: false, seq: 43, instance: "worker-a", phase: "done" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).not.toContainText("顺利完成");
  state.install = { ...state.install, seq: 42, instance: "worker-b" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).not.toContainText("顺利完成");
  state.install = { ...state.install, instance: "worker-a" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).toContainText("顺利完成");
  await page.locator("#taskCenterButton").click();
  await expect(page.locator("#upTitle")).toContainText("顺利完成");
  await page.locator("#btnUpClose").click();
  await expect(page.locator("#taskCenterButton")).toBeFocused();
  await page.locator("#taskCenterButton").click();
  await expect(page.locator("#upTitle")).toContainText("顺利完成");
  assert.equal(state.posts.filter(post => post.path === "kernel/install").length, 1);
  assert.ok(state.logQueries.every(query => query.seq === "42" && query.instance === "worker-a"));
});

browserTest("version, archive, mirror and running-task controls work with keyboard and expose semantics", async t => {
  const { page, state } = await fixture(t);
  state.status.install.running = true;
  state.install = { success: true, upgrading: true, running: true, seq: 42, instance: "worker-a", phase: "installing" };
  await page.goto(origin + prefix);
  const card = page.locator("#versionCardsContainer .ver-card-item").first();
  await expect(card).toHaveAttribute("tabindex", "0");
  await card.focus();
  await page.keyboard.press("Enter");
  await expect(card).toHaveAttribute("aria-pressed", "true");
  const archive = page.locator(".history-versions-toggle");
  await archive.focus();
  await page.keyboard.press("Enter");
  const historic = page.locator("#historyDropdown .history-item").first();
  await expect(historic).toBeVisible();
  await historic.focus();
  await page.keyboard.press("Space");
  await expect(page.locator("#btnUpgradeAction")).toContainText("1.9.0");
  await page.locator("#mirrorToggleBtn").click();
  const mirror = page.locator("#mirrorDropdown .history-item").first();
  await mirror.focus();
  await page.keyboard.press("Enter");
  await expect.poll(() => state.posts.some(post => post.path === "mirror")).toBe(true);
  const tip = page.locator("#installRunningTip");
  await expect(tip).toBeVisible();
  await tip.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
  assert.equal(state.posts.filter(post => post.path === "kernel/install").length, 0);
});

browserTest("clearing logs requires confirmation and retains displayed logs on failure", async t => {
  const { page, state } = await fixture(t, { clearStatus: 500 });
  await page.goto(origin + prefix);
  await expect(page.locator("#logTerminal")).toContainText("原始服务日志");
  const clear = page.locator('[onclick="clearTerminalLogs()"]');
  await clear.click();
  await expect(page.locator("#modalMessage")).toContainText("删除");
  await page.keyboard.press("Escape");
  assert.equal(state.posts.length, 0);
  await expect(page.locator("#logTerminal")).toContainText("原始服务日志");
  await clear.click();
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(() => state.posts.length).toBe(1);
  await expect(page.locator("#toast")).toContainText("失败");
  await expect(page.locator("#logTerminal")).toContainText("原始服务日志");
  state.clearStatus = 200;
  await clear.click();
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(() => state.posts.length).toBe(2);
  await expect(page.locator("#logTerminal")).not.toContainText("原始服务日志");
  assert.ok(state.posts.every(post => post.path === "logs/clear" && post.csrf === "test-token"));
});

browserTest("page reconstruction resumes only the stored task identity without another install POST", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix);
  await openInstall(page);
  await expect(page.locator("#taskCenterButton")).toBeVisible();
  await page.reload();
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
  await expect(page.locator("#upTitle")).toContainText("正在查看进行中的安装任务");
  assert.equal(state.posts.filter(post => post.path === "kernel/install").length, 1);
  await expect.poll(() => state.logQueries.length).toBeGreaterThan(0);
  assert.ok(state.logQueries.every(query => query.seq === "42" && query.instance === "worker-a"));
});

browserTest("reopening a terminal task refreshes logs written after completion", async t => {
  const { page, state } = await fixture(t, { installLogs: [] });
  await page.goto(origin + prefix);
  await openInstall(page);
  await expect(page.locator("#upTerminal")).toContainText("正在等待 pip 安装输出");
  state.install = { success: true, upgrading: false, running: false, seq: 42, instance: "worker-a", phase: "done" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).toContainText("顺利完成");
  await page.locator("#btnUpClose").click();
  state.installLogs = ["Successfully installed qwenpaw-2.0.0"];
  await page.locator("#taskCenterButton").click();
  await expect(page.locator("#upTerminal")).toContainText("Successfully installed qwenpaw-2.0.0");
});


browserTest("a full first log page allows loading older entries without unbounded output", async t => {
  const { page } = await fixture(t, { logs: Array.from({ length: 100 }, (_, index) => `日志 ${index + 1}`) });
  const tails = [];
  await page.route("**/app/qwenpaw/api/logs**", async route => {
    const tail = Number(new URL(route.request().url()).searchParams.get("tail") || 100);
    tails.push(tail);
    await route.fulfill({ json: { success: true, logs: Array.from({ length: Math.min(tail, 200) }, (_, index) => `日志 ${index + 1}`) } });
  });
  await page.goto(origin + prefix);
  await expect(page.locator("#logPageIndicator")).toHaveText("第 1 / 1 页");
  await expect(page.locator("#logPrevBtn")).toBeEnabled();
  await page.locator("#logPrevBtn").click();
  await expect(page.locator("#logPageIndicator")).toHaveText("第 1 / 2 页");
  await expect(page.locator("#logTerminal")).toContainText("日志 1");
  assert.ok(tails.includes(200));
});

browserTest("navigation initialization failure preserves working management controls", async t => {
  const { page, state } = await fixture(t, { initScript: () => {
    history.replaceState = () => { throw new Error("blocked history"); };
  } });
  await page.goto(origin + prefix);
  await expect(page.locator("html")).not.toHaveAttribute("data-mobile-section");
  await openInstall(page);
  await expect.poll(() => state.posts.filter(post => post.path === "kernel/install").length).toBe(1);
});

browserTest("older live log responses cannot overwrite the latest refresh", async t => {
  const { page } = await fixture(t);
  await page.goto(origin + prefix);
  await expect(page.locator("#logTerminal")).toContainText("原始服务日志");
  await page.evaluate(() => window.toggleAutoRefresh(false));
  let release;
  const held = new Promise(resolve => { release = resolve; });
  t.after(() => release());
  let requests = 0;
  await page.route("**/app/qwenpaw/api/logs?**", async route => {
    const sequence = ++requests;
    if (sequence === 1) await held;
    await route.fulfill({ json: { success: true, logs: [sequence === 1 ? "旧响应日志" : "最新响应日志"] } });
  });
  await page.evaluate(() => { window.pendingLogRefresh = window.fetchLogs(); });
  await expect.poll(() => requests).toBe(1);
  await page.evaluate(() => window.fetchLogs());
  await expect(page.locator("#logTerminal")).toContainText("最新响应日志");
  release();
  await page.evaluate(() => window.pendingLogRefresh);
  await expect(page.locator("#logTerminal")).toContainText("最新响应日志");
  await expect(page.locator("#logTerminal")).not.toContainText("旧响应日志");
});

browserTest("log history stops requesting older pages at the 2000-line window limit", async t => {
  const { page } = await fixture(t);
  const tails = [];
  await page.route("**/app/qwenpaw/api/logs?**", async route => {
    const tail = Number(new URL(route.request().url()).searchParams.get("tail"));
    tails.push(tail);
    await route.fulfill({ json: { success: true, logs: Array.from({ length: Math.min(tail, 2000) }, (_, index) => `日志 ${index + 1}`) } });
  });
  await page.goto(origin + prefix);
  await expect(page.locator("#logPrevBtn")).toBeEnabled();
  await page.evaluate(() => window.toggleAutoRefresh(false));
  for (let pageCount = 2; pageCount <= 20; pageCount++) {
    await page.locator("#logPrevBtn").click();
    await expect(page.locator("#logPageIndicator")).toHaveText(`第 1 / ${pageCount} 页`);
  }
  await expect(page.locator("#logWindowHint")).toBeVisible();
  await expect(page.locator("#logPrevBtn")).toBeDisabled();
  const requests = tails.length;
  await page.evaluate(() => window.logGoPrev());
  assert.equal(tails.length, requests);
  assert.ok(tails.every(tail => tail <= 2000));
});

for (const outcome of ["accepted", "rejected", "network-error"]) {
  browserTest(`late install ${outcome} cannot replace a subsequently attached task`, async t => {
    const { page, state } = await fixture(t);
    let release;
    const held = new Promise(resolve => { release = resolve; });
    t.after(() => release());
    await page.route("**/api/kernel/install", async route => {
      state.posts.push({ path: "kernel/install", body: route.request().postDataJSON() });
      state.install = { success: true, upgrading: true, seq: 42, instance: "worker-a", phase: "installing" };
      await held;
      if (outcome === "network-error") return route.abort("failed");
      return route.fulfill({ json: { success: outcome === "accepted", seq: 42, instance: "worker-a" } });
    });
    await page.goto(origin + prefix);
    await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
    await page.evaluate(() => { window.pendingInstallRequest = window.checkOrUpgrade(); });
    await page.locator("#modalConfirmBtn").click();
    await expect.poll(() => state.posts.length).toBe(1);
    await page.locator("#btnUpMinimize").click();
    state.install = { ...state.install, seq: 43 };
    await page.evaluate(() => window.attachRunningInstall());
    const identity = () => page.evaluate(() => JSON.parse(sessionStorage.getItem("qwenpaw.install-task.v1")));
    await expect.poll(async () => (await identity())?.seq).toBe(43);
    release();
    await page.evaluate(() => window.pendingInstallRequest);
    assert.deepEqual(await identity(), { seq: 43, instance: "worker-a" });
    await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
    assert.equal(state.posts.length, 1);
  });
}

browserTest("install submits the version confirmed before an asynchronous selection change", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix);
  await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
  await page.locator("#btnUpgradeAction").click();
  await expect(page.locator("#modalMessage")).toContainText("2.0.0");
  await page.evaluate(() => window.pickVersion("1.9.0"));
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(() => state.posts.filter(post => post.path === "kernel/install").length).toBe(1);
  assert.equal(state.posts.find(post => post.path === "kernel/install").body.version, "2.0.0");
});
