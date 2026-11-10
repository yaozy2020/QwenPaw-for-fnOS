use std::{collections::HashSet, time::Duration};

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use http_body_util::{BodyExt, Limited};
use tokio_stream::StreamExt;

use super::GatewayState;

const WEB_PATH: &str = "/web";
const WEB_PREFIX: &str = "/app/qwenpaw/web";

pub(super) fn mount_path(path: &str) -> Result<Option<&str>, StatusCode> {
    let Some(tail) = path.strip_prefix(WEB_PATH) else {
        return Ok(None);
    };
    if !tail.is_empty() && !tail.starts_with('/') {
        return Ok(None);
    }
    let tail = if tail.is_empty() { "/" } else { tail };
    validate_path(tail)?;
    Ok(Some(tail))
}

pub(super) fn upgrade_requested(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
        || headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
}

pub(super) async fn forward(
    state: &GatewayState,
    request: Request<Body>,
    path: &str,
    secure: bool,
) -> Response {
    forward_impl(state, request, path, secure, true).await
}

pub(crate) async fn forward_root(
    state: &GatewayState,
    request: Request<Body>,
    path: &str,
) -> Response {
    forward_impl(state, request, path, false, false).await
}

async fn forward_impl(
    state: &GatewayState,
    request: Request<Body>,
    path: &str,
    secure: bool,
    rewrite_paths: bool,
) -> Response {
    let method = request.method().clone();
    let head_request = method == Method::HEAD;
    if !matches!(method, Method::GET | Method::HEAD | Method::POST) {
        let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD, POST"));
        return response;
    }

    let forwarded_host = match forwarded_host(request.headers(), secure) {
        Ok(host) => host,
        Err(status) => return status.into_response(),
    };
    let mut headers = match request_headers(
        request.headers(),
        secure,
        &forwarded_host,
        if rewrite_paths { WEB_PREFIX } else { "/" },
    ) {
        Ok(headers) => headers,
        Err(status) => return status.into_response(),
    };
    let rewritable_path = is_rewritable_path(path);
    let needs_identity = rewritable_path
        && (rewrite_paths
            || matches!(path, "/" | "/login" | "/console")
            || path.ends_with(".html"));
    let rewrite_head = head_request && needs_identity;
    if matches!(method, Method::GET | Method::HEAD) && needs_identity {
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        headers.remove(header::RANGE);
        headers.remove(header::IF_RANGE);
    }

    let port = state.app.core.kernel.port();
    if !rewrite_paths && is_config_path(path) {
        if let Err(status) = verify_config_session(state, port, &headers).await {
            return status.into_response();
        }
    }
    let mut target = format!("http://127.0.0.1:{port}{path}");
    if let Some(query) = request.uri().query() {
        target.push('?');
        target.push_str(query);
    }
    let uri = match target.parse::<Uri>() {
        Ok(uri) => uri,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let (mut parts, body) = request.into_parts();
    parts.uri = uri;
    parts.headers = headers;
    if rewrite_head {
        parts.method = Method::GET;
    }
    let request = Request::from_parts(parts, body);

    let upstream = match state.web_client.request(request).await {
        Ok(response) => response,
        Err(error) if error.is_connect() => {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };

    let status = upstream.status();
    let mut response_headers = response_headers(upstream.headers(), rewrite_paths);
    let content_type = response_headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let rewrite_kind = content_type
        .and_then(rewrite_kind)
        .filter(|kind| rewrite_paths || matches!(kind, RewriteKind::Html));
    let is_sse = content_type
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
    let upstream_body = upstream.into_body();
    let body = if is_sse {
        if head_request {
            Body::empty()
        } else {
            let permit = match state.sse_connections.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => return super::retryable_overload(),
            };
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            tokio::spawn(async move {
                let _permit = permit;
                let mut stream = upstream_body.into_data_stream();
                while let Some(chunk) = stream.next().await {
                    if sender.send(chunk).await.is_err() {
                        break;
                    }
                }
            });
            Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receiver))
        }
    } else if (method == Method::GET || rewrite_head) && status == StatusCode::OK {
        if let Some(kind) = rewrite_kind {
            if response_headers
                .get(header::CONTENT_ENCODING)
                .is_some_and(|value| value != "identity")
            {
                return StatusCode::BAD_GATEWAY.into_response();
            }
            let bytes = match Limited::new(upstream_body, MAX_REWRITE_BYTES)
                .collect()
                .await
            {
                Ok(bytes) => bytes.to_bytes(),
                Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
            };
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
            };
            let rewritten = if rewrite_paths {
                rewrite_text(text, kind)
            } else {
                inject_language(text)
            };
            remove_rewrite_entity_headers(&mut response_headers);
            let Ok(content_length) = HeaderValue::from_str(&rewritten.len().to_string()) else {
                return StatusCode::BAD_GATEWAY.into_response();
            };
            response_headers.insert(header::CONTENT_LENGTH, content_length);
            if head_request {
                Body::empty()
            } else {
                Body::from(rewritten)
            }
        } else if head_request {
            Body::empty()
        } else {
            Body::new(upstream_body)
        }
    } else if head_request {
        Body::empty()
    } else {
        Body::new(upstream_body)
    };

    let mut response = Response::builder()
        .status(status)
        .body(body)
        .expect("upstream response status");
    *response.headers_mut() = response_headers;
    response
}

