use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAX_STATIC_BYTES: u64 = 8 * 1024 * 1024;
const CSRF_TOKEN_BYTES: usize = 16;

#[derive(Debug, PartialEq, Eq)]
enum PathError {
    BadRequest,
    NotFound,
}

#[derive(Debug)]
enum ResourceError {
    NotFound,
    Internal,
}

struct CgiRequest<'a> {
    method: &'a str,
    request_uri: &'a str,
    accept_encoding: &'a str,
    cookie: &'a str,
    https: bool,
}

struct CgiResponse {
    status: u16,
    headers: Vec<(&'static str, String)>,
    content_length: usize,
    body: Vec<u8>,
}

impl CgiResponse {
    fn error(status: u16, message: &'static str, head: bool) -> Self {
        let body = format!("{message}\n").into_bytes();
        Self {
            status,
            headers: vec![
                ("Content-Type", "text/plain; charset=utf-8".into()),
                ("X-Content-Type-Options", "nosniff".into()),
            ],
            content_length: body.len(),
            body: if head { Vec::new() } else { body },
        }
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn extract_path(request_uri: &str) -> Result<String, PathError> {
    if request_uri.contains('\0') {
        return Err(PathError::BadRequest);
    }
    if request_uri.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(PathError::NotFound);
    }

    let uri_path = request_uri
        .split_once('?')
        .map_or(request_uri, |(path, _)| path);
    if request_uri.len() > 8 * 1024
        || uri_path.contains('\\')
        || uri_path.contains('#')
        || (!uri_path.is_empty() && !uri_path.starts_with('/'))
        || uri_path.starts_with("//")
    {
        return Err(PathError::BadRequest);
    }
    let marker = uri_path
        .match_indices("index.cgi")
        .find(|(offset, _)| {
            let before = uri_path[..*offset].chars().next_back();
            let after = uri_path[*offset + "index.cgi".len()..].chars().next();
            (before.is_none() || before == Some('/')) && (after.is_none() || after == Some('/'))
        })
        .map(|(offset, _)| offset + "index.cgi".len());

    let Some(marker) = marker else {
        return Ok("index.html".into());
    };
    let tail = &uri_path[marker..];
    if tail.is_empty() || tail == "/" {
        return Ok("index.html".into());
    }
    if !tail.starts_with('/') || tail.starts_with("//") {
        return Err(PathError::BadRequest);
    }

    let mut decoded = Vec::with_capacity(tail.len() - 1);
    let encoded_path = &tail.as_bytes()[1..];
    let mut index = 0;
    while index < encoded_path.len() {
        let byte = encoded_path[index];
        if byte == b'%' {
            let high = encoded_path
                .get(index + 1)
                .and_then(|value| hex_value(*value));
            let low = encoded_path
                .get(index + 2)
                .and_then(|value| hex_value(*value));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(PathError::BadRequest);
            };
            let decoded_byte = (high << 4) | low;
            if matches!(decoded_byte, b'/' | b'\\' | 0) {
                return Err(PathError::BadRequest);
            }
            decoded.push(decoded_byte);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }

    let decoded = String::from_utf8(decoded).map_err(|_| PathError::BadRequest)?;
    if decoded.contains('%')
        || decoded.contains('\\')
        || decoded.contains('\0')
        || decoded.starts_with('/')
        || decoded
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(PathError::BadRequest);
    }
    if decoded.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(PathError::NotFound);
    }

    let asset = decoded.strip_prefix("assets/");
    let allowed_asset = asset.is_some_and(|asset| {
        asset.split('/').all(|component| {
            !component.starts_with('.')
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        }) && matches!(
            asset.rsplit_once('.').map(|(_, extension)| extension),
            Some("css" | "js" | "jpg" | "jpeg" | "png" | "gif" | "svg")
        )
    });
    if decoded == "index.html" || allowed_asset {
        Ok(decoded)
    } else {
        Err(PathError::NotFound)
    }
}

fn mime_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("txt" | "log") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn entry_quality(entry: &str) -> f32 {
    let mut parts = entry.split(';');
    let _coding = parts.next();
    for parameter in parts {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("q") {
            let Ok(quality) = value.trim().parse::<f32>() else {
                return 0.0;
            };
            return if quality.is_finite() && (0.0..=1.0).contains(&quality) {
                quality
            } else {
                0.0
            };
        }
    }
    1.0
}

