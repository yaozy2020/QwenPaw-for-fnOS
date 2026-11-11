use std::{io, sync::Arc};

use tokio::{
    net::TcpListener,
    sync::{watch, Mutex},
};

use super::{web_tcp_router_with_access, AppState};

#[derive(Clone)]
pub struct WebProxyManager {
    app: AppState,
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    port: u16,
    public: bool,
    shutdown: watch::Sender<bool>,
}

impl WebProxyManager {
    pub async fn start(app: AppState, requested_port: u16, public: bool) -> io::Result<Self> {
        let (listener, port) = Self::bind_with_fallback(requested_port, public).await?;
        persist_state(&app, port, public)?;
        let (shutdown, stopped) = watch::channel(false);
        spawn(app.clone(), listener, stopped, public);
        Ok(Self {
            app,
            inner: Arc::new(Mutex::new(Inner {
                port,
                public,
                shutdown,
            })),
        })
    }

    pub async fn reconfigure(&self, requested_port: u16, public: bool) -> io::Result<u16> {
        let mut inner = self.inner.lock().await;
        let previous_port = inner.port;
        let previous_public = inner.public;
        let app = self.app.clone();
        let same_port_access_change = requested_port == previous_port && public != previous_public;
        if same_port_access_change {
            let _ = inner.shutdown.send(true);
        }
        let bound = if same_port_access_change {
            bind_after_close(requested_port, public).await
        } else {
            bind(requested_port, public).await
        };
        let (listener, port) = match bound {
            Ok(bound) => bound,
            Err(error) => {
                if same_port_access_change {
                    restore_previous(&app, &mut inner, previous_port, previous_public).await?;
                }
                return Err(error);
            }
        };
        if let Err(error) = persist_state(&app, port, public) {
            drop(listener);
            if same_port_access_change {
                restore_previous(&app, &mut inner, previous_port, previous_public).await?;
            }
            return Err(error);
        }
        let (shutdown, stopped) = watch::channel(false);
        spawn(app, listener, stopped, public);
        let previous = std::mem::replace(
            &mut *inner,
            Inner {
                port,
                public,
                shutdown,
            },
        );
        let _ = previous.shutdown.send(true);
        Ok(port)
    }

    pub async fn state(&self) -> (u16, bool) {
        let inner = self.inner.lock().await;
        (inner.port, inner.public)
    }

    async fn bind_with_fallback(
        requested_port: u16,
        public: bool,
    ) -> io::Result<(TcpListener, u16)> {
        match bind(requested_port, public).await {
            Ok(bound) => Ok(bound),
            Err(error) if requested_port != 0 && error.kind() == io::ErrorKind::AddrInUse => {
                bind(0, public).await
            }
            Err(error) => Err(error),
        }
    }
}

async fn bind(port: u16, public: bool) -> io::Result<(TcpListener, u16)> {
    let host = if public { "0.0.0.0" } else { "127.0.0.1" };
    let listener = TcpListener::bind((host, port)).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

async fn bind_after_close(port: u16, public: bool) -> io::Result<(TcpListener, u16)> {
    let mut last_error = None;
    for _ in 0..100 {
        match bind(port, public).await {
            Ok(bound) => return Ok(bound),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("previous web proxy listener did not close")))
}

async fn restore_previous(
    app: &AppState,
    inner: &mut Inner,
    port: u16,
    public: bool,
) -> io::Result<()> {
    let (listener, _) = bind_after_close(port, public).await?;
    let (shutdown, stopped) = watch::channel(false);
    spawn(app.clone(), listener, stopped, public);
    inner.shutdown = shutdown;
    Ok(())
}

fn persist_state(app: &AppState, port: u16, public: bool) -> io::Result<()> {
    let var = std::path::Path::new(&app.var_dir);
    let result = crate::settings_transaction::persist_web_proxy(var, port, public);
    if result.is_err()
        && var
            .join("web-proxy-transaction.json")
            .try_exists()
            .unwrap_or(true)
    {
        app.install.block_recovery("反代配置事务尚待恢复".into());
    }
    result
}

fn spawn(app: AppState, listener: TcpListener, mut stopped: watch::Receiver<bool>, public: bool) {
    tokio::spawn(async move {
        let result = axum::serve(listener, web_tcp_router_with_access(app, public))
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
