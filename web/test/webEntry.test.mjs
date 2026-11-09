import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const [html, main, desktopConfig] = await Promise.all([
  readFile(new URL("../src/index.html", import.meta.url), "utf8"),
  readFile(new URL("../src/main.ts", import.meta.url), "utf8"),
  readFile(new URL("../../app/ui/config", import.meta.url), "utf8").then(JSON.parse),
]);

const gatewayPrefix = "/app/qwenpaw";
const webEntryPath = `${gatewayPrefix}/web/`;

test("console entry stays same-origin and does not open a port-based popup", () => {
  const button = html.match(/<a\b[^>]*\bid="btnWebConsole"[^>]*>/)?.[0];
  assert.ok(button, "Web console entry anchor exists");
  assert.match(button, /href="\/app\/qwenpaw\/web\/"/);
  assert.doesNotMatch(button, /\btarget=/);
  assert.doesNotMatch(main, /btnConsole\.href\s*=\s*`http:\/\/\$\{host\}:\$\{activePort\}`/);
});

test("desktop main entry shares the console Gateway prefix and socket without a bare port URL", () => {
  const entries = desktopConfig[".url"];
  const consoleEntry = entries["qwenpaw.console"];
  const mainEntry = entries["qwenpaw.main"];

  assert.equal(mainEntry.type, "iframe");
  assert.equal(mainEntry.protocol, "");
  assert.equal(mainEntry.port, undefined);
  assert.equal(mainEntry.url, webEntryPath);
  assert.equal(mainEntry.url, `${mainEntry.gatewayPrefix}/web/`);
  assert.equal(mainEntry.gatewayPrefix, consoleEntry.gatewayPrefix);
  assert.equal(mainEntry.gatewaySocket, consoleEntry.gatewaySocket);
  assert.doesNotMatch(mainEntry.url, /^https?:\/\/|:\d+/);
  assert.equal(mainEntry.noDisplay, false);
});
