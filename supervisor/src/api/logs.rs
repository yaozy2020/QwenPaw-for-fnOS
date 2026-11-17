use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;

use super::{check_token, err_json, AppState};
use crate::install::truncate_utf8;

fn tail_lines(path: &std::path::Path, max: usize, window: u64) -> Vec<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    read_tail_lines(&mut file, len, max, window).unwrap_or_default()
}

fn read_tail_lines(
    reader: &mut (impl std::io::Read + std::io::Seek),
    len: u64,
    max: usize,
    window: u64,
) -> std::io::Result<Vec<String>> {
    const LINE_CAP: usize = 2000;
    const BLOCK_BYTES: u64 = 8192;
    if max == 0 || window == 0 || len == 0 {
        return Ok(Vec::new());
    }
    let lower_bound = len.saturating_sub(window);
    let mut start = len;
    let mut chunks = Vec::new();
    let mut newlines = 0usize;
    let mut trailing_newline = false;
    while start > lower_bound {
        let size = (start - lower_bound).min(BLOCK_BYTES);
        start -= size;
        reader.seek(std::io::SeekFrom::Start(start))?;
        let mut chunk = vec![0; size as usize];
        reader.read_exact(&mut chunk)?;
        if chunks.is_empty() {
            trailing_newline = chunk.last() == Some(&b'\n');
        }
        newlines += chunk.iter().filter(|&&byte| byte == b'\n').count();
        chunks.push(chunk);
        if newlines.saturating_sub(usize::from(trailing_newline)) >= max {
            break;
        }
    }
    let buf = if chunks.len() == 1 {
        chunks.pop().unwrap()
    } else {
        chunks.into_iter().rev().flatten().collect()
    };
    let text = String::from_utf8_lossy(&buf);
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    let mut lines: Vec<_> = text
        .lines()
        .rev()
        .take(max)
        .map(|line| truncate_utf8(line, LINE_CAP))
        .collect();
    lines.reverse();
    Ok(lines)
}

/// 内核运行日志尾部(旧形状:{success, logs: [字符串数组]})。
pub(super) async fn logs(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let tail = req
        .uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("tail="))
                .map(|p| p[5..].parse::<usize>().ok())
        })
        .flatten()
        .unwrap_or(80)
        .clamp(1, 2000);
    let log_path = std::path::PathBuf::from(&state.kernel_log);
    let logs = tokio::task::spawn_blocking(move || tail_lines(&log_path, tail, 256 * 1024))
        .await
        .unwrap_or_default();
    Json(json!({"success": true, "logs": logs})).into_response()
}

pub(super) async fn logs_clear(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    // 截断落盘移入 spawn_blocking:与全库 async 卫生纪律一致(审查 P2-7)
    let log_path = state.kernel_log.clone();
    let _ = tokio::task::spawn_blocking(move || std::fs::write(&log_path, "")).await;
    Json(json!({"success": true, "message": "日志已清理"})).into_response()
}

