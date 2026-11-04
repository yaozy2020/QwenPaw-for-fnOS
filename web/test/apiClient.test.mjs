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

test("deriveApiBase preserves CGI subpath deployment and the standalone fallback", () => {
  assert.equal(deriveApiBase("/apps/custom/index.cgi"), "/apps/custom/api.cgi");
  assert.equal(deriveApiBase("/cgi/ThirdParty/qwenpaw/index.cgi"), "/cgi/ThirdParty/qwenpaw/api.cgi");
  assert.equal(deriveApiBase("/console/"), "/cgi/ThirdParty/qwenpaw/api.cgi");
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
    pathname: "/apps/qwenpaw/index.cgi",
    cookie: () => "qwenpaw_csrf=csrf-token",
    fetch: async () => jsonResponse({ success: false, running: true, seq: 17, instance: "worker-a" }, 409),
  });

  const result = await client.upgrade("2.2.2.post1", "");
  assert.equal(result.status, 409);
  assert.equal(result.ok, false);
  assert.deepEqual(result.data, { success: false, running: true, seq: 17, instance: "worker-a" });
});

test("upgrade request preserves encoded parameters, POST, CSRF and keepalive", async () => {
  let captured;
  const client = createApiClient({
    pathname: "/apps/qwenpaw/index.cgi",
    cookie: () => "qwenpaw_csrf=legacy-token; __Host-qwenpaw_csrf=csrf-token",
    fetch: async (url, init) => {
      captured = { url, init };
      return jsonResponse({ success: true, seq: 18, instance: "worker-a" }, 202);
    },
  });

  const result = await client.upgrade("2.2 + rc", "https://mirror.example/simple/?a=1&b=2");
  assert.equal(result.data.seq, 18);
  assert.equal(captured.url, "/apps/qwenpaw/api.cgi?action=upgrade&version=2.2%20%2B%20rc&mirror=https%3A%2F%2Fmirror.example%2Fsimple%2F%3Fa%3D1%26b%3D2");
  assert.equal(captured.init.method, "POST");
  assert.equal(captured.init.keepalive, true);
  assert.equal(new Headers(captured.init.headers).get("X-CSRF-Token"), "csrf-token");
  assert.equal(
    client.upgradeBeaconUrl("2.2 + rc", "https://mirror.example/simple/?a=1&b=2"),
    "/apps/qwenpaw/api.cgi?action=upgrade&version=2.2%20%2B%20rc&mirror=https%3A%2F%2Fmirror.example%2Fsimple%2F%3Fa%3D1%26b%3D2&_csrf=csrf-token",
  );
});

test("non-JSON responses fail explicitly", async () => {
  const client = createApiClient({
    pathname: "/index.cgi",
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
    pathname: "/index.cgi",
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

test("network failures are identified separately for the install beacon fallback", async () => {
  const client = createApiClient({
    pathname: "/index.cgi",
    fetch: async () => { throw new TypeError("offline"); },
  });

  await assert.rejects(client.upgrade("2.2.2.post1", ""), (error) => {
    assert.ok(error instanceof ApiClientError);
    assert.equal(error.kind, "network");
    assert.match(error.message, /offline/);
    return true;
  });
});
