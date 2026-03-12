//! Tool registration for tantivy-mcp.
//!
//! All tools directly call IndexManager methods without any async wrapper overhead.
//! The IndexManager handles its own internal locking via DashMap and per-index RwLock.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::index::{Document, FieldDef, IndexConfig, IndexManager, SearchQuery};
use crate::mcp::{McpTool, ToolRegistry, ToolResult};
use crate::Result;

/// Register all MCP tools.
pub fn register_tools(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    // Index management tools
    register_index_create(registry, manager.clone());
    register_index_drop(registry, manager.clone());
    register_index_list(registry, manager.clone());
    register_index_stats(registry, manager.clone());

    // Document tools
    register_document_add(registry, manager.clone());
    register_document_delete(registry, manager.clone());
    register_document_commit(registry, manager.clone());

    // Search tool
    register_search(registry, manager.clone());

    // Maintenance tools
    register_index_compact(registry, manager.clone());
    register_index_rebuild(registry, manager.clone());
    register_maintenance_status(registry, manager.clone());
}

fn register_index_create(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "index_create".to_string(),
            description: "Create a new index with a custom schema".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Index name"
                    },
                    "fields": {
                        "type": "array",
                        "description": "Field definitions",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "type": {
                                    "type": "string",
                                    "enum": ["text", "string", "i64", "f64", "datetime", "string_array", "json"]
                                },
                                "indexed": { "type": "boolean", "default": true },
                                "stored": { "type": "boolean", "default": true }
                            },
                            "required": ["name", "type"]
                        }
                    },
                    "default_ttl": {
                        "type": "string",
                        "description": "Default TTL for documents (e.g., '7d', '30d')"
                    }
                },
                "required": ["name", "fields"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_index_create(params, manager)
        },
    );
}

fn handle_index_create(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;
    let name = params["name"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing name".to_string()))?
        .to_string();

    let fields: Vec<FieldDef> = serde_json::from_value(params["fields"].clone())?;

    let config = params
        .get("default_ttl")
        .and_then(|v| v.as_str())
        .map(|ttl| IndexConfig {
            default_ttl: Some(ttl.to_string()),
            auto_compact: None,
        });

    let schema = manager.create_index(&name, fields, config)?;

    Ok(ToolResult::text(
        json!({
            "success": true,
            "index": schema.name,
            "fields": schema.fields.len(),
            "created_at": schema.created_at
        })
        .to_string(),
    ))
}

fn register_index_drop(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "index_drop".to_string(),
            description: "Delete an index and all its data".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Index name to delete"
                    }
                },
                "required": ["name"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_index_drop(params, manager)
        },
    );
}

fn handle_index_drop(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;
    let name = params["name"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing name".to_string()))?;

    manager.drop_index(name)?;

    Ok(ToolResult::text(
        json!({
            "success": true,
            "message": format!("Index '{}' deleted", name)
        })
        .to_string(),
    ))
}

fn register_index_list(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "index_list".to_string(),
            description: "List all indexes with their schemas".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        move |_params| {
            let manager = manager.clone();
            handle_index_list(manager)
        },
    );
}

fn handle_index_list(manager: Arc<IndexManager>) -> Result<ToolResult> {
    let indexes = manager.list_indexes();

    Ok(ToolResult::text(
        json!({
            "indexes": indexes
        })
        .to_string(),
    ))
}

fn register_index_stats(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "index_stats".to_string(),
            description: "Get statistics and health status for an index".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Index name (optional, returns all if not specified)"
                    }
                }
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_index_stats(params, manager)
        },
    );
}

fn handle_index_stats(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let name = params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str());

    if let Some(index_name) = name {
        let stats = manager.get_stats(index_name)?;
        Ok(ToolResult::text(json!(stats).to_string()))
    } else {
        let indexes = manager.list_indexes();
        let mut all_stats = Vec::new();
        for index_name in indexes {
            if let Ok(stats) = manager.get_stats(&index_name) {
                all_stats.push(stats);
            }
        }
        Ok(ToolResult::text(
            json!({ "indexes": all_stats }).to_string(),
        ))
    }
}