const MAX_REWRITE_BYTES: usize = 8 * 1024 * 1024;
const MAX_VERIFY_BYTES: usize = 8 * 1024;
const VERIFY_TIMEOUT: Duration = Duration::from_secs(2);
const LANGUAGE_INIT: &str = "<script>try{if(localStorage.getItem('language')===null)localStorage.setItem('language','zh')}catch{}</script>";

fn is_config_path(path: &str) -> bool {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    let decoded_twice = percent_encoding::percent_decode_str(&decoded).decode_utf8_lossy();
    let matched = [path, decoded.as_ref(), decoded_twice.as_ref()]
        .into_iter()
        .any(|path| path == "/api/config" || path.starts_with("/api/config/"));
    matched
}

async fn verify_config_session(
    state: &GatewayState,
    port: u16,
    headers: &HeaderMap,
) -> Result<(), StatusCode> {
    let mut request = Request::get(format!("http://127.0.0.1:{port}/api/auth/verify"))
        .body(Body::empty())
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let cookie = headers.get(header::COOKIE);
    let has_cookie = cookie
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').any(|cookie| {
                cookie.split_once('=').is_some_and(|(name, value)| {
                    !name.trim().is_empty() && !value.trim().is_empty()
                })
            })
        });
    let authorization = match super::unique_header(headers, header::AUTHORIZATION.as_str()) {
        Ok(Some(value))
            if value
                .strip_prefix("Bearer ")
                .is_some_and(|token| !token.trim().is_empty()) =>
        {
            Some(value)
        }
        Ok(None) => None,
        _ => return Err(StatusCode::UNAUTHORIZED),
    };
    if !has_cookie && authorization.is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if let Some(cookie) = cookie {
        request.headers_mut().insert(header::COOKIE, cookie.clone());
    }
    if let Some(authorization) = authorization {
        request.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(authorization).map_err(|_| StatusCode::UNAUTHORIZED)?,
        );
    }
    tokio::time::timeout(VERIFY_TIMEOUT, async {
        let response = state
            .web_client
            .request(request)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        if !response.status().is_success() {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Limited::new(response.into_body(), MAX_VERIFY_BYTES)
            .collect()
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        Ok(())
    })
    .await
    .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
}

#[derive(Clone, Copy)]
enum RewriteKind {
    Html,
    JavaScript,
    Css,
}

fn is_rewritable_path(path: &str) -> bool {
    path == "/"
        || path.ends_with(".html")
        || path.ends_with(".js")
        || path.ends_with(".mjs")
        || path.ends_with(".css")
}

fn rewrite_kind(content_type: &str) -> Option<RewriteKind> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => Some(RewriteKind::Html),
        "text/javascript"
        | "application/javascript"
        | "text/ecmascript"
        | "application/ecmascript" => Some(RewriteKind::JavaScript),
        "text/css" => Some(RewriteKind::Css),
        _ => None,
    }
}

