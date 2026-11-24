Shell 是 Git Bash（POSIX/bash 语义，运行在 Windows 上）：
- 串联命令: cmd1 && cmd2（前段成功才继续）或 cmd1; cmd2
- 变量用 $ 前缀: VAR="value"；读取环境变量: $VAR_NAME
- 创建目录: mkdir -p <path>
- 路径用 bash 口径: E:\demo 写作 /e/demo；沙箱内优先用相对路径
- 禁止 PowerShell 语法: $env:VAR、Get-ChildItem、Remove-Item、Set-Content、Out-File 等
- 禁止 cd 切换目录，需要换目录时用 RunCommand 的 dir 参数
- 不要用 tee / sed -i / touch 写文件，改用 WriteFile / EditFile 工具
- 输出为 UTF-8，中文正常显示
