# hobob/static — 前端静态资源

| 文件 | 用途 |
| --- | --- |
| `index.js`（298 行） | 页面交互：`reload_filters`（下拉筛选经 `/card/filter/options`）、`enforce_tab_load`（标签页懒加载 `/card/ulist/...`）、`cur_filter`/`cur_order`（拼请求路径）、SSE 订阅 `/ev/engine` 刷新状态等。依赖 jQuery（由 `templates/index.html` 引入 CDN）。 |
| `favicon.ico` | 站点图标，经 `/favicon.ico` 提供。 |

由 `www.rs` 的 `warp::path("static").and(warp::fs::dir("./static"))` 提供——**工作目录必须是 crate 根**（`hobob/`）才能命中。
