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
3. **dl 下载重试队列**：媒体下载登记为 `pending`，即起任务排下载槽（全库并发上限
   `MAX_CONCURRENT_DL=5`；排到槽才转 `busy`，
   其余保持 `pending`，面板与在传的行同显「下载中」）。**登记即入槽**：每条行
   落库的同时即起下载任务排进闸门，闸门 FIFO 唤醒——排队顺序 = 登记顺序 = 转发
   块内解析顺序（先解析先下载），槽位一空立刻接续下一行，不依赖任何轮询。
   sweeper 每 20s 一轮只管**退避到期的重试行**与重启遗留的 pending（每轮放 50
   行，大退避波次一轮抽干；`_dl_inflight` 挡住对已在槽内排队行的重复 spawn）。
   失败由 sweeper 每 20s 扫描自动重做，**生死以进度为准**（2026-11-07 起，
   替代旧的按次数判死）：本次尝试有任何字节进账就不算失败——重置计数 60 秒后快试；
   无字节进账且停滞超 `DL_STALL_DEAD_S`（30 分钟）才转 `dead`。停滞钟是每行的
   `prog_at`（最近一次字节进账时刻，`set_dl_progress` 每块推进、认领时起算），
   排队等槽、等退避不计时——积压再久也不会被误判失败。已认领的行若任务半路
   丢掉（`CancelledError` 不走 `except Exception` 等路径）会卡成假「下载中」：
   `_attempt_row` 对取消也补写回失败，sweeper 每轮再把停滞超线的 busy 行按同一条
   标准回收转 dead（`reap_stale_busy`，真在传的行 prog_at 每块推进、storage 的
   sock_read 超时兜底，到不了这条线）——「下载中」恒等于真在传的 ≤6 行
   （5 个占槽媒体 + 不占槽的转发壳/引用展开行）。
   图片/视频失败后还会自动改走
   `file_id -> get_file` 取本地原件的兜底路径——这条调用在**下载槽内**执行，预算
   分档：image/video 只给 1 次 30s（只是直链失效后的碰运气路径，NapCat 现场拉
   原件 90s×3 会把槽位 0 字节空占近 5 分钟，实测一张失效图片占槽 4.6 分钟、
   四槽全灭面板就是「下载中却没速度」）；file/record 的 get_file 是唯一取件路径，
   保留 90s×3。重试前还会先用 `get_msg` 换一条新鲜的
   CDN 地址——存下来的 url 带 rkey 签名，隔几天必失效，拿死链重试只是白跑。
   注意这只对**消息还查得到**的近期失败有效：实测 2026-10-13，库里 159 条 `dead` 中
   最老的 10 条 CDN 媒体行 `get_msg` 已回「消息不存在」，那些要靠 `get_file` 本地原件路径。
4. **断线缺口补录（默认开启）**：每次重连后只补掉线那几分钟的历史，见下节。
5. **停机补拉（默认关闭）**：14 天活跃会话的全量回捞，见下节。
6. **磁力链接登记**：含 `magnet:` 的消息立即唤醒 sweeper 同步一轮；按 btih 与 xlsx 主表
   比对，缺失的特征码追加一行。游标只在写表成功后推进，Excel 开着时先攒内存。

检索：`messages_fts`（FTS5 trigram 分词，中文子串免分词）+ `media`（MD5 内容登记）。
除 `openpyxl`（磁力登记表读写）外无新增第三方依赖（sqlite3 为标准库）。

```bash
python search.py 关键词                        # 全文检索（bot 运行中也可用）
python search.py 关键词 --scope group --limit 100
python search.py --file abcd1234                # 按 MD5 前缀查媒体落点
python replay.py                               # 查看队列状态
python replay.py --retry                       # 把 error/dead 重置回队列重做
python dedupe_magnets.py                       # 磁力登记表按特征码去重（清历史重复行）
```

---

## 补拉：缺口补录（默认开）与全量回捞（默认关）

两条都走 NapCat 的历史 API（`get_group_msg_history` / `get_friend_msg_history`），
区别只在**范围**和**代价**。

### 断线缺口补录 `gap_backfill_enabled`（默认 true）

只补「这段掉线期间」的消息：连接结束时记下掉线时刻，重连后对最近 3 天活跃过的
每个会话打**一次**历史请求、单页 20 条，取时间 ≥ 掉线时刻-2 分钟且库里没有的，
走正常入库管道。进程被杀/重启的那段也算缺口——起点取 `events` 表最后一条事件的
接收时间，所以启动连上后同样会补一次。

```json
"gap_backfill_enabled": true
```

