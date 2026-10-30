# fnUI Design - fnOS 用户界面设计工具

## 项目简介

fnUI Design 是一个专门为 fnOS 生态系统设计的现代化用户界面设计工具和开发框架。它提供了一套完整的 UI 组件库、设计规范和开发工具，帮助开发者快速构建美观、高效、一致的 fnOS 应用程序界面。

作为 fnOS 的前端开发解决方案，fnUI Design 不仅是一个简单的 UI 框架，更是一个集成了设计系统、开发工具和最佳实践的完整解决方案。它采用最新的前端技术栈，结合 fnOS 的独特需求，为开发者提供了一流的开发体验。

## 核心特性

### 🎨 现代化设计系统
- **统一的视觉语言**：基于 Nuxt UI 和 Tailwind CSS v4，提供一致的设计风格
- **响应式布局**：自适应桌面端和移动端，大屏侧边栏导航，小屏底部 Tabbar
- **主题系统**：支持浅色/深色主题自动切换，与 fnOS 系统主题完美集成
- **丰富的组件库**：提供 100+ 精心设计的 UI 组件，覆盖常见业务场景

### 🚀 开发效率提升
- **开箱即用**：预配置的开发环境，无需繁琐的搭建过程
- **热重载**：开发过程中实时预览修改效果
- **TypeScript 支持**：完整的类型定义，提升代码质量和开发效率
- **模块化架构**：组件化开发，代码复用性强

### 🔧 fnOS 深度集成
- **配置系统对接**：自动读取和保存 fnOS 的 localStorage 配置
- **主题同步**：与 fnOS 系统主题实时同步，支持跨标签页联动
- **子目录部署**：支持在 fnOS 的子路径下运行，路径自动适配
- **iframe 兼容**：完美支持在 iframe 环境中运行

### 📱 优秀的用户体验
- **流畅的动画**：精心设计的过渡动画和交互反馈
- **无障碍支持**：符合 WCAG 标准，支持键盘导航和屏幕阅读器
- **性能优化**：按需加载、代码分割，确保快速响应
- **国际化就绪**：内置多语言支持框架

## 技术栈

### 核心技术
- **Vue 3**：渐进式 JavaScript 框架，组合式 API
- **Vite**：下一代前端构建工具，极速开发体验
- **TypeScript**：JavaScript 的超集，类型安全
- **Vue Router**：官方路由管理器，支持文件系统路由

### UI 框架
- **Nuxt UI v4**：基于 Vue 3 的 UI 组件库
- **Tailwind CSS v4**：实用优先的 CSS 框架
- **Lucide Icons**：精美的开源图标库

### 开发工具
- **ESLint**：代码质量检查
- **vue-tsc**：Vue TypeScript 类型检查
- **PostCSS**：CSS 后处理器

## 项目结构

```
fnDesign/
├── src/
│   ├── assets/          # 静态资源
│   │   └── css/         # 全局样式和主题变量
│   ├── components/      # 可复用组件
│   │   ├── Layout.vue   # 主布局容器
│   │   ├── DesktopSidebar.vue  # 桌面端侧边栏
│   │   └── MobileTabbar.vue    # 移动端底部导航
│   ├── composables/     # 组合式函数
│   │   └── useTheme.ts  # 主题管理
│   ├── config/          # 配置文件
│   │   └── nav.js       # 导航菜单配置
│   ├── pages/           # 页面组件
│   │   ├── index.vue    # 首页/仪表盘
│   │   ├── apps/        # 应用管理
│   │   ├── dashboard/   # 数据看板
│   │   ├── profile/     # 个人中心
│   │   └── settings/    # 系统设置
│   ├── App.vue          # 根组件
│   └── main.ts          # 应用入口
├── public/              # 公共静态资源
├── .env                 # 环境变量配置
├── vite.config.ts       # Vite 配置
└── package.json         # 项目依赖
```

## 快速开始

### 安装依赖
```bash
pnpm install
```

### 开发模式
```bash
pnpm dev
```

### 生产构建
```bash
pnpm build
```

### 部署配置

#### 本地开发
```env
# .env
VITE_BASE_PATH=/
```

