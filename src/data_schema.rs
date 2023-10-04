use anyhow::{anyhow, Result};
use boon::{Compiler, SchemaIndex, Schemas};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

lazy_static::lazy_static! {
    pub static ref CHAIR_DATA_SCHEMA: Arc<ChairData> = Arc::new(ChairData::new());
}

pub struct ChairData {
    scope: Schemas,
    uri_sid: HashMap<String, SchemaIndex>,
}

struct ChairDataBuilder {
    scope: Schemas,
    uri_sid: HashMap<String, SchemaIndex>,
    compiler: Compiler,
}

impl ChairDataBuilder {
    fn new() -> Self {
        let mut compiler = Compiler::new();
        compiler.set_default_draft(boon::Draft::V2020_12);
        Self {
            scope: Schemas::new(),
            uri_sid: Default::default(),
            compiler,
        }
    }

    fn schema(mut self, uri: &str, value: Value) -> Self {
        self.compiler
            .add_resource(uri, value)
            .map_err(|e| panic!("ChairDataBuilder add_resource error: {}", e))
            .ok();
        self.compiler
            .compile(uri, &mut self.scope)
            .map_err(|e| panic!("ChairDataBuilder compiler error: {}", e))
            .ok()
            .map(|id| self.uri_sid.insert(uri.into(), id));
        self
    }

    fn done(self) -> ChairData {
        ChairData {
            scope: self.scope,
            uri_sid: self.uri_sid,
        }
    }
}

impl ChairData {
    fn new() -> Self {
        ChairDataBuilder::new()
            .schema(schema_uri!("utils/ts"), utils_ts_string())
            .schema(schema_uri!("log"), log_schema())
            .schema(schema_uri!("runtime/bucket"), rt_bucket_schema())
            .schema(schema_uri!("runtime/db"), rt_db_schema())
            .schema(schema_uri!("follow"), follow_schema())
            .schema(schema_uri!("refresh"), refresh_schema())
            .schema(schema_uri!("toggle_group"), toggle_group_schema())
            .done()
    }

    fn expect_impl(&self, id: &str, data: &Value) -> Result<()> {
        let id = self
            .uri_sid
            .get(id)
            .unwrap_or_else(|| panic!("not registered schema: {}", id));
        Ok(self
            .scope
            .validate(data, *id)
            .map_err(|e| anyhow!("boon: {:?}", e))?)
    }

    pub fn expect(id: &str, data: &Value) -> Result<()> {
        CHAIR_DATA_SCHEMA.expect_impl(id, data)
    }
}

fn utils_ts_string() -> Value {
    json!({
        "description": "A string deserializable to chrono timestamp",
        "type": "string",
        "pattern": "^\\d{4}(-\\d\\d){2}T\\d\\d(:\\d\\d){2}\\.\\d{3,}",
    })
}

fn rt_db_schema() -> Value {
    json!({
        "description": "runtime.db schema",
        "type": "object",
        "properties": {
            "dump_time": {
                "$ref": schema_uri!("utils/ts"),
            },
            "dump_timeout_min": {
                "type": "integer",
                "minimum": 1,
            },
            "vlog_dump_gap_sec": {
                "type": "integer",
                "minimum": 1,
            }
        },
        "additionalProperties": false,
    })
}

fn log_schema() -> Value {
    json!({
        "description": "backend logging messages to frontend",
        "type": "object",
        "properties": {
            "ts": {
                "$ref": schema_uri!("utils/ts"),
            },
            "level": {
                "type": "integer",
                "minimum": -9,
                "maximum": 9,
            },
            "msg": { "type": "string", },
        },
        "required": [
            "ts", "level", "msg",
        ],
        "additionalProperties": false,
    })
}

fn rt_bucket_schema() -> Value {
    json!({
        "description": "runtime.bucket schema",
        "type": "object",
        "properties": {
            "atime": {
                "$ref": schema_uri!("utils/ts"),
            },
            "min_gap": { "type": "integer", "minimum": 1, },
            "min_change_gap": { "type": "integer", "minimum": 1, },
            "gap": { "type": "integer", "minimum": 1, },
        },
        "additionalProperties": false,
    })
}

fn follow_schema() -> Value {
    json!({
        "description": "operate follow option schema",
        "type": "object",
        "properties": {
            "uid": { "type": "integer", },
            "enable": { "type": "boolean", },
        },
        "required": [ "uid", ],
        "additionalProperties": false,
    })
}

fn refresh_schema() -> Value {
    json!({
        "description": "operate refresh option schema",
        "type": "object",
        "properties": {
            "uid": { "type": "integer", },
        },
        "required": [ "uid", ],
        "additionalProperties": false,
    })
}

fn toggle_group_schema() -> Value {
    json!({
        "description": "operate toggle/group option schema",
        "type": "object",
        "properties": {
            "uid": { "type": "integer", },
            "gid": { "type": "integer", },
        },
        "required": [ "uid", "gid", ],
        "additionalProperties": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_schema() {
        let val = json!({
            "ts": serde_json::to_value(chrono::Utc::now()).unwrap(),
            "level": 1,
            "msg": "msg from recoverable error level",
        });
        assert!(ChairData::expect(schema_uri!("log"), &val).is_ok());
        let val = json!({
            "ts": "bad-ts",
            "level": 1,
            "msg": "msg from recoverable error level",
        });
        assert!(ChairData::expect(schema_uri!("log"), &val).is_err());
    }
}