/// pip 日志尾部(Rust 侧 JSON 转义,替代桥侧 python3;含引号/反斜杠安全)
pub(super) async fn install_log(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
) -> axum::response::Response {
    if !check_token(&state.token, &req) {
        return err_json(StatusCode::UNAUTHORIZED, "invalid token");
    }
    let query = req.uri().query().unwrap_or_default();
    let mut seq = None;
    let mut instance = None;
    for field in query.split('&') {
        if let Some(value) = field.strip_prefix("seq=") {
            seq = value.parse::<u64>().ok();
        } else if let Some(value) = field.strip_prefix("instance=") {
            instance = Some(value);
        }
    }
    let snapshot = state.install.snapshot();
    let matches = seq == Some(snapshot.seq)
        && instance.is_some_and(|value| !value.is_empty() && value == super::instance_id());
    if !matches {
        return err_json(StatusCode::NOT_FOUND, "安装任务身份不匹配");
    }
    let tail = query
        .split('&')
        .find_map(|field| field.strip_prefix("tail="))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(40)
        .clamp(1, 500);
    let path = std::path::PathBuf::from(format!("{}/qwenpaw-upgrade.log.pip", state.var_dir));
    let lines = tokio::task::spawn_blocking(move || tail_lines(&path, tail, 256 * 1024))
        .await
        .unwrap_or_default();
    Json(json!({"success": true, "logs": lines})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, Read, Seek, SeekFrom};

    #[derive(Default)]
    struct CountedReader {
        cursor: Cursor<Vec<u8>>,
        bytes_read: usize,
    }

    impl Read for CountedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.cursor.read(buffer)?;
            self.bytes_read += count;
            Ok(count)
        }
    }

    impl Seek for CountedReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.cursor.seek(position)
        }
    }

    fn reference(bytes: &[u8], max: usize, window: u64) -> Vec<String> {
        let start = bytes.len().saturating_sub(window as usize);
        let text = String::from_utf8_lossy(&bytes[start..]);
        let mut lines: Vec<_> = text.lines().map(|line| truncate_utf8(line, 2000)).collect();
        if start > 0 && !lines.is_empty() {
            lines.remove(0);
        }
        let tail: Vec<_> = lines.into_iter().rev().take(max).collect();
        tail.into_iter().rev().collect()
    }

    #[test]
    fn tail_matches_existing_line_window_and_utf8_rules() {
        let cases = [
            Vec::new(),
            b"one".to_vec(),
            b"one\n".to_vec(),
            b"\n\n\n".to_vec(),
            b"one\r\ntwo\r\nthree\r".to_vec(),
            b"one\ntwo\nthree".to_vec(),
            b"one\ntwo\nthree\n".to_vec(),
            b"invalid\xff\xfe\nlast\n".to_vec(),
            format!("first\n{}\nlast\n", "中".repeat(4000)).into_bytes(),
            (0..10_000)
                .map(|index| format!("日志 {index}\r\n"))
                .collect::<String>()
                .into_bytes(),
        ];
        for bytes in cases {
            for max in [0, 1, 2, 40, 100, 2000] {
                for window in [0, 1, 5, 8191, 8192, 8193, 256 * 1024] {
                    let actual =
                        read_tail_lines(&mut Cursor::new(&bytes), bytes.len() as u64, max, window)
                            .unwrap();
                    assert_eq!(
                        actual,
                        reference(&bytes, max, window),
                        "len={} max={max} window={window}",
                        bytes.len()
                    );
                }
            }
        }
    }

    #[test]
    fn short_tail_reads_only_the_needed_end_blocks() {
        let bytes = b"ordinary log line\n".repeat(100_000);
        let len = bytes.len() as u64;
        let mut reader = CountedReader {
            cursor: Cursor::new(bytes),
            ..Default::default()
        };
        let lines = read_tail_lines(&mut reader, len, 100, 256 * 1024).unwrap();
        assert_eq!(lines, vec!["ordinary log line"; 100]);
        assert!(
            reader.bytes_read <= 8192,
            "100-line refresh read {} bytes",
            reader.bytes_read
        );
    }

    #[test]
    fn tail_does_not_read_appendages_beyond_the_captured_file_length() {
        let mut reader = CountedReader {
            cursor: Cursor::new(b"first\nsecond\nappended\n".to_vec()),
            ..Default::default()
        };
        let lines = read_tail_lines(&mut reader, 13, 100, 256 * 1024).unwrap();
        assert_eq!(lines, ["first", "second"]);
        assert_eq!(reader.bytes_read, 13);
    }

    #[test]
    fn oversized_lines_never_expand_the_read_window() {
        let bytes = vec![b'x'; 1024 * 1024];
        let mut reader = CountedReader {
            cursor: Cursor::new(bytes),
            ..Default::default()
        };
        let lines = read_tail_lines(&mut reader, 1024 * 1024, 100, 256 * 1024).unwrap();
        assert!(lines.is_empty());
        assert_eq!(reader.bytes_read, 256 * 1024);
    }

    #[tokio::test]
    async fn log_routes_preserve_auth_tail_limits_and_install_identity() {
        use crate::api::{
            build_router, instance_id,
            test_support::{free_port, TestDir},
        };
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;

        let dir = TestDir::new("log-tail");
        let state = dir.state(free_port());
        let bytes: String = (0..4000).map(|index| format!("日志 {index}\n")).collect();
        std::fs::write(&state.kernel_log, &bytes).unwrap();
        std::fs::write(format!("{}/qwenpaw-upgrade.log.pip", state.var_dir), &bytes).unwrap();
        let router = build_router(state.clone());
        let seq = state.install.snapshot().seq;
        for (path, authenticated, status, count) in [
            ("/logs?tail=100".to_owned(), false, 401, 0),
            ("/logs?tail=100".to_owned(), true, 200, 100),
            ("/logs?tail=9999".to_owned(), true, 200, 2000),
            ("/logs?tail=0".to_owned(), true, 200, 1),
            ("/kernel/install-log?tail=100".to_owned(), true, 404, 0),
            (
                format!(
                    "/kernel/install-log?seq={seq}&instance={}&tail=9999",
                    instance_id()
                ),
                true,
                200,
                500,
            ),
        ] {
            let mut request = Request::builder().uri(&path);
            if authenticated {
                request = request.header("X-Supervisor-Token", &state.token);
            }
            let response = router
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status, "{path}");
            if status == 200 {
                let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
                    .await
                    .unwrap();
                let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(value["success"], true);
                let logs = value["logs"].as_array().unwrap();
                assert_eq!(logs.len(), count);
                assert_eq!(logs.last().unwrap(), "日志 3999");
            }
        }
    }
}