#### fnOS 部署
```env
# .env
VITE_BASE_PATH=/cgi/ThirdParty/com.yourapp.id/index.cgi/
```

## 设计理念

### 1. 简洁至上
- 最小化视觉噪音，突出核心内容
- 合理的留白和间距，提升可读性
- 清晰的视觉层次，引导用户注意力

### 2. 一致性
- 统一的组件风格和交互模式
- 标准化的颜色、字体、图标系统
- 可预测的用户体验

### 3. 高效开发
- 组件化思维，避免重复造轮子
- 配置驱动，减少硬编码
- 完善的文档和示例

### 4. 可扩展性
- 灵活的插件系统
- 易于定制主题和组件
- 支持第三方库集成

## 页面功能

### 🏠 首页仪表盘
- 数据概览卡片
- 实时性能监控
- 快捷操作入口
- 最近活动记录

### 📱 应用管理
- 应用列表展示
- 搜索和筛选
- 应用详情查看
- 安装/卸载管理

### 📊 数据看板
- 可视化图表
- 数据分析工具
- 自定义报表
- 导出功能

### 👤 个人中心
- 用户信息管理
- 头像上传
- 个人设置
- 活动历史

### ⚙️ 系统设置
- 主题切换（浅色/深色/跟随系统）
- 通知偏好设置
- 安全设置
- 语言选择

## 主题系统

### 自动主题切换
fnUI Design 会自动读取 fnOS 的主题配置，并实时响应主题变化：

- **浅色模式**：明亮的界面，适合日间使用
- **深色模式**：护眼的暗色界面，适合夜间使用
- **跟随系统**：自动跟随操作系统主题设置

### 主题配置格式
```json
{
  "userPreference": {
    "theme": 30  // 10=浅色, 20=深色, 30=跟随系统
  }
}
```

### 自定义主题色
通过修改 CSS 变量轻松定制主题色：

```css
:root {
  --ui-primary: #0066ff;  /* 主题色 */
  --ui-bg: #F2F3F4;       /* 背景色 */
  --ui-bg-card: #F6F6F6;  /* 卡片背景 */
}
```

## 组件库

### 基础组件
- Button（按钮）
- Input（输入框）
- Select（选择器）
- Switch（开关）
- Card（卡片）
- Badge（徽章）
- Avatar（头像）
- Icon（图标）

### 表单组件
- Form（表单）
- FormField（表单字段）
- Textarea（文本域）
- RadioGroup（单选组）
- Checkbox（复选框）
- DatePicker（日期选择器）

### 导航组件
- Tabs（标签页）
- NavigationMenu（导航菜单）
- Breadcrumb（面包屑）
- Pagination（分页）

### 反馈组件
- Toast（轻提示）
- Modal（模态框）
- Alert（警告）
- Progress（进度条）
- Loading（加载）

### 数据展示
- Table（表格）
- List（列表）
- Tree（树形）
- Chart（图表）

## 最佳实践

### 1. 组件命名
- 使用 PascalCase 命名组件文件
- 组件名应该是多个单词，避免与 HTML 元素冲突
- 高复用组件放在 `components/` 目录

### 2. 样式规范
- 优先使用 Tailwind CSS 工具类
- 自定义样式使用 CSS 变量
- 避免内联样式，保持样式可维护

### 3. 状态管理
- 简单状态使用 `ref` 和 `reactive`
- 复杂状态考虑使用 Pinia
- 跨组件状态使用 `provide/inject`

### 4. 性能优化
- 使用 `v-if` 和 `v-show` 合理控制渲染
- 大列表使用虚拟滚动
- 图片懒加载
- 路由懒加载

## 浏览器支持

- Chrome >= 90
- Firefox >= 88
- Safari >= 14
- Edge >= 90

## 贡献指南

我们欢迎所有形式的贡献，包括但不限于：

- 提交 Issue 报告 Bug 或提出新功能建议
- 提交 Pull Request 改进代码
- 完善文档和示例
- 分享使用经验

## 开源协议

MIT License

## 联系方式

- 项目地址：[GitHub Repository]
- 问题反馈：[GitHub Issues]
- 文档站点：[Documentation Site]

---

**fnUI Design** - 让 fnOS 应用开发更简单、更高效、更优雅！
