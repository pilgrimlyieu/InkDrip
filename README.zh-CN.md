<div align="right">

**[简体中文](README.zh-CN.md)** | **[English](README.md)**

</div>

# InkDrip

**将书籍转换为 RSS 订阅 — 细水长流地享受阅读。**

InkDrip 是一个自托管服务，可将电子书拆分为小片段，并通过标准 Atom/RSS 订阅源按可配置的计划发布。用任意 RSS 阅读器（FreshRSS、Miniflux、Inoreader 等）订阅，每天读一点——不多也不少。

## 功能特性

- **多格式支持** — EPUB、纯文本及 Markdown
- **智能分段** — 遵循段落与句子边界，避免在语义中断处切割
- **可配置计划** — 设置每日阅读字数、发布时间、时区及跳过特定日期
- **标准订阅格式** — 输出 Atom 及 RSS 2.0，通过 `/feeds/:slug/atom.xml` 或 `/feeds/:slug/rss.xml` 访问
- **多书并行** — 每本书独立订阅源，同时管理多本书
- **聚合订阅** — 将多个订阅源合并为一个统一的 Feed
- **OPML 导出** — 一键将所有订阅源导入阅读器
- **文件监控** — 将书籍放入指定目录即可自动导入
- **内容变换** — 阅读进度指示器、自定义 CSS、导航链接
- **钩子系统** — 通过 JSON stdin/stdout 在关键管线节点运行外部命令
- **撤销 / 重做** — 回滚或重播对书籍与订阅的近期操作
- **极小占用** — 单一二进制文件，SQLite 存储，<15MB Docker 镜像，<10MB 内存

## 快速开始

### Docker（推荐）

```bash
# 拉取并运行
docker run -d \
  --name inkdrip \
  -p 8080:8080 \
  -v inkdrip-data:/data \
  -e INKDRIP__SERVER__BASE_URL=http://your-server:8080 \
  pilgrimlyieu/inkdrip:latest

# 上传书籍
curl -F "file=@my-book.epub" http://localhost:8080/api/books

# 创建订阅（使用响应中的 book ID）
curl -X POST http://localhost:8080/api/books/<BOOK_ID>/feeds \
  -H "Content-Type: application/json" \
  -d '{"words_per_day": 3000}'

# 在 RSS 阅读器中订阅：
# http://localhost:8080/feeds/<slug>/atom.xml   （Atom 格式）
# http://localhost:8080/feeds/<slug>/rss.xml    （RSS 2.0 格式）
```

> **部署注意：** 请将 `INKDRIP__SERVER__BASE_URL` 设置为你的公网地址。
> 使用 `localhost` 或 `127.0.0.1` 会在启动时产生警告，且外部阅读器无法正确访问订阅链接。

### Docker Compose

完整配置（含 RSSHub 与 FreshRSS）请参见 [docker-compose.yml](docker-compose.yml)。

### 从源码构建

```bash
# 需要 Rust 1.85+
cargo build --release

# 运行服务器
./target/release/inkdrip-server

# 或使用 CLI
./target/release/inkdrip-cli --help
```

## CLI 使用

CLI 通过 HTTP API 与运行中的服务器通信。

**全局参数**（适用于所有命令）：

| 参数      | 环境变量        | 默认值                  | 说明           |
| --------- | --------------- | ----------------------- | -------------- |
| `--url`   | `INKDRIP_URL`   | `http://localhost:8080` | 服务器地址     |
| `--token` | `INKDRIP_TOKEN` | *(空)*                  | API 认证 Token |
| `--json`  | —               | —                       | 输出原始 JSON  |

