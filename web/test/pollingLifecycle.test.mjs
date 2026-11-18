import test from "node:test";
import assert from "node:assert/strict";
import { createPollingLifecycle } from "../src/pollingLifecycle.ts";

function fakeHost() {
  const originalDocument = globalThis.document;
  const originalSetInterval = globalThis.setInterval;
  const originalClearInterval = globalThis.clearInterval;
  const listeners = new Map();
  const timers = new Map();
  let nextId = 1;
  globalThis.document = {
    hidden: false,
    addEventListener(type, callback) { listeners.set(type, callback); },
    removeEventListener(type) { listeners.delete(type); },
  };
  globalThis.setInterval = (callback, delay) => {
    const id = nextId++;
    timers.set(id, { callback, delay });
    return id;
  };
  globalThis.clearInterval = id => { timers.delete(id); };
  return {
    listeners,
    timers,
    restore() {
      globalThis.document = originalDocument;
      globalThis.setInterval = originalSetInterval;
      globalThis.clearInterval = originalClearInterval;
    },
  };
}

test("polling lifecycle is idempotent, pauses in background and resumes immediately", () => {
  const host = fakeHost();
  let refreshes = 0;
  const lifecycle = createPollingLifecycle({ refresh: () => { refreshes += 1; }, intervalMs: 15000, immediate: true });
  try {
    lifecycle.start();
    lifecycle.start();
    assert.equal(refreshes, 1);
    assert.equal(host.timers.size, 1);
    assert.deepEqual([...host.timers.values()][0].delay, 15000);

    globalThis.document.hidden = true;
    host.listeners.get("visibilitychange")();
    assert.equal(host.timers.size, 0);
    assert.equal(lifecycle.running, false);

    globalThis.document.hidden = false;
    host.listeners.get("visibilitychange")();
    assert.equal(refreshes, 2);
    assert.equal(host.timers.size, 1);
    assert.equal(lifecycle.running, true);

    lifecycle.stop();
    assert.equal(host.timers.size, 0);
    lifecycle.destroy();
    assert.equal(host.listeners.size, 0);
  } finally {
    lifecycle.destroy();
    host.restore();
  }
});

test("polling lifecycle never runs a refresh while the document is hidden", () => {
  const host = fakeHost();
  globalThis.document.hidden = true;
  let refreshes = 0;
  const lifecycle = createPollingLifecycle({ refresh: () => { refreshes += 1; }, intervalMs: 30000, immediate: true });
  try {
    lifecycle.start();
    assert.equal(refreshes, 0);
    assert.equal(host.timers.size, 0);
  } finally {
    lifecycle.destroy();
    host.restore();
  }
});

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const flush = () => new Promise(resolve => setImmediate(resolve));

test("slow refreshes stay single-flight and repeated ticks become one trailing refresh", async () => {
  const host = fakeHost();
  const requests = [];
  let concurrent = 0;
  let maximum = 0;
  const lifecycle = createPollingLifecycle({
    refresh: async () => {
      const request = deferred();
      requests.push(request);
      maximum = Math.max(maximum, ++concurrent);
      try { await request.promise; } finally { --concurrent; }
    },
    intervalMs: 15000,
    immediate: true,
  });
  try {
    lifecycle.start();
    const tick = [...host.timers.values()][0].callback;
    for (let index = 0; index < 20; index++) tick();
    const pending = lifecycle.refresh();
    assert.equal(requests.length, 1);
    requests[0].resolve();
    await flush();
    assert.equal(requests.length, 2);
    assert.equal(maximum, 1);
    requests[1].resolve();
    await pending;
    assert.equal(requests.length, 2, "a burst must not create a backlog");
  } finally {
    lifecycle.destroy();
    requests.forEach(request => request.resolve());
    host.restore();
  }
});

for (const action of ["hide", "stop", "destroy"]) {
  test(`${action} cancels a queued refresh after a slow request`, async () => {
    const host = fakeHost();
    const held = deferred();
    let refreshes = 0;
    const lifecycle = createPollingLifecycle({
      refresh: () => { ++refreshes; return held.promise; }, intervalMs: 30000, immediate: true,
    });
    try {
      lifecycle.start();
      const pending = lifecycle.refresh();
      if (action === "hide") {
        globalThis.document.hidden = true;
        host.listeners.get("visibilitychange")();
      } else lifecycle[action]();
      held.resolve();
      await pending;
      assert.equal(refreshes, 1);
      assert.equal(host.timers.size, 0);
      if (action === "destroy") {
        lifecycle.start();
        await lifecycle.refresh();
        lifecycle.destroy();
        assert.equal(refreshes, 1);
        assert.equal(host.listeners.size, 0);
      } else {
        globalThis.document.hidden = false;
        if (action === "stop") lifecycle.start();
        else host.listeners.get("visibilitychange")();
        await flush();
        assert.equal(refreshes, 2, "returning to the foreground or restarting still refreshes");
      }
    } finally {
      lifecycle.destroy();
      held.resolve();
      host.restore();
    }
  });
}

test("foreground resume during an in-flight refresh coalesces without overlapping", async () => {
  const host = fakeHost();
  const held = deferred();
  let refreshes = 0;
  const lifecycle = createPollingLifecycle({
    refresh: () => { ++refreshes; return refreshes === 1 ? held.promise : undefined; },
    intervalMs: 15000, immediate: true,
  });
  try {
    lifecycle.start();
    globalThis.document.hidden = true;
    host.listeners.get("visibilitychange")();
    globalThis.document.hidden = false;
    host.listeners.get("visibilitychange")();
    host.listeners.get("visibilitychange")();
    assert.equal(refreshes, 1);
    assert.equal(host.timers.size, 1);
    held.resolve();
    await flush();
    assert.equal(refreshes, 2);
  } finally {
    lifecycle.destroy();
    host.restore();
  }
});

test("synchronous throws and async rejections are reported and subsequent refreshes recover", async () => {
  const host = fakeHost();
  const originalError = console.error;
  const errors = [];
  console.error = (...args) => errors.push(args);
  let refreshes = 0;
  const lifecycle = createPollingLifecycle({
    refresh: () => {
      if (++refreshes === 1) throw new Error("sync failure");
      if (refreshes === 2) return Promise.reject(new Error("async failure"));
    },
    intervalMs: 30000,
  });
  try {
    lifecycle.start();
    await lifecycle.refresh();
    const rejected = lifecycle.refresh();
    const coalesced = lifecycle.refresh();
    await Promise.all([rejected, coalesced]);
    assert.equal(refreshes, 3);
    assert.equal(errors.length, 2);
    assert.match(errors[0][1].message, /sync failure/);
    assert.match(errors[1][1].message, /async failure/);
  } finally {
    lifecycle.destroy();
    console.error = originalError;
    host.restore();
  }
});
