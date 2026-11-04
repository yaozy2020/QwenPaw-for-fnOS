use std::fmt;

use reqwest::Url;

#[derive(Debug, PartialEq, Eq)]
pub enum IndexError {
    InvalidMirror(String),
    Request,
    HttpStatus(u16),
    TooLarge,
    Empty,
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMirror(message) => write!(f, "已保存的 PyPI 镜像地址无效: {message}"),
            Self::Request => f.write_str("请求 PyPI Simple Index 失败"),
            Self::HttpStatus(status) => write!(f, "PyPI Simple Index 返回 HTTP {status}"),
            Self::TooLarge => f.write_str("PyPI Simple Index 响应超过 4 MiB 上限"),
            Self::Empty => f.write_str("PyPI Simple Index 未返回可用的 QwenPaw 版本"),
        }
    }
}

pub fn index_url(mirror: &str) -> Result<Url, IndexError> {
    let sanitized = crate::install::sanitize_mirror(mirror);
    if sanitized != mirror {
        return Err(IndexError::InvalidMirror(
            "下载源需为 http(s):// 开头且仅含 URL 安全字符的地址".into(),
        ));
    }
    let value = if sanitized.is_empty() {
        "https://pypi.org/simple/qwenpaw/".to_string()
    } else {
        format!("{}/qwenpaw/", sanitized.trim_end_matches('/'))
    };
    Url::parse(&value).map_err(|_| IndexError::InvalidMirror("URL 格式无效".into()))
}

pub fn parse_simple_index(base: &Url, html: &str) -> Result<Vec<String>, IndexError> {
    use percent_encoding::percent_decode_str;
    use scraper::{Html, Selector};
    use std::collections::BTreeSet;

    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href]").expect("static link selector");
    let mut versions = BTreeSet::new();
    for anchor in document.select(&selector) {
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let Ok(link) = base.join(href) else {
            continue;
        };
        let Some(encoded_filename) = link.path().rsplit('/').next() else {
            continue;
        };
        let Ok(filename) = percent_decode_str(encoded_filename).decode_utf8() else {
            continue;
        };
        if let Some(version) = artifact_version(&filename) {
            versions.insert(version.to_string());
        }
    }

    if versions.is_empty() {
        return Err(IndexError::Empty);
    }
    Ok(versions.into_iter().collect())
}

fn artifact_version(filename: &str) -> Option<&str> {
    let lower = filename.to_ascii_lowercase();
    let version = if lower.ends_with(".whl") {
        let stem = &filename[..filename.len() - ".whl".len()];
        let parts: Vec<_> = stem.split('-').collect();
        if parts.len() < 5 || normalize_distribution(parts[0]) != "qwenpaw" {
            return None;
        }
        parts[1]
    } else {
        let suffix_len = [".tar.gz", ".zip"]
            .into_iter()
            .find(|suffix| lower.ends_with(suffix))?
            .len();
        let stem = &filename[..filename.len() - suffix_len];
        let (distribution, version) = stem.split_once('-')?;
        if normalize_distribution(distribution) != "qwenpaw" {
            return None;
        }
        version
    };
    crate::version::Pep440Version::parse(version).map(|_| version)
}

fn normalize_distribution(name: &str) -> String {
    let mut normalized = String::new();
    let mut pending_separator = false;
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            pending_separator = !normalized.is_empty();
        } else {
            if pending_separator {
                normalized.push('-');
                pending_separator = false;
            }
            normalized.push(character.to_ascii_lowercase());
        }
    }
    normalized
}

pub fn fetch_versions(url: &Url) -> Result<Vec<String>, IndexError> {
    use std::time::Duration;

    fetch_versions_with_timeouts(url, Duration::from_secs(5), Duration::from_secs(15))
}

