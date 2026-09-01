# monkey — Tampermonkey 油猴脚本

把 bilibili 网页操作接到 hobob 的 API（部署在 `my-pi:3731`）。需 Tampermonkey 安装（见文件头部注释的 install hint），依赖 jQuery CDN + `GM.xmlHttpRequest`（需 `@connect *`）。

## 脚本

### `follow_op.js` — 空间页关注接入 hobob

- 在 bilibili 用户空间页（`space.bilibili.com/<uid>`）运行：从 `.up-name`/`.staff-name` 提取 uid。
- 若该 uid 已关注（GET `/get/user/{id}` 返回对象），隐藏"关注"按钮并提示"已关注"；否则把按钮点击改为 POST `/op/follow`（`{"enable": true, "uid": id}`）。
- 附带把页面头部登录入口改为跳转该 UP 主的视频列表页（`regPlaylist`，延迟 2s 注册）。

### `set_live_url.js` — 直播页上报直播 URL

- 在 bilibili 直播页运行：取 `a.living-section__link` 的 href 与路径中的 uid，POST 到 `/op/setliveurl`（`{"uid", "live"}`）；3s 与 15s 各执行一次（等待动态加载）。

## ⚠️ 已知不一致（重要）

两个脚本调用的端点 **在当前 `hobob/src/www.rs` 路由中不存在**：

- `GET /get/user/{id}` — 当前无此路由（www.rs 只有 `/card/one/{uid}`）
- `POST /op/setliveurl` — 当前无此路由（www.rs 的 `/op/*` 只有 follow/refresh/silence/toggle/group/touch/group）

git 历史（`monkey: setliveurl helper`、`monkey: fix get live url`）显示这些功能曾存在，但当前代码已移除。**脚本已过时**，需配合恢复相应路由（或在 www.rs 添加等价端点）才能工作；硬编码的 `my-pi:3731` 也需按实际部署地址修改。
