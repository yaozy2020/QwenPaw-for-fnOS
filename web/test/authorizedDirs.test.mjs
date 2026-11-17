import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { buildAuthorizationUrl, parseAuthCallback, mergeProjectDirs, removeProjectDir, bindAuthorizedDirectory, unlinkAuthorizedDirectory, readAuthorizationResult, authorizeDirectory } from "../src/authorizedDirs.ts";

test("package declares the fnOS file authorization scopes used by the picker", async () => {
  const resource = JSON.parse(await readFile(new URL("../../config/resource", import.meta.url), "utf8"));
  assert.deepEqual(new Set(resource["api-scope"]), new Set([
    "trim.file.sharedAccess",
    "trim.file.userAccess",
    "trim.file.userAcl",
    "trim.file.path",
  ]));
  const manifest = await readFile(new URL("../../manifest", import.meta.url), "utf8");
  assert.match(manifest, /^micro_app\s*=\s*true$/m);
});

test("authorization URL preserves state, directory mode and callback", () => {
  const url = new URL(buildAuthorizationUrl("https://nas.example:5667", "state-1", "/app/qwenpaw/auth-callback.html"));
  assert.equal(url.pathname, "/app-auth/pick-user-file");
  assert.equal(url.searchParams.get("appName"), "qwenpaw");
  assert.equal(url.searchParams.get("directory"), "true");
  assert.equal(url.searchParams.get("redirectUri"), "https://nas.example:5667/app/qwenpaw/auth-callback.html");
  assert.equal(url.searchParams.get("state"), "state-1");
});

test("reauthorization URL targets the selected directory", () => {
  const url = new URL(buildAuthorizationUrl("https://nas.example", "state-2", "/callback", "/vol1/work"));
  assert.equal(url.pathname, "/app-auth/authorize-user-file");
  assert.equal(url.searchParams.get("path"), "/vol1/work");
});

test("callback accepts only the matching state and returns paths", () => {
  const result = parseAuthCallback("?status=success&method=pick-user-file&state=abc&path=%5B%22%2Fvol1%2Fwork%22%5D", "abc");
  assert.deepEqual(result.paths, ["/vol1/work"]);
  const reauthorized = parseAuthCallback("?status=success&method=authorizeUserFile&state=abc", "abc");
  assert.equal(reauthorized.method, "authorizeUserFile");
  assert.deepEqual(reauthorized.paths, []);
  assert.throws(() => parseAuthCallback("?status=success&method=pick-user-file&state=other&path=%5B%22%2Fvol1%2Fwork%22%5D", "abc"));
  assert.throws(() => parseAuthCallback("?status=cancel&method=pick-user-file&state=abc", "abc"));
});

test("merge preserves existing entries and deduplicates by normalized path", () => {
  const existing = [{ path: "/vol1/work", label: "代码" }, { path: "/vol1/docs", label: "资料" }];
  assert.deepEqual(
    mergeProjectDirs(existing, "/vol1/work/", "新名称"),
    [{ path: "/vol1/work", label: "新名称" }, { path: "/vol1/docs", label: "资料" }],
  );
  assert.deepEqual(
    mergeProjectDirs(existing, "/vol1/media", "媒体"),
    [...existing, { path: "/vol1/media", label: "媒体" }],
  );
});

test("remove only unlinks the selected directory", () => {
  const entries = [{ path: "/vol1/work" }, { path: "/vol1/docs" }];
  assert.deepEqual(removeProjectDir(entries, "/vol1/work/"), [{ path: "/vol1/docs" }]);
});

test("removing the last project directory uses the upstream DELETE route", async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (input, init = {}) => {
    calls.push({ url: String(input), method: init.method || "GET" });
    return new Response(JSON.stringify({ project_dirs: [{ path: "/vol1/work" }] }), { status: 200 });
  };
  try {
    await unlinkAuthorizedDirectory("/vol1/work");
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.deepEqual(calls.map(call => call.method), ["GET", "DELETE"]);
});

