# baidu-pan-cli

百度网盘 Rust 客户端的命令行版本 —— 直接驱动 `baidu-netdisk-rust` 核心库，
能力与 HTTP API 等价。

不同于 web 服务器，`baidu-pan-cli` 是**单个二进制**：

- 不需要启动 HTTP 服务
- 不需要前端
- 不需要 WebSocket
- 启动一次即用：登录 → 列举 → 上传/下载/转存 → 任务管理

它与服务器共用同一份配置（`config/app.toml`）和账号数据
（`config/accounts.json`）。两边创建的任务、登录的账号、设置的限速
等完全互通。

## 编译

```bash
cd cli
cargo build --release
# 产物在 ../target/release/baidu-pan-cli
```

## 全局参数

| 参数 | 说明 |
| --- | --- |
| `--config <PATH>` | 配置文件路径（默认 `config/app.toml`） |
| `--account <UID>` | 临时覆盖活跃账号 UID |
| `--json` | stdout 走 JSON；stderr 走 NDJSON 进度事件 |
| `-q, --quiet` | 抑制 stderr 进度；stdout 仅输出最终结果 |
| `-v, --verbose` | tracing 日志级别调到 debug |

## 子命令速查

```text
登录：
  baidu-pan-cli login cookie "BDUSS=xxx; STOKEN=yyy;"
  baidu-pan-cli login qrcode
  baidu-pan-cli login status <SIGN>
  baidu-pan-cli whoami
  baidu-pan-cli logout

账号：
  baidu-pan-cli account list
  baidu-pan-cli account switch <UID>
  baidu-pan-cli account delete <UID>

文件：
  baidu-pan-cli ls /path/to/dir [--page 1] [--page-size 100]
  baidu-pan-cli mkdir /new/dir
  baidu-pan-cli rm /a /b /c                       # 一次多个
  baidu-pan-cli mv /src /dest [--name new.txt]
  baidu-pan-cli cp /src /dest
  baidu-pan-cli rename /path/old.txt new.txt
  baidu-pan-cli search keyword [--num 100]

上传：
  baidu-pan-cli upload ./local.txt /backup/local.txt
  baidu-pan-cli upload-folder ./mydir /backup/mydir
  baidu-pan-cli upload-batch ./a.txt=/x.txt ./b.txt=/y.txt

下载：
  baidu-pan-cli download /backup/file.zip [--to /tmp/file.zip]
  baidu-pan-cli download-folder /backup/dir [--to /tmp/dir]

转存：
  baidu-pan-cli share preview "https://pan.baidu.com/s/1xxx" [--password pwd]
  baidu-pan-cli share transfer "https://pan.baidu.com/s/1xxx" --save /transferred [--password pwd] [--auto-download]

任务：
  baidu-pan-cli task list [--downloads] [--uploads] [--transfers]
  baidu-pan-cli task status <ID>
  baidu-pan-cli task pause <ID>
  baidu-pan-cli task resume <ID>
  baidu-pan-cli task cancel <ID>
  baidu-pan-cli task wait <ID> [--timeout-s 600]

配置：
  baidu-pan-cli config show
  baidu-pan-cli config reload

健康：
  baidu-pan-cli ping
```

## 退出码

| Code | 含义 |
| --- | --- |
| 0 | 成功 |
| 1 | 通用运行时错误（核心库 / JSON 序列化） |
| 2 | 参数 / 用法错误 |
| 3 | 鉴权 / 账号问题（NotLoggedIn / UnknownAccount） |
| 4 | 任务状态异常（不存在 / 超时 / 失败） |
| 5 | IO / 路径不存在 |

## 输出约定

### 默认（人类模式）

- 进度写到 stderr（覆盖同一行），用 `\r\x1b[2K` 清行；
- 最终结果写到 stdout（一行文本 / 表格）；
- 任务完成时打印一行汇总。

### `--json` 模式

- stdout：单个 JSON 对象（结果）；
- stderr：NDJSON 事件流（`{"event":"progress","msg":"..."}` 等），方便 `jq` / 日志收集；
- 适合上层脚本解析（`./baidu-pan-cli --json ls / | jq .items`）。

### `-q` / `--quiet`

- stderr 完全静默；
- stdout 仅打印最终结果；
- 适合自动化流水线。

## 与 web 服务器的关系

`baidu-pan-cli` 与服务器共享：

- **配置**：`config/app.toml`（server.port / download.download_dir / upload.* / 代理等）
- **账号**：`config/accounts.json`
- **任务持久化**：`config/baidu-pcs.db` + `wal/` 目录
- **加密密钥**：`config/encryption.json`（如果启用了客户端加密）

因此两边对账号、任务的状态完全一致：CLI 创建的下载任务在服务器
启动后立即可见；服务器创建的账号 CLI 也能直接登录使用。

### 不能替代的服务器功能

CLI 是**单次进程**：启动 → 执行命令 → 退出。下列"长期运行"的能力
只有服务器能提供：

- 自动备份（需要事件监听 + 持续轮询）
- 分享同步订阅（需要周期性拉取快照）
- WebSocket 实时推送（前端 Web UI 依赖）
- 离线下载监听器

如果需要这些能力的只读视图，CLI 提供 `autobackup list` /
`share-sync list` 之类命令（当前版本为只读，需手工启用服务器）。

## 实现细节

- **进程模型**：每次 CLI 调用都执行完整启动流程（AppState::new →
  load_initial_session）。首次启动有几百毫秒开销（迁移 / 预热 /
  账号加载）；后续通过 SQLite / accounts.json 缓存加速。
- **等待语义**：`upload` / `download` / `share transfer` 默认阻塞
  直到任务到终态；`--no-wait` 入队即返回，任务持久化在 WAL。
- **多账号**：CLI 默认使用 accounts.json 中的活跃账号；用
  `--account <UID>` 临时切换（不修改 accounts.json）。
- **登录之后**：CLI 会自动构建 per-uid manager，与服务器登录链路
  完全一致（详见 `src/commands/login.rs::persist_user`）。
- **日志**：默认走 `logs/` 目录下的滚动文件；`-v` 切到 debug 级别。

## License

Apache-2.0，详见仓库根目录 LICENSE。