- 常量：`GAP_CHAT_DAYS=3`、`GAP_PAGE=20`、`GAP_TIMEOUT=45`、`GAP_LEADIN_S=120`、
  `GAP_MAX_DOWN_S=7 天`、`GAP_MAX_FAILS=3`。缺口超过一周不自动拉，日志会提示改用
  `python replay.py`。
- 页大小是实测定的（2026-10-13，同一私聊会话）：历史接口耗时几乎线性于 count——
  `count=5` 1.1s、`count=10` 7.0s、`count=20` **10.2s（覆盖 22 小时）**、
  `count=50` **34.3s**。所以定在 20 条：一次请求够覆盖一整天的消息量，又不撞超时。
- 只有一页都问不到（超时/API 不可用）时**缺口不销账**，下次重连再补；连续
  `GAP_MAX_FAILS=3` 轮问不到才放弃并在日志里提示改用全量回捞或 `replay.py`。
- 实测规模：最近 3 天活跃会话 1 个（14 天内 20 个）→ 一次热态 10s 左右的请求。
  启动那轮 `get_friend_msg_history` 撞了 30s 超时（冷启），第二轮就正常了。
- 想彻底关掉：`config.json` 里置 `false` 重启。

### 全量回捞 `backfill_enabled`（默认 false）

每 6 小时 + 每次重连，把最近 14 天活跃会话各翻 3 页 ×100 条历史拉回来比对补录
（`BACKFILL_*` 常量控制页数、间隔等）。一轮几百次请求。

**这个功能默认关闭**，开关在 `config.json`：

```json
"backfill_enabled": false
```

关闭原因：批量回捞会触发 QQ 服务端限流，实测日志出现

```
农场 | 获取视频url失败 Resource type 'video-url-get' is currently disabled until ...
[API ERROR] get_group_msg_history: {"retcode": 1200, "message": "消息undefined不存在"}
```

缺口补录同样依赖历史 API，所以两者共用一个 `API_MIN_GAP_S=0.3` 的串行闸门
（`_api_gate()`），把对 QQ 侧的消息类请求压到最快 0.3 秒一次；`get_msg`（引用还原
与死信换签）也走这个闸门。要恢复全量回捞随时把 `backfill_enabled` 改成 `true`。

**关掉全量回捞不影响实时存档。** bot 有两条独立管道：

| 管道 | 入口 | 作用 |
|---|---|---|
| 实时流 | `self.q.put()` → `worker()` | 推送过来的**新消息**，存档主力 |
| 回填 | `gap_task()` / `backfill_task()` → NapCat 历史 API | 断线期间漏掉的补捞 |

`dl` 状态里的 `dead` **不是终态**：转死后转入每日复检，复检失败计满
`DL_LATE_TRIES=15` 次才真的不再碰（复检中进了字节即复活回队列，计数清零）。
所以 ViewMan 面板不显示累计死信（那串数只会一直挂着），显示的是**失败** = bot
还没放弃的行（`dl='dead'` 且 `tries < DL_LATE_TRIES`，与列表失败行同口径）——
救回或复检穷尽才减，**不按自然日清零**：失败是待处理的任务，不是日记，午夜
清零只会让人以为问题没了（2026-11-08 实测：昨夜 600+ 条失败午夜全部隐身）。
每日复检再失败**不刷新** `dead_at`（`bump_dl` 对 dead_at 在身的行只推进复检时刻
与 `tries`），旧积压因此不会被天天重计成「新失败」。复检中只要重新见到字节进账，
`bump_dl` 直接把行复活回 `pending`（清 `dead_at`/`tries`）——半下载的行离成功只差
几次重试，不该按失败计。复检**救回**（下载成功）的行由 `mark_dl_ok` 抹净全部失败
痕迹（`err`/`tries`/`dead_at`/`prog_at`）——成功即不再是失败记录，「失败」随之递减。
`replay.py --retry` 和面板「重试」按钮把行重置回队列（`tries=0`、清失败痕迹）后按
新标准重新走。累计死信仍然计入侧栏的积压徽标。

### 面板「下载列表」：每日清零 + 按状态筛选

面板只回答一个问题——**今天存了些什么**。所有「今日」口径都按本地自然日（今天 00:00 起算，
Rust 侧 `today_start_ts()` 用 chrono 算，与 sqlite 的 `localtime` 同一时区），午夜过后自动归零：