fn encoding_quality(header: &str, coding: &str) -> f32 {
    let mut explicit = None;
    let mut wildcard = None;
    for entry in header.split(',') {
        let name = entry.split(';').next().unwrap_or_default().trim();
        if name.eq_ignore_ascii_case(coding) && explicit.is_none() {
            explicit = Some(entry_quality(entry));
        } else if name == "*" && wildcard.is_none() {
            wildcard = Some(entry_quality(entry));
        }
    }
    explicit.or(wildcard).unwrap_or(0.0)
}

fn accepts_encoding(header: &str, coding: &str) -> bool {
    encoding_quality(header, coding) > 0.0
}

fn resource_error(error: io::Error) -> ResourceError {
    match error.raw_os_error() {
        Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP | libc::EACCES) => ResourceError::NotFound,
        _ => ResourceError::Internal,
    }
}

fn open_root(root: &Path) -> Result<fs::File, ResourceError> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)
        .map_err(resource_error)
}

fn open_resource(root: &fs::File, name: &str) -> Result<Option<fs::File>, ResourceError> {
    use std::{
        ffi::CString,
        os::fd::{AsRawFd, FromRawFd},
    };

    let mut directory = root.try_clone().map_err(resource_error)?;
    let mut components = name.split('/').peekable();
    while let Some(component) = components.next() {
        if component.is_empty() || component == "." || component == ".." {
            return Err(ResourceError::NotFound);
        }
        let name = CString::new(component).map_err(|_| ResourceError::NotFound)?;
        let is_directory = components.peek().is_some();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if is_directory { libc::O_DIRECTORY } else { 0 };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(resource_error(error))
            };
        }
        let file = unsafe { fs::File::from_raw_fd(fd) };
        if is_directory {
            directory = file;
            continue;
        }
        let metadata = file.metadata().map_err(resource_error)?;
        if !metadata.is_file() {
            return Err(ResourceError::NotFound);
        }
        if metadata.len() > MAX_STATIC_BYTES {
            return Err(ResourceError::Internal);
        }
        return Ok(Some(file));
    }
    Err(ResourceError::NotFound)
}

fn read_limited(file: fs::File) -> Result<Vec<u8>, ResourceError> {
    let mut bytes = Vec::new();
    file.take(MAX_STATIC_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(resource_error)?;
    if bytes.len() as u64 > MAX_STATIC_BYTES {
        return Err(ResourceError::Internal);
    }
    Ok(bytes)
}

fn read_representation(
    root: &fs::File,
    resource: &str,
    accept_encoding: &str,
) -> Result<(Vec<u8>, Option<&'static str>), ResourceError> {
    let plain = open_resource(root, resource)?.ok_or(ResourceError::NotFound)?;
    for (coding, suffix) in [("br", ".br"), ("gzip", ".gz")] {
        if accepts_encoding(accept_encoding, coding) {
            if let Some(file) = open_resource(root, &format!("{resource}{suffix}"))? {
                return read_limited(file).map(|bytes| (bytes, Some(coding)));
            }
        }
    }
    read_limited(plain).map(|bytes| (bytes, None))
}

fn unique_csrf_cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    let mut value = None;
    for part in cookie.split(';') {
        let Some((cookie_name, cookie_value)) = part.trim().split_once('=') else {
            continue;
        };
        if cookie_name.trim() == name {
            if value.is_some() {
                return None;
            }
            value = Some(cookie_value.trim());
        }
    }
    value
}

fn has_csrf_cookie(cookie: &str, secure: bool) -> bool {
    let name = if secure {
        "__Host-qwenpaw_csrf"
    } else {
        "qwenpaw_csrf"
    };
    unique_csrf_cookie_value(cookie, name).is_some_and(|value| !value.is_empty())
}

