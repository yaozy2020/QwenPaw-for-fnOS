import test, { before, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { buildPanel } from "../build-panel.mjs";
import { chromium, expect } from "@playwright/test";

async function pickVersion(page, version) {
  await page.locator('#targetVersion').click();
  await page.locator(`#versionOptions [data-version="${version}"]`).click();
}

async function openLogTools(page) {
  const tools = page.locator('#logTools');
  if (!await tools.evaluate(element => element.open)) await tools.locator('summary').click();
}

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
const screenshotDirectory = process.env.QWENPAW_UI_ARTIFACT_DIR
  || (process.platform === "win32"
    ? "C:/Users/Docker/AppData/Local/QwenPawDevTools/perf13-ui"
    : "/tmp/qwenpaw-perf13-ui");

const browserTest = (name, fn) => browserUnavailable
  ? test(name, { skip: browserUnavailable }, fn)
  : test(name, fn);

before(async () => {
  if (browserUnavailable) return;
  await mkdir(screenshotDirectory, { recursive: true });
  ({ html } = await buildPanel('dev', { testHooks: true }));
  browser = await chromium.launch({ executablePath: chromiumExecutable, headless: true, args: ["--no-sandbox"] });
});

after(async () => { await browser?.close(); });

async function fixture(t, options = {}) {
  const state = {
    status: { success: true, installed: false, running: false, install: { running: false } },
    proxy: { success: true, port: 43126, public: false },
    proxyStatus: 200,
    versions: { success: true, latest_stable: "2.0.0", recommended_version: "2.0.0", versions: [
      { version: "2.1.0b1", is_beta: true }, { version: "2.0.0", is_beta: false }, { version: "1.9.0", is_beta: false },
    ] },
    install: { success: true, upgrading: false, running: false, seq: 0, instance: "worker-a", phase: "idle" },
    logs: ["原始服务日志"],
    installLogs: ["本任务日志"],
    clearStatus: 200,
    posts: [], logQueries: [], liveLogQueries: [], requestPaths: [], statusPolls: 0, statusRequests: 0,
    ...options,
  };
  const context = await browser.newContext({ locale: options.locale ?? "zh-CN", viewport: options.viewport, deviceScaleFactor: options.deviceScaleFactor, javaScriptEnabled: options.javaScriptEnabled, userAgent: options.userAgent });
  t.after(() => context.close());
  await context.addCookies([{ name: "qwenpaw_csrf", value: "test-token", url: origin }]);
  const page = await context.newPage();
  page.setDefaultTimeout(6000);
  if (options.initScript) await page.addInitScript(options.initScript, options.initScriptArgs);
  await page.route("**/*", async route => {
    const req = route.request();
    const url = new URL(req.url());
    if (url.pathname === prefix) return route.fulfill({ contentType: "text/html", body: html });
    if (url.pathname === `${prefix}web/`) return route.fulfill({ contentType: "text/html", body: "<html><body>QwenPaw Web</body></html>" });
    if (url.pathname === `${prefix}web/api/workspace/project-directory/dirs`) {
      if (req.method() === "GET") state.projectReads = (state.projectReads || 0) + 1;
      if (req.method() !== "GET") (state.projectWrites ??= []).push(req.method());
      if (req.method() === "PUT") state.projectDirs = req.postDataJSON().project_dirs;
      if (req.method() === "DELETE") state.projectDirs = [];
      return route.fulfill({ json: { project_dirs: state.projectDirs || [] } });
    }
    if (!url.pathname.startsWith(`${prefix}api/`)) return route.abort();
    const path = url.pathname.slice(`${prefix}api/`.length);
    state.requestPaths.push({ method: req.method(), path });
    if (req.method() === "POST") {
      state.posts.push({ path, body: path === 'kernel/admin-password' ? null : req.postDataJSON(), csrf: req.headers()["x-csrf-token"] });
      if (path === "kernel/install") {
        state.install = { success: true, upgrading: true, running: true, seq: 42, instance: "worker-a", phase: "installing" };
        return route.fulfill({ json: { success: true, seq: 42, instance: "worker-a" } });
      }
      if (path === 'stop' && state.controlHeld) await state.controlHeld;
      if (path === "kernel/admin-password") {
        if (state.resetHeld) await state.resetHeld;
        if (state.resetReply) return route.fulfill(state.resetReply);
      }
      if (path === "authorized-dirs/check") return route.fulfill({ json: { success: true, readable: true, writable: true } });
      if (path === "logs/clear") {
        if (state.clearStatus === 200) state.logs = [];
        return route.fulfill({ status: state.clearStatus, json: { success: state.clearStatus === 200, message: "删除失败" } });
      }
      if (path === "web-proxy/port") return route.fulfill({ json: { success: true, port: req.postDataJSON().port } });
      return route.fulfill({ json: { success: true } });
    }
    if (path === "kernel/versions" && state.versionsHeld) await state.versionsHeld;
    if (path === "status") {
      state.statusRequests++;
      const statusCode = state.statusCode || 200;
      const status = structuredClone(state.status);
      if (state.statusHeld) {
        const held = state.statusHeld;
        state.statusHeld = null;
        await held;
      }
      return route.fulfill({ status: statusCode, json: status });
    }
    if (path === "web-proxy/port") return route.fulfill({ status: state.proxyStatus, json: state.proxy });
    if (path === "kernel/versions") return route.fulfill({ json: state.versions });
    if (path === "kernel/install-status") { state.statusPolls++; return route.fulfill({ json: state.install }); }
    if (path === "kernel/install-log") {
      state.logQueries.push({ seq: url.searchParams.get("seq"), instance: url.searchParams.get("instance") });
      return route.fulfill({ json: { success: true, logs: state.installLogs } });
    }
    if (path === "logs") {
      state.liveLogQueries.push(Number(url.searchParams.get("tail")));
      const logs = structuredClone(state.logs);
      if (state.logsHeld) await state.logsHeld;
      return route.fulfill({ json: { success: true, logs } });
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

browserTest('homepage has one labeled version picker without expanding a detail view', async t => {
  const {page, state} = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix);
  const select = page.locator('#targetVersion');
  await expect(select).toHaveAttribute('aria-haspopup', 'listbox');
  await expect(select).toBeVisible();
  await expect(select).toBeDisabled();
  assert.equal(state.requestPaths.filter(r => r.path === 'kernel/versions').length, 0);
  await page.getByRole('button', {name: '检查更新', exact: true }).click();
  await select.click();
  const rows = page.getByRole('option');
  await expect(rows.locator('.history-version')).toHaveText(['v2.1.0b1', 'v2.0.0', 'v1.9.0']);
  await expect(rows.nth(0)).toContainText('测试版');
  await expect(rows.nth(1)).toContainText('正式版');
  await expect(rows.nth(1)).toContainText('随包适配补丁');
  await expect(rows.nth(1)).toContainText('当前安装');
  await expect(rows.nth(1)).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(select).toBeFocused();
  await select.click();
  await page.locator('#versionTitle').click();
  await expect(select).toHaveAttribute('aria-expanded', 'false');
  await pickVersion(page, '1.9.0');
  await expect(select).toBeFocused();
  assert.equal(state.posts.filter(r => r.path === 'kernel/install').length, 0);
  await expect(page.locator('#downgradeWarnBar')).toBeVisible();
  await expect(page).toHaveURL(/#overview$/);
});

browserTest('maintenance controls mount on first visit and keep the open recovery guide on return', async t => {
  const { page } = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator('#overviewPid')).toHaveText('421');
  assert.equal(await page.locator('#btnChangeServicePort, #authorizedDirsList').count(), 0);
  await page.locator('[data-section="authorization"]').click();
  await expect(page.locator('#authorizedDirsList')).toContainText('尚未绑定项目目录');
  assert.equal(await page.locator('#btnChangeServicePort').count(), 0);
  const guide = page.locator('#authorization details');
  await guide.locator('summary').click();
  await page.locator('[data-section="configuration"]').click();
  await expect(page.locator('#valPort')).toHaveText('18088');
  await page.goBack();
  await expect(page.locator('#authorization')).toBeVisible();
  await expect(guide).toHaveJSProperty('open', true);
});

browserTest('checking install protection submits one protected job after confirmation', async t => {
  const {page, state} = await fixture(t);
  await page.goto(origin + prefix);
  const backup = page.getByLabel('安装前备份已有数据');
  await expect(backup).not.toBeChecked(); await backup.check();
  await openInstall(page);
  assert.deepEqual(state.posts.filter(p => p.path === 'kernel/install').map(p => p.body), [{version:'2.0.0',mirror:'',backup:true}]);
});

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

browserTest('fnOS console title follows the current section and locale', async t => {
  const { page } = await fixture(t, {
    userAgent: 'Mozilla/5.0 FNAppType/Android FNAppVer/1.38.2',
    viewport: { width: 390, height: 844 },
  });
  page.on('pageerror', error => console.error('title regression pageerror:', error.message));
  await page.goto(origin + prefix);
  await expect(page).toHaveTitle('概览 · QwenPaw');
  await page.locator('[data-section="configuration"]').click();
  await expect(page).toHaveTitle('配置 · QwenPaw');
  await page.locator('#localeToggleBtn').click();
  await expect(page).toHaveTitle('Settings · QwenPaw');
  await page.locator('[data-section="authorization"]').click();
  await expect(page).toHaveTitle('Access & Backup · QwenPaw');
  await page.locator('[data-section="logs"]').click();
  await expect(page).toHaveTitle('Logs · QwenPaw');
});

const runningStatus = {
  success: true, installed: true, running: true, pid: 421,
  startAt: Math.floor(Date.now() / 1000) - 3600, cpu: 1.5, memKB: 102400,
  version: "2.0.0", port: 18088, authEnabled: true, authStatus: "protected", install: { running: false },
};

function installStatusEventHost() {
  window.testHidden = false;
  Object.defineProperty(document, "hidden", { configurable: true, get: () => window.testHidden });
  window.setTestHidden = hidden => {
    window.testHidden = hidden;
    document.dispatchEvent(new Event("visibilitychange"));
  };
  const originalSetInterval = window.setInterval;
  const originalClearInterval = window.clearInterval;
  window.intervalCallbacks = new Map();
  window.setInterval = (callback, delay, ...args) => {
    const id = originalSetInterval(callback, delay, ...args);
    window.intervalCallbacks.set(id, { callback, delay });
    return id;
  };
  window.clearInterval = id => {
    window.intervalCallbacks.delete(id);
    originalClearInterval(id);
  };
  window.fireInterval = delay => {
    for (const timer of window.intervalCallbacks.values()) {
      if (timer.delay === delay) timer.callback();
    }
  };
  class FakeEventSource {
    static instances = [];
    listeners = new Map();
    constructor(url) { this.url = url; FakeEventSource.instances.push(this); }
    addEventListener(type, listener) { this.listeners.set(type, listener); }
    emit(type, data) { this.listeners.get(type)?.({ data }); }
    close() { this.closed = true; }
  }
  window.EventSource = FakeEventSource;
}

async function emitSnapshot(page, snapshot, index = 0) {
  await page.evaluate(({ snapshot, index }) => {
    window.EventSource.instances[index].emit("snapshot", JSON.stringify(snapshot));
  }, { snapshot, index });
}

browserTest("panel: mobile overview reads no logs and opens full history on demand", async t => {
  const { page, state } = await fixture(t, { viewport: { width: 390, height: 844 }, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#overviewLogPreview")).toHaveCount(0);
  assert.deepEqual(state.liveLogQueries, []);
  await expect(page.locator("#logTerminal .log-chunk")).toHaveCount(0);
  await page.locator("#mobileSectionNav [href='#logs']").click();
  await expect(page.locator("#logTerminal")).toContainText("原始服务日志");
  assert.deepEqual(state.liveLogQueries, [2000]);
  await page.locator("#mobileSectionNav [href='#configuration']").click();
  await page.evaluate(() => window.fetchLogs());
  assert.deepEqual(state.liveLogQueries, [2000]);
});

browserTest("panel: missing EventSource keeps navigation and fallback status updates", async t => {
  const { page, state } = await fixture(t, { viewport: { width: 390, height: 844 }, status: runningStatus, initScript: () => { delete window.EventSource; } });
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(origin + prefix);
  await expect(page.locator("html")).toHaveAttribute("data-mobile-section", "overview");
  state.status = { ...runningStatus, running: false };
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("已停止");
  assert.deepEqual(errors, []);
});

browserTest("panel: recovery stays visible and blocks new lifecycle actions until cleared", async t => {
  const { page, state } = await fixture(t, { status: { ...runningStatus, running: false, install: { running: false, recovery_required: true, recovery_error: "无法核验进程身份" } } });
  await page.goto(origin + prefix);
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("需要恢复");
  await expect(page.locator("#statusCardMount")).toContainText("无法核验进程身份");
  await expect(page.locator("#btnStartService")).not.toBeVisible();
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  state.status = { ...runningStatus, running: false };
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator("#btnStartService")).toBeVisible();
  await page.getByRole('button', {name:'检查更新', exact: true }).click();
  await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
});

browserTest("panel: destructive confirmation defaults to cancel", async t => {
  const { page, state } = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix + "#version");
  await page.locator("#btnUninstallKernel").click();
  await expect(page.locator("#modalTitle")).toHaveText("仅卸载 QwenPaw 内核");
  await expect(page.locator("#modalCancelBtn")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("#btnUninstallKernel")).toBeFocused();
  assert.deepEqual(state.posts, []);
});

browserTest("revealed notices keep the icon beside the message", async t => {
  const { page } = await fixture(t, { viewport: desktop, versions: { success: false, message: "mirror down" } });
  await page.goto(origin + prefix + "#version");
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
  await page.goto(origin + prefix + "#version");
  await expect(page.locator("#btnUninstallKernel")).toBeVisible();
  const uninstallDisplay = await page.locator("#btnUninstallKernel").evaluate(el => getComputedStyle(el).display);
  assert.equal(uninstallDisplay, await page.locator("#btnUpgradeAction").evaluate(el => getComputedStyle(el).display));
  assert.notEqual(uninstallDisplay, "block");
  await expect(page.locator("#targetVersion")).toContainText("2.0.0");
  await expect(page.locator("#downgradeWarnBar")).toBeHidden();
  await page.evaluate(() => window.pickVersion("1.9.0"));
  await expect(page.locator("#btnUpgradeAction")).toHaveText("降级至 v1.9.0");
  await expect(page.locator("#downgradeWarnBar")).toBeVisible();
  await expect(page.locator("#downgradeWarnBar")).toHaveCSS("display", "flex");
  await page.evaluate(() => window.pickVersion("2.0.0"));
  await expect(page.locator("#downgradeWarnBar")).toBeHidden();
});

browserTest("empty version responses never create selectable ghost cards or pin the selection", async t => {
  const { page, state } = await fixture(t, {
    viewport: desktop,
    versions: { success: true, latest_stable: "", versions: [] },
  });
  await page.goto(origin + prefix + "#version");
  await expect(page.locator("#targetVersion")).toBeDisabled();
  await expect(page.getByRole("option")).toHaveCount(0);
  await expect(page.locator('#versionSourceStatus')).toContainText('当前下载源没有可选版本');
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  await page.evaluate(() => window.pickVersion(""));
  state.versions = { success: true, latest_stable: "2.0.0", recommended_version: "2.0.0", versions: [{ version: "2.0.0", is_beta: false }] };
  await page.evaluate(() => window.checkOnlineVersions());
  await expect(page.locator("#targetVersion")).toContainText("2.0.0");
  await page.locator("#targetVersion").click();
  await expect(page.getByRole("option", {selected:true})).toContainText("2.0.0");
  await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
});

browserTest("without a listed recommendation both install actions require an explicit pick", async t => {
  const { page, state } = await fixture(t, { viewport: desktop, versions: {
    success: true, latest_stable: "9.0.0", recommended_version: "8.0.0", versions: [{ version: "9.0.0", is_beta: false }],
  } });
  await page.goto(origin + prefix + "#version");
  await expect(page.locator("#versionSourceStatus")).toContainText("当前下载源未列出本安装包带有适配补丁的版本");
  await expect(page.locator("#overviewInstallButton")).toBeDisabled();
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  await pickVersion(page, "9.0.0");
  await page.locator("#btnUpgradeAction").click();
  await expect(page.locator("#customModal")).toContainText("本安装包没有此版本的适配补丁");
  assert.equal(state.posts.filter(x => x.path === "kernel/install").length, 0);
});

browserTest("a version refresh without beta removes the stale beta card", async t => {
  const { page, state } = await fixture(t, { viewport: desktop });
  await page.goto(origin + prefix + "#version");
  await page.locator("#targetVersion").click();
  await expect(page.getByRole("listbox")).toContainText("2.1.0b1");
  state.versions = { success: true, latest_stable: "2.0.0", recommended_version: "2.0.0", versions: [{ version: "2.0.0", is_beta: false }] };
  await page.evaluate(() => window.checkOnlineVersions());
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await page.locator("#targetVersion").click();
  await expect(page.getByRole("listbox")).not.toContainText("2.1.0b1");
  await expect(page.getByRole("option")).toHaveCount(1);
});

browserTest("an App user opens Web in a clean full-screen overlay and returns via host back", async t => {
  const { page } = await fixture(t, { userAgent: 'Mobile FNAppType/iOS', status: runningStatus });
  await page.goto(origin + prefix + '#overview');
  await page.locator('#btnWebConsole').click();
  await expect(page.locator('#webEntryOverlay')).toBeVisible();
  await expect(page.locator('#webEntryBack')).toHaveCount(0);
  await page.goBack();
  await expect(page.locator('#webEntryOverlay')).toBeHidden();
  await expect(page.locator('#btnWebConsole')).toBeFocused();
  assert.equal(new URL(page.url()).hash, '#overview');
});

browserTest("a failed refresh keeps the last version list visible without permitting a stale install", async t => {
  const { page, state } = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix + '#version');
  await expect(page.locator('#targetVersion')).toContainText('2.0.0');
  state.versions = { success: false, message: '镜像未响应' };
  await page.getByRole('button', { name: '检查更新', exact: true }).click();
  await expect(page.locator('#versionSourceStatus')).toContainText('镜像未响应');
  await expect(page.locator('#targetVersion')).toContainText('2.0.0');
  await expect(page.locator('#btnUpgradeAction')).toBeDisabled();
});

browserTest("DSH overview keeps service and deployment cards together in four existing sections", async t => {
  for (const width of [390, 1440]) {
    const { page, state } = await fixture(t, { viewport: {width, height: 844}, status: runningStatus });
    await page.goto(origin + prefix);
    const nav = page.getByRole("navigation", {name: "控制台分区"});
    await expect(nav.getByRole("link")).toHaveText(["概览", "配置", "授权与备份", "日志"]);
    await expect(page.locator("#version")).toBeVisible();
    const service = await box(page.locator("#overview"));
    const version = await box(page.locator("#version"));
    const rail = await box(nav);
    if (width > 768) {
      assert.ok(rail.x + rail.width <= service.x && Math.abs(service.y - version.y) <= 1);
      assert.ok(version.x >= service.x + service.width);
    } else {
      assert.equal(rail.x, 0);
      assert.equal(rail.width, width);
      assert.equal(rail.y + rail.height, 844);
      assert.ok(version.y >= service.y + service.height);
      for (const link of await nav.getByRole("link").all()) {
        const icon = await box(link.locator("svg"));
        const label = await box(link.locator("span"));
        assert.ok(icon.y + icon.height <= label.y);
      }
    }
    assert.ok(!state.requestPaths.some(({path}) => path === "kernel/versions"));
    await page.getByRole('button', {name:'检查更新', exact: true }).click();
    await expect(nav.locator('[aria-current="page"]')).toHaveText("概览");
    await expect(page.locator("#overview")).toBeVisible();
    await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
    await page.goto(origin + prefix + "#plugins");
    await expect(nav.locator('[aria-current="page"]')).toHaveText("概览");
    await expect(page).toHaveURL(/#overview$/);
  }
});

browserTest("overview keeps the picker visible when returning from other sections", async t => {
  const { page } = await fixture(t, { viewport: desktop, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator('#btnUpgradeAction')).toBeVisible();
  await page.locator('[data-section="logs"]').click();
  await expect(page.locator('#targetVersion')).toBeHidden();
  await page.goBack();
  await expect(page.locator('#targetVersion')).toBeVisible();
});

browserTest("keyboard browsing commits only after Enter and returns focus", async t => {
  const { page } = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix + '#version');
  const select = page.locator('#targetVersion');
  await expect(select).toContainText('2.0.0');
  await select.press('Enter');
  await page.keyboard.press('ArrowUp');
  await expect(page.getByRole('option').first()).toBeFocused();
  await expect(select).toContainText('2.0.0');
  await page.keyboard.press('Enter');
  await expect(select).toContainText('2.1.0b1');
  await expect(select).toBeFocused();
  await expect(page.locator('#btnUpgradeAction')).toContainText('2.1.0b1');
});

browserTest("healthy overview defers all unrelated maintenance reads on desktop and mobile", async t => {
  for (const width of [390, 1440]) {
    const {page, state} = await fixture(t, {viewport: {width, height: 900}, status: runningStatus});
    await page.goto(origin + prefix);
    await expect(page.locator("#version #valVersion")).toHaveText("2.0.0");
    await expect(page.locator("#mobileSectionNav")).toBeVisible();
    assert.deepEqual(state.liveLogQueries, []);
    await page.screenshot({path: join(screenshotDirectory, `overview-${width}.png`)});
    assert.equal(state.projectReads || 0, 0);
    assert.ok(!state.requestPaths.some(({path}) => ["kernel/versions", "mirror", "web-proxy/port"].includes(path)));
    await page.locator('[data-section="authorization"]').click();
    await expect(page.locator("#authorizedDirsList")).toContainText("尚未绑定项目目录");
    assert.equal(state.projectReads, 1);
    await page.screenshot({path: join(screenshotDirectory, `authorization-${width}.png`)});
    await page.locator('[data-section="configuration"]').click();
    await expect(page.locator("#valWebProxyPort")).toHaveText("43126");
    assert.equal(state.requestPaths.filter(({path}) => path === "web-proxy/port").length, 1);
    await page.screenshot({path: join(screenshotDirectory, `configuration-${width}.png`)});
    await page.locator('[data-section="overview"]').click();
    await page.getByRole('button', {name:'检查更新', exact: true }).click();
    await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
    await page.goBack(); await page.goForward();
    await expect(page.locator("#version")).toBeVisible();
    assert.equal(state.requestPaths.filter(({path}) => path === "kernel/versions").length, 1);
  }
});

browserTest("overview attaches to an existing install without fetching versions or posting an install", async t => {
  const {page, state} = await fixture(t, {
    status: {success: true, installed: false, running: false, install: {running: true}},
    install: {success: true, upgrading: true, seq: 7, instance: "other-device", phase: "installing"},
  });
  await page.goto(origin + prefix);
  await expect(page.locator("#overviewInstaller")).toBeHidden();
  await page.locator("#overviewRunningInstall").click();
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
  await expect.poll(() => state.logQueries).toContainEqual({seq: "7", instance: "other-device"});
  assert.equal(state.posts.length, 0);
  assert.ok(!state.requestPaths.some(({path}) => path === "kernel/versions"));
});

browserTest("mobile running overview presents a labelled conversation action and compact metrics", async t => {
  const { page } = await fixture(t, { viewport: { width: 360, height: 800 }, status: runningStatus });
  await page.goto(origin + prefix);
  const headerEntry = page.locator("#btnWebConsole");
  const entry = page.getByRole("link", { name: "进入 Web 对话" });
  await expect(entry).toHaveCount(1);
  await expect(entry).toBeVisible();
  await expect(headerEntry.locator("#webConsoleText")).toHaveText("进入 Web 对话");
  await expect(headerEntry.locator("#webConsoleMobileText")).toHaveText("对话");
  await expect(headerEntry.locator("#webConsoleMobileText")).toBeVisible();
  await expect(headerEntry).toHaveAttribute("aria-label", "进入 Web 对话");
  const header = await box(page.locator("header"));
  assert.ok(header.height <= 90, `header height ${header.height}`);
  assert.equal(await page.locator("header").evaluate(element => getComputedStyle(element).position), "static");
  assert.equal(await page.locator("#mobileSectionNav").evaluate(element => getComputedStyle(element).position), "fixed");
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await expect(page.locator("#overviewTitle")).toHaveText("QwenPaw");
  await expect(page.locator("#versionTitle")).toHaveText("部署版本");
  await expect(page.locator("#statusCardMount .stat")).toHaveCount(4);
  await expect(page.locator("#statusCardMount .stat-label")).toHaveText(["CPU 使用率", "内存占用", "进程 PID", "运行时间"]);
  await expect(page.locator(".overview-details")).toBeHidden();
  await expect(page.locator(".overview-log-preview")).toHaveCount(0);
  const stop = await box(page.locator("#btnStopService"));
  const restart = await box(page.locator("#btnRestartService"));
  assert.ok(Math.abs(stop.y - restart.y) <= 1, "stop and restart share the primary action row");
  const title = await page.locator(".brand-info h1").evaluate(element => ({
    scroll: element.scrollWidth,
    client: element.clientWidth,
  }));
  assert.ok(title.scroll <= title.client, `brand title overflows its container: ${title.scroll}/${title.client}`);
  const popupPromise = page.waitForEvent("popup");
  const requestPromise = page.context().waitForEvent("request", {
    predicate: request => request.url() === `${origin}/app/qwenpaw/web/`,
  });
  await entry.click();
  const [popup, request] = await Promise.all([popupPromise, requestPromise]);
  assert.ok(popup);
  assert.equal(request.url(), `${origin}/app/qwenpaw/web/`);
  await popup.close();
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

browserTest('stop waits for confirmation from the service, prevents duplicate clicks and refreshes immediately', async t => {
  let release;
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost,
    controlHeld: new Promise(resolve => { release = resolve; }) });
  await page.goto(origin + prefix);
  const stop = page.locator('#btnStopService');
  await stop.click();
  await page.locator('#modalConfirmBtn').click();
  try {
    await expect(stop).toBeDisabled();
    await expect(page.locator('#statusCheckedAt')).toContainText('正在停止服务');
    await expect(page.locator('#statusCardMount .status-text')).toHaveText('运行中');
    await stop.evaluate(el => el.click());
    assert.deepEqual(state.posts.map(p => p.path), ['stop']);
    state.status = { ...runningStatus, running: false, pid: '0', startAt: 0 };
  } finally { release(); }
  await expect(page.locator('#btnStartService')).toBeEnabled({ timeout: 1000 });
  await expect(page.locator('#statusCardMount .status-text')).toHaveText('已停止', { timeout: 1000 });
});

browserTest("running uptime ticks locally without another status request", async t => {
  const { page, state } = await fixture(t, {
    status: { ...runningStatus, startAt: Math.floor(Date.now() / 1000) - 3665 },
    initScript: installStatusEventHost,
  });
  await page.goto(origin + prefix);
  const uptime = page.locator("#statusUptime");
  await expect(uptime).toContainText("已运行");
  const metric = page.locator('#statusCardMount .stat').filter({hasText:'运行时间'}).locator('.stat-value');
  const initial = await metric.innerText();
  const requests = state.statusRequests;
  await expect.poll(() => metric.innerText(), { timeout: 3000 }).not.toBe(initial);
  await expect(uptime).toContainText(await metric.innerText());
  assert.equal(state.statusRequests, requests, "local ticking must not add a network request");
});

browserTest("mobile log terminal fills the page and preserves manual scrolling", async t => {
  const { page, state } = await fixture(t, {
    viewport: { width: 390, height: 844 },
    logs: Array.from({ length: 160 }, (_, index) => `日志 ${index + 1}`),
  });
  await page.goto(origin + prefix);
  await page.locator('#mobileSectionNav [data-section="logs"]').click();
  const terminal = page.locator("#logTerminal");
  await expect(terminal).toContainText("日志 160");
  await expect(page.locator(".terminal-footer")).toHaveCount(0);
  await expect(page.locator('#logTools')).not.toHaveAttribute('open', '');
  await expect(page.locator('#exportLogsBtn')).toBeVisible();
  await expect(page.locator('#autoScrollCheck')).toBeVisible();
  await expect(terminal).toHaveCSS('background-color', 'rgb(252, 251, 248)');
  const metrics = await terminal.evaluate(element => ({
    client: element.clientHeight,
    scroll: element.scrollHeight,
    top: element.scrollTop,
  }));
  assert.ok(metrics.client > 420, `terminal should use the available mobile space, got ${metrics.client}px`);
  assert.ok(metrics.scroll > metrics.client, "all log lines share one scrolling window");
  assert.ok(metrics.top + metrics.client >= metrics.scroll - 2, `latest logs start in view: ${JSON.stringify(metrics)}`);
  await openLogTools(page);
  await expect(page.getByLabel("关键字筛选")).toHaveAttribute("name", "log-query");
  const targetHeights = await page.locator(".terminal-controls .btn-sm, .log-level-btn, .log-search-input").evaluateAll(elements => elements.map(element => element.getBoundingClientRect().height));
  assert.ok(targetHeights.every(height => height + 0.01 >= 44), `mobile controls need 44px targets: ${targetHeights}`);

  await terminal.evaluate(element => { element.scrollTop = 0; });
  state.logs = [...state.logs, "日志 161"];
  await page.evaluate(() => window.fetchLogs());
  await expect(terminal).toContainText("日志 161");
  assert.equal(await terminal.evaluate(element => element.scrollTop), 0, "reading older logs must not jump to the bottom");
  await page.locator('#logTools summary').click();
  await page.screenshot({ path: join(screenshotDirectory, "qwenpaw-mobile-log-final.png"), fullPage: true });
});

browserTest("unchanged log windows preserve DOM nodes, selection and manual scroll without mutations", async t => {
  const logs = Array.from({ length: 160 }, (_, index) => index % 2 ? `日志 ${index + 1}` : { message: `日志 ${index + 1}` });
  const { page, state } = await fixture(t, { viewport: desktop, logs });
  await page.goto(origin + prefix + "#logs");
  const terminal = page.locator("#logTerminal");
  await expect(terminal).toContainText("日志 160");
  await terminal.evaluate(element => {
    element.scrollTop = 0;
    const firstLine = element.querySelector(".log-chunk > div");
    window.originalLogNode = firstLine;
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(firstLine);
    selection.removeAllRanges();
    selection.addRange(range);
    window.logMutations = [];
    window.logObserver = new MutationObserver(records => window.logMutations.push(...records.map(record => record.type)));
    window.logObserver.observe(element, { subtree: true, childList: true, characterData: true });
  });
  state.logs = structuredClone(logs.slice(-100));
  await page.evaluate(async () => { for (let index = 0; index < 3; index++) await window.fetchLogs(); });
  const unchanged = await terminal.evaluate(element => ({
    sameNode: element.querySelector(".log-chunk > div") === window.originalLogNode,
    selection: window.getSelection().toString(), top: element.scrollTop,
    mutations: window.logMutations,
  }));
  assert.equal(unchanged.sameNode, true);
  assert.equal(unchanged.selection, "日志 1");
  assert.equal(unchanged.top, 0);
  assert.deepEqual(unchanged.mutations, []);
  assert.deepEqual(state.liveLogQueries, [2000, 100, 100, 100]);

  state.logs = [...logs.slice(-99), { message: "日志 161" }];
  await page.evaluate(() => window.fetchLogs());
  await expect(terminal).toContainText("日志 161");
  assert.equal(await terminal.evaluate(element => element.scrollTop), 0);
  await terminal.evaluate(element => { element.scrollTop = element.scrollHeight; });
  state.logs = [...logs.slice(-98), { message: "日志 161" }, "日志 162"];
  await page.evaluate(() => window.fetchLogs());
  await expect(terminal).toContainText("日志 162");
  assert.equal(await terminal.evaluate(element => element.scrollHeight - element.clientHeight - element.scrollTop <= 2), true);
});

browserTest("log windows remain bounded and rotation replaces unrelated history", async t => {
  const logs = Array.from({ length: 2000 }, (_, index) => `行 ${index + 1}`);
  const { page, state } = await fixture(t, { viewport: desktop, logs });
  await page.goto(origin + prefix + "#logs");
  const terminal = page.locator("#logTerminal");
  await expect(terminal.locator(".log-chunk > div")).toHaveCount(2000);
  state.logs = [...logs.slice(-99), "行 2001"];
  await page.evaluate(() => window.fetchLogs());
  await expect(terminal.locator(".log-chunk > div")).toHaveCount(2000);
  await expect(terminal.locator(".log-chunk > div").first()).toHaveText("行 2");
  await expect(terminal.locator(".log-chunk > div").last()).toHaveText("行 2001");
  state.logs = ["轮转后的日志"];
  await page.evaluate(() => window.fetchLogs());
  await expect(terminal.locator(".log-chunk > div")).toHaveCount(1);
  await expect(terminal).toHaveText("轮转后的日志");
});

browserTest("offscreen log chunks skip rendering work without discarding history", async t => {
  const logs = Array.from({ length: 2000 }, (_, index) => `行 ${index + 1}`);
  const { page } = await fixture(t, { viewport: desktop, logs });
  await page.goto(origin + prefix + "#logs");
  const terminal = page.locator("#logTerminal");
  await expect(terminal.locator(".log-chunk")).toHaveCount(40);
  await expect(terminal.locator(".log-chunk > div")).toHaveCount(2000);
  await expect(terminal.locator(".log-chunk > div").last()).toHaveText("行 2000");
  const rendering = await terminal.locator(".log-chunk").first().evaluate(element => {
    const style = getComputedStyle(element);
    return { contentVisibility: style.contentVisibility, intrinsicSize: style.containIntrinsicSize };
  });
  assert.equal(rendering.contentVisibility, "auto");
  assert.match(rendering.intrinsicSize, /auto/);
  await expect(terminal.locator('.log-chunk').last()).toHaveCSS('content-visibility', 'visible');
});

browserTest("log clear discards held pre-clear responses, preserves failed clears and fetches a fresh window", async t => {
  const { page, state } = await fixture(t, { viewport: desktop });
  await page.goto(origin + prefix + "#logs");
  const terminal = page.locator("#logTerminal");
  await expect(terminal).toContainText("原始服务日志");
  state.clearStatus = 503;
  await page.evaluate(() => { window.pendingClear = window.clearTerminalLogs(); });
  await page.locator("#modalConfirmBtn").click();
  await page.evaluate(() => window.pendingClear);
  await expect(terminal).toContainText("原始服务日志");
  await expect(page.locator("#toast")).toContainText("删除日志失败");
  state.clearStatus = 200;
  let release;
  state.logs = ["清空前的慢响应"];
  state.logsHeld = new Promise(resolve => { release = resolve; });
  t.after(() => release());
  const requests = state.liveLogQueries.length;
  await page.evaluate(() => { window.pendingLogs = window.fetchLogs(); });
  await expect.poll(() => state.liveLogQueries.length).toBe(requests + 1);
  await page.evaluate(() => { window.pendingClear = window.clearTerminalLogs(); });
  await page.locator("#modalConfirmBtn").click();
  await page.evaluate(() => window.pendingClear);
  await expect(terminal).toContainText("当前无日志输出");
  release();
  state.logsHeld = null;
  await page.evaluate(() => window.pendingLogs);
  await expect(terminal).not.toContainText("清空前的慢响应");
  await expect(page.locator("#overviewLogPreview")).toHaveCount(0);
  state.logs = ["清空后的新日志"];
  await page.evaluate(() => window.fetchLogs());
  await expect(terminal).toHaveText("清空后的新日志");
  assert.equal(state.liveLogQueries.at(-1), 2000);
});

browserTest("slow log polling coalesces ticks and foreground recovery without parallel reads", async t => {
  const { page, state } = await fixture(t, { viewport: desktop, initScript: installStatusEventHost });
  await page.goto(origin + prefix + "#logs");
  await expect(page.locator("#logTerminal")).toContainText("原始服务日志");
  let release;
  state.logsHeld = new Promise(resolve => { release = resolve; });
  t.after(() => release());
  const requests = state.liveLogQueries.length;
  await page.evaluate(() => { for (let index = 0; index < 20; index++) window.fireInterval(15000); });
  await expect.poll(() => state.liveLogQueries.length).toBe(requests + 1);
  await page.evaluate(() => {
    window.setTestHidden(true);
    window.setTestHidden(false);
    for (let index = 0; index < 20; index++) window.fireInterval(15000);
  });
  assert.equal(state.liveLogQueries.length, requests + 1);
  state.logs = ["恢复后新日志"];
  state.logsHeld = null;
  release();
  await expect(page.locator("#logTerminal")).toHaveText("恢复后新日志");
  assert.equal(state.liveLogQueries.length, requests + 2, "all pending ticks collapse to one follow-up read");
});

browserTest("log export downloads the complete visible history window", async t => {
  const { page } = await fixture(t, {
    viewport: { width: 390, height: 844 },
    logs: Array.from({ length: 160 }, (_, index) => `导出日志 ${index + 1}`),
  });
  await page.goto(origin + prefix);
  await page.locator('#mobileSectionNav [data-section="logs"]').click();
  const pending = page.waitForEvent("download");
  await page.locator("#exportLogsBtn").click();
  const download = await pending;
  assert.match(download.suggestedFilename(), /^qwenpaw-.*\.log$/);
  const content = await readFile(await download.path(), "utf8");
  assert.match(content, /导出日志 1/);
  assert.match(content, /导出日志 160/);
});

browserTest("long logs scroll inside a bounded terminal that follows the newest line", async t => {
  const { page } = await fixture(t, { viewport: desktop, logs: Array.from({ length: 100 }, (_, index) => `日志 ${index + 1}`) });
  await page.goto(origin + prefix + "#logs");
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
    await page.goto(origin + prefix + (name === "检查更新" ? "#version" : "#configuration"));
    const bounds = await box(page.getByRole("button", { name, exact: true }));
    assert.ok(bounds.height >= 28, `${name} height ${bounds.height}`);
  }
  const archive = await box(page.locator("#targetVersion"));
  assert.ok(archive.height >= 28 && archive.width >= 64, `archive toggle ${archive.width}x${archive.height}`);
  for (const selector of ["#webProxyPublic", "#autoScrollCheck", "#autoRefreshCheck"]) {
    await page.goto(origin + prefix + (selector === "#webProxyPublic" ? "#configuration" : "#logs"));
    if (selector === "#autoRefreshCheck") await openLogTools(page);
    const bounds = await box(page.locator(selector));
    assert.ok(bounds.width >= 16 && bounds.height >= 16, `${selector} ${bounds.width}x${bounds.height}`);
  }
});

browserTest("theme toggle flips light/dark directly while settings selector restores system mode", async t => {
  const { page } = await fixture(t, { initScript: () => localStorage.setItem("qwenpaw_theme_v2", "dark") });
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto(origin + prefix + "#configuration");
  const toggle = page.locator("#themeToggleBtn");
  await expect(toggle).toHaveAttribute("aria-label", "夜晚模式，切换到白天模式");
  await expect(page.locator("#themeText")).toHaveText("夜晚");
  await toggle.click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator("html")).toHaveAttribute("data-theme-mode", "light");
  await expect(page.locator("#themeText")).toHaveText("白天");
  await expect(page.locator("#themeIconSun")).toBeVisible();
  await expect(toggle).toHaveAttribute("aria-label", "白天模式，切换到夜晚模式");
  await toggle.click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme-mode", "dark");
  await expect(page.locator("#themeText")).toHaveText("夜晚");
  await expect(page.locator("#themeIconMoon")).toBeVisible();

  await page.locator('#themeModeGroup [data-theme-option="system"]').click();
  await expect(page.locator("html")).toHaveAttribute("data-theme-mode", "system");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator("#themeText")).toHaveText("白天");
  await expect(page.locator("#themeIconSun")).toBeVisible();
  assert.equal(await page.evaluate(() => localStorage.getItem("qwenpaw_theme_v2")), null);
  await page.emulateMedia({ colorScheme: "dark" });
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.locator("#themeText")).toHaveText("夜晚");
  await expect(toggle).toHaveAttribute("aria-label", "夜晚模式，切换到白天模式");
});