```bash
# 设置服务器地址（或使用 --url 参数）
export INKDRIP_URL=http://localhost:8080

# 上传书籍
inkdrip add my-book.epub --title "书名" --author "作者名"

# 列出书籍 / 订阅
inkdrip list books
inkdrip list feeds

# 创建订阅
inkdrip feed create <BOOK_ID> --words-per-day 3000 --delivery-time 08:00

# 暂停 / 恢复 / 查看状态
inkdrip feed pause <FEED_ID>
inkdrip feed resume <FEED_ID>
inkdrip feed status <FEED_ID>

# 立即推进 N 个即将发布的片段（默认: 1）
inkdrip feed advance <FEED_ID> --count 3

# 编辑书籍元数据或订阅配置
inkdrip edit book <BOOK_ID> --title "新标题" --author "新作者"
inkdrip edit feed <FEED_ID> --words-per-day 2000 --delivery-time 09:00

# 重新分段（保留已发布的片段）
inkdrip resplit <BOOK_ID> --target-words 1200

# 阅读指定片段
inkdrip read <BOOK_ID> <SEGMENT_INDEX>

# 删除书籍及其所有订阅
inkdrip remove <BOOK_ID>

# 撤销 / 重做 / 历史
inkdrip history list            # 查看近期操作历史
inkdrip history undo            # 撤销上一步操作
inkdrip history redo            # 重做上一步撤销
inkdrip history clear           # 清除历史并永久删除软删除资源

# 聚合订阅
inkdrip aggregate create <SLUG> --title "每日阅读" --feeds <SLUG_1> --feeds <SLUG_2>
inkdrip aggregate list
inkdrip aggregate delete <AGGREGATE_ID>

# 调试 / 查看
inkdrip debug segments <BOOK_ID>         # 列出所有片段
inkdrip debug releases <FEED_ID>         # 列出发布时间表
inkdrip debug preview <FEED_ID> --limit 5  # 预览即将发布的片段
```

## 配置

将 [config.example.toml](config.example.toml) 复制为 `config.toml`（Docker 中为 `data/config.toml`）。

所有配置项均可通过 `INKDRIP__` 前缀的环境变量覆盖：

```bash
INKDRIP__SERVER__PORT=9090
INKDRIP__DEFAULTS__WORDS_PER_DAY=2000
INKDRIP__DEFAULTS__TIMEZONE=America/New_York
INKDRIP__WATCH__ENABLED=true
```

### 主要配置项

| 配置项                          | 默认值                  | 说明                                                       |
| ------------------------------- | ----------------------- | ---------------------------------------------------------- |
| `server.host`                   | `0.0.0.0`               | HTTP 服务器绑定地址                                        |
| `server.port`                   | `8080`                  | 监听端口                                                   |
| `server.base_url`               | `http://localhost:8080` | 用于生成订阅链接的公开 URL                                 |
| `server.api_token`              | *(空)*                  | API 认证 Bearer Token；为空则不启用认证                    |
| `server.public_feeds`           | `true`                  | 订阅/OPML/聚合端点是否公开；设为 `false` 则需要 Token 认证 |
| `server.max_upload_bytes`       | `52428800`              | 最大上传文件大小（字节），默认 50 MiB                      |
| `storage.data_dir`              | `./data`                | 数据库、书籍及图片存储目录                                 |
| `defaults.words_per_day`        | `3000`                  | 每日阅读字数预算                                           |
| `defaults.target_segment_words` | `1500`                  | 每段目标字数                                               |
| `defaults.max_segment_words`    | `2000`                  | 每段最大字数                                               |
| `defaults.min_segment_words`    | `500`                   | 每段最小字数                                               |
| `defaults.delivery_time`        | `08:00`                 | 每日发布时间（HH:MM）                                      |
| `defaults.timezone`             | `Asia/Shanghai`         | 排程使用的时区                                             |
| `defaults.skip_days`            | `[]`                    | 跳过的日期（见下方）                                       |
| `watch.enabled`                 | `false`                 | 是否自动导入目录中的书籍                                   |
| `watch.dir`                     | `./books`               | 监控的书籍目录                                             |
| `watch.auto_create_feed`        | `true`                  | 检测到新书时自动创建订阅                                   |
| `watch.scan_interval_secs`      | `300`                   | 目录扫描间隔（秒）                                         |
| `feed.format`                   | `atom`                  | 默认订阅格式（`atom` 或 `rss`）                            |
| `feed.items_limit`              | `50`                    | 每次请求返回的最大条目数                                   |
| `history.stack_depth`           | `50`                    | 最多保留的撤销操作数                                       |