fn register_document_add(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "document_add".to_string(),
            description: "Add or update a document in an index".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "string",
                        "description": "Index name"
                    },
                    "id": {
                        "type": "string",
                        "description": "Unique document ID"
                    },
                    "fields": {
                        "type": "object",
                        "description": "Field values"
                    },
                    "ttl": {
                        "type": "string",
                        "description": "Optional TTL override (e.g., '1d', '7d')"
                    }
                },
                "required": ["index", "id", "fields"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_document_add(params, manager)
        },
    );
}

fn handle_document_add(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;

    let index_name = params["index"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing index".to_string()))?;

    let doc_id = params["id"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing id".to_string()))?
        .to_string();

    let fields: Map<String, Value> = serde_json::from_value(params["fields"].clone())?;

    let mut doc = Document::new(doc_id.clone(), fields);

    // Handle TTL
    if let Some(ttl_str) = params.get("ttl").and_then(|v| v.as_str()) {
        let ttl = parse_ttl(ttl_str)?;
        let expires_at = chrono::Utc::now() + ttl;
        doc = doc.with_expiry(expires_at);
    }

    manager.add_document(index_name, doc)?;

    Ok(ToolResult::text(
        json!({
            "success": true,
            "id": doc_id
        })
        .to_string(),
    ))
}

fn register_document_delete(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "document_delete".to_string(),
            description: "Delete a document from an index".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "string",
                        "description": "Index name"
                    },
                    "id": {
                        "type": "string",
                        "description": "Document ID to delete"
                    }
                },
                "required": ["index", "id"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_document_delete(params, manager)
        },
    );
}

fn handle_document_delete(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;

    let index_name = params["index"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing index".to_string()))?;

    let doc_id = params["id"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing id".to_string()))?;

    manager.delete_document(index_name, doc_id)?;

    Ok(ToolResult::text(
        json!({
            "success": true,
            "message": format!("Document '{}' deleted", doc_id)
        })
        .to_string(),
    ))
}

fn register_document_commit(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "document_commit".to_string(),
            description: "Commit pending changes to an index".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "string",
                        "description": "Index name"
                    }
                },
                "required": ["index"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_document_commit(params, manager)
        },
    );
}

fn handle_document_commit(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;

    let index_name = params["index"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing index".to_string()))?;

    manager.commit(index_name)?;

    Ok(ToolResult::text(
        json!({
            "success": true,
            "message": format!("Index '{}' committed", index_name)
        })
        .to_string(),
    ))
}

fn register_search(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "search".to_string(),
            description: "Search for documents in an index".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "string",
                        "description": "Index name"
                    },
                    "query": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" },
                            "filters": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "field": { "type": "string" },
                                        "op": { "type": "string", "enum": ["eq", "ne", "gt", "gte", "lt", "lte", "contains"] },
                                        "value": {}
                                    }
                                }
                            },
                            "limit": { "type": "integer", "default": 10 },
                            "offset": { "type": "integer", "default": 0 }
                        }
                    }
                },
                "required": ["index"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_search(params, manager)
        },
    );
}

fn handle_search(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;

    let index_name = params["index"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing index".to_string()))?;

    let query: SearchQuery = if let Some(query_obj) = params.get("query") {
        serde_json::from_value(query_obj.clone())?
    } else {
        SearchQuery::default()
    };

    let results = manager.search(index_name, &query)?;

    Ok(ToolResult::text(
        json!({
            "results": results,
            "count": results.len()
        })
        .to_string(),
    ))
}

fn register_index_compact(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "index_compact".to_string(),
            description: "Compact an index to optimize storage and query performance".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "string",
                        "description": "Index name"
                    }
                },
                "required": ["index"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_index_compact(params, manager)
        },
    );
}

fn handle_index_compact(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;

    let index_name = params["index"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing index".to_string()))?;

    let stats_before = manager.get_stats(index_name)?;
    manager.compact(index_name)?;
    let stats_after = manager.get_stats(index_name)?;

    Ok(ToolResult::text(
        json!({
            "success": true,
            "segments_before": stats_before.segment_count,
            "segments_after": stats_after.segment_count,
            "deleted_before": stats_before.deleted_count,
            "deleted_after": stats_after.deleted_count,
            "bytes_saved": stats_before.size_bytes.saturating_sub(stats_after.size_bytes)
        })
        .to_string(),
    ))
}

fn register_index_rebuild(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "index_rebuild".to_string(),
            description: "Rebuild an index with optional new schema for migration".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "string",
                        "description": "Index name"
                    },
                    "fields": {
                        "type": "array",
                        "description": "Optional new field definitions for schema migration",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "type": {
                                    "type": "string",
                                    "enum": ["text", "string", "i64", "f64", "datetime", "string_array", "json"]
                                },
                                "indexed": { "type": "boolean", "default": true },
                                "stored": { "type": "boolean", "default": true }
                            },
                            "required": ["name", "type"]
                        }
                    },
                    "batch_size": {
                        "type": "integer",
                        "description": "Number of documents to process per batch",
                        "default": 1000
                    }
                },
                "required": ["index"]
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_index_rebuild(params, manager)
        },
    );
}

