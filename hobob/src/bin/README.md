# hobob/src/bin — 辅助二进制

## `show_expect_value.rs`

- 作用：打印若干手写 `Chunk` AST 示例的 pretty JSON（`=== sample N ===` 分隔），供人工对照 `test_data` 中解析器期望输出。
- 运行：`cargo run -p hobob --bin show_expect_value`
- 无参数、无副作用；修改 `chunk.rs` AST 时可运行它重新生成参照。