browserTest("theme: unavailable matchMedia does not prevent the dashboard mounting", async t => {
  const { page } = await fixture(t, { status: runningStatus, initScript: () => { delete window.matchMedia; } });
  await page.goto(origin + prefix);
  await expect(page.locator('#statusBadge')).toContainText('运行中');
  await page.locator('#themeToggleBtn').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('#themeIconMoon')).toBeVisible();
});

browserTest("theme: a manual choice survives system changes when storage is blocked", async t => {
  const { page } = await fixture(t, { initScript: () => {
    Object.defineProperty(window, 'localStorage', { get() { throw new DOMException('Blocked', 'SecurityError'); } });
    window.systemThemeEvents = 0;
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => window.systemThemeEvents++);
  } });
  await page.emulateMedia({colorScheme:'light'});
  await page.goto(origin + prefix);
  await page.locator('#themeToggleBtn').click();
  await expect(page.locator('#themeText')).toHaveText('夜晚');
  await page.emulateMedia({colorScheme:'dark'});
  await page.waitForFunction(() => window.systemThemeEvents === 1);
  await page.emulateMedia({colorScheme:'light'});
  await page.waitForFunction(() => window.systemThemeEvents === 2);
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('#themeToggleBtn')).toHaveAccessibleName('夜晚模式，切换到白天模式');
});

