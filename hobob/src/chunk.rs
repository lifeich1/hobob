use serde_json::Value;

pub struct Chunk(pub Vec<Expr>);

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
    CondExpr(CondExpr),
    Reg(Reg, Value),
}

pub enum CondExpr {
    Eq(Reg, Reg),
    IsNum(Reg),
    NumLess(Reg),
    NumGreater(Reg),
}

pub struct Reg(String);