完整配置参考（含 `[transforms]`、`[hooks]`、`[parser.txt]`、`[[aggregates]]` 等）请参见 [config.example.toml](config.example.toml)。

### 跳过日期

`skip_days` 接受一个日期名称数组（支持全名或缩写，大小写不敏感）：

| 全名        | 缩写  | 日期 |
| ----------- | ----- | ---- |
| `monday`    | `mon` | 周一 |
| `tuesday`   | `tue` | 周二 |
| `wednesday` | `wed` | 周三 |
| `thursday`  | `thu` | 周四 |
| `friday`    | `fri` | 周五 |
| `saturday`  | `sat` | 周六 |
| `sunday`    | `sun` | 周日 |

示例：`skip_days = ["saturday", "sunday"]` 跳过周末。

> **注意：** JSON API 中的 `skip_days` 接受 `u8` 位域整数
> （`MON=1, TUE=2, WED=4, THU=8, FRI=16, SAT=32, SUN=64`）。

## API 参考

### 书籍

| 方法     | 路径                             | 说明                                                  |
| -------- | -------------------------------- | ----------------------------------------------------- |
| `POST`   | `/api/books`                     | 上传书籍（multipart：`file`，可选 `title`、`author`） |
| `GET`    | `/api/books`                     | 列出所有书籍                                          |
| `GET`    | `/api/books/:id`                 | 书籍详情（含章节与订阅源）                            |
| `PATCH`  | `/api/books/:id`                 | 更新书籍元数据                                        |
| `DELETE` | `/api/books/:id`                 | 删除书籍及其所有订阅                                  |
| `GET`    | `/api/books/:id/segments`        | 列出书籍的所有片段                                    |
| `GET`    | `/api/books/:id/segments/:index` | 读取指定片段                                          |
| `POST`   | `/api/books/:id/resplit`         | 重新分段（保留已发布的片段）                          |

### 订阅

| 方法     | 路径                      | 说明                     |
| -------- | ------------------------- | ------------------------ |
| `POST`   | `/api/books/:id/feeds`    | 为书籍创建订阅           |
| `GET`    | `/api/feeds`              | 列出所有订阅及进度       |
| `GET`    | `/api/feeds/:id`          | 订阅详情                 |
| `PATCH`  | `/api/feeds/:id`          | 更新订阅（状态、计划等） |
| `DELETE` | `/api/feeds/:id`          | 删除订阅                 |
| `GET`    | `/api/feeds/:id/releases` | 查看发布时间表           |
| `GET`    | `/api/feeds/:id/preview`  | 预览即将发布的片段       |
| `POST`   | `/api/feeds/:id/advance`  | 立即推进即将发布的片段   |

### 聚合订阅

| 方法     | 路径                                 | 说明             |
| -------- | ------------------------------------ | ---------------- |
| `POST`   | `/api/aggregates`                    | 创建聚合订阅     |
| `GET`    | `/api/aggregates`                    | 列出所有聚合订阅 |
| `GET`    | `/api/aggregates/:id`                | 聚合订阅详情     |
| `PATCH`  | `/api/aggregates/:id`                | 更新聚合订阅     |
| `DELETE` | `/api/aggregates/:id`                | 删除聚合订阅     |
| `POST`   | `/api/aggregates/:id/feeds/:feed_id` | 添加源订阅       |
| `DELETE` | `/api/aggregates/:id/feeds/:feed_id` | 移除源订阅       |

