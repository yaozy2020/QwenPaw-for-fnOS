//! 状态失效通知。订阅者合并尚未消费的通知，不保存快照或变更历史。

use tokio::sync::watch;

#[derive(Clone)]
pub struct Events {
    tx: watch::Sender<()>,
}

impl Events {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(());
        Self { tx }
    }

    pub fn invalidate(&self) {
        let _ = self.tx.send(());
    }

    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.tx.subscribe()
    }
}
