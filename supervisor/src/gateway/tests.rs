use super::*;
use crate::api::test_support::{free_port, TestDir};
use std::io::Write;
use std::os::unix::{
    fs::{symlink, PermissionsExt},
    net::{UnixListener as StdUnixListener, UnixStream},
};

#[test]
fn gateway_peer_trust_uses_root_uid_and_only_allows_test_fallback_for_nonroot() {
    let helper = Path::new("/tmp/gateway-helper");
    assert!(trusted_gateway_peer(0, None, None));
    assert!(trusted_gateway_peer(
        0,
        Some(Path::new("/tmp/unrelated")),
        Some(helper)
    ));
    assert!(!trusted_gateway_peer(1000, None, None));
    assert!(!trusted_gateway_peer(1000, None, Some(helper)));
    assert!(trusted_gateway_peer(1000, Some(helper), Some(helper)));
    assert!(!trusted_gateway_peer(
        1000,
        Some(Path::new("/tmp/unrelated")),
        Some(helper)
    ));
}

#[test]
fn path_requires_the_observed_full_prefix_and_never_double_strips() {
    assert_eq!(normalize_path(PREFIX).unwrap(), "/");
    assert_eq!(normalize_path(&format!("{PREFIX}/")).unwrap(), "/");
    assert_eq!(
        normalize_path(&format!("{PREFIX}/api/status")).unwrap(),
        "/api/status"
    );
    for path in ["/status", "/", "/app/qwenpaw-other/api/status"] {
        assert_eq!(normalize_path(path), Err(StatusCode::NOT_FOUND));
    }
    for (path, tail) in [
        ("/app/qwenpaw/../var", "/../var"),
        ("/app/qwenpaw//api/status", "//api/status"),
    ] {
        assert_eq!(normalize_path(path).unwrap(), tail);
    }
    for path in ["/app/qwenpaw/api/%73tatus", "/app/qwenpaw/api/a//b"] {
        assert_eq!(normalize_path(path), Err(StatusCode::BAD_REQUEST));
    }
    assert_eq!(
        normalize_path("/app/qwenpaw/app/qwenpaw").unwrap(),
        "/app/qwenpaw"
    );
}

fn admin_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-trim-userid", HeaderValue::from_static("1000"));
    headers.insert("x-trim-username", HeaderValue::from_static("test-admin"));
    headers.insert("x-trim-isadmin", HeaderValue::from_static("true"));
    headers
}

fn peer() -> Peer {
    let pid = std::process::id() as i32;
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    Peer {
        uid: 0,
        pid,
        starttime: crate::service::parse_starttime(&stat).unwrap(),
    }
}

fn stale_peer(uid: u32) -> Peer {
    Peer {
        uid,
        pid: i32::MAX,
        starttime: 0,
    }
}

fn request(
    method: &str,
    path: &str,
    headers: HeaderMap,
    body: impl Into<Body>,
    accepted: Option<Peer>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(body.into())
        .unwrap();
    *request.headers_mut() = headers;
    request.extensions_mut().insert(accepted);
    request
}

fn fixture(label: &str) -> (TestDir, AppState, Router) {
    let dir = TestDir::new(label);
    let mut state = dir.state(free_port());
    state.token = "server-internal-token-not-for-browsers".into();
    let www = dir.0.join("www");
    fs::create_dir(&www).unwrap();
    fs::write(www.join("index.html"), b"<html>gateway fixture</html>").unwrap();
    fs::create_dir(www.join("assets")).unwrap();
    fs::write(www.join("assets/test.js"), b"/* fixture */").unwrap();
    let router = build_router(state.clone(), www, None);
    (dir, state, router)
}

