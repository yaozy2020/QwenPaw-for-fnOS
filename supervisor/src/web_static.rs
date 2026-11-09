use std::{
    fs,
    io::{self, Read},
    path::Path,
};

const MAX_STATIC_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct StaticResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Serves a root-relative path after the transport removes its route prefix.
/// Content-Length describes the selected representation, including for HEAD.

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PathError {
    BadRequest,
    NotFound,
}

#[derive(Debug)]
enum ResourceError {
    NotFound,
    Internal,
}

impl StaticResponse {
    fn new(status: u16, mut headers: Vec<(String, String)>, body: Vec<u8>, head: bool) -> Self {
        headers.push(("Content-Length".into(), body.len().to_string()));
        Self {
            status,
            headers,
            body: if head { Vec::new() } else { body },
        }
    }

    pub(crate) fn error(status: u16, message: &'static str, head: bool) -> Self {
        Self::new(
            status,
            vec![
                ("Content-Type".into(), "text/plain; charset=utf-8".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
            ],
            format!("{message}\n").into_bytes(),
            head,
        )
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

pub(crate) fn uri_path(request_uri: &str) -> Result<&str, PathError> {
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
    Ok(uri_path)
}

fn extract_resource(request_path: &str) -> Result<String, PathError> {
    let uri_path = uri_path(request_path)?;
    if uri_path.is_empty() || uri_path == "/" {
        return Ok("index.html".into());
    }
    let encoded_path = &uri_path.as_bytes()[1..];

    let mut decoded = Vec::with_capacity(encoded_path.len());
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
    let _ = parts.next();
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

pub fn serve(
    root: &Path,
    request_path: &str,
    method: &str,
    accept_encoding: Option<&str>,
) -> StaticResponse {
    let head = method == "HEAD";
    if method != "GET" && !head {
        let mut response = StaticResponse::error(405, "Method Not Allowed", head);
        response.headers.push(("Allow".into(), "GET, HEAD".into()));
        return response;
    }

    let resource = match extract_resource(request_path) {
        Ok(resource) => resource,
        Err(PathError::BadRequest) => return StaticResponse::error(400, "Bad Request", head),
        Err(PathError::NotFound) => return StaticResponse::error(404, "Not Found", head),
    };

    let root = match open_root(root) {
        Ok(root) => root,
        Err(ResourceError::NotFound) => return StaticResponse::error(404, "Not Found", head),
        Err(ResourceError::Internal) => {
            return StaticResponse::error(500, "Internal Server Error", head)
        }
    };

    let (body, encoding) =
        match read_representation(&root, &resource, accept_encoding.unwrap_or_default()) {
            Ok(representation) => representation,
            Err(ResourceError::NotFound) => return StaticResponse::error(404, "Not Found", head),
            Err(ResourceError::Internal) => {
                return StaticResponse::error(500, "Internal Server Error", head)
            }
        };

    let mut headers = vec![
        ("Content-Type".into(), mime_type(&resource).into()),
        ("X-Content-Type-Options".into(), "nosniff".into()),
        ("Vary".into(), "Accept-Encoding".into()),
    ];
    if let Some(encoding) = encoding {
        headers.push(("Content-Encoding".into(), encoding.into()));
    }
    if resource.ends_with(".html") || resource.ends_with(".htm") {
        headers.push((
            "Cache-Control".into(),
            "no-cache, no-store, must-revalidate, max-age=0".into(),
        ));
        headers.push(("Pragma".into(), "no-cache".into()));
        headers.push(("Expires".into(), "0".into()));
    }

    StaticResponse::new(200, headers, body, head)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serve(root: &Path, path: &str, method: &str, encoding: Option<&str>) -> StaticResponse {
        super::serve(root, path, method, encoding)
    }
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "qwenpaw-web-static-{}-{unique}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn write(&self, name: &str, body: &[u8]) {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }

        fn get(&self, path: &str, encoding: Option<&str>) -> StaticResponse {
            serve(&self.0, path, "GET", encoding)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn header<'a>(response: &'a StaticResponse, name: &str) -> Option<&'a str> {
        let mut values = response.headers.iter().filter(|(key, _)| key == name);
        let value = values.next().map(|(_, value)| value.as_str());
        assert!(values.next().is_none(), "duplicate response header: {name}");
        value
    }

    #[test]
    fn serves_only_index_aliases_with_unchanged_html_and_cache_headers() {
        let root = Fixture::new();
        root.write("index.html", b"<html>unchanged bytes</html>");
        for path in ["", "/", "/index.html", "/?cache=1", "/index.html?x=%00"] {
            let response = root.get(path, None);
            assert_eq!(response.status, 200, "{path}");
            assert_eq!(response.body, b"<html>unchanged bytes</html>");
            assert_eq!(
                header(&response, "Content-Type"),
                Some("text/html; charset=utf-8")
            );
            assert_eq!(header(&response, "X-Content-Type-Options"), Some("nosniff"));
            assert_eq!(header(&response, "Vary"), Some("Accept-Encoding"));
            assert_eq!(header(&response, "Content-Length"), Some("28"));
            assert_eq!(
                header(&response, "Cache-Control"),
                Some("no-cache, no-store, must-revalidate, max-age=0")
            );
            assert_eq!(header(&response, "Pragma"), Some("no-cache"));
            assert_eq!(header(&response, "Expires"), Some("0"));
            assert_eq!(header(&response, "Set-Cookie"), None);
        }
    }

    #[test]
    fn serves_allowlisted_assets_with_mime_without_html_headers_or_cookies() {
        let root = Fixture::new();
        for (name, mime) in [
            ("styles/site-v1_2.css", "text/css; charset=utf-8"),
            ("app.js", "application/javascript; charset=utf-8"),
            ("image.jpg", "image/jpeg"),
            ("image.jpeg", "image/jpeg"),
            ("image.png", "image/png"),
            ("image.gif", "image/gif"),
            ("image.svg", "image/svg+xml"),
        ] {
            root.write(&format!("assets/{name}"), &[0, 255, 13, 10]);
            let response = serve(&root.0, &format!("/assets/{name}"), "GET", None);
            assert_eq!(response.status, 200, "{name}");
            assert_eq!(response.body, [0, 255, 13, 10]);
            assert_eq!(header(&response, "Content-Type"), Some(mime));
            assert_eq!(header(&response, "Vary"), Some("Accept-Encoding"));
            assert_eq!(header(&response, "Content-Length"), Some("4"));
            for name in ["Cache-Control", "Pragma", "Expires", "Set-Cookie"] {
                assert_eq!(header(&response, name), None);
            }
        }
    }

    #[test]
    fn rejects_unknown_routes_and_files_outside_build_allowlist_without_spa_fallback() {
        let root = Fixture::new();
        root.write("index.html", b"index");
        for path in [
            "/index.cgi",
            "/app/qwenpaw",
            "/app/qwenpaw/index.html",
            "/api/status",
            "/main.js",
            "/favicon.ico",
            "/index.html.gz",
            "/index.html.br",
            "/var/supervisor-token",
            "/target/debug/supervisor",
            "/assets/.hidden.js",
            "/assets/.hidden/app.js",
            "/assets/app.js.gz",
            "/assets/app.js.br",
            "/assets/app.txt",
            "/assets/APP.JS",
            "/assets/a b.js",
            "/assets/中文.js",
        ] {
            if path != "/app/qwenpaw" {
                root.write(path.trim_start_matches('/'), b"secret");
            }
            let response = root.get(path, None);
            assert_eq!(response.status, 404, "{path}");
            assert_eq!(response.body, b"Not Found\n");
            assert_eq!(header(&response, "Set-Cookie"), None);
        }
    }

    #[test]
    fn rejects_raw_encoded_and_double_encoded_unsafe_paths() {
        let root = Fixture::new();
        root.write("index.html", b"index");
        for path in [
            "/../secret",
            "/assets/../index.html",
            "/assets/./app.js",
            "/assets//app.js",
            "//index.html",
            "/assets/",
            "/%2e%2e/secret",
            "/assets/%2E%2E/index.html",
            "/%252e%252e/secret",
            "/%2fetc/passwd",
            "/assets%2Fapp.js",
            "/%252fetc/passwd",
            "/%00",
            "/assets/app.js\0",
            "/?x=\0",
            "/%2500",
            "/%5cetc%5cpasswd",
            "/assets\\app.js",
            "/%255csecret",
            "/%zz",
            "/%a",
            "/%",
            "/%ff",
            "/%c0%af",
            "/index.html#x",
            "index.html",
            "http://example.invalid/index.html",
        ] {
            let response = root.get(path, None);
            assert_eq!(response.status, 400, "{path:?}");
            assert_eq!(response.body, b"Bad Request\n");
        }
        assert_eq!(
            root.get(&format!("/{}", "a".repeat(8192)), None).status,
            400
        );
    }

    #[test]
    fn decodes_safe_path_octets_once_and_ignores_query() {
        let root = Fixture::new();
        root.write("index.html", b"html");
        root.write("assets/app.js", b"script");
        assert_eq!(root.get("/%69ndex.html?x=1", None).body, b"html");
        assert_eq!(
            root.get("/assets/%61pp.js?x=/../secret", None).body,
            b"script"
        );
    }

    #[test]
    fn control_character_errors_never_echo_request_or_inject_headers() {
        let root = Fixture::new();
        for path in [
            "/missing-\x01.js",
            "/missing-\x1b.js",
            "/missing-\x7f.js",
            "/missing-\r\nX-Injected: bad.js",
            "/missing-%0d%0aX-Injected%3a%20bad.js",
        ] {
            let response = root.get(path, None);
            assert_eq!(response.status, 404, "{path:?}");
            assert_eq!(response.body, b"Not Found\n");
            assert!(response.headers.iter().all(|(name, value)| {
                !name.contains(['\r', '\n']) && !value.contains(['\r', '\n'])
            }));
        }
    }

    #[test]
    fn unsupported_methods_return_allow_without_cookie() {
        let root = Fixture::new();
        root.write("index.html", b"html");
        for method in ["POST", "PUT", "DELETE", "OPTIONS", "get", "head", ""] {
            let response = serve(&root.0, "/", method, None);
            assert_eq!(response.status, 405, "{method}");
            assert_eq!(response.body, b"Method Not Allowed\n");
            assert_eq!(header(&response, "Allow"), Some("GET, HEAD"));
            assert_eq!(header(&response, "Content-Length"), Some("19"));
            assert_eq!(header(&response, "Set-Cookie"), None);
        }
    }

    #[test]
    fn head_matches_get_headers_and_length_without_body_on_success_and_errors() {
        let root = Fixture::new();
        root.write("index.html", b"plain html");
        root.write("index.html.gz", &[0, 255, 31, 139]);
        for (path, encoding, status) in [
            ("/", None, 200),
            ("/index.html", Some("gzip"), 200),
            ("/../secret", None, 400),
            ("/var/token", None, 404),
            ("/assets/missing.js", None, 404),
        ] {
            let get = root.get(path, encoding);
            let head = serve(&root.0, path, "HEAD", encoding);
            assert_eq!(get.status, status, "{path}");
            assert_eq!(head.status, get.status);
            assert_eq!(head.headers, get.headers);
            assert_eq!(
                header(&head, "Content-Length"),
                Some(get.body.len().to_string().as_str())
            );
            assert!(head.body.is_empty());
        }
    }

    #[test]
    fn compression_preserves_brotli_priority_and_honors_q_zero() {
        let root = Fixture::new();
        root.write("index.html", b"plain");
        root.write("index.html.br", &[0, 255, 66, 82]);
        root.write("index.html.gz", &[31, 139, 0]);
        for (encoding, expected_encoding, expected_body) in [
            ("gzip;q=1, BR;q=0.4", Some("br"), &[0, 255, 66, 82][..]),
            ("br;q=0, gzip;q=1", Some("gzip"), &[31, 139, 0][..]),
            ("br;q=0, gzip;q=0, *;q=1", None, &b"plain"[..]),
            ("gzip;q=0, *;q=0.5", Some("br"), &[0, 255, 66, 82][..]),
            ("*;q=0", None, &b"plain"[..]),
            ("Br; Q=0.8", Some("br"), &[0, 255, 66, 82][..]),
            ("br;q=NaN, gzip;q=-1", None, &b"plain"[..]),
            ("br;q=1.1, gzip;q=invalid", None, &b"plain"[..]),
            ("br;q=0, br;q=1, gzip;q=0", None, &b"plain"[..]),
            ("", None, &b"plain"[..]),
        ] {
            let response = root.get("/", Some(encoding));
            assert_eq!(response.status, 200, "{encoding}");
            assert_eq!(response.body, expected_body, "{encoding}");
            assert_eq!(
                header(&response, "Content-Encoding"),
                expected_encoding,
                "{encoding}"
            );
            assert_eq!(
                header(&response, "Content-Type"),
                Some("text/html; charset=utf-8")
            );
            assert_eq!(header(&response, "Vary"), Some("Accept-Encoding"));
        }
    }

    #[test]
    fn compression_falls_back_to_gzip_then_identity_but_requires_plain_file() {
        let root = Fixture::new();
        root.write("assets/app.js", b"script");
        root.write("assets/app.js.gz", &[0, 255, 31, 139]);
        let gzip = root.get("/assets/app.js", Some("br, gzip"));
        assert_eq!(gzip.status, 200);
        assert_eq!(gzip.body, [0, 255, 31, 139]);
        assert_eq!(header(&gzip, "Content-Encoding"), Some("gzip"));
        assert_eq!(
            header(&gzip, "Content-Type"),
            Some("application/javascript; charset=utf-8")
        );
        let plain = root.get("/assets/app.js", Some("br, gzip;q=0"));
        assert_eq!(plain.body, b"script");
        assert_eq!(header(&plain, "Content-Encoding"), None);
        fs::remove_file(root.0.join("assets/app.js")).unwrap();
        assert_eq!(root.get("/assets/app.js", Some("gzip")).status, 404);
    }

    #[test]
    fn rejects_symlinked_root_file_and_intermediate_directory() {
        use std::os::unix::fs::symlink;

        let root = Fixture::new();
        root.write("outside/app.js", b"secret");
        fs::create_dir(root.0.join("www")).unwrap();
        symlink(root.0.join("outside/app.js"), root.0.join("www/index.html")).unwrap();
        symlink(root.0.join("outside"), root.0.join("www/assets")).unwrap();
        symlink(root.0.join("www"), root.0.join("linked-www")).unwrap();
        for (directory, path) in [("www", "/"), ("www", "/assets/app.js"), ("linked-www", "/")] {
            let response = serve(&root.0.join(directory), path, "GET", None);
            assert_eq!(response.status, 404);
            assert_eq!(response.body, b"Not Found\n");
        }
    }

    #[test]
    fn rejects_symlinked_sidecars_instead_of_falling_back_to_plain() {
        use std::os::unix::fs::symlink;

        let root = Fixture::new();
        root.write("index.html", b"html");
        root.write("secret", b"secret");
        for (encoding, suffix) in [("br", "br"), ("gzip", "gz")] {
            symlink(
                root.0.join("secret"),
                root.0.join(format!("index.html.{suffix}")),
            )
            .unwrap();
            let response = root.get("/", Some(encoding));
            assert_eq!(response.status, 404);
            assert_eq!(response.body, b"Not Found\n");
        }
    }

    #[test]
    fn rejects_non_regular_files_without_blocking() {
        let root = Fixture::new();
        fs::create_dir(root.0.join("index.html")).unwrap();
        assert_eq!(root.get("/", None).status, 404);
        fs::create_dir(root.0.join("assets")).unwrap();
        let path =
            std::ffi::CString::new(root.0.join("assets/pipe.js").as_os_str().as_encoded_bytes())
                .unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert_eq!(root.get("/assets/pipe.js", None).status, 404);
    }

    #[test]
    fn enforces_plain_and_sidecar_size_limit_for_get_and_head() {
        let root = Fixture::new();
        root.write("index.html", b"html");
        for (file, encoding) in [
            ("index.html.br", Some("br")),
            ("index.html.gz", Some("gzip")),
            ("index.html", None),
        ] {
            fs::File::create(root.0.join(file))
                .unwrap()
                .set_len(MAX_STATIC_BYTES + 1)
                .unwrap();
            for method in ["GET", "HEAD"] {
                let response = serve(&root.0, "/", method, encoding);
                assert_eq!(response.status, 500, "{file} {method}");
                assert_eq!(header(&response, "Content-Length"), Some("22"));
                assert_eq!(header(&response, "Set-Cookie"), None);
                if method == "HEAD" {
                    assert!(response.body.is_empty());
                } else {
                    assert_eq!(response.body, b"Internal Server Error\n");
                }
            }
        }
    }

    #[test]
    fn accepts_file_exactly_at_size_limit() {
        let root = Fixture::new();
        fs::create_dir(root.0.join("assets")).unwrap();
        fs::File::create(root.0.join("assets/app.js"))
            .unwrap()
            .set_len(MAX_STATIC_BYTES)
            .unwrap();
        let response = root.get("/assets/app.js", None);
        assert_eq!(response.status, 200);
        assert_eq!(response.body.len() as u64, MAX_STATIC_BYTES);
    }

    #[test]
    fn missing_root_and_missing_index_return_generic_not_found() {
        let root = Fixture::new();
        for directory in [&root.0, &root.0.join("missing")] {
            let response = serve(directory, "/", "GET", None);
            assert_eq!(response.status, 404);
            assert_eq!(response.body, b"Not Found\n");
        }
    }
}
