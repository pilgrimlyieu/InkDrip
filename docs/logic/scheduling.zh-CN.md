<div align="right">

**[简体中文](scheduling.zh-CN.md)** | **[English](scheduling.md)**

</div>

# 调度算法

InkDrip 的调度器为订阅源中的所有片段计算发布时间戳，根据每日字数预算将片段分配到各天。实现位于 [`inkdrip-core/src/scheduler.rs`](/inkdrip-core/src/scheduler.rs)。

## 配置项

| 参数     | 配置键                   | 默认值            | 说明                                        |
| -------- | ------------------------ | ----------------- | ------------------------------------------- |
| 每日字数 | `defaults.words_per_day` | 3000              | 每日字数预算                                |
| 投递时间 | `defaults.delivery_time` | `"08:00"`         | 固定每日投递时间（HH:MM）                   |
| 时区     | `defaults.timezone`      | `"Asia/Shanghai"` | IANA 时区名或 `UTC±N`                       |
| 跳过天数 | `defaults.skip_days`     | `[]`              | 跳过的星期几（如 `["saturday", "sunday"]`） |
| 预算模式 | `defaults.budget_mode`   | `"strict"`        | 预算执行模式：`"strict"` 或 `"flexible"`    |

以上为创建新订阅源时的默认值，可通过 API 按订阅源覆盖。每个订阅源在创建时**快照**这些配置——之后修改 `[defaults]` 不会影响已有订阅源。

## 算法

调度器采用**贪心预算分配**策略：
1. 初始化 `current_date` 为订阅源的 `start_at` 日期，`daily_used` 为 0。
2. 按顺序遍历每个片段：
   - 根据 `budget_mode` 决定是否前进到下一天：
     - **Strict 模式**：若加入该片段会超过 `words_per_day`（且当日已有内容），则前进到下一天。
     - **Flexible 模式**：若加入该片段会使当日总字数*远离* `words_per_day`，则前进到下一天。该模式使用「更接近目标」的启发式（与分割器相同），允许可控的超调以使每日总字数更接近预算。
   - 赋值 `release_at = current_date + delivery_time`（使用配置的时区）。
   - 将片段的 `word_count` 加到 `daily_used`。
3. 前进日期时，跳过 `skip_days` 中指定的星期。

### 预算模式

| 模式       | 行为                                                                             |
| ---------- | -------------------------------------------------------------------------------- |
| `strict`   | 严格不超过 `words_per_day`。若加入片段会导致超出预算，则将其推迟到下一天。       |
| `flexible` | 若加入片段能使当日总字数更接近 `words_per_day`，则允许加入，即使会略微超出预算。 |

**示例**：假设 `words_per_day = 3000`，有两个片段分别为 1550 和 1480 字（总计 3030）：
- **Strict 模式**：仅第一个片段（1550）被安排在第 1 天；第二个（1480）推到第 2 天。
- **Flexible 模式**：两个片段都安排在第 1 天，因为 3030 比单独的 1550 更接近 3000。

### 关键行为

- **单日多片段**：只要预算模式允许，一天可容纳多个片段。短片段会自然聚集在同一天。
- **超大片段**：超过 `words_per_day` 的单个片段会被分配到独立的一天——调度时不会进一步拆分。
- **跳过日**：支持跳过周末或任意星期组合。调度器在寻找下一个有效日期时会跳过所有标记的日子。
- **同日排序（错开）**：分配到同一天的片段按阅读顺序错开发布时间。设有 N 个片段同日发布，第 k 个片段（0-indexed）的 `release_at` = `delivery_time − (N − 1 − k) 秒`。因此最后一个片段正好在 `delivery_time` 发布，前面的片段依次提前一秒。RSS 阅读器按时间降序展示时，阅读顺序即为从上到下。

## 发布时间

分配到同一日期的所有片段共享相同的 `release_at` 时间戳——即配置的 `delivery_time`（使用配置的时区）。RSS 阅读器在该时间后拉取即可看到新片段。

## RSS 订阅源限制

RSS 订阅源限制为最近的 N 个片段（可通过 `feed.items_limit` 配置，默认 50），以保持订阅源大小可控。片段按 `release_at` 降序排列，确保：
- 订阅源始终显示**最新发布的**片段
- `updated`（Atom）和 `lastBuildDate`（RSS）时间戳反映最新内容
- 即使总片段数超过限制，RSS 阅读器仍能检测到新发布

这意味着：
- 片段 1-50：全部在订阅源中可见
- 片段 51 发布：订阅源显示片段 2-51（片段 1 被移出）
- 订阅源 `updated` 时间戳随每次新发布更新

已消费早期片段的阅读器会将其缓存，因此这仅影响在多个片段发布后加入的新订阅者。

## 重新调度

当订阅源配置变更（如修改 `words_per_day`、`skip_days` 或 `budget_mode`）时，调度器重新计算所有未来发布时间。已发布的片段不会被回退。实现方式：
1. 收集该书的所有片段。
2. 使用新配置重新计算完整调度。
3. 仅更新尚未发布的片段的 `release_at`。

## 预估

辅助函数 `estimate_days(total_words, words_per_day)` 提供粗略天数：`ceil(total_words / words_per_day)`。不计入跳过日，仅用于 UI 展示。

## 数据流

```
创建订阅源请求
        │
        ▼
  ScheduleConfig {
    start_at, words_per_day,
    delivery_time, timezone,
    skip_days, budget_mode
  }
        │
        ▼
  compute_release_schedule(segments, config, feed_id)
        │
        ▼
  Vec<SegmentRelease> {
    segment_id, feed_id, release_at
  }
        │
        ▼
  存入数据库
        │
        ▼
  serve_feed() 查询: release_at ≤ now
```
