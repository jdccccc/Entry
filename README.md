逻辑：
1. 维护4个App_State：MainMenu、TodoView、CyberView、BillView
2. 每个App_State对应一个界面，界面上有不同的操作选项
3. 主循环：
   - 初始化：加载配置 → 开启 raw 模式 + 备用屏幕 → 进入主循环
   - 主循环：
     - MainMenu：选择 TODO / Cyber / Bill，或退出
       - MainMenu有四个栏目，分别是Hello,Menu,Help,Weather
         - 按下Enter键进入对应界面
         - help仅显示操作帮助信息
         - Weather 异步显示当前天气，手动获取
     - TODO / Cyber：读取配置中的文件 → 解析 Markdown 表格 → 通用表格组件渲染 → `jk`移动，`e` 编辑，`r` 重新载入
       - 如果对应的文件不存在，在第二栏提示“文件不存在”，不需要额外的提示信息和任何判断逻辑
     - Bill：
       1. 进入页面显示待分析账单与可导出的报表数量, 如果没有账单, 则第二栏提示“暂无账单”，不需要额外的提示信息和任何判断逻辑
       2. `a` 分析，`o` 导出, `r` 重新载入