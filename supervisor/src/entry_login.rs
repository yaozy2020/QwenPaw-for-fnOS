use crate::kernel::KernelManager;
use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use std::{
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::Duration,
};

pub const RECORD: &str = "entry-login-default.v1";

pub fn path(pid_file: &Path) -> PathBuf {
    pid_file.with_file_name(RECORD)
}

pub fn selected(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub async fn send(
    kernel: &KernelManager,
    mut req: Request<Body>,
) -> Result<Response<Incoming>, StatusCode> {
    let unavailable = StatusCode::SERVICE_UNAVAILABLE;
    let identity = kernel.entry_identity().ok_or(unavailable)?;
    let uid = unsafe { libc::geteuid() };
    let name = format!("\0qwenpaw.entry.v2.{uid}.{}", identity.pid);
    let stream = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::UnixStream::connect(Path::new(std::ffi::OsStr::from_bytes(name.as_bytes()))),
    )
    .await
    .map_err(|_| unavailable)?
    .map_err(|_| unavailable)?;
    let peer = stream.peer_cred().map_err(|_| unavailable)?;
    if peer.pid() != Some(identity.pid)
        || peer.uid() != uid
        || !matches!(
            crate::procfs::identity_status(identity),
            crate::procfs::IdentityStatus::Alive
        )
    {
        return Err(unavailable);
    }
    let target = req
        .uri()
        .path_and_query()
        .ok_or(StatusCode::BAD_REQUEST)?
        .as_str()
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    *req.uri_mut() = target;
    req.headers_mut().insert(
        axum::http::header::HOST,
        axum::http::HeaderValue::from_static("localhost"),
    );
    let (mut client, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| unavailable)?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client
        .send_request(req)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

pub fn compatible(venv: &Path) -> bool {
    let Ok(dirs) = std::fs::read_dir(venv.join("lib")) else {
        return false;
    };
    let mut apps = dirs
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with("python3."))
        .map(|d| d.path().join("site-packages/qwenpaw/app"))
        .filter(|d| d.is_dir());
    let Some(app) = apps.next() else { return false };
    if apps.next().is_some() {
        return false;
    }
    [
        (
            "fnos_entry.py",
            include_bytes!("../../patches/qwenpaw-2.3.0.post1-base-path/payload/app/fnos_entry.py")
                .as_slice(),
        ),
        (
            "auth.py",
            include_bytes!("../../patches/qwenpaw-2.3.0.post1-base-path/payload/app/auth.py")
                .as_slice(),
        ),
        (
            "_app.py",
            include_bytes!("../../patches/qwenpaw-2.3.0.post1-base-path/payload/app/_app.py")
                .as_slice(),
        ),
    ]
    .iter()
    .all(|(name, expected)| std::fs::read(app.join(name)).is_ok_and(|b| b == *expected))
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    v: u8,
    instance: String,
    owner: String,
}

pub async fn migrate(
    kernel: &KernelManager,
    token: String,
) -> Result<(), (StatusCode, &'static str)> {
    let record = kernel.entry_record();
    match selected(&record) {
        Ok(true) => {
            return Err((StatusCode::CONFLICT, "已选择入口登录；不会覆盖迁移记录"));
        }
        Ok(false) => {}
        Err(_) => {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "无法确认迁移记录状态"));
        }
    }
    let owner = prove_owner(kernel, token).await?;
    let data = serde_json::to_string(&owner)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "记录编码失败"))?;
    crate::install::atomic_write_0600(
        record
            .to_str()
            .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "迁移记录路径无效"))?,
        &data,
    )
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "迁移记录保存失败"))
}

type LoginError = (StatusCode, &'static str);

async fn prove_owner(kernel: &KernelManager, token: String) -> Result<Record, LoginError> {
    if token.is_empty() || token.len() > 4096 || !token.is_ascii() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "请先从飞牛桌面用原 QwenPaw 账号登录一次",
        ));
    }
    let path = format!("{}/api/auth/verify", kernel.base_path());
    let req = Request::post(path)
        .header("x-fnos-entry-command", "prove")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .map_err(|_| (StatusCode::BAD_REQUEST, "无效的旧会话"))?;
    tokio::time::timeout(Duration::from_secs(4), async {
        let resp = send(kernel, req).await.map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "尚无可用的迁移版内核，请先安装匹配的内核适配并启动服务",
            )
        })?;
        if resp.status() != StatusCode::OK {
            return Err((
                StatusCode::UNAUTHORIZED,
                "旧账号归属无法确认；请重新登录原账号",
            ));
        }
        let body = axum::body::to_bytes(Body::new(resp.into_body()), 2048)
            .await
            .map_err(|_| (StatusCode::BAD_GATEWAY, "内核返回无效"))?;
        let r: Record = serde_json::from_slice(&body)
            .map_err(|_| (StatusCode::BAD_GATEWAY, "内核不支持迁移协议"))?;
        if r.v != 1
            || r.instance != "default"
            || r.owner.trim().is_empty()
            || r.owner.len() > 256
            || r.owner.chars().any(char::is_control)
        {
            return Err((StatusCode::BAD_GATEWAY, "内核身份无效"));
        }
        Ok(r)
    })
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "旧账号校验超时"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_distinguishes_missing_present_and_io_errors() {
        let dir = crate::api::test_support::TestDir::new("entry-login-selected");
        let record = dir.0.join("entry-login-default.v1");
        assert!(!selected(&record).unwrap());
        std::fs::write(&record, "record").unwrap();
        assert!(selected(&record).unwrap());

        let invalid = std::path::PathBuf::from("x".repeat(5000));
        assert!(selected(&invalid).is_err());
    }
}
