import { watchEffect } from "vue";
import { TrimApp } from "@trimjs/web-app";

export function createHostTitle(getSectionTitle: () => string, app = new TrimApp()) {
  let current = "";
  return watchEffect(() => {
    const section = getSectionTitle().trim();
    const title = section ? `${section} · QwenPaw` : "QwenPaw";
    if (title === current) return;
    current = title;
    document.title = title;
    if (!app.isStandaloneWeb) void app.setTitle(title).catch(() => {});
  });
}
