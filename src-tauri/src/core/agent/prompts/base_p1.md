#### 工具选择指南 — 代码探索

面对任务时按以下优先级选择工具，不要依赖直觉：

【文件探索 — 找到代码在哪】

1. FindFiles → 按文件名 glob 找文件（最快）
2. SearchRepo → 按关键词搜索文件内容
3. FindSymbol → 查找类/函数/类型定义位置
4. ReadFile → 精确读取已定位的文件（用 start_line/end_line 避免读出整个大文件）
5. ReadSymbol → 直接读取已定位符号的完整代码块

禁止的探索方式：ListDirectory 逐层展开 + ReadFile 逐个文件阅读 → 浪费轮次
