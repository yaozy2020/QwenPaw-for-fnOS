<script setup lang="ts">
import { useTemplateRef } from 'vue';
const dialog = useTemplateRef<HTMLDialogElement>('dialog');
function open() {
  if (dialog.value && !dialog.value.open) dialog.value.showModal();
}
defineExpose({ open });
</script>

<template>
  <dialog ref="dialog" id="aboutDialog" class="about-dialog" aria-labelledby="aboutTitle">
    <div class="about-header">
      <h2 id="aboutTitle">关于 QwenPaw</h2>
      <p>QwenPaw for fnOS 是第三方部署外壳，负责安装、运行和运维管理；QwenPaw 本体由所选 PyPI 镜像安装。</p>
    </div>
    <div class="about-links" aria-label="项目地址">
      <a href="https://github.com/agentscope-ai/QwenPaw" target="_blank" rel="noopener noreferrer">
        <strong>上游 QwenPaw 官网 / 项目</strong><span>github.com/agentscope-ai/QwenPaw</span>
      </a>
      <a href="https://github.com/yaozy2020/QwenPaw-for-fnOS" target="_blank" rel="noopener noreferrer">
        <strong>QwenPaw for fnOS 项目</strong><span>github.com/yaozy2020/QwenPaw-for-fnOS</span>
      </a>
    </div>
    <section class="faq" aria-labelledby="faqTitle">
      <h3 id="faqTitle">常见问题处理</h3>
      <details>
        <summary>无法打开 Web 对话</summary>
        <p>先在概览确认内核已安装且正在运行，再刷新状态。仍无法打开时，到运行日志检查端口、健康检查和网关错误。</p>
      </details>
      <details>
        <summary>安装或更新失败</summary>
        <p>检查 NAS 网络与 DNS，在版本管理中切换下载源后重试。已有任务时先打开任务详情，不要重复提交安装。</p>
      </details>
      <details>
        <summary>忘记 QwenPaw 管理员密码</summary>
        <p>用飞牛管理员打开管理台，在“配置 → 访问鉴权”重置 QwenPaw 管理员密码，再用原用户名和新密码登录。无需旧密码，不影响飞牛账号、配置或记忆。</p>
      </details>
      <details>
        <summary>升级后出现权限或运行异常</summary>
        <p>保留日志和用户数据，不要手工删除 venv 或共享目录。记录错误时间与提示，到项目 Issues 附上日志反馈。</p>
      </details>
    </section>
    <form method="dialog" class="about-actions">
      <button type="submit" class="btn-primary" autofocus>关闭</button>
    </form>
  </dialog>
</template>
