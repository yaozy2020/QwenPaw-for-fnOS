import { computed, ref } from "vue";

export type Theme = "light" | "dark";
export type ThemeMode = "system" | Theme;
export type UiStyle = "default" | "miuix";

export const THEME_STORAGE_KEY = "qwenpaw_theme_v2";
export const LEGACY_THEME_STORAGE_KEY = "qwenpaw_theme";
export const UI_STYLE_STORAGE_KEY = "qwenpaw_ui_style_v1";

export function parseThemeMode(value: unknown): ThemeMode {
  return value === "light" || value === "dark" ? value : "system";
}

export function readThemeMode(storage?: Pick<Storage, "getItem"> | null): ThemeMode {
  try {
    return parseThemeMode(storage?.getItem(THEME_STORAGE_KEY) ?? null);
  } catch {
    return "system";
  }
}

export function writeThemeMode(mode: ThemeMode, storage?: Pick<Storage, "setItem" | "removeItem"> | null): void {
  try {
    if (mode === "system") storage?.removeItem(THEME_STORAGE_KEY);
    else storage?.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    /* Storage may be blocked; keep the choice in memory for this session. */
  }
}

export function parseUiStyle(value: unknown): UiStyle {
  return value === "miuix" ? "miuix" : "default";
}

export function readUiStyle(storage?: Pick<Storage, "getItem"> | null): UiStyle {
  try {
    return parseUiStyle(storage?.getItem(UI_STYLE_STORAGE_KEY) ?? null);
  } catch {
    return "default";
  }
}

export function writeUiStyle(style: UiStyle, storage?: Pick<Storage, "setItem" | "removeItem"> | null): void {
  try {
    if (style === "default") storage?.removeItem(UI_STYLE_STORAGE_KEY);
    else storage?.setItem(UI_STYLE_STORAGE_KEY, style);
  } catch {
    /* Storage may be blocked; keep the choice in memory for this session. */
  }
}

function safeThemeStorage(): Storage | null {
  try {
    return typeof window !== "undefined" ? window.localStorage : null;
  } catch {
    return null;
  }
}

function detectSystemDark(): boolean {
  try {
    return typeof window !== "undefined" && !!window.matchMedia?.("(prefers-color-scheme: dark)").matches;
  } catch {
    return false;
  }
}

export function resolveTheme(mode: ThemeMode, systemDark: boolean): Theme {
  return mode === "system" ? (systemDark ? "dark" : "light") : mode;
}

/**
 * Header toggle flips directly between light and dark based on the currently
 * resolved theme; returning to "system" (follow OS) lives in Settings.
 */
export function nextThemeMode(mode: ThemeMode, systemDark: boolean): Theme {
  return resolveTheme(mode, systemDark) === "dark" ? "light" : "dark";
}

export function themeColorFor(theme: Theme, style: UiStyle = "default"): string {
  if (style === "miuix") return theme === "dark" ? "#131315" : "#f4f3f0";
  return theme === "dark" ? "#141414" : "#f2f1ed";
}

export const themeMode = ref<ThemeMode>(
  typeof window === "undefined" ? "system" : readThemeMode(safeThemeStorage()),
);
export const systemDark = ref<boolean>(detectSystemDark());
export const currentTheme = computed<Theme>(() => resolveTheme(themeMode.value, systemDark.value));

export const uiStyle = ref<UiStyle>(
  typeof window === "undefined" ? "default" : readUiStyle(safeThemeStorage()),
);

export function syncDocumentTheme(
  resolved: Theme = currentTheme.value,
  mode: ThemeMode = themeMode.value,
  style: UiStyle = uiStyle.value,
): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.dataset.themeMode = mode;
  document.documentElement.dataset.uiStyle = style;
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute("content", themeColorFor(resolved, style));
}

export function setThemeMode(mode: ThemeMode, persist = true): Theme {
  const normalized = parseThemeMode(mode);
  themeMode.value = normalized;
  if (persist) writeThemeMode(normalized, safeThemeStorage());
  const resolved = resolveTheme(normalized, systemDark.value);
  syncDocumentTheme(resolved, normalized, uiStyle.value);
  return resolved;
}

export function toggleTheme(persist = true): Theme {
  return setThemeMode(nextThemeMode(themeMode.value, systemDark.value), persist);
}

export function syncThemeFromStorage(): Theme {
  if (typeof window === "undefined") return currentTheme.value;
  themeMode.value = readThemeMode(safeThemeStorage());
  syncDocumentTheme(currentTheme.value, themeMode.value, uiStyle.value);
  return currentTheme.value;
}

export function syncThemeFromSystem(isDark: boolean): Theme {
  systemDark.value = isDark;
  syncDocumentTheme(currentTheme.value, themeMode.value, uiStyle.value);
  return currentTheme.value;
}

export function syncDocumentUiStyle(style: UiStyle = uiStyle.value): void {
  syncDocumentTheme(currentTheme.value, themeMode.value, style);
}

export function setUiStyle(style: UiStyle, persist = true): UiStyle {
  const normalized = parseUiStyle(style);
  uiStyle.value = normalized;
  if (persist) writeUiStyle(normalized, safeThemeStorage());
  syncDocumentTheme(currentTheme.value, themeMode.value, normalized);
  return normalized;
}

export function syncUiStyleFromStorage(): UiStyle {
  if (typeof window === "undefined") return uiStyle.value;
  const stored = readUiStyle(safeThemeStorage());
  uiStyle.value = stored;
  syncDocumentTheme(currentTheme.value, themeMode.value, stored);
  return stored;
}
