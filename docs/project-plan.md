# PulseBridge / 留下心跳项目计划书（当前资产版）

> 修订日期：2026-09-03
> 本文以当前仓库中的代码、文档、测试和已记录的硬件实验为准。状态分为：**已完成**、**部分完成**、**未开始**、**待实机验收**。

## 一、项目目标

PulseBridge 的目标是构建低功耗、低延迟、自托管的实时身体数据基础设施；“留下心跳”是建立在该基础设施上的用户与公开展示产品。

当前 v1 只承诺心率，不把 HRV、Stress、SpO2、Body Battery、GPS、历史数据库、OAuth 或 Federation 当作已经存在的功能。

当前实时链路以 WebSocket 为唯一订阅主线：

```text
Garmin Forerunner 255
    ↓ Multi-Link REAL_TIME_HR 或标准 HRS Broadcast
Android Bridge
    ↓ UDP + ChaCha20-Poly1305
Rust Server
    ↓ 内存状态 + Metric Bus
    ├─ Web Dashboard
    ├─ REST Snapshot
    ├─ WebSocket /ws
    ├─ VRChat OSC Bridge
    └─ NapCat/QQ 状态 Bridge
```

在现有实时底座之上，可以提前建设一个纯 UI 分发入口：**留下心跳 Embed Kit / Share Kit**。它不改变采集、UDP、Server 或 WebSocket 协议，只把已有的实时状态包装成网页、直播覆盖层和动态图片。

## 二、当前资产盘点

| 资产 | 当前状态 | 事实边界 |
|---|---|---|
| `protocol/protocol.md` | 已完成基础 v1 | UDP 单包、AEAD、时间偏差、序列与 64 位重放窗口、地址重绑定规则已写明；独立 v1 测试向量已补齐；payload 仍是心率专用 4 字节结构 |
| `shared/pulsebridge-api/` | 已完成当前订阅契约 | Rust 类型包含 `Presence`、`Metric::HeartRate`、`MetricEvent`、`DeviceSnapshot`、`ServerMessage`；目前只有心率 Metric |
| `server/` | 已完成本地实时 MVP | Rust/Tokio/Axum；UDP 解包、ChaCha20-Poly1305、内存 Store、latest-state、Presence 衰减、REST、WebSocket、静态 Dashboard、Simulator 均存在 |
| `android/` | 实现已落地，待实机长时验收 | Kotlin Android App、Multi-Link、标准 HRS Broadcast、Foreground Service、自动重连、发送即变化、10 秒 heartbeat、屏幕关闭 watchdog、WakeLock 均存在 |
| `tools/mltest/` | 已完成探测工具 | 用于 FR255 Multi-Link 服务探测、注册和原始帧观察，不是最终用户 App |
| `vrchat-bridge/` | v1 已完成 | 独立进程，消费 `/ws`，向本机 VRChat OSC `/chatbox/input` 发送文本；支持设备选择、刷新、限速和 BPM 补空/补零格式 |
| `napcat-bridge/` | v1 已完成 | 消费 `/ws`，更新 NapCat 自定义在线状态；支持心率区间、格式、限频和 Access Token 配置 |
| `server/web/` | 已完成基础展示 | 当前是设备 Dashboard，不是“留下心跳”公开个人主页 |
| `docs/phase0-multilink.md` | 已完成实验证据 | 记录 2026-09-02 在 FR255 + OPPO PKB110 + ColorOS 上与 Garmin Connect 共存的 Multi-Link 结果 |
| `docs/multilink-services.md` | 已完成探索记录 | 已观察到 HRV、Stress、SpO2、Body Battery、Respiration、Steps 等服务，但尚未接入正式 wire payload 和共享 Metric 枚举 |
| `docs/battery-test.md` | 测试方案已完成，数据未完成 | 24 小时屏幕关闭存活、手表耗电、手机耗电和 Broadcast 对照仍需实测 |

## 三、已达到的里程碑

### M0：Garmin Multi-Link 可行性验证 — 已完成，结论 GREEN

已在 FR255 上证明：第二个 GATT 客户端可以与 Garmin Connect 共存，未观察到掉线或重新连接；`REAL_TIME_HR` 注册成功，并以约 1 Hz 输出心率。

已确认的实现约束：

- 不能把句柄写死，必须使用手表注册返回的句柄。
- 当前证据只覆盖同一部手机上的多个 App，不代表第二部手机也能同时连接。
- 关闭句柄的消息格式仍未确认，当前客户端采用断开 GATT 的安全退路。
- lane 0 在重启或 Garmin 同步后的长期可用性仍待验证。

