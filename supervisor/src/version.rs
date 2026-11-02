//! PEP 440 版本解析、稳定/Beta 分类与语义排序
//! 后端唯一权威实现：统一 CGI、监督者与前端三方口径。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pep440Version {
    pub raw: String,
    pub release: Vec<u64>,
    pub stage: u8, // 0=dev, 1=alpha, 2=beta, 3=rc, 4=final, 5=post
    pub stage_num: u64,
    pub post_num: u64,
    pub dev_num: u64,
}

impl PartialOrd for Pep440Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pep440Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 1. release 元组依次比对
        let max_len = self.release.len().max(other.release.len());
        for i in 0..max_len {
            let a = self.release.get(i).copied().unwrap_or(0);
            let b = other.release.get(i).copied().unwrap_or(0);
            if a != b {
                return a.cmp(&b);
            }
        }
        // 2. 阶段比对 (dev < a < b < rc < final < post)
        if self.stage != other.stage {
            return self.stage.cmp(&other.stage);
        }
        // 3. 阶段号比对 (b2 > b1, rc2 > rc1)
        if self.stage_num != other.stage_num {
            return self.stage_num.cmp(&other.stage_num);
        }
        // 4. post 编号比对
        if self.post_num != other.post_num {
            return self.post_num.cmp(&other.post_num);
        }
        // 5. dev 编号比对
        self.dev_num.cmp(&other.dev_num)
    }
}

impl Pep440Version {
    pub fn parse(raw: &str) -> Option<Self> {
        let clean = raw.trim().trim_start_matches(['v', 'V']);
        if clean.is_empty() {
            return None;
        }

        // 分离 release 部分与后缀部分
        // 例如 2.3.0b2 -> release: [2, 3, 0], rest: "b2"
        let mut idx = 0;
        let bytes = clean.as_bytes();
        let mut release = Vec::new();
        let mut current_num: Option<u64> = None;

        while idx < bytes.len() {
            let b = bytes[idx];
            if b.is_ascii_digit() {
                let digit = (b - b'0') as u64;
                current_num = Some(current_num.unwrap_or(0) * 10 + digit);
                idx += 1;
            } else if b == b'.' {
                if let Some(n) = current_num.take() {
                    release.push(n);
                    idx += 1;
                    // 如果点后面不是数字，则可能是 .dev, .post, .b 等后缀
                    if idx < bytes.len() && !bytes[idx].is_ascii_digit() {
                        break;
                    }
                } else {
                    return None;
                }
            } else {
                // 遇到非数字且非点，进入后缀解析阶段
                break;
            }
        }
        if let Some(n) = current_num.take() {
            release.push(n);
        }
        if release.is_empty() {
            return None;
        }

        let mut rest = clean[idx..].to_ascii_lowercase();
        // 规范化 rest 前导符号
        if rest.starts_with('.') || rest.starts_with('-') || rest.starts_with('_') {
            rest = rest[1..].to_string();
        }

        let mut stage = 4u8; // 默认为 final
        let mut stage_num = 0u64;
        let mut post_num = 0u64;
        let mut dev_num = 0u64;

        if !rest.is_empty() {
            if rest.contains("post") {
                stage = 5;
                post_num = extract_number(&rest);
            } else if rest.contains("dev") {
                stage = 0;
                dev_num = extract_number(&rest);
            } else if rest.starts_with("rc") || rest.starts_with('c') || rest.starts_with("pre") {
                stage = 3;
                stage_num = extract_number(&rest);
            } else if rest.starts_with('b') || rest.starts_with("beta") {
                stage = 2;
                stage_num = extract_number(&rest);
            } else if rest.starts_with('a') || rest.starts_with("alpha") {
                stage = 1;
                stage_num = extract_number(&rest);
            } else {
                // 其他情况兜底归入 dev (0)
                stage = 0;
                stage_num = extract_number(&rest);
            }
        }

        Some(Self {
            raw: raw.to_string(),
            release,
            stage,
            stage_num,
            post_num,
            dev_num,
        })
    }

    pub fn is_beta(&self) -> bool {
        self.stage < 4
    }
}

