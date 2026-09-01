# hobob/src/test_data — chunk 解析器测试用例

| 文件 | 内容 |
| --- | --- |
| `chunk_001.in.txt` + `chunk_001.expect.json` | 已启用用例：源码 → 期望 AST |
| `chunk_002.in.txt` + `chunk_002.expect.json` | 已启用用例 |
| `chunk_003.in.txt`、`chunk_004.in.txt` | 只有输入、无 expect.json，**未接入测试** |

- 测试入口：`chunk.rs` 的 `test_parse`（`chunkir::ChunkParser` 解析 `.in.txt`，与 `.expect.json` 反序列化结果 `assert_eq!`）。
- 修改解析器/语法时，先更新 expect 或用 `show_expect_value` 生成参照。
