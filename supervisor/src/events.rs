//! 状态广播:预渲染快照 JSON 的 broadcast 通道,axum SSE 直接订阅。

use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Events {
    tx: broadcast::Sender<String>,
}

impl Events {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self { tx }
    }

    /// 广播一份快照 JSON(调用方已序列化);无订阅者时为空操作。
    pub fn publish(&self, snapshot_json: String) {
        let _ = self.tx.send(snapshot_json);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}
