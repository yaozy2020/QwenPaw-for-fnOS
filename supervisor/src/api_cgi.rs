use std::{
    env, fs,
    io::{self, Read, Write},
    path::Path,
    time::Duration,
};

use crate::{config::Config, control};

const MAX_QUERY_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_FIELDS: usize = 128;
const MAX_FIELD_NAME_BYTES: usize = 128;
const MAX_UPSTREAM_BYTES: usize = 1024 * 1024;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Route {
    method: &'static str,
    path: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Field(String, String);

#[derive(Default)]
struct Parameters {
    query: Vec<Field>,
    body: Vec<Field>,
}

impl Parameters {
    fn get(&self, name: &str) -> Option<&str> {
        field_value(&self.query, name).or_else(|| field_value(&self.body, name))
    }
}

struct CgiResponse {
    status: u16,
    allow: Option<&'static str>,
    body: Vec<u8>,
}

impl CgiResponse {
    fn json(status: u16, value: serde_json::Value) -> Self {
        Self {
            status,
            allow: None,
            body: serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    fn json_error(status: u16, message: &str) -> Self {
        Self::json(
            status,
            serde_json::json!({"success": false, "message": message}),
        )
    }
}

fn field_value<'a>(fields: &'a [Field], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.0 == name)
        .map(|field| field.1.as_str())
}

fn route_for(action: &str) -> Option<Route> {
    let (method, path) = match action {
        "status" => ("GET", "/status"),
        "logs" => ("GET", "/logs"),
        "version_list" => ("GET", "/kernel/versions"),
        "get_mirror" => ("GET", ""),
        "upgrade_status" => ("GET", "/kernel/install-status"),
        "upgrade_logs" => ("GET", "/kernel/install-log"),
        "start" => ("POST", "/start"),
        "stop" => ("POST", "/stop"),
        "restart" => ("POST", "/restart"),
        "clear_logs" => ("POST", "/logs/clear"),
        "upgrade" => ("POST", "/kernel/install"),
        "uninstall_kernel" => ("POST", "/kernel/uninstall"),
        "reset_auth" => ("POST", "/kernel/reset-auth"),
        "change_port" => ("POST", "/port"),
        "set_mirror" => ("POST", ""),
        _ => return None,
    };
    Some(Route { method, path })
}

fn decode_component(input: &str, plus_as_space: bool) -> Result<String, String> {
    let source = input.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        match source[index] {
            b'%' => {
                let high = source.get(index + 1).and_then(|byte| hex_value(*byte));
                let low = source.get(index + 2).and_then(|byte| hex_value(*byte));
                let (Some(high), Some(low)) = (high, low) else {
                    return Err("invalid percent escape".into());
                };
                decoded.push((high << 4) | low);
                index += 3;
            }
            b'+' if plus_as_space => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    let value = String::from_utf8(decoded).map_err(|_| "form value is not UTF-8".to_string())?;
    if value.contains('\0') {
        return Err("form value contains NUL".into());
    }
    Ok(value)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_form(input: &str, plus_as_space: bool) -> Result<Vec<Field>, String> {
    let mut fields = Vec::new();
    for pair in input.split('&').filter(|pair| !pair.is_empty()) {
        if fields.len() == MAX_FIELDS {
            return Err("too many form fields".into());
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = decode_component(name, plus_as_space)?;
        if name.is_empty() {
            continue;
        }
        if name.len() > MAX_FIELD_NAME_BYTES {
            return Err("form field name is too long".into());
        }
        let value = decode_component(value, plus_as_space)?;
        fields.push(Field(name, value));
    }
    Ok(fields)
}

fn parse_json_object(input: &[u8]) -> Result<Vec<Field>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(input).map_err(|_| "invalid JSON".to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "JSON body must be an object".to_string())?;
    if object.len() > MAX_FIELDS {
        return Err("too many JSON fields".into());
    }
    let mut fields = Vec::with_capacity(object.len());
    for (name, value) in object {
        if name.is_empty() || name.len() > MAX_FIELD_NAME_BYTES {
            return Err("JSON field name is invalid".into());
        }
        let value = match value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::Bool(value) => value.to_string(),
            _ => return Err("JSON fields must be scalar values".into()),
        };
        fields.push(Field(name.clone(), value));
    }
    Ok(fields)
}

fn csrf_cookie_value<'a>(cookie: &'a str, expected_name: &str) -> Option<&'a str> {
    let mut value = None;
    for part in cookie.split(';') {
        let Some((name, value_part)) = part.trim().split_once('=') else {
            continue;
        };
        if name.trim() == expected_name {
            if value.is_some() {
                return None;
            }
            value = Some(value_part.trim());
        }
    }
    value
}

