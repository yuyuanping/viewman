# QQ Bot Saver

通过 NapCatQQ 监听 QQ 消息，自动保存文本、图片、视频和文件到本地磁盘。

由 viewman（Tauri 桌面应用）的「QQ」标签页统一管理状态与启停，也可独立运行。

---

## 快速开始

```bat
:: 一键启动 NapCat + bot + 风控看门狗（需管理员权限）
D:\opencword\viewman\qqbot\start.bat
```

或在 viewman 里：左侧「QQ」标签 → 点「启动 Bot」。

**需要扫码时**用可见窗口的入口，才能看到二维码：

```bat
D:\tool\NapCat.Shell.Windows.OneKey\NapCat.44498.Shell\napcat.bat
```

两个必须知道的前提：

- **每次冷启动都要重新扫码。** NapCat 注入 QQ 进程，QQ 每次启动都要重新授权。
  二维码约 2 分钟过期，过期后重跑 `napcat.bat` 会自动换一张。
- **管理员权限是必需的。** NapCat 与 bot.py 都是提权进程，非提权 shell 杀不掉
  （`拒绝访问`），会导致重启失败。右键「以管理员身份运行」。

开机自启（计划任务 `QQBotSaver`）已**禁用**，每次使用手动启动。

---

## 文件结构

### 程序目录 `D:\opencword\viewman\qqbot\`

```
bot.py              # 主逻辑：WS 连接、API 调用、事件队列流水线、main
config.py           # 配置加载（config.json + 环境变量覆盖）
db.py               # SQLite 持久层：事件队列、消息/媒体索引、FTS5 全文检索
storage.py          # 落盘层：文本/图片/视频/文件，MD5 去重，扩展名推断
forward.py          # 合并转发：递归展开（最多 3 层），媒体单次下载
magnets.py          # 磁力链接登记表：新链接按 btih 比对 xlsx 主表，缺则补行
riskwatch.py        # 风控看门狗：盯 NapCat/bot 日志，命中风险信号记本地告警日志
search.py           # 命令行全文检索工具
replay.py           # 队列状态查看 / error、dead 项重置重放
backup.py           # 每日备份 bot.db/xlsx/近 3 天消息 md
config.json         # 配置（含明文 token，已被 .gitignore 忽略）
config.example.json # 配置模板
start.bat           # 一键启动（隐藏 NapCat + 无控制台拉起 bot + 看门狗）
stop.bat            # 停止 bot + NapCat + QQ
start_hidden.vbs    # 无黑框入口
README.md
```

### NapCat 安装目录 `D:\tool\NapCat.Shell.Windows.OneKey\`

只保留**一套**自包含安装（旧的多套已合并清理）：

```
NapCat.44498.Shell\                     # 唯一 Shell
├── QQ.exe                              # 1.6M 启动器
├── NapCatWinBootMain.exe               # 注入器
├── NapCatWinBootHook.dll
├── napcat.bat / napcat.quick.bat       # 可见窗口入口，扫码用
└── versions\9.9.26-44498\resources\app\
    ├── <QQ 运行时一堆 dll>             # wmpfsdk/avsdk/win64 等
    └── napcat\                         # ← 内核在这里
        ├── napcat.mjs
        ├── loadNapCat.js
        ├── qqnt.json
        ├── node_modules\
        └── config\                     # ← 实际生效的 OneBot 配置
```

> 内核与 QQ 同属 `NapCat.44498.Shell`，**不存在跨目录接线**。曾经的故障正是
> `NAPCAT_DIR=4.18.28` 配 `NAPCAT_QQ=44498` 造成的：内核从一套加载，配置却顺着
> QQ.exe 去另一套读（日志表现为「适配器就绪但 9501/9502 不监听」）。

### 数据目录 `D:\QQBot_Data\`

```
messages\YYYY-MM-DD.md      # 所有消息文本（按日分文件，导出视图）
videos\                     # 全部视频（统一目录，按 MD5 命名）
bot.db                      # SQLite 索引库（事件队列 + 消息/媒体元数据）
bot.log                     # bot 运行日志
startup.log                 # start.bat 启动流程日志
napcat_console.log          # NapCat stdout（含二维码 ASCII 画）— riskwatch 盯着它
riskwatch.log               # 风控看门狗日志
bot.lock                    # 单实例锁文件（进程存活探针，见下）
磁力链接汇总.xlsx            # 磁力登记表
extract_magnets.py          # 离线全量重扫 + 磁力解析规则
private\                    # 私聊媒体 images/ files/
group\                      # 群聊媒体 images/ files/
```

