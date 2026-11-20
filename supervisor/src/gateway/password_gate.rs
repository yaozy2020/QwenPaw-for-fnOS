use super::{unique_header, GatewayState};
use crate::proxy_auth::TTL;
use axum::{
    body::Body,
    extract::{Form, FromRequest, State},
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use std::time::Duration;

const FORM: &str = include_str!("proxy_login.html");

fn page(status: StatusCode, notice: &'static str) -> Response {
    (status, Html(FORM.replace("<!--notice-->", notice))).into_response()
}

fn browser_scheme(headers: &HeaderMap) -> Result<Option<super::OriginScheme>, ()> {
    if matches!(
        unique_header(headers, "sec-fetch-site")?,
        Some("cross-site" | "same-site")
    ) {
        return Err(());
    }
    let Some(origin) = unique_header(headers, "origin")? else {
        return Ok(None);
    };
    let browser = super::parse_origin(origin).ok_or(())?;
    let host = unique_header(headers, "host")?.ok_or(())?;
    let external = super::parse_authority(browser.scheme, host).ok_or(())?;
    (browser == external)
        .then_some(Some(browser.scheme))
        .ok_or(())
}

pub(super) fn authorized(state: &GatewayState, headers: &HeaderMap) -> bool {
    let auth = &state.app.proxy_auth;
    let (mut secure, mut plain) = (None, None);
    for value in headers.get_all(header::COOKIE) {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for part in value.split(';') {
            if let Some((name, value)) = part.trim().split_once('=') {
                let slot = if name == auth.cookie {
                    &mut plain
                } else if name.strip_prefix("__Host-") == Some(&auth.cookie) {
                    &mut secure
                } else {
                    continue;
                };
                if slot.replace(value).is_some() {
                    return false;
                }
            }
        }
    }
    secure.or(plain).is_some_and(|t| auth.accepts(t))
}

pub(super) fn same_origin_write(headers: &HeaderMap) -> bool {
    browser_scheme(headers).is_ok()
}

pub(super) fn challenge(state: &GatewayState) -> Response {
    match state.app.proxy_auth.password_status() {
        Some(false) => return Redirect::to("/").into_response(),
        None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Some(true) => (),
    }
    (StatusCode::UNAUTHORIZED, Html(FORM)).into_response()
}

pub(super) async fn login(State(state): State<GatewayState>, req: Request<Body>) -> Response {
    #[derive(serde::Deserialize)]
    struct Credentials {
        password: String,
    }
    if state.app.proxy_auth.password_status() != Some(true) {
        return challenge(&state);
    }
    let Ok(Some(scheme)) = browser_scheme(req.headers()) else {
        return page(StatusCode::FORBIDDEN, "<p role=alert>无法验证登录来源。请从原地址打开；使用反代时请保留浏览器访问的域名和端口（Host）。</p>");
    };
    let Ok(Ok(Form(credentials))) = tokio::time::timeout(
        Duration::from_secs(5),
        Form::<Credentials>::from_request(req, &()),
    )
    .await
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match state.app.proxy_auth.login(credentials.password).await {
        Ok(token) => {
            let target = if crate::install::base_path_patch::active(
                &std::path::Path::new(&state.app.core.home_dir).join("venv"),
            ) {
                "/app/qwenpaw/web/"
            } else {
                "/"
            };
            let (prefix, secure) = match scheme {
                super::OriginScheme::Https => ("__Host-", "; Secure"),
                super::OriginScheme::Http => ("", ""),
            };
            let mut response = Redirect::to(target).into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!(
                    "{prefix}{}={token}; Path=/{secure}; HttpOnly; SameSite=Strict; Max-Age={}",
                    state.app.proxy_auth.cookie,
                    TTL.as_secs()
                ))
                .unwrap(),
            );
            response
        }
        Err(StatusCode::UNAUTHORIZED) => page(
            StatusCode::UNAUTHORIZED,
            "<p role=alert>密码不正确。请输入反代访问密码，不是飞牛账号密码。</p>",
        ),
        Err(StatusCode::TOO_MANY_REQUESTS) => {
            let mut response = page(
                StatusCode::TOO_MANY_REQUESTS,
                "<p role=alert>尝试太快，请稍等一秒再登录。</p>",
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            response
        }
        Err(status) => status.into_response(),
    }
}

pub(super) async fn cache_policy(request: Request<Body>, next: axum::middleware::Next) -> Response {
    let login = request.uri().path() == "/_qwenpaw_proxy/login";
    let navigation = request.method() == Method::GET;
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if login || (navigation && response.status() == StatusCode::UNAUTHORIZED) {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; script-src 'self'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
        );
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        );
    }
    response
}