### M1：Simulator → Rust → WebSocket → Dashboard — 已完成

当前可运行链路为：

```text
server simulator
    ↓ UDP
server/src/udp.rs
    ↓ 解密、重放检查、时间检查
server/src/state.rs
    ↓ latest state + broadcast Metric Bus
server/src/http.rs
    ├─ GET /api/devices
    ├─ GET /api/device/:id
    └─ GET /ws
```

设备在 15 秒内显示 `online`，60 秒内显示 `stale`，之后 `offline`；非 online 状态不继续暴露旧的实时心率。

### M2：Android Bridge 代码闭环 — 部分完成，待实机验收

Android 已具备：

- Multi-Link 默认源和标准 Heart Rate Service Broadcast 备用源。
- Foreground Service、部分 CPU WakeLock、自动重连。
- 30 秒无心率通知的 silent-stream watchdog。
- 心率变化立即上报、无变化 10 秒 heartbeat。
- 手机电量、watch 连接状态、sensor contact、resting HR 的 v1 上报字段。

代码级测试和构建已经通过，但以下仍不能视为完成：ColorOS 屏幕关闭 24 小时存活、全日 Multi-Link 手表耗电、手机耗电、真实公网链路和实际 APK 使用体验。

### M3：本地输出适配器 — v1 已完成

VRChat 和 NapCat 均已从 Server 中独立出来，只共享 `shared/pulsebridge-api` 的数据契约。

VRChat 当前是 Chatbox 文本显示，不是 Avatar 参数或 Avatar HUD。Chatbox 是单一文本槽位，自动刷新可能覆盖用户手动聊天；因此它是 v1 兼容方案，不应描述为独立且无冲突的长期显示方案。

### M4：Embed Kit / Share Kit — E1/E2 已完成，E3–E7 未开始

这不是新的数据链路，而是现有 WebSocket 和静态 Web 的 UI 入口。当前已经具备：

- `server/web/` 静态资源托管。
- `/ws` 连接时 Snapshot、之后定时 Snapshot、心率变化 Metric 的实时契约。
- `online` / `stale` / `offline` 和 `age_ms`，可以直接驱动 UI 状态。
- latest-state 语义，适合 iframe、OBS Browser Source 和 Web Component。
- REST 设备快照，可作为动态 SVG/PNG/WebP 的服务端数据源。

因此 Embed Kit 可以在 P1 实机链路验收的同时开工。第一版可以先按 `device_id` 或服务器配置的临时公开标识渲染，不等待完整 User/Profile/Enrollment；对公网开放前仍必须接入后续的 Profile、Visibility 和授权边界。

## 四、开发顺序

### P0：协议和实现基线 — 已完成 / 部分完成

保留并维护：

- UDP telemetry v1 和 `protocol/protocol.md` 测试向量。
- `shared/pulsebridge-api` 的 WebSocket 消费契约。
- Rust Server 的 replay、clock skew、presence、latest-state 语义。
- Android PacketCodec 与 Rust codec 的字节兼容。

已补齐：测试向量位于 `protocol/test-vectors/telemetry-v1.json`，共享 WebSocket JSON 的版本兼容规则位于 `protocol/websocket-v1.md`；Rust 与 Android 均对同一完整报文进行校验。

### P1：真实设备链路闭环 — 最高优先级

目标：

```text
FR255
  ↓ Multi-Link
Android APK
  ↓ UDP AEAD
公网或局域网 Rust Server
  ↓ WebSocket /ws
Dashboard / VRChat / NapCat
```

验收条件：

1. 真实 FR255 连续产生心率，Android 状态和 Server Dashboard 同步。
2. 手机锁屏至少运行 24 小时，记录 uptime、samples、reconnects、手机耗电和断流情况。
3. Multi-Link 与 Broadcast 分别完成手表耗电对照。
4. 断网、重连、Wi-Fi/5G 切换后，只恢复最新状态，不伪造历史回放。
5. 记录 Server 接收时间、Android 发送时间和来源时间，形成实际延迟基线。

### P2：Embed Kit / Share Kit — E1/E2 已完成，可继续与 P1 并行

#### P2.1 目标与分层

对外展示分成两条路线：

```text
Realtime
  WebSocket → HTML / iframe / OBS / Web Component

Snapshot
  HTTP → SVG / PNG / WebP / OpenGraph
```

Realtime 可以接近当前心率更新频率；Snapshot 只代表请求时的状态，刷新速度受 GitHub、论坛、图片代理和缓存策略影响，不能宣称为 1 Hz 实时。

#### P2.2 第一批 UI 入口

固定布局建议：