fn remove_rewrite_entity_headers(headers: &mut HeaderMap) {
    for name in [
        header::CONTENT_LENGTH,
        header::CONTENT_ENCODING,
        header::CONTENT_RANGE,
        header::ACCEPT_RANGES,
        header::ETAG,
    ] {
        headers.remove(name);
    }
    for name in ["content-md5", "digest", "content-digest", "repr-digest"] {
        headers.remove(name);
    }
}

fn rewrite_text(text: &str, kind: RewriteKind) -> String {
    match kind {
        RewriteKind::Html => rewrite_html(text),
        RewriteKind::JavaScript => rewrite_javascript(text),
        RewriteKind::Css => rewrite_css(text),
    }
}

fn rewrite_html(text: &str) -> String {
    let mut rewritten = rewrite_html_attributes(text);
    rewritten = rewritten.replace(
        r#""/api/version""#,
        &format!(r#""{WEB_PREFIX}/api/version""#),
    );
    inject_language(&rewritten)
}

fn inject_language(text: &str) -> String {
    let mut rewritten = text.to_owned();
    if !rewritten.contains("localStorage.setItem('language','zh')") {
        if let Some(index) = rewritten.find("<script type=\"module\"") {
            rewritten.insert_str(index, LANGUAGE_INIT);
        } else if let Some(index) = rewritten.find("</head>") {
            rewritten.insert_str(index, LANGUAGE_INIT);
        }
    }
    rewritten
}

fn rewrite_html_attributes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(relative) = bytes[cursor..].iter().position(|byte| *byte == b'<') else {
            break;
        };
        let start = cursor + relative;
        if bytes.get(start..start + 4) == Some(b"<!--") {
            let Some(end) = find_bytes(&bytes[start + 4..], b"-->") else {
                break;
            };
            cursor = start + 4 + end + 3;
            continue;
        }
        let Some(tag_end) = find_html_tag_end(bytes, start + 1) else {
            break;
        };
        let mut index = start + 1;
        let closing = bytes.get(index) == Some(&b'/');
        if closing {
            index += 1;
        }
        let tag_start = index;
        while index < tag_end && !bytes[index].is_ascii_whitespace() && bytes[index] != b'/' {
            index += 1;
        }
        let tag = &text[tag_start..index];
        while index < tag_end {
            while index < tag_end && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
                index += 1;
            }
            let name_start = index;
            while index < tag_end
                && !bytes[index].is_ascii_whitespace()
                && !matches!(bytes[index], b'=' | b'/' | b'>')
            {
                index += 1;
            }
            if name_start == index {
                index += 1;
                continue;
            }
            let name = &text[name_start..index];
            while index < tag_end && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index == tag_end || bytes[index] != b'=' {
                continue;
            }
            index += 1;
            while index < tag_end && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            let quote = bytes
                .get(index)
                .copied()
                .filter(|byte| matches!(byte, b'\'' | b'"'));
            if quote.is_some() {
                index += 1;
            }
            let value_start = index;
            while index < tag_end
                && if let Some(quote) = quote {
                    bytes[index] != quote
                } else {
                    !bytes[index].is_ascii_whitespace() && bytes[index] != b'>'
                }
            {
                index += 1;
            }
            let value_end = index;
            let value = &text[value_start..value_end];
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "href" | "src" | "action" | "poster" | "data"
            ) && value.starts_with('/')
                && !value.starts_with("//")
                && !value.starts_with(WEB_PREFIX)
            {
                ranges.push((value_start, value_end));
            }
            if quote.is_some() && index < tag_end {
                index += 1;
            }
        }
        cursor = tag_end + 1;
        if !closing && (tag.eq_ignore_ascii_case("script") || tag.eq_ignore_ascii_case("style")) {
            let close = if tag.eq_ignore_ascii_case("script") {
                b"</script".as_slice()
            } else {
                b"</style".as_slice()
            };
            if let Some(relative) = find_ascii_case_insensitive(&bytes[cursor..], close) {
                cursor += relative;
            }
        }
    }

    let mut rewritten = String::with_capacity(text.len() + ranges.len() * WEB_PREFIX.len());
    let mut copied = 0;
    for (start, end) in ranges {
        rewritten.push_str(&text[copied..start]);
        rewritten.push_str(WEB_PREFIX);
        rewritten.push_str(&text[start..end]);
        copied = end;
    }
    rewritten.push_str(&text[copied..]);
    rewritten
}

