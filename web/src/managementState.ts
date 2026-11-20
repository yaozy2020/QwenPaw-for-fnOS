import { reactive } from "vue";

const authStates = {
  initial: { label: "状态获取中...", admin: "-", tone: "text-muted", description: "本控制台使用飞牛账号登录；QwenPaw 对话端使用独立管理员账号，两套密码不会自动同步。" },
  uninitialized: { label: "已开启（待设置密码）", admin: "尚未创建管理员", tone: "warning", description: "对话端已要求登录验证，但还没有管理员。请进入对话端完成初始设置；该账号独立于飞牛账号。" },
  protected: { label: "已开启（需要密码）", admin: "管理员已创建", tone: "success", description: "忘记密码时，可在此为原 QwenPaw 管理员设置新密码。无需重新注册，不影响飞牛账号、配置或记忆。" },
  disabled: { label: "未开启（公开访问）", admin: "未启用账号验证", tone: "danger-text", description: "任何能访问该端口的人无需密码即可使用对话端。如需保护，请在对话端设置 QwenPaw 管理员。" },
  unknown: { label: "无法确认", admin: "无法读取", tone: "text-muted", description: "内核未运行或状态接口不可达，暂时无法确认密码状态；这不代表验证已关闭。请启动内核后再确认。" },
  offline: { label: "无法获取", admin: "无法获取", tone: "text-muted", description: "控制台无法连接监督者（可能服务重启中或会话已过期）。这不代表内核已停止，稍后会自动重试。" },
};

export function authPresentation(status: string) {
  return status === "protected" || status === "disabled" || status === "uninitialized" || status === "offline"
    ? authStates[status] : authStates.unknown;
}

export function createManagementState() {
  return reactive({
    servicePort: "18088", serviceReachable: false, backupAvailable: false, auth: authStates.initial,
    proxyPort: null as number | null, proxyPublic: null as boolean | null, proxyDraft: false,
    proxyPasswordConfigured: null as boolean | null,
    entryLogin: null as boolean | null,
    pending: false, saving: false,
  });
}
export type ManagementState = ReturnType<typeof createManagementState>;