fn env_is_https() -> bool {
    env::var("HTTPS")
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "on" | "1" | "true"))
        || env::var("REQUEST_SCHEME")
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("https"))
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn has_valid_csrf(
    cookie: &str,
    header: &str,
    query: &[Field],
    body: &[Field],
    https: bool,
) -> bool {
    let cookie_name = if https {
        "__Host-qwenpaw_csrf"
    } else {
        "qwenpaw_csrf"
    };
    let Some(cookie_value) =
        csrf_cookie_value(cookie, cookie_name).filter(|value| !value.is_empty())
    else {
        return false;
    };
    let supplied = if !header.is_empty() {
        Some(header)
    } else {
        field_value(query, "_csrf").or_else(|| field_value(body, "_csrf"))
    };
    supplied.is_some_and(|value| !value.is_empty() && constant_time_eq(cookie_value, value))
}

fn normalize_upstream_body(action: &str, body: &[u8]) -> Vec<u8> {
    if serde_json::from_slice::<serde_json::Value>(body).is_ok() {
        return body.to_vec();
    }
    if action == "version_list" {
        serde_json::json!({
            "success": false,
            "versions": [],
            "latest_stable": "",
            "message": "监督者未响应,请稍后重试"
        })
        .to_string()
        .into_bytes()
    } else {
        serde_json::json!({
            "success": false,
            "message": "监督者未响应,请稍后重试"
        })
        .to_string()
        .into_bytes()
    }
}

fn read_cgi_body(
    reader: &mut impl Read,
    content_length: &str,
) -> Result<Vec<u8>, (u16, &'static str)> {
    if content_length.is_empty() {
        return Ok(Vec::new());
    }
    let length = content_length
        .parse::<usize>()
        .map_err(|_| (400, "请求体长度无效"))?;
    if length > MAX_BODY_BYTES {
        return Err((413, "请求体超过 64 KiB 上限"));
    }
    let mut body = Vec::with_capacity(length);
    reader
        .take(length as u64)
        .read_to_end(&mut body)
        .map_err(|_| (400, "读取请求体失败"))?;
    if body.len() != length {
        return Err((400, "请求体长度不完整"));
    }
    Ok(body)
}

fn parse_request_body(body: &[u8], content_type: &str) -> Result<Vec<Field>, String> {
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if media_type == "application/json" {
        parse_json_object(body)
    } else if media_type == "application/x-www-form-urlencoded" {
        let body = std::str::from_utf8(body).map_err(|_| "form body is not UTF-8".to_string())?;
        parse_form(body, true)
    } else if media_type.is_empty() {
        if body
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace())
            == Some(b'{')
        {
            parse_json_object(body)
        } else {
            let body =
                std::str::from_utf8(body).map_err(|_| "form body is not UTF-8".to_string())?;
            parse_form(body, true)
        }
    } else {
        Err("unsupported request content type".into())
    }
}

fn read_token(path: &Path) -> Result<String, ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if !metadata.file_type().is_file() {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(());
        }
    }
    let token = fs::read_to_string(path).map_err(|_| ())?;
    let token = token.trim();
    if token.len() < 32 {
        return Err(());
    }
    Ok(token.to_string())
}

fn upstream_url(
    port: u16,
    route: Route,
    action: &str,
    params: &Parameters,
) -> Result<reqwest::Url, ()> {
    let mut url =
        reqwest::Url::parse(&format!("http://127.0.0.1:{}{}", port, route.path)).map_err(|_| ())?;
    match action {
        "logs" => {
            let lines = params.get("lines").unwrap_or_default();
            let lines = if lines.is_empty() || !lines.bytes().all(|byte| byte.is_ascii_digit()) {
                "80"
            } else if lines.len() > 6 {
                "100000"
            } else {
                lines
            };
            url.query_pairs_mut().append_pair("tail", lines);
        }
        "upgrade_logs" => {
            url.query_pairs_mut().append_pair("tail", "40");
        }
        _ => {}
    }
    Ok(url)
}

