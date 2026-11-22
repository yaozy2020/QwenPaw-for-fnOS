import { localizeBackendMessage, t } from "./i18n";

export function operationSucceeded<T extends { success?: boolean; code?: string; message?: string }>(
  result: { ok: boolean; status: number; data: T },
): T {
  if (!result.ok || result.data.success !== true) {
    throw new Error(localizeBackendMessage(result.data.code, result.data.message) || t("common.opFailed", { status: result.status }));
  }
  return result.data;
}