browserTest("theme: changing or clearing the preference in another tab updates the open panel", async t => {
  const { page } = await fixture(t);
  await page.emulateMedia({ colorScheme: 'light' });
  await page.goto(origin + prefix);
  await expect(page.locator('#themeIconSun')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'system');
  const other = await page.context().newPage();
  await other.route(origin + prefix, route => route.fulfill({ contentType: 'text/html', body: '<title>Settings</title>' }));
  await other.goto(origin + prefix);
  await other.evaluate(() => localStorage.setItem('qwenpaw_theme_v2', 'dark'));
  await expect(page.locator('#themeIconMoon')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await other.evaluate(() => localStorage.removeItem('qwenpaw_theme_v2'));
  await expect(page.locator('#themeIconSun')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'system');
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect(page.locator('#themeIconMoon')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  assert.equal(await page.evaluate(() => localStorage.getItem('qwenpaw_theme_v2')), null);
});

browserTest("theme: uiStyle switches between default and Miuix across light/dark and syncs across tabs", async t => {
  const { page } = await fixture(t, { status: runningStatus });
  await page.emulateMedia({ colorScheme: 'light' });
  await page.goto(origin + prefix + '#configuration');
  await expect(page.locator('html')).toHaveAttribute('data-ui-style', 'default');
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content', '#f2f1ed');
  await expect(page.locator('#uiStyleGroup [data-ui-style-option="default"]')).toHaveAttribute('aria-pressed', 'true');

  await page.locator('#uiStyleGroup [data-ui-style-option="miuix"]').click();
  await expect(page.locator('html')).toHaveAttribute('data-ui-style', 'miuix');
  await expect(page.locator('#uiStyleGroup [data-ui-style-option="miuix"]')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content', '#f4f3f0');
  assert.equal(await page.evaluate(() => localStorage.getItem('qwenpaw_ui_style_v1')), 'miuix');

  await page.locator('#themeToggleBtn').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('html')).toHaveAttribute('data-ui-style', 'miuix');
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content', '#131315');

  const other = await page.context().newPage();
  await other.route(origin + prefix, route => route.fulfill({ contentType: 'text/html', body: '<title>Sync</title>' }));
  await other.goto(origin + prefix);
  await other.evaluate(() => localStorage.removeItem('qwenpaw_ui_style_v1'));
  await expect(page.locator('html')).toHaveAttribute('data-ui-style', 'default');
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content', '#141414');
});

browserTest("i18n: follows browser English locale, toggles languages live, and syncs across tabs", async t => {
  const { page } = await fixture(t, { locale: "en-US", status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page).toHaveTitle("Overview · QwenPaw");
  await expect(page.locator("#sectionTitle")).toHaveText("Overview");
  await expect(page.locator("#statusBadge")).toContainText("Running");
  await expect(page.locator("#localeText")).toHaveText("EN");
  await expect(page.locator("#themeText")).toHaveText("Light");

  await page.locator("#localeToggleBtn").click();
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  await expect(page).toHaveTitle("概览 · QwenPaw");
  await expect(page.locator("#sectionTitle")).toHaveText("概览");
  await expect(page.locator("#statusBadge")).toContainText("运行中");
  await expect(page.locator("#localeText")).toHaveText("中文");
  await expect(page.locator("#themeText")).toHaveText("白天");

  const other = await page.context().newPage();
  await other.route(origin + prefix, route => route.fulfill({ contentType: "text/html", body: "<title>Sync</title>" }));
  await other.goto(origin + prefix);
  await other.evaluate(() => localStorage.setItem("qwenpaw_locale_v1", "en"));
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.locator("#sectionTitle")).toHaveText("Overview");
});

browserTest("i18n: English mobile header fits all sections without overlap or overflow at 360px", async t => {
  const { page } = await fixture(t, { locale: "en-US", viewport: { width: 360, height: 844 }, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#localeToggleBtn .locale-short")).toBeVisible();
  await expect(page.locator("#localeToggleBtn .locale-short")).toHaveText("EN");
  const controlSelectors = ["#localeToggleBtn", "#themeToggleBtn", "#aboutButton", "#btnWebConsole"];
  const intersects = (a, b) => a.x < b.x + b.width && a.x + a.width > b.x
    && a.y < b.y + b.height && a.y + a.height > b.y;
  for (const section of ["overview", "configuration", "authorization", "logs"]) {
    await page.locator(`#mobileSectionNav a[data-section="${section}"]`).click();
    const header = await box(page.locator("header"));
    const title = await box(page.locator(".brand-info h1"));
    const controls = await Promise.all(controlSelectors.map(selector => box(page.locator(selector))));
    for (const [index, control] of controls.entries()) {
      assert.equal(intersects(title, control), false, `${section}: ${controlSelectors[index]} overlaps title`);
      assert.ok(control.width >= 44 && control.height >= 44, `${section}: ${controlSelectors[index]} < 44px`);
    }
    assert.ok(header.height <= 90, `${section}: header wrapped (${header.height}px)`);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${section}: horizontal overflow`);
  }
});

browserTest("i18n: live locale switch updates transient error banners and active install dialog", async t => {
  const { page } = await fixture(t, {
    status: { ...runningStatus, install: { running: true } },
    install: { success: true, upgrading: true, running: true, seq: 42, instance: "worker-a", phase: "downloading" },
    installLogs: [],
    versions: { success: false, message: "mirror timeout" },
  });
  await page.goto(origin + prefix);
  await page.getByRole("button", { name: "检查更新", exact: true }).click();
  await expect(page.locator("#versionSourceStatus")).toContainText("当前下载源不可用");
  await page.locator("#localeToggleBtn").click();
  await expect(page.locator("#versionSourceStatus")).toContainText("The current download source is unavailable");

  await page.locator("#installRunningTip").click();
  await expect(page.locator("#upTitle")).toHaveText("Viewing active install task…");
  await expect(page.locator("#upTerminal")).toContainText("Downloading dependencies…");
  await page.locator("#btnUpMinimize").click();
  await page.locator("#localeToggleBtn").click();
  await page.locator("#taskCenterButton").click();
  await expect(page.locator("#upTitle")).toHaveText("正在查看进行中的安装任务...");
  await expect(page.locator("#upTerminal")).toContainText("正在下载依赖包...");
});

browserTest("i18n: settings language selector can return to follow system locale", async t => {
  const { page } = await fixture(t, { locale: "en-US", status: runningStatus });
  await page.goto(origin + prefix + "#configuration");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.locator("html")).toHaveAttribute("data-locale-mode", "system");
  await expect(page.locator('#localeModeGroup [data-locale-option="system"]')).toHaveAttribute("aria-pressed", "true");

  await page.locator('#localeModeGroup [data-locale-option="zh-CN"]').click();
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  await expect(page.locator("html")).toHaveAttribute("data-locale-mode", "zh-CN");
  await expect(page.locator("#configurationTitle")).toHaveText("服务配置");
  assert.equal(await page.evaluate(() => localStorage.getItem("qwenpaw_locale_v1")), "zh-CN");

  await page.locator('#localeModeGroup [data-locale-option="system"]').click();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.locator("html")).toHaveAttribute("data-locale-mode", "system");
  await expect(page.locator("#configurationTitle")).toHaveText("Service Settings");
  assert.equal(await page.evaluate(() => localStorage.getItem("qwenpaw_locale_v1")), null);
});

browserTest("about dialog exposes troubleshooting and official links with native focus handling", async t => {
  const { page } = await fixture(t, { viewport: desktop });
  await page.goto(origin + prefix);
  const trigger = page.locator("#aboutButton");
  await trigger.click();
  const dialog = page.locator("#aboutDialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toHaveJSProperty("open", true);
  await expect(dialog.locator(".faq details")).toHaveCount(4);
  await expect(dialog.locator(".about-links a").nth(0)).toHaveAttribute("href", "https://github.com/agentscope-ai/QwenPaw");
  await expect(dialog.locator(".about-links a").nth(1)).toHaveAttribute("href", "https://github.com/yaozy2020/QwenPaw-for-fnOS");
  for (const link of await dialog.locator(".about-links a").all()) {
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(link).toHaveAttribute("rel", /noopener/);
  }
  await dialog.locator("summary").first().click();
  await expect(dialog.locator("details").first()).toHaveAttribute("open", "");
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
  await expect(trigger).toBeFocused();
});

browserTest("mobile header keeps visible actions within a compact, non-overlapping layout", async t => {
  const { page } = await fixture(t, {
    viewport: { width: 390, height: 844 },
    status: { ...runningStatus, install: { running: true } },
    install: { success: true, upgrading: true, running: true, seq: 42, instance: "worker-a", phase: "installing" },
  });
  await page.goto(origin + prefix);
  await expect(page.locator("#webConsoleText")).toHaveText("进入 Web 对话");
  await expect(page.locator("#webConsoleMobileText")).toHaveText("对话");
  await expect(page.locator("#webConsoleMobileText")).toBeVisible();
  await expect(page.locator("#btnWebConsole")).toHaveAttribute("aria-label", "进入 Web 对话");
  await expect(page.locator("#themeToggleBtn")).toHaveAttribute("aria-label", /.+/);
  await expect(page.locator("#aboutButton")).toHaveAttribute("aria-label", "关于与常见问题");
  await page.locator('#mobileSectionNav a[data-section="overview"]').click();
  await page.locator('#installRunningTip').click();
  await page.locator("#btnUpMinimize").click();
  await expect(page.locator("#taskCenterButton")).toBeVisible();
  const header = await box(page.locator("header"));
  assert.ok(header.height <= 150, `header height ${header.height}px exceeds two compact rows`);
  assert.equal(await page.locator("#statusBadge").isVisible(), false, "状态由概览承担，顶部不重复占高");
  assert.equal(await page.locator(".brand-info p").isVisible(), false, "顶部副标题在移动端收起");
  const brand = await box(page.locator(".brand"));
  const title = await box(page.locator(".brand-info h1"));
  const selectors = ["#themeToggleBtn", "#aboutButton", "#taskCenterButton", "#btnWebConsole"];
  const actions = await Promise.all(selectors.map(selector => box(page.locator(selector))));
  const intersects = (a, b) => a.x < b.x + b.width && a.x + a.width > b.x
    && a.y < b.y + b.height && a.y + a.height > b.y;
  for (const [index, action] of actions.entries()) {
    assert.ok(action.width >= 44 && action.height >= 44, `${selectors[index]} is smaller than 44px`);
    assert.ok(action.x >= header.x && action.x + action.width <= header.x + header.width, `${selectors[index]} leaves the header horizontally`);
    assert.ok(action.y >= header.y && action.y + action.height <= header.y + header.height, `${selectors[index]} leaves the header vertically`);
    assert.equal(intersects(brand, action), false, `${selectors[index]} overlaps the brand`);
    assert.equal(intersects(title, action), false, `${selectors[index]} overlaps the brand title`);
  }
  assert.equal(await page.locator("#taskCenterButton").evaluate(element => getComputedStyle(element, "::after").content), '"任务"');
  const nav = await box(page.locator("#mobileSectionNav"));
  assert.ok(header.y + header.height <= nav.y, "header and navigation remain in document flow without overlap");
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
});

for (const width of [360, 375, 390, 414]) {
  browserTest(`mobile header title and controls do not overlap at ${width}px`, async t => {
    const { page } = await fixture(t, { viewport: { width, height: 844 }, status: runningStatus });
    await page.goto(origin + prefix);

    const header = await box(page.locator("header"));
    const brand = await box(page.locator(".brand"));
    const title = await box(page.locator(".brand-info h1"));
    const controlSelectors = ["#themeToggleBtn", "#aboutButton", "#btnWebConsole"];
    const controls = await Promise.all(controlSelectors.map(selector => box(page.locator(selector))));
    const intersects = (a, b) => a.x < b.x + b.width && a.x + a.width > b.x
      && a.y < b.y + b.height && a.y + a.height > b.y;

    for (const [index, control] of controls.entries()) {
      assert.equal(intersects(brand, control), false, `${controlSelectors[index]} overlaps the brand`);
      assert.equal(intersects(title, control), false, `${controlSelectors[index]} overlaps the title`);
      assert.ok(control.width >= 44 && control.height >= 44, `${controlSelectors[index]} is smaller than 44px`);
    }
    assert.ok(header.height <= 90, `header wrapped to multiple rows (${header.height}px)`);
    assert.ok(Math.max(...controls.map(control => control.y)) - Math.min(...controls.map(control => control.y)) <= 1,
      "header actions share one row");
    await expect(page.locator("#webConsoleMobileText")).toBeVisible();
    await expect(page.locator("#webConsoleMobileText")).toHaveText("对话");
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("概览");
  });
}

browserTest("header actions stay within the page at 200 percent browser zoom", async t => {
  const { page } = await fixture(t, {
    viewport: { width: 195, height: 422 },
    deviceScaleFactor: 2,
    status: runningStatus,
  });
  await page.goto(origin + prefix);
  assert.equal(await page.evaluate(() => innerWidth), 195);
  assert.equal(await page.evaluate(() => devicePixelRatio), 2);
  const action = await box(page.locator("#btnWebConsole"));
  const documentWidth = await page.evaluate(() => document.documentElement.scrollWidth);
  assert.ok(action.x + action.width <= 195, `conversation action ends at ${action.x + action.width}px`);
  assert.ok(documentWidth <= 195, `document scrolls to ${documentWidth}px`);
  const header = await box(page.locator("header"));
  const nav = await box(page.locator("#mobileSectionNav"));
  assert.ok(header.y + header.height <= nav.y, "header and navigation remain in document flow without overlap");
});

browserTest("195px viewport keeps section headings and overview content readable at DPR2", async t => {
  const { page } = await fixture(t, {
    viewport: { width: 195, height: 422 },
    deviceScaleFactor: 2,
    status: runningStatus,
  });
  await page.goto(origin + prefix);
  assert.equal(await page.evaluate(() => devicePixelRatio), 2);
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 195);

  const headings = [];
  for (const section of ["overview", "configuration", "authorization", "logs"]) {
    await page.locator(`#mobileSectionNav [data-section="${section}"]`).click();
    const panel = page.locator(`#${section}`);
    await expect(panel).toBeVisible();
    const heading = panel.locator(".card-title h2").first();
    const metrics = await heading.evaluate(element => {
      const style = getComputedStyle(element);
      const title = element.closest(".card-title");
      return {
        text: element.innerText.replace(/\s+/g, " ").trim(),
        clientWidth: element.clientWidth,
        scrollWidth: element.scrollWidth,
        clientHeight: element.clientHeight,
        scrollHeight: element.scrollHeight,
        fontSize: Number.parseFloat(style.fontSize),
        lineHeight: Number.parseFloat(style.lineHeight),
        textLines: [...element.childNodes].flatMap(node => {
          if (node.nodeType !== Node.TEXT_NODE || !node.textContent.trim()) return [];
          const range = document.createRange();
          range.selectNodeContents(node);
          return [...range.getClientRects()].filter(rect => rect.width > 0).map(rect => Math.round(rect.top));
        }).filter((top, index, tops) => tops.indexOf(top) === index).length,
        titleWidth: title.clientWidth,
        titleScrollWidth: title.scrollWidth,
        titleHeight: title.clientHeight,
        titleScrollHeight: title.scrollHeight,
      };
    });
    headings.push({ section, ...metrics });
    assert.ok(metrics.clientWidth >= metrics.fontSize * 3, `${section} heading has no room for a short horizontal word: ${JSON.stringify(metrics)}`);
    assert.ok(metrics.scrollWidth <= metrics.clientWidth, `${section} heading clips horizontally: ${JSON.stringify(metrics)}`);
    assert.ok(metrics.clientHeight >= metrics.lineHeight, `${section} heading clips vertically: ${JSON.stringify(metrics)}`);
    assert.ok(metrics.textLines <= 2, `${section} heading wraps into too many narrow lines: ${JSON.stringify(metrics)}`);
    assert.ok(metrics.titleScrollWidth <= metrics.titleWidth, `${section} title/tools row clips horizontally: ${JSON.stringify(metrics)}`);
    assert.ok(metrics.titleScrollHeight <= metrics.titleHeight, `${section} title/tools row clips vertically: ${JSON.stringify(metrics)}`);
    const panelMetrics = await panel.evaluate(element => ({
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
    }));
    assert.ok(panelMetrics.scrollWidth <= panelMetrics.clientWidth, `${section} card clips horizontally: ${JSON.stringify(panelMetrics)}`);
  }

  await page.locator('#mobileSectionNav [data-section="overview"]').click();
  const summary = await page.locator(".status-description").evaluate(element => ({
    text: element.innerText,
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    fontSize: Number.parseFloat(getComputedStyle(element).fontSize),
    lineHeight: Number.parseFloat(getComputedStyle(element).lineHeight),
  }));
  assert.ok(summary.clientWidth >= summary.fontSize * 3, `status summary is compressed into a narrow column: ${JSON.stringify(summary)}`);
  assert.ok(summary.scrollWidth <= summary.clientWidth, `status summary clips horizontally: ${JSON.stringify(summary)}`);
  assert.ok(summary.clientHeight >= summary.lineHeight * 2, `status summary has no room to wrap: ${JSON.stringify(summary)}`);
  assert.ok(summary.scrollHeight <= summary.clientHeight, `status summary clips vertically: ${JSON.stringify(summary)}`);

  const stats = await page.locator(".stat-grid").evaluate(element => ({
    columns: getComputedStyle(element).gridTemplateColumns.split(" ").length,
    children: [...element.children].map(child => ({
      text: child.innerText.replace(/\s+/g, " ").trim(),
      width: child.clientWidth,
      scrollWidth: child.scrollWidth,
      height: child.clientHeight,
      scrollHeight: child.scrollHeight,
      fontSize: Number.parseFloat(getComputedStyle(child.querySelector(".stat-value")).fontSize),
    })),
  }));
  assert.equal(stats.columns, 1, `overview metrics should stack at this width: ${JSON.stringify(stats)}`);
  assert.ok(stats.children.every(stat => stat.width >= stat.fontSize * 3 && stat.scrollWidth <= stat.width && stat.scrollHeight <= stat.height),
    `overview metrics are compressed or clipped: ${JSON.stringify(stats)}`);
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 195);
  assert.deepEqual(headings.map(({ section }) => section), ["overview", "configuration", "authorization", "logs"]);
  await page.addStyleTag({ content: "[data-mobile-panel] { display: flex !important; } .grid { display: flex !important; flex-direction: column; }" });
  await page.screenshot({ path: join(screenshotDirectory, "supplement-browser-zoom-200-390x844-full.png"), fullPage: true });
});

browserTest("brand title and header navigation remain readable at 200 percent text size", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 }, status: runningStatus });
  await page.goto(origin + prefix);
  await page.evaluate(() => {
    const elements = [...document.querySelectorAll("header, header *, #mobileSectionNav, #mobileSectionNav *")]
      .filter(element => element instanceof HTMLElement && element.innerText?.trim() && element.getClientRects().length);
    for (const element of elements) {
      const size = Number.parseFloat(getComputedStyle(element).fontSize);
      if (Number.isFinite(size) && size > 0) element.style.setProperty("font-size", `${size * 2}px`, "important");
    }
  });

  const title = await box(page.locator(".brand-info h1"));
  const titleMetrics = await page.locator(".brand-info h1").evaluate(element => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
    clientHeight: element.clientHeight,
    scrollHeight: element.scrollHeight,
    fontSize: parseFloat(getComputedStyle(element).fontSize),
  }));
  assert.ok(titleMetrics.fontSize >= 32, "the actual section title scales with user text size");
  assert.ok(titleMetrics.scrollWidth <= titleMetrics.clientWidth, `brand title clips horizontally (${titleMetrics.scrollWidth}px > ${titleMetrics.clientWidth}px)`);
  assert.ok(titleMetrics.scrollHeight <= titleMetrics.clientHeight, `brand title clips vertically (${titleMetrics.scrollHeight}px > ${titleMetrics.clientHeight}px)`);
  const selectors = ["#themeToggleBtn", "#aboutButton", "#btnWebConsole"];
  const intersects = (a, b) => a.x < b.x + b.width && a.x + a.width > b.x
    && a.y < b.y + b.height && a.y + a.height > b.y;
  for (const selector of selectors) {
    const action = await box(page.locator(selector));
    assert.equal(intersects(title, action), false, `${selector} overlaps the enlarged brand title`);
  }
  const header = await box(page.locator("header"));
  const nav = await box(page.locator("#mobileSectionNav"));
  assert.ok(header.y + header.height <= nav.y, "header and navigation remain in document flow without overlap");
  for (const link of await page.locator("#mobileSectionNav a").all()) {
    const metrics = await link.evaluate(element => ({ clientWidth: element.clientWidth, scrollWidth: element.scrollWidth, clientHeight: element.clientHeight, scrollHeight: element.scrollHeight }));
    assert.ok(metrics.scrollWidth <= metrics.clientWidth, `navigation label clips horizontally (${metrics.scrollWidth}px > ${metrics.clientWidth}px)`);
    assert.ok(metrics.scrollHeight <= metrics.clientHeight, `navigation label clips vertically (${metrics.scrollHeight}px > ${metrics.clientHeight}px)`);
  }
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
});

