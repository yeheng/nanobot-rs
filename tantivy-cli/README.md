# tantivy-cli

A command-line interface (CLI) tool for managing full-text search indexes using the Tantivy search engine.

## Features

- **JSON Document Indexing**: Index arbitrary JSON documents with dynamic schemas
- **Full-Text Search**: BM25 ranking, fuzzy matching, and phrase queries
- **Multi-Index Support**: Create multiple named indexes with different schemas
- **TTL Support**: Automatic document expiration with configurable time-to-live
- **Index Maintenance**: Compaction, rebuild, and health monitoring
- **Pure CLI**: No MCP protocol, direct command-line interaction

## Installation

```bash
# From source
cd tantivy-cli
cargo install --path .

# Or build release binary
cargo build --release
```

## Usage

### Index Management

```bash
# Create an index with schema
tantivy-cli index create --name emails --fields '[{"name":"subject","type":"text"},{"name":"body","type":"text"},{"name":"from","type":"string"}]'

# List all indexes
tantivy-cli index list

# Get index statistics
tantivy-cli index stats --name emails

# Compact an index
tantivy-cli index compact --name emails

# Rebuild an index
tantivy-cli index rebuild --name emails

# Delete an index
tantivy-cli index drop --name emails
```

### Document Operations

```bash
# Add a document
tantivy-cli doc add --index emails --id email-001 --fields '{"subject":"Hello World","body":"This is the email content...","from":"sender@example.com"}'

# Add a document with TTL
tantivy-cli doc add --index emails --id email-002 --fields '{"subject":"Temporary"}' --ttl 7d

# Delete a document
tantivy-cli doc delete --index emails --id email-001

# Commit pending changes
tantivy-cli doc commit --index emails
```

### Search

```bash
# Simple text search
tantivy-cli search --index emails --query '{"text":"hello"}'

# Search with filters
tantivy-cli search --index emails --query '{"text":"hello","filters":[{"field":"from","op":"eq","value":"sender@example.com"}],"limit":10}'

# Search with highlighting
tantivy-cli search --index emails --query '{"text":"hello","highlight":{"highlight_tag":"em"}}'
```

### Maintenance

```bash
# Get maintenance status for all indexes
tantivy-cli maintain status

# Get status for specific index
tantivy-cli maintain status --index emails

# Get job status
tantivy-cli maintain job-status

# Get specific job status
tantivy-cli maintain job-status --job-id abc123
```

### Common Options

```bash
# Specify custom index directory
tantivy-cli --index-dir /path/to/indexes index list

# Set log level
tantivy-cli --log-level debug index list

# Disable automatic maintenance
tantivy-cli --auto-maintain=false index create --name test --fields '[{"name":"title","type":"text"}]'
```

## Field Types

| Type | Description | Indexed | Stored |
|------|-------------|---------|--------|
| `text` | Full-text (tokenized) | Yes | Yes |
| `string` | Exact match only | Yes | Yes |
| `i64` | 64-bit integer | Yes | Yes |
| `f64` | 64-bit float | Yes | Yes |
| `datetime` | ISO 8601 timestamp | Yes | Yes |
| `string_array` | Multiple string values | Yes | Yes |
| `json` | Nested JSON object | No | Yes |

## TTL Format

Time-to-live can be specified with these units:
- `s`, `sec`, `seconds` - Seconds
- `m`, `min`, `minutes` - Minutes
- `h`, `hour`, `hours` - Hours
- `d`, `day`, `days` - Days
- `w`, `week`, `weeks` - Weeks

Examples: `1d`, `7d`, `30d`, `1w`

## Data Storage

Default data directory: `~/.nanobot/tantivy/`

Structure:
```
tantivy/
└── indexes/
    ├── emails/
    │   ├── metadata.json
    │   └── *.tantivy*
    └── documents/
        ├── metadata.json
        └── *.tantivy*
```

## Search Query Format

The search command accepts JSON queries with the following structure:

```json
{
  "text": "hello",
  "filters": [
    {"field": "from", "op": "eq", "value": "sender@example.com"}
  ],
  "limit": 10,
  "offset": 0,
  "highlight": {
    "fields": ["subject", "body"],
    "highlight_tag": "em",
    "num_snippets": 2
  }
}
```

### Filter Operators

| Operator | Description |
|----------|-------------|
| `eq` | Equal |
| `ne` | Not equal |
| `gt` | Greater than |
| `gte` | Greater than or equal |
| `lt` | Less than |
| `lte` | Less than or equal |
| `contains` | Contains |

## Job System

All write operations (add, delete, commit, compact) are executed as background jobs. Each operation returns a job ID that can be used to check status:

```bash
tantivy-cli maintain job-status --job-id 550e8400-e29b-41d4-a716-446655440000
```

## License

MIT License
