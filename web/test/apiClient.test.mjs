import test from "node:test";
import assert from "node:assert/strict";
import {
  ApiClientError,
  createApiClient,
  deriveApiBase,
  readCsrfToken,
} from "../src/api.ts";

const jsonResponse = (data, status = 200) => new Response(JSON.stringify(data), {
  status,
  headers: { "content-type": "application/json" },
});

test("deriveApiBase only accepts the Gateway console prefix", () => {
  assert.equal(deriveApiBase("/app/qwenpaw"), "/app/qwenpaw/api");
  assert.equal(deriveApiBase("/app/qwenpaw/"), "/app/qwenpaw/api");
  assert.equal(deriveApiBase("/app/qwenpaw/index.html"), "/app/qwenpaw/api");
  assert.throws(() => deriveApiBase("/console/"), /Gateway/);
  assert.throws(() => deriveApiBase("/app/qwenpaw-extra/index.html"), /Gateway/);
});

test("readCsrfToken prefers the host-only cookie and falls back to the legacy cookie", () => {
  assert.equal(readCsrfToken("session=x; qwenpaw_csrf=legacy; __Host-qwenpaw_csrf=host-token; theme=dark"), "host-token");
  assert.equal(readCsrfToken("session=x; qwenpaw_csrf=token%2Fvalue; theme=dark"), "token%2Fvalue");
  assert.equal(readCsrfToken("not_qwenpaw_csrf=wrong; other=1"), "");
  assert.equal(readCsrfToken(""), "");
});

test("readCsrfToken rejects duplicated CSRF cookie names", () => {
  assert.equal(readCsrfToken("__Host-qwenpaw_csrf=first; __Host-qwenpaw_csrf=second"), "");
  assert.equal(readCsrfToken("__Host-qwenpaw_csrf=first; __Host-qwenpaw_csrf=second; qwenpaw_csrf=legacy"), "");
  assert.equal(readCsrfToken("qwenpaw_csrf=first; qwenpaw_csrf=second"), "");
});

test("HTTP 409 JSON remains available to callers with its status", async () => {
  const client = createApiClient({
    pathname: "/app/qwenpaw/index.html",
    cookie: () => "qwenpaw_csrf=csrf-token",
    fetch: async () => jsonResponse({ success: false, running: true, seq: 17, instance: "worker-a" }, 409),
  });

  const result = await client.upgrade("2.2.2.post1", "");
  assert.equal(result.status, 409);
  assert.equal(result.ok, false);
  assert.deepEqual(result.data, { success: false, running: true, seq: 17, instance: "worker-a" });
});

test("upgrade uses the Gateway REST route, encoded JSON body, CSRF and keepalive", async () => {
  let captured;
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    cookie: () => "qwenpaw_csrf=legacy-token; __Host-qwenpaw_csrf=csrf-token",
    fetch: async (url, init) => {
      captured = { url, init };
      return jsonResponse({ success: true, seq: 18, instance: "worker-a" }, 202);
    },
  });

  const result = await client.upgrade("2.2 + rc", "https://mirror.example/simple/?a=1&b=2");
  assert.equal(result.data.seq, 18);
  assert.equal(captured.url, "/app/qwenpaw/api/kernel/install");
  assert.equal(captured.init.method, "POST");
  assert.equal(captured.init.body, JSON.stringify({ version: "2.2 + rc", mirror: "https://mirror.example/simple/?a=1&b=2" }));
  assert.equal(captured.init.keepalive, true);
  assert.ok(captured.init.signal instanceof AbortSignal);
  assert.equal(captured.init.signal.aborted, false);
  assert.equal(new Headers(captured.init.headers).get("X-CSRF-Token"), "csrf-token");
  assert.equal(new Headers(captured.init.headers).get("Content-Type"), "application/json");

  assert.equal("upgradeBeaconRequest" in client, false);
});

