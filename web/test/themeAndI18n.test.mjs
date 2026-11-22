import test from "node:test";
import assert from "node:assert/strict";
import {
  THEME_STORAGE_KEY,
  UI_STYLE_STORAGE_KEY,
  nextThemeMode,
  parseThemeMode,
  parseUiStyle,
  readThemeMode,
  readUiStyle,
  resolveTheme,
  themeColorFor,
  writeThemeMode,
  writeUiStyle,
} from "../src/theme.ts";
import {
  LOCALE_STORAGE_KEY,
  detectBrowserLocale,
  formatMessage,
  localizeBackendMessage,
  normalizeLocale,
  readLocaleMode,
  readStoredLocale,
  resolveInitialLocale,
  tFor,
  writeStoredLocale,
} from "../src/i18n.ts";
import { formatUptime } from "../src/statusStore.ts";

function memoryStorage(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => { map.set(key, String(value)); },
    removeItem: (key) => { map.delete(key); },
  };
}

test("theme: default mode is system and resolves against OS prefers-color-scheme", () => {
  const storage = memoryStorage();
  assert.equal(readThemeMode(storage), "system");
  assert.equal(parseThemeMode("invalid"), "system");
  assert.equal(resolveTheme("system", false), "light");
  assert.equal(resolveTheme("system", true), "dark");
  assert.equal(resolveTheme("light", true), "light");
  assert.equal(resolveTheme("dark", false), "dark");
  assert.equal(themeColorFor("light"), "#f2f1ed");
  assert.equal(themeColorFor("dark"), "#141414");
});

test("theme: header toggle flips directly between light and dark without cycling system", () => {
  // OS light: system -> dark <-> light
  assert.equal(nextThemeMode("system", false), "dark");
  assert.equal(nextThemeMode("dark", false), "light");
  assert.equal(nextThemeMode("light", false), "dark");

  // OS dark: system -> light <-> dark
  assert.equal(nextThemeMode("system", true), "light");
  assert.equal(nextThemeMode("light", true), "dark");
  assert.equal(nextThemeMode("dark", true), "light");
});

test("theme: writeThemeMode persists manual choices and clears storage for system", () => {
  const storage = memoryStorage();
  writeThemeMode("dark", storage);
  assert.equal(storage.getItem(THEME_STORAGE_KEY), "dark");
  assert.equal(readThemeMode(storage), "dark");

  writeThemeMode("system", storage);
  assert.equal(storage.getItem(THEME_STORAGE_KEY), null);
  assert.equal(readThemeMode(storage), "system");

  const blocked = {
    getItem() { throw new Error("blocked"); },
    setItem() { throw new Error("blocked"); },
    removeItem() { throw new Error("blocked"); },
  };
  assert.equal(readThemeMode(blocked), "system");
  assert.doesNotThrow(() => writeThemeMode("dark", blocked));
});

test("theme: uiStyle defaults to default, persists miuix, and resolves Miuix theme-color", () => {
  const storage = memoryStorage();
  assert.equal(readUiStyle(storage), "default");
  assert.equal(parseUiStyle("miuix"), "miuix");
  assert.equal(parseUiStyle("unknown"), "default");
  assert.equal(themeColorFor("light", "miuix"), "#f4f3f0");
  assert.equal(themeColorFor("dark", "miuix"), "#131315");

  writeUiStyle("miuix", storage);
  assert.equal(storage.getItem(UI_STYLE_STORAGE_KEY), "miuix");
  assert.equal(readUiStyle(storage), "miuix");

  writeUiStyle("default", storage);
  assert.equal(storage.getItem(UI_STYLE_STORAGE_KEY), null);
  assert.equal(readUiStyle(storage), "default");

  const blocked = {
    getItem() { throw new Error("blocked"); },
    setItem() { throw new Error("blocked"); },
    removeItem() { throw new Error("blocked"); },
  };
  assert.equal(readUiStyle(blocked), "default");
  assert.doesNotThrow(() => writeUiStyle("miuix", blocked));
});

test("i18n: locale detection, storage precedence, and interpolation", () => {
  assert.equal(normalizeLocale("zh-Hans-CN"), "zh-CN");
  assert.equal(normalizeLocale("en-GB"), "en");
  assert.equal(normalizeLocale("fr-FR"), null);

  assert.equal(detectBrowserLocale(["en-US", "zh-CN"], "zh-CN"), "en");
  assert.equal(detectBrowserLocale(["ja-JP", "zh-TW"], "en-US"), "zh-CN");
  assert.equal(detectBrowserLocale([], "fr-FR"), "zh-CN");

  const storage = memoryStorage();
  assert.equal(readLocaleMode(storage), "system");
  assert.equal(resolveInitialLocale(storage, ["en-US"], "en-US"), "en");
  writeStoredLocale("zh-CN", storage);
  assert.equal(storage.getItem(LOCALE_STORAGE_KEY), "zh-CN");
  assert.equal(readLocaleMode(storage), "zh-CN");
  assert.equal(readStoredLocale(storage), "zh-CN");
  assert.equal(resolveInitialLocale(storage, ["en-US"], "en-US"), "zh-CN");
  writeStoredLocale(null, storage);
  assert.equal(readLocaleMode(storage), "system");
  assert.equal(readStoredLocale(storage), null);

  assert.equal(formatMessage("v{version} ({count})", { version: "2.3.0", count: 3 }), "v2.3.0 (3)");
  assert.equal(tFor("zh-CN", "overview.uptimePrefix", { uptime: "10秒" }), "已运行 10秒");
  assert.equal(tFor("en", "overview.uptimePrefix", { uptime: "10s" }), "Up 10s");
});

test("i18n: formatUptime supports zh-CN and en locales", () => {
  assert.equal(formatUptime(45, "zh-CN"), "45秒");
  assert.equal(formatUptime(45, "en"), "45s");
  assert.equal(formatUptime(125, "zh-CN"), "2分 5秒");
  assert.equal(formatUptime(125, "en"), "2m 5s");
  assert.equal(formatUptime(90061, "zh-CN"), "1天 01:01");
  assert.equal(formatUptime(90061, "en"), "1d 01:01");
});

test("i18n: localizeBackendMessage translates known codes and falls back to raw message", () => {
  assert.equal(localizeBackendMessage("service_started", "QwenPaw 已启动", "en"), "QwenPaw started");
  assert.equal(localizeBackendMessage("service_started", "QwenPaw 已启动", "zh-CN"), "QwenPaw 已启动");
  assert.equal(localizeBackendMessage("port_in_use", "端口已被内核或管理器使用", "en"), "Port is already used by the kernel or supervisor");
  assert.equal(localizeBackendMessage("unknown_code", "custom detail", "en"), "custom detail");
  assert.equal(localizeBackendMessage(undefined, "raw error", "en"), "raw error");
});
