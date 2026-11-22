import test from "node:test";
import assert from "node:assert/strict";
import { nextTick, ref } from "vue";

async function documentFixture(t) {
  const previousDocument = globalThis.document;
  const previousWindow = globalThis.window;
  const previousNavigator = globalThis.navigator;
  globalThis.document = { title: "" };
  globalThis.window = { addEventListener() {} };
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { userAgent: "" } });
  const { createHostTitle } = await import("../src/hostTitle.ts");
  t.after(() => {
    globalThis.document = previousDocument;
    globalThis.window = previousWindow;
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: previousNavigator });
  });
  return { document: globalThis.document, createHostTitle };
}

test("host title forwards section changes to the fnOS SDK", async t => {
  const { document, createHostTitle } = await documentFixture(t);
  const section = ref("概览");
  const calls = [];
  const stop = createHostTitle(() => section.value, {
    isStandaloneWeb: false,
    setTitle: async title => { calls.push(title); },
  });
  t.after(stop);

  assert.equal(document.title, "概览 · QwenPaw");
  section.value = "配置";
  await nextTick();
  assert.equal(document.title, "配置 · QwenPaw");
  section.value = "";
  await nextTick();
  assert.equal(document.title, "QwenPaw");
  assert.deepEqual(calls, ["概览 · QwenPaw", "配置 · QwenPaw", "QwenPaw"]);
});

test("standalone browser updates document.title without a host call", async t => {
  const { document, createHostTitle } = await documentFixture(t);
  const section = ref("Overview");
  const calls = [];
  const stop = createHostTitle(() => section.value, {
    isStandaloneWeb: true,
    setTitle: async title => { calls.push(title); },
  });
  t.after(stop);

  section.value = "Logs";
  await nextTick();
  assert.equal(document.title, "Logs · QwenPaw");
  assert.deepEqual(calls, []);
});
