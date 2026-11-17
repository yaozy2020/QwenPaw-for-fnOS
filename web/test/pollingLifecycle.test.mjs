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
