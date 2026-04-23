// --- 取消机制模块 (Cancellation) ---
// 提供 Agent 循环的中途取消能力，允许用户通过前端「停止生成」按钮中断正在执行的任务。

use tokio_util::sync::CancellationToken;
use tokio::sync::Mutex;

/// 取消状态管理器
/// 每次 `ask_jarvis` 调用时创建新的 CancellationToken，
/// 用户点击「停止」时通过 `cancel_jarvis` command 触发取消信号。
pub struct CancellationState {
    pub token: Mutex<Option<CancellationToken>>,
}

impl CancellationState {
    pub fn new() -> Self {
        Self {
            token: Mutex::new(None),
        }
    }

    /// 创建并存储新的取消令牌，返回该令牌的克隆（供 Agent 循环使用）
    pub async fn create_token(&self) -> CancellationToken {
        let token = CancellationToken::new();
        let clone = token.clone();
        *self.token.lock().await = Some(token);
        clone
    }

    /// 触发取消信号
    pub async fn cancel(&self) {
        if let Some(token) = self.token.lock().await.take() {
            token.cancel();
        }
    }

    /// 清除当前令牌（Agent 循环正常结束时调用）
    pub async fn clear(&self) {
        *self.token.lock().await = None;
    }
}
