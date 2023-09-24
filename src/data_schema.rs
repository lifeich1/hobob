use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::sync::Mutex;
use url::Url;
use valico::json_schema::scope::Scope;
use valico::json_schema::SchemaVersion;

lazy_static::lazy_static! {
    pub static ref CHAIR_DATA_SCHEMA: Mutex<ChairData> = Mutex::new(ChairData::new());
}

pub struct ChairData {
    scope: Scope,
}

impl ChairData {
    fn new() -> Self {
        let this = Self {
            scope: Scope::new().set_version(SchemaVersion::Draft2019_09),
        };
        this.schema(schema_uri!("log"), log_schema())
    }

    fn schema(mut self, id: &str, schema: Value) -> Self {
        self.scope
            .compile_with_id(&Url::parse(id).expect("valid uri"), schema, true)
            .expect("mush be valid schema");
        self
    }

    fn expect_impl(&self, id: &str, data: &Value) -> Result<()> {
        let result = self
            .scope
            .resolve(&Url::parse(id).expect("valid uri"))
            .expect("registered schema")
            .validate(data);
        if result.is_valid() {
            Ok(())
        } else {
            Err(anyhow!("Not pass schema {}: {:?}", id, result))
        }
    }

    pub fn expect(id: &str, data: &Value) -> Result<()> {
        CHAIR_DATA_SCHEMA
            .lock()
            .expect("mutex crack, must reboot")
            .expect_impl(id, data)
    }
}

fn log_schema() -> Value {
    json!({
        "description": "backend logging messages to frontend",
        "type": "object",
        "properties": {
            "ts": {
                "type": "string",
                "pattern": "^\\d{4}(-\\d\\d){2}T\\d\\d(:\\d\\d){2}\\.\\d{3,}",
            },
            "level": {
                "type": "integer",
                "minimum": -9,
                "maximum": 9,
            },
            "msg": {
                "type": "string",
            },
        },
        "required": [
            "ts", "level", "msg",
        ],
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
