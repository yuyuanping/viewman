# Windows Shell 调用约定（给 Agent / 协作者）

本机为 Windows。以下规则用于让 AI 编码助手（及人类）在本仓库里**稳、快、不踩转义坑**地调用命令行。

## 一、可用命令入口一览

**Shell：**
- `cmd.exe` — 默认主力
- Windows PowerShell 5.1 — 仅旧版管理场景
- `pwsh` 7.6.3 — Windows 管理的首选（比 5.1 新）

**开发：**
- Python 3.14
- Node / npm
- Git
- `rg`（ripgrep，比 find/grep 快）

**系统 / 运维：**
- OpenSSH（`ssh` / `scp`）
- `curl`
- `tar`
- `wsl`
- `robocopy`
- `schtasks`（计划任务）
- `reg`（注册表）
- `netsh`

**常用 CLI：**
- GitHub CLI `gh`
- Kimi CLI
- VS Code（`code`）

---

## 二、默认用 cmd，别上来就 pwsh

运行以下场景时，**直接用 `cmd`**（一行、不含复杂文本即可）：
- Python / Node / Git / SSH / Docker
- 现成 CLI（`gh`、`rg`、`curl`…）
- `.bat` / `.cmd` 脚本

**仅当涉及 Windows 管理才用 `pwsh`**，例如：
- 注册表（`reg` 不够用时）
- 服务、进程
- WMI / CIM
- PowerShell 对象管道（`Get-Process | Where-Object` 之类）

> 优先 `pwsh` 7，不再默认用旧版 PowerShell 5.1。

---

## 三、复杂内容不要在 shell 里硬转义

只要命令里出现以下任一情况，**立刻改用脚本文件**，别在命令行里手搓引号：
- JSON
- 正则
- HTML
- 多行文本
- 嵌套引号
- 变量插值

对应落点：
| 内容类型 | 改用 |
|---|---|
| 文件小改（代码/配置） | `edit_file` / `patch` 工具 |
| 文本 / 数据逻辑 | 临时 `.py` 脚本 |
| Windows 管理多步骤 | 临时 `.ps1` 脚本 |

**`cmd` 转义失败一次就停止硬拗**：不继续给引号打补丁，直接换成脚本文件。

---

## 四、路径与环境变量固定写法

- **工具调用优先传 `cwd`**，避免依赖 `cd` 留下的状态（不同调用间不保证 cwd 连续）。
- **cmd 临时变量**：`set "A=B"&& command`
- **pwsh 临时变量**：`$env:A='B'; command`
- **中文 / 空格 / 特殊字符路径**：
  - 优先用 Python `pathlib` 处理
  - PowerShell 里用 `-LiteralPath`（避免把路径当通配符解析）

---

## 五、为什么这么定（给 Windows 宝宝们的 TL;DR）

1. `cmd` 启动快、转义简单，一行命令能跑就别开 pwsh。
2. pwsh 的对象管道很强，但**命令行里拼 JSON / 正则 / 引号**会让人崩溃——这种活交给 `.py` / `.ps1` 文件。
3. 路径有中文或空格时，shell 通配符和引号是重灾区；`pathlib` + `-LiteralPath` 是安全带。
4. 每次都传 `cwd`，别假设上一条命令 `cd` 过去了——状态不跨调用保留。
