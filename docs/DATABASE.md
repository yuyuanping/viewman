# 数据库设计

## 概述

- 引擎：SQLite，经 `rusqlite`（`bundled` 特性，自带静态编译的 SQLite）。
- 文件：随应用数据目录，Windows 下为 `%APPDATA%/com.viewman.app/viewman.db`。
  路径由 `lib.rs::get_db_path` 经 `app.path().app_data_dir()` 取得，启动时确保目录存在（fail-fast）。
- 模式：`WAL` + `synchronous=NORMAL` + `foreign_keys=ON` + `busy_timeout=5000ms`。
  WAL 提升写吞吐，NORMAL 在崩溃时最多丢最后一个事务而非损坏库（配合断点续跑的"进程被杀"场景）。

## 启动初始化流程

`init_db(db_path)`（`db.rs:250`）按序执行：

1. 打开连接，设置上述四项 PRAGMA。
2. `create_tables`：`CREATE TABLE IF NOT EXISTS` 建全部表 + FTS5 虚表 + 触发器 + 索引。
3. `ensure_columns`：给老库补列（`IF NOT EXISTS` 不加字段，需显式 `ALTER TABLE ADD COLUMN`）。
4. `ensure_fts`：老库 FTS 回填（触发器只管新写入，老记录需 `rebuild` 一次）；失败不致命，记错即可。

此连接经 `Mutex` 存入 `AppState.db` 作为全局写连接；只读查询另开 `open_read_conn`（同样 WAL）走并发读，不与写抢全局锁。

## 表结构

### videos

| 字段 | 类型 | 说明 |
|---|---|---|
| id | TEXT PK | UUID v4 |
| path | TEXT UNIQUE NOT NULL | 文件绝对路径，去重依据 |
| filename | TEXT NOT NULL | 文件名（FTS 索引） |
| duration | REAL | 时长(秒)，缺失表示未探测 |
| width / height | INTEGER | 分辨率 |
| file_size | INTEGER NOT NULL | 字节数 |
| created_at | TEXT NOT NULL | 入库时间 ISO8601 |
| thumbnail_path | TEXT | 缩略图缓存路径，NULL=未生成 |
| video_codec | TEXT | 编码名（HEVC 检测用） |
| anchor_phash / anchor_dhash | INTEGER | 锚点帧（缩略图那帧）感知/差值哈希 |
| mid_phash / mid_dhash | INTEGER | 中段复核帧指纹 |
| tail_phash / tail_dhash | INTEGER | 结尾复核帧指纹 |
| sig_modified_at | TEXT | 指纹时间戳（缓存是否作废） |

六列复核帧指纹对"锚点对得上"的候选才抽，全允许为 NULL。

### watch_progress

| 字段 | 类型 | 说明 |
|---|---|---|
| id | TEXT PK | UUID |
| video_id | TEXT UNIQUE NOT NULL | 外键 → videos.id，`ON DELETE CASCADE` |
| position | REAL NOT NULL DEFAULT 0 | 播放位置(秒) |
| updated_at | TEXT NOT NULL | 最近一次保存时间 |

### images

| 字段 | 类型 | 说明 |
|---|---|---|
| id | TEXT PK | UUID |
| path | TEXT UNIQUE NOT NULL | 绝对路径 |
| filename | TEXT NOT NULL | 文件名（FTS 索引） |
| width / height | INTEGER | ffprobe 探测，NULL=未探测 |
| file_size | INTEGER NOT NULL | 字节数 |
| created_at | TEXT NOT NULL | 入库时间 |
| thumbnail_path | TEXT | 缩略图路径 |
| modified_at | TEXT | 文件修改时间，照片整理排序用 |
| phash | INTEGER | 感知哈希（相似检测） |
| dhash | INTEGER | 差值哈希 |
| sig_pixels | BLOB | 32×32 缩略像素，重复判定的最后一道 |
| sig_modified_at | TEXT | 指纹时间戳 |
| is_animated | INTEGER | 动图判定缓存（NULL=未判定/已失效） |

### settings

