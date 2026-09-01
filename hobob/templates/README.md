# hobob/templates — tera HTML 模板

| 文件 | 用途 |
| --- | --- |
| `index.html`（153 行） | 首页骨架：筛选下拉、tab（default/video/live 排序）、卡片容器、SSE 连接；引入 jQuery + `static/index.js` |
| `user_cards.html`（84 行） | UP 主卡片列表（`/card/one/{uid}` 与 `/card/ulist/...` 共用），含直播/视频/关注状态展示 |
| `filter_options.html` | 分组筛选 `<option>` 列表（`/card/filter/options`） |
| `failure.html` | 渲染失败兜底页（`render_fail` 统一使用） |

**加载方式**（`www.rs` 的 `TERA`）：
- debug 构建：从磁盘 `templates/**/*.html` 加载——改模板后无需重编译，但工作目录必须是 `hobob/`。
- release 构建：`include_str!` 编译期内嵌——改模板必须重新编译。
