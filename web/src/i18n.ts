import { ref } from "vue";

export type Locale = "zh-CN" | "en";
export type LocaleMode = "system" | Locale;

export const LOCALE_STORAGE_KEY = "qwenpaw_locale_v1";

import { zhCN, type MessageKey } from "./locales/zh-CN.ts";
import { en } from "./locales/en.ts";

export type { MessageKey };

const dictionaries: Record<Locale, Record<MessageKey, string>> = {
  "zh-CN": zhCN,
  en,
};

export function normalizeLocale(value: unknown): Locale | null {
  if (typeof value !== "string") return null;
  const lower = value.trim().toLowerCase();
  if (!lower) return null;
  if (lower.startsWith("zh")) return "zh-CN";
  if (lower.startsWith("en")) return "en";
  return null;
}

export function detectBrowserLocale(languages?: readonly string[] | null, language?: string | null): Locale {
  const candidates: string[] = [];
  if (languages) {
    for (const item of languages) candidates.push(item);
  }
  if (language) candidates.push(language);
  for (const item of candidates) {
    const matched = normalizeLocale(item);
    if (matched) return matched;
  }
  return "zh-CN";
}

export function readStoredLocale(storage?: Pick<Storage, "getItem"> | null): Locale | null {
  try {
    return normalizeLocale(storage?.getItem(LOCALE_STORAGE_KEY) ?? null);
  } catch {
    return null;
  }
}

export function writeStoredLocale(locale: Locale | null, storage?: Pick<Storage, "setItem" | "removeItem"> | null): void {
  try {
    if (locale === null) storage?.removeItem(LOCALE_STORAGE_KEY);
    else storage?.setItem(LOCALE_STORAGE_KEY, locale);
  } catch {
    /* Storage may be blocked; keep the choice in memory for this page. */
  }
}

export function readLocaleMode(storage?: Pick<Storage, "getItem"> | null): LocaleMode {
  return readStoredLocale(storage) ?? "system";
}

export function resolveInitialLocale(
  storage?: Pick<Storage, "getItem"> | null,
  languages?: readonly string[] | null,
  language?: string | null,
): Locale {
  return readStoredLocale(storage) ?? detectBrowserLocale(languages, language);
}

export function formatMessage(template: string, params?: Record<string, string | number>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : match,
  );
}

function safeLocaleStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try { return window.localStorage; } catch { return null; }
}

function browserDetectedLocale(): Locale {
  return detectBrowserLocale(
    typeof navigator !== "undefined" ? navigator.languages : null,
    typeof navigator !== "undefined" ? navigator.language : null,
  );
}

function initialLocaleState(): { mode: LocaleMode; locale: Locale } {
  if (typeof window === "undefined") return { mode: "system", locale: "zh-CN" };
  const stored = readStoredLocale(safeLocaleStorage());
  return stored ? { mode: stored, locale: stored } : { mode: "system", locale: browserDetectedLocale() };
}

const initial = initialLocaleState();
export const localeMode = ref<LocaleMode>(initial.mode);
export const currentLocale = ref<Locale>(initial.locale);

export function syncDocumentLocale(locale: Locale = currentLocale.value, mode: LocaleMode = localeMode.value): void {
  if (typeof document === "undefined") return;
  document.documentElement.lang = locale;
  document.documentElement.dataset.localeMode = mode;
}

export function setLocaleMode(mode: LocaleMode, persist = true): Locale {
  localeMode.value = mode;
  const resolved: Locale = mode === "system" ? browserDetectedLocale() : mode;
  currentLocale.value = resolved;
  if (persist) {
    writeStoredLocale(mode === "system" ? null : mode, safeLocaleStorage());
  }
  syncDocumentLocale(resolved, mode);
  return resolved;
}

export function setLocale(locale: Locale, persist = true): void {
  setLocaleMode(locale, persist);
}

export function toggleLocale(): Locale {
  const next: Locale = currentLocale.value === "zh-CN" ? "en" : "zh-CN";
  return setLocaleMode(next, true);
}

export function syncLocaleFromStorage(): void {
  if (typeof window === "undefined") return;
  const stored = readStoredLocale(safeLocaleStorage());
  localeMode.value = stored ?? "system";
  currentLocale.value = stored ?? browserDetectedLocale();
  syncDocumentLocale(currentLocale.value, localeMode.value);
}

export function syncLocaleFromSystem(): void {
  if (localeMode.value !== "system") return;
  currentLocale.value = browserDetectedLocale();
  syncDocumentLocale(currentLocale.value, localeMode.value);
}

export function t(key: MessageKey, params?: Record<string, string | number>): string {
  const dict = dictionaries[currentLocale.value] ?? zhCN;
  return formatMessage(dict[key] ?? zhCN[key], params);
}

export function tFor(locale: Locale, key: MessageKey, params?: Record<string, string | number>): string {
  return formatMessage(dictionaries[locale][key], params);
}

const backendCodeKeys: Readonly<Record<string, MessageKey>> = {
  operation_in_progress: "backend.operation_in_progress",
  recovery_pending: "backend.recovery_pending",
  install_busy: "backend.install_busy",
  kernel_not_installed: "backend.kernel_not_installed",
  service_started: "backend.service_started",
  service_already_running: "backend.service_already_running",
  service_stopped: "backend.service_stopped",
  service_restarted: "backend.service_restarted",
  logs_cleared: "backend.logs_cleared",
  install_started: "backend.install_started",
  kernel_uninstalled: "backend.kernel_uninstalled",
  native_auth_uninitialized: "backend.native_auth_uninitialized",
  native_auth_mismatch: "backend.native_auth_mismatch",
  native_auth_unconfirmed: "backend.native_auth_unconfirmed",
  native_auth_no_ack: "backend.native_auth_no_ack",
  mirror_saved: "backend.mirror_saved",
  port_in_use: "backend.port_in_use",
  web_proxy_disabled: "backend.web_proxy_disabled",
  web_proxy_port_changed: "backend.web_proxy_port_changed",
  csrf_refreshed: "backend.csrf_refreshed",
  native_auth_adopted_restart_required: "backend.native_auth_adopted_restart_required",
};

export function localizeBackendMessage(code: unknown, fallback?: string, locale: Locale = currentLocale.value): string {
  if (typeof code === "string" && Object.prototype.hasOwnProperty.call(backendCodeKeys, code)) {
    return tFor(locale, backendCodeKeys[code]);
  }
  return fallback || "";
}