test("Gateway pages call native REST routes with exact request shapes", async () => {
  const captured = [];
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    cookie: () => "qwenpaw_csrf=csrf-token",
    now: () => 123,
    fetch: async (url, init = {}) => {
      captured.push({
        url,
        method: init.method || "GET",
        body: init.body ?? null,
        csrf: new Headers(init.headers).get("X-CSRF-Token"),
        contentType: new Headers(init.headers).get("Content-Type"),
        keepalive: init.keepalive ?? false,
      });
      return jsonResponse({ success: true, seq: 18, instance: "worker-a" });
    },
  });

  assert.equal(client.base, "/app/qwenpaw/api");
  await client.status();
  await client.changePort(18099);
  await client.resetAuth();
  await client.uninstallKernel();
  await client.controlService("start");
  await client.controlService("stop");
  await client.controlService("restart");
  await client.getMirror();
  await client.setMirror("https://mirror.example/simple?a=1&b=2");
  await client.versionList();
  await client.upgradeStatus();
  const accepted = await client.upgrade("2.2 + rc", "https://mirror.example/simple/?a=1&b=2");
  await client.upgradeLogs(18, "worker-a/+ &");
  await client.logs(9);
  await client.clearLogs();

  assert.equal(accepted.data.seq, 18);
  assert.equal(accepted.data.instance, "worker-a");
  assert.deepEqual(captured, [
    { url: "/app/qwenpaw/api/status?_t=123", method: "GET", body: null, csrf: null, contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/port", method: "POST", body: "{\"port\":18099}", csrf: "csrf-token", contentType: "application/json", keepalive: false },
    { url: "/app/qwenpaw/api/kernel/reset-auth", method: "POST", body: null, csrf: "csrf-token", contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/kernel/uninstall", method: "POST", body: null, csrf: "csrf-token", contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/start", method: "POST", body: null, csrf: "csrf-token", contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/stop", method: "POST", body: null, csrf: "csrf-token", contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/restart", method: "POST", body: null, csrf: "csrf-token", contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/mirror?_t=123", method: "GET", body: null, csrf: null, contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/mirror", method: "POST", body: "{\"mirror\":\"https://mirror.example/simple?a=1&b=2\"}", csrf: "csrf-token", contentType: "application/json", keepalive: false },
    { url: "/app/qwenpaw/api/kernel/versions?_t=123", method: "GET", body: null, csrf: null, contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/kernel/install-status?_t=123", method: "GET", body: null, csrf: null, contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/kernel/install", method: "POST", body: "{\"version\":\"2.2 + rc\",\"mirror\":\"https://mirror.example/simple/?a=1&b=2\"}", csrf: "csrf-token", contentType: "application/json", keepalive: true },
    { url: "/app/qwenpaw/api/kernel/install-log?tail=40&seq=18&instance=worker-a%2F%2B%20%26&_t=123", method: "GET", body: null, csrf: null, contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/logs?tail=9&_t=123", method: "GET", body: null, csrf: null, contentType: null, keepalive: false },
    { url: "/app/qwenpaw/api/logs/clear", method: "POST", body: null, csrf: "csrf-token", contentType: null, keepalive: false },
  ]);
});

test("Web proxy port API reads the active access and submits public or loopback changes with CSRF", async () => {
  const captured = [];
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    cookie: () => "__Host-qwenpaw_csrf=csrf-token",
    now: () => 321,
    fetch: async (url, init = {}) => {
      captured.push({
        url,
        method: init.method || "GET",
        body: init.body ?? null,
        csrf: new Headers(init.headers).get("X-CSRF-Token"),
      });
      return init.method === "POST"
        ? jsonResponse({ success: true, port: 43127, message: "changed" })
        : jsonResponse({ success: true, port: 43126, public: false });
    },
  });

  const current = await client.webProxyPort();
  const changed = await client.changeWebProxyPort(0, true);
  await client.changeWebProxyPort(43128, false);

  assert.deepEqual(current.data, { success: true, port: 43126, public: false });
  assert.deepEqual(changed.data, { success: true, port: 43127, message: "changed" });
  assert.deepEqual(captured, [
    { url: "/app/qwenpaw/api/web-proxy/port?_t=321", method: "GET", body: null, csrf: null },
    { url: "/app/qwenpaw/api/web-proxy/port", method: "POST", body: JSON.stringify({ port: 0, public: true }), csrf: "csrf-token" },
    { url: "/app/qwenpaw/api/web-proxy/port", method: "POST", body: JSON.stringify({ port: 43128, public: false }), csrf: "csrf-token" },
  ]);
});

test("Gateway JSON errors keep status and data while non-JSON responses remain explicit", async () => {
  const calls = [];
  const client = createApiClient({
    pathname: "/app/qwenpaw",
    now: () => 99,
    fetch: async (url) => {
      calls.push(url);
      if (url === "/app/qwenpaw/api/kernel/install") {
        return jsonResponse({ success: false, running: true, seq: 17, instance: "worker-b" }, 409);
      }
      return new Response("gateway unavailable", {
        status: 502,
        headers: { "content-type": "text/html" },
      });
    },
  });

  const conflict = await client.upgrade("2.2.2", "");
  assert.equal(conflict.status, 409);
  assert.equal(conflict.ok, false);
  assert.deepEqual(conflict.data, { success: false, running: true, seq: 17, instance: "worker-b" });
  await assert.rejects(client.status(), (error) => {
    assert.ok(error instanceof ApiClientError);
    assert.equal(error.kind, "non-json");
    assert.equal(error.status, 502);
    return true;
  });
  assert.deepEqual(calls, [
    "/app/qwenpaw/api/kernel/install",
    "/app/qwenpaw/api/status?_t=99",
  ]);
});