fn find_html_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in bytes.iter().copied().enumerate().skip(start) {
        if let Some(active) = quote {
            if byte == active {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some(index);
        }
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

fn rewrite_javascript(text: &str) -> String {
    let mut rewritten = text
        .replace(
            r#"function ce(e){const n="/api",o=e.startsWith("/")?e:`/${e}`;return`${n}${o}`}"#,
            r#"function ce(e){const n="/app/qwenpaw/web/api",o=e.startsWith("/")?e:`/${e}`;return`${n}${o}`}"#,
        )
        .replace(
            r#"function vE(){return"/api"}"#,
            r#"function vE(){return"/app/qwenpaw/web/api"}"#,
        )
        .replace(
            r#"function Wa(e){return/^\/console(?:\/|$)/.test(e)?AA:void 0}"#,
            r#"function Wa(e){return/^\/app\/qwenpaw\/web\/console(?:\/|$)/.test(e)?"/app/qwenpaw/web/console":/^\/app\/qwenpaw\/web(?:\/|$)/.test(e)?"/app/qwenpaw/web":/^\/console(?:\/|$)/.test(e)?AA:void 0}"#,
        )
        .replace(
            r#"function Qa(e){return Ai&&(e===Ai||e.startsWith(`${Ai}/`))?Ai:void 0}"#,
            r#"function Qa(e){return /^\/app\/qwenpaw\/web(?:\/|$)/.test(e)?"/app/qwenpaw/web":Ai&&(e===Ai||e.startsWith(`${Ai}/`))?Ai:void 0}"#,
        )
        .replace(
            r#"window.location.assign("/login")"#,
            &format!(r#"window.location.assign("{WEB_PREFIX}/login")"#),
        )
        .replace(
            r#"window.location.href="/login""#,
            &format!(r#"window.location.href="{WEB_PREFIX}/login""#),
        )
        .replace(r#""/api/version""#, &format!(r#""{WEB_PREFIX}/api/version""#));
    for asset in [
        "logo-dark.svg",
        "logo-light.svg",
        "qwenpaw-avatar.gif",
        "qwenpaw.png",
    ] {
        rewritten = rewritten.replace(
            &format!("\"/{asset}\""),
            &format!("\"{WEB_PREFIX}/{asset}\""),
        );
    }
    for variable in ["i", "w"] {
        rewritten = rewritten.replace(
            &format!("`/${{{variable}?\"logo-dark.svg\":\"logo-light.svg\"}}`"),
            &format!("`{WEB_PREFIX}/${{{variable}?\"logo-dark.svg\":\"logo-light.svg\"}}`"),
        );
    }
    rewritten = rewritten
        .replace(r#""/api""#, &format!(r#""{WEB_PREFIX}/api""#))
        .replace(r#"'/api'"#, &format!(r#"'{WEB_PREFIX}/api'"#));
    rewritten = rewrite_quoted_asset_prefix(&rewritten, "/api/", &format!("{WEB_PREFIX}/api/"));
    rewritten =
        rewrite_quoted_asset_prefix(&rewritten, "/assets/", &format!("{WEB_PREFIX}/assets/"));
    rewrite_vite_dependency_assets(&rewritten)
}

fn rewrite_quoted_asset_prefix(text: &str, prefix: &str, replacement: &str) -> String {
    let mut rewritten = text.to_owned();
    for quote in ["\"", "'", "`"] {
        rewritten = rewritten.replace(
            &format!("{quote}{prefix}"),
            &format!("{quote}{replacement}"),
        );
    }
    rewritten
}

fn rewrite_vite_dependency_assets(text: &str) -> String {
    let marker = "m.f||(m.f=[";
    let Some(dependencies_start) = text.find(marker).map(|index| index + marker.len()) else {
        return text.to_owned();
    };
    let Some(dependencies_end) = text[dependencies_start..]
        .find("])))=>i.map(i=>d[i])")
        .map(|index| dependencies_start + index)
    else {
        return text.to_owned();
    };

    let dependencies = rewrite_quoted_asset_prefix(
        &text[dependencies_start..dependencies_end],
        "assets/",
        &format!("{}/assets/", WEB_PREFIX.trim_start_matches('/')),
    );
    let mut rewritten = String::with_capacity(text.len() + dependencies.len());
    rewritten.push_str(&text[..dependencies_start]);
    rewritten.push_str(&dependencies);
    rewritten.push_str(&text[dependencies_end..]);
    rewritten
}

fn rewrite_css(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor + 4 <= bytes.len() {
        let Some(relative) = find_ascii_case_insensitive(&bytes[cursor..], b"url(") else {
            break;
        };
        let open = cursor + relative;
        let mut index = open + 4;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let quote = bytes
            .get(index)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        if quote.is_some() {
            index += 1;
        }
        let value_start = index;
        while index < bytes.len()
            && if let Some(quote) = quote {
                bytes[index] != quote
            } else {
                bytes[index] != b')'
            }
        {
            index += 1;
        }
        let value_end = index;
        let value = &text[value_start..value_end];
        if value.starts_with('/') && !value.starts_with("//") && !value.starts_with(WEB_PREFIX) {
            ranges.push((value_start, value_end));
        }
        let Some(close) = bytes[index..].iter().position(|byte| *byte == b')') else {
            break;
        };
        cursor = index + close + 1;
    }

    let mut rewritten = String::with_capacity(text.len() + ranges.len() * WEB_PREFIX.len());
    let mut copied = 0;
    for (start, end) in ranges {
        rewritten.push_str(&text[copied..start]);
        rewritten.push_str(WEB_PREFIX);
        rewritten.push_str(&text[start..end]);
        copied = end;
    }
    rewritten.push_str(&text[copied..]);
    rewritten
}

fn forwarded_host(headers: &HeaderMap, secure: bool) -> Result<String, StatusCode> {
    let proto = if secure { "https" } else { "http" };
    let host = match super::unique_header(headers, "x-forwarded-host")
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        Some(host) => host,
        None => super::unique_header(headers, header::HOST.as_str())
            .map_err(|_| StatusCode::BAD_REQUEST)?
            .unwrap_or("localhost"),
    };
    if super::parse_forwarded_origin(proto, host).is_none() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(host.to_owned())
}

fn request_headers(
    source: &HeaderMap,
    secure: bool,
    forwarded_host: &str,
    forwarded_prefix: &str,
) -> Result<HeaderMap, StatusCode> {
    let mut headers = source.clone();
    let connection_headers = connection_headers(&headers);
    let remove: Vec<_> = headers
        .keys()
        .filter(|name| {
            is_hop_by_hop(name, &connection_headers)
                || name.as_str() == "host"
                || name.as_str() == "content-length"
                || name.as_str() == "expect"
                || name.as_str() == "forwarded"
                || name.as_str() == "x-csrf-token"
                || name.as_str() == "x-supervisor-token"
                || name.as_str().starts_with("x-trim-")
                || name.as_str().starts_with("x-forwarded-")
        })
        .cloned()
        .collect();
    for name in remove {
        headers.remove(name);
    }
    filter_request_cookies(&mut headers)?;

    headers.insert(
        "x-forwarded-host",
        HeaderValue::from_str(forwarded_host).map_err(|_| StatusCode::BAD_REQUEST)?,
    );
    headers.insert(
        "x-forwarded-proto",
        HeaderValue::from_static(if secure { "https" } else { "http" }),
    );
    headers.insert(
        "x-forwarded-prefix",
        HeaderValue::from_str(forwarded_prefix).map_err(|_| StatusCode::BAD_REQUEST)?,
    );
    Ok(headers)
}

fn filter_request_cookies(headers: &mut HeaderMap) -> Result<(), StatusCode> {
    let mut cookies = Vec::new();
    for value in headers.get_all(header::COOKIE) {
        let value = value.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
        cookies.extend(value.split(';').map(|cookie| cookie.trim().to_owned()));
    }
    headers.remove(header::COOKIE);
    let retained: Vec<_> = cookies
        .into_iter()
        .filter(|cookie| {
            let name = cookie
                .split_once('=')
                .map_or(cookie.as_str(), |(name, _)| name.trim());
            !is_internal_cookie(name)
        })
        .collect();
    if !retained.is_empty() {
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&retained.join("; ")).map_err(|_| StatusCode::BAD_REQUEST)?,
        );
    }
    Ok(())
}

fn response_headers(source: &HeaderMap, rewrite_paths: bool) -> HeaderMap {
    let connection_headers = connection_headers(source);
    let mut headers = HeaderMap::new();
    for (name, value) in source {
        if is_hop_by_hop(name, &connection_headers)
            || name == "forwarded"
            || name == "x-supervisor-token"
            || name == "x-csrf-token"
            || name.as_str().starts_with("x-trim-")
            || name.as_str().starts_with("x-forwarded-")
        {
            continue;
        }
        if name == header::SET_COOKIE {
            if let Some(value) = rewrite_set_cookie(value, rewrite_paths) {
                headers.append(name.clone(), value);
            }
        } else {
            headers.append(name.clone(), value.clone());
        }
    }
    headers
}

fn rewrite_set_cookie(value: &HeaderValue, rewrite_path: bool) -> Option<HeaderValue> {
    let input = value.to_str().ok()?;
    let cookie = input.split(';').next()?.trim();
    let name = cookie.split_once('=')?.0.trim();
    if is_internal_cookie(name) {
        return None;
    }

    let mut output = String::new();
    let mut attributes = input.split(';');
    output.push_str(attributes.next()?.trim());
    for attribute in attributes {
        let attribute = attribute.trim();
        if let Some((name, path)) = attribute.split_once('=') {
            if rewrite_path
                && name.trim().eq_ignore_ascii_case("path")
                && path.trim().starts_with('/')
            {
                let path = path.trim();
                output.push_str("; Path=");
                if path == WEB_PREFIX || path.starts_with(&format!("{WEB_PREFIX}/")) {
                    output.push_str(path);
                } else {
                    output.push_str(WEB_PREFIX);
                    output.push_str(path);
                }
                continue;
            }
        }
        output.push_str("; ");
        output.push_str(attribute);
    }
    HeaderValue::from_str(&output).ok()
}

fn is_internal_cookie(name: &str) -> bool {
    matches!(
        name,
        "qwenpaw_csrf" | "__Host-qwenpaw_csrf" | "supervisor-token" | "supervisor_token" | "ost"
    )
}

fn connection_headers(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|value| HeaderName::from_bytes(value.trim().as_bytes()).ok())
        .collect()
}

fn is_hop_by_hop(name: &HeaderName, connection_headers: &HashSet<HeaderName>) -> bool {
    connection_headers.contains(name)
        || matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "proxy-connection"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "http2-settings"
        )
}

pub(super) fn validate_path(path: &str) -> Result<(), StatusCode> {
    if path.is_empty()
        || !path.starts_with('/')
        || path.len() > 8 * 1024
        || path.contains(['\\', '\0', '#'])
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut decoded = path.to_owned();
    for _ in 0..=2 {
        let bytes = decoded.as_bytes();
        for (offset, byte) in bytes.iter().enumerate() {
            if *byte == b'%' {
                let Some(digits) = bytes.get(offset + 1..offset + 3) else {
                    return Err(StatusCode::BAD_REQUEST);
                };
                if !digits.iter().all(u8::is_ascii_hexdigit) {
                    return Err(StatusCode::BAD_REQUEST);
                }
            }
        }
        if decoded.contains(['\\', '\0', '#'])
            || decoded[1..].contains("//")
            || decoded.split('/').any(|part| matches!(part, "." | ".."))
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        decoded = percent_encoding::percent_decode_str(&decoded)
            .decode_utf8_lossy()
            .into_owned();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_rewrites_root_assets_for_all_quote_forms_without_touching_other_paths() {
        let source = r#"const a="/assets/entry.js",b='/assets/login.js',c=`/assets/ui.css`,d="/app/qwenpaw/web/assets/prefixed.js",e="/login",f=`/${w?"logo-dark.svg":"logo-light.svg"}`,g=`/${i?"logo-dark.svg":"logo-light.svg"}`;"#;
        let rewritten = rewrite_javascript(source);

        for expected in [
            r#""/app/qwenpaw/web/assets/entry.js""#,
            "'/app/qwenpaw/web/assets/login.js'",
            "`/app/qwenpaw/web/assets/ui.css`",
            r#""/app/qwenpaw/web/assets/prefixed.js""#,
            r#""/login""#,
            r#"`/app/qwenpaw/web/${w?"logo-dark.svg":"logo-light.svg"}`"#,
            r#"`/app/qwenpaw/web/${i?"logo-dark.svg":"logo-light.svg"}`"#,
        ] {
            assert!(rewritten.contains(expected), "missing {expected}");
        }
        assert!(!rewritten.contains("/app/qwenpaw/web/app/qwenpaw/web/assets/"));
        assert_eq!(rewrite_javascript(&rewritten), rewritten);
    }

    #[test]
    fn javascript_rewrites_concatenated_api_prefix_for_auth_status() {
        let source = r#"const endpoint = "/api" + "/auth/status";"#;
        let rewritten = rewrite_javascript(source);

        assert_eq!(
            rewritten,
            r#"const endpoint = "/app/qwenpaw/web/api" + "/auth/status";"#
        );
        assert_eq!(rewrite_javascript(&rewritten), rewritten);
    }

    #[test]
    fn javascript_preserves_gateway_basename_for_upstream_router() {
        let source = r#"function Qa(e){return Ai&&(e===Ai||e.startsWith(`${Ai}/`))?Ai:void 0}"#;
        let rewritten = rewrite_javascript(source);
        assert!(rewritten
            .contains(r#"return /^\/app\/qwenpaw\/web(?:\/|$)/.test(e)?"/app/qwenpaw/web":Ai&&"#));
        assert_eq!(rewrite_javascript(&rewritten), rewritten);
    }

    #[test]
    fn vite_dependencies_keep_relative_mount_paths_for_the_preload_loader() {
        let sample = include_str!("testdata/qwenpaw-2.2.2.post1-entry.js").replace(
            r#""assets/ui-vendor-UcHCQ9Ai.css""#,
            r#""assets/ui-vendor-UcHCQ9Ai.css","/assets/root-only.js","/app/qwenpaw/web/assets/already-prefixed.css""#,
        );
        let rewritten = rewrite_javascript(&sample);

        for expected in [
            r#""app/qwenpaw/web/assets/index-BIY47dPN.js""#,
            r#""app/qwenpaw/web/assets/ui-vendor-UcHCQ9Ai.css""#,
            r#""/app/qwenpaw/web/assets/root-only.js""#,
            r#""/app/qwenpaw/web/assets/already-prefixed.css""#,
        ] {
            assert!(rewritten.contains(expected), "missing {expected}");
        }
        for unexpected in [
            r#""assets/index-BIY47dPN.js""#,
            r#""assets/ui-vendor-UcHCQ9Ai.css""#,
            "/app/qwenpaw/web/app/qwenpaw/web/assets/",
            "//app/qwenpaw/web/assets/",
        ] {
            assert!(!rewritten.contains(unexpected), "unexpected {unexpected}");
        }
        let dependency = "app/qwenpaw/web/assets/ui-vendor-UcHCQ9Ai.css";
        let preload_url = format!("/{dependency}");
        assert_eq!(
            preload_url,
            "/app/qwenpaw/web/assets/ui-vendor-UcHCQ9Ai.css"
        );
        assert!(!preload_url.starts_with("//app/"));
        assert_eq!(rewrite_javascript(&rewritten), rewritten);
    }
}
