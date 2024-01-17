use anyhow::Result;
use im::{HashMap, OrdSet};
use serde_json::Value;

#[derive(Clone)]
pub enum DNode {
    Plain(Value),
    Dir(HashMap<String, u64>),
    Index(OrdSet<(u64, u64)>),
}
#[derive(Clone)]
pub struct Bench {
    fs: HashMap<u64, DNode>,
}

impl Default for Bench {
    fn default() -> Self {
        let mut fs = HashMap::<u64, DNode>::default();
        fs.insert(0, DNode::Dir(HashMap::default()));
        Self { fs }
    }
}

impl Bench {
    #[must_use]
    pub fn get(&self, path: &Value) -> Option<&DNode> {
        Self::as_slice_path(path)
            .ok()
            .and_then(|p| self.slice_path_get(p))
    }

    /// # Errors
    /// TODO
    pub fn set(&mut self, path: &Value, content: Value) -> Result<()> {
        Self::as_slice_path(path).and_then(|p| self.slice_path_set(p, content))
    }

    #[must_use]
    pub fn pull_log(&self, ack: i64) -> Vec<(u64, Value)> {
        todo!()
    }

    fn slice_path_get(&self, path: &[Value]) -> Option<&DNode> {
        todo!()
    }

    fn slice_path_set(&mut self, path: &[Value], content: Value) -> Result<()> {
        todo!()
    }

    fn as_slice_path(path: &Value) -> Result<&[Value]> {
        todo!()
    }
}
