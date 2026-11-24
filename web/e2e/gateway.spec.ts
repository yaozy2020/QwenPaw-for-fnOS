import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { createHash, randomUUID } from "node:crypto";
import { appendFile, copyFile, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";

async function pickVersion(page: Page, version: string) {
  await page.locator('#targetVersion').click();
  await page.locator(`#versionOptions [data-version="${version}"]`).click();
}

async function openLogTools(page: Page) {
  const tools = page.locator('#logTools');
  if (!await tools.evaluate(element => (element as HTMLDetailsElement).open)) await tools.locator('summary').click();
}

const prefix = "/app/qwenpaw/";
const api = `${prefix}api/`;
test.use({ locale: "zh-CN" });

interface Metadata {
  appPort: number;
  transport: string;
  fixtureVersion: number;
  identityBoundary: string;
  managementTCP: string;
  adapter: { pid: number; starttime: number; exe: string; uid: number; role: string; uidMap: string };
  injectedIdentity: Record<string, string>;
  binaries: Record<string, { path: string; sha256: string }>;
  mirrors: { aliyun: string; tsinghua: string };
  files: Record<"auth" | "config" | "installLog" | "kernelLog" | "mirror" | "pin" | "servicePort" | "uiConfig" | "webProxyPort" | "venv", string>;
}
interface Forwarded {
  method: string;
  path: string;
  origin: string | null;
  upstreamHost: string;
  fetchSite: string | null;
  contentType: string | null;
  csrfHeader: boolean;
  csrfQuery: boolean;
  body: { version?: string; mirror?: string; port?: number };
  status: number;
  success: boolean | null;
  seq: number | null;
  instance: string | null;
  receiptLost: boolean;
  version?: string;
  mirror?: string;
}
interface Observations {
  indexRequests: string[];
  upgradePosts: Forwarded[];
  upgradeStatuses: Array<{ phase: string; seq: number; instance: string; upgrading: boolean }>;
  forwarded: Forwarded[];
}
async function metadata(request: APIRequestContext): Promise<Metadata> {
  const response = await request.get("/__e2e/meta");
  expect(response.ok()).toBeTruthy();
  return await response.json() as Metadata;
}
async function observations(request: APIRequestContext): Promise<Observations> {
  const response = await request.get("/__e2e/observations");
  expect(response.ok()).toBeTruthy();
  return await response.json() as Observations;
}
async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
}

test('monitor shows cold status before slow metadata, streams live process metrics and confirms start and stop', async ({ page, request }) => {
  const { files, binaries, appPort } = await metadata(request);
  expect(await exists(files.venv)).toBe(false);
  await mkdir(files.venv);
  const bin = join(files.venv, 'bin');
  await mkdir(bin);
  const python = join(bin, 'python3');
  const root = dirname(dirname(files.venv));
  const hold = join(root, 'hold-metadata');
  await writeFile(hold, '1');
  await copyFile(binaries.runtime.path, python);
  await writeFile(join(files.venv, 'installed'), '2.0.0\n');
  let statusReads = 0;
  page.on('request', r => { if (new URL(r.url()).pathname === `${api}status`) statusReads++; });
  await page.addInitScript(() => {
    const times: number[] = [];
    Object.assign(window, { monitorTicks: times });
    window.EventSource = class extends EventSource {
      constructor(url: string | URL, init?: EventSourceInit) {
        super(url, init);
        this.addEventListener('metrics', event => {
          const sample = JSON.parse((event as MessageEvent).data);
          if (sample.pid > 0 && sample.startAt > 0 && sample.memKB > 0 && typeof sample.cpu === 'number') times.push(performance.now());
        });
      }
    };
  });
  const state = page.locator('#statusCardMount .status-text');
  const phase = Date.now();
  await page.goto(prefix, {waitUntil:'domcontentloaded'});
  try {
    await expect(state).toHaveText('已停止', {timeout: 1500});
    console.log(`cold navigation to confirmed status: ${Date.now()-phase} ms`);
    await expect(page.locator('#overviewVersion')).toHaveText('待确认');
    await expect(page.locator('#overviewInstallButton')).toBeHidden();
    await expect.poll(() => exists(join(root, 'metadata-started')), {timeout:3000}).toBe(true);
  } finally { await rm(hold, {force: true}); }
  await expect(page.locator('#overviewVersion')).toHaveText('2.0.0', {timeout: 3000});
  const control = async (name: 'start' | 'stop', label: string, next: string) => {
    await page.getByRole('button', {name: label, exact:true}).click();
    const reply = page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === `${api}${name}`).then(response => ({response, at:Date.now()}));
    await page.locator('#modalConfirmBtn').click();
    const {response, at} = await reply;
    expect(response.status()).toBe(200);
    expect((await response.json()).success).toBe(true);
    await expect(state).toHaveText(next, {timeout: 1000});
    console.log(`${name} response to confirmed status: ${Date.now()-at} ms`);
  };
  await control('start', '启动服务', '运行中');
  await expect.poll(async () => {
    const response = await request.get(`http://127.0.0.1:${appPort}/api/version`).catch(() => null);
    return response?.ok() ? (await response.json()).version : null;
  }).toBe('2.0.0');
  const shownPid = Number(await page.locator('#overviewPid').innerText());
  expect(shownPid).toBeGreaterThan(1);
  expect(await exists(`/proc/${shownPid}/stat`)).toBe(true);
  const clock = page.locator('#statusCardMount .stat').filter({hasText:'运行时间'}).locator('.stat-value');
  const initial = await clock.innerText();
  const ticks = () => page.evaluate(() => (window as Window & {monitorTicks?: number[]}).monitorTicks || []);
  await expect.poll(async () => (await ticks()).length, {timeout: 4000}).toBeGreaterThan(0);
  const before = statusReads;
  await expect.poll(async () => (await ticks()).length, {timeout: 4000}).toBeGreaterThanOrEqual(3);
  const [a,b,c] = await ticks();
  console.log(`monitor frame intervals: ${(b-a).toFixed(0)}, ${(c-b).toFixed(0)} ms`);
  for (const gap of [b-a,c-b]) { expect(gap).toBeGreaterThan(500); expect(gap).toBeLessThan(1600); }
  await expect(clock).not.toHaveText(initial);
  await expect(page.locator('#statusUptime')).toContainText(await clock.innerText());
  await expect(page.locator('#statusCardMount')).toContainText(/\d+\.\d%/);
  expect(statusReads).toBe(before);
  const manual = page.waitForResponse(r => new URL(r.url()).pathname === `${api}status`);
  const refreshAt = Date.now();
  await page.getByRole('button', {name: '刷新服务状态'}).click();
  const refreshed = await manual;
  expect(refreshed.status()).toBe(200);
  const snapshot = await refreshed.json();
  expect(String(snapshot.pid)).toBe(String(shownPid));
  await expect(page.locator('#refreshDashboardBtn')).toBeEnabled({timeout: 1000});
  console.log(`manual refresh click through render: ${Date.now()-refreshAt} ms`);
  expect(statusReads).toBe(before + 1);
  // Use the real start time, advancing only the browser's clock.
  for (const [elapsed, text] of [[86399, '23:59:59'], [86400, '1天 00:00'], [86459, '1天 00:00'], [86460, '1天 00:01']] as const) {
    await page.clock.setFixedTime((snapshot.startAt + elapsed) * 1000);
    await expect(clock).toHaveText(text);
    await expect(page.locator('#statusUptime')).toHaveText(`已运行 ${text}`);
  }
  expect(statusReads).toBe(before + 1);
  await control('stop', '停止服务', '已停止');
  expect(await exists(`/proc/${shownPid}/stat`)).toBe(false);
  await expect(page.locator('#statusUptime')).toBeEmpty();
  await page.locator('#btnUninstallKernel').click();
  await page.locator('#modalConfirmBtn').click();
  await expect(state).toHaveText('内核未安装');
  expect(await exists(files.venv)).toBe(false);
});