fn handle_index_rebuild(params: Option<Value>, manager: Arc<IndexManager>) -> Result<ToolResult> {
    use crate::maintenance::rebuild_index;
    use crate::maintenance::RebuildResult;

    let params = params.ok_or_else(|| crate::Error::McpError("Missing params".to_string()))?;

    let index_name = params["index"]
        .as_str()
        .ok_or_else(|| crate::Error::McpError("Missing index".to_string()))?;

    let new_fields: Option<Vec<crate::index::FieldDef>> = if let Some(fields) = params.get("fields")
    {
        Some(serde_json::from_value(fields.clone())?)
    } else {
        None
    };

    let batch_size = params
        .get("batch_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as usize;

    let result: RebuildResult = rebuild_index(&manager, index_name, new_fields, batch_size)?;
    Ok(ToolResult::text(json!(result).to_string()))
}

/// Parse a TTL string into a duration.
fn parse_ttl(ttl: &str) -> Result<chrono::Duration> {
    let ttl = ttl.trim();

    if ttl.is_empty() {
        return Err(crate::Error::ParseError("Empty TTL".to_string()));
    }

    let numeric_end = ttl.find(|c: char| !c.is_ascii_digit()).unwrap_or(ttl.len());

    if numeric_end == 0 {
        return Err(crate::Error::ParseError(format!("Invalid TTL: {}", ttl)));
    }

    let number: i64 = ttl[..numeric_end]
        .parse()
        .map_err(|_| crate::Error::ParseError(format!("Invalid TTL number: {}", ttl)))?;

    let unit = &ttl[numeric_end..];

    let duration = match unit {
        "s" | "sec" | "seconds" => chrono::Duration::seconds(number),
        "m" | "min" | "minutes" => chrono::Duration::minutes(number),
        "h" | "hour" | "hours" => chrono::Duration::hours(number),
        "d" | "day" | "days" => chrono::Duration::days(number),
        "w" | "week" | "weeks" => chrono::Duration::weeks(number),
        _ => {
            return Err(crate::Error::ParseError(format!(
                "Unknown TTL unit: {}",
                unit
            )))
        }
    };

    Ok(duration)
}

fn register_maintenance_status(registry: &mut ToolRegistry, manager: Arc<IndexManager>) {
    registry.register(
        McpTool {
            name: "maintenance_status".to_string(),
            description: "Get maintenance status for all indexes or a specific index".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": {
                        "type": "string",
                        "description": "Optional index name (returns all if not specified)"
                    }
                }
            }),
        },
        move |params| {
            let manager = manager.clone();
            handle_maintenance_status(params, manager)
        },
    );
}

fn handle_maintenance_status(
    params: Option<Value>,
    manager: Arc<IndexManager>,
) -> Result<ToolResult> {
    let index_name = params
        .as_ref()
        .and_then(|p| p.get("index"))
        .and_then(|v| v.as_str());

    if let Some(name) = index_name {
        let stats = manager.get_stats(name)?;
        let config_opt = manager.get_config(name)?;

        Ok(ToolResult::text(
            json!({
                "index": name,
                "stats": stats,
                "config": config_opt
            })
            .to_string(),
        ))
    } else {
        let indexes = manager.list_indexes();
        let mut all_status = Vec::new();

        for idx in indexes {
            if let Ok(stats) = manager.get_stats(&idx) {
                let config_opt = manager.get_config(&idx).ok().flatten();
                all_status.push(json!({
                    "index": idx,
                    "stats": stats,
                    "config": config_opt
                }));
            }
        }

        Ok(ToolResult::text(
            json!({
                "indexes": all_status
            })
            .to_string(),
        ))
    }
}