browserTest("uninstalled overview provides the first-run guide and homepage installer", async t => {
  const { page, state } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  await expect(page.locator("#statusCardMount .status-description")).toHaveText("先安装 QwenPaw 内核，即可开始使用 AI 对话。");
  await expect(page.locator("#btnWebConsole")).toHaveAttribute("href", "#version");
  await expect(page.locator("#webConsoleText")).toHaveText("安装并开始使用");
  await expect(page.locator("#webConsoleMobileText")).toHaveText("安装");
  await expect(page.locator("#webConsoleMobileText")).toBeVisible();
  await expect(page.locator("#btnWebConsole")).toHaveAttribute("aria-label", "安装并开始使用");
  await expect(page.locator("#statusCheckedAt")).toContainText("最近同步");
  await expect(page.locator("#overviewAuth")).toHaveText("无法确认");
  await expect(page.locator("#overviewGuide")).toBeVisible();
  await expect(page.locator("#overviewGuide li")).toHaveCount(3);
  await expect(page.locator("#overviewLogPreview")).toHaveCount(0);
  await expect(page.locator("#overviewInstaller")).toBeVisible();
  await expect(page.locator("#overviewInstallVersion")).toHaveText("v2.0.0 · 随包适配补丁");
  await expect(page.locator("#releaseNotesDialog")).toHaveCount(0);
  await page.locator("#overviewInstallButton").click();
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
  assert.equal(state.posts.filter(post => post.path === "kernel/install").length, 1);
});