fn make_csrf_cookie(
    cookie: &str,
    secure: bool,
    random: &mut impl Read,
) -> io::Result<Option<String>> {
    if has_csrf_cookie(cookie, secure) {
        return Ok(None);
    }
    let mut token = [0_u8; CSRF_TOKEN_BYTES];
    random.read_exact(&mut token)?;
    let mut value = String::with_capacity(CSRF_TOKEN_BYTES * 2);
    for byte in token {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    let cookie = if secure {
        format!("__Host-qwenpaw_csrf={value}; Path=/; Secure; SameSite=Lax")
    } else {
        format!("qwenpaw_csrf={value}; Path=/; SameSite=Lax")
    };
    Ok(Some(cookie))
}

fn env_is_https() -> bool {
    env::var("HTTPS")
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "on" | "1" | "true"))
        || env::var("REQUEST_SCHEME")
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("https"))
}

fn handle_request(www_root: &Path, request: CgiRequest<'_>) -> CgiResponse {
    let head = request.method == "HEAD";
    if request.method != "GET" && !head {
        let mut response = CgiResponse::error(405, "Method Not Allowed", head);
        response.headers.push(("Allow", "GET, HEAD".into()));
        return response;
    }

    let resource = match extract_path(request.request_uri) {
        Ok(resource) => resource,
        Err(PathError::BadRequest) => return CgiResponse::error(400, "Bad Request", head),
        Err(PathError::NotFound) => return CgiResponse::error(404, "Not Found", head),
    };

    let root = match open_root(www_root) {
        Ok(root) => root,
        Err(ResourceError::NotFound) => return CgiResponse::error(404, "Not Found", head),
        Err(ResourceError::Internal) => {
            return CgiResponse::error(500, "Internal Server Error", head)
        }
    };

    let (body, encoding) = match read_representation(&root, &resource, request.accept_encoding) {
        Ok(representation) => representation,
        Err(ResourceError::NotFound) => return CgiResponse::error(404, "Not Found", head),
        Err(ResourceError::Internal) => {
            return CgiResponse::error(500, "Internal Server Error", head)
        }
    };

    let content_length = body.len();
    let mut headers = vec![
        ("Content-Type", mime_type(&resource).into()),
        ("X-Content-Type-Options", "nosniff".into()),
        ("Vary", "Accept-Encoding".into()),
    ];
    if let Some(encoding) = encoding {
        headers.push(("Content-Encoding", encoding.into()));
    }
    if resource.ends_with(".html") || resource.ends_with(".htm") {
        headers.push((
            "Cache-Control",
            "no-cache, no-store, must-revalidate, max-age=0".into(),
        ));
        headers.push(("Pragma", "no-cache".into()));
        headers.push(("Expires", "0".into()));
        if !has_csrf_cookie(request.cookie, request.https) {
            let mut random = match fs::File::open("/dev/urandom") {
                Ok(random) => random,
                Err(_) => return CgiResponse::error(500, "Internal Server Error", head),
            };
            match make_csrf_cookie(request.cookie, request.https, &mut random) {
                Ok(Some(cookie)) => headers.push(("Set-Cookie", cookie)),
                Ok(None) => {}
                Err(_) => return CgiResponse::error(500, "Internal Server Error", head),
            }
        }
    }

    CgiResponse {
        status: 200,
        headers,
        content_length,
        body: if head { Vec::new() } else { body },
    }
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    }
}

fn write_response(mut output: impl Write, response: &CgiResponse) -> io::Result<()> {
    if response.status != 200 {
        write!(
            output,
            "Status: {} {}\r\n",
            response.status,
            status_reason(response.status)
        )?;
    }
    for (name, value) in &response.headers {
        write!(output, "{name}: {value}\r\n")?;
    }
    write!(
        output,
        "Content-Length: {}\r\n\r\n",
        response.content_length
    )?;
    output.write_all(&response.body)?;
    output.flush()
}

fn app_dest() -> PathBuf {
    env::var_os("TRIM_APPDEST")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/apps/qwenpaw/target"))
}

