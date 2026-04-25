# Embedding Search Removal Design

Date: 2026-04-26
Status: Approved

## Background

The embedding search subsystem (`fastembed` + `vector_math.rs`) was built for
semantic routing of tools/skills and history recall. In practice:

- `ToolRegistry::get_top_k()` and `SkillsRegistry::get_top_k()` are **never called** externally (dead code).
- `WikiQueryEngine` already uses pure Tantivy BM25 with no vector dependency.
- `HistoryRecallHook` is the only runtime consumer, executing synchronous embedding computation on every message.
- `fastembed` pulls in an ONNX runtime (~20-100MB model) for a use case better served by keyword search.

## Decision

Remove all local embedding infrastructure. The `local-embedding` feature flag,
`fastembed` dependency, and every file that exclusively serves embedding will be
deleted.

## Scope

### Files Deleted Entirely

| File | Reason |
|------|--------|
| `storage/src/vector_math.rs` | Hand-rolled cosine similarity / top-K for a non-problem |
| `storage/src/embedder.rs` | ONNX-based TextEmbedder wrapping fastembed |
| `engine/src/hooks/history.rs` | HistoryRecallHook (only runtime embedding consumer) |

### Files Modified

| File | Change |
|------|--------|
| `storage/src/lib.rs` | Remove vector_math/embedder module declarations and re-exports |
| `storage/src/search/mod.rs` | Remove vector_math re-exports |
| `storage/src/session_store.rs` | Remove `save_embedding`, `load_embeddings`, `has_embedding` |
| `storage/Cargo.toml` | Remove `fastembed`, `bytemuck`, `parking_lot` deps; remove `local-embedding` feature |
| `engine/src/tools/registry.rs` | Remove `embeddings` field, `get_top_k()`, `initialize_embeddings()` |
| `engine/src/skills/registry.rs` | Remove `embeddings` field, `get_top_k()`, `initialize_embeddings()` |
| `engine/src/search/mod.rs` | Remove vector/embedder re-exports |
| `engine/src/session/context.rs` | Remove `recall_history()` method |
| `engine/src/session/builder.rs` | Remove `local-embedding` cfg branches |
| `engine/src/hooks/mod.rs` | Remove `HistoryRecallHook` export |
| `engine/src/lib.rs` | Remove vector/embedder re-exports and cfg gates |
| `engine/src/config/` | Remove `EmbeddingConfig` references |
| `engine/Cargo.toml` | Remove `local-embedding` feature flag |
| `cli/Cargo.toml` | Remove `local-embedding` feature flag |

### Execution Order (6 steps)

1. **T6**: Remove embedding code from `ToolRegistry` and `SkillsRegistry`
2. Remove `HistoryRecallHook` and `AgentContext::recall_history()`
3. Clean up re-export chains (`search/mod.rs`, `lib.rs` in both crates)
4. **T10**: Delete `vector_math.rs`
5. **T7**: Delete `embedder.rs`, remove `fastembed`/`bytemuck`/`parking_lot` from `storage/Cargo.toml`
6. **T9**: Remove `local-embedding` feature flag from all `Cargo.toml` files and config types

### Verification

- `cargo build --release --workspace` compiles without errors
- `cargo test --workspace` passes (embedding-dependent tests removed)
- No `local-embedding` feature flag references remain
- Binary size reduced by removing ONNX runtime dependency

### Not In Scope

- Remote embedding API replacement (can be added later if needed)
- Wiki query engine changes (already pure Tantivy BM25)
- Any new functionality