browserTest("running overview provides direct console access and hides installation actions", async t => {
  const { page } = await fixture(t, { status: {
    success: true, installed: true, running: true, pid: 421,
    startAt: Math.floor(Date.now() / 1000) - 3600, cpu: 1.5, memKB: 102400,
    version: "2.0.0", authStatus: "protected", install: { running: false },
  } });
  await page.goto(origin + prefix);
  await expect(page.locator("#statusCardMount .status-description")).toContainText("内核进程正在运行");
  await expect(page.locator("#btnWebConsole")).toHaveAttribute("href", "/app/qwenpaw/web/");
  await expect(page.locator("#webConsoleText")).toHaveText("进入 Web 对话");
  await expect(page.locator("#webConsoleMobileText")).toHaveText("对话");
  await expect(page.locator("#webConsoleMobileText")).toBeHidden();
  await expect(page.locator("#overviewInstaller")).toBeHidden();
  await expect(page.locator("#btnStartService")).toBeHidden();
  await expect(page.locator("#btnRestartService")).toBeVisible();
  await expect(page.locator("#statusCheckedAt")).toContainText("最近同步");
  await expect(page.locator("#overviewVersion")).toHaveText("2.0.0");
  await expect(page.locator("#overviewPort")).toHaveText("18088");
  await expect(page.locator("#overviewAuth")).toHaveText("已开启（需要密码）");
  await expect(page.locator("#overviewGuide")).toBeHidden();
  await expect(page.locator("#overviewLogPreview")).toHaveCount(0);
});

browserTest("PC browser opens QwenPaw Web in a new tab", async t => {
  const { page } = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix);
  const popupPromise = page.waitForEvent("popup");
  const requestPromise = page.context().waitForEvent("request", {
    predicate: request => request.url() === `${origin}/app/qwenpaw/web/`,
  });
  await page.locator("#btnWebConsole").click();
  const [popup, request] = await Promise.all([popupPromise, requestPromise]);
  assert.ok(popup);
  assert.equal(request.url(), `${origin}/app/qwenpaw/web/`);
  await expect(page.locator("#webEntryOverlay")).toBeHidden();
});

browserTest("fnOS App opens QwenPaw Web in a full-screen overlay and closes it before leaving", async t => {
  const { page } = await fixture(t, {
    status: runningStatus,
    userAgent: "Mozilla/5.0 FNAppType/fnOS FNAppVer/1.38.0",
  });
  await page.goto(origin + prefix);
  const beforeHistory = await page.evaluate(() => history.length);
  await page.locator("#btnWebConsole").click();
  await expect(page.locator("#webEntryOverlay")).toBeVisible();
  await expect(page.locator("#webEntryFrame")).toHaveAttribute("src", "/app/qwenpaw/web/");
  assert.equal(await page.evaluate(() => history.length), beforeHistory + 1);
  await page.goBack();
  await expect(page.locator("#webEntryOverlay")).toBeHidden();
  await expect(page.locator("#overview")).toBeVisible();
});

browserTest("manual refresh exposes busy state until status finishes without loading logs", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix);
  await expect(page.locator("#overviewInstaller")).toBeVisible();
  let release;
  state.statusHeld = new Promise(resolve => { release = resolve; });
  t.after(() => release());
  await page.locator("#refreshDashboardBtn").click();
  await expect(page.locator("#refreshDashboardBtn")).toBeDisabled();
  await expect(page.locator("#refreshDashboardBtn")).toHaveAttribute("aria-busy", "true");
  release();
  assert.deepEqual(state.liveLogQueries, []);
  await expect(page.locator("#refreshDashboardBtn")).toBeEnabled();
  await expect(page.locator("#refreshDashboardBtn")).not.toHaveAttribute("aria-busy");
});

browserTest("mobile overview drops the duplicated shortcut entries without horizontal overflow", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  await expect(page.locator(".overview-shortcuts")).toHaveCount(0);
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  const about = page.locator("#aboutButton");
  await expect(about).toBeVisible();
  await about.click();
  await expect(page.locator("#aboutDialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator("#aboutDialog")).not.toBeVisible();
});

browserTest("mobile navigation stays fixed above the safe-area bottom without covering content", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  const nav = page.locator("#mobileSectionNav");
  await expect(nav).toBeVisible();
  await expect(nav).toHaveCSS("position", "fixed");
  const bounds = await box(nav);
  assert.ok(Math.abs(bounds.y + bounds.height - 844) <= 1, `navigation must sit at the viewport bottom: ${bounds.y + bounds.height}`);
  const padding = await page.locator("body").evaluate(el => parseFloat(getComputedStyle(el).paddingBottom));
  assert.ok(padding >= 56, `content needs bottom clearance: ${padding}`);
});

browserTest("mobile navigation exposes four pages and synchronizes hash and history", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  const nav = page.locator("#mobileSectionNav");
  const overview = nav.locator('[data-section="overview"]');
  const authorization = nav.locator('[data-section="authorization"]');
  await expect(nav).toHaveCSS("display", "grid");
  await expect(nav).toHaveCSS("position", "fixed");
  assert.equal(await nav.locator("a").count(), 4);
  await expect(nav.locator("a")).toHaveText(["概览", "配置", "授权与备份", "日志"]);
  for (const link of await nav.locator("a").all()) {
    const size = await link.boundingBox();
    assert.ok(size && size.width >= 44 && size.height >= 44);
  }
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  await expect(overview).toHaveAttribute("aria-current", "page");
  assert.equal(await page.evaluate(() => location.hash), "#overview");
  await expect(page.locator("#overview")).toBeVisible();
  await expect(page.locator("#version")).toBeVisible();
  await expect(page.locator("#btnUpgradeAction")).toBeVisible();
  await expect(page.locator("#configuration")).toBeHidden();
  await expect(page.locator("#authorization")).toBeHidden();
  await expect(page.locator("#logs")).toBeHidden();

  await authorization.click();
  await expect(authorization).toHaveAttribute("aria-current", "page");
  await expect(overview).not.toHaveAttribute("aria-current");
  await expect(page.locator("#authorization")).toBeVisible();
  await expect(page.locator("#overview")).toBeHidden();
  await expect(page.locator("#version")).toBeHidden();
  assert.equal(await page.evaluate(() => location.hash), "#authorization");

  await page.goBack();
  await expect(overview).toHaveAttribute("aria-current", "page");
  await expect(page.locator("#overview")).toBeVisible();
  assert.equal(await page.evaluate(() => location.hash), "#overview");
  await page.goForward();
  await expect(authorization).toHaveAttribute("aria-current", "page");
  await expect(page.locator("#authorization")).toBeVisible();
  const configuration = nav.locator('[data-section="configuration"]');
  await configuration.focus();
  await page.keyboard.press("Enter");
  await expect(configuration).toHaveAttribute("aria-current", "page");
  assert.equal(await page.evaluate(() => location.hash), "#configuration");
});