### 操作历史

| 方法     | 路径                | 说明                         |
| -------- | ------------------- | ---------------------------- |
| `GET`    | `/api/history`      | 查看近期操作历史             |
| `POST`   | `/api/history/undo` | 撤销上一步操作               |
| `POST`   | `/api/history/redo` | 重做上一步撤销               |
| `DELETE` | `/api/history`      | 清除历史并永久删除软删除资源 |

### 公开端点

| 方法  | 路径                         | 说明                     |
| ----- | ---------------------------- | ------------------------ |
| `GET` | `/feeds/:slug/atom.xml`      | Atom 订阅                |
| `GET` | `/feeds/:slug/rss.xml`       | RSS 2.0 订阅             |
| `GET` | `/aggregates/:slug/atom.xml` | 聚合 Atom 订阅           |
| `GET` | `/aggregates/:slug/rss.xml`  | 聚合 RSS 订阅            |
| `GET` | `/images/:book_id/:file`     | 书籍图片                 |
| `GET` | `/opml`                      | 导出所有订阅的 OPML 文件 |
| `GET` | `/health`                    | 健康检查                 |

> **认证说明：** 当设置了 `api_token` 且 `public_feeds = false` 时，订阅/OPML/聚合端点需要 `Bearer <token>` 认证头。图片（`/images/`）及 `/health` 始终公开。

### 创建订阅请求体

```json
{
  "words_per_day": 3000,
  "delivery_time": "08:00",
  "skip_days": 96,
  "timezone": "Asia/Shanghai",
  "slug": "my-custom-slug",
  "start_at": "2026-03-01T08:00:00+08:00"
}
```

所有字段均为可选，未填写时使用配置文件中的默认值。

`skip_days` 为 `u8` 位域整数：`MON=1, TUE=2, WED=4, THU=8, FRI=16, SAT=32, SUN=64`。
周末跳过：`32 + 64 = 96`。

## 工作原理

1. **上传** — 解析书籍文件，提取章节（EPUB 遵循阅读顺序，TXT 使用分隔符，Markdown 使用标题）
2. **分段** — 在段落边界处将章节切分为片段，目标每段约 1500 字
3. **排程** — 创建订阅时，依据每日字数预算预先计算所有片段的发布时间戳
4. **服务** — RSS 阅读器拉取订阅端点时，仅返回 `release_at ≤ 当前时间` 的片段
5. **变换** — 发布前，片段经过可配置的处理管线（进度指示器、CSS、导航链接）

无需后台调度器——发布时机在创建时即已计算，每次请求时惰性求值。

## 项目架构

```
inkdrip-core/           核心库：解析、分段、排程、订阅生成
inkdrip-store-sqlite/   SQLite 存储后端
inkdrip-server/         HTTP 服务器（axum）
inkdrip-cli/            命令行工具（clap + reqwest）
```

工作区划分为独立 crate 以实现模块化。存储层基于 trait（`BookStore`），方便未来接入其他后端。

## 支持的格式

| 格式     | 扩展名             | 章节识别方式               |
| -------- | ------------------ | -------------------------- |
| EPUB     | `.epub`            | EPUB spine（阅读顺序）     |
| 纯文本   | `.txt`、`.text`    | `===` 分隔线或多个连续空行 |
| Markdown | `.md`、`.markdown` | `#` 和 `##` 标题           |

## 文档

- [分段算法](./docs/logic/splitting.md) - 详细解释了语义分段策略，如何在保持自然阅读边界的前提下将章节拆分为适合 RSS 投递的片段。
- [变换管线与钩子系统](./docs/logic/pipeline.md) - 介绍了内容变换管线以及如何使用钩子在关键节点运行自定义处理。
- [调度算法](./docs/logic/scheduling.md) - 解释了如何根据订阅的排程配置计算片段的发布时间戳。

## 许可证

[AGPL-3.0](LICENSE)