test('overview exposes a single in-panel release picker and offline recovery instructions', async ({page}) => {
  await page.goto(prefix);
  await expect(page).toHaveTitle('概览 · QwenPaw');
  const picker = page.locator('#targetVersion');
  await expect(picker).toBeVisible();
  await expect(page.locator('#btnChangeServicePort, #authorizedDirsList')).toHaveCount(0);
  if (process.env.QWENPAW_E2E_INCLUDE_VERIFIED === "1") await expect(picker).toContainText('2.3.0.post1');
  for (const width of [390, 1280]) {
    await page.setViewportSize({ width, height: 844 });
    if (process.env.QWENPAW_UI_ARTIFACT_DIR) {
      await page.screenshot({path: join(process.env.QWENPAW_UI_ARTIFACT_DIR, `overview-${width}.png`), fullPage: width > 390, animations: "disabled"});
    }
    if (process.env.QWENPAW_UI_ARTIFACT_DIR && width === 390) {
      await page.setViewportSize({ width, height: 1000 });
    }
    await picker.click();
    const list = page.getByRole('listbox', { name: '目标版本' });
    await expect(list).not.toContainText(/推荐|fnOS 已验证|官方认证/);
    const bounds = await list.boundingBox();
    expect(bounds).not.toBeNull();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    expect(await list.getByRole('option').first().evaluate(el => el.getBoundingClientRect().height)).toBeGreaterThanOrEqual(44);
    if (process.env.QWENPAW_UI_ARTIFACT_DIR && width === 390) {
      await page.locator('#btnUpgradeAction').evaluate(element => element.scrollIntoView({block: 'center'}));
    }
    if (process.env.QWENPAW_UI_ARTIFACT_DIR) {
      const path = join(process.env.QWENPAW_UI_ARTIFACT_DIR, `release-picker-${width}.png`);
      if (width === 390) await page.screenshot({path, animations: "disabled"});
      else await page.locator('#version').screenshot({path, animations: "disabled"});
    }
    await page.keyboard.press('Escape');
    await expect(picker).toBeFocused();
    if (process.env.QWENPAW_UI_ARTIFACT_DIR && width === 390) {
      await page.setViewportSize({ width, height: 844 });
    }
  }
  await pickVersion(page, '2.1.0b1');
  await expect(picker).toContainText('测试版');
  await expect(page.locator('#versionOrder')).toHaveText('按发布时间从新到旧');
  await pickVersion(page, '2.0.0');
  await expect(picker).toContainText('正式版');
  await expect(page).toHaveURL(/#overview$/);
  await page.getByLabel('安装前备份已有数据').check();
  await page.locator('#btnUpgradeAction').click();
  await expect(page.locator('#customModal')).toContainText('备份或校验失败将中止安装');
  await page.locator('#modalCancelBtn').click();
  await page.locator('[data-section="authorization"]').click();
  await expect(page.locator('#openBackupBtn')).toBeDisabled();
  await page.getByText('安装失败，如何离线恢复？').click();
  const recoveryFile = page.getByText('preinstall-data.tar', {exact:true});
  await expect(recoveryFile).toBeVisible();
  await page.locator('[data-section="configuration"]').click();
  await expect(page).toHaveTitle('配置 · QwenPaw');
  await expect(page.locator('#btnChangeServicePort')).toBeVisible();
  await page.locator('#themeToggleBtn').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('#themeToggleBtn')).toHaveAttribute('data-theme-mode', 'dark');
  if (process.env.QWENPAW_UI_ARTIFACT_DIR) {
    await page.screenshot({path: join(process.env.QWENPAW_UI_ARTIFACT_DIR, 'configuration-dark-1280.png'), fullPage: true, animations: "disabled"});
  }
  await page.locator('#localeToggleBtn').click();
  await expect(page).toHaveTitle('Settings · QwenPaw');
  await expect(page.locator('html')).toHaveAttribute('lang', 'en');
  await expect(page.locator('#localeToggleBtn')).toHaveAttribute('data-locale', 'en');
  if (process.env.QWENPAW_UI_ARTIFACT_DIR) {
    await page.screenshot({path: join(process.env.QWENPAW_UI_ARTIFACT_DIR, 'configuration-en-dark-1280.png'), fullPage: true, animations: "disabled"});
  }
  await page.locator('#localeToggleBtn').click();
  await expect(page).toHaveTitle('配置 · QwenPaw');
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await page.locator('#themeToggleBtn').click();
  await expect(page.locator('#themeToggleBtn')).toHaveAttribute('data-theme-mode', 'light');
  await page.locator('#themeToggleBtn').click();
  await expect(page.locator('#themeToggleBtn')).toHaveAttribute('data-theme-mode', 'dark');
  await page.locator('#themeModeGroup [data-theme-option="system"]').click();
  await expect(page.locator('#themeToggleBtn')).toHaveAttribute('data-theme-mode', 'system');
  await page.goBack();
  await expect(recoveryFile).toBeVisible();
});

test("packaged panel uses Vue handlers and follows a real log append before fallback polling", async ({ page, request }) => {
  const fixture = await metadata(request);
  await page.goto(prefix);
  await expect(page.locator('#statusCardMount .status-text')).toHaveText('内核未安装');
  expect(await page.locator('[onclick], [onchange]').count()).toBe(0);
  expect(await page.evaluate(() => 'checkOrUpgrade' in window)).toBe(false);
  const watching = page.waitForResponse(r => r.url().endsWith(`${api}events?watch=logs`) && r.status() === 200);
  await page.locator('[data-section="logs"]').click();
  await watching;
  await page.waitForTimeout(2200); // Allow the first 2s metadata sample before changing the file.
  const line = `tail-${randomUUID()}`;
  await appendFile(fixture.files.kernelLog, `\n${line}\n`);
  await expect(page.locator('#logTerminal')).toContainText(line, { timeout: 7000 });
  const statusOnly = page.waitForRequest(r => r.url().endsWith(`${api}events`));
  await openLogTools(page);
  await page.locator('#autoRefreshCheck').uncheck();
  await statusOnly;
});

