use serde_json::{json, Value};
use url::Url;
use valico::json_schema::scope::Scope;
use valico::json_schema::SchemaVersion;

pub struct ChairData {
    scope: Scope,
}

impl ChairData {
    pub fn new() -> Self {
        let mut scope = Scope::new().set_version(SchemaVersion::Draft2019_09);
        scope
            .compile_with_id(
                &Url::parse("http://lintd.xyz/hobob/log").unwrap(),
                log_schema(),
                true,
            )
            .expect("must be valid schema");
        Self { scope }
    }

    pub fn expect_log(&self, data: &Value) {
        let res = self
            .scope
            .resolve(&Url::parse("http://lintd.xyz/hobob/log").unwrap())
            .unwrap()
            .validate(data);
        if !res.is_valid() {
            panic!("expect_log invalid: data: {:?}\nerr: {:?}", data, res);
        }
    }
}

fn log_schema() -> Value {
    // TODO
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;

    // FIXME enable when ok
    // #[test]
    fn test_log_schema() {
        let validator = ChairData::new();
        validator.expect_log(&json!({
            "ts": "2023-09-20 22:16:33 +0800",
            "level": 0,
            "msg": "msg from recoverable error level",
        }));
    }
}
