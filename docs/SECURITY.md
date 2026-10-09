# 安全模型

## 数据本地化

- 全部媒体元数据、检测结果、播放进度存本地 SQLite 与 JSON 缓存，**不上传任何文件**到远程服务器。
- 应用不内置任何网络上报；前端无远程资源加载。

## Tauri 安全配置取舍

`src-tauri/tauri.conf.json` 与 `capabilities/default.json`：

- `app.security.csp = null`：不强制内容安全策略。本地媒体管理器需按任意磁盘路径取缩略图/视频，且应用不加载远程内容，故保持放开。
  **一旦引入远程内容（网页、在线源），必须重新收紧 CSP**。
- `app.security.assetProtocol.scope = ["**"]`：WebView 可读整个磁盘的本地文件（缩略图、视频源、转码缓存都按绝对路径访问）。同理放开。
- 权限声明（`capabilities`）：仅 `core:default`、`opener:default`、`dialog:default`，无文件系统写/网络等额外授权。

## 无凭据入库

- 仓库不含任何密码、密钥、API token。
- `trash`/`winreg`/`notify` 等系统调用均为本地操作，不涉及凭据。
- `.gitignore` 已排除 `*.db`、`*.log`、`*.tmp`、`*.tsv`、`node_modules/`、`dist/`、`src-tauri/target/`，防止本地数据库与构建产物误提交。

## 删除安全

- 删除一律走系统回收站（`trash` crate），不直接抹盘，可恢复。
- 批量删除有二次确认；返回三类清单（`DeletionReport`：deleted / stale / locked），路径已失效的清理库记录但磁盘无事可做，被占用（locked）的需关占用程序重试。

## 进程与外部命令

- ffmpeg / ffprobe 经 `hidden_command` 启动（Windows `CREATE_NO_WINDOW`，不闪黑窗、不暴露交互 shell）。
- `output_timeout` 给所有外部进程设超时（探测 30s，转码更长），防止网络盘/坏文件卡死 worker。
- PotPlayer 检测仅读取固定路径与注册表 `InstallPath`（只读），不写入注册表。

## 并发与库完整性

- 写连接 `Mutex` 串行化；只读查询另开 WAL 连接并发读，不与写抢全局锁。
- 删除/写入走事务，失败回滚保库记录与进度。
- 库损坏自愈：疑似 `malformed`/`corrupt` 错误先无损重建 FTS 再重试（`delete_records_with_heal`），仍失败附 `integrity_check` 结果供定位。
- 启动 fail-fast：应用数据目录、数据库初始化失败直接报错退出，避免带病运行。

## 已知取舍记录

完整的安全与设计取舍见 `docs/superpowers/specs/2026-06-27-viewman-design.md` 的「当前实现范围」「已知取舍」两节，包括并行任务锁中毒处理与检测阈值标定说明。
