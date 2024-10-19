use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct Chunk(pub Vec<Expr>);

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum Expr {
    Nop,
    Ret(Value),
    If(CondExpr, Chunk, Chunk),
    FetchUp(i64, Reg),
    FetchRandLive,
    Set(Vec<Value>, Value),
    Get(Vec<Value>, Reg),
    SetIndex(String, i64, Value),
    GetIndex(String, i64, Reg),
    PrintReg(Reg),
    Print(Vec<Value>),
    PrintIndex(String, i64),
    Extract(Reg, Vec<Value>, Reg),
    Reg(Reg, Value),
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum CondExpr {
    Eq(Reg, Reg),
    IsNum(Reg),
    NumLess(Reg),
    NumGreater(Reg),
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum Reg {
    Named(String),
    Buff(u32),
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::chunkir;

    macro_rules! ast_case {
        ($token:literal) => {{
            let ans = serde_json::from_str::<Chunk>(include_str!(concat!(
                "./test_data/",
                $token,
                ".expect.json"
            )))
            .unwrap_or_else(|e| panic!("read expect.json of {} error: {}", $token, e));
            let out = chunkir::ChunkParser::new()
                .parse(include_str!(concat!("./test_data/", $token, ".in.txt")))
                .unwrap_or_else(|e| panic!("parse error on {}.in.txt: {}", $token, e));
            assert_eq!(out, ans);
        }};
    }

    #[test]
    fn test_parse() {
        ast_case!("chunk_001");
    }
}
