use std::{io, sync::Arc};

use tokio::{
    net::TcpListener,
    sync::{watch, Mutex},
};

use super::{web_tcp_router, AppState};
use crate::install::atomic_write_0600;

#[derive(Clone)]
pub struct WebProxyManager {
    app: AppState,
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    port: u16,
    shutdown: watch::Sender<bool>,
}

impl WebProxyManager {
    pub async fn start(app: AppState, requested_port: u16) -> io::Result<Self> {
        let (listener, port) = Self::bind_with_fallback(requested_port).await?;
        persist_port(&app, port)?;
        let (shutdown, stopped) = watch::channel(false);
        spawn(app.clone(), listener, stopped);
        Ok(Self {
            app,
            inner: Arc::new(Mutex::new(Inner { port, shutdown })),
        })
    }

    pub async fn reconfigure(&self, requested_port: u16) -> io::Result<u16> {
        let mut inner = self.inner.lock().await;
        let app = self.app.clone();
        let (listener, port) = bind(requested_port).await?;
        persist_port(&app, port)?;
        let (shutdown, stopped) = watch::channel(false);
        spawn(app, listener, stopped);
        let previous = std::mem::replace(&mut *inner, Inner { port, shutdown });
        let _ = previous.shutdown.send(true);
        Ok(port)
    }

    pub async fn port(&self) -> u16 {
        self.inner.lock().await.port
    }

    async fn bind_with_fallback(requested_port: u16) -> io::Result<(TcpListener, u16)> {
        match bind(requested_port).await {
            Ok(bound) => Ok(bound),
            Err(error) if requested_port != 0 && error.kind() == io::ErrorKind::AddrInUse => {
                bind(0).await
            }
            Err(error) => Err(error),
        }
    }
}

async fn bind(port: u16) -> io::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

fn persist_port(app: &AppState, port: u16) -> io::Result<()> {
    let path = format!("{}/web_proxy_port", app.var_dir);
    atomic_write_0600(&path, &format!("{port}\n"))
}

fn spawn(app: AppState, listener: TcpListener, mut stopped: watch::Receiver<bool>) {
    tokio::spawn(async move {
        let result = axum::serve(listener, web_tcp_router(app))
            .with_graceful_shutdown(async move {
                let _ = stopped.changed().await;
            })
            .await;
        if let Err(error) = result {
            eprintln!("web proxy listener stopped: {error}");
        }
    });
}

#[cfg(test)]
#[path = "web_proxy_manager_tests.rs"]
mod tests;