| 面板数字 | 口径 | 是否每日清零 |
|---|---|---|
| 累计已下载 N 个（字节） | `dl='ok'` 且 `kind IN (image,video,file)` 的全部行 | 否 |
| 今日 N 个（字节） | 同上，但 `ts >= 今天 00:00` **或** `ok_at >= 今天 00:00`（今天才下载完成的救回行也计） | 是 |
| 今日重复跳过 N | `dedup > 0` 的非转发行，且 `ts >= 今天 00:00` | 是 |
| 失败 N | `dl='dead'` 且复检未穷尽（`tries < 15`）——救回或 bot 放弃才离场 | 否（迅雷式：失败是待处理任务，不按日清零） |
| 下载中 N（含排队） | 队列当前长度（pending+busy 合并） | 不是日计数，是实时积压 |

列表默认看**「今日」**而非「最近 N 条」，无条数上限；右上角可切**「历史」**页签（见下）。
一行入选的条件：

1. `ts >= 今天 00:00`——今天到的消息（含今早排队、下午才下完的）；
2. **父行的 `ts >= 今天 00:00`**——今天收到的转发带进来的全部明细行；
3. `dl='dead' 且 tries < 15`——**失败行不按日退场**：bot 没放弃（还在每日复检）就
   一直挂在列表，救回转「已下载」、复检穷尽才退场（历史页签仍可查，「重试」唤回
   重新入列）；旧口径「今天才转死」在午夜会把还没处理完的失败整批清出列表；
4. `dl='ok' 且 ok_at >= 今天 00:00`——**今天才下载完成的**（复检救回的旧积压、
   补录的老消息），按完成时刻归日：积压每救回一行列表就多一条「已下载」，
   而不是悄悄消失（实测一天救回 1125 行全被这条分支漏掉）。

纯文本直发消息（`dl='none'` 且无 `parent`）不进列表——面板是下载/归档视角。
**转发壳行（kind='forward'）同样不进列表**：只列实际内容行（纯文本转发的归档内容
只在日记与 FTS 里）。排序是**迅雷式**：正在下载的组置顶（busy 明细把整组一起带上、
不拆块），其余按**完成时间倒序**——刚下完的紧跟置顶区，救回的旧积压按完成时刻排队
（不再按消息新旧散开），没完成时刻的行（排队/失败）沉底按消息序；组完成时刻取组内
最大 `ok_at`（看最后一块完成），块内**已完成的明细按各自完成时刻倒序**——并行下载
的完成序≠解析序，不按各自时刻排的话「完成」列顺着读是乱跳的；未完成的明细（busy/
pending/文本）垫底按解析序；正在处理中的行恒可见（busy 明细
含旧积压的复检、busy 转发壳的全部明细整块），不落任何归日分支也不会隐身；
标签前缀是内层发送者昵称（`sender_nick`），转发里不同人发的同名文件才分得清。
列表列：内容 / 接收时间 / 完成 / 大小 / 类型 / 状态。「完成」= `messages.ok_at`
（`mark_dl_ok` 收敛为 ok 时落的 Unix 秒），未完成与历史 ok 行（旧版没记过时刻）显示 "-"。
「大小」列在传行显示 `已下 / 总量`——总量 Content-Length 优先、登记期解析到的预估
大小兜底（转发段自带 `file_size`，登记明细行时落 `messages.bytes`，排队就有数，
成功后 `mark_dl_ok` 以实际落盘字节数覆盖）；都没有只显已落盘字节；
「状态」列下载中带实时百分比与速度——速度按 8 秒窗口差分估算（进度按 1MB 块跳变，
逐秒差分全是锯齿），15 秒无进账自动隐藏。列表近实时靠推送：bot 每次落库，Rust
监视 bot.db(-wal) 防抖后发 `bot-dl-changed`，前端拉轻量快照（useBotStatus），叠在
10s 全量轮询之上。防抖是「500ms 静默或 1s 封顶」——活动下载的进度写入从不断流，
纯静默防抖会被饿死（下载越忙越不推送，正好和实时相反）。
「内容」按 **文件名 → 文本前 80 字 → 落盘文件名（path 基名）→ 类型中文名** 取值：
QQ 的图片/视频事件多数不带原始文件名（落盘以 md5 为名），path 基名就是最具体的内容；
展开中的转发壳、取不到原文的引用连 path 都没有，先显类型名（合并转发/引用回复/图片…），
展开完成或引用取回后自动变具体——任何行都不再落空标签。

前端按状态筛选，不回后端：`全部 / 下载中 / 失败 / 重复跳过 / 已下载 / 已归档`。
排队与在传合并为一个「下载中」（迅雷式任务视角：排队等槽是同一次下载的等待段），
在传的行另带实时百分比/已落盘字节，排队行显示纯「下载中」；归类规则在
`src/botDlFilter.ts`（行内标签与筛选角标共用同一套判定，单测在
`src/botDlFilter.test.mjs`）。