pub fn run() -> i32 {
    let method = env::var("REQUEST_METHOD").unwrap_or_else(|_| "GET".into());
    let request_uri = env::var("REQUEST_URI").unwrap_or_default();
    let accept_encoding = env::var("HTTP_ACCEPT_ENCODING").unwrap_or_default();
    let cookie = env::var("HTTP_COOKIE").unwrap_or_default();
    let response = handle_request(
        &app_dest().join("www"),
        CgiRequest {
            method: &method,
            request_uri: &request_uri,
            accept_encoding: &accept_encoding,
            cookie: &cookie,
            https: env_is_https(),
        },
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
    use std::{
        io::Cursor,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    fn fixture() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "qwenpaw-index-cgi-{}-{unique}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn request<'a>(
        method: &'a str,
        request_uri: &'a str,
        accept_encoding: &'a str,
        cookie: &'a str,
        https: bool,
    ) -> CgiRequest<'a> {
        CgiRequest {
            method,
            request_uri,
            accept_encoding,
            cookie,
            https,
        }
    }

    #[test]
    fn extracts_index_cgi_tail_and_defaults_to_index_html() {
        assert_eq!(
            extract_path("/cgi/ThirdParty/qwenpaw/index.cgi/"),
            Ok("index.html".into())
        );
        assert_eq!(
            extract_path("/cgi/ThirdParty/qwenpaw/index.cgi/index.html?cache=1"),
            Ok("index.html".into())
        );
        assert_eq!(
            extract_path("/cgi/qwenpaw/index.cgi"),
            Ok("index.html".into())
        );
        assert_eq!(
            extract_path("/cgi/qwenpaw/index.cgi?x=1"),
            Ok("index.html".into())
        );
    }

    #[test]
    fn rejects_encoded_and_double_encoded_traversal() {
        for uri in [
            "/cgi/qwenpaw/index.cgi/%2e%2e/secret",
            "/cgi/qwenpaw/index.cgi/%252e%252e/secret",
            "/cgi/qwenpaw/index.cgi/%2fetc/passwd",
            "/cgi/qwenpaw/index.cgi/%00",
            "/cgi/qwenpaw/index.cgi/%5cetc%5cpasswd",
            "/cgi/qwenpaw/index.cgi/../secret",
            "/cgi/qwenpaw/index.cgi//etc/passwd",
            "/cgi/qwenpaw/index.cgi/%zz",
        ] {
            assert_eq!(extract_path(uri), Err(PathError::BadRequest), "{uri}");
        }
    }

    #[test]
    fn control_characters_in_unknown_paths_are_safe_not_found_but_nul_is_bad_request() {
        for tail in [
            "missing-\x01-name.txt",
            "missing-\x1b-name.txt",
            "missing-\x7f-name.txt",
            "missing-\r\nX-Injected: unsafe.txt",
            "missing-%0d%0aX-Injected%3a%20unsafe.txt",
        ] {
            let uri = format!("/index.cgi/{tail}");
            assert_eq!(extract_path(&uri), Err(PathError::NotFound), "{uri:?}");
            let response =
                handle_request(Path::new("/unused"), request("GET", &uri, "", "", false));
            assert_eq!(response.status, 404);
            assert_eq!(response.body, b"Not Found\n");
            let mut wire = Vec::new();
            write_response(&mut wire, &response).unwrap();
            assert!(!String::from_utf8(wire).unwrap().contains("X-Injected"));
        }
        for uri in ["/index.cgi/missing-\0.txt", "/index.cgi/missing-%00.txt"] {
            assert_eq!(extract_path(uri), Err(PathError::BadRequest));
        }
    }

    #[test]
    fn serves_allowlisted_asset_types_with_mime_and_without_html_cookie() {
        let root = fixture();
        fs::create_dir_all(root.join("assets/styles")).unwrap();
        fs::write(root.join("assets/styles/site.css"), b"body{color:red}").unwrap();
        fs::write(root.join("assets/icon.png"), [0, 255, 137, 80, 78, 71]).unwrap();
        for (path, mime, body) in [
            (
                "assets/styles/site.css",
                "text/css; charset=utf-8",
                &b"body{color:red}"[..],
            ),
            (
                "assets/icon.png",
                "image/png",
                &[0, 255, 137, 80, 78, 71][..],
            ),
        ] {
            let uri = format!("/index.cgi/{path}");
            let response = handle_request(&root, request("GET", &uri, "", "", false));
            assert_eq!(response.status, 200, "{path}");
            assert_eq!(response.body, body);
            assert!(response.headers.contains(&("Content-Type", mime.into())));
            assert!(!response
                .headers
                .iter()
                .any(|(name, _)| matches!(*name, "Set-Cookie" | "Cache-Control")));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_resources_outside_the_build_allowlist() {
        for uri in [
            "/cgi/qwenpaw/index.cgi/main.js",
            "/cgi/qwenpaw/index.cgi/index.html.gz",
            "/cgi/qwenpaw/index.cgi/target/debug/supervisor",
            "/cgi/qwenpaw/index.cgi/var/supervisor-token",
        ] {
            assert_eq!(extract_path(uri), Err(PathError::NotFound), "{uri}");
        }
    }

    #[test]
    fn encoding_negotiation_prefers_explicit_values_over_wildcard_and_honors_q_zero() {
        assert!(!accepts_encoding("gzip;q=0, *;q=1", "gzip"));
        assert!(accepts_encoding("gzip;q=0, *;q=1", "br"));
        assert!(accepts_encoding("Br; Q=0.8, gzip;q=0.4", "br"));
        assert!(!accepts_encoding("*;q=0", "br"));
        assert!(!accepts_encoding("GZIP;Q=0", "gzip"));
        assert!(!accepts_encoding("br;q=1.1", "br"));
    }

    #[test]
    fn csrf_cookie_is_random_and_secure_only_for_https() {
        let mut random = Cursor::new([0xa5_u8; CSRF_TOKEN_BYTES]);
        assert_eq!(
            make_csrf_cookie("", true, &mut random).unwrap(),
            Some(format!(
                "__Host-qwenpaw_csrf={}; Path=/; Secure; SameSite=Lax",
                "a5".repeat(CSRF_TOKEN_BYTES)
            ))
        );

        let mut random = Cursor::new([0x11_u8; CSRF_TOKEN_BYTES]);
        let issued = make_csrf_cookie("other=x; qwenpaw_csrf=legacy-token", true, &mut random)
            .unwrap()
            .unwrap();
        assert!(issued.starts_with("__Host-qwenpaw_csrf="));

        let mut random = Cursor::new([0x11_u8; CSRF_TOKEN_BYTES]);
        assert_eq!(
            make_csrf_cookie("other=x; qwenpaw_csrf=existing", false, &mut random).unwrap(),
            None
        );

        let mut random = Cursor::new([0x11_u8; CSRF_TOKEN_BYTES]);
        let cookie = make_csrf_cookie("", false, &mut random).unwrap().unwrap();
        assert!(cookie.ends_with("; Path=/; SameSite=Lax"));
        assert!(!cookie.contains("Secure"));
    }

    #[test]
    fn serves_html_with_headers_without_changing_its_bytes() {
        let root = fixture();
        let html = b"<html><body>same bytes</body></html>";
        fs::write(root.join("index.html"), html).unwrap();

        let response = handle_request(
            &root,
            request(
                "GET",
                "/cgi/qwenpaw/index.cgi/",
                "",
                "qwenpaw_csrf=old",
                false,
            ),
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.body, html);
        assert!(response
            .headers
            .contains(&("Content-Type", "text/html; charset=utf-8".into())));
        assert!(response
            .headers
            .contains(&("X-Content-Type-Options", "nosniff".into())));
        assert!(response
            .headers
            .contains(&("Vary", "Accept-Encoding".into())));
        assert!(response.headers.iter().any(|(name, value)| {
            *name == "Cache-Control" && value == "no-cache, no-store, must-revalidate, max-age=0"
        }));
        assert!(!response
            .headers
            .iter()
            .any(|(name, _)| *name == "Set-Cookie"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn head_keeps_representation_length_and_sends_no_body() {
        let root = fixture();
        fs::write(root.join("index.html"), b"a body for get and head").unwrap();
        let get = handle_request(
            &root,
            request("GET", "/index.cgi/", "", "qwenpaw_csrf=old", false),
        );
        let head = handle_request(
            &root,
            request("HEAD", "/index.cgi/", "", "qwenpaw_csrf=old", false),
        );

        assert_eq!(head.status, 200);
        assert!(head.body.is_empty());
        assert_eq!(head.content_length, get.content_length);
        let mut wire = Vec::new();
        write_response(&mut wire, &head).unwrap();
        assert!(wire.ends_with(b"\r\n\r\n"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compressed_representation_is_sent_byte_for_byte() {
        let root = fixture();
        let html = b"uncompressed html";
        let compressed = [0, 255, 66, 82, 10, 13, 128];
        fs::write(root.join("index.html"), html).unwrap();
        fs::write(root.join("index.html.br"), compressed).unwrap();
        fs::write(root.join("index.html.gz"), [9, 8, 7]).unwrap();

        let response = handle_request(
            &root,
            request(
                "GET",
                "/index.cgi/index.html",
                "gzip;q=1, BR;q=0.4",
                "qwenpaw_csrf=x",
                false,
            ),
        );

        assert_eq!(response.status, 200);
        assert_eq!(response.body, compressed);
        assert!(response
            .headers
            .contains(&("Content-Encoding", "br".into())));
        assert_eq!(response.content_length, compressed.len());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_out_of_root_symlinks_and_does_not_disclose_paths() {
        use std::os::unix::fs::symlink;

        let root = fixture();
        let outside = root.with_extension("outside");
        fs::write(&outside, b"secret bytes").unwrap();
        symlink(&outside, root.join("index.html")).unwrap();
        let response = handle_request(&root, request("GET", "/index.cgi/", "", "", false));

        assert_eq!(response.status, 404);
        assert_eq!(response.body, b"Not Found\n");
        assert!(
            !String::from_utf8_lossy(&response.body).contains(outside.to_string_lossy().as_ref())
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn gzip_fallback_and_identity_follow_explicit_negotiation() {
        let root = fixture();
        fs::write(root.join("index.html"), b"plain").unwrap();
        fs::write(root.join("index.html.gz"), [0, 255, 31, 139]).unwrap();
        for (header, expected, encoding) in [
            ("BR, GZip; Q=1", &[0, 255, 31, 139][..], Some("gzip")),
            ("br;q=0, gzip;q=0, *;q=1", &b"plain"[..], None),
            ("", &b"plain"[..], None),
            ("*;q=0.7", &[0, 255, 31, 139][..], Some("gzip")),
            ("gzip;q=0, *;q=1", &b"plain"[..], None),
        ] {
            let response = handle_request(
                &root,
                request("GET", "/index.cgi/", header, "qwenpaw_csrf=x", false),
            );
            assert_eq!(response.status, 200, "{header}");
            assert_eq!(response.body, expected, "{header}");
            assert_eq!(
                response
                    .headers
                    .iter()
                    .find(|(name, _)| *name == "Content-Encoding")
                    .map(|(_, value)| value.as_str()),
                encoding,
                "{header}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_html_visit_sets_cookie_without_modifying_compressed_body() {
        let root = fixture();
        fs::write(root.join("index.html"), b"unchanged html").unwrap();
        fs::write(root.join("index.html.br"), [0, 255, 13, 10]).unwrap();
        let mut tokens = Vec::new();
        for _ in 0..2 {
            let response = handle_request(&root, request("GET", "/index.cgi/", "br", "", true));
            assert_eq!(response.status, 200);
            assert_eq!(response.body, [0, 255, 13, 10]);
            let cookie = response
                .headers
                .iter()
                .find(|(name, _)| *name == "Set-Cookie")
                .map(|(_, value)| value)
                .unwrap();
            assert!(cookie.ends_with("; Path=/; Secure; SameSite=Lax"));
            assert!(!cookie.contains("Domain="));
            assert!(!cookie.contains("HttpOnly"));
            let token = cookie
                .strip_prefix("__Host-qwenpaw_csrf=")
                .unwrap()
                .split(';')
                .next()
                .unwrap();
            assert_eq!(token.len(), CSRF_TOKEN_BYTES * 2);
            assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
            tokens.push(token.to_string());
        }
        assert_ne!(tokens[0], tokens[1]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn csrf_cookie_requires_exact_name_and_nonempty_value_and_random_failure_is_fatal() {
        for cookie in [
            "other=x",
            "qwenpaw_csrf=",
            "qwenpaw_csrf=  ",
            "xqwenpaw_csrf=x",
        ] {
            let mut random = Cursor::new([0x11_u8; CSRF_TOKEN_BYTES]);
            assert!(
                make_csrf_cookie(cookie, false, &mut random)
                    .unwrap()
                    .is_some(),
                "{cookie}"
            );
        }
        assert!(make_csrf_cookie("", false, &mut io::empty()).is_err());
        assert_eq!(
            make_csrf_cookie("qwenpaw_csrf=x", false, &mut io::empty()).unwrap(),
            None
        );

        for (cookie, secure) in [
            (
                "__Host-qwenpaw_csrf=first; __Host-qwenpaw_csrf=second",
                true,
            ),
            ("qwenpaw_csrf=first; qwenpaw_csrf=second", false),
        ] {
            let mut random = Cursor::new([0x11_u8; CSRF_TOKEN_BYTES]);
            assert!(make_csrf_cookie(cookie, secure, &mut random)
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn head_errors_have_no_body_and_get_errors_do_not_echo_request() {
        let root = fixture();
        for (uri, expected_status, expected_body) in [
            ("/index.cgi/%2e%2e/secret", 400, "Bad Request\n"),
            ("/index.cgi/var/secret", 404, "Not Found\n"),
            ("/index.cgi/", 404, "Not Found\n"),
        ] {
            let get = handle_request(&root, request("GET", uri, "", "", false));
            assert_eq!(get.status, expected_status);
            assert_eq!(get.body, expected_body.as_bytes());
            let head = handle_request(&root, request("HEAD", uri, "", "", false));
            assert_eq!(head.status, expected_status);
            assert!(head.body.is_empty());
            assert_eq!(head.content_length, expected_body.len());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn head_compressed_response_has_same_headers_without_body() {
        let root = fixture();
        fs::write(root.join("index.html"), b"plain html").unwrap();
        fs::write(root.join("index.html.gz"), [0, 255, 31, 139]).unwrap();
        let get = handle_request(
            &root,
            request("GET", "/index.cgi/", "gzip", "qwenpaw_csrf=x", false),
        );
        let head = handle_request(
            &root,
            request("HEAD", "/index.cgi/", "gzip", "qwenpaw_csrf=x", false),
        );
        assert_eq!(head.status, 200);
        assert_eq!(head.headers, get.headers);
        assert_eq!(head.content_length, 4);
        assert!(head.body.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_symlinked_root_and_compressed_sidecars() {
        use std::os::unix::fs::symlink;

        let root = fixture();
        fs::create_dir(root.join("www")).unwrap();
        fs::create_dir(root.join("var")).unwrap();
        fs::write(root.join("www/index.html"), b"plain").unwrap();
        fs::write(root.join("var/secret"), b"secret").unwrap();
        symlink(root.join("var/secret"), root.join("www/index.html.gz")).unwrap();
        symlink(root.join("www"), root.join("linked-www")).unwrap();
        for (www, header) in [(root.join("www"), "gzip"), (root.join("linked-www"), "")] {
            let response = handle_request(&www, request("GET", "/index.cgi/", header, "", false));
            assert_eq!(response.status, 404);
            assert_eq!(response.body, b"Not Found\n");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enforces_size_limit_for_plain_and_compressed_files() {
        let root = fixture();
        fs::write(root.join("index.html"), b"plain").unwrap();
        let oversized = fs::File::create(root.join("index.html.gz")).unwrap();
        oversized.set_len(MAX_STATIC_BYTES + 1).unwrap();
        let response = handle_request(&root, request("GET", "/index.cgi/", "gzip", "", false));
        assert_eq!(response.status, 500);
        assert_eq!(response.body, b"Internal Server Error\n");
        fs::File::create(root.join("index.html"))
            .unwrap()
            .set_len(MAX_STATIC_BYTES + 1)
            .unwrap();
        let response = handle_request(&root, request("GET", "/index.cgi/", "", "", false));
        assert_eq!(response.status, 500);
        assert_eq!(response.body, b"Internal Server Error\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn does_not_serve_sidecars_when_plain_build_artifact_is_missing() {
        let root = fixture();
        fs::write(root.join("index.html.gz"), [0, 255, 31, 139]).unwrap();
        let response = handle_request(&root, request("GET", "/index.cgi/", "gzip", "", false));
        assert_eq!(response.status, 404);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_non_get_methods_with_allow_header() {
        let root = fixture();
        let response = handle_request(&root, request("POST", "/index.cgi/", "", "", false));

        assert_eq!(response.status, 405);
        assert!(response.headers.contains(&("Allow", "GET, HEAD".into())));
        fs::remove_dir_all(root).unwrap();
    }
}
