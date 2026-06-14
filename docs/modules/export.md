# Export Module

Export functionality for conversations and memories to various file formats.

## Design

- **`formats.rs`** — Export format definitions and serialization
  - `ExportFormat` — Supported formats: Markdown, JSON, JSONL
  - `JsonLineMemory` — JSONL memory record format
  - `JsonLineMessage` — JSONL message record format
- **`service.rs`** — `ExportService` with import/export operations
  - `ExportOptions` — Format, filtering, and destination options
  - `ExportStats` — Export operation statistics
  - `ImportOptions` — Skip, update, dry-run semantics
  - `ImportStats` — Import operation statistics

### Supported Formats

| Format | Extension | Use Case |
|--------|-----------|----------|
| Markdown | `.md` | Human-readable transcripts |
| JSON | `.json` | Structured data with full metadata |
| JSONL | `.jsonl` | Line-delimited for streaming processing |

## Key Types

```rust
pub enum ExportFormat {
    Markdown,
    Json,
    JsonLines,
}

pub struct ExportOptions {
    pub format: ExportFormat,
    pub destination: PathBuf,
    pub filter: Option<ExportFilter>,
    pub include_metadata: bool,
}

pub struct ExportStats {
    pub memories_exported: usize,
    pub conversations_exported: usize,
    pub messages_exported: usize,
    pub bytes_written: usize,
}

pub struct ImportOptions {
    pub skip_existing: bool,
    pub update_existing: bool,
    pub dry_run: bool,
    pub validate: bool,
}

pub struct ImportStats {
    pub memories_imported: usize,
    pub conversations_imported: usize,
    pub skipped: usize,
    pub updated: usize,
    pub errors: usize,
}
```

## Implemented Features

- Export to Markdown, JSON, and JSONL formats
- Import with skip/update/dry-run semantics
- Memory export with full metadata
- Conversation export with message history
- Import validation before insertion
- Export statistics tracking
- Filtered export by date range, user, or conversation

