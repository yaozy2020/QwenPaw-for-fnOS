import { expect, test } from "@playwright/test";
import { readFile, stat } from "node:fs/promises";

async function pathExists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
}

test("real Chromium management flow persists settings, installs, polls, logs, and uninstalls safely", async ({ page, request }) => {
  const pageErrors: string[] = [];
  const consoleErrors: Array<{ text: string; url: string }> = [];
  const failedRequests: Array<{ method: string; url: string; errorText: string }> = [];
  page.on("pageerror", error => pageErrors.push(error.message));
  page.on("console", message => {
    if (message.type() === "error") {
      consoleErrors.push({ text: message.text(), url: message.location().url });
    }
  });
  page.on("requestfailed", request => {
    failedRequests.push({
      method: request.method(),
      url: request.url(),
      errorText: request.failure()?.errorText || "unknown",
    });
  });

  const metadataResponse = await request.get("/__e2e/meta");
  expect(metadataResponse.ok()).toBeTruthy();
  const fixture = await metadataResponse.json() as {
    appPort: number;
    files: {
      auth: string;
      config: string;
      installLog: string;
      kernelLog: string;
      mirror: string;
      pin: string;
      servicePort: string;
      uiConfig: string;
      venv: string;
    };
    binaries: {
      apiCgi: { path: string; sha256: string };
      indexCgi: { path: string; sha256: string };
      runtime: { path: string; sha256: string };
      supervisor: { path: string; sha256: string };
      webConsole: { path: string; sha256: string };
    };
    mirrors: { aliyun: string; tsinghua: string };
  };
  expect(fixture.binaries.supervisor.path).toMatch(/qwenpaw-supervisor$/);
  expect(fixture.binaries.runtime.path).toMatch(/qwenpaw-test-runtime$/);
  for (const binary of Object.values(fixture.binaries)) {
    expect(binary.sha256).toMatch(/^[a-f0-9]{64}$/);
  }

  await page.goto("/cgi/ThirdParty/qwenpaw/index.cgi/");
  const hostCsrfCookie = (await page.context().cookies()).find(cookie => cookie.name === "__Host-qwenpaw_csrf");
  expect(hostCsrfCookie).toBeDefined();
  expect(hostCsrfCookie?.secure).toBe(true);
  expect(hostCsrfCookie?.path).toBe("/");
  await page.context().addCookies([{
    name: "qwenpaw_csrf",
    value: "sibling-domain-cookie",
    url: new URL("/", page.url()).href,
    sameSite: "Lax",
  }]);
  const csrfCredentials: Array<{ header: string; query: string | null }> = [];
  page.on("request", request => {
    if (request.method() === "POST" && request.url().includes("/api.cgi?")) {
      csrfCredentials.push({
        header: request.headers()["x-csrf-token"] || "",
        query: new URL(request.url()).searchParams.get("_csrf"),
      });
    }
  });
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("内核未安装");
  await expect(page.locator("#valVersion")).toHaveText("未安装");
  await expect(page.locator("#versionCardsContainer .ver-number").first()).toHaveText("v2.1.0b1");
  await expect(page.locator("#mirrorLabel")).toContainText("阿里云镜像源");
  await expect(page.locator("#logTerminal")).toContainText("browser-e2e-runtime-log");

  await page.locator("#mirrorToggleBtn").click();
  await page.locator("#mirrorDropdown").getByText("清华大学镜像源", { exact: true }).click();
  await expect(page.locator("#mirrorLabel")).toContainText("清华大学镜像源");
  await expect.poll(async () => (await readFile(fixture.files.mirror, "utf8")).trim())
    .toBe(fixture.mirrors.tsinghua);

  await page.locator("#btnUpgradeAction").click();
  await expect(page.locator("#modalTitle")).toHaveText("安装内核确认");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#upgradeModal")).toHaveClass(/active/);
  await expect(page.locator("#upTitle")).toHaveText("安装已顺利完成！", { timeout: 45_000 });
  await expect.poll(async () => (await readFile(`${fixture.files.venv}/installed`, "utf8")).trim()).toBe("2.0.0");
  await expect.poll(async () => (await readFile(fixture.files.pin, "utf8")).trim()).toBe("2.0.0");
  await expect(page.locator("#upTerminal")).toContainText("Successfully installed qwenpaw-2.0.0");
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  await expect(page.locator("#valVersion")).toHaveText("2.0.0");
  const initialHealth = await request.get(`http://127.0.0.1:${fixture.appPort}/api/version`);
  expect(initialHealth.ok()).toBeTruthy();
  expect(await initialHealth.json()).toEqual({ version: "2.0.0" });
  expect(await readFile(fixture.files.installLog, "utf8")).toContain("Successfully installed qwenpaw-2.0.0");

  const targetPortResponse = await request.get("/__e2e/allocate-port");
  expect(targetPortResponse.ok()).toBeTruthy();
  const { port: targetPort } = await targetPortResponse.json() as { port: number };
  await page.locator("#btnUpClose").click();
  await page.getByRole("button", { name: "修改端口" }).click();
  await expect(page.locator("#modalTitle")).toHaveText("修改服务端口");
  await page.locator("#modalInput").fill(String(targetPort));
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#modalTitle")).toHaveText("修改服务端口确认");
  await page.locator("#modalConfirmBtn").click();
  await expect.poll(async () => (await readFile(fixture.files.servicePort, "utf8")).trim())
    .toBe(String(targetPort));
  await expect(page.locator("#valPort")).toHaveText(String(targetPort));
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  await expect(page.locator("#valVersion")).toHaveText("2.0.0");
  const desktopConfig = JSON.parse(await readFile(fixture.files.uiConfig, "utf8")) as {
    ".url": { "qwenpaw.main": { port: string; noDisplay: boolean } };
  };
  expect(desktopConfig[".url"]["qwenpaw.main"]).toEqual({ port: String(targetPort), noDisplay: false });
  const changedPortHealth = await request.get(`http://127.0.0.1:${targetPort}/api/version`);
  expect(changedPortHealth.ok()).toBeTruthy();
  expect(await changedPortHealth.json()).toEqual({ version: "2.0.0" });

  await page.locator("#versionCardsContainer .ver-number").filter({ hasText: "v2.1.0b1" }).click();
  await expect(page.locator("#btnUpgradeAction")).toHaveText("执行更新至 v2.1.0b1");
  const armLoss = await request.post("/__e2e/lose-next-upgrade-receipt");
  expect(armLoss.ok()).toBeTruthy();
  await page.locator("#btnUpgradeAction").click();
  await expect(page.locator("#modalTitle")).toHaveText("在线升级确认");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#upTitle")).toHaveText("无法确认安装任务结果", { timeout: 25_000 });
  try {
    await expect.poll(async () => readFile(fixture.files.installLog, "utf8"))
      .toContain("Successfully installed qwenpaw-2.1.0b1");
  } catch (error) {
    const observationResponse = await request.get("/__e2e/observations");
    const observations: unknown = await observationResponse.json();
    const statusResponse = await request.get("/cgi/ThirdParty/qwenpaw/api.cgi?action=upgrade_status");
    console.error("upgrade observations after missing install log:", JSON.stringify(observations));
    console.error("last backend install status:", await statusResponse.text());
    throw error;
  }
  expect((await readFile(fixture.files.installLog, "utf8")).match(/Successfully installed qwenpaw-2\.1\.0b1/g)).toHaveLength(1);
  await expect.poll(async () => (await readFile(`${fixture.files.venv}/installed`, "utf8")).trim()).toBe("2.1.0b1");
  await expect.poll(async () => (await readFile(fixture.files.pin, "utf8")).trim()).toBe("2.1.0b1");
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("运行中");
  await expect(page.locator("#valVersion")).toHaveText("2.1.0b1");
  const betaHealth = await request.get(`http://127.0.0.1:${targetPort}/api/version`);
  expect(betaHealth.ok()).toBeTruthy();
  expect(await betaHealth.json()).toEqual({ version: "2.1.0b1" });
  await page.locator("#btnUpClose").click();

  await page.locator("#btnUninstallKernel").click();
  await expect(page.locator("#modalTitle")).toHaveText("仅卸载 QwenPaw 内核");
  await page.locator("#modalConfirmBtn").click();
  await expect(page.locator("#statusCardMount .status-text")).toHaveText("内核未安装", { timeout: 20_000 });
  await expect(page.locator("#valVersion")).toHaveText("未安装");
  await expect.poll(async () => pathExists(fixture.files.venv)).toBe(false);
  await expect.poll(async () => pathExists(fixture.files.pin)).toBe(false);
  expect(await readFile(fixture.files.config, "utf8")).toBe("{\"fixture\":\"preserve-user-data\"}\n");
  expect(await readFile(fixture.files.auth, "utf8")).toBe("isolated-browser-auth-must-survive\n");
  expect((await readFile(fixture.files.mirror, "utf8")).trim()).toBe(fixture.mirrors.tsinghua);

  await expect.poll(async () => {
    const response = await request.get("/__e2e/observations");
    if (!response.ok()) return -1;
    return ((await response.json()) as { upgradePosts: unknown[] }).upgradePosts.length;
  }).toBe(3);
  const observationResponse = await request.get("/__e2e/observations");
  expect(observationResponse.ok()).toBeTruthy();
  const observations = await observationResponse.json() as {
    indexRequests: string[];
    upgradePosts: Array<{
      instance: string | null;
      receiptLost: boolean;
      seq: number | null;
      status: number | null;
      success: boolean | null;
      version: string;
    }>;
    upgradeStatuses: Array<{
      instance: string | null;
      phase: string | null;
      seq: number | null;
      upgrading: boolean | null;
    }>;
  };
  expect(observations.indexRequests).toContain("/mirror/aliyun/qwenpaw/");
  expect(observations.upgradePosts.map(post => post.version)).toEqual(["2.0.0", "2.1.0b1", "2.1.0b1"]);
  expect(observations.upgradePosts.map(post => post.receiptLost)).toEqual([false, true, false]);
  const acceptedPosts = observations.upgradePosts.filter(post => post.success === true);
  expect(acceptedPosts.map(post => post.version)).toEqual(["2.0.0", "2.1.0b1"]);
  expect(acceptedPosts.every(post => Number.isInteger(post.seq) && Boolean(post.instance))).toBe(true);
  const acceptedSeqs = acceptedPosts.map(post => post.seq);
  expect(new Set(acceptedSeqs).size).toBe(2);
  expect(observations.upgradePosts[2]).toMatchObject({
    success: false,
    seq: acceptedPosts[1].seq,
    instance: acceptedPosts[1].instance,
  });
  for (const post of acceptedPosts) {
    expect(observations.upgradeStatuses).toContainEqual(expect.objectContaining({
      phase: "done",
      seq: post.seq,
      instance: post.instance,
      upgrading: false,
    }));
  }

  expect(csrfCredentials.length).toBeGreaterThan(0);
  expect(csrfCredentials.every(({ header, query }) =>
    (header || query) === hostCsrfCookie?.value
  )).toBe(true);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toHaveLength(1);
  expect(consoleErrors[0].text).toBe("Failed to load resource: net::ERR_EMPTY_RESPONSE");
  expect(consoleErrors[0].url).toMatch(/\/api\.cgi\?action=upgrade&version=2\.1\.0b1(?:&|$)/);
  expect(failedRequests).toHaveLength(1);
  expect(failedRequests[0].method).toBe("POST");
  expect(failedRequests[0].url).toMatch(/\/api\.cgi\?action=upgrade&version=2\.1\.0b1(?:&|$)/);
  expect(failedRequests[0].errorText).toBe("net::ERR_EMPTY_RESPONSE");
});