> 文本统一写 `messages/YYYY-MM-DD.md`，群消息以 `[群号/昵称]` 前缀区分发言人；
> 图片按来源分流到 `private/`、`group/`；视频统一存顶层 `videos/`。
> 历史目录说明：顶层 `groups/`（若在）是早期存档，**请勿删除**；
> `private/videos/`、`group/videos/` 是统一前的旧位置，残留文件启动时自动搬入 `videos/`。

---

## 进程存活判定：用锁，不用命令行

**别用 `tasklist` / PowerShell 按命令行匹配判断进程是否活着。**

`start ""` 启动的 pythonw，其 `Win32_Process.CommandLine` **实测为空**
（WMI 读不到），因此基于命令行的匹配会漏判。`start.bat` 里那句
`WARN no bot.py alive after launch` 就是这个原因产生的**误报**——bot 实际在跑。

viewman 的后端改用 **bot.lock 文件锁探针**判定：bot.py 自身用
`msvcrt.locking`（即 `LockFileEx`）持单实例锁，Rust 侧 FFI 直调 kernel32 尝试
`LockFileEx` 抢锁，抢不到 = bot 在跑。这是最权威的判据，因为用的正是 bot 自己的
单实例机制。探测后立即 `UnlockFileEx`，且以 `.truncate(false)` 打开，不动锁文件内容。

同理，`taskkill` 也可能杀不掉——见「已知限制」里的管理员权限那条。

---

## 可靠性架构（asyncio + SQLite 队列）

消息处理管道分三层，核心原则：**先落库、后处理**——WS 收到的每条原始事件先写入
`bot.db` 的 `events` 表（WAL 模式），之后任何环节崩溃/超时都不丢消息：

1. **events 事件队列**：`new -> busy -> done`；处理异常转 `error` 按指数退避自动重试，
   穷尽（6 次）转 `dead`。进程重启时遗留的 `busy` 自动回队，未处理积压自动重放。
   `done` 事件保留 30 天（`EVENT_RETENTION_DAYS`）后启动时自动清理。
2. **顺序消费**：单 worker 按序处理文本并写 md 视图 + `messages` 表。
3. **dl 下载重试队列**：媒体下载登记为 `pending`，即时尝试一次；失败由 sweeper 每 20s
   扫描、指数退避自动重做，最多 `MAX_DL_TRIES=8` 次转 `dead`。图片/视频失败后还会
   自动改走 `file_id -> get_file` 取本地原件的兜底路径。
4. **停机补拉（默认关闭）**：见下节。
5. **磁力链接登记**：含 `magnet:` 的消息立即唤醒 sweeper 同步一轮；按 btih 与 xlsx 主表
   比对，缺失的特征码追加一行。游标只在写表成功后推进，Excel 开着时先攒内存。

检索：`messages_fts`（FTS5 trigram 分词，中文子串免分词）+ `media`（MD5 内容登记）。
除 `openpyxl`（磁力登记表读写）外无新增第三方依赖（sqlite3 为标准库）。

```bash
python search.py 关键词                        # 全文检索（bot 运行中也可用）
python search.py 关键词 --scope group --limit 100
python search.py --file abcd1234                # 按 MD5 前缀查媒体落点
python replay.py                               # 查看队列状态
python replay.py --retry                       # 把 error/dead 重置回队列重做
```

---

## 停机补拉（默认关闭）

WS 连上/重连后，原本会调 `get_group_msg_history` / `get_friend_msg_history`
把最近 14 天活跃会话的漫游历史批量拉回来补录（`BACKFILL_*` 常量控制页数、间隔等）。

**这个功能默认关闭**，开关在 `config.json`：

```json
"backfill_enabled": false
```

关闭原因：批量回捞会触发 QQ 服务端限流，实测日志出现

```
农场 | 获取视频url失败 Resource type 'video-url-get' is currently disabled until ...
[API ERROR] get_group_msg_history: {"retcode": 1200, "message": "消息undefined不存在"}
```

