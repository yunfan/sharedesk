# RustDesk Share

一个匿名、无账号、基于房间的 P2P 桌面共享站点。

目录已经拆开：

- `server/`：独立 Rust 服务端
- `turn/`：独立 Rust TURN/STUN 服务
- `client/`：独立静态前端
- `deploy/`：compose 示例

这不是 Galene 的完整 Rust 重写，而是针对“匿名共享桌面”场景裁剪过的 MVP。

## 功能

- 匿名房间，不需要注册或登录
- 单 host，多 viewer
- host 使用浏览器 `getDisplayMedia` 共享桌面
- viewer 通过房间链接直接观看
- 房间加入密码
- host 可中途修改加入密码，不影响已加入 viewer
- host 可踢人
- host 可按客户端 token 拉黑
- host 可按网络指纹拉黑
- 网络指纹来源包括服务端看到的接入 IP 与浏览器通过 WebRTC candidate 提取的地址信息
- 参与者可更新自己的显示名
- WebSocket 信令
- 支持 STUN
- 支持静态 TURN 凭据
- 支持 Rust TURN/STUN 服务
- 支持 TURN REST 风格临时凭据
- 默认内置若干匿名免费公共 ICE 候选，并追加环境变量里显式配置的地址

## 路由

- `/` 主页
- `/room/:room?role=host|viewer` 房间页
- `/backend/ws/:room/:role` WebSocket 信令
- `/backend/api/ice?room=<id>&role=<host|viewer>` ICE 配置
- `/backend/api/rooms/:room` 房间状态

前后端同域时，建议把后端统一挂在 `/backend` 下，页面仍然走站点根路径。
后端基址可以通过 `ROOM_WEB_BACKENDBASE` 配置，默认就是 `/backend`。

## 客户端构建

```bash
cd client
make dist
```

输出目录：

```text
client/dist
```

## TURN 构建

```bash
cd turn
cargo run
```

Linux musl 静态构建：

```bash
cd turn
./scripts/build-musl.sh
```

输出目录：

```text
turn/dist
```

## 服务端运行

```bash
cd server
cargo run
```

这个服务端只负责 `/backend/*`，不再托管前端静态文件。
前端静态文件应由 Caddy 直接从 `client/dist` 提供。

## 服务端配置

所有环境变量都以 `ROOM_` 开头，并统一采用：

```text
ROOM_领域_配置项
```

其中领域和配置项都只使用一个单词。

当前支持：

- `ROOM_SERVER_LISTEN`
- `ROOM_SERVER_ORIGINANY`
- `ROOM_SERVER_MAXONLINE`
- `ROOM_WEB_BACKENDBASE`
- `ROOM_ICE_STUNURLS`
- `ROOM_TURN_MODE`
- `ROOM_TURN_URLS`
- `ROOM_TURN_USERNAME`
- `ROOM_TURN_PASSWORD`
- `ROOM_TURN_SECRET`
- `ROOM_TURN_TTLSECONDS`
- `ROOM_TURN_LISTEN`
- `ROOM_TURN_EXTERNAL`
- `ROOM_TURN_REALM`
- `ROOM_TURN_PORTRANGE`
- `ROOM_LOG_FILTER`

其中 TURN 服务自身主要读取：

- `ROOM_TURN_LISTEN`
- `ROOM_TURN_EXTERNAL`
- `ROOM_TURN_REALM`
- `ROOM_TURN_SECRET`
- `ROOM_TURN_USERNAME`
- `ROOM_TURN_PASSWORD`
- `ROOM_TURN_PORTRANGE`

默认值集中定义在：

- [`server/src/config.rs`](</home/jyf/work/codetool/repo/rust-anon-desktop-share/rustdesk-share/server/src/config.rs>)

默认 ICE 策略：

- 默认内置多个公开匿名 STUN 候选
- 当 `ROOM_TURN_MODE=disabled` 时，会额外尝试内置匿名公共 TURN 候选
- 如果你通过 `ROOM_ICE_STUNURLS` 或 `ROOM_TURN_URLS` 显式配置地址，这些地址会追加进默认集合

注意：

- 这些公共免费节点仅适合“尽量一试”的匿名默认项，不保证中国大陆长期可用
- 代码默认包含的匿名公共 TURN 仅用于兜底测试，不建议作为生产依赖

## 当前控制模型

- 房间只允许一个 `host`
- `viewer` 加入时需要提供正确房间密码
- host 修改密码后，仅影响之后的新加入者
- 已在房间内的 viewer 不会被重新校验或踢出
- host 可以对 viewer 执行：
  - 仅踢出
  - 踢出并按 `client_token` 拉黑
  - 踢出并按网络指纹拉黑

网络拉黑目前支持两类指纹：

- `wsip:<remote-ip>`：服务端 websocket 连接来源 IP
- `cand:<ip>:<port>`：浏览器从 WebRTC candidate 中提取并上报的地址信息

注意：浏览器可见的 candidate 内容受浏览器、网络环境、mDNS、TURN 中继等影响，不保证总能得到稳定公网地址，所以网络拉黑应视为“尽量增强”而不是绝对封禁。

## Docker Compose

仓库里带了一个 `deploy/docker-compose.yml`：

```bash
cd rustdesk-share/client
make dist
cd ../deploy
docker compose up --build
```

它会启动：

- Rust 信令站点
- Rust TURN/STUN 服务

部署前请至少修改：

- `ROOM_TURN_SECRET`
- `ROOM_TURN_EXTERNAL`
- 反向代理和 TLS

## Caddy

仓库根目录提供了 [`Caddyfile`](</home/jyf/work/codetool/repo/rust-anon-desktop-share/rustdesk-share/Caddyfile>)，默认把：

- `/`、`/room/*` 直接从 `client/dist` 提供
- `/backend/*` 代理给后端 API 和 WebSocket

把 `share.example.com` 改成你的真实域名即可。

## 架构限制

- 当前房间只允许一个 host
- 当前是 host 到每个 viewer 的直连分发，不是 SFU
- 适合轻量共享桌面，不适合大规模观看
- 浏览器层面不能强制指定某个进程共享，只能通过窗口共享参数优先引导用户选择某个应用窗口