test("Gateway transport exposes the prefixed console over real UDS and enforces real CSRF", async ({ page, request }, testInfo) => {
  const fixture = await metadata(request);
  testInfo.annotations.push({ type: "transport", description: fixture.transport },
    { type: "identity-boundary", description: fixture.identityBoundary },
    { type: "supervisor-sha256", description: fixture.binaries.supervisor.sha256 });
  console.log("Gateway test provenance:", JSON.stringify(fixture));
  expect(fixture.transport).toBe("HTTP -> AF_UNIX/SOCK_STREAM -> release supervisor Gateway");
  expect(fixture.identityBoundary).toContain("NOT fnOS identity/session acceptance");
  expect(fixture.fixtureVersion).toBe(1);
  expect(fixture.managementTCP).toBe("not forwarded");
  expect(fixture.adapter).toMatchObject({ uid: 0, role: "test-admin" });
  expect(fixture.adapter.pid).toBeGreaterThan(1);
  expect(fixture.adapter.starttime).toBeGreaterThan(0);
  expect(fixture.adapter.exe).toMatch(/^\/tmp\/qpg-peer-[^/]+\/python3$/);
  expect(fixture.adapter.uidMap).toBeTruthy();
  expect(fixture.injectedIdentity["X-Trim-Isadmin"]).toBe("true");
  for (const binary of Object.values(fixture.binaries)) {
    expect(binary.sha256).toMatch(/^[a-f0-9]{64}$/);
    expect(createHash("sha256").update(await readFile(binary.path)).digest("hex")).toBe(binary.sha256);
  }
  const response = await page.goto(prefix);
  expect(response?.status(), "Gateway console entry must exist").toBe(200);
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("内核未安装");
  await expect(page.locator("#releaseNotesDialog")).toHaveCount(0);
  const csrf = (await page.context().cookies()).find(cookie => cookie.name === "__Host-qwenpaw_csrf");
  expect(csrf).toMatchObject({ secure: true, path: "/", sameSite: "Lax" });
  expect(csrf?.value).toMatch(/^\d+\.[a-f0-9]{64}\.[a-f0-9]{64}$/);
  const before = await readFile(fixture.files.mirror, "utf8");
  const rejections = await page.evaluate(async ({ api, token, mirror }) => {
    const results: number[] = [];
    for (const headers of [{}, { "X-CSRF-Token": "wrong-token" }]) {
      const response = await fetch(`${api}mirror`, { method: "POST", headers: { "Content-Type": "application/json", ...headers }, body: JSON.stringify({ mirror: "https://denied.invalid/simple" }) });
      results.push(response.status);
    }
    const queryOnly = await fetch(`${api}port?_csrf=${token}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ port: 0 }) });
    results.push(queryOnly.status);
    const headerWithQuery = await fetch(`${api}mirror?_csrf=ignored`, { method: "POST", headers: { "Content-Type": "application/json", "X-CSRF-Token": token }, body: JSON.stringify({ mirror }) });
    return { results, headerWithQuery: headerWithQuery.status };
  }, { api, token: csrf!.value, mirror: before.trim() });
  expect(rejections).toEqual({ results: [403, 403, 403], headerWithQuery: 200 });
  await page.context().addCookies([{ ...csrf!, value: "forged-pair" }]);
  expect(await page.evaluate(async api => (await fetch(`${api}mirror`, { method: "POST", headers: { "Content-Type": "application/json", "X-CSRF-Token": "forged-pair" }, body: JSON.stringify({ mirror: "https://denied.invalid/simple" }) })).status, api)).toBe(403);
  await page.context().addCookies([csrf!]);
  const attackerServer = createServer((_request, response) => {
    response.writeHead(200, { "Content-Type": "text/html" });
    response.end("<!doctype html><title>Local cross-site origin</title>");
  });
  await new Promise<void>(resolve => attackerServer.listen(0, "127.0.0.1", resolve));
  const attackerOrigin = `http://localhost:${(attackerServer.address() as AddressInfo).port}`;
  const attacker = await page.context().newPage();
  try {
    await attacker.goto(attackerOrigin);
    await attacker.evaluate(async ({ url, token }) => {
      await fetch(url, { method: "POST", mode: "no-cors", body: new URLSearchParams({ _csrf: token, mirror: "https://denied.invalid/simple" }) });
    }, { url: new URL(`${api}mirror`, page.url()).href, token: csrf!.value });
    await expect.poll(async () => (await observations(request)).forwarded.some(item => item.method === "POST" && item.path === `${api}mirror` && item.origin === attackerOrigin && item.fetchSite === "cross-site" && item.status === 403)).toBe(true);
  } finally {
    await attacker.close();
    await new Promise<void>((resolve, reject) => attackerServer.close(error => error ? reject(error) : resolve()));
  }
  expect(await readFile(fixture.files.mirror, "utf8")).toBe(before);
  expect(await exists(fixture.files.venv)).toBe(false);
  const responseWithForgedTrim = await request.get(`${api}status`, { headers: { "X-Trim-Userid": "untrusted", "X-Trim-Isadmin": "false", "X-Supervisor-Token": "browser-forgery", "X-Forwarded-Proto": "untrusted" } });
  expect(responseWithForgedTrim.status()).toBe(200);
  expect((await responseWithForgedTrim.json()).installed).toBe(false);
  for (const path of ["/status", "/api/status", "/__qwenpaw_gateway_ready", "/cgi/ThirdParty/qwenpaw/index.cgi/", "/cgi/ThirdParty/qwenpaw/api.cgi?action=status"]) {
    expect((await request.get(path)).status(), `must not expose alternate management ingress: ${path}`).toBe(404);
  }
});

test("Web proxy port UI handles validation, conflict and successful automatic selection", async ({ page }) => {
  const writes: Array<{ port: number; csrf: string }> = [];
  await page.route("**/api/web-proxy/port**", async route => {
    const request = route.request();
    if (request.method() === "GET") {
      await route.fulfill({ json: { success: true, port: 43126, public: false } });
      return;
    }
    const { port } = request.postDataJSON() as { port: number };
    writes.push({ port, csrf: request.headers()["x-csrf-token"] || "" });
    await route.fulfill(port === 0
      ? { json: { success: true, port: 43128 } }
      : { status: 409, json: { success: false, message: "端口被占用" } });
  });

  await page.goto(prefix + "#configuration");
  await expect(page.locator("#valWebProxyPort")).toHaveText("43126");
  const button = page.getByRole("button", { name: "修改反代端口" });

  await button.click();
  await page.locator("#modalInput").fill("100");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#toast")).toContainText("端口无效");
  expect(writes).toHaveLength(0);

  await button.click();
  await page.locator("#modalInput").fill("43127");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#modalTitle")).toHaveText("切换 Web 反代端口");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#toast")).toContainText("端口被占用");
  await expect(page.locator("#valWebProxyPort")).toHaveText("43126");

  await button.click();
  await page.locator("#modalInput").fill("0");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#modalTitle")).toHaveText("切换 Web 反代端口");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#valWebProxyPort")).toHaveText("43128");
  expect(writes.map(item => item.port)).toEqual([43127, 0]);
  expect(writes.every(item => item.csrf.length > 0)).toBe(true);
});

for (const responseCase of [
  { label: "HTTP 403 despite success=true", status: 403, body: { success: true } },
  { label: "HTTP 500 despite success=true", status: 500, body: { success: true } },
  { label: "HTTP 200 without success", status: 200, body: {} },
]) {
  test(`management actions never report success for ${responseCase.label}`, async ({ page }) => {
    const posts: string[] = [];
    let validToggle = false;
    await page.route(`**${api}**`, async route => {
      const request = route.request();
      const path = new URL(request.url()).pathname.slice(api.length);
      if (request.method() === "GET") {
        const data = path === "status"
          ? { success: true, installed: true, running: false, version: "2.0.0", port: 18088, authStatus: "protected", install: { running: false } }
          : path === "logs" ? { success: true, logs: [] }
            : path === "kernel/versions" ? { success: true, latest_stable: "2.1.0", recommended_version: "2.1.0", versions: [{ version: "2.1.0", is_beta: false }] }
              : path === "mirror" ? { success: true, mirror: "" }
                : path === "web-proxy/port" ? { success: true, port: 43126, public: false }
                  : { success: true, upgrading: false };
        await route.fulfill({ json: data });
        return;
      }
      posts.push(path);
      await route.fulfill(validToggle && path === "web-proxy/port"
        ? { json: { success: true, port: 43126, public: true } }
        : { status: responseCase.status, json: { ...responseCase.body, port: 43127, seq: 31, instance: "worker-a" } });
    });

    await page.goto(prefix);
    await expect(page.locator("#btnStartService")).toBeVisible();
    await page.locator('[data-section="configuration"]').click();
    await page.getByRole("button", { name: "修改端口" }).click();
    await page.locator("#modalInput").fill("23001");
    await page.locator("#modalConfirmBtn").click();
    await page.locator("#modalConfirmBtn").click();
    await expect(page.locator("#toast")).toContainText("修改端口失败");
    await expect(page.locator("#valPort")).toHaveText("18088");

    await page.getByRole("button", { name: "修改反代端口" }).click();
    await page.locator("#modalInput").fill("43127");
    await page.locator("#modalConfirmBtn").click();
    await page.locator("#modalConfirmBtn").click();
    await expect(page.locator("#toast")).toContainText("修改反代端口失败");
    await expect(page.locator("#valWebProxyPort")).toHaveText("43126");

    await page.getByRole("button", { name: "重置 QwenPaw 管理员密码" }).click();
    await page.getByLabel('新密码', {exact:true}).fill('fixture');
    await page.getByLabel('确认新密码').fill('fixture');
    await page.getByRole('button', {name:'保存新密码'}).click();
    await expect(page.locator("#toast")).toContainText("密码重置未确认");
    await page.locator('[data-section="overview"]').click();
    await page.locator("#btnStartService").click();
    await page.locator("#modalConfirmBtn").click();
    await expect(page.locator("#toast")).toContainText("操作失败");

    await expect(page.locator('#targetVersion')).toBeVisible();
    await page.locator("#mirrorToggleBtn").click();
    await page.locator("#mirrorDropdown .history-item").first().click();
    await expect(page.locator("#toast")).toContainText("下载源保存失败");
    await page.locator("#btnUninstallKernel").click();
    await page.locator("#modalConfirmBtn").click();
    await expect(page.locator("#toast")).toContainText("卸载请求失败");

    await page.getByRole('button', { name: '检查更新', exact: true }).click();
    await pickVersion(page, '2.1.0');
    await page.locator("#btnUpgradeAction").click();
    await page.locator("#modalConfirmBtn").click();
    await expect(page.locator("#toast")).toContainText("安装请求失败");
    await expect(page.locator("#upgradeModal")).not.toHaveClass(/active/);
    expect(posts).toEqual(["port", "web-proxy/port", "kernel/admin-password", "start", "mirror", "kernel/uninstall", "kernel/install"]);

    await page.locator('[data-section="configuration"]').click();
    validToggle = true;
    await page.locator("#webProxyPublic").check();
    await page.locator("#modalConfirmBtn").click();
    await expect(page.locator("#toast")).toContainText("已允许外部访问 Web 反代");
    await expect(page.locator("#webProxyPublic")).toBeChecked();
  });
}

test("version action stays disabled until a successful version list has loaded", async ({ page }) => {
  let releaseVersions!: () => void;
  const versionsHeld = new Promise<void>(resolve => { releaseVersions = resolve; });
  let installPosts = 0;
  await page.route(`**${api}**`, async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname.slice(api.length);
    if (request.method() === "POST" && path === "kernel/install") installPosts++;
    if (request.method() === "GET" && path === "kernel/versions") {
      await versionsHeld;
      await route.fulfill({ status: 503, json: { success: true, versions: [{ version: "2.9.9", is_beta: false }] } });
      return;
    }
    const data = path === "status"
      ? { success: true, installed: false, running: false, port: 18088, install: { running: false } }
      : path === "logs" ? { success: true, logs: [] }
        : path === "mirror" ? { success: true, mirror: "" }
          : path === "web-proxy/port" ? { success: true, port: 43126, public: false }
            : { success: true, upgrading: false };
    await route.fulfill({ json: data });
  });

  await page.goto(prefix + "#version");
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  releaseVersions();
  await expect(page.locator("#versionSourceStatus")).toBeVisible();
  await expect(page.locator("#btnUpgradeAction")).toBeDisabled();
  await page.locator("#btnUpgradeAction").evaluate(button => (button as HTMLButtonElement).click());
  await expect(page.locator("#customModal")).not.toHaveClass(/active/);
  expect(installPosts).toBe(0);
});

test("lost acceptance attaches without replay, polls identified logs and times out a stuck task", async ({ page }) => {
  const identity = { seq: 31, instance: "worker-a" };
  const logQueries: Array<{ seq: string | null; instance: string | null }> = [];
  let statusPolls = 0;
  let installPosts = 0;
  let active = false;
  let releaseInstall!: () => void;
  let signalInstall!: () => void;
  const installHeld = new Promise<void>(resolve => { releaseInstall = resolve; });
  const installStarted = new Promise<void>(resolve => { signalInstall = resolve; });
  await page.route(`**${api}**`, async route => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname.slice(api.length);
    if (request.method() === "POST" && path === "kernel/install") {
      installPosts++;
      signalInstall();
      await installHeld;
      active = true;
      await route.abort("failed");
      return;
    }
    if (request.method() === "GET" && path === "kernel/install-log") {
      const query = { seq: url.searchParams.get("seq"), instance: url.searchParams.get("instance") };
      logQueries.push(query);
      await route.fulfill({ status: query.seq === String(identity.seq) && query.instance === identity.instance ? 200 : 404,
        json: query.seq === String(identity.seq) && query.instance === identity.instance
          ? { success: true, logs: ["matching task log"] }
          : { success: false, logs: ["private other task log"] } });
      return;
    }
    if (request.method() === "GET" && path === "kernel/install-status") {
      statusPolls++;
      await route.fulfill({ json: active
        ? { success: true, upgrading: true, running: true, ...identity, phase: "installing" }
        : { success: true, upgrading: false, running: false, seq: 30, instance: "old-worker", phase: "done" } });
      return;
    }
    const data = path === "status"
      ? { success: true, installed: false, running: false, port: 18088, install: { running: false } }
      : path === "logs" ? { success: true, logs: [] }
        : path === "kernel/versions" ? { success: true, latest_stable: "2.0.0", recommended_version: "2.0.0", versions: [{ version: "2.0.0", is_beta: false }] }
          : path === "mirror" ? { success: true, mirror: "" }
            : path === "web-proxy/port" ? { success: true, port: 43126, public: false }
              : { success: true };
    await route.fulfill({ json: data });
  });

  await page.goto(prefix + "#version");
  await expect(page.locator("#btnUpgradeAction")).toBeEnabled();
  await page.locator("#btnUpgradeAction").click();
  await page.locator("#modalConfirmBtn").click();
  try {
    await installStarted;
    await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
    await page.waitForTimeout(1_800);
    expect(logQueries).toEqual([]);
    expect(installPosts).toBe(1);
  } finally {
    releaseInstall();
  }
  await expect(page.locator("#upTitle")).toHaveText("QwenPaw 安装任务");
  await expect.poll(() => logQueries.length).toBeGreaterThan(0);
  expect(logQueries.every(query => query.seq === String(identity.seq) && query.instance === identity.instance)).toBe(true);
  await expect(page.locator("#upTerminal")).toContainText("matching task log");
  expect(installPosts).toBe(1);
  await page.evaluate(() => {
    const now = Date.now;
    Date.now = () => now() + 10 * 60 * 1000 + 1;
  });
  await expect(page.locator("#upTitle")).toHaveText("任务仍未结束", { timeout: 5_000 });
  const stoppedAt = statusPolls;
  await page.waitForTimeout(1_500);
  expect(statusPolls).toBe(stoppedAt);
  expect(installPosts).toBe(1);
});

test("real Gateway Web proxy port UI reads, rejects conflict and switches listeners", async ({ page, request }) => {
  test.skip(process.env.QWENPAW_E2E_WEB_PROXY_PORT !== "0", "requires an enabled automatic Web proxy fixture");
  const fixture = await metadata(request);
  const previousPort = Number((await readFile(fixture.files.webProxyPort, "utf8")).trim());
  expect(previousPort).toBeGreaterThan(0);
  const initialRead = page.waitForResponse(response =>
    response.request().method() === "GET" && new URL(response.url()).pathname === `${api}web-proxy/port`);
  await page.goto(prefix + "#configuration");
  expect((await initialRead).status()).toBe(200);
  await expect(page.locator("#valWebProxyPort")).toHaveText(String(previousPort));
  expect((await request.get(`http://127.0.0.1:${previousPort}/api/status`)).status()).toBe(401);
  const waiting = await request.get(`http://127.0.0.1:${previousPort}/`, { headers: { Accept: 'text/html' } });
  expect(waiting.status()).toBe(503);
  expect(await waiting.text()).toContain('重新检查');

  const allocated = await request.get("/__e2e/allocate-port");
  const { port: occupiedPort } = await allocated.json() as { port: number };
  const button = page.getByRole("button", { name: "修改反代端口" });
  await button.click();
  await page.locator("#modalInput").fill(String(occupiedPort));
  await page.locator("#modalConfirmBtn").click();
  const conflict = page.waitForResponse(response =>
    response.request().method() === "POST" && new URL(response.url()).pathname === `${api}web-proxy/port`);
  await page.locator("#modalConfirmBtn").click();
  expect((await conflict).status()).toBe(409);
  await expect(page.locator("#valWebProxyPort")).toHaveText(String(previousPort));
  expect(Number((await readFile(fixture.files.webProxyPort, "utf8")).trim())).toBe(previousPort);

  await button.click();
  await page.locator("#modalInput").fill("0");
  await page.locator("#modalConfirmBtn").click();
  const switched = page.waitForResponse(response =>
    response.request().method() === "POST" && new URL(response.url()).pathname === `${api}web-proxy/port`);
  await page.locator("#modalConfirmBtn").click();
  const switchedResponse = await switched;
  expect(switchedResponse.status()).toBe(200);
  const { port: newPort } = await switchedResponse.json() as { port: number };
  expect(newPort).toBeGreaterThan(0);
  expect(newPort).not.toBe(previousPort);
  await expect(page.locator("#valWebProxyPort")).toHaveText(String(newPort));
  expect(Number((await readFile(fixture.files.webProxyPort, "utf8")).trim())).toBe(newPort);
  expect((await request.get(`http://127.0.0.1:${newPort}/api/status`)).status()).toBe(401);
  expect((await request.get(`http://127.0.0.1:${newPort}/api/version`)).status()).toBe(503);
  await expect(page.locator('#proxyPasswordState')).toHaveText('QwenPaw 原生账号');
  await expect(page.getByRole('button', {name:/设置反代密码|切换为原生登录/})).toHaveCount(0);
  // TLS legacy-password upgrade coverage lives in scripts/test-entry-migration.py with a real native account.
  await expect.poll(async () => {
    try {
      await request.get(`http://127.0.0.1:${previousPort}/api/status`, { timeout: 1000 });
      return true;
    } catch {
      return false;
    }
  }).toBe(false);
});

test("real Gateway Chromium flow persists, restores accepted install progress, handles lost acceptance and uninstalls", async ({ page, request }, testInfo) => {
  test.setTimeout(240_000);
  const fixture = await metadata(request);
  testInfo.annotations.push({ type: "supervisor-sha256", description: fixture.binaries.supervisor.sha256 }, { type: "identity-boundary", description: fixture.identityBoundary });
  const pageErrors: string[] = [];
  let reloadPageErrors: string[] | null = null;
  const consoleErrors: Array<{ text: string; url: string }> = [];
  const failedRequests: Array<{ method: string; url: string; errorText: string }> = [];
  const establishedEventStreams = new Set<string>();
  const browserRequests: string[] = [];
  const installStatusRequests: number[] = [];
  const installLogUrls: string[] = [];
  const csrfCredentials: Array<{ header: string; query: string | null }> = [];
  const installAttemptResults = new Map<object, { responseStatus?: number; failed: boolean }>();
  page.on("pageerror", error => {
    if (reloadPageErrors) reloadPageErrors.push(error.message);
    else pageErrors.push(error.message);
  });
  page.on("console", message => {
    if (message.type() === "error") consoleErrors.push({ text: message.text(), url: message.location().url });
  });
  page.on("requestfailed", request => {
    const attempt = installAttemptResults.get(request);
    if (attempt) attempt.failed = true;
    failedRequests.push({ method: request.method(), url: request.url(), errorText: request.failure()?.errorText || "unknown" });
  });
  page.on("response", response => {
    const attempt = installAttemptResults.get(response.request());
    if (attempt) attempt.responseStatus = response.status();
    const url = new URL(response.url());
    if (response.request().method() === "GET" && response.status() === 200
      && url.pathname === `${api}events`
      && response.headers()["content-type"]?.startsWith("text/event-stream")) {
      establishedEventStreams.add(response.url());
    }
  });
  page.on("request", request => {
    browserRequests.push(request.url());
    if (request.method() === "GET" && new URL(request.url()).pathname === `${api}kernel/install-status`) {
      installStatusRequests.push(Date.now());
    }
    if (request.method() === "GET" && new URL(request.url()).pathname === `${api}kernel/install-log`) {
      installLogUrls.push(request.url());
    }
    if (request.method() === "POST" && new URL(request.url()).pathname === `${api}kernel/install`) {
      installAttemptResults.set(request, { failed: false });
    }
    if (request.method() === "POST") csrfCredentials.push({ header: request.headers()["x-csrf-token"] || "", query: new URL(request.url()).searchParams.get("_csrf") });
  });
  await page.goto(prefix);
  const origin = new URL(page.url()).origin;
  const csrf = (await page.context().cookies()).find(cookie => cookie.name === "__Host-qwenpaw_csrf");
  expect(csrf).toBeDefined();
  await page.context().addCookies([{ name: "qwenpaw_csrf", value: "sibling-domain-cookie", url: new URL("/", page.url()).href, sameSite: "Lax" }]);
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("内核未安装");
  await expect(page.locator("#valVersion")).toHaveText("未安装");
  await page.locator("#targetVersion").click();
  await expect(page.getByRole("option").first()).toContainText("v2.1.0b1");
  await expect(page.locator("#mirrorLabel")).toContainText("阿里云镜像源");
  await page.locator('[data-section="logs"]').click();
  await expect(page.locator("#logTerminal")).toContainText("browser-e2e-runtime-log");
  await page.locator('[data-section="configuration"]').click();
  await page.locator('[data-section="overview"]').click();
  await expect(page.locator('#targetVersion')).toBeVisible();
  await page.locator("#mirrorToggleBtn").click();
  await page.locator("#mirrorDropdown").getByText("清华大学镜像源", { exact: true }).click();
  await expect(page.locator("#mirrorLabel")).toContainText("清华大学镜像源");
  await expect.poll(async () => (await readFile(fixture.files.mirror, "utf8")).trim()).toBe(fixture.mirrors.tsinghua);
  const mirrorReload = await page.context().newPage();
  await mirrorReload.goto(prefix);
  await expect(mirrorReload.locator("#mirrorLabel")).toContainText("清华大学镜像源");
  await mirrorReload.close();

  await pickVersion(page, "2.0.0");
  const acceptedResponse = page.waitForResponse(response => response.request().method() === "POST" && new URL(response.url()).pathname === `${api}kernel/install`);
  await page.locator("#btnUpgradeAction").click();
  await expect(page.locator("#modalTitle")).toHaveText("安装内核确认");
  await page.locator("#modalConfirmBtn").click();
  const accepted = await acceptedResponse;
  expect(accepted.status()).toBe(200);
  expect(await accepted.json()).toMatchObject({ success: true, seq: expect.any(Number), instance: expect.any(String) });
  await expect(page.locator("#upTitle")).toHaveText("安装已顺利完成！", { timeout: 45_000 });
  await expect.poll(async () => (await readFile(`${fixture.files.venv}/installed`, "utf8")).trim()).toBe("2.0.0");
  await expect.poll(async () => (await readFile(fixture.files.pin, "utf8")).trim()).toBe("2.0.0");
  await expect(page.locator("#upTerminal")).toContainText("Successfully installed qwenpaw-2.0.0");
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  await expect(page.locator("#valVersion")).toHaveText("2.0.0");
  const health = await request.get(`http://127.0.0.1:${fixture.appPort}/api/version`);
  expect(health.ok()).toBeTruthy();
  expect(await health.json()).toEqual({ version: "2.0.0" });
  expect((await observations(request)).upgradePosts).toHaveLength(1);

  const controlRoot = dirname(dirname(fixture.files.venv));
  const slowPip = join(controlRoot, "slow-pip");
  const pipStarted = join(controlRoot, "pip-started");
  await page.locator("#btnUpClose").click();
  await writeFile(slowPip, "hold accepted task for browser recovery\n");
  await rm(pipStarted, { force: true });
  await pickVersion(page, "2.1.0b1");
  const recoveryAcceptance = page.waitForResponse(response => response.request().method() === "POST" && new URL(response.url()).pathname === `${api}kernel/install`);
  await page.locator("#btnUpgradeAction").click();
  await expect(page.locator("#modalTitle")).toHaveText("在线升级确认");
  await page.locator("#modalConfirmBtn").click();
  const recoveryResponse = await recoveryAcceptance;
  expect(recoveryResponse.status()).toBe(200);
  const recoveryTask = await recoveryResponse.json() as { success: boolean; seq: number; instance: string };
  expect(recoveryTask).toMatchObject({ success: true, seq: expect.any(Number), instance: expect.any(String) });
  await expect.poll(async () => {
    const response = await request.get(`${api}kernel/install-status`);
    if (!response.ok()) return false;
    const status = await response.json() as { seq?: number; instance?: string; upgrading?: boolean };
    return status.seq === recoveryTask.seq && status.instance === recoveryTask.instance && status.upgrading === true;
  }).toBe(true);
  await expect.poll(() => exists(pipStarted)).toBe(true);
  await expect.poll(() => page.evaluate(({ seq, instance }) => Object.values(sessionStorage).some(value => {
    try {
      const saved = JSON.parse(value);
      return saved.seq === seq && saved.instance === instance;
    } catch {
      return false;
    }
  }), recoveryTask)).toBe(true);

  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.waitForTimeout(150);
  const beforeResume = installStatusRequests.length;
  const resumedAt = Date.now();
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("pageshow"));
  });
  await expect.poll(() => installStatusRequests.length, { timeout: 700 }).toBeGreaterThan(beforeResume);
  const resumedRequestAt = installStatusRequests.at(-1) ?? Number.NEGATIVE_INFINITY;
  expect(resumedRequestAt - resumedAt).toBeLessThan(700);

  const beforeRebuildPoll = installStatusRequests.length;
  const isWebKit = testInfo.project.use.browserName === "webkit";
  const webkitReloadPageErrors: string[] = [];
  if (isWebKit) reloadPageErrors = webkitReloadPageErrors;
  try {
    await page.reload();
  } finally {
    reloadPageErrors = null;
  }
  if (isWebKit) {
    expect(pageErrors).toEqual([]);
    const allowedEndpoints = ["status", "kernel/install-log", "kernel/install-status"];
    expect(webkitReloadPageErrors.length).toBeLessThanOrEqual(allowedEndpoints.length);
    const canceledEndpoints = webkitReloadPageErrors.map(message => {
      const match = message.match(/^\/([^/]+)(\/[^?\s]+)\?(\S+) due to access control checks\.$/);
      expect(match, `unexpected WebKit reload pageerror: ${message}`).not.toBeNull();
      const [, host, pathname, query] = match!;
      const canceledUrl = new URL(`https://${host}${pathname}?${query}`);
      expect(canceledUrl.origin).toBe(origin);
      expect(canceledUrl.pathname.startsWith(`${prefix}api/`)).toBe(true);
      const endpoint = canceledUrl.pathname.slice(`${prefix}api/`.length);
      expect(allowedEndpoints).toContain(endpoint);
      const expectedQueryKeys = endpoint === "kernel/install-log" ? ["_t", "instance", "seq", "tail"] : ["_t"];
      expect([...canceledUrl.searchParams.keys()].sort()).toEqual(expectedQueryKeys);
      expect(canceledUrl.searchParams.get("_t")).toMatch(/^\d+$/);
      if (endpoint === "kernel/install-log") {
        expect(canceledUrl.searchParams.get("tail")).toBe("40");
        expect(canceledUrl.searchParams.get("seq")).toBe(String(recoveryTask.seq));
        expect(canceledUrl.searchParams.get("instance")).toBe(recoveryTask.instance);
      }
      return endpoint;
    });
    // Only requests still in flight when WebKit tears down the document can be cancelled.
    expect(new Set(canceledEndpoints).size).toBe(canceledEndpoints.length);
  }
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
  await expect(page.locator("#upTitle")).toHaveText("QwenPaw 安装任务");
  await expect.poll(() => installStatusRequests.length).toBeGreaterThan(beforeRebuildPoll);
  await expect.poll(async () => {
    const seen = await observations(request);
    return seen.upgradeStatuses.some(status => status.seq === recoveryTask.seq
      && status.instance === recoveryTask.instance && status.upgrading === true);
  }).toBe(true);
  await expect.poll(async () => (await observations(request)).upgradePosts).toHaveLength(2);
  await expect(page.locator("#upTitle")).toHaveText("安装任务已顺利完成！", { timeout: 75_000 });
  await expect.poll(async () => (await readFile(fixture.files.pin, "utf8")).trim()).toBe("2.1.0b1");
  await rm(slowPip, { force: true });

  const targetPortResponse = await request.get("/__e2e/allocate-port");
  expect(targetPortResponse.ok()).toBeTruthy();
  const { port: targetPort } = await targetPortResponse.json() as { port: number };
  await page.locator("#btnUpClose").click();
  await page.locator('[data-section="configuration"]').click();
  await page.getByRole("button", { name: "修改端口" }).click();
  await expect(page.locator("#modalTitle")).toHaveText("修改服务端口");
  await page.locator("#modalInput").fill(String(targetPort));
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#modalTitle")).toHaveText("修改服务端口确认");
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(async () => (await readFile(fixture.files.servicePort, "utf8")).trim()).toBe(String(targetPort));
  await expect(page.locator("#valPort")).toHaveText(String(targetPort));
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  const desktop = JSON.parse(await readFile(fixture.files.uiConfig, "utf8"));
  const entries = desktop[".url"] as Record<string, Record<string, unknown>>;
  const mainEntry = entries["qwenpaw.main"];
  const consoleEntry = entries["qwenpaw.console"];
  expect(mainEntry).toMatchObject({
    type: "iframe",
    protocol: "",
    url: `${prefix}web/`,
    gatewayPrefix: prefix.slice(0, -1),
    gatewaySocket: "app.sock",
    noDisplay: false,
  });
  expect(mainEntry.gatewayPrefix).toBe(consoleEntry.gatewayPrefix);
  expect(mainEntry.gatewaySocket).toBe(consoleEntry.gatewaySocket);
  expect(mainEntry.url).not.toMatch(/^https?:\/\/|:\d+/);
  const webConsole = page.locator("#btnWebConsole");
  await expect(webConsole).toHaveAttribute("href", `${prefix}web/`);
  await expect(webConsole).not.toHaveAttribute("target", "_blank");
  const webConsoleUrl = new URL((await webConsole.getAttribute("href"))!, page.url());
  expect(webConsoleUrl.origin).toBe(origin);
  expect(webConsoleUrl.pathname).toBe(`${prefix}web/`);
  expect(webConsoleUrl.port).toBe(new URL(page.url()).port);
  expect(webConsoleUrl.port).not.toBe(String(targetPort));
  const changedHealth = await request.get(`http://127.0.0.1:${targetPort}/api/version`);
  expect(changedHealth.ok()).toBeTruthy();
  expect(await changedHealth.json()).toEqual({ version: "2.1.0b1" });
  await expect(request.get(`http://127.0.0.1:${fixture.appPort}/api/version`, { timeout: 1000 })).rejects.toThrow();
  consoleErrors.length = 0;
  failedRequests.length = 0;

  await page.locator('[data-section="overview"]').click();
  await expect(page.locator('#targetVersion')).toBeDisabled();
  await page.getByRole('button', { name: '检查更新', exact: true }).click();
  await pickVersion(page, "2.1.0b1");
  await expect(page.locator("#btnUpgradeAction")).toHaveText("重新安装 v2.1.0b1");
  await writeFile(slowPip, "hold task after dropping the acceptance receipt\n");
  await rm(pipStarted, { force: true });
  try {
    expect((await request.post("/__e2e/lose-next-upgrade-receipt")).ok()).toBeTruthy();
    await page.locator("#btnUpgradeAction").click();
    await expect(page.locator("#modalTitle")).toHaveText("重新安装当前版本");
    await page.locator("#modalConfirmBtn").click();
    await expect.poll(() => exists(pipStarted)).toBe(true);
    let lostReceiptTask: { seq: number; instance: string } | null = null;
    await expect.poll(async () => {
      const response = await request.get(`${api}kernel/install-status`);
      if (!response.ok()) return false;
      const status = await response.json() as { upgrading?: boolean; seq?: number; instance?: string };
      if (!status.upgrading || !Number.isInteger(status.seq) || !status.instance) return false;
      lostReceiptTask = { seq: status.seq!, instance: status.instance };
      return true;
    }).toBe(true);
    const attachedTask = lostReceiptTask!;
    await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
    await expect(page.locator("#upTitle")).toHaveText("QwenPaw 安装任务");
    await expect.poll(() => installLogUrls.some(url => {
      const parsed = new URL(url);
      return parsed.searchParams.get("seq") === String(attachedTask.seq)
        && parsed.searchParams.get("instance") === attachedTask.instance;
    })).toBe(true);
    await expect.poll(async () => (await observations(request)).upgradePosts).toHaveLength(3);
    await page.waitForTimeout(1800);
    expect((await observations(request)).upgradePosts).toHaveLength(3);
  } finally {
    await rm(slowPip, { force: true });
  }
  await expect(page.locator("#upTitle")).toHaveText("安装任务已顺利完成！", { timeout: 75_000 });
  await expect.poll(async () => (await readFile(`${fixture.files.venv}/installed`, "utf8")).trim()).toBe("2.1.0b1");
  await expect.poll(async () => (await readFile(fixture.files.pin, "utf8")).trim()).toBe("2.1.0b1");
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  await expect(page.locator("#valVersion")).toHaveText("2.1.0b1");
  const betaHealth = await request.get(`http://127.0.0.1:${targetPort}/api/version`);
  expect(betaHealth.ok()).toBeTruthy();
  expect(await betaHealth.json()).toEqual({ version: "2.1.0b1" });
  await page.locator("#btnUpClose").click();

  await page.locator('[data-section="logs"]').click();
  await openLogTools(page);
  await page.getByRole("button", { name: "清空日志", exact: true }).click();
  await expect(page.locator("#modalTitle")).toHaveText("删除服务日志");
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(async () => readFile(fixture.files.kernelLog, "utf8")).not.toContain("browser-e2e-runtime-log");
  await expect.poll(async () => (await observations(request)).forwarded.some(item => item.path === `${api}logs/clear` && item.method === "POST" && item.success === true)).toBe(true);
  await page.goto(prefix + "#version");
  await page.locator("#btnUninstallKernel").click();
  await expect(page.locator("#modalTitle")).toHaveText("仅卸载 QwenPaw 内核");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("内核未安装", { timeout: 20_000 });
  await expect(page.locator("#valVersion")).toHaveText("未安装");
  await expect.poll(() => exists(fixture.files.venv)).toBe(false);
  await expect.poll(() => exists(fixture.files.pin)).toBe(false);
  await expect(request.get(`http://127.0.0.1:${targetPort}/api/version`, { timeout: 1000 })).rejects.toThrow();
  expect(await readFile(fixture.files.config, "utf8")).toBe('{"fixture":"preserve-user-data"}\n');
  expect(await readFile(fixture.files.auth, "utf8")).toBe("isolated-browser-auth-must-survive\n");
  expect((await readFile(fixture.files.mirror, "utf8")).trim()).toBe(fixture.mirrors.tsinghua);

  await expect.poll(async () => (await observations(request)).upgradePosts.length).toBe(3);
  const seen = await observations(request);
  expect(seen.indexRequests).toContain("/mirror/aliyun/qwenpaw/");
  expect(seen.indexRequests).toContain("/mirror/tsinghua/qwenpaw/");
  expect(seen.upgradePosts.map(post => post.version)).toEqual(["2.0.0", "2.1.0b1", "2.1.0b1"]);
  expect(seen.upgradePosts.map(post => post.receiptLost)).toEqual([false, false, true]);
  expect(seen.upgradePosts.map(post => post.status)).toEqual([200, 200, 200]);
  expect(seen.upgradePosts.map(post => post.success)).toEqual([true, true, true]);
  expect(seen.upgradePosts.every(post => Number.isInteger(post.seq) && Boolean(post.instance))).toBe(true);
  expect(new Set(seen.upgradePosts.map(post => post.seq)).size).toBe(3);
  expect(seen.upgradePosts.every(post => post.csrfHeader && !post.csrfQuery && post.contentType === "application/json")).toBe(true);
  expect(seen.upgradePosts.every(post => post.mirror === fixture.mirrors.tsinghua)).toBe(true);
  expect(seen.upgradePosts.every(post => post.origin === origin && post.upstreamHost === "qwenpaw-backend.internal" && post.fetchSite === "same-origin")).toBe(true);
  for (const post of seen.upgradePosts) expect(seen.upgradeStatuses).toContainEqual(expect.objectContaining({ phase: "done", seq: post.seq, instance: post.instance, upgrading: false }));
  expect(installLogUrls.every(url => {
    const parsed = new URL(url);
    return Boolean(parsed.searchParams.get("seq") && parsed.searchParams.get("instance"));
  })).toBe(true);
  expect(seen.forwarded.some(item => item.path === `${api}kernel/install-log` && item.status === 200)).toBe(true);
  expect(csrfCredentials.length).toBeGreaterThan(0);
  expect(csrfCredentials.every(({ header, query }) => header === csrf?.value && query === null)).toBe(true);
  expect(browserRequests.every(url => new URL(url).origin === origin && new URL(url).pathname.startsWith(prefix))).toBe(true);
  expect(pageErrors).toEqual([]);
  const expectedConsoleText = (() => {
    switch (testInfo.project.use.browserName) {
      case "chromium":
        return "Failed to load resource: net::ERR_EMPTY_RESPONSE";
      case "webkit":
        return "Failed to load resource: Connection terminated unexpectedly";
      default:
        throw new Error(`Unsupported browser engine: ${testInfo.project.use.browserName ?? "unknown"}`);
    }
  })();
  expect(consoleErrors).toEqual([{ text: expectedConsoleText, url: `${origin}${api}kernel/install` }]);
  expect([...installAttemptResults.values()].map(attempt => attempt.responseStatus)).toEqual([200, 200, undefined]);
  expect([...installAttemptResults.values()].map(attempt => attempt.failed)).toEqual([false, false, true]);
  const unexpectedFailedRequests = failedRequests.filter(({ method, url, errorText }) => {
    const parsed = new URL(url);
    const closedEventStream = method === "GET" && parsed.origin === origin
      && parsed.pathname === `${api}events` && establishedEventStreams.has(url)
      && /abort|cancel|terminated unexpectedly/i.test(errorText);
    return !closedEventStream;
  });
  expect(unexpectedFailedRequests).toEqual([{ method: "POST", url: `${origin}${api}kernel/install`, errorText: expect.stringMatching(/\S/) }]);
});

