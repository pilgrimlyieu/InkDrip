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

以上为创建新订阅源时的默认值，可通过 API 按订阅源覆盖。

## 算法

调度器采用**贪心预算分配**策略：
1. 初始化 `current_date` 为订阅源的 `start_at` 日期，`daily_remaining` 为 `words_per_day`。
2. 按顺序遍历每个片段：
   - 若片段的 `word_count` 超过 `daily_remaining` **且**当日已消耗部分预算，则前进到下一个有效日期并重置预算。
   - 赋值 `release_at = current_date + delivery_time`（使用配置的时区）。
   - 从 `daily_remaining` 中减去片段的 `word_count`。
3. 前进日期时，跳过 `skip_days` 中指定的星期。

### 关键行为

- **单日多片段**：只要累计字数不超过预算，一天可容纳多个片段。短片段会自然聚集在同一天。
- **超大片段**：超过 `words_per_day` 的单个片段会被分配到独立的一天——调度时不会进一步拆分。
- **跳过日**：支持跳过周末或任意星期组合。调度器在寻找下一个有效日期时会跳过所有标记的日子。

## 发布时间

分配到同一日期的所有片段共享相同的 `release_at` 时间戳——即配置的 `delivery_time`（使用配置的时区）。RSS 阅读器在该时间后拉取即可看到新片段。

## 重新调度

当订阅源配置变更（如修改 `words_per_day` 或 `skip_days`）时，调度器重新计算所有未来发布时间。已发布的片段不会被回退。实现方式：
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
    skip_days
  }
        │
        ▼
  compute_release_schedule(segments, config)
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