**失败行按内容聚合**（2026-11-08 起）：同一段转发被连发几遍时，同一文件会在
不同转发块下各挂一行失败、各自计各自的复检，列表上一片重复红字（实测 214 条
失败里 31 行是 19 组内容的重复副本）。前端按 `kind+name` 把死行归组显示一行
带 ×N 角标——QQ 媒体文件名即内容 md5，同名就是同内容；无名行不聚。
代表行取最近转死的那份（失败原因最新鲜）；「重试」用 `bot_dl_retry` 一并拉起
组内全部副本，任一份成功后 `mark_dl_ok` 的同名救回把整组转「已下载」，组自然
消失。纯展示层：`collapseDeadDuplicates`（含单测）只动列表行与计数，排序与
后端数据不变；侧栏积压徽标仍是原始行数口径。

**「历史」页签（去掉时间限制）**：列表右上角「今日 / 历史」切换。历史不限日期，
走按需的 `bot_dl_history` 命令一次取最近的记录（缺省 500 条、上限 2000，超出显示
「只显示最近 N 条」）——它**不在快照轮询里**：全库几十万行不可能每 2 秒端一遍，
点进页签才查一次，右上角给手动「刷新」。入选条件与当日列表同源（有下载/归档动作，
或被转发带进来的明细行；纯文本直发同样不进列表），排序同键，转发与其明细连成一块。
与当日列表相反，这里**故意用一条 OR 查询**：按需查询不在乎扫表加排序的几十毫秒，
LIMIT 让 sqlite 走 top-N，不值得为它复制三支索引查询；切回「今日」恢复实时刷新。

**失败行的「重试」按钮**：失败行状态后面有「重试」，点击调 `bot_dl_retry` 把该行
重置回队列头（`dl='pending'`、`tries=0`、`next_try=0`、清 err/dead_at）——与
`replay.py --retry` 同口径的按钮版；聚合行（带 ×N）一次拉起组内全部副本。
不必惊动 bot 进程：运行中的 bot 由 sweeper
在 `SWEEP_INTERVAL`（20 秒）内扫到并重下；bot 没跑时重置也会留着，下次启动接手。
重置后再失败按新标准走（有进度就续命，停滞 30 分钟才再转死）。历史页签重试成功后
自动重拉列表。

**明细行为什么按父行归日**：明细行的 `ts` 是**内层消息的原始时间**（`bot.py` 里
`report` 传的就是 `inner_ts`）——转发是今天收到的，里面的图可能是三个月前发的。
按自己的 ts 归日会让今天收到的转发在面板里只剩孤零零一个父行、里面几百张图一条都不显示
（实测某天 19 个转发、705 条明细全部漏掉）。它是被哪条转发带进来的，就记在哪天。

> 取数拆成六条查询，各自命中索引后在内存里按 id 去重、内存排序：
> `ts >= 今天` 走 `messages_ts`；`JOIN messages p ON p.id = m.parent` 走 `messages_ts` +
> `messages_parent`；`dl='dead' AND tries < 15`（失败行不归日）走 `messages_due`；
> `dl='ok' AND ok_at >= 今天` 走 `messages_ok_at`；`dl='busy'` 与 `p.dl='busy'`
> 走 `messages_due` 的 dl 前缀（正在处理恒可见）。写成一条带
> `ORDER BY id DESC` 的 OR 查询，sqlite 会判成「倒着扫表更便宜」而丢掉索引——6 万行实测
> 25ms/次，2 秒一轮的快照纯浪费；拆开后 1.5ms。
> `messages_parent` / `messages_ok_at` 索引由 `db.py` 启动时建（老库补列之后，
> 否则老库会炸启动）；没有它们，对应分支会退化成全表扫 27ms。
> 第一条**不要**加 `parent IS NULL`：那会让 sqlite 改用 `messages_parent` 扫出全部 5 万条
> 顶层行（28ms vs 0.16ms）。顺带命中「ts 是今天的明细行」也无妨——转发里的消息时间必然
> 不晚于转发本身，三支重叠由去重兜住。

---

## 配置

编辑 `config.json`（实际键）：

```json
{
  "data_dir":       "D:\\QQBot_Data",
  "ws_url":         "ws://127.0.0.1:9501/",
  "api_url":        "http://127.0.0.1:9502",
  "ws_token":       "在此填写WebSocket的token",
  "napcat_root":    "NapCat OneKey 根目录（viewman 用，可省略走默认）",
  "qq_account":     "你的QQ号（可选，NapCat 快速登录用；留空则每次扫码）",
  "dl_limit":       2,
  "save_group":     false（只存私聊；true 恢复群聊存档）,
  "gap_backfill_enabled": true（断线缺口补录，默认开）,
  "backfill_enabled": false
}
```

