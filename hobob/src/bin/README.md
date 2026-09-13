# hobob/src/bin — 辅助二进制

## `perf_smoke.rs`

- 作用：M1 §7 性能冒烟——模拟 200 up × 连续 10 轮 fetch 突发，走真实 hub/持久化路径
  （chair apply → hub run → `persist_diff` 直写 + stage），采集六项指标。**不联网**：fetch 数据用内嵌 JSON mock。
- 运行（release）：`cargo run --release -p hobob --bin perf_smoke`
- 输出指标由人工回填 `.plans/m1-ecs-core.md` §7 指标表与「实施记录」。

> `show_expect_value.rs`（打印 `Chunk` AST 示例 JSON）已随 M3 T1 移除 chunk/lalrpop 一并删除。