for (const width of [360, 390, 768, 1440]) {
  browserTest(`console navigation and content fit without horizontal overflow at ${width}px`, async t => {
    const mobile = width <= 768;
    const { page } = await fixture(t, { viewport: { width, height: mobile ? 844 : 900 } });
    await page.goto(origin + prefix);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    assert.equal(await page.evaluate(() => document.body.scrollWidth <= innerWidth), true);
    const nav = page.locator("#mobileSectionNav");
    if (mobile) {
      await expect(nav).toBeVisible();
      await expect(nav).toHaveCSS("position", "fixed");
      await expect(nav).toHaveCSS("display", "grid");
      for (const link of await nav.locator("a").all()) {
        const size = await link.boundingBox();
        assert.ok(size && size.width >= 40 && size.height >= 44);
      }
    } else {
      await expect(nav).toBeVisible();
      await expect(nav).toHaveCSS("position", "sticky");
      for (const section of ["configuration", "authorization", "logs"]) {
        await expect(page.locator(`#${section}`)).toBeHidden();
      }
    }
  });
}

browserTest("mobile configuration is grouped as its own section", async t => {
  const { page } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  await page.locator('#mobileSectionNav [data-section="configuration"]').click();
  await expect(page.locator("#configuration")).toBeVisible();
  for (const selector of ["#valPort", "#valWebProxyPort", "#webProxyPublic", "#valAuthStatus", "#resetAuthBtn"]) {
    await expect(page.locator(selector)).toBeVisible();
    assert.equal(await page.locator(`#configuration ${selector}`).count(), 1);
  }
  await expect(page.locator("#overview")).toBeHidden();
  await expect(page.locator("#version")).toBeHidden();
  await expect(page.locator('#mobileSectionNav [data-section="configuration"]')).toHaveAttribute("aria-current", "page");
});

browserTest("mobile authorization page owns directory controls and opens the upstream backup page", async t => {
  const { page } = await fixture(t, {
    viewport: { width: 390, height: 844 },
    status: runningStatus,
    userAgent: "Mozilla/5.0 FNAppType/Android FNAppVer/1.38.2",
  });
  await page.goto(origin + prefix);
  await page.locator('#mobileSectionNav [data-section="authorization"]').click();
  await expect(page.locator("#authorization")).toBeVisible();
  await expect(page.locator("#authorized-dirs")).toBeVisible();
  await expect(page.locator("#addAuthorizedDirBtn")).toBeVisible();
  await expect(page.locator("#configuration")).toBeHidden();
  await expect(page.locator("#overview")).toBeHidden();
  await expect(page.locator("#version")).toBeHidden();
  const backup = page.locator("#openBackupBtn");
  await expect(backup).toBeEnabled();
  await backup.click();
  await expect(page.locator("#webEntryOverlay")).toBeVisible();
  await expect(page.locator("#webEntryFrame")).toHaveAttribute("src", "/app/qwenpaw/web/backups");
});

browserTest("console reconstruction displays the directory and opens it in the native file manager", async t => {
  const state = "recovered-auth";
  const search = `?status=success&method=pickUserFile&state=${state}&path=%5B%22%2Fvol1%2Fwork%22%5D`;
  const { page, state: fixtureState } = await fixture(t, {
    initScript: ({ state, search }) => {
      localStorage.setItem("qwenpaw.auth-result.v1", JSON.stringify({
        state,
        result: { status: "success", error: "", method: "pickUserFile", state, path: ["/vol1/work"] },
        search,
        ts: Date.now(),
      }));
      window.__openedNativeDirs = [];
      Object.defineProperty(window, "parent", {
        configurable: true,
        value: {
          postMessage(message) {
            if (message.penpal === "syn") {
              const response = new MessageEvent("message", {
                origin: location.origin,
                data: { penpal: "synAck", methodNames: ["openFileManager"] },
              });
              Object.defineProperty(response, "source", { value: window.parent });
              window.dispatchEvent(response);
            } else if (message.penpal === "call") {
              window.__openedNativeDirs.push(message);
              const response = new MessageEvent("message", {
                origin: location.origin,
                data: { penpal: "reply", id: message.id, resolution: "fulfilled", returnValue: null },
              });
              Object.defineProperty(response, "source", { value: window.parent });
              window.dispatchEvent(response);
            }
          },
        },
      });
    },
    initScriptArgs: { state, search },
  });
  await page.goto(origin + prefix);
  await expect.poll(() => page.evaluate(() => localStorage.getItem("qwenpaw.auth-result.v1"))).toBeNull();
  assert.deepEqual(fixtureState.projectDirs, [{ path: "/vol1/work" }]);
  await expect(page.locator("#authorizedDirsList")).toHaveCount(0);
  await page.locator('[data-section="authorization"]').click();
  await expect(page.locator("#authorizedDirsList")).toContainText("/vol1/work");
  await page.getByRole("button", { name: "打开目录", exact: true }).click();
  const opened = await page.evaluate(() => window.__openedNativeDirs);
  assert.deepEqual(opened.map(call => ({ methodName: call.methodName, args: call.args })), [
    { methodName: "openFileManager", args: ["/vol1/work"] },
  ]);
});

browserTest("native directory open reports the bridge result instead of claiming success", async t => {
  const state = "native-open-failure";
  const search = `?status=success&method=pickUserFile&state=${state}&path=%5B%22%2Fvol1%2Fwork%22%5D`;
  const { page } = await fixture(t, {
    initScript: ({ state, search }) => {
      localStorage.setItem("qwenpaw.auth-result.v1", JSON.stringify({
        state,
        result: { status: "success", error: "", method: "pickUserFile", state, path: ["/vol1/work"] },
        search,
        ts: Date.now(),
      }));
      window.__openCalls = [];
      Object.defineProperty(window, "parent", {
        configurable: true,
        value: {
          postMessage(message) {
            if (message.penpal === "syn") {
              const response = new MessageEvent("message", {
                origin: location.origin,
                data: { penpal: "synAck", methodNames: ["openFileManager"] },
              });
              Object.defineProperty(response, "source", { value: window.parent });
              window.dispatchEvent(response);
            } else if (message.penpal === "call") {
              window.__openCalls.push(message);
              const response = new MessageEvent("message", {
                origin: location.origin,
                data: {
                  penpal: "reply",
                  id: message.id,
                  resolution: "rejected",
                  returnValueIsError: true,
                  returnValue: { message: "E_UNSUPPORTED" },
                },
              });
              Object.defineProperty(response, "source", { value: window.parent });
              window.dispatchEvent(response);
            }
          },
        },
      });
    },
    initScriptArgs: { state, search },
  });
  await page.goto(origin + prefix + "#authorization");
  await page.getByRole("button", { name: "打开目录", exact: true }).click();
  await expect(page.locator("#authorizedDirsStatus")).toContainText("E_UNSUPPORTED");
  await expect(page.locator("#authorizedDirsStatus")).toHaveAttribute("data-error", "true");
  await expect(page.locator("#authorizedDirsStatus")).not.toContainText("已请求飞牛打开文件管理器");
  const calls = await page.evaluate(() => window.__openCalls);
  assert.deepEqual(calls.map(call => ({ methodName: call.methodName, args: call.args })), [
    { methodName: "openFileManager", args: ["/vol1/work"] },
  ]);
});

browserTest("mobile directory open uses the TrimApp Flutter envelope and app API version", async t => {
  const { page } = await fixture(t, {
    projectDirs: [{ path: "/vol1/work" }],
    userAgent: "Mozilla/5.0 FNAppType/Android FNAppVer/1.38.2",
    initScript: () => {
      window.__openCalls = [];
      window.flutter_inappwebview = {
        _platformReady: true,
        callHandler(name, path) {
          window.__openCalls.push({ name, path });
          if (name === "getAppMessage") {
            return Promise.resolve(JSON.stringify({ result: JSON.stringify({ appApi: "v1.0" }) }));
          }
          if (name === "openFileManager") {
            return Promise.resolve(JSON.stringify({ result: JSON.stringify(null) }));
          }
          return Promise.resolve(null);
        },
      };
    },
  });
  await page.goto(origin + prefix + "#authorization");
  await page.getByRole("button", { name: "打开目录", exact: true }).click();
  await expect(page.locator("#authorizedDirsStatus")).toContainText("openFileManager 返回：null");
  assert.deepEqual(await page.evaluate(() => window.__openCalls), [
    { name: "getAppMessage", path: undefined },
    { name: "openFileManager", path: "/vol1/work" },
  ]);
});

browserTest("mobile directory open reports host appMessage when platform config cannot initialize", async t => {
  const { page } = await fixture(t, {
    projectDirs: [{ path: "/vol1/work" }],
    userAgent: "Mozilla/5.0 FNAppType/Android FNAppVer/1.38.2",
    initScript: () => {
      window.__openCalls = [];
      window.flutter_inappwebview = {
        _platformReady: true,
        callHandler(name, path) {
          window.__openCalls.push({ name, path });
          return Promise.resolve(null);
        },
      };
    },
  });
  await page.goto(origin + prefix + "#authorization");
  await page.getByRole("button", { name: "打开目录", exact: true }).click();
  await expect(page.locator("#authorizedDirsStatus")).toContainText("openFileManager requires appApi >= v1.0");
  await expect(page.locator("#authorizedDirsStatus")).toContainText("getAppMessage=null");
  assert.deepEqual(await page.evaluate(() => window.__openCalls), [
    { name: "getAppMessage", path: undefined },
    { name: "getAppMessage", path: undefined },
  ]);
});

browserTest("legacy version deep links open deployment inside overview", async t => {
  const { page } = await fixture(t, { viewport: { width: 375, height: 812 } });
  await page.goto(origin + prefix + "#version");
  await expect(page.locator("#overview")).toBeVisible();
  await expect(page.locator("#btnUpgradeAction")).toBeVisible();
  await expect(page.locator("#configuration")).toBeHidden();
  await expect(page.locator("#authorization")).toBeHidden();
  await expect(page.locator('#mobileSectionNav [data-section="overview"]')).toHaveAttribute("aria-current", "page");
  assert.equal(await page.evaluate(() => location.hash), "#version");
});

browserTest('history failing after entering logs releases the subscription and shows the single-page fallback', async t => {
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(origin + prefix + '#logs');
  await expect(page.locator('#logTerminal')).toContainText('原始服务日志');
  await page.evaluate(() => {
    history.replaceState = () => { throw new Error('blocked history'); };
    location.hash = '#unknown';
  });
  await expect(page.locator('#overview')).toBeVisible();
  await expect(page.locator('#logs')).toBeVisible();
  await expect(page.locator('#btnChangeServicePort')).toBeVisible();
  await expect(page.locator('#authorizedDirsList')).toBeVisible();
  await expect(page.locator('#mobileSectionNav [aria-current]')).toHaveCount(0);
  assert.equal(await page.evaluate(() => window.EventSource.instances.filter(s => !s.closed && s.url.includes('watch=logs')).length), 0);
  assert.deepEqual(state.liveLogQueries, [2000]);
  assert.deepEqual(errors, []);
});

browserTest('blocked pushState leaves native anchors and manual log reads available', async t => {
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
  await page.goto(origin + prefix + '#overview');
  await page.evaluate(() => { history.pushState = () => { throw new Error('blocked history'); }; });
  await page.getByRole('link', { name: '配置', exact: true }).click();
  await expect(page).toHaveURL(/#configuration$/);
  await page.getByRole('link', { name: '日志', exact: true }).click();
  await expect(page).toHaveURL(/#logs$/);
  assert.deepEqual(state.liveLogQueries, []);
  await openLogTools(page);
  await page.getByRole('button', { name: '刷新日志' }).click();
  await expect(page.locator('#logTerminal')).toContainText('原始服务日志');
  assert.deepEqual(state.liveLogQueries, [2000]);
  assert.equal(await page.evaluate(() => window.EventSource.instances.filter(s => !s.closed && s.url.includes('watch=logs')).length), 0);
});

browserTest('version and log history keep title, active link, focus and reads in sync', async t => {
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
  await page.goto(origin + prefix + '#version');
  const active = page.locator('#mobileSectionNav a[aria-current="page"]');
  await expect(active).toHaveText('概览');
  await expect(page.locator('#sectionTitle')).toHaveText('概览');
  assert.deepEqual(state.liveLogQueries, []);
  await page.getByRole('link', {name:'日志', exact:true}).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('#logs')).toBeFocused();
  await expect(page.locator('#sectionTitle')).toHaveText('日志');
  await expect(active).toHaveText('日志');
  await expect(page.locator('#logTerminal')).toContainText('原始服务日志');
  await page.goBack();
  await expect(page.locator('#version')).toBeFocused();
  await expect(active).toHaveText('概览');
  await expect(page.locator('#sectionTitle')).toHaveText('概览');
  assert.deepEqual(state.liveLogQueries, [2000]);
  await page.goForward();
  await expect(page.locator('#logs')).toBeFocused();
  await expect(active).toHaveText('日志');
  await expect.poll(() => state.liveLogQueries).toEqual([2000,100]);
  assert.equal(state.requestPaths.filter(r => r.path === 'kernel/versions').length, 1);
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
  await expect(page.locator("#mobileSectionNav")).toBeVisible();
  await expect(page.locator("#overview")).toBeVisible();
  await expect(page.locator("#btnUpgradeAction")).toBeVisible();
  for (const section of ["configuration", "authorization", "logs"]) {
    await expect(page.locator(`#${section}`)).toBeHidden();
  }
  await page.setViewportSize({ width: 375, height: 812 });
  await expect(page.locator("#mobileSectionNav")).toBeVisible();
  await expect(page.locator("#overview")).toBeVisible();
  await expect(page.locator("#btnUpgradeAction")).toBeVisible();
  await expect(page.locator("#configuration")).toBeHidden();
  await expect(page.locator('#mobileSectionNav [data-section="overview"]')).toHaveAttribute("aria-current", "page");
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
    await expect(page.locator("#mobileSectionNav")).toBeVisible();
    for (const section of ["overview", "configuration", "authorization", "logs"]) {
      await expect(page.locator(`#${section}`)).toBeVisible();
    }
    await page.locator('#mobileSectionNav [data-section="overview"]').click();
    await expect(page.locator("html")).not.toHaveAttribute("data-mobile-section");
    for (const section of ["overview", "configuration", "authorization", "logs"]) {
      await expect(page.locator(`#${section}`)).toBeVisible();
    }
  });
}