test("structured upstream errors remain readable", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (_input, init = {}) => {
    if ((init.method || "GET") === "GET") {
      return new Response(JSON.stringify({ project_dirs: [{ path: "/vol1/work" }] }), { status: 200 });
    }
    return new Response(JSON.stringify({ detail: [{ msg: "empty list is not allowed" }] }), { status: 422 });
  };
  try {
    await assert.rejects(unlinkAuthorizedDirectory("/vol1/work"), error => {
      assert.match(error.message, /empty list is not allowed/);
      assert.doesNotMatch(error.message, /\[object Object\]/);
      return true;
    });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("pathless reauthorization callback renders success and closes automatically", async () => {
  const html = await readFile(new URL("../src/auth-callback.html", import.meta.url), "utf8");
  const source = html.match(/<script>([\s\S]*?)<\/script>/)?.[1];
  assert.ok(source);
  const elements = Object.fromEntries(["title", "message", "path", "close"].map(id => [id, {
    textContent: "", hidden: true, addEventListener() {},
  }]));
  const window = {
    location: { search: "?status=success&method=authorizeUserFile&state=abc", origin: "https://nas.example" },
    localStorage: { setItem() {} },
    opener: null,
    closed: false,
    close() { this.closed = true; },
    timers: [],
    setTimeout(callback, milliseconds) { this.timers.push({ callback, milliseconds }); },
  };
  window.parent = window;
  globalThis.fetch = async () => new Response("{}", { status: 200 });
  new Function("window", "document", "location", source)(
    window,
    { getElementById: id => elements[id], cookie: "" },
    window.location,
  );
  assert.equal(elements.title.textContent, "目录授权成功");
  assert.deepEqual(window.timers.map(timer => timer.milliseconds), [300, 1500]);
  window.timers[0].callback();
  assert.equal(window.closed, true);
});

test("callback returns to the console when the WebView blocks window.close", async () => {
  const html = await readFile(new URL("../src/auth-callback.html", import.meta.url), "utf8");
  const source = html.match(/<script>([\s\S]*?)<\/script>/)?.[1];
  assert.ok(source);
  const elements = Object.fromEntries(["title", "message", "path", "close"].map(id => [id, {
    textContent: "", hidden: true, addEventListener() {},
  }]));
  const window = {
    location: { search: "?status=success&method=pickUserFile&state=abc&path=%5B%22%2Fvol1%2Fwork%22%5D", origin: "https://nas.example", replace() {} },
    localStorage: { setItem() {} },
    opener: null,
    closed: false,
    close() {},
    setTimeout(callback, milliseconds) { this.timer = { callback, milliseconds }; },
  };
  window.parent = window;
  window.top = { location: { replace(url) { this.url = url; } } };
  const requests = [];
  globalThis.fetch = async (url, init) => {
    requests.push({ url, init });
    return new Response("{}", { status: 200 });
  };
  new Function("window", "document", "location", source)(
    window,
    { getElementById: id => elements[id], cookie: "__Host-qwenpaw_csrf=test-token" },
    window.location,
  );
  assert.equal(requests[0].url, "/app/qwenpaw/api/authorized-dirs/result");
  assert.equal(requests[0].init.method, "POST");
  assert.equal(requests[0].init.headers["X-CSRF-Token"], "test-token");
  assert.match(requests[0].init.body, /"state":"abc"/);
  window.timer.callback();
  assert.equal(window.top.location.url, "/app/qwenpaw/#authorization");
});

test("authorization result polling reads the same-origin hand-off", async () => {
  const originalFetch = globalThis.fetch;
  const urls = [];
  globalThis.fetch = async url => {
    urls.push(String(url));
    return new Response(JSON.stringify({ ok: true, state: "abc", search: "?status=success&state=abc" }), { status: 200 });
  };
  try {
    assert.equal(await readAuthorizationResult("abc"), "?status=success&state=abc");
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.deepEqual(urls, ["/app/qwenpaw/api/authorized-dirs/result?state=abc"]);
});


test("binding uses upstream validation without a separate permission gate", async () => {
  const originalFetch = globalThis.fetch;
  const calls = [];
  globalThis.fetch = async (input, init = {}) => {
    const url = String(input);
    const method = init.method || "GET";
    calls.push({ url, method, body: init.body });
    if (url === "/app/qwenpaw/web/api/workspace/project-directory/dirs" && method === "GET") {
      return new Response(JSON.stringify({ project_dirs: [] }), { status: 200 });
    }
    if (url === "/app/qwenpaw/web/api/workspace/project-directory/dirs" && method === "PUT") {
      return new Response(JSON.stringify({ project_dirs: [{ path: "/vol1/work" }] }), { status: 200 });
    }
    throw new Error(`unexpected request: ${method} ${url}`);
  };
  try {
    await bindAuthorizedDirectory("/vol1/work");
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.deepEqual(calls.map(call => `${call.method} ${call.url}`), [
    "GET /app/qwenpaw/web/api/workspace/project-directory/dirs",
    "PUT /app/qwenpaw/web/api/workspace/project-directory/dirs",
  ]);
  assert.deepEqual(JSON.parse(calls[1].body), { project_dirs: [{ path: "/vol1/work" }] });
});


test("binding strips read-only metadata returned by the upstream GET", async () => {
  const originalFetch = globalThis.fetch;
  let savedBody = "";
  globalThis.fetch = async (_input, init = {}) => {
    if ((init.method || "GET") === "GET") {
      return new Response(JSON.stringify({ project_dirs: [{
        path: "/vol1/existing",
        label: "已有目录",
        exists: true,
        nested_with: null,
        is_workspace: false,
      }] }), { status: 200 });
    }
    savedBody = String(init.body || "");
    return new Response(JSON.stringify({ project_dirs: [] }), { status: 200 });
  };
  try {
    await bindAuthorizedDirectory("/vol2/new");
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.deepEqual(JSON.parse(savedBody), { project_dirs: [
    { path: "/vol1/existing", label: "已有目录" },
    { path: "/vol2/new" },
  ] });
});

test("embedded callback does not reload the console before directory binding finishes", async () => {
  const html = await readFile(new URL("../src/auth-callback.html", import.meta.url), "utf8");
  const source = html.match(/<script>([\s\S]*?)<\/script>/)?.[1];
  assert.ok(source);
  const elements = Object.fromEntries(["title", "message", "path", "close"].map(id => [id, {
    textContent: "", hidden: true, addEventListener() {},
  }]));
  const messages = [];
  const window = {
    location: { search: "?status=success&method=pickUserFile&state=abc&path=%5B%22%2Fvol1%2Fwork%22%5D", origin: "https://nas.example" },
    localStorage: { setItem() {} },
    opener: null,
    parent: { postMessage(payload) { messages.push(payload); } },
    top: { location: { replace() { throw new Error("must not reload the embedding console"); } } },
    closed: false,
    close() {},
    timers: [],
    setTimeout(callback, milliseconds) { this.timers.push({ callback, milliseconds }); },
  };
  globalThis.fetch = async () => new Response("{}", { status: 200 });
  new Function("window", "document", "location", source)(
    window,
    { getElementById: id => elements[id], cookie: "" },
    window.location,
  );
  assert.equal(messages.length, 1);
  window.timers.find(timer => timer.milliseconds === 1500).callback();
});

test("authorization accepts a structured result when the host omits the raw query", async () => {
  const originalWindow = globalThis.window;
  const originalDocument = globalThis.document;
  const originalStorage = globalThis.localStorage;
  const originalFetch = globalThis.fetch;
  const events = new EventTarget();
  let state = "";
  const popup = {
    closed: false,
    location: { replace(url) { state = new URL(url).searchParams.get("state") || ""; } },
    close() { this.closed = true; },
  };
  Object.assign(events, {
    parent: events,
    location: { origin: "https://nas.example" },
    open: () => popup,
    setInterval,
    clearInterval,
    setTimeout,
    clearTimeout,
  });
  const calls = [];
  globalThis.window = events;
  globalThis.document = new EventTarget();
  globalThis.document.hidden = false;
  globalThis.localStorage = { getItem() { return null; }, removeItem() {} };
  globalThis.fetch = async (url, init = {}) => {
    const method = init.method || "GET";
    calls.push({ url: String(url), method, body: init.body });
    if (String(url).includes("/workspace/project-directory/dirs")) {
      const entries = method === "PUT" ? [{ path: "/vol1/work", label: "" }] : [];
      return new Response(JSON.stringify({ project_dirs: entries }), { status: 200 });
    }
    return new Response("{}", { status: 404 });
  };
  try {
    const pending = authorizeDirectory();
    events.dispatchEvent(new MessageEvent("message", {
      origin: "https://nas.example",
      data: {
        type: "qwenpaw:authorized-directory",
        state,
        result: { status: "success", error: "", method: "pickUserFile", state, path: ["/vol1/work"] },
      },
    }));
    const result = await pending;
    assert.deepEqual(result.paths, ["/vol1/work"]);
    await bindAuthorizedDirectory(result.paths[0]);
    const save = calls.find(call => call.method === "PUT");
    assert.ok(save, "structured authorization result must reach the upstream save request");
    assert.deepEqual(JSON.parse(save.body), { project_dirs: [{ path: "/vol1/work" }] });
  } finally {
    globalThis.window = originalWindow;
    globalThis.document = originalDocument;
    globalThis.localStorage = originalStorage;
    globalThis.fetch = originalFetch;
  }
});

test("authorization does not miss a fast callback during surface navigation", async () => {
  const originalWindow = globalThis.window;
  const originalDocument = globalThis.document;
  const originalStorage = globalThis.localStorage;
  const originalFetch = globalThis.fetch;
  const events = new EventTarget();
  const timers = [];
  let pending;
  const popup = {
    closed: false,
    location: {
      replace(url) {
        const state = new URL(url).searchParams.get("state") || "";
        const search = `?status=success&method=pickUserFile&state=${state}&path=%5B%22%2Fvol1%2Fwork%22%5D`;
        events.dispatchEvent(new MessageEvent("message", {
          origin: "https://nas.example",
          data: { type: "qwenpaw:authorized-directory", state, result: { state }, search },
        }));
      },
    },
    close() { this.closed = true; },
  };
  Object.assign(events, {
    parent: events,
    location: { origin: "https://nas.example" },
    open: () => popup,
    setInterval,
    clearInterval,
    setTimeout(callback) { timers.push(callback); return timers.length; },
    clearTimeout() {},
  });
  globalThis.window = events;
  globalThis.document = new EventTarget();
  globalThis.document.hidden = false;
  globalThis.localStorage = { getItem() { return null; }, removeItem() {} };
  globalThis.fetch = () => new Promise(() => {});
  try {
    pending = authorizeDirectory();
    const outcome = await Promise.race([
      pending.then(() => "settled", () => "settled"),
      new Promise(resolve => setTimeout(() => resolve("timeout"), 50)),
    ]);
    assert.equal(outcome, "settled", "the listener must be installed before navigation can complete");
  } finally {
    for (const callback of timers) callback();
    await pending?.catch(() => {});
    globalThis.window = originalWindow;
    globalThis.document = originalDocument;
    globalThis.localStorage = originalStorage;
    globalThis.fetch = originalFetch;
  }
});

test("callback reports result publication failure and stays open", async () => {
  const html = await readFile(new URL("../src/auth-callback.html", import.meta.url), "utf8");
  const source = html.match(/<script>([\s\S]*?)<\/script>/)?.[1];
  assert.ok(source);
  const originalFetch = globalThis.fetch;
  const elements = Object.fromEntries(["title", "message", "path", "close"].map(id => [id, {
    textContent: "", hidden: true, listeners: {}, addEventListener(name, callback) { this.listeners[name] = callback; },
  }]));
  const window = {
    location: { search: "?status=success&method=pickUserFile&state=abc&path=%5B%22%2Fvol1%2Fwork%22%5D", origin: "https://nas.example" },
    localStorage: { setItem() {} },
    opener: null,
    closed: false,
    close() { this.closed = true; },
    timers: [],
    setTimeout(callback, milliseconds) { this.timers.push({ callback, milliseconds }); },
  };
  window.parent = window;
  globalThis.fetch = async () => new Response("{}", { status: 503 });
  try {
    new Function("window", "document", "location", source)(
      window,
      { getElementById: id => elements[id], cookie: "" },
      window.location,
    );
    await Promise.resolve();
    await Promise.resolve();
    assert.match(elements.title.textContent, /回传失败/);
    assert.match(elements.message.textContent, /503/);
    window.timers.find(timer => timer.milliseconds === 300).callback();
    assert.equal(window.closed, false, "failed publication must not hide the diagnostic by closing the callback");
  } finally {
    globalThis.fetch = originalFetch;
  }
});
