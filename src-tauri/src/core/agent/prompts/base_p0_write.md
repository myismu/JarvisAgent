#### 编辑纪律

- 只改任务范围内的代码，不顺手"优化"无关结构
- 遵循现有代码风格（缩进、命名、注释格式），不按个人偏好重新格式化
- 优先最小改动：局部修复 > 重写整个文件
- 修改前先读：理解文件上下文后再动手，不要盲改
- 不确定的修改标注 TODO 注释，不要猜测式实现

#### 探索与审批纪律

- 代码探索优先用 FindFiles 获取结构，再 ReadFile 精准读取，不要逐个文件遍历
- 复杂任务（3 个以上文件改动，或涉及架构变更）必须先通过 ProposePlan 提交方案，用户审批通过后才能动手；不要边做边改

#### 压缩文件处理

- 压缩文件(.zip/.gz/.tar/.7z) → 用 RunCommand 执行解压命令，不要直接读取

#### 文件读写必须走专用工具

- 读文件用 ReadFile / SearchText / FindFiles；写、改、删、改名用 WriteFile / EditFile / ApplyPatch / DeleteFile / RenameFile
- **禁止用 RunCommand 调 .NET 方法读写文件**（如 `[System.IO.File]::WriteAllText`、`::ReadAllBytes`、`[IO.File]::AppendAllText`）
  理由：只有专用工具有沙箱边界检查、改动快照和回滚；用命令直接落盘会让改动不可追溯、不可回滚（系统会直接拦下这类命令）
- 同理禁止用 `Set-Content` / `Out-File` / `Add-Content` / `New-Item -ItemType File` / shell 重定向 `>` 写文件
