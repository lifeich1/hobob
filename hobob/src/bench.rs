use anyhow::Result;
use im::{HashMap, OrdSet};
use serde_json::Value;

#[derive(Clone)]
pub enum DNode {
    Plain(Value),
    Dir(HashMap<String, u64>),
    Index(OrdSet<(u64, u64)>),
}
#[derive(Default, Clone)]
pub struct Bench {
    fs: HashMap<u64, DNode>,
}

impl Bench {
    pub fn get(&self, path: &Value) -> Option<&DNode> {
        todo!()
    }

    pub fn set(&self, path: &Value, content: Value) -> Result<()> {
        todo!()
    }

    pub fn pull_log(&self, ack: i64) -> Vec<(u64, Value)> {
        todo!()
    }
}
