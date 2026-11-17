use std::{
    collections::HashMap,
    io::Read,
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamNote {
    pub body: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotesResponse {
    pub available: bool,
    pub note: String,
    pub note_zh: String,
    pub source: String,
    pub error: String,
}

#[derive(Default)]
pub struct NotesCache {
    entries: Mutex<HashMap<String, UpstreamNote>>,
}

impl NotesCache {
    pub fn lookup_with<F>(&self, version: &str, fetch: F) -> NotesResponse
    where
        F: FnOnce() -> Result<UpstreamNote, String>,
    {
        let note_zh = local_summary(version).to_string();
        let cached = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(version)
            .cloned();
        let upstream = match cached {
            Some(note) => Ok(note),
            None => fetch().inspect(|note| {
                self.entries
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(version.to_string(), note.clone());
            }),
        };
        match upstream {
            Ok(note) => NotesResponse {
                available: true,
                note: note.body,
                note_zh,
                source: note.source,
                error: String::new(),
            },
            Err(_) => NotesResponse {
                available: false,
                note: String::new(),
                source: if note_zh.is_empty() {
                    "unavailable".into()
                } else {
                    "local".into()
                },
                note_zh,
                error: "上游 Release 暂时无法获取，请稍后重试".into(),
            },
        }
    }
}

pub fn lookup(version: &str) -> NotesResponse {
    static CACHE: OnceLock<NotesCache> = OnceLock::new();
    CACHE
        .get_or_init(NotesCache::default)
        .lookup_with(version, || fetch_upstream(version))
}

fn local_summary(version: &str) -> &'static str {
    match version {
        "2.3.0" => {
            "新增与改进\n• Piccolo 4B、9B、27B 本地决策模型，可用于工具审核与上下文压缩，支持 Hugging Face 和 ModelScope 下载。\n• 新增顾问模式与实验性 Python Code Mode；桌面窗口可共享后台服务，关闭窗口后任务继续运行。\n• 控制台支持 QwenPaw 社区浏览、排序、评论与编辑器，并可同步部分消息到收件箱。\n• ReMe 升级，增加图片记忆、自动标签与标签检索；新增 OpenViking 记忆插件。\n• 支持本地 ChatGPT 订阅登录、决策模型工具审批、OpenUI 交互组件和中英文评测页面。\n• 插件安装、更新与依赖回滚改进，Skill 下载支持取消，增加 Firecrawl 与 Tavily 搜索配置。\n\n主要修复\n• 修复会话草稿、长对话导航、模型设置、媒体预览与历史记录的一致性问题。\n• 改进插件依赖检查、共享升级与中断恢复，失败时保留已安装内容。\n• 修复反向代理子路径下的路由、资源地址和登录跳转。\n• 更新安装依赖；对应更新与插件安装路径要求 pip 26.2.2 或 uv 0.13.4 及以上。"
        }
        "2.3.0-beta.4" | "2.3.0b4" => {
            "• 修复渠道更新时的插件依赖保留与渠道启动问题。\n• 支持中断后的增量依赖安装恢复。\n• 社区支持远程密码登录并保存凭据。"
        }
        "2.2.2.post1" => {
            "• 修复依赖、安装器与构建流程中的 emSign CA 信任问题。\n• Node.js 更新至 22.23.4，包含安全修复。"
        }
        _ => "",
    }
}

pub fn fetch_upstream(version: &str) -> Result<UpstreamNote, String> {
    fetch_upstream_with_bases(version, "https://api.github.com", "https://github.com")
}

fn fetch_upstream_with_bases(
    version: &str,
    api_base: &str,
    web_base: &str,
) -> Result<UpstreamNote, String> {
    if crate::version::Pep440Version::parse(version).is_none() {
        return Err("invalid version".into());
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("QwenPaw-for-fnOS")
        .build()
        .map_err(|error| error.to_string())?;
    let api_url = format!(
        "{}/repos/agentscope-ai/QwenPaw/releases/tags/v{version}",
        api_base.trim_end_matches('/')
    );
    if let Ok(note) = fetch_api_note(&client, &api_url) {
        return Ok(note);
    }
    let web_url = format!(
        "{}/agentscope-ai/QwenPaw/releases/tag/v{version}",
        web_base.trim_end_matches('/')
    );
    fetch_html_note(&client, &web_url)
}

fn read_limited(response: reqwest::blocking::Response, limit: u64) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err("response too large".into());
    }
    let mut body = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    if body.len() as u64 > limit {
        return Err("response too large".into());
    }
    Ok(body)
}