| 入口 | 建议地址 | 用途 |
|---|---|---|
| Minimal | `/embed/{target}/minimal` | `♥ 143`，Footer、状态栏 |
| Compact | `/embed/{target}/compact` | 头像/名称/心率，侧边栏和小组件 |
| Card | `/embed/{target}/card` | 头像、名称、简介、心率、在线状态 |
| Live | `/embed/{target}/live` | 透明背景、OBS Browser Source、直播覆盖层 |
| Activity | `/embed/{target}/activity` | 未来接入 Running、配速、距离后使用 |

其中 `{target}` 在身份系统完成前可以是 `device_id` 或受控映射；完成 Profile 后再稳定为 `/u/{handle}` 和 `/embed/{handle}/...`。

#### P2.3 Live Overlay 要求

- 支持透明背景和固定画布尺寸，例如 160×60、320×80、600×160。
- 内部直接连接当前 `/ws`，接收 Snapshot 和 Metric；断线后指数退避重连，重新连接后先使用最新 Snapshot。
- 显示 `LIVE`、`STALE`、`OFFLINE`，优先使用 Server 的 `presence` 与 `age_ms`，不自行把旧心率伪装成实时值。
- 可按当前 BPM 估算心跳动画周期 `60 / HR`；明确标注这不是 RR Interval 的真实心搏同步。
- URL 参数支持 `theme`、`transparent`、`show_avatar`、`show_name`、`show_status`、`show_zone`、`animate` 等展示选项。

第一版不要求服务端接受订阅指令，Embed 页面可连接 `/ws` 后按 target 过滤 Snapshot/Metric。未来有多用户授权需求时，再设计带身份和 scope 的开发者 WebSocket；不把尚未实现的 `/api/v1/ws` 当成当前接口。

#### P2.4 Snapshot 分发

提供动态图片入口：

```text
/embed/{target}/badge.svg
/embed/{target}/minimal.svg
/embed/{target}/card.png
/embed/{target}/card.webp
```

可包含头像、名称、状态文案、心率和 `LIVE/STALE/OFFLINE`。GitHub Badge、论坛签名和 Markdown 属于 Snapshot；OpenGraph Card 也属于 Snapshot，不能保证页面停留时自动变化。

公开动态图片必须遵守 Visibility：在当前尚无身份和隐私控制时，只用于本地/受控测试，不直接开放任意设备的公网心率。

#### P2.5 Web Component 与分享入口

后续提供：

```html
<script type="module" src="/embed/pulse-heartbeat.js"></script>
<pulse-heartbeat target="1" layout="compact" theme="auto" show-status></pulse-heartbeat>
```

组件内部负责 WebSocket、重连、Snapshot、Metric、离线状态和渲染。Web 分享入口最终应集中提供：个人主页、OBS URL、iframe、Web Component、GitHub/Markdown、SVG/PNG/WebP 和 WebSocket 文档的复制按钮，让普通用户按“放到 OBS / GitHub / 博客”选择，而不必理解协议。

#### P2.6 实现拆分与验收

按以下顺序实现：

```text
E1  minimal / compact / card Web Embed
E2  Live Overlay：透明背景、OBS、重连、离线状态、心跳动画
E3  Dynamic SVG：badge.svg、minimal.svg
E4  Dynamic PNG/WebP：card.png、card.webp
E5  OpenGraph 分享卡片
E6  Web Component：pulse-heartbeat.js
E7  oEmbed：粘贴个人页 URL 自动获得 iframe
```

P2 的最小验收线 E1 + E2 已通过：浏览器/OBS 能通过 `/ws` 显示当前心率，断线可恢复，旧值会进入 stale/offline，透明背景和基本参数可用。E3–E7 可按平台需求逐步增加。

### P3：正式设备身份与 Enrollment — 未开始

按以下顺序实现：

```text
首次部署 → Bootstrap Token → Admin + Passkey
用户/设备权限 → QR Enrollment → DeviceId + 独立 PSK
Android Keystore 保存 → Server 撤销/禁用/重新配对
```

验收重点是：QR 单次使用、10 分钟过期、撤销后立即拒绝、错误密钥不能影响其他设备，以及任何日志不包含 PSK 或 Recovery Code。

### P4：Control Plane — 未开始

先做最小可用范围：

- 用户、设备、Owner、启用/禁用。
- `ADMIN` / `USER` 两种角色。
- Passkey 管理和必要的审计事件。
- 持久化数据库只承载身份、设备、授权和配置，不把 1 Hz 实时流直接写入数据库。

实时数据仍走 WebSocket；数据库不是实时总线。

### P5：留下心跳 Profile 和公开页面 — 未开始

在身份基础上增加：

