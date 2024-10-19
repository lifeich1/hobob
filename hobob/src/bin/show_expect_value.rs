use hobob::chunk::*;
use serde_json::json;

fn output(c: Chunk, ind: &mut i32) {
    let s = serde_json::to_string_pretty(&c).unwrap_or_else(|e| panic!("format string error: {e}"));
    if *ind > 0 {
        println!("");
    }
    *ind += 1;
    println!("=== sample {ind} ===");
    println!("{s}");
    println!("=== end ===");
}

fn main() {
    let c = Chunk(vec![Expr::Ret(json!(0))]);
    let mut i = 0;
    output(c, &mut i);
}
