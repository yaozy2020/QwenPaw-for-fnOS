# QwenPaw for fnOS 监督者后端设计(v2.0 提案)

> 状态:设计评审稿。动机与三起实机事故见下文;实现前需确认开放问题一节。

## 1. 动机(三起已发生的事故)

1. **内核安装被 App 切后台中断**:安装(pip)运行在瞬态 CGI worker 的进程组里,App 切后台时网关回收 worker 进程组,nohup 只挡 SIGHUP,挡不住整组清理(v1.0.11 已用 setsid 止血,但安装仍活在 CGI 会话里)
2. **b2 内核引入长驻子进程**(依赖托管工作进程),单进程停止必然遗留孤儿;孤儿持有的依赖环境锁正是 b2 启动崩溃的祸根
3. **进度体验**:安装进度靠刮日志文件,跨请求状态不可靠

根因共性:外壳是"瞬态脚本 + 后台进程",没有**常驻监督者**持有内核及其子进程。

## 2. 架构

```
浏览器 / 飞牛App
   │ (fnOS 网关,登录态校验)
   ├── /cgi/.../index.cgi          ← 控制面板(静态,恒可达)
   ├── /cgi/.../api.cgi            ← 瘦桥:鉴权(网关)+ token 头 + 转发/流式
   │        │ (仅本机回环 HTTP + token)
   │        ▼
   │   监督者 supervisor(FastAPI,常驻)  127.0.0.1:19090
   │        ├─ 内核生命周期:启动/停止/安装/版本管理(子进程,独立进程组)
   │        ├─ /proc 收养:重启后重新发现存活内核
   │        ├─ 安装进度:解析 pip 输出 → SSE 推送
   │        └─ psutil:CPU/内存/运行时长
   ▼
内核本体 Web :18088(0.0.0.0,直连,不反代、不改端口)
```

- **监督者不反代内核**:qwenpaw 本体自带完整鉴权与 UI,dsh 反代的三个意图(token 交换/鉴权层/兼容性注入)均不存在;等待页由控制台 cgi 承担
- **监督者只绑回环**:不对局域网暴露,安全面收敛;控制台 JS 无法直连 NAS 回环,故 cgi 桥接是必须的
- **内核 18088 直连不动**:停止/未安装时该端口拒绝连接,由控制台引导(已实现)

## 3. 关键决策(含再过一遍时的修正)

| # | 决策 | 说明/修正 |
|:--|:--|:--|
| D1 | **不用 Pdeathsig** | 初稿误列。内核必须**在监督者重启(外壳升级)后存活**,否则每次外壳升级内核都要重启;改为监督者启动时 `/proc` 收养(发现 `python3 -m qwenpaw app --port <port>` 进程则重新接管,对齐 dsh `findDshPid`) |
| D2 | **监督者持有内核为直接子进程** | `Popen(start_new_session=True)`:内核自成进程组,组信号可连带 b2 依赖托管子进程;不依赖 bash 包装,v1.0.9 的 pid 错位问题在架构上消失 |
| D3 | **停止 = 三段式在监督者内实现** | 组 TERM(宽限)→ 单 KILL → 组 KILL 清扫;继承 v1.0.10 沙箱已验证的语义 |
| D4 | **App 停止语义** | 飞牛「停止应用」→ main stop → 停监督者 → 监督者停内核:整栈一致下线(用户意图就是全停) |
| D5 | **外壳升级语义** | fnOS 停应用 → 全栈下线 → 装新外壳 → 监督者启动 → **自动拉起已安装内核**(D1 收养或冷启动) |
| D6 | **回环鉴权用共享 token,不用 SO_PEERCRED** | TCP 回环无对端凭证;token 落 `<VAR_DIR>/supervisor-token`(0600,qwenpaw 属主),cgi 与监督者同读;同机其他用户不可读 |
| D7 | **SSE 与轮询双轨** | 控制台优先 SSE;网关若缓冲 SSE 则自动回退轮询(现有轮询逻辑保留为兜底) |
| D8 | **首次安装不预装内核(继承 v1.0.7)** | 监督者启动时内核缺失 → 状态 not_installed,控制台引导安装(已实现的行为平移)。注意:不预装的是**内核**,监督者自身依赖必须首装即就位(否则控制台都起不来) |
| D9 | **监督者依赖:wheel 随 fpk 分发,venv 放 TRIM_PKGVAR** | venv 三大风险与对策:①位置——@apphome/@appcenter 会随升级被清(内核 venv 三次实证),监督者 venv 放 `<VAR_DIR>/supervisor-venv`(@appdata,已实证穿越升级);②网络依赖——D 路径教训,安装期不得拉网:构建时 `pip download` 固定版本到 fpk 内 `supervisor-wheels/`,安装时 `--no-index --find-links` 离线秒装,依赖版本随外壳钉死(双轨制延伸);③python312 漂移——install_callback 检测 venv 完整性,缺损时从随包 wheels 离线重建(秒级)。fpk 体积 150K → ~5MB |

## 4. 监督者 API(草案)

| 方法 | 路径 | 说明 |
|:--|:--|:--|
| GET  | /status | running/pid/version/installed/uptime/cpu/mem(psutil) |
| POST | /start /stop /restart | 内核生命周期 |
| POST | /kernel/install | body: {version \| "stable" \| "latest" \| "beta"};SSE 进度流 |
| GET  | /kernel/versions | PyPI 可用版本 + dist-tags(带缓存) |
| POST | /kernel/pin | 写版本钉 |
| GET  | /events | SSE:状态变更 + 安装进度 |
| GET  | /logs?tail=n | 运行日志 |

- 鉴权:请求头 `X-Supervisor-Token`(cgi 从 token 文件读取后附带)
- 监督者自身由 `cmd/main start/stop/status` 管理(manifest 的 service 入口改指监督者)

## 5. 生命周期矩阵

| 事件 | 外壳 | 监督者 | 内核 |
|:--|:--|:--|:--|
| 首次安装 | 落包,起监督者 | 启动,状态 not_installed | 未安装(控制台引导) |
| 控制台安装内核 | 不变 | 运行 pip(子进程,SSE 进度) | 就绪后自动启动 |
| 外壳升级 | 换包 | 停止(连带内核)→ 新监督者启动 | 自动拉起(冷启动) |
| 仅内核升级 | 不变 | 运行 pip,重启内核子进程 | 新版本热替换 |
| 停止应用 | 不变 | 停止(停内核) | 下线 |
| 监督者崩溃 | 不变 | — | 存活,监督者重启后收养 |

## 6. 需要确认的开放问题

1. fnOS 网关对 SSE 的缓冲行为未知——实现时以实测为准,轮询兜底常驻
2. 监督者监听端口的冲突自愈(对齐现有 19091→18088 模式)
3. b1(2.3.0b1)与 b2 的托管子进程行为差异——b2 验证通过后,b1 亦走同一机制
4. 日志轮转:qwenpaw.log 当前无限增长,监督者启动时顺带处理(保留最近 N MB)

## 7. 实施切分

- **v2.0.0-alpha**:监督者骨架(status/start/stop/install+SSE/收养)+ cgi 瘦桥 + 控制台接新 API;manifest main 指向监督者
- **v2.1**:等待/引导页打磨、psutil 概览、日志轮转
- 技术栈:FastAPI + uvicorn + sse-starlette + psutil,**依赖 wheels 随 fpk 分发、离线安装**(见 D9);监督者 venv 与内核 venv 分离,互不影响