fn mirror_set_response(value: Option<&str>, cfg: &Config) -> CgiResponse {
    let Some(value) = value else {
        return CgiResponse::json(
            200,
            serde_json::json!({"success": false, "message": "下载源参数缺失"}),
        );
    };
    if let Err(message) = crate::install::validate_mirror(value) {
        return CgiResponse::json(
            200,
            serde_json::json!({"success": false, "message": message}),
        );
    }
    match control::submit_mirror(&control::socket_path(&cfg.var_dir), value) {
        Ok(()) => CgiResponse::json(
            200,
            serde_json::json!({"success": true, "message": "下载源已保存"}),
        ),
        Err(error) => CgiResponse::json(
            200,
            serde_json::json!({
                "success": false,
                "message": "下载源写入失败(权限?)",
                "error": error
            }),
        ),
    }
}

fn forward(
    cfg: &Config,
    action: &str,
    route: Route,
    params: &Parameters,
    body: Option<Vec<u8>>,
) -> CgiResponse {
    let token = match read_token(Path::new(&cfg.token_file)) {
        Ok(token) => token,
        Err(()) => {
            return CgiResponse::json_error(503, "无法读取安全的监督者凭据");
        }
    };
    let url = match upstream_url(cfg.listen_port, route, action, params) {
        Ok(url) => url,
        Err(()) => return CgiResponse::json_error(502, "监督者请求地址无效"),
    };
    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(UPSTREAM_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return CgiResponse::json_error(502, "无法创建监督者连接"),
    };
    let method = if route.method == "GET" {
        reqwest::Method::GET
    } else {
        reqwest::Method::POST
    };
    let mut request = client
        .request(method, url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("X-Supervisor-Token", token);
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = match request.send() {
        Ok(response) => response,
        Err(_) => return CgiResponse::json_error(502, "监督者未响应,请稍后重试"),
    };
    let status = response.status().as_u16();
    let mut response_body = Vec::new();
    if response
        .take((MAX_UPSTREAM_BYTES + 1) as u64)
        .read_to_end(&mut response_body)
        .is_err()
        || response_body.len() > MAX_UPSTREAM_BYTES
    {
        return CgiResponse::json_error(502, "监督者响应超过 1 MiB 上限");
    }
    CgiResponse {
        status,
        allow: None,
        body: normalize_upstream_body(action, &response_body),
    }
}

fn dispatch(cfg: &Config, action: &str, params: &Parameters, route: Route) -> CgiResponse {
    match action {
        "get_mirror" => {
            let (_, body) = crate::mirror::get_json(&cfg.var_dir);
            CgiResponse {
                status: 200,
                allow: None,
                body: body.into_bytes(),
            }
        }
        "set_mirror" => mirror_set_response(params.get("mirror"), cfg),
        "upgrade" => {
            let version = params
                .get("version")
                .filter(|version| !version.is_empty())
                .unwrap_or("latest");
            let mirror = params
                .get("mirror")
                .map(str::to_string)
                .or_else(|| crate::mirror::read(&cfg.var_dir).ok())
                .unwrap_or_default();
            let body = serde_json::to_vec(&serde_json::json!({
                "version": version,
                "mirror": mirror,
            }))
            .unwrap_or_else(|_| b"{}".to_vec());
            forward(cfg, action, route, params, Some(body))
        }
        "change_port" => {
            let Some(port) = params.get("port") else {
                return CgiResponse::json(
                    200,
                    serde_json::json!({
                        "success": false,
                        "message": "端口参数非法,需为纯数字"
                    }),
                );
            };
            if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return CgiResponse::json(
                    200,
                    serde_json::json!({
                        "success": false,
                        "message": "端口参数非法,需为纯数字"
                    }),
                );
            }
            let body = format!("{{\"port\":{port}}}").into_bytes();
            forward(cfg, action, route, params, Some(body))
        }
        _ => forward(cfg, action, route, params, None),
    }
}

struct CgiRequest<'a> {
    method: &'a str,
    query_string: &'a str,
    cookie: &'a str,
    csrf_header: &'a str,
    content_type: &'a str,
    content_length: &'a str,
    https: bool,
}