**关闭不影响实时存档。** bot 有两条独立管道：

| 管道 | 入口 | 作用 |
|---|---|---|
| 实时流 | `self.q.put()` → `worker()` | 推送过来的**新消息**，存档主力 |
| 回填 | `backfill_task()` → NapCat 历史 API | 断线期间漏掉的补捞 |

关掉的只是第二条。要恢复随时把 `backfill_enabled` 改成 `true`。

`dl` 状态里的 `dead` **不是终态**：跑满 8 次后会转入每日复检，
所以面板显示的是「待复检」而不是「已弃/失败」。

---

## 配置

编辑 `config.json`（实际键）：

```json
{
  "data_dir":       "D:\\QQBot_Data",
  "ws_url":         "ws://127.0.0.1:9501/",
  "api_url":        "http://127.0.0.1:9502",
  "ws_token":       "在此填写WebSocket的token",
  "qq_account":     "你的QQ号（可选，NapCat 快速登录用；留空则每次扫码）",
  "dl_limit":       2,
  "block_terms":    ["javdb", "onlyfans", "siterip"],
  "save_group":     false（只存私聊；true 恢复群聊存档）,
  "backfill_enabled": false
}
```

- `dl_limit`：同时进行的下载数上限。
- `block_terms`：命中即不下载的关键词。
- `magnet_xlsx` / `magnet_rules` 可省略，默认取 `data_dir` 下的同名文件；
  任一缺失或主表表头认不出「特征码」列时，只是不再登记磁力链接，**存档不受影响**
  （看 `bot.log` 的 `[MAGNET]`）。

支持用环境变量覆盖任意配置（前缀 `QQBOT_`），例如
`set QQBOT_WS_TOKEN=你的token`。`config.json` 含明文 token，**切勿提交**。

### NapCat 侧配置

实际生效的 OneBot 配置在**内核目录**下（NapCat 顺着 QQ.exe 找）：

```
D:\tool\NapCat.Shell.Windows.OneKey\NapCat.44498.Shell\versions\9.9.26-44498\resources\app\napcat\config\
```

该目录下每个账号一份 `onebot11_*.json`（`onebot11.json` 为默认，
`onebot11_<QQ号>.json` 为账号专属）。**三份都要启用**，否则对应账号不开端口：

```json
{
  "network": {
    "websocketServers": [{
      "host": "127.0.0.1",
      "port": 9501,
      "messagePostFormat": "array",
      "token": "与 config.json 的 ws_token 一致"
    }],
    "httpServers": [{
      "host": "127.0.0.1",
      "port": 9502,
      "enable": true,
      "token": "同上"
    }]
  },
  "enableLocalFile2Url": true
}
```

- `messagePostFormat` 必须为 `"array"`，否则脚本无法解析。
- 端口必须与 `config.json` 的 `ws_url` / `api_url` 一致。
- 改完在 NapCat WebUI 或重启 NapCat 生效。

> viewman 的「QQ」标签页内置**配置体检**，会检查：内核目录是否找到、QQ.exe 是否存在、
> 内核与 QQ 是否同属一套 Shell、生效配置目录是否存在、每份 onebot11 是否启用了端口。
> 今晚的两类误报（找不到深嵌内核、拿 npm 占位版本 0.0.1 去比 QQ build 号）都已修掉。

---

## 使用

### 启动 / 停止

```bat
:: 启动（管理员）
D:\opencword\viewman\qqbot\start.bat

:: 停止（管理员）
D:\opencword\viewman\qqbot\stop.bat
```

`start.bat` 的流程：检查 NapCatWinBootMain 是否在跑 → 没有则隐藏启动（并轮询 9501
端口最多 90 秒）→ 杀旧 bot.py → `pythonw.exe` 拉起新 bot.py → 拉起 riskwatch.py。

> `start.bat` 的 stdout 重定向**不能删**——`riskwatch.py` 靠监控
> `napcat_console.log` 做风险检测，日志断了它就失效。

验证是否正常：

```bat
:: NapCat 端口（两个都 LISTENING 才算就绪）
netstat -ano | findstr ":9501 :9502"
```

没有窗口 = 看不到实时输出，运行状态看日志：
`D:\QQBot_Data\napcat_console.log`、`bot.log`、`startup.log`。

