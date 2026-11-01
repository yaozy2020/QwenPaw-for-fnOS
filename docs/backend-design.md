# QwenPaw for fnOS 监督者后端设计(v2.0 提案)

> 状态:**历史文档**。本文是 v2.0 监督者的早期设计提案(含部分早期 Python/FastAPI
> 备选内容),实际实现为 Rust(axum),细节以 `supervisor/src/` 为准;文中"开放
> 问题"多数已随 v2.0.0-12 安全整改关闭。保留本文仅为记录设计动机与三起实机
> 事故的来龙去脉,请勿作为现状参考。

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

## 6. 需要确认的开放问题(2026-10-31 调研后修订)

1. ~~fnOS 网关对 SSE 的缓冲行为未知~~ → **已解决**:dsh 的 SSE(visitors/status/logs/update 四条流)在 fnOS 网关上生产运行,配方实证有效(X-Accel-Buffering: no + 15s 心跳注释行对抗网关 300s read_timeout + 首帧全量快照 + 自适应降频)
2. 监督者监听端口的冲突自愈(对齐现有 19091→18088 模式)
3. b1(2.3.0b1)与 b2 的托管子进程行为差异——b2 验证通过后,b1 亦走同一机制
4. 日志轮转:qwenpaw.log 当前无限增长,监督者启动时顺带处理(保留最近 N MB)
5. 统一网关入口下,桌面图标双入口(本体/控制台)的 manifest 配置方式(desktop_applaunchname 与 gatewayPrefix 的对应关系)

## 7. 实施切分

- **v2.0.0-alpha**:监督者骨架(status/start/stop/install+SSE/收养)+ cgi 瘦桥 + 控制台接新 API;manifest main 指向监督者
- **v2.1**:等待/引导页打磨、psutil 概览、日志轮转
- 技术栈:FastAPI + uvicorn + sse-starlette + psutil,**依赖 wheels 随 fpk 分发、离线安装**(见 D9);监督者 venv 与内核 venv 分离,互不影响


## 8. 调研结论(2026-10-31,三路实证:dsh 源码深读 / 官方文档全量 / 同类开源对比)

> 调研方式:dsh 后端全量源码深读(MIT,单二进制 Go,依赖仅 creack/pty);官方文档 15 页全量;同类项目对比(CasaOS/1Panel/Dockge/HA Supervisor/supervisord/vLLM/Jupyter)。

### 8.1 对既有决策的修订

| 决策 | 调研结论 | 修订 |
|:--|:--|:--|
| D7(SSE) | **风险大幅降级**:dsh 的四条 SSE 流在 fnOS 网关上生产运行;血泪配方齐备 | 采纳 dsh 配方(见 8.2-S2) |
| D12(停止语义) | **官方契约比想象的更窄**:平台 stop 只 TERM PID 文件里的单个 pid(官方 Native 示例:TERM→10s 轮询→KILL→清理),**平台不做任何监督**;setsid/进程组在官方文档属未定义区(既无允许也无禁止) | 监督者收到 TERM 必须自己级联停内核;setsid 已实测可用,保留但标注"未定义区" |
| 入口模型(新增 D10) | 官方三种入口:端口服务(与 NAS 登录态无关)/ CGI(明确不适合后台进程、流式、长请求)/ **统一网关**(复用登录态、支持 WebSocket、注入 X-Trim-* 用户上下文);index.cgi 文档原文:"需要后台进程、流式响应、长时间请求时,请使用端口服务或统一网关" | **采用统一网关**(gatewaySocket + gatewayPrefix):监督者 API 与控制面板走同一条网关入口,index.cgi/api.cgi 退役(dsh 同款:admin unix socket + 网关) |
| 进度推送(新增 D11) | 五项目实证排行:任务落库+轮询兜底(1Panel 主干)> SSE+心跳 > WebSocket(网关兼容风险最高)> 消息总线(过重);HA 的"推送+REST 双轨"最稳;**无编造百分比**是 dsh/npm 的共同纪律 | 任务对象常驻内存 + 日志落盘(监督者重启后可恢复显示)+ SSE 推送 + 轮询兜底,四件套齐上 |
| /proc 收养(新增 D13) | dsh 的实现规范:cmdline 按 NUL 切分,按 argv 特征(basename 集合+子命令+端口)匹配取最大 pid;僵尸判定看 `/proc/<pid>/stat` **最后一个右括号之后**的状态字符(不能只看目录存在);重发现结果缓存 2s;双 PID 文件(监督者自身/被管进程)分离且被管 pid 随重发现刷新 | 照抄该规范 |

### 8.2 必须原样继承的 dsh 血泪清单(每条都有线上事故背书)

- S1 SSE 心跳:15s 发 `: ping` 注释行——fnOS 网关 300s read_timeout 掐空闲连接,曾致"实际在下载却一直 0%"
- S2 前端 EventSource:onerror **绝不永久 close**(曾致重连后再也连不上),指数退避 1s→15s,重连靠后端首帧快照追平
- S3 **绝不按进程名杀**:pkill -x 杀掉了持锁的 node,留下 120s 陈旧锁——只按 PID/进程组杀
- S4 取消只发信号,终态唯一写入者是安装协程(否则取消与收尾竞态会误写状态);取消/失败删光半成品与缓存
- S5 僵尸判定看 stat 状态字段,不看 /proc 目录存在
- S6 推送节流 300ms + 关键节点强推;Seq 作为前端"结果只提示一次"的去重键
- S7 配置原子写:tmp → fsync → rename,0600
- S8 不编造百分比:只报"已获取 N 项"+ 尾行消息

### 8.3 技术栈与先例确认

- **Python 常驻监督者有直系先例**:HA Supervisor(asyncio+aiohttp,生产级最重的 Python 管理后端)、vLLM(FastAPI 常驻服务管 Python 子进程)、supervisord(进程语义黄金标准)
- **无四位一体现成 OSS**(FastAPI+pip 生命周期+监督+进度推送),为组合先例:HA(形态/进度模型)+ supervisord(停止语义)+ 1Panel(任务落库与轮询)+ vLLM(可行性背书)
- **排除项(均有证据)**:复用 supervisord 进程(XML-RPC+stdin 协议双桥接,成本>自研几个函数)、Docker 化内核(改变分发语义)、systemd 单元(fnOS 无权限模型)、Celery/RQ(单机单对象过重)
- **依赖极简实证**:dsh 全标准库零框架;五项目无 DB/无消息队列——NAS 规模"文件+内存"即全部状态;FastAPI/uvicorn 保留(用户偏好成熟架构),但依赖清单克制

### 8.4 官方文档关键原文备份

- framework:"如果应用正在运行,飞牛 fnOS 可能会在升级前停止应用,并在升级完成后重新启动";"脚本应尽量具备可重复执行能力"
- index-cgi:"当应用需要后台进程、WebSocket、流式响应、长时间请求、高流量 API 或复杂 API 时,请使用端口服务或统一网关"
- app-entry 三模型:端口服务("和 NAS 用户登录态无关")/ CGI / 统一网关("复用系统访问域名、支持 WebSocket、获取网关提供的用户上下文")
- gateway-registration:gatewaySocket + gatewayPrefix 决定统一网关入口,"protocol 和 port 会被忽略";注入 X-Trim-Userid/Isadmin/Username
- stop 官方示例:PID 文件单 pid → TERM → 10s 轮询 → KILL → 清理;平台不做监督