fn fetch_versions_with_timeouts(
    url: &Url,
    connect_timeout: std::time::Duration,
    total_timeout: std::time::Duration,
) -> Result<Vec<String>, IndexError> {
    use std::io::Read;

    const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(total_timeout)
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("QwenPaw")
        .build()
        .map_err(|_| IndexError::Request)?;
    let response = client
        .get(url.clone())
        .send()
        .map_err(|_| IndexError::Request)?;
    if !response.status().is_success() {
        return Err(IndexError::HttpStatus(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err(IndexError::TooLarge);
    }

    let index_url = response.url().clone();
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(4096)
            .min(MAX_RESPONSE_BYTES) as usize,
    );
    response
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| IndexError::Request)?;
    if body.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(IndexError::TooLarge);
    }
    parse_simple_index(&index_url, &String::from_utf8_lossy(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{BTreeSet, HashMap},
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    fn route(path: &str, status: u16, headers: &[(&str, &str)], body: &str) -> (String, String) {
        let mut response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        response.push_str("\r\n");
        response.push_str(body);
        (path.to_string(), response)
    }

    fn route_without_content_length(path: &str, status: u16, body: &str) -> (String, String) {
        (
            path.to_string(),
            format!("HTTP/1.1 {status} Test\r\nConnection: close\r\n\r\n{body}"),
        )
    }

    fn serve(routes: Vec<(String, String)>) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let routes: HashMap<_, _> = routes.into_iter().collect();
        let route_count = routes.len();
        thread::spawn(move || {
            for _ in 0..route_count {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                if let Some(response) = routes.get(path) {
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        Url::parse(&format!("http://{address}/simple/qwenpaw/")).unwrap()
    }

    #[test]
    fn parses_wheels_sdists_encoded_names_entities_and_relative_links() {
        let base = Url::parse("http://mirror.example/simple/qwenpaw/").unwrap();
        let html = r#"
            <html><body>
              <a href="../../files/qwenpaw-2.0.0-py3-none-any.whl">wheel</a>
              <a href="qwenpaw-2.0.0.tar.gz#sha256=abc">duplicate sdist</a>
              <a href="qwenpaw-2.1.0.zip?download=1&amp;source=index">zip</a>
              <a href="qwenpaw-2.2.0%2Bnas-py3-none-any.whl">encoded wheel</a>
              <a href="qwenpawx-99.0.0.tar.gz">different project</a>
              <a href="qwenpaw-addon-8.0.0-py3-none-any.whl">different project</a>
              <a href="other-1.0.0-py3-none-any.whl">different project</a>
            </body></html>
        "#;
        let actual = parse_simple_index(&base, html).unwrap();
        assert_eq!(
            actual.into_iter().collect::<BTreeSet<_>>(),
            ["2.0.0", "2.1.0", "2.2.0+nas"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }

    #[test]
    fn rejects_index_without_target_project_releases() {
        let base = Url::parse("http://mirror.example/simple/qwenpaw/").unwrap();
        assert_eq!(
            parse_simple_index(&base, "<a href='other-1.0.zip'>other</a>"),
            Err(IndexError::Empty)
        );
        assert_eq!(parse_simple_index(&base, ""), Err(IndexError::Empty));
    }

    #[test]
    fn builds_official_and_selected_mirror_urls_but_rejects_invalid_saved_values() {
        assert_eq!(
            index_url("").unwrap().as_str(),
            "https://pypi.org/simple/qwenpaw/"
        );
        assert_eq!(
            index_url("https://mirror.example/custom/simple/")
                .unwrap()
                .as_str(),
            "https://mirror.example/custom/simple/qwenpaw/"
        );
        assert!(matches!(
            index_url("https://mirror.example/simple;bad"),
            Err(IndexError::InvalidMirror(_))
        ));
    }

    #[test]
    fn fetches_a_local_index_and_resolves_links_against_its_url() {
        let base = serve(vec![route(
            "/simple/qwenpaw/",
            200,
            &[("Content-Type", "text/html")],
            "<a href='../files/qwenpaw-3.0.0.tar.gz'>release</a>",
        )]);
        assert_eq!(fetch_versions(&base).unwrap(), ["3.0.0"]);
    }

    #[test]
    fn rejects_non_success_http_statuses() {
        let url = serve(vec![route(
            "/simple/qwenpaw/",
            503,
            &[],
            "temporarily unavailable",
        )]);
        assert_eq!(fetch_versions(&url), Err(IndexError::HttpStatus(503)));
    }

    #[test]
    fn rejects_responses_larger_than_four_mib() {
        let body = "x".repeat(4 * 1024 * 1024 + 1);
        let url = serve(vec![route_without_content_length(
            "/simple/qwenpaw/",
            200,
            &body,
        )]);
        assert_eq!(fetch_versions(&url), Err(IndexError::TooLarge));
    }

    #[test]
    fn rejects_empty_simple_index_responses() {
        let url = serve(vec![route("/simple/qwenpaw/", 200, &[], "<html></html>")]);
        assert_eq!(fetch_versions(&url), Err(IndexError::Empty));
    }

    #[test]
    fn enforces_the_total_request_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            thread::sleep(Duration::from_millis(100));
            let response = route("/", 200, &[], "<html></html>").1;
            let _ = stream.write_all(response.as_bytes());
        });
        let url = Url::parse(&format!("http://{address}/simple/qwenpaw/")).unwrap();
        let start = std::time::Instant::now();

        assert_eq!(
            fetch_versions_with_timeouts(&url, Duration::from_secs(1), Duration::from_millis(20),),
            Err(IndexError::Request)
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }

    #[test]
    fn follows_at_most_three_redirects() {
        let routes = vec![
            route("/r0", 302, &[("Location", "/r1")], ""),
            route("/r1", 302, &[("Location", "/r2")], ""),
            route("/r2", 302, &[("Location", "/r3")], ""),
            route("/r3", 200, &[], "<a href='qwenpaw-3.1.0.zip'>release</a>"),
        ];
        let url = serve(routes);
        let url = url.join("/r0").unwrap();
        assert_eq!(fetch_versions(&url).unwrap(), ["3.1.0"]);

        let routes = vec![
            route("/r0", 302, &[("Location", "/r1")], ""),
            route("/r1", 302, &[("Location", "/r2")], ""),
            route("/r2", 302, &[("Location", "/r3")], ""),
            route("/r3", 302, &[("Location", "/r4")], ""),
        ];
        let url = serve(routes).join("/r0").unwrap();
        assert_eq!(fetch_versions(&url), Err(IndexError::Request));
    }
}