browserTest("mobile section switches preserve the active install poll and never resubmit", async t => {
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
  await page.locator('#mobileSectionNav [data-section="overview"]').click();
  await openInstall(page);
  await page.locator("#btnUpMinimize").click();
  await expect.poll(() => state.statusPolls).toBeGreaterThan(0);

  const result = await page.evaluate(() => {
    const audit = window.navigationAudit;
    const before = { fetches: audit.fetches, timers: audit.timers.length, history: history.length };
    const activePoll = audit.timers.findLast(event => event.action === "set" && event.delay === 800);
    for (const section of ["configuration", "authorization", "logs", "overview", "overview"]) {
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
  await expect(page.locator("#overview")).toBeVisible();
  assert.deepEqual(result.timers.map(e => [e.action, e.delay]), [["set", 15000], ["clear", undefined]]);
  assert.equal(result.timers[0].id, result.timers[1].id);
  assert.equal(result.historyEntries, 4);
  const polls = state.statusPolls;
  await expect.poll(() => state.statusPolls).toBeGreaterThan(polls);
  assert.equal(state.posts.filter(post => post.path === "kernel/install").length, 1);
});

browserTest("previously retired service ports pass validation and submit", async t => {
  const { page, state } = await fixture(t, { viewport: desktop, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  await page.locator('[data-section="configuration"]').click();
  for (const port of [19090, 19091]) {
    await page.locator("#btnChangeServicePort").click();
    await page.locator("#customModal.active #modalInput").fill(String(port));
    await page.locator("#customModal.active #modalConfirmBtn").click();
    await page.locator("#customModal.active #modalTitle").filter({ hasText: "修改服务端口确认" }).waitFor();
    await page.locator("#customModal.active #modalConfirmBtn").click();
    await expect(page.locator("#toast")).toContainText(`端口已变更为 ${port}`);
  }
  assert.deepEqual(state.posts.filter(post => post.path === "port").map(post => post.body.port), [19090, 19091]);
});

browserTest("unreachable status clears stale overview values until a fresh snapshot arrives", async t => {
  const { page, state } = await fixture(t, { viewport: desktop, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#overviewPid")).toHaveText("421");
  await expect(page.locator("#overviewVersion")).toHaveText("2.0.0");
  await expect(page.locator("#overviewPort")).toHaveText("18088");
  await expect(page.locator("#overviewAuth")).toContainText("需要密码");

  state.statusCode = 503;
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator("#statusBadge")).toContainText("状态不可达");
  for (const selector of ["#overviewPid", "#overviewVersion", "#overviewPort", "#overviewAuth"]) {
    await expect(page.locator(selector)).toHaveText("—");
  }
  await page.locator('[data-section="configuration"]').click();
  await expect(page.locator("#valPort")).toHaveText("无法获取");
  await expect(page.locator("#valAdminStatus")).toHaveText("无法获取");
  await expect(page.locator("#statusCardMount .status-description")).toContainText("HTTP 503");
  await expect(page.locator("#btnChangeServicePort")).toBeDisabled();
  await page.locator("#btnChangeServicePort").dispatchEvent("click");
  await expect(page.locator("#customModal")).not.toHaveClass(/active/);
  await expect(page.locator("#toast")).toContainText("服务状态尚未同步");
  assert.equal(state.posts.filter(post => post.path === "port").length, 0);

  state.statusCode = 200;
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator("#overviewPid")).toHaveText("421");
  await expect(page.locator("#overviewVersion")).toHaveText("2.0.0");
  await expect(page.locator("#overviewPort")).toHaveText("18088");
  await expect(page.locator("#overviewAuth")).toContainText("需要密码");
});

browserTest("unreachable status stays disabled across delayed version loads and selections, then recovers", async t => {
  let release;
  const versionsHeld = new Promise(resolve => { release = resolve; });
  const { page, state } = await fixture(t, { statusCode: 503, versionsHeld });
  t.after(() => release());
  await page.goto(origin + prefix + "#version");
  await expect(page.locator("#statusBadge")).toContainText("状态不可达");
  release();
  await expect(page.locator("#targetVersion")).toContainText("2.0.0");
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  await pickVersion(page, "2.0.0");
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
  await page.goto(origin + prefix + "#configuration");
  await expect(page.locator("#valWebProxyPort")).toHaveText("43126");
  await expect(page.locator("#webProxyPublic")).toBeDisabled();
  assert.equal(await page.locator("#webProxyPublic").evaluate(el => el.indeterminate), true);
  await page.getByRole("button", {name: "修改反代端口"}).click();
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
  await page.goto(origin + prefix + "#configuration");
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
  await page.goto(origin + prefix + "#version");
  const picker = page.locator('#targetVersion');
  await expect(picker).toBeEnabled();
  await picker.focus(); await page.keyboard.press('ArrowDown');
  await page.keyboard.press('End'); await page.keyboard.press('Enter');
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
  await expect(page.locator("#upTerminal")).toContainText("正在安装内核");
  state.install = { success: true, upgrading: false, running: false, seq: 42, instance: "worker-a", phase: "done" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).toContainText("顺利完成");
  await page.locator("#btnUpClose").click();
  state.installLogs = ["Successfully installed qwenpaw-2.0.0"];
  await page.locator("#taskCenterButton").click();
  await expect(page.locator("#upTerminal")).toContainText("Successfully installed qwenpaw-2.0.0");
});

browserTest("terminal completion replaces the running fallback when no install logs are returned", async t => {
  const { page, state } = await fixture(t, { installLogs: [] });
  await page.goto(origin + prefix);
  await openInstall(page);
  await expect(page.locator("#upTerminal")).toContainText("正在安装内核");
  state.install = { success: true, upgrading: false, running: false, seq: 42, instance: "worker-a", phase: "done" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).toContainText("顺利完成");
  await expect(page.locator("#upTerminal")).not.toContainText("正在安装内核");
  await expect(page.locator("#upTerminal")).toContainText("未获取到日志输出");
});

browserTest("after a version change the same target is a reinstall, not the old list's upgrade", async t => {
  const { page, state } = await fixture(t, {status: {...runningStatus}});
  state.versions.versions[0].action = 'upgrade';
  state.versions.versions[1].action = 'reinstall';
  await page.goto(origin + prefix + '#version');
  await pickVersion(page, '2.1.0b1');
  await expect(page.locator('#btnUpgradeAction')).toHaveText('执行更新至 v2.1.0b1');
  state.status = {...runningStatus, version: '2.1.0b1'};
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator('#btnUpgradeAction')).toHaveText('重新安装 v2.1.0b1');
  await pickVersion(page, '2.0.0');
  await expect(page.locator('#btnUpgradeAction')).toHaveText('降级至 v2.0.0');
});

browserTest("an owned terminal task updates the overview without waiting for SSE or closing the dialog", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix);
  await openInstall(page);
  state.status = { ...runningStatus };
  state.install = { success: true, upgrading: false, seq: 42, instance: 'worker-a', phase: 'done' };
  await expect(page.locator('#upTitle')).toContainText('顺利完成');
  await expect(page.locator('#statusCardMount .status-text')).toHaveText('运行中', {timeout: 4000});
  await expect(page.locator('#upgradeModal')).toHaveClass(/active/);
});

browserTest("terminal completion preserves received task output when the final log read is empty", async t => {
  const { page, state } = await fixture(t, { viewport: { width: 390, height: 844 } });
  await page.goto(origin + prefix);
  await page.locator('#mobileSectionNav a[data-section="overview"]').click();
  await openInstall(page);
  await page.locator("#btnUpMinimize").click();
  await page.locator("#taskCenterButton").click();
  await expect(page.locator("#upTerminal")).toContainText("本任务日志");
  // 此用例验证没有最终日志时的回退；非空的最新任务日志应优先显示。
  state.installLogs = [];
  state.install = { success: true, upgrading: false, running: false, seq: 42, instance: "worker-a", phase: "done" };
  await pollNow(page, state);
  await expect(page.locator("#upTitle")).toContainText("顺利完成");
  await expect(page.locator("#upTerminal")).not.toContainText("正在安装内核");
  await expect(page.locator("#upTerminal")).toContainText("本任务日志");
});

browserTest("navigation initialization failure preserves working management controls", async t => {
  const { page, state } = await fixture(t, { viewport: { width: 390, height: 844 }, initScript: () => {
    history.replaceState = () => { throw new Error("blocked history"); };
  } });
  await page.goto(origin + prefix);
  await expect(page.locator("html")).not.toHaveAttribute("data-mobile-section");
  await openInstall(page);
  await expect.poll(() => state.posts.filter(post => post.path === "kernel/install").length).toBe(1);
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

browserTest("late status success and failure cannot overwrite a newer snapshot", async t => {
  const { page, state } = await fixture(t, { viewport: desktop, status: runningStatus });
  await page.goto(origin + prefix);
  await expect(page.locator("#overviewPid")).toHaveText("421");

  let releaseFailure;
  state.statusCode = 503;
  state.statusHeld = new Promise(resolve => { releaseFailure = resolve; });
  let requests = state.statusRequests;
  const staleFailure = page.evaluate(() => window.fetchStatus());
  await expect.poll(() => state.statusRequests).toBeGreaterThan(requests);
  state.statusCode = 200;
  state.status = { ...runningStatus, pid: 777 };
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator("#overviewPid")).toHaveText("777");
  releaseFailure();
  await staleFailure;
  await expect(page.locator("#overviewPid")).toHaveText("777");
  await expect(page.locator("#statusBadge")).toContainText("运行中");

  let releaseSuccess;
  state.status = { ...runningStatus, running: false, pid: 111 };
  state.statusHeld = new Promise(resolve => { releaseSuccess = resolve; });
  requests = state.statusRequests;
  const staleSuccess = page.evaluate(() => window.fetchStatus());
  await expect.poll(() => state.statusRequests).toBeGreaterThan(requests);
  state.status = { ...runningStatus, pid: 888 };
  await page.evaluate(() => window.fetchStatus());
  await expect(page.locator("#overviewPid")).toHaveText("888");
  releaseSuccess();
  await staleSuccess;
  await expect(page.locator("#overviewPid")).toHaveText("888");
  await expect(page.locator("#statusBadge")).toContainText("运行中");
});

browserTest("only the visible live log view subscribes to file changes and fetches the new tail", async t => {
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
  await page.goto(origin + prefix + '#logs');
  await expect(page.locator('#logTerminal')).toContainText('原始服务日志');
  assert.equal(await page.evaluate(() => window.EventSource.instances.at(-1).url), '/app/qwenpaw/api/events?watch=logs');
  state.logs.push('新增服务日志');
  await page.evaluate(() => window.EventSource.instances.at(-1).emit('log-change', ''));
  await expect(page.locator('#logTerminal')).toContainText('新增服务日志');
  await openLogTools(page);
  await page.locator('#autoRefreshCheck').uncheck();
  assert.equal(await page.evaluate(() => window.EventSource.instances.at(-1).url), '/app/qwenpaw/api/events');
  const before = state.liveLogQueries.length;
  await page.evaluate(() => window.EventSource.instances.find(s => s.url.endsWith('watch=logs')).emit('log-change', ''));
  assert.equal(state.liveLogQueries.length, before);
});

browserTest("initial and later SSE snapshots update the dashboard without another status GET", async t => {
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
  await page.goto(origin + prefix);
  await expect(page.locator("#statusBadge")).toContainText("运行中");
  assert.equal(await page.evaluate(() => window.EventSource.instances[0].url), "/app/qwenpaw/api/events?watch=metrics");
  assert.equal(state.statusRequests, 1, "keep the initial status GET");
  await emitSnapshot(page, { ...runningStatus, pid: "777" });
  await expect(page.locator("#overviewPid")).toHaveText("777");
  const cpu = page.locator('#statusCardMount .stat-value').first();
  const sample = { pid:777, startAt:runningStatus.startAt, cpu:17.3, memKB:2048 };
  await page.evaluate(m => window.EventSource.instances[0].emit('metrics', JSON.stringify(m)), sample);
  await expect(cpu).toHaveText('17.3%');
  await page.evaluate(m => window.EventSource.instances[0].emit('metrics', JSON.stringify(m)), {...sample, startAt:1, cpu:99});
  await expect(cpu).toHaveText('17.3%');
  const stopped = { ...runningStatus, running: false, pid: "0", startAt: 0, cpu: 0, memKB: 0 };
  await emitSnapshot(page, stopped);
  await expect(page.locator("#statusBadge")).toContainText("已停止", { timeout: 500 });
  await expect(page.locator("#btnStartService")).toBeVisible();
  assert.equal(state.statusRequests, 1, "snapshot contents must be used even when GET would still report running");
});

for (const outcome of ["success", "failure"]) {
  browserTest(`SSE wins over an in-flight status GET ${outcome}`, async t => {
    const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
    await page.goto(origin + prefix);
    await expect(page.locator("#overviewPid")).toHaveText("421");
    let release;
    state.statusCode = outcome === "failure" ? 503 : 200;
    state.status = { ...runningStatus, running: false, pid: "0" };
    state.statusHeld = new Promise(resolve => { release = resolve; });
    t.after(() => release());
    const requests = state.statusRequests;
    await page.evaluate(() => { window.pendingStatus = window.fetchStatus(); });
    await expect.poll(() => state.statusRequests).toBe(requests + 1);
    await emitSnapshot(page, { ...runningStatus, pid: "888" });
    await expect(page.locator("#overviewPid")).toHaveText("888");
    assert.equal(state.statusRequests, requests + 1);
    release();
    await page.evaluate(() => window.pendingStatus);
    await expect(page.locator("#overviewPid")).toHaveText("888");
    await expect(page.locator("#statusBadge")).toContainText("运行中");
  });
}

browserTest("malformed SSE bursts recover once and cannot turn unknown data into a stopped service", async t => {
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
  const pageErrors = [];
  page.on("pageerror", error => pageErrors.push(error.message));
  await page.goto(origin + prefix);
  await expect(page.locator("#overviewPid")).toHaveText("421");
  let release;
  state.statusCode = 503;
  state.statusHeld = new Promise(resolve => { release = resolve; });
  t.after(() => release());
  const before = state.statusRequests;
  await page.evaluate(() => {
    const stream = window.EventSource.instances[0];
    for (let index = 0; index < 20; index++) {
      for (const data of ["{", "null", "[]", '{"ok":false}', '{"success":true,"running":false}',
        '{"success":true,"installed":true,"running":"false"}']) stream.emit("snapshot", data);
      stream.emit("error");
    }
  });
  await expect.poll(() => state.statusRequests).toBe(before + 1);
  await expect(page.locator("#overviewPid")).toHaveText("421");
  release();
  await expect(page.locator("#statusBadge")).toContainText("状态不可达");
  await page.evaluate(() => {
    for (let index = 0; index < 20; index++) window.EventSource.instances[0].emit("snapshot", "{}");
  });
  assert.equal(state.statusRequests, before + 1, "failed recovery must not cause an event GET storm");
  state.statusCode = 200;
  await emitSnapshot(page, { ...runningStatus, pid: "999" });
  await expect(page.locator("#overviewPid")).toHaveText("999");
  assert.equal(state.statusRequests, before + 1);
  await page.evaluate(() => window.fireInterval(30000));
  await expect(page.locator("#overviewPid")).toHaveText("421");
  assert.equal(state.statusRequests, before + 2, "the 30-second fallback remains available after invalid events");
  assert.deepEqual(pageErrors, []);
});

browserTest("SSE disconnect and foreground resume recover while closed-stream callbacks stay ignored", async t => {
  const { page, state } = await fixture(t, { status: runningStatus, initScript: installStatusEventHost });
  await page.goto(origin + prefix);
  await expect(page.locator("#overviewPid")).toHaveText("421");
  state.status = { ...runningStatus, pid: 777 };
  const requests = state.statusRequests;
  await page.evaluate(() => window.EventSource.instances[0].emit("error"));
  await expect(page.locator("#overviewPid")).toHaveText("777");
  assert.equal(state.statusRequests, requests + 1, "disconnection performs a bounded status recovery");
  await emitSnapshot(page, { ...runningStatus, pid: "888" });
  await expect(page.locator("#overviewPid")).toHaveText("888");
  assert.equal(state.statusRequests, requests + 1, "reconnected snapshots do not GET again");

  await page.evaluate(() => window.setTestHidden(true));
  assert.equal(await page.evaluate(() => window.EventSource.instances[0].closed), true);
  await emitSnapshot(page, { ...runningStatus, pid: "111" });
  await page.evaluate(() => { window.fireInterval(15000); window.fireInterval(30000); });
  await expect(page.locator("#overviewPid")).toHaveText("888");
  assert.equal(state.statusRequests, requests + 1);
  state.status = { ...runningStatus, pid: 999 };
  await page.evaluate(() => window.setTestHidden(false));
  await expect(page.locator("#overviewPid")).toHaveText("999");
  assert.equal(state.statusRequests, requests + 2, "foreground resume refreshes immediately");
  assert.equal(await page.evaluate(() => window.EventSource.instances.length), 2);
  await emitSnapshot(page, { ...runningStatus, pid: "111" });
  await expect(page.locator("#overviewPid")).toHaveText("999");
  await emitSnapshot(page, { ...runningStatus, pid: "1000" }, 1);
  await expect(page.locator("#overviewPid")).toHaveText("1000");
  await page.evaluate(() => {
    window.dispatchEvent(new Event("pageshow"));
    window.dispatchEvent(new Event("pageshow"));
  });
  assert.equal(await page.evaluate(() => window.EventSource.instances.length), 2, "resume must not duplicate a live stream");
});

browserTest("install submits the version confirmed before an asynchronous selection change", async t => {
  const { page, state } = await fixture(t);
  await page.goto(origin + prefix + "#version");
  await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
  await page.locator("#btnUpgradeAction").click();
  await expect(page.locator("#modalMessage")).toContainText("2.0.0");
  await page.evaluate(() => window.pickVersion("1.9.0"));
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(() => state.posts.filter(post => post.path === "kernel/install").length).toBe(1);
  assert.equal(state.posts.find(post => post.path === "kernel/install").body.version, "2.0.0");
});


browserTest("polling uses low-frequency fallbacks and never requests release notes", async t => {
  const { page, state } = await fixture(t, { initScript: () => {
    window.pollingDelays = [];
    const original = window.setInterval;
    window.setInterval = (callback, delay, ...args) => {
      window.pollingDelays.push(delay);
      return original(callback, delay, ...args);
    };
  } });
  await page.goto(origin + prefix);
  const delays = await page.evaluate(() => window.pollingDelays);
  assert.ok(!delays.includes(15000), "overview must not poll logs");
  await page.locator("#mobileSectionNav [href='#logs']").click();
  await expect.poll(() => page.evaluate(() => window.pollingDelays.includes(15000))).toBe(true);
  assert.ok(delays.includes(30000), `missing 30-second status fallback: ${delays}`);
  assert.ok(!delays.includes(3500) && !delays.includes(5000), `legacy high-frequency polling remains: ${delays}`);
  assert.equal(state.requestPaths.some(request => request.path === "kernel/release-notes"), false);
});

browserTest("settings actions: two activations produce one confirmed reset and cancellation writes nothing", async t => {
  const { page, state } = await fixture(t, { status: runningStatus, proxy: { success: true, port: 43126, public: false, entryLogin: true } });
  await page.goto(origin + prefix + "#configuration");
  const reset = page.getByRole("button", { name: "重置 QwenPaw 管理员密码" });
  await reset.focus();
  await reset.evaluate(el => { el.click(); el.click(); });
  await page.getByLabel('新密码', {exact:true}).fill('fixture-one');
  await page.keyboard.press("Escape");
  await expect(reset).toBeFocused();
  assert.deepEqual(state.posts, []);
  let release;
  state.resetHeld = new Promise(resolve => { release = resolve; });
  try {
    await reset.evaluate(el => { el.click(); el.click(); });
    const first = page.getByLabel('新密码', {exact:true});
    await expect(first).toHaveValue('');
    await first.fill('fixture-two');
    const repeat = page.getByLabel('确认新密码');
    await repeat.fill('mismatch');
    await page.getByRole('button', {name:'保存新密码'}).click();
    assert.equal(await repeat.evaluate(el => el.validity.customError), true);
    assert.deepEqual(state.posts, []);
    await repeat.fill('fixture-two');
    await page.getByRole('button', {name:'保存新密码'}).dblclick();
    await expect.poll(() => state.posts.length).toBe(1);
    await expect(reset).toBeDisabled();
    await reset.dispatchEvent("click");
    await expect(page.locator("#customModal")).not.toHaveClass(/active/);
  } finally { release(); }
  await expect(reset).toBeEnabled();
  await expect(page.locator("#toast")).toContainText("密码已重置");
  assert.deepEqual(state.posts.map(p => p.path), ["kernel/admin-password"]);
});

browserTest("password recovery never confirms an HTTP refusal or a missing acknowledgement", async t => {
  const { page, state } = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix + '#configuration');
  for (const [status, success] of [[403, true], [503, false], [200, undefined]]) {
    state.resetReply = { status, json: {success} };
    await page.locator('#resetAuthBtn').click();
    const password = page.getByLabel('新密码', {exact: true});
    await expect(password).toHaveValue('');
    await password.fill('fixture');
    await page.getByLabel('确认新密码').fill('fixture');
    await page.getByRole('button', {name:'保存新密码'}).click();
    await expect(page.locator('#toast')).toContainText('密码重置未确认');
    await expect(page.locator('#resetAuthBtn')).toBeEnabled();
  }
  assert.deepEqual(state.posts.map(p => p.path), Array(3).fill('kernel/admin-password'));
});

browserTest("settings actions: reauthorization locks row actions and checks once without rewriting directories", async t => {
  const path = "/vol1/work";
  const { page, state } = await fixture(t, { status: runningStatus, projectDirs: [{path}], initScript: () => {
    window.authUrls = [];
    window.open = () => ({closed: false, close() {}, location: {replace: url => window.authUrls.push(url)}});
  } });
  await page.goto(origin + prefix + "#authorization");
  const reauth = page.getByRole("button", {name: "重新授权"});
  await reauth.evaluate(el => { el.click(); el.click(); });
  await expect(page.getByRole("button", {name: "移除绑定"})).toBeDisabled();
  await expect(page.locator("#refreshAuthorizedDirsBtn")).toBeDisabled();
  const urls = await page.evaluate(() => window.authUrls);
  assert.equal(urls.length, 1);
  const query = new URL(urls[0]).searchParams;
  assert.equal(query.get("path"), path);
  await page.evaluate(state => window.dispatchEvent(new MessageEvent("message", {origin: location.origin,
    data: {state, result: {state, status: "success", method: "authorizeUserFile"}}})), query.get("state"));
  await expect(reauth).toBeEnabled();
  await expect(page.locator("#toast")).toContainText("目录已重新授权，可读写");
  assert.deepEqual(state.posts, [{ path: "authorized-dirs/check", body: {path}, csrf: "test-token" }]);
  assert.deepEqual(state.projectWrites ?? [], []);
});

browserTest("settings actions: cancelling public access restores the private toggle without a write", async t => {
  const { page, state } = await fixture(t, {status: runningStatus});
  await page.goto(origin + prefix + "#configuration");
  const scope = page.locator("#webProxyPublic");
  await scope.check();
  await page.keyboard.press("Escape");
  await expect(scope).not.toBeChecked();
  await expect(scope).toBeFocused();
  assert.deepEqual(state.posts, []);
});

browserTest("settings actions: double removal keeps confirmation focus and only unbinds the selected directory", async t => {
  const keep = {path: "/vol1/keep", label: "保留"};
  const { page, state } = await fixture(t, {projectDirs: [keep, {path: "/vol1/old"}]});
  await page.goto(origin + prefix + "#authorization");
  const row = page.locator("#authorizedDirsList .info-row").filter({hasText: "/vol1/old"});
  const remove = row.getByRole("button", {name: "移除绑定"});
  await remove.focus();
  await remove.evaluate(el => { el.click(); el.click(); });
  await page.keyboard.press("Escape");
  await expect(remove).toBeFocused();
  assert.deepEqual(state.projectWrites ?? [], []);
  await remove.evaluate(el => { el.click(); el.click(); });
  await page.locator("#modalConfirmBtn").click();
  await expect(row).toHaveCount(0);
  assert.deepEqual(state.projectWrites, ["PUT"]);
  assert.deepEqual(state.projectDirs, [keep]);
});

browserTest('fresh and unknown proxy state never offer shell password or migration', async t => {
  const {page, state} = await fixture(t, {proxy: {port:43126, public:false, passwordConfigured:false}});
  for (const status of [false, null]) {
    state.proxy.passwordConfigured = status;
    await page.goto(origin + prefix + '#configuration'); await page.reload();
    await expect(page.locator('#proxyPasswordState')).toHaveText(status === false ? 'QwenPaw 原生账号' : '无法确认');
    await expect(page.getByRole('button', {name:/设置反代密码|更换反代密码|切换为原生登录/})).toHaveCount(0);
  }
  assert.deepEqual(state.posts, []);
});

browserTest('legacy proxy migration is explicit, cancellable, and only sends the native password once', async t => {
  const {page, state} = await fixture(t, {proxy:{port:43126, public:false, passwordConfigured:true}, viewport:{width:375,height:812}});
  await page.goto(origin + prefix + '#configuration');
  const open = page.getByRole('button', {name:'切换为原生登录'});
  await open.click();
  const dialog = page.getByRole('dialog', {name:'切换为原生登录'});
  await expect(dialog.getByRole('button', {name:'取消'})).toBeFocused();
  await dialog.getByLabel('QwenPaw 密码').fill('discarded');
  await page.keyboard.press('Escape'); await expect(open).toBeFocused();
  assert.deepEqual(state.posts, []);
  await open.click();
  const password = dialog.getByLabel('QwenPaw 密码');
  await expect(password).toHaveValue(''); await expect(password).toHaveAttribute('autocomplete','current-password');
  const value = ' native fixture 🔒 ';
  await dialog.getByLabel('QwenPaw 用户名').fill('owner'); await password.fill(value);
  await dialog.getByRole('button',{name:'验证并切换'}).click();
  await expect(page.locator('#proxyPasswordState')).toHaveText('QwenPaw 原生账号');
  assert.deepEqual(state.posts,[{path:'web-proxy/use-native-login',body:{confirm:true,username:'owner',password:value},csrf:'test-token'}]);
  await expect(open).toHaveCount(0);
});

browserTest('log panel filters by level and keyword without extra requests and copies filtered lines', async t => {
  const { page, state } = await fixture(t, {
    logs: [
      'INFO boot completed',
      'WARN slow mirror response',
      'ERROR gateway timeout on /api/status',
      'INFO heartbeat ok',
    ],
  });
  await page.addInitScript(() => {
    window.__copiedLogs = null;
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: async text => { window.__copiedLogs = text; } },
    });
  });
  await page.goto(origin + prefix + '#logs');
  const terminal = page.locator('#logTerminal');
  await expect(terminal).toContainText('INFO heartbeat ok');
  const beforeReads = state.logRequests;

  await openLogTools(page);
  await page.locator('.log-level-btn[data-level="warn"]').click();
  await expect(terminal).toContainText('WARN slow mirror response');
  await expect(terminal).toContainText('ERROR gateway timeout');
  await expect(terminal).not.toContainText('INFO boot completed');
  await expect(page.locator('#logFilterCount')).toHaveText('显示 2 / 4 行');

  await page.locator('#logSearchInput').fill('gateway');
  await expect(terminal).toContainText('ERROR gateway timeout');
  await expect(terminal).not.toContainText('WARN slow mirror response');
  await expect(page.locator('#logFilterCount')).toHaveText('显示 1 / 4 行');

  await page.locator('#copyLogsBtn').click();
  await expect(page.locator('#toast')).toContainText('已复制 1 行日志');
  assert.equal(await page.evaluate(() => window.__copiedLogs), 'ERROR gateway timeout on /api/status');

  await page.locator('#clearLogFilterBtn').click();
  await expect(terminal).toContainText('INFO boot completed');
  await expect(page.locator('#logFilterCount')).toHaveCount(0);
  assert.equal(state.logRequests, beforeReads);
});

browserTest('mirror dropdown supports keyboard navigation and restores focus on Escape', async t => {
  const { page } = await fixture(t, { status: runningStatus });
  await page.goto(origin + prefix);
  const trigger = page.locator('#mirrorToggleBtn');
  await trigger.focus();
  await page.keyboard.press('ArrowDown');
  const dropdown = page.locator('#mirrorDropdown');
  await expect(dropdown).toHaveClass(/show/);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Escape');
  await expect(dropdown).not.toHaveClass(/show/);
  await expect(trigger).toBeFocused();
});
