Shell 是 PowerShell 5.1（非 Bash），语法不同：
- 允许用 && 串联命令（程序会自动把它展开为"逐段执行、前段失败即停"，与 bash 语义一致）
- 禁止使用 || 串联命令（不支持，会直接报语法错误）
- 串联命令也可用 ; 分隔: cmd1; cmd2（不管前一个是否成功，顺序执行）
- 变量用 $ 前缀: $var = "value"
- 读取环境变量: $env:VAR_NAME
- 创建目录: mkdir "<path>" -Force（不用 mkdir -p，-p 不被支持）
- 禁止使用 Linux 命令: pwd/rm/grep/cat/sed/awk（有对应 PowerShell cmdlet 或用途不同）
- 查看目录内容: Get-ChildItem（不用 ls）
- 用管道 | 连接命令可行，但注意 PowerShell 传递的是对象而非文本