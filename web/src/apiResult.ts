export function operationSucceeded<T extends { success?: boolean; message?: string }>(
  result: { ok: boolean; status: number; data: T },
): T {
  if (!result.ok || result.data.success !== true) {
    throw new Error(result.data.message || `操作失败（HTTP ${result.status}）`);
  }
  return result.data;
}
