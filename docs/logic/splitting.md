<div align="right">

**[简体中文](splitting.zh-CN.md)** | **[English](splitting.md)**

</div>

# Splitting Algorithm

InkDrip's semantic splitter breaks books into RSS-sized segments while preserving natural reading boundaries. The core implementation lives in [`inkdrip-core/src/splitter/semantic.rs`](/inkdrip-core/src/splitter/semantic.rs).

## Configuration

| Parameter    | Config Key                      | Default | Description                           |
| ------------ | ------------------------------- | ------- | ------------------------------------- |
| Target words | `defaults.target_segment_words` | 1500    | Ideal segment size                    |
| Max words    | `defaults.max_segment_words`    | 2000    | Hard upper bound                      |
| Min words    | `defaults.min_segment_words`    | 500     | Merge threshold for trailing segments |

## Four-Level Strategy

The splitter operates as a cascade — each level is a fallback when the previous level produces chunks that are too large.

### Level 1: Chapter Boundary

If a chapter's word count ≤ `max_words`, the entire chapter becomes one segment. No further splitting is needed.

### Level 2: Paragraph Boundary

For chapters exceeding `max_words`, the splitter walks through HTML block elements (`<p>`, `<div>`, `<blockquote>`, `<h1>`–`<h6>`, `<ul>`, `<ol>`, `<pre>`, `<table>`, `<section>`, `<article>`, `<main>`). It accumulates paragraphs into a buffer and flushes when the current total is closer to `target_words` than it would be after adding the next paragraph — i.e., the split point that minimises the distance to the target is chosen. This means a segment may slightly exceed `target_words` when doing so produces a more balanced split.

**Container Peeling:** If a block element like `<div>` contains nested block elements (e.g., `<div><p>A</p><p>B</p></div>`), the outer container is "peeled" and the inner blocks are extracted for individual processing. This is critical for EPUB content where chapters are often wrapped in container divs.

### Level 3: Sentence Boundary

When a single paragraph exceeds `max_words`, the splitter falls back to sentence-level splitting. The same "closer to target" heuristic from Level 2 applies here — the splitter flushes at the sentence boundary that minimises distance to `target_words`. Sentences are detected by scanning for terminal punctuation:
- **CJK terminals:** `。！？`
- **Latin terminals:** `.!?` (only when not preceded by abbreviation patterns like `Mr.`, `Dr.`, `e.g.`)
- **Closing punctuation:** After a terminal mark, any immediately following closing punctuation (`」』）】〕｝〉》›»\u{201D}` etc.) is consumed and attached to the same sentence, preventing orphaned quote marks at segment boundaries.

### Level 4: Word Boundary (Hard Split)

As a last resort for extremely long sentences (e.g., machine-generated text without punctuation), the splitter performs a hard word-count split. It walks through HTML-stripped text word by word and cuts at the boundary nearest to `target_words`.

## Trailing Segment Merge

After all chapters are processed, the splitter checks whether the final segment is below `min_words`. If so, it merges the trailing segment into its predecessor by concatenating the HTML content and recalculating word counts. This prevents tiny "leftover" segments that provide a poor reading experience.

## Title Context

Each segment carries a `title_context` string for RSS item titles:
- Single-segment chapter: `"Chapter 3"`
- Multi-segment chapter: `"Chapter 3 (1/4)"`, `"Chapter 3 (2/4)"`, etc.

## Data Flow

```
Book file  →  Parser (epub/markdown/txt)  →  Vec<Chapter>
                                                │
                                                ▼
                                          SemanticSplitter
                                                │
                                    ┌───────────┼───────────┐
                                    ▼           ▼           ▼
                              short chapter  paragraphs  sentences
                              (single seg)   (L2 split)  (L3 split)
                                    │           │           │
                                    └─────┬─────┘           │
                                          ▼                 ▼
                                   merge trailing    hard word split
                                          │               (L4)
                                          ▼
                                    Vec<Segment>  →  Database
```