fn extract_number(s: &str) -> u64 {
    let mut num = 0u64;
    let mut found = false;
    for b in s.bytes() {
        if b.is_ascii_digit() {
            found = true;
            num = num.saturating_mul(10).saturating_add((b - b'0') as u64);
        } else if found {
            break;
        }
    }
    num
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionItem {
    pub version: String,
    pub is_beta: bool,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionListResponse {
    pub success: bool,
    pub versions: Vec<VersionItem>,
    pub latest_stable: String,
    pub latest_beta: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// 计算当前版本与选定版本之间的操作动作
pub fn determine_action(current: &str, selected: &str) -> &'static str {
    let cur_clean = current.trim().trim_start_matches(['v', 'V']);
    let sel_clean = selected.trim().trim_start_matches(['v', 'V']);
    if cur_clean.is_empty() {
        return "install";
    }
    if cur_clean == sel_clean {
        return "reinstall";
    }
    let cur_v = Pep440Version::parse(cur_clean);
    let sel_v = Pep440Version::parse(sel_clean);
    match (cur_v, sel_v) {
        (Some(c), Some(s)) => {
            if s > c {
                "upgrade"
            } else if s < c {
                "downgrade"
            } else {
                "reinstall"
            }
        }
        _ => "upgrade",
    }
}

/// 对原始版本字符串列表做 PEP 440 权威排序并生成结构化响应
pub fn build_version_list(raw_versions: &[String], current_version: &str) -> VersionListResponse {
    let mut parsed: Vec<Pep440Version> = raw_versions
        .iter()
        .filter_map(|v| Pep440Version::parse(v))
        .collect();

    // 降序排序：最新版本排在最前面
    parsed.sort_by(|a, b| b.cmp(a));
    parsed.dedup_by(|a, b| a.raw == b.raw);

    // 取前 14 个版本
    parsed.truncate(14);

    let mut latest_stable = String::new();
    let mut latest_beta = String::new();

    for v in &parsed {
        if !v.is_beta() && latest_stable.is_empty() {
            latest_stable = v.raw.clone();
        } else if v.is_beta() && latest_beta.is_empty() {
            latest_beta = v.raw.clone();
        }
    }

    let items: Vec<VersionItem> = parsed
        .into_iter()
        .map(|v| {
            let action = determine_action(current_version, &v.raw).to_string();
            VersionItem {
                is_beta: v.is_beta(),
                action,
                version: v.raw,
            }
        })
        .collect();

    VersionListResponse {
        success: true,
        versions: items,
        latest_stable,
        latest_beta,
        message: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pep440_order_cases() {
        let v2_9_9 = Pep440Version::parse("2.9.9").unwrap();
        let v2_10_0 = Pep440Version::parse("2.10.0").unwrap();
        assert!(v2_10_0 > v2_9_9, "2.10.0 必须大于 2.9.9");

        let dev = Pep440Version::parse("2.1.0.dev1").unwrap();
        let alpha = Pep440Version::parse("2.1.0a1").unwrap();
        let beta = Pep440Version::parse("2.1.0b2").unwrap();
        let rc = Pep440Version::parse("2.1.0rc1").unwrap();
        let final_v = Pep440Version::parse("2.1.0").unwrap();
        let post = Pep440Version::parse("2.1.0.post1").unwrap();

        assert!(dev < alpha);
        assert!(alpha < beta);
        assert!(beta < rc);
        assert!(rc < final_v);
        assert!(final_v < post);

        assert!(dev.is_beta());
        assert!(alpha.is_beta());
        assert!(beta.is_beta());
        assert!(rc.is_beta());
        assert!(!final_v.is_beta());
        assert!(!post.is_beta());
    }

    #[test]
    fn determine_action_logic() {
        assert_eq!(determine_action("", "2.1.0"), "install");
        assert_eq!(determine_action("2.1.0", "2.1.0"), "reinstall");
        assert_eq!(determine_action("2.0.0", "2.1.0"), "upgrade");
        assert_eq!(determine_action("2.1.0", "2.0.0"), "downgrade");
        assert_eq!(determine_action("2.1.0", "2.1.0b1"), "downgrade");
        assert_eq!(determine_action("2.1.0b1", "2.1.0"), "upgrade");
    }

    #[test]
    fn build_version_list_output() {
        let list = vec![
            "2.0.0".into(),
            "2.10.0".into(),
            "2.9.0".into(),
            "2.1.0b2".into(),
            "2.1.0".into(),
        ];
        let res = build_version_list(&list, "2.1.0");
        assert!(res.success);
        assert_eq!(res.latest_stable, "2.10.0");
        assert_eq!(res.latest_beta, "2.1.0b2");
        assert_eq!(res.versions[0].version, "2.10.0");
        assert_eq!(res.versions[0].action, "upgrade");
        assert_eq!(res.versions.iter().find(|v| v.version == "2.1.0").unwrap().action, "reinstall");
        assert_eq!(res.versions.iter().find(|v| v.version == "2.0.0").unwrap().action, "downgrade");
    }
}