| 字段 | 类型 | 说明 |
|---|---|---|
| key | TEXT PK | 键 |
| value | TEXT NOT NULL | 值 |

用法（存 JSON 数组，视频/图片各一份互不干扰）：

| key | 内容 |
|---|---|
| `scan_roots` | 视频库扫描根目录 |
| `image_scan_roots` | 图片库扫描根目录 |
| `video_move_targets` | 视频批量移动目标目录 |
| `image_move_targets` | 图片批量移动目标目录 |

## 全文索引（FTS5）

```sql
CREATE VIRTUAL TABLE videos_fts USING fts5(filename, path, content='videos', content_rowid='rowid');
CREATE VIRTUAL TABLE images_fts USING fts5(filename, path, content='images', content_rowid='rowid');
```

由触发器维护同步（`videos_fts_insert/delete/update`、`images_fts_*`）。
将十万级前缀 LIKE 从 O(N) 全表扫描降到毫秒级 `MATCH`。

## 索引

| 索引 | 用途 |
|---|---|
| idx_videos_path / idx_images_path | 前缀 LIKE、检测待办 |
| idx_videos_filename / idx_images_filename | 文件名查询 |
| idx_videos_codec | HEVC 检测 |
| idx_videos_thumbnail_null | 缺缩略图待办（部分索引） |
| idx_images_phash_null | 缺指纹待办（部分索引） |
| idx_images_animated_null | 动图待判定（部分索引） |
| idx_images_thumbnail_null | 缺缩略图待办（部分索引） |
| idx_watch_progress_updated | 最近播放排序 |

## 迁移机制

`ensure_columns` 用 `PRAGMA table_info` 探现有列，缺则 `ALTER TABLE ADD COLUMN`，幂等可每次启动跑：

- videos：补 `thumbnail_path`、`video_codec`、六列复核指纹、`sig_modified_at`。
- images：补 `modified_at`、`phash`；把老列 `phash_modified_at` **重命名**为 `sig_modified_at`（保留时间戳、缓存不作废）；补 `dhash`、`sig_pixels`、`is_animated`。

`ensure_fts`：若 FTS 行数为 0 而主表有数据，执行 `INSERT INTO <fts>(<fts>) VALUES('rebuild')` 回填。

## 并发模型

- 单写连接（`AppState.db`，`Mutex` 串行化）。
- 只读查询走 `open_read_conn` 新开连接（WAL 并发读），不与写抢全局锁。
- 扫描多线程逐条 insert 时极易 `SQLITE_BUSY`，`busy_timeout=5000ms` 让写等待而非直接失败。
- 删除/写入走事务（`unchecked_transaction`），失败回滚保库记录与进度。

## 自检与自愈

- `integrity_check`：`PRAGMA integrity_check`，返回 `"ok"` 或首条损坏描述。
  定性是 FTS 索引坏（可无损重建）还是主表坏（需从备份恢复）。
- `rebuild_fts`：无损重建 FTS（主表不动），返回重建后行数。命令 `rebuild_fts_indexes` 暴露给前端。
- `is_corruption_error`：错误信息含 `malformed`/`corrupt` 判为疑似损坏。
- `delete_records_with_heal`：删除首次失败若疑似损坏，先无损重建 FTS 再重试一次；仍失败附即时自检结果。文件已先进回收站，重试时"磁盘已无文件"按已删除计。
- 维护命令：`check_db_integrity`、`rebuild_fts_indexes`、`db_file_path`。

## 检测结果缓存（JSON 落盘）

三趟检测（重复图 / 相似图 / 重复视频）结论各落一份 JSON 于 `<app_data>`：

- `duplicate-cache.json`（图片重复）
- `similar-cache.json`（图片相似）
- `video-duplicate-cache.json`（视频重复）

每份记 `libraryCount`（存结果时库内记录数）。前端只在条数变大时提示"建议重跑"；减少的条目由 `liveGroups`/`pruneGroupsToLive` 当场裁掉。相似缓存带每张 64 位指纹（几 MB），读写放阻塞线程池。详见 `src/detectionCache.ts` 与设计规格追记。