#[tokio::test]
async fn stale_root_peer_can_read_admin_page_but_stale_nonroot_peer_cannot() {
    let (_dir, _state, router) = fixture("gateway-stale-peer");
    for (uid, expected) in [(0, StatusCode::OK), (1000, StatusCode::FORBIDDEN)] {
        let stale = stale_peer(uid);
        assert!(stale.executable().is_err());
        let response = router
            .clone()
            .oneshot(request(
                "GET",
                PREFIX,
                admin_headers(),
                Body::empty(),
                Some(stale),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "stale peer uid={uid}");
    }
}

#[tokio::test]
async fn nonroot_peer_requires_matching_explicit_test_fallback() {
    let (dir, state, _) = fixture("gateway-test-fallback");
    let mut live = peer();
    live.uid = 1000;
    let executable = std::env::current_exe().unwrap();
    for (test_exe, accepted, expected) in [
        (None, live, StatusCode::FORBIDDEN),
        (Some(dir.0.join("other")), live, StatusCode::FORBIDDEN),
        (Some(executable.clone()), live, StatusCode::OK),
        (Some(executable), stale_peer(1000), StatusCode::FORBIDDEN),
    ] {
        let response = build_router(state.clone(), dir.0.join("www"), test_exe)
            .oneshot(request(
                "GET",
                PREFIX,
                admin_headers(),
                Body::empty(),
                Some(accepted),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

#[tokio::test]
async fn static_page_issues_signed_cookie_and_preserves_head_compression_and_allowlist() {
    let (dir, state, router) = fixture("gateway-static");
    fs::write(dir.0.join("www/index.html.gz"), b"compressed-fixture").unwrap();
    for (method, accept, length, encoding) in [
        ("GET", "gzip;q=0", 28, None),
        ("HEAD", "gzip", 18, Some("gzip")),
    ] {
        let mut headers = admin_headers();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert("accept-encoding", HeaderValue::from_str(accept).unwrap());
        let response = router
            .clone()
            .oneshot(request(
                method,
                PREFIX,
                headers,
                Body::empty(),
                Some(peer()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.starts_with("__Host-qwenpaw_csrf="));
        assert!(cookie.ends_with("; Path=/; SameSite=Lax; Secure"));
        assert!(!cookie.contains(&state.token));
        assert!(!cookie.contains("Domain="));
        assert_eq!(
            response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .count(),
            1
        );
        let token = cookie.split_once(';').unwrap().0.split_once('=').unwrap().1;
        let identity = platform_identity(&admin_headers()).unwrap();
        let csrf = Csrf {
            secret: &state.token,
            identity: &identity,
            secure: true,
        };
        assert!(csrf.valid(token, now()));
        let other_identity = Identity {
            userid: "1001".into(),
            username: identity.username.clone(),
            admin: true,
        };
        assert!(!Csrf {
            identity: &other_identity,
            ..csrf
        }
        .valid(token, now()));
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            length.to_string()
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            encoding
        );
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(bytes.len(), if method == "HEAD" { 0 } else { length });
    }
    for path in [
        "/app/qwenpaw/var/supervisor-token",
        "/app/qwenpaw/app/qwenpaw",
        "/app/qwenpaw/.hidden",
        "/app/qwenpaw/api.cgi",
    ] {
        let response = router
            .clone()
            .oneshot(request(
                "GET",
                path,
                admin_headers(),
                Body::empty(),
                Some(peer()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    for path in [
        "/app/qwenpaw/%2e%2e/secret",
        "/app/qwenpaw/%252e%252e/secret",
        "/app/qwenpaw/%2fetc",
        "/app/qwenpaw/%00",
        "/app/qwenpaw/%5cetc",
    ] {
        let response = router
            .clone()
            .oneshot(request(
                "GET",
                path,
                admin_headers(),
                Body::empty(),
                Some(peer()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    fs::remove_file(dir.0.join("www/assets/test.js")).unwrap();
    symlink(&state.token, dir.0.join("www/assets/test.js")).unwrap();
    let response = router
        .oneshot(request(
            "GET",
            "/app/qwenpaw/assets/test.js",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn static_path_errors_keep_shared_representation_length_for_get_and_head() {
    let (_dir, _state, router) = fixture("gateway-static-errors");
    for (path, status, body) in [
        (
            "/app/qwenpaw/assets/../index.html",
            StatusCode::BAD_REQUEST,
            &b"Bad Request\n"[..],
        ),
        (
            "/app/qwenpaw//index.html",
            StatusCode::BAD_REQUEST,
            &b"Bad Request\n"[..],
        ),
        (
            "/app/qwenpaw/%252e%252e/secret",
            StatusCode::BAD_REQUEST,
            &b"Bad Request\n"[..],
        ),
        (
            "/app/qwenpaw/missing.js",
            StatusCode::NOT_FOUND,
            &b"Not Found\n"[..],
        ),
    ] {
        for method in ["GET", "HEAD"] {
            let response = router
                .clone()
                .oneshot(request(
                    method,
                    path,
                    admin_headers(),
                    Body::empty(),
                    Some(peer()),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{method} {path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_LENGTH)
                    .and_then(|value| value.to_str().ok()),
                Some(body.len().to_string().as_str()),
                "{method} {path}"
            );
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(
                &bytes[..],
                if method == "HEAD" { &[][..] } else { body },
                "{method} {path}"
            );
        }
    }
}

#[tokio::test]
async fn gateway_preserves_api_not_found_and_method_statuses() {
    let (_dir, _state, router) = fixture("gateway-api-errors");
    for (method, path, expected) in [
        ("GET", "/app/qwenpaw/api/unknown", StatusCode::NOT_FOUND),
        (
            "GET",
            "/app/qwenpaw/api/start",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(request(
                method,
                path,
                admin_headers(),
                Body::empty(),
                Some(peer()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{method} {path}");
    }
}

#[tokio::test]
async fn static_gateway_rejects_write_methods_without_issuing_csrf_cookies() {
    let (_dir, _state, router) = fixture("gateway-static-method");
    let response = router
        .oneshot(request(
            "POST",
            PREFIX,
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()[header::ALLOW], "GET, HEAD");
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "19");
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(
        &to_bytes(response.into_body(), 4096).await.unwrap()[..],
        b"Method Not Allowed\n"
    );
}

const WRITE_ROUTES: &[&str] = &[
    "start",
    "stop",
    "restart",
    "logs/clear",
    "kernel/install",
    "kernel/uninstall",
    "kernel/pin",
    "kernel/reset-auth",
    "port",
    "web-proxy/port",
    "settings",
    "mirror",
];
const READ_ROUTES: &[&str] = &[
    "status",
    "events",
    "logs",
    "web-proxy/port",
    "mirror",
    "kernel/install-status",
    "kernel/install-log",
    "kernel/versions",
];

#[tokio::test]
async fn all_management_routes_reject_missing_normal_forged_and_duplicate_identity_without_side_effects(
) {
    let (dir, state, router) = fixture("gateway-role-matrix");
    let marker = Path::new(&state.kernel_log);
    fs::write(marker, b"must remain").unwrap();
    let auth = dir.0.join(".qwenpaw.secret/auth.json");
    fs::create_dir_all(auth.parent().unwrap()).unwrap();
    fs::write(&auth, b"must remain").unwrap();
    for (method, routes) in [("GET", READ_ROUTES), ("POST", WRITE_ROUTES)] {
        for route in routes {
            for case in 0..6 {
                let mut headers = admin_headers();
                let mut accepted = Some(peer());
                match case {
                    0 => {
                        headers.remove("x-trim-userid");
                    }
                    1 => {
                        headers.insert("x-trim-isadmin", HeaderValue::from_static("false"));
                    }
                    2 => {
                        accepted = None;
                    }
                    3 => {
                        let mut denied = peer();
                        denied.uid = 1000;
                        accepted = Some(denied);
                    }
                    4 => {
                        headers.append("x-trim-isadmin", HeaderValue::from_static("true"));
                    }
                    _ => {
                        headers.remove("x-trim-username");
                    }
                }
                headers.insert(
                    "x-supervisor-token",
                    HeaderValue::from_str(&state.token).unwrap(),
                );
                let response = router
                    .clone()
                    .oneshot(request(
                        method,
                        &format!("{PREFIX}/api/{route}"),
                        headers,
                        r#"{"port":18089,"version":"2.0.0"}"#,
                        accepted,
                    ))
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::FORBIDDEN,
                    "{route} case {case}"
                );
            }
        }
    }
    assert_eq!(fs::read(marker).unwrap(), b"must remain");
    assert_eq!(fs::read(&auth).unwrap(), b"must remain");
    assert!(!Path::new(&state.var_dir).join("service_port").exists());
    assert!(!state.install.snapshot().running);
    assert!(!state.kernel.status_now().running);
}

fn csrf_headers(state: &AppState, secure: bool) -> (HeaderMap, String) {
    let mut headers = admin_headers();
    let identity = platform_identity(&headers).unwrap();
    let token = Csrf {
        secret: &state.token,
        identity: &identity,
        secure,
    }
    .issue(now())
    .unwrap();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("{}={token}", cookie_name(secure))).unwrap(),
    );
    if secure {
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    }
    (headers, token)
}

#[tokio::test]
async fn mirror_api_reads_writes_and_rejects_invalid_or_busy_updates_without_side_effects() {
    let (_dir, state, router) = fixture("gateway-mirror-api");
    let read = router
        .clone()
        .oneshot(request(
            "GET",
            &format!("{PREFIX}/api/mirror"),
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let payload: serde_json::Value =
        serde_json::from_slice(&to_bytes(read.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload["success"], true);
    assert_eq!(payload["mirror"], "");

    let (mut headers, token) = csrf_headers(&state, false);
    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
    let path = Path::new(&state.var_dir).join("pip-mirror");
    let mirror_request = request(
        "POST",
        &format!("{PREFIX}/api/mirror"),
        headers.clone(),
        r#"{"mirror":"https://mirror.example/simple"}"#,
        Some(peer()),
    );
    let response = router.clone().oneshot(mirror_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload["success"], true);
    assert_eq!(fs::read(&path).unwrap(), b"https://mirror.example/simple\n");
    let before_bytes = fs::read(&path).unwrap();
    let before_modified = fs::metadata(&path).unwrap().modified().unwrap();

    let response = router
        .clone()
        .oneshot(request(
            "POST",
            &format!("{PREFIX}/api/mirror"),
            headers.clone(),
            r#"{"mirror":"https://mirror.example/simple;touch /tmp/invalid"}"#,
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fs::read(&path).unwrap(), before_bytes);
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        before_modified
    );

    let _operation = state.core.try_operation().unwrap();
    let response = router
        .oneshot(request(
            "POST",
            &format!("{PREFIX}/api/mirror"),
            headers,
            r#"{"mirror":"https://later.example/simple"}"#,
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(fs::read(&path).unwrap(), before_bytes);
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        before_modified
    );
}

#[tokio::test]
async fn csrf_rejections_cover_every_mutation_and_preserve_logs() {
    let (_dir, state, router) = fixture("gateway-csrf-matrix");
    fs::write(&state.kernel_log, b"keep logs").unwrap();
    for route in WRITE_ROUTES {
        for case in 0..8 {
            let (mut headers, token) = csrf_headers(&state, true);
            let mut body = "{}".to_owned();
            match case {
                0 => {}
                1 => {
                    headers.insert("x-csrf-token", HeaderValue::from_static("wrong"));
                }
                2 => {
                    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
                    headers.remove(header::COOKIE);
                }
                3 => {
                    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
                    headers.append(header::COOKIE, headers[header::COOKIE].clone());
                }
                4 => {
                    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
                    headers.append("x-csrf-token", HeaderValue::from_str(&token).unwrap());
                }
                5 => {
                    body = format!(r#"{{"_csrf":"{token}","_csrf":"{token}"}}"#);
                }
                6 => {
                    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
                    body = format!(r#"{{"_csrf":"{token}"}}"#);
                }
                _ => {
                    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
                    headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
                }
            }
            let response = router
                .clone()
                .oneshot(request(
                    "POST",
                    &format!("{PREFIX}/api/{route}"),
                    headers,
                    body,
                    Some(peer()),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{route}, CSRF case {case}"
            );
        }
    }
    assert_eq!(fs::read(&state.kernel_log).unwrap(), b"keep logs");
    assert!(!state.install.snapshot().running);
}

#[tokio::test]
async fn csrf_accepts_header_json_form_and_query_with_host_rewriting_and_overwrites_internal_token()
{
    let (_dir, state, router) = fixture("gateway-csrf-positive");
    for secure in [false, true] {
        for source in 0..4 {
            fs::write(&state.kernel_log, b"clear me").unwrap();
            let (mut headers, token) = csrf_headers(&state, secure);
            headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1"));
            headers.insert(
                header::ORIGIN,
                HeaderValue::from_static("https://nas.example.test"),
            );
            headers.insert(
                header::REFERER,
                HeaderValue::from_static("https://nas.example.test/app/qwenpaw/"),
            );
            headers.insert(
                "x-supervisor-token",
                HeaderValue::from_static("attacker-value"),
            );
            let mut path = format!("{PREFIX}/api/logs/clear");
            let body = match source {
                0 => {
                    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
                    "{}".into()
                }
                1 => format!(r#"{{"_csrf":"{token}"}}"#),
                2 => {
                    headers.insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/x-www-form-urlencoded"),
                    );
                    format!("_csrf={token}")
                }
                _ => {
                    path.push_str(&format!("?_csrf={token}"));
                    "{}".into()
                }
            };
            let response = router
                .clone()
                .oneshot(request("POST", &path, headers, body, Some(peer())))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "source {source}, secure {secure}"
            );
            assert!(!response.headers().contains_key("x-supervisor-token"));
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains(&state.token));
            assert_eq!(fs::read(&state.kernel_log).unwrap(), b"");
        }
    }
}

#[tokio::test]
async fn csrf_origin_policy_rejects_cross_site_and_malformed_forwarded_context() {
    let (_dir, state, router) = fixture("gateway-origin-policy");
    let mirror = Path::new(&state.var_dir).join("pip-mirror");
    fs::write(&mirror, b"baseline\n").unwrap();
    let (mut base, token) = csrf_headers(&state, true);
    base.insert(header::HOST, HeaderValue::from_static("127.0.0.1"));
    base.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://nas.example.test"),
    );
    base.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());

    let mut same_origin = base.clone();
    same_origin.insert(
        header::REFERER,
        HeaderValue::from_static("https://nas.example.test/app/qwenpaw/"),
    );
    let mut forwarded = base.clone();
    forwarded.insert(
        "x-forwarded-host",
        HeaderValue::from_static("nas.example.test"),
    );
    let mut cross_site = base.clone();
    cross_site.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://attacker.example.test"),
    );
    let mut malicious_forwarded = base.clone();
    malicious_forwarded.insert(
        "x-forwarded-host",
        HeaderValue::from_static("attacker.example.test"),
    );
    let mut duplicate_origin = same_origin.clone();
    duplicate_origin.append(
        header::ORIGIN,
        HeaderValue::from_static("https://nas.example.test"),
    );
    let mut duplicate_referer = same_origin.clone();
    duplicate_referer.append(
        header::REFERER,
        HeaderValue::from_static("https://nas.example.test/app/qwenpaw/"),
    );
    let mut referer_mismatch = base.clone();
    referer_mismatch.insert(
        header::REFERER,
        HeaderValue::from_static("https://attacker.example.test/app/qwenpaw/"),
    );
    let mut malformed_referer = base.clone();
    malformed_referer.insert(header::REFERER, HeaderValue::from_static("not-a-url"));
    let mut cases = vec![
        (
            "same-origin Origin and Referer with rewritten Host",
            same_origin.clone(),
            StatusCode::OK,
        ),
        (
            "legal forwarded host and proto without Referer",
            forwarded,
            StatusCode::OK,
        ),
        (
            "cross-site Origin without Sec-Fetch-Site or Referer",
            cross_site,
            StatusCode::FORBIDDEN,
        ),
        (
            "missing Referer without forwarded host",
            base.clone(),
            StatusCode::FORBIDDEN,
        ),
        (
            "malicious forwarded host",
            malicious_forwarded,
            StatusCode::FORBIDDEN,
        ),
        ("duplicate Origin", duplicate_origin, StatusCode::FORBIDDEN),
        (
            "duplicate Referer",
            duplicate_referer,
            StatusCode::FORBIDDEN,
        ),
        (
            "mismatched Referer",
            referer_mismatch,
            StatusCode::FORBIDDEN,
        ),
        (
            "malformed Referer",
            malformed_referer,
            StatusCode::FORBIDDEN,
        ),
    ];
    for (label, origin) in [
        ("null Origin", "null"),
        ("malformed Origin", "not-an-origin"),
        ("Origin with path", "https://nas.example.test/path"),
        ("Origin with userinfo", "https://user@nas.example.test"),
        ("non-http Origin", "ftp://nas.example.test"),
    ] {
        let mut headers = same_origin.clone();
        headers.insert(header::ORIGIN, HeaderValue::from_static(origin));
        cases.push((label, headers, StatusCode::FORBIDDEN));
    }

    for (label, headers, expected) in cases {
        let response = router
            .clone()
            .oneshot(request(
                "POST",
                &format!("{PREFIX}/api/mirror"),
                headers,
                r#"{"mirror":"https://mirror.example/simple"}"#,
                Some(peer()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{label}");
        if expected == StatusCode::OK {
            assert_eq!(
                fs::read(&mirror).unwrap(),
                b"https://mirror.example/simple\n",
                "{label}"
            );
            fs::write(&mirror, b"baseline\n").unwrap();
        } else {
            assert_eq!(fs::read(&mirror).unwrap(), b"baseline\n", "{label}");
        }
    }
}

#[test]
fn csrf_signatures_are_user_scheme_secret_and_time_bound() {
    let identity = platform_identity(&admin_headers()).unwrap();
    let csrf = Csrf {
        secret: "secret-a",
        identity: &identity,
        secure: true,
    };
    let token = csrf.issue(1000).unwrap();
    assert!(csrf.valid(&token, 1000));
    assert!(!csrf.valid(&token, 999));
    assert!(!csrf.valid(&token, 1000 + CSRF_LIFETIME + 1));
    assert!(!Csrf {
        secret: "secret-b",
        identity: &identity,
        secure: true
    }
    .valid(&token, 1000));
    assert!(!Csrf {
        secret: "secret-a",
        identity: &identity,
        secure: false
    }
    .valid(&token, 1000));
    let other = Identity {
        userid: "1001".into(),
        username: identity.username.clone(),
        admin: true,
    };
    assert!(!Csrf {
        secret: "secret-a",
        identity: &other,
        secure: true
    }
    .valid(&token, 1000));
    assert!(!csrf.valid("attacker.attacker.attacker", 1000));
}

#[test]
fn bind_reuses_control_safety_and_only_cleans_its_own_inode() {
    let dir = TestDir::new("gateway-bind");
    let path = dir.0.join("gateway.sock");
    fs::write(&path, b"sentinel").unwrap();
    assert!(bind(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"sentinel");
    fs::remove_file(&path).unwrap();
    symlink(dir.0.join("missing"), &path).unwrap();
    assert!(bind(&path).is_err());
    fs::remove_file(&path).unwrap();
    let bound = bind(&path).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let identity = SocketIdentity::from_path(&path).unwrap();
    assert!(bind(&path).is_err());
    drop(bound);
    let replacement = bind(&path).unwrap();
    let replacement_identity = SocketIdentity::from_path(&path).unwrap();
    identity.remove_if_owned(&path);
    assert!(path.exists());
    drop(replacement);
    replacement_identity.remove_if_owned(&path);
    assert!(!path.exists());
}

#[test]
fn bind_reclaims_an_independently_stale_owned_gateway_socket() {
    use std::os::unix::fs::OpenOptionsExt;

    let dir = TestDir::new("gateway-independent-stale-socket");
    let path = dir.0.join("gateway.sock");
    let stale = StdUnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let old = fs::symlink_metadata(&path).unwrap();
    let old_inode = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .unwrap();
    drop(stale);

    let bound = bind(&path).unwrap();
    let current = fs::symlink_metadata(&path).unwrap();
    assert!(current.file_type().is_socket());
    assert_eq!(current.uid(), unsafe { libc::geteuid() });
    assert_ne!((old.dev(), old.ino()), (current.dev(), current.ino()));
    drop(bound);
    drop(old_inode);
    assert!(!path.exists());
}

#[test]
fn bind_rejects_an_active_gateway_socket_without_replacing_it() {
    let dir = TestDir::new("gateway-active-socket");
    let path = dir.0.join("gateway.sock");
    let listener = StdUnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let original = fs::symlink_metadata(&path).unwrap();

    assert!(matches!(
        bind(&path),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse
    ));
    let current = fs::symlink_metadata(&path).unwrap();
    assert_eq!(
        (original.dev(), original.ino()),
        (current.dev(), current.ino())
    );
    assert!(current.file_type().is_socket());

    let client = UnixStream::connect(&path).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    drop((client, accepted));
    assert_eq!((original.dev(), original.ino()), {
        let current = fs::symlink_metadata(&path).unwrap();
        (current.dev(), current.ino())
    });
}

fn uds_response_headers(stream: &mut UnixStream) -> String {
    use std::io::Read;
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        response.push(byte[0]);
        assert!(
            response.len() <= 16 * 1024,
            "HTTP headers exceeded fixture limit"
        );
    }
    String::from_utf8(response).unwrap()
}

#[tokio::test]
async fn bind_to_start_socket_replacement_is_rejected_and_preserved() {
    let dir = TestDir::new("gateway-bind-start-replacement");
    let path = dir.0.join("gateway.sock");
    let bound = bind(&path).unwrap();
    let original = fs::symlink_metadata(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let replacement = StdUnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let replacement_identity = fs::symlink_metadata(&path).unwrap();
    assert_ne!(
        (original.dev(), original.ino()),
        (replacement_identity.dev(), replacement_identity.ino())
    );

    let state = dir.state(free_port());
    let started = match start(
        bound,
        state,
        Path::new("/tmp/target-supervisor"),
        Path::new("/tmp/cmd/qwenpaw-supervisor"),
        Path::new("/tmp/var/qwenpaw-supervisor-helper"),
    ) {
        Ok(gateway) => {
            gateway.shutdown().await;
            false
        }
        Err(_) => true,
    };
    let preserved = fs::symlink_metadata(&path).is_ok_and(|current| {
        current.file_type().is_socket()
            && (current.dev(), current.ino())
                == (replacement_identity.dev(), replacement_identity.ino())
    });
    assert!(
        started && preserved,
        "start must reject the replaced path and preserve the replacement socket"
    );
    drop(replacement);
}

#[test]
fn gateway_path_cannot_alias_private_socket() {
    let dir = TestDir::new("gateway-socket-path");
    let private = dir.0.join("supervisor-control.sock");
    assert!(validate_socket_path(&private, &private).is_err());
    let linked = dir.0.join("alias");
    symlink(&dir.0, &linked).unwrap();
    assert!(validate_socket_path(&linked.join("supervisor-control.sock"), &private).is_err());
    assert!(validate_socket_path(Path::new("relative.sock"), &private).is_err());
    assert!(validate_socket_path(Path::new(""), &private).is_err());
    assert!(validate_socket_path(&dir.0.join("gateway.sock"), &private).is_ok());
}

async fn uds_request(path: PathBuf, request: String) -> String {
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn real_uds_peer_trust_and_authenticated_probe_follow_effective_uid() {
    let (dir, state, _) = fixture("gateway-real-uds");
    let path = dir.0.join("gateway.sock");
    let BoundGateway { listener, cleanup } = bind(&path).unwrap();
    let listener = tokio::net::UnixListener::from_std(listener).unwrap();
    let router = build_router(state.clone(), dir.0.join("www"), None);
    let (tx, rx) = watch::channel(false);
    let worker = tokio::spawn(serve(listener, cleanup, router, rx));
    let forged = uds_request(path.clone(), format!("GET {PREFIX}/api/status HTTP/1.1\r\nHost: localhost\r\nX-Trim-Userid: 1000\r\nX-Trim-Username: admin\r\nX-Trim-Isadmin: true\r\nConnection: close\r\n\r\n")).await;
    let expected = if unsafe { libc::geteuid() } == 0 {
        "HTTP/1.1 200"
    } else {
        "HTTP/1.1 403"
    };
    assert!(
        forged.starts_with(expected),
        "unexpected peer response: {forged}"
    );
    let no_token = uds_request(
        path.clone(),
        format!("GET {READY_PATH} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert!(no_token.starts_with("HTTP/1.1 403"));
    let token_file = dir.0.join("probe-token");
    fs::write(&token_file, &state.token).unwrap();
    let probe_path = path.clone();
    let ready = tokio::task::spawn_blocking(move || {
        readiness(
            &probe_path,
            token_file.to_str().unwrap(),
            std::process::id() as i32,
        )
    })
    .await
    .unwrap();
    assert_eq!(ready, Ok(true));
    tx.send(true).unwrap();
    worker.await.unwrap();
    assert!(!path.exists());
    drop(bind(&path).unwrap());
}

#[test]
fn csrf_duplicates_and_scheme_fallback_are_rejected() {
    for cookie in [
        "qwenpaw_csrf=first; qwenpaw_csrf=second",
        "qwenpaw_csrf; qwenpaw_csrf=second",
        "__Host-qwenpaw_csrf=first; __Host-qwenpaw_csrf=second",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        assert!(selected_cookie(&headers, true).is_err());
        assert!(selected_cookie(&headers, false).is_err());
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("qwenpaw_csrf=legacy"),
    );
    assert_eq!(selected_cookie(&headers, true), Ok(None));
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("__Host-qwenpaw_csrf=secure"),
    );
    assert_eq!(selected_cookie(&headers, false), Ok(None));
    for (query, json) in [
        ("_csrf=a&_csrf=b", "{}"),
        ("%5fcsrf=a&_csrf=b", "{}"),
        ("_csrf=a", r#"{"_csrf":"b"}"#),
        ("", r#"{"_csrf":"a","\u005fcsrf":"b"}"#),
    ] {
        assert!(csrf_input(
            &HeaderMap::new(),
            Some(query),
            &mut json.as_bytes().to_vec()
        )
        .is_err());
    }
}

#[test]
fn readiness_never_sends_a_token_to_an_unexpected_socket_peer() {
    use std::io::Read;
    let dir = TestDir::new("gateway-probe-credentials");
    let path = dir.0.join("probe.sock");
    let BoundGateway {
        listener,
        cleanup: _cleanup,
    } = bind(&path).unwrap();
    let token_file = dir.0.join("token");
    fs::write(&token_file, "supervisor-secret-not-for-another-peer").unwrap();
    assert!(readiness(&path, token_file.to_str().unwrap(), i32::MAX).is_err());
    let (mut accepted, _) = listener.accept().unwrap();
    let mut received = Vec::new();
    accepted.read_to_end(&mut received).unwrap();
    assert!(received.is_empty());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(readiness(
        &path,
        token_file.to_str().unwrap(),
        std::process::id() as i32
    )
    .is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn real_uds_sse_quota_keeps_status_and_authenticated_readiness_available() {
    use std::io::Write;

    const EXPECTED_SSE_LIMIT: usize = 8;
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "real gateway-peer test requires the isolated root test environment"
    );
    let (dir, state, _) = fixture("gateway-sse-quota");
    let path = dir.0.join("gateway-sse.sock");
    let BoundGateway { listener, cleanup } = bind(&path).unwrap();
    let listener = tokio::net::UnixListener::from_std(listener).unwrap();
    let router = build_router(state.clone(), dir.0.join("www"), None);
    let (tx, rx) = watch::channel(false);
    let worker = tokio::spawn(serve(listener, cleanup, router, rx));
    let mut streams = Vec::new();
    let admin = "X-Trim-Userid: 1000\r\nX-Trim-Username: test-admin\r\nX-Trim-Isadmin: true\r\n";
    for index in 0..=EXPECTED_SSE_LIMIT {
        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .write_all(format!("GET {PREFIX}/api/events HTTP/1.1\r\nHost: localhost\r\n{admin}Connection: keep-alive\r\n\r\n").as_bytes())
            .unwrap();
        let response = uds_response_headers(&mut stream);
        if index < EXPECTED_SSE_LIMIT {
            assert!(
                response.starts_with("HTTP/1.1 200"),
                "SSE slot {index}: {response}"
            );
            streams.push(stream);
        } else {
            assert!(
                response.starts_with("HTTP/1.1 503"),
                "SSE overflow must be retryable: {response}"
            );
            assert!(
                response.to_ascii_lowercase().contains("retry-after: 1"),
                "missing Retry-After: {response}"
            );
        }
    }

    let status = uds_request(
        path.clone(),
        format!("GET {PREFIX}/api/status HTTP/1.1\r\nHost: localhost\r\n{admin}Connection: close\r\n\r\n"),
    )
    .await;
    assert!(
        status.starts_with("HTTP/1.1 200"),
        "status must retain non-SSE capacity: {status}"
    );

    let token_file = dir.0.join("sse-probe-token");
    fs::write(&token_file, &state.token).unwrap();
    assert_eq!(
        readiness(
            &path,
            token_file.to_str().unwrap(),
            std::process::id() as i32
        ),
        Ok(true)
    );

    drop(streams);
    tx.send(true).unwrap();
    worker.await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn real_uds_connection_limit_returns_retryable_http_instead_of_reset() {
    let (dir, state, _) = fixture("gateway-connection-quota");
    let path = dir.0.join("gateway-cap.sock");
    let BoundGateway { listener, cleanup } = bind(&path).unwrap();
    let listener = tokio::net::UnixListener::from_std(listener).unwrap();
    let router = build_router(state, dir.0.join("www"), None);
    let (tx, rx) = watch::channel(false);
    let worker = tokio::spawn(serve(listener, cleanup, router, rx));
    let mut pending = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .unwrap();
        pending.push(stream);
    }
    let rejected = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        response
    })
    .await
    .unwrap();
    assert!(
        rejected.starts_with("HTTP/1.1 503"),
        "connection overflow must return HTTP 503 instead of reset: {rejected:?}"
    );
    assert!(
        rejected.to_ascii_lowercase().contains("retry-after: 1"),
        "missing Retry-After: {rejected}"
    );

    drop(pending);
    tx.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test]
async fn actual_unix_http_uses_platform_identity_and_signed_double_submit_csrf() {
    let (dir, state, _) = fixture("gateway-http-e2e");
    fs::write(&state.kernel_log, b"do not clear until csrf passes").unwrap();
    let path = dir.0.join("gateway-e2e.sock");
    let BoundGateway { listener, cleanup } = bind(&path).unwrap();
    let listener = tokio::net::UnixListener::from_std(listener).unwrap();
    let router = build_router(state.clone(), dir.0.join("www"), None);
    let (tx, rx) = watch::channel(false);
    let worker = tokio::spawn(serve(listener, cleanup, router, rx));
    let admin = "X-Trim-Userid: 1000\r\nX-Trim-Username: test-admin\r\nX-Trim-Isadmin: true\r\n";
    let page = uds_request(
        path.clone(),
        format!("GET {PREFIX} HTTP/1.1\r\nHost: localhost\r\n{admin}Connection: close\r\n\r\n"),
    )
    .await;
    let status_line = page.lines().next().unwrap();
    if unsafe { libc::geteuid() } == 0 {
        assert!(status_line.starts_with("HTTP/1.1 200"), "{page}");
        let cookie = page
            .lines()
            .find(|line| line.starts_with("set-cookie:") || line.starts_with("Set-Cookie:"))
            .unwrap();
        let cookie = cookie.split_once(':').unwrap().1.trim();
        let (name, token) = cookie.split_once(';').unwrap().0.split_once('=').unwrap();
        assert_eq!(name, CSRF_COOKIE);
        assert!(!token.contains(&state.token));

        let denied = uds_request(path.clone(), format!("GET {PREFIX}/api/status HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nX-Trim-Userid: 1001\r\nX-Trim-Username: regular\r\nX-Trim-Isadmin: false\r\nConnection: close\r\n\r\n")).await;
        assert!(denied.starts_with("HTTP/1.1 403"));
        let status = uds_request(path.clone(), format!("GET {PREFIX}/api/status HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n{admin}X-Supervisor-Token: forged\r\nConnection: close\r\n\r\n")).await;
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        assert!(!status
            .to_ascii_lowercase()
            .contains("\r\ncontent-encoding:"));
        assert!(!status.contains(&state.token));

        let denied_write = uds_request(path.clone(), format!("POST {PREFIX}/api/logs/clear HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n{admin}Cookie: {name}={token}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}" )).await;
        assert!(denied_write.starts_with("HTTP/1.1 403"));
        assert_eq!(
            fs::read(&state.kernel_log).unwrap(),
            b"do not clear until csrf passes"
        );
        let cleared = uds_request(path.clone(), format!("POST {PREFIX}/api/logs/clear HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\n{admin}Cookie: {name}={token}\r\nX-CSRF-Token: {token}\r\nX-Supervisor-Token: forged\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}" )).await;
        assert!(cleared.starts_with("HTTP/1.1 200"), "{cleared}");
        assert!(!cleared.contains(&state.token));
        assert_eq!(fs::read(&state.kernel_log).unwrap(), b"");
    } else {
        assert!(
            status_line.starts_with("HTTP/1.1 403"),
            "non-root peer must not impersonate fnOS gateway: {page}"
        );
    }
    tx.send(true).unwrap();
    worker.await.unwrap();
    assert!(!path.exists());
}

const COMPRESSED_JS: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 75, 206, 207, 43, 206, 207, 73, 213, 203, 201, 79, 215, 80,
    74, 206, 207, 45, 40, 74, 45, 46, 78, 77, 81, 210, 180, 6, 0, 150, 11, 87, 17, 26, 0, 0, 0,
];

#[derive(Debug)]
struct CapturedWebRequest {
    method: Method,
    uri: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn capture_web_upstream(
    axum::extract::State(sender): axum::extract::State<
        tokio::sync::mpsc::UnboundedSender<CapturedWebRequest>,
    >,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
    let uri = parts.uri.path_and_query().map_or_else(
        || parts.uri.path().to_owned(),
        |value| value.as_str().to_owned(),
    );
    let _ = sender.send(CapturedWebRequest {
        method: parts.method.clone(),
        uri: uri.clone(),
        headers: parts.headers,
        body: bytes.to_vec(),
    });

    if uri == "/" {
        return upstream_sample(
            include_bytes!("testdata/qwenpaw-2.2.2.post1-index.html"),
            "text/html; charset=utf-8",
        );
    }
    if uri == "/assets/auth-230-fixture.js" {
        return upstream_sample(
            br#"const endpoint = "/api" + "/auth/status";"#,
            "text/javascript; charset=utf-8",
        );
    }
    if uri == "/assets/compressed-fixture.js" {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/javascript; charset=utf-8")
            .header(header::CONTENT_ENCODING, "gzip")
            .header(header::CONTENT_LENGTH, COMPRESSED_JS.len())
            .body(Body::from(COMPRESSED_JS))
            .unwrap();
    }
    if uri == "/assets/index-BpFaGbCq.js" {
        return upstream_sample(
            include_bytes!("testdata/qwenpaw-2.2.2.post1-entry.js"),
            "text/javascript; charset=utf-8",
        );
    }
    if uri == "/assets/index-BIY47dPN.js" {
        return upstream_sample(
            include_bytes!("testdata/qwenpaw-2.2.2.post1-login.js"),
            "text/javascript; charset=utf-8",
        );
    }
    if uri == "/assets/chat-vendor-DmgPIszx.css" {
        return upstream_sample(
            include_bytes!("testdata/qwenpaw-2.2.2.post1-chat.css"),
            "text/css; charset=utf-8",
        );
    }
    if uri.starts_with("/sse") {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        tokio::spawn(async move {
            if sender
                .send(Ok::<_, std::io::Error>("data: first\n\n".to_owned()))
                .await
                .is_ok()
            {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let _ = sender.send(Ok("data: second\n\n".to_owned())).await;
            }
        });
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(
                tokio_stream::wrappers::ReceiverStream::new(receiver),
            ))
            .unwrap();
    }
    if uri.starts_with("/api/console/chat") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(
                "data: {\"type\":\"message\",\"text\":\"hello\"}\n\n",
            ))
            .unwrap();
    }
    if uri.starts_with("/api/auth/login") {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::WWW_AUTHENTICATE, "Bearer realm=\"qwenpaw\"")
            .header(header::SET_COOKIE, "qwenpaw_csrf=internal; Path=/")
            .header(
                header::SET_COOKIE,
                "qwenpaw_session=upstream-session; Path=/; HttpOnly; SameSite=Lax",
            )
            .body(Body::from(r#"{"detail":"upstream unauthorized"}"#))
            .unwrap();
    }

    let status = if uri.starts_with("/not-found") {
        StatusCode::NOT_FOUND
    } else if uri.starts_with("/upstream-unavailable") {
        StatusCode::SERVICE_UNAVAILABLE
    } else if uri.starts_with("/upstream-bad-gateway") {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::ACCEPTED
    };
    let body = if parts.method == Method::HEAD {
        ""
    } else {
        "upstream"
    };
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::CONTENT_LENGTH, "8")
        .header("x-supervisor-token", "upstream-secret")
        .header("x-trim-userid", "2000")
        .header("connection", "x-hop")
        .header("x-hop", "must-not-escape")
        .header(header::SET_COOKIE, "qwenpaw_csrf=upstream; Path=/")
        .header(
            header::SET_COOKIE,
            "qwenpaw_session=app-session; Path=/; HttpOnly; SameSite=Lax",
        );
    builder.body(Body::from(body)).unwrap()
}

fn upstream_sample(body: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, body.len())
        .header(header::ETAG, "\"upstream-sample\"")
        .header(header::ACCEPT_RANGES, "bytes")
        .body(Body::from(body))
        .unwrap()
}

async fn start_web_upstream(
    state: &AppState,
) -> (
    tokio::sync::mpsc::UnboundedReceiver<CapturedWebRequest>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    state
        .core
        .kernel
        .set_port(listener.local_addr().unwrap().port());
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new()
        .fallback(capture_web_upstream)
        .with_state(sender);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (receiver, server)
}

async fn start_tcp_web_proxy(
    state: AppState,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, web_tcp_router(state)).await.unwrap();
    });
    (addr, server)
}

async fn tcp_web_request(
    addr: std::net::SocketAddr,
    method: Method,
    path: &str,
    headers: HeaderMap,
    body: Body,
) -> hyper::Response<hyper::body::Incoming> {
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector);
    let mut request = Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"))
        .body(body)
        .unwrap();
    *request.headers_mut() = headers;
    tokio::time::timeout(Duration::from_secs(3), client.request(request))
        .await
        .unwrap()
        .unwrap()
}

fn root_proxy_state(app: AppState) -> GatewayState {
    let connector = hyper_util::client::legacy::connect::HttpConnector::new();
    let web_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(connector);
    GatewayState {
        api: api::build_router(app.clone()),
        app,
        www_root: PathBuf::new(),
        test_exe: None,
        supervisor_executables: Vec::new(),
        sse_connections: Arc::new(Semaphore::new(MAX_SSE_CONNECTIONS)),
        web_client,
    }
}

fn root_auth_headers() -> HeaderMap {
    let mut headers = admin_headers();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer upstream-token"),
    );
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static(
            "qwenpaw_csrf=internal; __Host-qwenpaw_csrf=host; supervisor-token=secret; qwenpaw_session=retained",
        ),
    );
    headers.insert("x-supervisor-token", HeaderValue::from_static("secret"));
    headers.insert("x-csrf-token", HeaderValue::from_static("internal"));
    headers
}

#[tokio::test]
async fn tcp_root_serves_html_at_root_with_guarded_chinese_default() {
    let (_dir, state, _) = fixture("tcp-web-root-html");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let (addr, server) = start_tcp_web_proxy(state).await;
    let response = tcp_web_request(addr, Method::GET, "/", HeaderMap::new(), Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = to_bytes(Body::new(response.into_body()), 128 * 1024)
        .await
        .unwrap();
    let html = std::str::from_utf8(&html).unwrap();
    assert!(html.contains(
        "if(localStorage.getItem('language')===null)localStorage.setItem('language','zh')"
    ));
    assert!(html.contains("src=\"/assets/index-BpFaGbCq.js\""));
    assert!(!html.contains("/app/qwenpaw/web/assets/"));
    assert_eq!(captured.recv().await.unwrap().uri, "/");
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn tcp_root_forwards_assets_without_rewriting() {
    let (_dir, state, _) = fixture("tcp-web-root-assets");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let (addr, server) = start_tcp_web_proxy(state).await;
    let path = "/assets/index-BpFaGbCq.js";
    let response = tcp_web_request(addr, Method::GET, path, HeaderMap::new(), Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    let body = to_bytes(Body::new(response.into_body()), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        &body[..],
        include_bytes!("testdata/qwenpaw-2.2.2.post1-entry.js")
    );
    let received = captured.recv().await.unwrap();
    assert_eq!(received.uri, path);
    assert!(!received.headers.contains_key(header::ACCEPT_ENCODING));
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn tcp_root_compresses_uncompressed_upstream_javascript_as_gzip() {
    let (dir, state, _) = fixture("tcp-web-root-gzip-js");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let (addr, server) = start_tcp_web_proxy(state).await;
    let path = "/assets/index-BpFaGbCq.js";
    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

    let response = tcp_web_request(addr, Method::GET, path, headers, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    let compressed = to_bytes(Body::new(response.into_body()), 2 * 1024 * 1024)
        .await
        .unwrap();
    let received = captured.recv().await.unwrap();
    assert_eq!(received.uri, path);
    assert_eq!(received.headers[header::ACCEPT_ENCODING], "gzip");

    let gzip_file = dir.0.join("proxy-response.js.gz");
    fs::write(&gzip_file, &compressed).unwrap();
    let decoded = std::process::Command::new("gzip")
        .args(["-d", "-c"])
        .arg(gzip_file)
        .output()
        .expect("gzip executable is required to verify the response");
    assert!(
        decoded.status.success(),
        "gzip decode failed: {}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(
        decoded.stdout,
        include_bytes!("testdata/qwenpaw-2.2.2.post1-entry.js")
    );
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn tcp_sse_with_gzip_accepted_stays_uncompressed_and_streaming() {
    use http_body_util::BodyExt;

    let (_dir, state, _) = fixture("tcp-web-gzip-sse");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    state
        .core
        .kernel
        .set_port(listener.local_addr().unwrap().port());
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<&'static str, std::io::Error>>(1);
    sender.send(Ok("data: first\n\n")).await.unwrap();
    let body = Arc::new(Mutex::new(Some(Body::from_stream(
        tokio_stream::wrappers::ReceiverStream::new(receiver),
    ))));
    let (observed, mut captured) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new().fallback(move |request: Request<Body>| {
        let body = Arc::clone(&body);
        let observed = observed.clone();
        async move {
            observed
                .send((request.uri().to_string(), request.headers().clone()))
                .unwrap();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(body.lock().await.take().unwrap())
                .unwrap()
        }
    });
    let upstream = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (addr, server) = start_tcp_web_proxy(state).await;
    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let mut response = tcp_web_request(addr, Method::GET, "/sse", headers, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));

    let first = tokio::time::timeout(Duration::from_secs(3), async {
        let mut first = Vec::new();
        while !first.ends_with(b"\n\n") {
            let frame = response
                .body_mut()
                .frame()
                .await
                .expect("SSE stream closed before first event")
                .expect("SSE stream failed");
            if let Ok(data) = frame.into_data() {
                first.extend_from_slice(&data);
            }
        }
        first
    })
    .await
    .expect("first SSE event was buffered");
    assert_eq!(first, b"data: first\n\n");

    sender.send(Ok("data: second\n\n")).await.unwrap();
    drop(sender);
    let rest = tokio::time::timeout(
        Duration::from_secs(3),
        to_bytes(Body::new(response.into_body()), 4096),
    )
    .await
    .expect("remaining SSE event was buffered")
    .unwrap();
    assert_eq!(&rest[..], b"data: second\n\n");
    let (path, upstream_headers) = captured.recv().await.unwrap();
    assert_eq!(path, "/sse");
    assert_eq!(upstream_headers[header::ACCEPT_ENCODING], "gzip");
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn tcp_root_forwards_upstream_auth_and_scrubs_gateway_credentials() {
    let (_dir, state, _) = fixture("tcp-web-root-auth");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let (addr, server) = start_tcp_web_proxy(state).await;
    let path = "/api/auth/login?next=chat";
    let response = tcp_web_request(
        addr,
        Method::POST,
        path,
        root_auth_headers(),
        Body::from("{}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers()[header::WWW_AUTHENTICATE],
        "Bearer realm=\"qwenpaw\""
    );
    let body = to_bytes(Body::new(response.into_body()), 4096)
        .await
        .unwrap();
    assert_eq!(&body[..], br#"{"detail":"upstream unauthorized"}"#);
    let received = captured.recv().await.unwrap();
    assert_eq!(received.method, Method::POST);
    assert_eq!(received.uri, path);
    assert_eq!(
        received.headers[header::AUTHORIZATION],
        "Bearer upstream-token"
    );
    assert_eq!(received.headers[header::COOKIE], "qwenpaw_session=retained");
    assert!(!received
        .headers
        .keys()
        .any(|name| name.as_str().starts_with("x-trim-")));
    assert!(!received.headers.contains_key("x-supervisor-token"));
    assert!(!received.headers.contains_key("x-csrf-token"));
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn tcp_root_does_not_mount_supervisor_management_api() {
    let (_dir, state, _) = fixture("tcp-web-no-management");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let (addr, server) = start_tcp_web_proxy(state).await;
    let response = tcp_web_request(
        addr,
        Method::GET,
        "/api/status",
        HeaderMap::new(),
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        &to_bytes(Body::new(response.into_body()), 4096)
            .await
            .unwrap()[..],
        b"upstream"
    );
    assert_eq!(captured.recv().await.unwrap().uri, "/api/status");
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn tcp_prefixed_management_path_is_proxied_instead_of_mounted() {
    let (_dir, state, _) = fixture("tcp-web-prefixed-no-management");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let (addr, server) = start_tcp_web_proxy(state).await;
    let mut headers = root_auth_headers();
    headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let response = tcp_web_request(
        addr,
        Method::GET,
        "/app/qwenpaw/api/status",
        headers,
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    assert_eq!(
        &to_bytes(Body::new(response.into_body()), 4096)
            .await
            .unwrap()[..],
        b"upstream"
    );
    let received = captured.recv().await.unwrap();
    assert_eq!(received.uri, "/app/qwenpaw/api/status");
    assert_eq!(received.headers[header::ACCEPT_ENCODING], "gzip");
    assert!(!received.headers.contains_key("x-supervisor-token"));
    assert!(!received
        .headers
        .keys()
        .any(|name| name.as_str().starts_with("x-trim-")));
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn mounted_javascript_rewrites_concatenated_api_prefix_and_routes_auth_status() {
    let (_dir, state, router) = fixture("gateway-web-230-concatenated-api");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let response = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/assets/auth-230-fixture.js",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(response.into_body(), 4096).await.unwrap()[..],
        br#"const endpoint = "/app/qwenpaw/web/api" + "/auth/status";"#
    );
    assert_eq!(
        captured.recv().await.unwrap().uri,
        "/assets/auth-230-fixture.js"
    );

    let response = router
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/api/auth/status",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(captured.recv().await.unwrap().uri, "/api/auth/status");
    upstream.abort();
}

#[tokio::test]
async fn tcp_root_javascript_preserves_the_client_accept_encoding_header() {
    let (_dir, state, _) = fixture("tcp-web-root-js-encoding");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let (addr, server) = start_tcp_web_proxy(state).await;
    let path = "/assets/compressed-fixture.js";
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("gzip, br"),
    );

    let response = tcp_web_request(addr, Method::GET, path, headers, Body::empty()).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(
        &to_bytes(Body::new(response.into_body()), 4096)
            .await
            .unwrap()[..],
        COMPRESSED_JS
    );
    let received = tokio::time::timeout(Duration::from_secs(2), captured.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.headers[header::ACCEPT_ENCODING], "gzip, br");
    server.abort();
    upstream.abort();
}

#[tokio::test]
async fn gateway_proxy_port_api_requires_admin_and_csrf_and_switches_the_listener() {
    let (dir, mut state, _) = fixture("gateway-web-proxy-port-api");
    let (mut captured, upstream) = start_web_upstream(&state).await;
    let manager = crate::gateway::WebProxyManager::start(state.clone(), 0)
        .await
        .unwrap();
    let initial_port = manager.port().await;
    state.web_proxy = Some(Arc::new(manager.clone()));
    let router = build_router(state.clone(), dir.0.join("www"), None);
    let path = "/app/qwenpaw/api/web-proxy/port";

    let response = router
        .clone()
        .oneshot(request(
            "GET",
            path,
            HeaderMap::new(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            path,
            admin_headers(),
            r#"{"port":0}"#,
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(manager.port().await, initial_port);

    let response = router
        .clone()
        .oneshot(request(
            "GET",
            path,
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload["port"], initial_port);

    let (mut headers, token) = csrf_headers(&state, false);
    headers.insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
    let response = router
        .oneshot(request(
            "POST",
            path,
            headers,
            r#"{"port":0}"#,
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let new_port = payload["port"].as_u64().unwrap() as u16;
    assert_ne!(new_port, initial_port);
    assert_eq!(manager.port().await, new_port);
    assert_eq!(
        fs::read_to_string(Path::new(&state.var_dir).join("web_proxy_port")).unwrap(),
        format!("{new_port}\n")
    );

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], new_port));
    let response = tcp_web_request(
        addr,
        Method::GET,
        "/api/status",
        HeaderMap::new(),
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(captured.recv().await.unwrap().uri, "/api/status");
    upstream.abort();
}

#[test]
fn gateway_api_exposes_web_proxy_port_after_supervisor_startup() {
    if std::env::var_os("QWENPAW_TEST_WEB_PROXY_STARTUP_CHILD").is_some() {
        crate::serve();
        return;
    }

    use std::io::Read;
    use std::net::TcpListener;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let dir = TestDir::new("gateway-web-proxy-startup");
    let var = dir.0.join("var");
    let home = dir.0.join("home");
    let etc = dir.0.join("etc");
    let share = dir.0.join("share");
    let target = dir.0.join("target");
    let gateway = dir.0.join("gateway.sock");
    for path in [&var, &home, &etc, &share, &target] {
        fs::create_dir_all(path).unwrap();
    }
    let kernel_reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let kernel_port = kernel_reservation.local_addr().unwrap().port();
    let supervisor_reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let supervisor_port = supervisor_reservation.local_addr().unwrap().port();
    let binary = std::env::current_exe().unwrap();
    let mut command = Command::new(&binary);
    command
        .args([
            "--exact",
            "gateway::tests::gateway_api_exposes_web_proxy_port_after_supervisor_startup",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TRIM_PKGVAR", &var)
        .env("TRIM_PKGHOME", &home)
        .env("TRIM_PKGETC", &etc)
        .env("TRIM_DATA_SHARE_PATHS", &share)
        .env("TRIM_APPDEST", &target)
        .env("TRIM_SERVICE_PORT", kernel_port.to_string())
        .env("SUPERVISOR_PORT", supervisor_port.to_string())
        .env("SUPERVISOR_AUTOSTART", "0")
        .env("QWENPAW_GATEWAY_SOCKET", &gateway)
        .env("QWENPAW_WEB_PROXY_PORT", "0")
        .env("QWENPAW_TEST_WEB_PROXY_STARTUP_CHILD", "1")
        .env("QWENPAW_ENABLE_CRASH_POINT", "1")
        .env("QWENPAW_TEST_GATEWAY_PEER_EXE", &binary)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    drop(supervisor_reservation);
    let mut child = crate::api::test_support::ChildGuard(command.spawn().unwrap());
    let get = |path: &str| -> io::Result<String> {
        let mut stream = UnixStream::connect(&gateway)?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nX-Trim-Userid: 1000\r\nX-Trim-Username: test-admin\r\nX-Trim-Isadmin: true\r\nConnection: close\r\n\r\n"
        )?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if var.join("web_proxy_port").exists()
            && get("/app/qwenpaw/api/status")
                .is_ok_and(|response| response.starts_with("HTTP/1.1 200"))
        {
            break;
        }
        let log = || fs::read_to_string(var.join("supervisor.log")).unwrap_or_default();
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!(
                "supervisor exited before gateway readiness: {status}; {}",
                log()
            );
        }
        assert!(Instant::now() < deadline, "gateway not ready: {}", log());
        std::thread::sleep(Duration::from_millis(20));
    }

    let port = fs::read_to_string(var.join("web_proxy_port"))
        .unwrap()
        .trim()
        .parse::<u16>()
        .unwrap();
    assert_ne!(port, 0);
    let response = get("/app/qwenpaw/api/web-proxy/port").unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let payload: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(payload["port"], port);
}

#[tokio::test]
async fn forward_root_keeps_html_and_assets_at_root_and_guards_language_setting() {
    let (_dir, app, _) = fixture("direct-web-root-assets");
    let (mut captured, upstream) = start_web_upstream(&app).await;
    let state = root_proxy_state(app);
    let html = web_proxy::forward_root(
        &state,
        request("GET", "/", HeaderMap::new(), Body::empty(), None),
        "/",
    )
    .await;
    assert_eq!(html.status(), StatusCode::OK);
    let html = to_bytes(html.into_body(), 128 * 1024).await.unwrap();
    let html = std::str::from_utf8(&html).unwrap();
    let source = include_str!("testdata/qwenpaw-2.2.2.post1-index.html");
    let index = source.find("<script type=\"module\"").unwrap();
    let expected = format!(
        "{}<script>try{{if(localStorage.getItem('language')===null)localStorage.setItem('language','zh')}}catch{{}}</script>{}",
        &source[..index],
        &source[index..]
    );
    assert_eq!(html, expected);
    assert_eq!(captured.recv().await.unwrap().uri, "/");

    let path = "/assets/index-BpFaGbCq.js";
    let script = web_proxy::forward_root(
        &state,
        request("GET", path, HeaderMap::new(), Body::empty(), None),
        path,
    )
    .await;
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(script.into_body(), 2 * 1024 * 1024).await.unwrap()[..],
        include_bytes!("testdata/qwenpaw-2.2.2.post1-entry.js")
    );
    assert_eq!(captured.recv().await.unwrap().uri, path);
    upstream.abort();
}

#[tokio::test]
async fn forward_root_preserves_auth_response_and_filters_internal_request_headers() {
    let (_dir, app, _) = fixture("direct-web-root-auth");
    let (mut captured, upstream) = start_web_upstream(&app).await;
    let state = root_proxy_state(app);
    let path = "/api/auth/login?next=chat";
    let response = web_proxy::forward_root(
        &state,
        request("POST", path, root_auth_headers(), "{}", None),
        "/api/auth/login",
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers()[header::WWW_AUTHENTICATE],
        "Bearer realm=\"qwenpaw\""
    );
    assert_eq!(
        &to_bytes(response.into_body(), 4096).await.unwrap()[..],
        br#"{"detail":"upstream unauthorized"}"#
    );
    let received = captured.recv().await.unwrap();
    assert_eq!(received.uri, path);
    assert_eq!(
        received.headers[header::AUTHORIZATION],
        "Bearer upstream-token"
    );
    assert_eq!(received.headers[header::COOKIE], "qwenpaw_session=retained");
    assert!(!received
        .headers
        .keys()
        .any(|name| name.as_str().starts_with("x-trim-")));
    assert!(!received.headers.contains_key("x-supervisor-token"));
    assert!(!received.headers.contains_key("x-csrf-token"));
    upstream.abort();
}

#[tokio::test]
async fn forward_root_preserves_upstream_auth_cookie_root_path() {
    let (_dir, app, _) = fixture("direct-web-root-cookie");
    let (_captured, upstream) = start_web_upstream(&app).await;
    let state = root_proxy_state(app);
    let response = web_proxy::forward_root(
        &state,
        request("POST", "/api/auth/login", HeaderMap::new(), "{}", None),
        "/api/auth/login",
    )
    .await;
    let cookies: Vec<_> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .collect();
    assert_eq!(cookies.len(), 1);
    assert_eq!(
        cookies[0],
        "qwenpaw_session=upstream-session; Path=/; HttpOnly; SameSite=Lax"
    );
    upstream.abort();
}

#[tokio::test]
async fn web_proxy_forwards_http_and_scrubs_gateway_credentials() {
    let (_dir, state, router) = fixture("gateway-web-proxy-forward");
    let (mut captured, server) = start_web_upstream(&state).await;
    let mut headers = admin_headers();
    headers.insert(header::HOST, HeaderValue::from_static("internal-gateway"));
    headers.insert(
        "x-forwarded-host",
        HeaderValue::from_static("nas.example.test:8443"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    headers.insert(
        "x-forwarded-prefix",
        HeaderValue::from_static("/attacker-prefix"),
    );
    headers.insert("x-forwarded-for", HeaderValue::from_static("192.0.2.12"));
    headers.insert(
        "x-supervisor-token",
        HeaderValue::from_str(&state.token).unwrap(),
    );
    headers.insert("x-csrf-token", HeaderValue::from_static("gateway-csrf"));
    headers.insert("x-private-hop", HeaderValue::from_static("strip-me"));
    headers.insert(
        header::CONNECTION,
        HeaderValue::from_static("x-private-hop, keep-alive"),
    );
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static(
            "qwenpaw_csrf=gateway-csrf; supervisor-token=secret; qwenpaw_session=app-session",
        ),
    );
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer qwenpaw-token"),
    );

    let response = router
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/console/chat?thread=abc&stream=true",
            headers,
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers().get("x-supervisor-token"), None);
    assert_eq!(response.headers().get("x-trim-userid"), None);
    assert_eq!(response.headers().get("x-hop"), None);
    let set_cookies: Vec<_> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .collect();
    assert_eq!(set_cookies.len(), 1);
    assert_eq!(
        set_cookies[0],
        "qwenpaw_session=app-session; Path=/app/qwenpaw/web/; HttpOnly; SameSite=Lax"
    );
    assert_eq!(
        &to_bytes(response.into_body(), 4096).await.unwrap()[..],
        b"upstream"
    );

    let received = tokio::time::timeout(Duration::from_secs(2), captured.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.method, Method::GET);
    assert_eq!(received.uri, "/console/chat?thread=abc&stream=true");
    assert_eq!(
        received.headers[header::HOST],
        format!("127.0.0.1:{}", state.core.kernel.port())
    );
    assert_eq!(
        received.headers["x-forwarded-host"],
        "nas.example.test:8443"
    );
    assert_eq!(received.headers["x-forwarded-proto"], "https");
    assert_eq!(received.headers["x-forwarded-prefix"], "/app/qwenpaw/web");
    assert!(!received.headers.contains_key("x-forwarded-for"));
    assert!(!received.headers.contains_key("x-trim-userid"));
    assert!(!received.headers.contains_key("x-trim-username"));
    assert!(!received.headers.contains_key("x-trim-isadmin"));
    assert!(!received.headers.contains_key("x-supervisor-token"));
    assert!(!received.headers.contains_key("x-csrf-token"));
    assert!(!received.headers.contains_key("x-private-hop"));
    assert_eq!(
        received.headers[header::COOKIE],
        "qwenpaw_session=app-session"
    );
    assert_eq!(received.headers["authorization"], "Bearer qwenpaw-token");
    assert!(received.body.is_empty());
    server.abort();
}

#[tokio::test]
async fn web_proxy_streams_large_mutation_bodies_with_same_origin_without_gateway_csrf() {
    let (_dir, state, router) = fixture("gateway-web-proxy-post");
    let (mut captured, server) = start_web_upstream(&state).await;
    let body = vec![b'x'; 96 * 1024];
    let mut headers = admin_headers();
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://nas.example.test"),
    );
    headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    headers.insert(
        "x-forwarded-host",
        HeaderValue::from_static("nas.example.test"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );

    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/app/qwenpaw/web/upload?name=large.bin",
            headers,
            Body::from(body.clone()),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let received = tokio::time::timeout(Duration::from_secs(2), captured.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.method, Method::POST);
    assert_eq!(received.uri, "/upload?name=large.bin");
    assert_eq!(received.body, body);

    let cases = [
        (
            "invalid origin",
            "http://attacker.example.test",
            "same-origin",
            true,
        ),
        (
            "cross-site fetch",
            "http://nas.example.test",
            "cross-site",
            true,
        ),
        ("non-admin", "http://nas.example.test", "same-origin", false),
    ];
    for (label, origin, fetch_site, admin) in cases {
        let mut headers = admin_headers();
        headers.insert(header::ORIGIN, HeaderValue::from_static(origin));
        headers.insert("sec-fetch-site", HeaderValue::from_static(fetch_site));
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("nas.example.test"),
        );
        if !admin {
            headers.insert("x-trim-isadmin", HeaderValue::from_static("false"));
        }
        let response = router
            .clone()
            .oneshot(request(
                "POST",
                "/app/qwenpaw/web/upload",
                headers,
                Body::from("must-not-reach-kernel"),
                Some(peer()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{label}");
        assert!(captured.try_recv().is_err(), "{label} reached the upstream");
    }
    server.abort();
}

#[tokio::test]
async fn qwenpaw_222_post1_assets_are_rewritten_under_web_mount() {
    let (_dir, state, router) = fixture("gateway-web-proxy-qwenpaw-assets");
    let (mut captured, server) = start_web_upstream(&state).await;

    let html = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(html.status(), StatusCode::OK);
    let html_bytes = to_bytes(html.into_body(), 128 * 1024).await.unwrap();
    let html = String::from_utf8(html_bytes.to_vec()).unwrap();
    assert!(html.contains("href=\"/app/qwenpaw/web/online.svg\""));
    assert!(html.contains("src=\"/app/qwenpaw/web/assets/index-BpFaGbCq.js\""));
    assert!(html.contains("href=\"/app/qwenpaw/web/assets/chat-vendor-DmgPIszx.css\""));
    assert!(html.contains("src=\"/app/qwenpaw/web/qwenpaw.png\""));
    let language_setup = html.find("localStorage.setItem('language','zh')").unwrap();
    let module_entry = html
        .find("type=\"module\" crossorigin src=\"/app/qwenpaw/web/assets/index-BpFaGbCq.js\"")
        .unwrap();
    assert!(
        language_setup < module_entry,
        "Chinese locale must be seeded before the app module"
    );
    let html_request = captured.recv().await.unwrap();
    assert_eq!(html_request.uri, "/");
    assert_eq!(html_request.headers[header::ACCEPT_ENCODING], "identity");

    let entry = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/assets/index-BpFaGbCq.js",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(entry.status(), StatusCode::OK);
    let entry_len = entry.headers()[header::CONTENT_LENGTH]
        .to_str()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert_eq!(entry.headers().get(header::ETAG), None);
    assert_eq!(entry.headers().get(header::ACCEPT_RANGES), None);
    let entry_bytes = to_bytes(entry.into_body(), 2 * 1024 * 1024).await.unwrap();
    assert_eq!(entry_bytes.len(), entry_len);
    let entry = String::from_utf8(entry_bytes.to_vec()).unwrap();
    for expected in [
        "function ce(e){const n=\"/app/qwenpaw/web/api\"",
        "function vE(){return\"/app/qwenpaw/web/api\"}",
        "/app/qwenpaw/web/login",
        "/app/qwenpaw/web/logo-dark.svg",
        "/app/qwenpaw/web/logo-light.svg",
        "/app/qwenpaw/web/qwenpaw-avatar.gif",
        "/app/qwenpaw/web/qwenpaw.png",
        "/app/qwenpaw/web/console",
        "\"app/qwenpaw/web/assets/index-BIY47dPN.js\"",
        "\"app/qwenpaw/web/assets/ui-vendor-UcHCQ9Ai.css\"",
    ] {
        assert!(
            entry.contains(expected),
            "missing rewritten entry value: {expected}"
        );
    }
    for invalid_path in [
        "\"assets/index-BIY47dPN.js\"",
        "\"assets/ui-vendor-UcHCQ9Ai.css\"",
        "\"/app/qwenpaw/web/assets/index-BIY47dPN.js\"",
        "\"/app/qwenpaw/web/assets/ui-vendor-UcHCQ9Ai.css\"",
        "//app/qwenpaw/web/assets/",
    ] {
        assert!(
            !entry.contains(invalid_path),
            "invalid preload path: {invalid_path}"
        );
    }
    let ui_vendor_dependency = "app/qwenpaw/web/assets/ui-vendor-UcHCQ9Ai.css";
    let ui_vendor_url = format!("/{ui_vendor_dependency}");
    assert_eq!(
        ui_vendor_url,
        "/app/qwenpaw/web/assets/ui-vendor-UcHCQ9Ai.css"
    );
    assert!(!ui_vendor_url.starts_with("//app/"));
    let entry_request = captured.recv().await.unwrap();
    assert_eq!(entry_request.uri, "/assets/index-BpFaGbCq.js");
    assert_eq!(entry_request.headers[header::ACCEPT_ENCODING], "identity");

    let login = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/assets/index-BIY47dPN.js",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    let login_bytes = to_bytes(login.into_body(), 32 * 1024).await.unwrap();
    let login = String::from_utf8(login_bytes.to_vec()).unwrap();
    assert!(login.contains("/app/qwenpaw/web/logo-dark.svg"));
    assert!(login.contains("/app/qwenpaw/web/logo-light.svg"));
    assert_eq!(
        captured.recv().await.unwrap().uri,
        "/assets/index-BIY47dPN.js"
    );

    let css = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/assets/chat-vendor-DmgPIszx.css",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    let css_bytes = to_bytes(css.into_body(), 64 * 1024).await.unwrap();
    let css = String::from_utf8(css_bytes.to_vec()).unwrap();
    assert!(css.contains("url(/app/qwenpaw/web/assets/KaTeX_AMS-Regular-BQhdFMY1.woff2)"));
    assert!(!css.contains("url(/assets/"));
    assert_eq!(
        captured.recv().await.unwrap().uri,
        "/assets/chat-vendor-DmgPIszx.css"
    );

    for path in ["/assets/index-BpFaGbCq.js", "/api/console/chat"] {
        let response = router
            .clone()
            .oneshot(request(
                "GET",
                path,
                admin_headers(),
                Body::empty(),
                Some(peer()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(
            captured.try_recv().is_err(),
            "host route {path} reached app upstream"
        );
    }
    server.abort();
}

#[tokio::test]
async fn qwenpaw_chat_post_streams_sse_with_origin_and_admin_but_without_gateway_csrf() {
    let (_dir, state, router) = fixture("gateway-web-proxy-qwenpaw-chat");
    let (mut captured, server) = start_web_upstream(&state).await;
    let mut headers = admin_headers();
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://nas.example.test"),
    );
    headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    headers.insert(
        "x-forwarded-host",
        HeaderValue::from_static("nas.example.test"),
    );
    headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer qwenpaw-session-token"),
    );
    let response = tokio::time::timeout(
        Duration::from_millis(750),
        router.clone().oneshot(request(
            "POST",
            "/app/qwenpaw/web/api/console/chat?agent_id=default",
            headers,
            r#"{"message":"hello"}"#,
            Some(peer()),
        )),
    )
    .await
    .expect("chat SSE response headers must not wait for the stream to finish")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let received = captured.recv().await.unwrap();
    assert_eq!(received.method, Method::POST);
    assert_eq!(received.uri, "/api/console/chat?agent_id=default");
    assert_eq!(
        received.headers[header::AUTHORIZATION],
        "Bearer qwenpaw-session-token"
    );
    assert!(!received.headers.contains_key("x-csrf-token"));
    assert!(!received.headers.contains_key(header::COOKIE));
    assert_eq!(received.body, br#"{"message":"hello"}"#);
    assert_eq!(
        &to_bytes(response.into_body(), 4096).await.unwrap()[..],
        b"data: {\"type\":\"message\",\"text\":\"hello\"}\n\n"
    );

    let mut cross_site = admin_headers();
    cross_site.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://nas.example.test"),
    );
    cross_site.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
    cross_site.insert(
        "x-forwarded-host",
        HeaderValue::from_static("nas.example.test"),
    );
    let denied = router
        .oneshot(request(
            "POST",
            "/app/qwenpaw/web/api/console/chat",
            cross_site,
            r#"{"message":"denied"}"#,
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert!(
        captured.try_recv().is_err(),
        "cross-site chat reached app upstream"
    );
    server.abort();
}

#[tokio::test]
async fn web_proxy_rejects_missing_or_invalid_identity_before_upstream_access() {
    let (_dir, state, router) = fixture("gateway-web-proxy-auth");
    let (mut captured, server) = start_web_upstream(&state).await;

    let no_identity = HeaderMap::new();
    let mut non_admin = admin_headers();
    non_admin.insert("x-trim-isadmin", HeaderValue::from_static("false"));
    let mut duplicate_identity = admin_headers();
    duplicate_identity.append("x-trim-userid", HeaderValue::from_static("1000"));
    let mut untrusted_peer = peer();
    untrusted_peer.uid = 1000;
    for (label, headers, accepted) in [
        ("missing identity", no_identity, Some(peer())),
        ("non-admin", non_admin, Some(peer())),
        ("duplicate identity", duplicate_identity, Some(peer())),
        ("untrusted peer", admin_headers(), Some(untrusted_peer)),
    ] {
        let response = router
            .clone()
            .oneshot(request(
                "GET",
                "/app/qwenpaw/web/",
                headers,
                Body::empty(),
                accepted,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{label}");
        assert!(captured.try_recv().is_err(), "{label} reached the upstream");
    }

    let no_peer = Request::builder()
        .uri("/app/qwenpaw/web/")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(no_peer).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "missing peer extension"
    );
    assert!(
        captured.try_recv().is_err(),
        "missing peer reached the upstream"
    );
    server.abort();
}

#[tokio::test]
async fn web_proxy_rewrites_head_length_to_match_get_representation() {
    let (_dir, state, router) = fixture("gateway-web-proxy-head-rewrite");
    let (mut captured, server) = start_web_upstream(&state).await;

    let get = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    let get_length = get.headers()[header::CONTENT_LENGTH].clone();
    assert!(!to_bytes(get.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(captured.recv().await.unwrap().method, Method::GET);

    let mut headers = admin_headers();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-10"));
    headers.insert(
        header::IF_RANGE,
        HeaderValue::from_static("\"upstream-sample\""),
    );
    let head = router
        .oneshot(request(
            "HEAD",
            "/app/qwenpaw/web/",
            headers,
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_LENGTH], get_length);
    assert!(to_bytes(head.into_body(), 4096).await.unwrap().is_empty());

    let received = captured.recv().await.unwrap();
    assert_eq!(received.method, Method::GET);
    assert_eq!(received.headers[header::ACCEPT_ENCODING], "identity");
    assert!(!received.headers.contains_key(header::RANGE));
    assert!(!received.headers.contains_key(header::IF_RANGE));
    server.abort();
}

#[tokio::test]
async fn web_proxy_preserves_mount_boundaries_head_errors_and_dynamic_kernel_port() {
    let (_dir, state, router) = fixture("gateway-web-proxy-boundaries");
    let (mut captured, server) = start_web_upstream(&state).await;
    let response = router
        .clone()
        .oneshot(request(
            "HEAD",
            "/app/qwenpaw/web/?head=yes",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "8");
    assert!(to_bytes(response.into_body(), 4096)
        .await
        .unwrap()
        .is_empty());
    let received = captured.recv().await.unwrap();
    assert_eq!(received.method, Method::GET);
    assert_eq!(received.uri, "/?head=yes");

    let encoded_path = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/assets/a%20b?name=x%20y",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(encoded_path.status(), StatusCode::ACCEPTED);
    assert_eq!(
        captured.recv().await.unwrap().uri,
        "/assets/a%20b?name=x%20y"
    );

    let encoded_traversal = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/%2e%2e/api/status",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(encoded_traversal.status(), StatusCode::BAD_REQUEST);
    assert!(captured.try_recv().is_err());

    let not_found = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/not-found",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
    assert_eq!(captured.recv().await.unwrap().uri, "/not-found");

    let upstream_unavailable = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/upstream-unavailable",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(
        upstream_unavailable.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(captured.recv().await.unwrap().uri, "/upstream-unavailable");

    let upstream_bad_gateway = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/upstream-bad-gateway",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(upstream_bad_gateway.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(captured.recv().await.unwrap().uri, "/upstream-bad-gateway");

    let outside_mount = router
        .clone()
        .oneshot(request(
            "GET",
            "/app/qwenpaw/webish",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(outside_mount.status(), StatusCode::NOT_FOUND);
    assert!(captured.try_recv().is_err());

    state.core.kernel.set_port(0);
    let unavailable = router
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    server.abort();
}

#[tokio::test]
async fn web_proxy_streams_sse_and_rejects_websocket_upgrade_explicitly() {
    let (_dir, state, router) = fixture("gateway-web-proxy-stream");
    let (mut captured, server) = start_web_upstream(&state).await;
    let response = tokio::time::timeout(
        Duration::from_millis(750),
        router.clone().oneshot(request(
            "GET",
            "/app/qwenpaw/web/sse",
            admin_headers(),
            Body::empty(),
            Some(peer()),
        )),
    )
    .await
    .expect("the gateway must not wait for the SSE stream to finish")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert_eq!(captured.recv().await.unwrap().uri, "/sse");
    let mut stream = response.into_body().into_data_stream();
    let first = tokio::time::timeout(
        Duration::from_millis(500),
        tokio_stream::StreamExt::next(&mut stream),
    )
    .await
    .expect("the first SSE chunk must stream promptly")
    .unwrap()
    .unwrap();
    assert_eq!(&first[..], b"data: first\n\n");

    let mut headers = admin_headers();
    headers.insert(
        header::CONNECTION,
        HeaderValue::from_static("keep-alive, Upgrade"),
    );
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    let response = router
        .oneshot(request(
            "GET",
            "/app/qwenpaw/web/socket",
            headers,
            Body::empty(),
            Some(peer()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert!(
        captured.try_recv().is_err(),
        "Upgrade was forwarded to the HTTP upstream"
    );
    server.abort();
}
