use anyhow::{bail, Context, Result};
use im::{HashMap, OrdSet};
use serde_json::{json, Value};

#[derive(Clone)]
pub enum DNode {
    Plain(Value),
    Dir(HashMap<String, u64>),
    Index(OrdSet<(u64, u64)>),
}
#[derive(Clone)]
pub struct Bench {
    fs: HashMap<u64, DNode>,
    next_id: u64,
}

impl Default for Bench {
    fn default() -> Self {
        let mut fs = HashMap::<u64, DNode>::default();
        fs.insert(0, DNode::Dir(HashMap::default()));
        let mut obj = Self { fs, next_id: 1 };
        obj.set_value(&json!(["log", 0]), json!("genesis moment"))
            .expect("Bench init log file crash");
        obj
    }
}

impl Bench {
    #[must_use]
    pub fn get(&self, path: &Value) -> Option<&DNode> {
        Self::as_slice_path(path)
            .ok()
            .and_then(|p| self.slice_path_get(&p))
    }

    /// # Errors
    /// Path invalid or replace node failed.
    pub fn set(&mut self, path: &Value, content: DNode) -> Result<()> {
        Self::as_slice_path(path).and_then(|p| self.slice_path_set(&p, content))
    }

    /// # Errors
    /// Path invalid or replace node failed.
    pub fn set_value(&mut self, path: &Value, content: Value) -> Result<()> {
        self.set(path, DNode::Plain(content))
    }

    #[must_use]
    pub fn pull_log(&self, ack: i64) -> Vec<(u64, Value)> {
        todo!()
    }

    fn slice_path_get_id(&self, path: &[&Value]) -> Result<u64> {
        if path.is_empty() {
            bail!("path is empty");
        }
        let mut id: u64 = 0;
        let mut frags = 0;
        for p in path {
            let Some(node) = self.fs.get(&id) else {
                bail!("path {:?} not exists", &path[0..frags])
            };
            frags += 1;
            match (node, p) {
                (DNode::Dir(d), Value::String(s)) => match d.get(s) {
                    Some(v) => id = *v,
                    None => bail!("path {:?} not exists", &path[0..frags]),
                },
                (DNode::Index(ls), Value::Number(n)) => {
                    id = match n
                        .as_u64()
                        .or_else(|| {
                            n.as_i64()
                                .and_then(|x| usize::try_from(-x).ok())
                                .map(|x| ls.len().wrapping_sub(x))
                                .map(u64::try_from)
                                .and_then(Result::ok)
                        })
                        .and_then(|x| ls.get_next(&(x, 0)).filter(|t| t.0 == x))
                        .map(|t| t.1)
                    {
                        Some(v) => v,
                        None => bail!("path {:?} not exists", &path[0..frags]),
                    }
                }
                _ => bail!("path fragment {:?} type mismatch", path[frags - 1]),
            }
        }
        Ok(id)
    }

    fn slice_path_get(&self, path: &[&Value]) -> Option<&DNode> {
        let Ok(id) = self.slice_path_get_id(path) else {
            return None;
        };
        self.fs.get(&id)
    }

    fn slice_path_set(&mut self, path: &[&Value], content: DNode) -> Result<()> {
        if path.is_empty() {
            bail!("path is empty");
        }
        let id = if path.len() > 1 {
            self.slice_path_get_id(&path[0..path.len() - 1])?
        } else {
            0
        };
        let p = path[path.len() - 1];
        let Some(n) = self.fs.get_mut(&id) else {
            bail!("id {id} ref but not in fs");
        };
        match (n, p) {
            (DNode::Dir(dir), Value::String(s)) => {
                if let Some(rm) = dir.insert(s.into(), self.next_id) {
                    self.rm_tree(rm).context("failed remove old tree")?;
                }
            }
            (DNode::Index(ind), Value::Number(n)) => {
                let Some(key) = n.as_u64() else {
                    bail!("set with negative index key");
                };
                let mut rm_ids: Vec<u64> = Vec::new();
                while let Some(rm) = ind.get_next(&(key, 0)).filter(|t| t.0 == key).copied() {
                    ind.remove(&rm);
                    rm_ids.push(rm.1);
                }
                ind.insert((key, self.next_id));
                for rm in rm_ids {
                    self.rm_tree(rm)?;
                }
            }
            _ => bail!("path {:?} type not match", &path[0..path.len() - 1]),
        }
        self.fs.insert(self.next_id, content);
        self.next_id += 1;
        Ok(())
    }

    fn as_slice_path(path: &Value) -> Result<Vec<&Value>> {
        match path {
            &Value::String(_) => Ok(vec![path]),
            Value::Array(a) => Ok(a.iter().by_ref().collect()),
            _ => anyhow::bail!("path type neither string nor array, parse error: {path:?}"),
        }
    }

    fn rm_tree(&mut self, id: u64) -> Result<()> {
        todo!()
    }
}
