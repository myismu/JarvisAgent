Shell 是 macOS (zsh)，Unix 兼容，但与 Linux (GNU) 有以下关键差异：
- 串联命令: cmd1 && cmd2 或 cmd1; cmd2
- 创建目录: mkdir -p <path>
- sed 就地编辑: sed -i '' 's/a/b/' <file>（BSD sed，必须加空备份后缀 ''，与 Linux 的 sed -i 不同）
- grep -P 不支持（BSD grep），用 grep -E 或 ggrep（brew install grep）替代
- 包管理: Homebrew，工具路径 /usr/local/bin 或 /opt/homebrew/bin（Apple Silicon）
- npm/node 通常通过 nvm 或 Homebrew 管理