fn fetch_api_note(client: &reqwest::blocking::Client, url: &str) -> Result<UpstreamNote, String> {
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpstreamNote {
            body: String::new(),
            source: "none".into(),
        });
    }
    if !response.status().is_success() {
        return Err(format!("GitHub API returned {}", response.status()));
    }
    let body = read_limited(response, 2 * 1024 * 1024)?;
    let release: serde_json::Value =
        serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    let note = release
        .get("body")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(UpstreamNote {
        body: note,
        source: "github-api".into(),
    })
}

fn fetch_html_note(client: &reqwest::blocking::Client, url: &str) -> Result<UpstreamNote, String> {
    let response = client.get(url).send().map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpstreamNote {
            body: String::new(),
            source: "none".into(),
        });
    }
    if !response.status().is_success() {
        return Err(format!("GitHub HTML returned {}", response.status()));
    }
    let body = read_limited(response, 6 * 1024 * 1024)?;
    let document = scraper::Html::parse_document(&String::from_utf8_lossy(&body));
    let selector = scraper::Selector::parse("[data-test-selector=\"body-content\"]")
        .map_err(|_| "invalid release body selector".to_string())?;
    let content = document
        .select(&selector)
        .next()
        .ok_or_else(|| "release body not found".to_string())?
        .inner_html();
    Ok(UpstreamNote {
        body: flatten_html(&content),
        source: "github-html".into(),
    })
}

fn flatten_html(html: &str) -> String {
    let mut output = String::new();
    let mut chars = html.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '<' {
            let mut tag = String::new();
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
                tag.push(next);
            }
            let tag = tag.trim().to_ascii_lowercase();
            if tag == "li" {
                output.push_str("\n• ");
            } else if tag == "br"
                || tag.starts_with("/h")
                || matches!(tag.as_str(), "/p" | "/div" | "/li" | "/ul" | "/ol" | "/pre")
            {
                output.push('\n');
            }
        } else {
            output.push(character);
        }
    }
    let output = output
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            if lines.last().is_some_and(|previous| !previous.is_empty()) {
                lines.push(line);
            }
        } else {
            lines.push(line);
        }
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn serve(status: u16, body: &str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (url, server)
    }

    #[test]
    fn successful_fetch_is_cached_but_failure_keeps_local_summary_and_allows_retry() {
        let cache = NotesCache::default();
        let mut calls = 0;
        let unavailable = cache.lookup_with("2.3.0", || {
            calls += 1;
            Err("offline".into())
        });
        assert!(!unavailable.available);
        assert!(unavailable.note_zh.contains("Piccolo"));
        assert_eq!(unavailable.source, "local");

        let fetched = cache.lookup_with("2.3.0", || {
            calls += 1;
            Ok(UpstreamNote {
                body: "Upstream features".into(),
                source: "github-api".into(),
            })
        });
        assert!(fetched.available);
        assert_eq!(fetched.note, "Upstream features");
        assert_eq!(fetched.note_zh, unavailable.note_zh);

        let cached = cache.lookup_with("2.3.0", || -> Result<UpstreamNote, String> {
            panic!("a successful result must not be fetched again")
        });
        assert_eq!(cached.note, "Upstream features");
        assert_eq!(calls, 2);
    }

    #[test]
    fn api_failure_falls_back_to_release_html_and_flattens_text() {
        let (api, api_server) = serve(403, "rate limited");
        let (web, web_server) = serve(
            200,
            r#"<html><body><div data-test-selector="body-content" class="markdown-body"><h3>New Features</h3><ul><li>Improved <a href="/pr/1">dependency recovery</a></li><li>Second item</li></ul></div></body></html>"#,
        );
        let note = fetch_upstream_with_bases("2.3.0", &api, &web).unwrap();
        api_server.join().unwrap();
        web_server.join().unwrap();
        assert_eq!(note.source, "github-html");
        assert!(note.body.contains("• Improved dependency recovery"));
        assert!(note.body.contains("• Second item"));
        assert!(!note.body.contains("<a"));
    }

    #[test]
    fn missing_release_is_a_successful_empty_result_and_is_cached() {
        let (api, api_server) = serve(404, "not found");
        let cache = NotesCache::default();
        let missing = cache.lookup_with("9.9.9", || {
            fetch_upstream_with_bases("9.9.9", &api, "http://127.0.0.1:1")
        });
        api_server.join().unwrap();
        assert!(missing.available);
        assert_eq!(missing.source, "none");
        assert!(missing.note.is_empty());
        assert!(missing.note_zh.is_empty());
        let cached = cache.lookup_with("9.9.9", || -> Result<UpstreamNote, String> {
            panic!("an empty release result must also be cached")
        });
        assert!(cached.available);
        assert_eq!(cached.source, "none");
    }
}