- `dl_limit`：每秒允许的下载数（令牌桶限速，只作用于**真的要下载**的媒体；
  命中 md5 去重的免下载判定不占额度）。
- `gap_backfill_enabled` / `backfill_enabled`：两条历史回捞管道的开关，
  默认只开窄范围的缺口补录，见「补拉」一节。
- `magnet_xlsx` / `magnet_rules` 可省略，默认取 `data_dir` 下的同名文件；
  任一缺失或主表表头认不出「特征码」列时，只是不再登记磁力链接，**存档不受影响**
  （看 `bot.log` 的 `[MAGNET]`）。
- `link_xlsx` 可省略（也可用环境变量 `QQBOT_LINK_XLSX` 覆盖），默认
  `data_dir\网址链接汇总.xlsx`；不存在会自动新建。
  私聊/群聊文本里的 http(s) 网址按链接去重后登记（大小写、收尾标点、`&amp;`
  转义视为同一条），磁力链接不进这张表，归 `magnet_xlsx` 管（看 `bot.log` 的 `[LINK]`）。

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
| 表情包/商城表情 | 按图片下载（GIF 保留动图） | 同左 | 图片类去重 |
| 视频 | 按 MD5 命名下载（统一入 `videos\`） | 同左 | 全量 md5 去重，删过的不再重拉 |
| 文件 | 按 MD5 命名下载 | 同左 | 同名跳过（需 NapCat 支持 `get_file`） |
| 合并转发 | 递归展开内容（最多 3 层）写 .md | 同左 | — |
| 回复（引用） | `get_msg` 取回原文，写 `[回复←昵称：原文]` | 同左 | 原文里的媒体仅留标签，不额外下载 |
| 卡片/分享（json/xml/share/music/location） | 提取展示文案与全部网址/磁力链接写入库和 .md | 同左 | 链接不下载，只存档文本 |

---

## 已知限制

- **每次冷启动都要重新扫码**：NapCat 注入 QQ 进程，QQ 每次启动都要重新授权。
- **管理员权限**：NapCat 启动的 QQ 与 bot.py 都是提权进程，普通用户级别的
  `Stop-Process` / `taskkill` 会返回「拒绝访问」。重启请走提权的 `start.bat`。
- **命令行不可见**：`start ""` 启动的 pythonw，`CommandLine` 为空，
  按命令行匹配的进程检测会漏判。判定存活请用 `bot.lock` 锁探针。
- **文件下载**：NapCat 的 `get_file` 可能超时。失败不丢消息，进入 dl 重试队列
  自动重做（进度判生死：停滞 30 分钟转 `dead`），可 `replay.py --retry` 重放。
- **嵌套合并转发**：NapCat 无法通过 API 展开内层合并转发，但可通过消息体的
  `content` 字段直接提取内容。
- **引用消息**：事件里只有被引用消息的 id，需回查 `get_msg`；原消息被撤回或超出
  漫游范围时取不到，退避重试穷尽后 md 只留 `[回复]`。
- **磁力登记表**：写入列按主表**表头文字**定位，手工删过列也能对齐；但若把「特征码」
  列改名或删掉，登记会自动停用（只影响登记，不影响存档）。实时补的行 `出现次数`
  恒为 1，需要全量口径时重跑 `python D:\QQBot_Data\extract_magnets.py`。
- **QQ 服务端限流**：批量回捞视频会触发 `video-url-get` 临时禁用（`retcode 1200`）。
  回填默认关闭可规避；实时收消息时若偶发该错误，属 QQ 侧独立风控。
- **不支持的消息类型**：红包等少数类型仅记录占位符；语音与卡片/分享链接已按存档处理
  （语音落盘、卡片提取文案与链接）。卡片里的链接不会自动下载，只入档文本。

---

## 技术栈

- **NapCatQQ Shell** — QQ 协议实现，对外暴露 OneBot 11 接口（WS 9501 / HTTP 9502）
- **Python asyncio** — 单事件循环：WS 连接（心跳+自动重连）、消息处理、媒体下载
- **aiohttp** — WebSocket 客户端与 HTTP 文件下载
- **openpyxl** — 磁力登记表增量读写
- **SQLite (WAL + FTS5 trigram)** — 事件持久队列、消息/媒体索引、全文检索
- **Rust + Tauri** — viewman 的状态检查、启停控制、下载进度展示