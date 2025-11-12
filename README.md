# Entry

## 逻辑说明（极简）
- 启动：加载配置 → 开启 raw 模式 + 备用屏幕 → 进入主循环
- 主循环：
  - 主菜单：选择 TODO/Cyber，或退出
  - TODO 视图：读取 `TODO.md` → 解析 Markdown 表格 → 表格渲染与滚动 → `e` 打开 Neovim 编辑并返回后重载
  - Cyber 视图：同上，但文件为 `CyberResource.md`
- 退出：恢复终端模式并离开备用屏幕