test("Gateway install network failure is not retried using a legacy transport", async () => {
  const calls = [];
  const client = createApiClient({
    pathname: "/app/qwenpaw/index.html",
    cookie: () => "qwenpaw_csrf=csrf-token",
    fetch: async (url) => {
      calls.push(url);
      throw new TypeError("offline");
    },
  });

  await assert.rejects(client.upgrade("2.2.2.post1", ""), (error) => {
    assert.ok(error instanceof ApiClientError);
    assert.equal(error.kind, "network");
    return true;
  });
  assert.deepEqual(calls, ["/app/qwenpaw/api/kernel/install"]);
});

test("non-JSON responses fail explicitly", async () => {
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    fetch: async () => new Response("<!doctype html><title>Gateway error</title>", {
      status: 502,
      headers: { "content-type": "text/html" },
    }),
  });

  await assert.rejects(client.status(), (error) => {
    assert.ok(error instanceof ApiClientError);
    assert.equal(error.kind, "non-json");
    assert.match(error.message, /non-JSON/i);
    assert.match(error.message, /502/);
    return true;
  });
});

test("truncated JSON responses fail explicitly instead of producing an empty success", async () => {
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    fetch: async () => new Response('{"success":true,"seq":', {
      status: 200,
      headers: { "content-type": "application/json" },
    }),
  });

  await assert.rejects(client.status(), (error) => {
    assert.ok(error instanceof ApiClientError);
    assert.equal(error.kind, "json");
    assert.match(error.message, /invalid or truncated JSON/i);
    return true;
  });
});

test("POST requests time out and abort instead of leaving acceptance pending", async () => {
  let capturedSignal;
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    writeTimeoutMs: 20,
    fetch: async (_url, init) => {
      capturedSignal = init?.signal;
      return await new Promise(() => {});
    },
  });
  const result = await Promise.race([
    client.upgrade("2.2.2", "").then(value => ({ value }), error => ({ error })),
    new Promise(resolve => setTimeout(() => resolve({ pending: true }), 250)),
  ]);
  assert.equal(result.pending, undefined, "POST must settle even when acceptance never responds");
  assert.ok(result.error instanceof ApiClientError);
  assert.equal(result.error.kind, "network");
  assert.match(result.error.message, /timed out/i);
  assert.ok(capturedSignal instanceof AbortSignal);
  assert.equal(capturedSignal.aborted, true);
});

test("GET requests time out and abort the underlying request", async () => {
  let capturedSignal;
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    readTimeoutMs: 20,
    fetch: async (_url, init) => {
      capturedSignal = init?.signal;
      return await new Promise(() => {});
    },
  });

  const result = await Promise.race([
    client.status().then(value => ({ value }), error => ({ error })),
    new Promise(resolve => setTimeout(() => resolve({ pending: true }), 250)),
  ]);
  assert.equal(result.pending, undefined, "GET must settle even when fetch never resolves");
  assert.ok(result.error instanceof ApiClientError);
  assert.equal(result.error.kind, "network");
  assert.match(result.error.message, /timed out/i);
  assert.ok(capturedSignal instanceof AbortSignal);
  assert.equal(capturedSignal.aborted, true);
});

test("GET timeout remains bounded without AbortController support", async () => {
  const previous = globalThis.AbortController;
  Object.defineProperty(globalThis, "AbortController", { configurable: true, writable: true, value: undefined });
  try {
    const client = createApiClient({
      pathname: "/app/qwenpaw/",
      readTimeoutMs: 20,
      fetch: async () => await new Promise(() => {}),
    });
    const result = await Promise.race([
      client.status().then(value => ({ value }), error => ({ error })),
      new Promise(resolve => setTimeout(() => resolve({ pending: true }), 250)),
    ]);
    assert.equal(result.pending, undefined, "GET must settle without AbortController");
    assert.ok(result.error instanceof ApiClientError);
    assert.match(result.error.message, /timed out/i);
  } finally {
    Object.defineProperty(globalThis, "AbortController", { configurable: true, writable: true, value: previous });
  }
});

test("network failures are identified separately", async () => {
  const client = createApiClient({
    pathname: "/app/qwenpaw/",
    fetch: async () => { throw new TypeError("offline"); },
  });

  await assert.rejects(client.upgrade("2.2.2.post1", ""), (error) => {
    assert.ok(error instanceof ApiClientError);
    assert.equal(error.kind, "network");
    assert.match(error.message, /offline/);
    return true;
  });
});