### 备份

`backup.py` 把 `bot.db`（SQLite backup API 一致快照，运行中也可取）、
`磁力链接汇总.xlsx`、近 3 天的消息 md 拷到 `C:\QQBot_Backup\<日期>\`，保留 14 天。

> ⚠️ 计划任务 `QQBot_Backup`（每日 03:30）当前仍指向**已退休的旧路径**
> `D:\opencword\bot\backup.py`，需要重建任务：
>
> ```bat
> schtasks /create /tn QQBot_Backup ^
>   /tr "D:\Python311\python.exe D:\opencword\viewman\qqbot\backup.py" ^
>   /sc daily /st 03:30 /f
> ```

---

## 支持的 QQ 消息

**私聊**：全量保存（文本前缀 `[昵称(QQ号)]`），媒体入 `private/`。

**群聊**：图片、视频、文件、合并转发落盘（媒体入 `group/`）；文本同样写统一日记，前缀 `[群号/昵称]`。

| 类型 | 私聊 | 群聊 | 去重 |
|------|------|------|------|
| 文本 | .md 文件（带 `[群号/昵称]` 前缀） | 同左 | — |
| 图片 | 按 MD5 命名下载 | 同左 | 全量 md5 去重，删过的不再重拉 |
| 表情包 | 记 `[表情包]` 占位，不下载 | 同左 | — |
| 视频 | 按 MD5 命名下载（统一入 `videos\`） | 同左 | 全量 md5 去重，删过的不再重拉 |
| 文件 | 按 MD5 命名下载 | 同左 | 同名跳过（需 NapCat 支持 `get_file`） |
| 合并转发 | 递归展开内容（最多 3 层）写 .md | 同左 | — |
| 回复（引用） | `get_msg` 取回原文，写 `[回复←昵称：原文]` | 同左 | 原文里的媒体仅留标签，不额外下载 |

---

## 已知限制

- **每次冷启动都要重新扫码**：NapCat 注入 QQ 进程，QQ 每次启动都要重新授权。
- **管理员权限**：NapCat 启动的 QQ 与 bot.py 都是提权进程，普通用户级别的
  `Stop-Process` / `taskkill` 会返回「拒绝访问」。重启请走提权的 `start.bat`。
- **命令行不可见**：`start ""` 启动的 pythonw，`CommandLine` 为空，
  按命令行匹配的进程检测会漏判。判定存活请用 `bot.lock` 锁探针。
- **文件下载**：NapCat 的 `get_file` 可能超时。失败不丢消息，进入 dl 重试队列
  自动重做（约 8 次、覆盖 40 分钟），仍失败转 `dead`，可 `replay.py --retry` 重放。
- **嵌套合并转发**：NapCat 无法通过 API 展开内层合并转发，但可通过消息体的
  `content` 字段直接提取内容。
- **引用消息**：事件里只有被引用消息的 id，需回查 `get_msg`；原消息被撤回或超出
  漫游范围时取不到，退避重试穷尽后 md 只留 `[回复]`。
- **磁力登记表**：写入列按主表**表头文字**定位，手工删过列也能对齐；但若把「特征码」
  列改名或删掉，登记会自动停用（只影响登记，不影响存档）。实时补的行 `出现次数`
  恒为 1，需要全量口径时重跑 `python D:\QQBot_Data\extract_magnets.py`。
- **QQ 服务端限流**：批量回捞视频会触发 `video-url-get` 临时禁用（`retcode 1200`）。
  回填默认关闭可规避；实时收消息时若偶发该错误，属 QQ 侧独立风控。
- **不支持的消息类型**：语音（voice）、位置分享、红包、卡片消息等仅记录占位符。

---

## 技术栈

- **NapCatQQ Shell** — QQ 协议实现，对外暴露 OneBot 11 接口（WS 9501 / HTTP 9502）
- **Python asyncio** — 单事件循环：WS 连接（心跳+自动重连）、消息处理、媒体下载
- **aiohttp** — WebSocket 客户端与 HTTP 文件下载
- **openpyxl** — 磁力登记表增量读写
- **SQLite (WAL + FTS5 trigram)** — 事件持久队列、消息/媒体索引、全文检索
- **Rust + Tauri** — viewman 的状态检查、启停控制、下载进度展示