#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Downloading,
    Installing,
    Starting,
    Done,
    Failed,
    /// 取消不恢复服务，避免安装任务对抗关停。
    Cancelled,
}

/// 线上 phase 字符串(与旧桥/控制台兼容):取消也归入 failed —— 控制台只认
/// done/failed 终态,"已被停止"的细节走 message/error 字段,不扩协议。
pub fn phase_wire(p: &Phase) -> &'static str {
    match p {
        Phase::Idle => "idle",
        Phase::Downloading => "downloading",
        Phase::Installing => "installing",
        Phase::Starting => "starting",
        Phase::Done => "done",
        Phase::Failed | Phase::Cancelled => "failed",
    }
}

/// 安装渠道:latest / beta / 精确版本。精确版本经 PEP 440 字符白名单校验后
/// 以 `qwenpaw=={v}` 单个 argv 传给 pip;latest/beta 是枚举而非字符串拼装。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallChannel {
    Latest,
    Beta,
    Exact(String),
}

impl InstallChannel {
    /// 控制台/桥送来的 version 字段 → 渠道;非法值返回 None(调用方拒绝请求)。
    pub fn parse(v: &str) -> Option<InstallChannel> {
        match v {
            "latest" => Some(InstallChannel::Latest),
            "beta" | "pre" => Some(InstallChannel::Beta),
            other if is_safe_exact_version(other) => Some(InstallChannel::Exact(other.to_string())),
            _ => None,
        }
    }
}

/// 精确版本白名单:仅 PEP 440 合法字符(字母数字 . _ + -),不得以 . 或 - 开头
/// (后者会被 pip 当作选项)。长度上限 64。凭此拒绝 requirement 语法、URL、
/// 本地路径、git+https、附加参数与一切 Shell 元字符 —— 不依赖 pip 解析报错,
/// 也不自行实现不完整的 PEP 440 语义校验。
pub(super) fn is_safe_exact_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
        && !v.starts_with('.')
        && !v.starts_with('-')
}

pub fn uv_bootstrap_args() -> Vec<String> {
    ["install", "--upgrade", "--disable-pip-version-check", "uv"]
        .iter()
        .map(|value| (*value).to_string())
        .collect()
}

/// uv 只向当前暂存 venv 安装；镜像经 UV_DEFAULT_INDEX 传递。
pub fn uv_args(channel: &InstallChannel) -> Vec<String> {
    let mut args: Vec<String> = ["pip", "install", "--upgrade"]
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    if channel == &InstallChannel::Beta {
        args.push("--pre".into());
    }
    args.push(match channel {
        InstallChannel::Exact(version) => format!("qwenpaw=={version}"),
        _ => "qwenpaw".into(),
    });
    args
}

/// Mirror URLs use the same narrow alphabet at platform configuration and install time.
pub fn validate_mirror(mirror: &str) -> Result<(), String> {
    if mirror.is_empty() {
        return Ok(());
    }
    if mirror.len() > 2048 {
        return Err("下载源地址过长".into());
    }

    let remainder = mirror
        .strip_prefix("https://")
        .or_else(|| mirror.strip_prefix("http://"))
        .ok_or_else(|| "下载源需为 http(s):// 开头且仅含 URL 安全字符的地址".to_string())?;
    if !mirror
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b":/._~-".contains(&byte))
    {
        return Err("下载源需为 http(s):// 开头且仅含 URL 安全字符的地址".into());
    }
    let authority = remainder.split('/').next().unwrap_or_default();
    if authority.is_empty() || !authority.bytes().any(|byte| byte.is_ascii_alphanumeric()) {
        return Err("下载源需为 http(s):// 开头且仅含 URL 安全字符的地址".into());
    }
    Ok(())
}

pub fn resolve_mirror(mirror: &str) -> Result<String, String> {
    let resolved = match mirror {
        "tsinghua" => "https://pypi.tuna.tsinghua.edu.cn/simple",
        "aliyun" => "https://mirrors.aliyun.com/pypi/simple/",
        "ustc" => "https://pypi.mirrors.ustc.edu.cn/simple/",
        value => value,
    };
    validate_mirror(resolved)?;
    Ok(resolved.to_string())
}

/// Legacy install callers use an empty mirror to select the official index.
/// Keep that fallback for compatibility while platform mirror writes use the
/// strict Result-returning validator so invalid input cannot become empty.
pub fn sanitize_mirror(mirror: &str) -> String {
    if validate_mirror(mirror).is_ok() {
        mirror.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod mirror_validation_tests {
    use super::{resolve_mirror, validate_mirror};

    #[test]
    fn allows_explicit_official_and_strict_http_mirrors() {
        assert!(validate_mirror("").is_ok());
        assert!(validate_mirror("http://mirror.example/simple").is_ok());
        assert!(validate_mirror("https://mirror.example:8443/simple/").is_ok());
    }

    #[test]
    fn resolves_shared_aliases_and_validates_custom_mirrors() {
        assert_eq!(
            resolve_mirror("tsinghua").unwrap(),
            "https://pypi.tuna.tsinghua.edu.cn/simple"
        );
        assert_eq!(
            resolve_mirror("aliyun").unwrap(),
            "https://mirrors.aliyun.com/pypi/simple/"
        );
        assert_eq!(
            resolve_mirror("ustc").unwrap(),
            "https://pypi.mirrors.ustc.edu.cn/simple/"
        );
        assert_eq!(
            resolve_mirror("https://mirror.example/custom/simple").unwrap(),
            "https://mirror.example/custom/simple"
        );
        assert!(resolve_mirror("https://mirror.example/simple;bad").is_err());
    }

    #[test]
    fn rejects_malformed_schemes_and_unsafe_characters() {
        for invalid in [
            "ftp://mirror.example/simple",
            "https://",
            "https:///simple",
            "https://mirror.example/simple;rm",
            "https://mirror.example/simple?x=1",
            "https://mirror.example/有中文",
            " https://mirror.example/simple",
            "https://mirror.example/simple\\n",
        ] {
            assert!(validate_mirror(invalid).is_err(), "accepted {invalid:?}");
        }
    }
}
