# .agents/skills — 项目级 agent 技能

Reasonix（及 Codex/Claude Code 等工具）的项目级技能发现目录之一（`<workspace>/{.reasonix,.agents,.agent,.claude}/skills/`），无需额外配置即被自动发现。技能体按需加载（`run_skill` 或 `/技能名` 调用时才读入上下文），是"减少 context 占用"的机制：**常规代码理解请读各目录 README（常驻导航），深度专项理解用技能（按需加载）**。

## 布局约定

- 每个技能一个子目录：`<技能名>/SKILL.md`
- `SKILL.md` 需带 YAML frontmatter：`name` + `description`（description 写清"何时使用"，决定技能被索引/匹配的效果）
- 支持 flat 形式 `<名字>.md`，但目录 + `SKILL.md` 是推荐形式

## 技能清单

| 技能 | 内容 | 何时调用 |
| --- | --- | --- |
| `hobob-lib` | `hobob/src/lib.rs` 入口/启动/宏的深度文档 | 改启动逻辑、日志、CLI、`vpath!`/`schema_uri!` 宏，或梳理 main_loop 时序时 |

## 维护约定

- 新增技能：按上述布局建目录写 `SKILL.md`，并在本 README 清单补一行。
- 技能内容与 `hobob/src/README.md` 的对应小节可能重叠；技能可写得"更深/更专"，README 保持导航性。