```text
avatar、handle、display_name、bio、status_line、heart_rate_zones
```

建议先提供设备无关的内部 UserId，再由 handle 映射到公开页面，例如 `/u/sighjune`。公开 API 仍以 REST 快照 + WebSocket 实时流为准。

### P6：Visibility / Privacy — 未开始

第一版仅实现 `PUBLIC` / `PRIVATE`，每项 Metric 独立控制。Location 单独建权限，并默认 `PRIVATE`；不要因为心率公开就把位置或完整健康数据默认公开。

### P7：OAuth 第三方授权 — 未开始

在 Profile 和 Visibility 稳定后，再实现 Authorization Code + PKCE、Scopes、Access/Refresh Token、Connected Apps 和撤销。第三方实时读取使用经授权的 WebSocket 订阅，除非有明确理由，不引入第二套 SSE 订阅协议。

### P8：输出适配器扩展 — 部分完成

已完成：

- VRChat v1：WebSocket → 本机 OSC Chatbox。
- NapCat/QQ v1：WebSocket → 自定义在线状态。

待完成：

- VRChat Avatar Parameters / 独立 HUD，解决 Chatbox 与手动聊天共用文本槽位的问题。
- OBS、Home Assistant、Minecraft 等适配器，均应保持独立进程或独立项目，只依赖共享契约。

### P9：更多 Metrics — 未开始产品接入

优先级建议：`HRV` → `Stress` / `Body Battery` / `Respiration` → 运动相关指标。每项必须记录来源服务、帧格式、更新频率、功耗影响和默认隐私级别；严禁为了“实时”主动打开用户关闭的高耗电传感器，尤其是 SpO2 和高频加速度计。

### P10：功耗、历史和 Federation — 后置

- 功耗优化：先有真实基线，再比较 LIVE、ADAPTIVE、变化触发、批量等策略。
- 历史数据：v1 不保存完整长期 1 Hz 历史；后续单独设计 History Extension 和降采样存储。
- Federation：不属于 v1；后续再考虑 `/.well-known/pulsebridge`、WebFinger、签名委托和 Server-to-server federation。

## 五、v1 完成定义

PulseBridge v1 的完成线是下面的最小闭环：

```text
部署 Server
  ↓
配置 Android 设备密钥（当前 v1 仍可手工配置）
  ↓
FR255 REAL_TIME_HR
  ↓
Android Foreground Service
  ↓ UDP AEAD
Rust Server
  ↓
WebSocket /ws
  ├─ Dashboard 显示当前心率和在线状态
  ├─ VRChat v1 可显示心率
  └─ NapCat v1 可更新状态
```

并且必须通过 P1 的真实设备、锁屏存活、断线恢复和耗电验收。Bootstrap/Passkey/QR、用户公开主页、Privacy、OAuth、Avatar HUD、历史和 Federation 不属于当前 v1 的完成前置条件，但应保留在后续路线中。

Embed Kit 是可以提前交付的 UI 入口，不改变上述核心 v1 的设备验收线。建议将 `E1 + E2` 作为紧接 Server MVP 之后的首个产品化交付；当 Profile 和 Visibility 完成后，再把受控的 `target` 映射为公开 handle，并开放公网分享。

## 六、当前下一步

1. 在真实 Android/FR255 上执行 `docs/battery-test.md` 的 Test C，再执行 Test A/B。
2. 用真实设备跑通 Android → UDP → Server → WebSocket → Dashboard/Embed，并记录延迟和断线恢复。
3. 根据实测结果修正 ColorOS 后台存活，并持续维护协议向量与 WebSocket v1 契约。
4. 实机链路和 UI 入口稳定后，再开始 P3 的设备 Enrollment；不要提前开发 OAuth、Federation 或完整历史系统。

## 七、当前验证记录

2026-09-03 在本仓库执行：

```text
cargo test --manifest-path shared/pulsebridge-api/Cargo.toml       → 1 passed
cargo test --manifest-path server/Cargo.toml --all-targets         → 8 passed
cargo test --manifest-path vrchat-bridge/Cargo.toml                → 8 passed
cargo test --manifest-path napcat-bridge/Cargo.toml                → 0 tests, build passed
android/gradlew.bat test                                           → BUILD SUCCESSFUL
protocol/test-vectors/telemetry-v1.json                            → Rust/Android codec vector checks passed
Embed E1/E2                                                         → local HTTP, WebSocket and browser stale-state checks passed
```

以上是代码级证据，不等于物理设备已完成 24 小时运行或功耗验收。硬件证据以 `docs/phase0-multilink.md` 和后续新增的实测记录为准。