fn handle_request<R: Read>(cfg: &Config, request: CgiRequest<'_>, mut reader: R) -> CgiResponse {
    if request.query_string.len() > MAX_QUERY_BYTES {
        return CgiResponse::json_error(413, "查询参数超过 16 KiB 上限");
    }
    let query = match parse_form(request.query_string, false) {
        Ok(fields) => fields,
        Err(_) => return CgiResponse::json_error(400, "查询参数格式非法"),
    };
    let Some(action) = field_value(&query, "action").map(str::to_string) else {
        return CgiResponse::json_error(404, "未知操作");
    };
    let Some(route) = route_for(&action) else {
        return CgiResponse::json_error(404, "未知操作");
    };
    let effective_method = if request.method == "HEAD" {
        "GET"
    } else {
        request.method
    };
    if effective_method != route.method {
        let allow = if route.method == "GET" {
            "GET, HEAD"
        } else {
            "POST"
        };
        let mut response =
            CgiResponse::json_error(405, &format!("此操作仅允许 {} 请求", route.method));
        response.allow = Some(allow);
        return response;
    }

    let body_fields = if effective_method == "POST" {
        let body = match read_cgi_body(&mut reader, request.content_length) {
            Ok(body) => body,
            Err((status, message)) => return CgiResponse::json_error(status, message),
        };
        match parse_request_body(&body, request.content_type) {
            Ok(fields) => fields,
            Err(_) => return CgiResponse::json_error(400, "请求参数格式非法"),
        }
    } else {
        Vec::new()
    };
    if effective_method == "POST"
        && !has_valid_csrf(
            request.cookie,
            request.csrf_header,
            &query,
            &body_fields,
            request.https,
        )
    {
        return CgiResponse::json_error(403, "CSRF 校验失败:请刷新控制台页面后重试");
    }

    let params = Parameters {
        query,
        body: body_fields,
    };
    dispatch(cfg, &action, &params, route)
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

fn write_response(mut output: impl Write, response: &CgiResponse) -> io::Result<()> {
    if response.status != 200 {
        writeln!(
            output,
            "Status: {} {}",
            response.status,
            status_reason(response.status)
        )?;
    }
    if let Some(allow) = response.allow {
        writeln!(output, "Allow: {allow}")?;
    }
    output.write_all(b"Content-Type: application/json\r\n\r\n")?;
    output.write_all(&response.body)?;
    output.flush()
}

pub fn run(cfg: &Config) -> i32 {
    let method = env::var("REQUEST_METHOD").unwrap_or_else(|_| "GET".into());
    let query = env::var("QUERY_STRING").unwrap_or_default();
    let cookie = env::var("HTTP_COOKIE").unwrap_or_default();
    let csrf_header = env::var("HTTP_X_CSRF_TOKEN").unwrap_or_default();
    let content_type = env::var("CONTENT_TYPE").unwrap_or_default();
    let content_length = env::var("CONTENT_LENGTH").unwrap_or_default();
    let response = handle_request(
        cfg,
        CgiRequest {
            method: &method,
            query_string: &query,
            cookie: &cookie,
            csrf_header: &csrf_header,
            content_type: &content_type,
            content_length: &content_length,
            https: env_is_https(),
        },
        io::stdin().lock(),
    );
    if write_response(io::stdout().lock(), &response).is_ok() {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_form_fields_with_standard_body_space_and_empty_values() {
        let fields = parse_form(
            "version=2.1%2Bnas&mirror=https%3A%2F%2Fmirror.example%2Fa+b&empty=",
            true,
        )
        .unwrap();
        assert_eq!(
            fields,
            vec![
                Field("version".into(), "2.1+nas".into()),
                Field("mirror".into(), "https://mirror.example/a b".into()),
                Field("empty".into(), String::new()),
            ]
        );
    }

    #[test]
    fn parses_json_object_fields_and_rejects_non_objects() {
        assert_eq!(
            parse_json_object(br#"{"version":"2.1+nas","port":18088}"#).unwrap(),
            vec![
                Field("port".into(), "18088".into()),
                Field("version".into(), "2.1+nas".into())
            ]
        );
        assert!(parse_json_object(br#"["not", "an", "object"]"#).is_err());
    }

    #[test]
    fn maps_every_legacy_action_to_its_supervisor_route_and_method() {
        let cases = [
            ("status", "GET", "/status"),
            ("logs", "GET", "/logs"),
            ("version_list", "GET", "/kernel/versions"),
            ("get_mirror", "GET", ""),
            ("upgrade_status", "GET", "/kernel/install-status"),
            ("upgrade_logs", "GET", "/kernel/install-log"),
            ("start", "POST", "/start"),
            ("stop", "POST", "/stop"),
            ("restart", "POST", "/restart"),
            ("clear_logs", "POST", "/logs/clear"),
            ("upgrade", "POST", "/kernel/install"),
            ("uninstall_kernel", "POST", "/kernel/uninstall"),
            ("reset_auth", "POST", "/kernel/reset-auth"),
            ("change_port", "POST", "/port"),
            ("set_mirror", "POST", ""),
        ];
        for (action, method, path) in cases {
            assert_eq!(route_for(action), Some(Route { method, path }), "{action}");
        }
        assert_eq!(route_for("unknown"), None);
    }

    #[test]
    fn requires_scheme_scoped_double_submit_csrf_and_supports_header_or_query_token() {
        let query = vec![Field("_csrf".into(), "query-token".into())];
        assert!(has_valid_csrf(
            "qwenpaw_csrf=cookie-token",
            "cookie-token",
            &[],
            &[],
            false
        ));
        assert!(has_valid_csrf(
            "qwenpaw_csrf=query-token",
            "",
            &query,
            &[],
            false
        ));
        assert!(has_valid_csrf(
            "__Host-qwenpaw_csrf=host-token",
            "host-token",
            &[],
            &[],
            true
        ));
        assert!(!has_valid_csrf(
            "qwenpaw_csrf=legacy-token",
            "legacy-token",
            &[],
            &[],
            true
        ));
        assert!(!has_valid_csrf(
            "__Host-qwenpaw_csrf=host-token",
            "host-token",
            &[],
            &[],
            false
        ));
        assert!(!has_valid_csrf(
            "qwenpaw_csrf=cookie-token",
            "wrong",
            &query,
            &[],
            false
        ));
        assert!(!has_valid_csrf("other=value", "value", &[], &[], false));
        assert!(!has_valid_csrf(
            "qwenpaw_csrf=cookie-token; qwenpaw_csrf=attacker-token",
            "cookie-token",
            &[],
            &[],
            false
        ));
        assert!(!has_valid_csrf(
            "__Host-qwenpaw_csrf=host-token; __Host-qwenpaw_csrf=attacker-token",
            "host-token",
            &[],
            &[],
            true
        ));
    }

    #[test]
    fn replaces_non_json_upstream_body_with_legacy_json_fallback() {
        let body = normalize_upstream_body("status", b"upstream unavailable");
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "success": false,
                "message": "监督者未响应,请稍后重试"
            })
        );
    }

    #[test]
    fn forwards_authenticated_request_and_preserves_upstream_status_and_body() {
        use std::os::unix::fs::PermissionsExt;
        use std::{
            io::{BufRead, BufReader, Write},
            net::TcpListener,
            thread,
            time::{SystemTime, UNIX_EPOCH},
        };

        let directory = std::env::temp_dir().join(format!(
            "qwenpaw-api-cgi-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let token = "t".repeat(48);
        let token_file = directory.join("supervisor-token");
        std::fs::write(&token_file, &token).unwrap();
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected_token = token.clone();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.is_empty() {
                    break;
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let request = request.to_ascii_lowercase();
            assert!(request.starts_with("get /status http/1.1\r\n"));
            assert!(request.contains(&format!("x-supervisor-token: {expected_token}\r\n")));

            let body = br#"{"success":false,"message":"busy"}"#;
            write!(
                reader.get_mut(),
                "HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            reader.get_mut().write_all(body).unwrap();
        });

        let path = |name: &str| directory.join(name).to_string_lossy().into_owned();
        let cfg = Config {
            var_dir: path("var"),
            etc_dir: path("etc"),
            home_dir: path("home"),
            share_dir: path("share"),
            ui_config_file: path("ui-config"),
            kernel_venv: path("venv"),
            kernel_pid_file: path("kernel.pid"),
            kernel_log: path("kernel.log"),
            kernel_port: crate::api::test_support::free_port(),
            kernel_log_level: "info".into(),
            token_file: token_file.to_string_lossy().into_owned(),
            supervisor_pid_file: path("supervisor.pid"),
            supervisor_log: path("supervisor.log"),
            listen_port: port,
            auto_start: false,
        };
        let response = handle_request(
            &cfg,
            CgiRequest {
                method: "GET",
                query_string: "action=status",
                cookie: "",
                csrf_header: "",
                content_type: "",
                content_length: "",
                https: false,
            },
            io::empty(),
        );
        server.join().unwrap();
        assert_eq!(response.status, 409);
        assert_eq!(response.body, br#"{"success":false,"message":"busy"}"#);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