test("sessionStorage unavailable: a pending install-log request does not block owned install progress or resubmission", async ({ page, request }) => {
  test.setTimeout(150_000);
  const fixture = await metadata(request);
  const before = await observations(request);
  const controlRoot = dirname(dirname(fixture.files.venv));
  const slowPip = join(controlRoot, "slow-pip");
  const pipStarted = join(controlRoot, "pip-started");
  const pageErrors: string[] = [];
  const statusRequests: number[] = [];
  let firstInstallLogRequestReached!: () => void;
  let releaseFirstInstallLogRequest!: () => void;
  let installLogRequestHeld = false;
  let acceptedSeq = 0;
  let acceptedInstance = "";
  const firstInstallLogRequest = new Promise<void>(resolve => { firstInstallLogRequestReached = resolve; });
  page.on("pageerror", error => pageErrors.push(error.message));
  page.on("request", browserRequest => {
    if (browserRequest.method() === "GET" && new URL(browserRequest.url()).pathname === `${api}kernel/install-status`) {
      statusRequests.push(Date.now());
    }
  });
  await writeFile(slowPip, "hold task while install status remains observable\n");
  await rm(pipStarted, { force: true });
  await page.route(`**${api}kernel/install-log**`, async route => {
    if (!installLogRequestHeld) {
      installLogRequestHeld = true;
      firstInstallLogRequestReached();
      await new Promise<void>(resolve => { releaseFirstInstallLogRequest = resolve; });
    }
    await route.continue();
  });
  await page.addInitScript(() => {
    Object.defineProperty(window, "sessionStorage", {
      configurable: true,
      get() { throw new DOMException("sessionStorage blocked", "SecurityError"); },
    });
  });

  try {
    await page.goto(prefix);
    await expect(page.locator("#statusCardMount .status-text")).toHaveText("内核未安装");
    await expect(page.locator("#upgradeModal")).not.toHaveClass(/active/);

    await expect(page.locator('#targetVersion')).toBeVisible();
    await pickVersion(page, "2.0.0");
    const acceptance = page.waitForResponse(response => response.request().method() === "POST" && new URL(response.url()).pathname === `${api}kernel/install`);
    await page.locator("#btnUpgradeAction").click();
    await expect(page.locator("#modalTitle")).toHaveText("安装内核确认");
    await page.locator("#modalConfirmBtn").click();
    const acceptedResponse = await acceptance;
    expect(acceptedResponse.status()).toBe(200);
    const accepted = await acceptedResponse.json() as { success: boolean; seq: number; instance: string };
    expect(accepted).toMatchObject({ success: true, seq: expect.any(Number), instance: expect.any(String) });
    acceptedSeq = accepted.seq;
    acceptedInstance = accepted.instance;
    await firstInstallLogRequest;
    await expect.poll(() => exists(pipStarted)).toBe(true);
    await expect.poll(async () => {
      const response = await request.get(`${api}kernel/install-status`);
      if (!response.ok()) return false;
      const status = await response.json() as { seq?: number; instance?: string; upgrading?: boolean };
      return status.seq === accepted.seq && status.instance === accepted.instance && status.upgrading === true;
    }).toBe(true);
    await expect(page.locator("#upTerminal")).not.toContainText("正在获取实时安装状态...", { timeout: 4_000 });

    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await page.waitForTimeout(150);
    const beforeResume = statusRequests.length;
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
      document.dispatchEvent(new Event("visibilitychange"));
      window.dispatchEvent(new Event("pageshow"));
    });
    await expect.poll(() => statusRequests.length, { timeout: 1_000 }).toBeGreaterThan(beforeResume);
    expect(installLogRequestHeld).toBe(true);

    await page.waitForTimeout(1000);
    const after = await observations(request);
    expect(after.upgradePosts).toHaveLength(before.upgradePosts.length + 1);
    expect(after.upgradePosts.at(-1)).toMatchObject({ success: true, seq: accepted.seq, instance: accepted.instance });
    expect(pageErrors).toEqual([]);
  } finally {
    releaseFirstInstallLogRequest?.();
    await rm(slowPip, { force: true });
  }

  await expect(page.locator("#upTitle")).toHaveText("安装已顺利完成！", { timeout: 75_000 });
  await expect.poll(async () => {
    const seen = await observations(request);
    return seen.upgradeStatuses.some(status => status.seq === acceptedSeq
      && status.instance === acceptedInstance && status.phase === "done");
  }).toBe(true);

  await page.locator("#btnUpClose").click();
  await page.goto(prefix + "#version");
  await page.locator("#btnUninstallKernel").click();
  await expect(page.locator("#modalTitle")).toHaveText("仅卸载 QwenPaw 内核");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("内核未安装", { timeout: 20_000 });
});
