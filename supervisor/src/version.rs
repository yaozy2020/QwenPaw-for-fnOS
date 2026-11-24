//! PEP 440 版本解析、稳定/Beta 分类与语义排序。
//! 后端唯一实现：统一监督者与前端版本口径。
//!
//! 覆盖范围（非完整 PEP 440）：epoch、release、pre/post/dev 阶段与编号。
//! 未实现 local version(`+abc`)、隐式 post(`1.0-1`)与任意 dash/underscore 归一化的全部细节。
//! 对 QwenPaw 实际存在的版本号足够；遇到未知后缀会保守归入 dev 且不做静默丢弃。

use serde::{Deserialize, Serialize};

pub fn verified_kernel_version() -> &'static str {
    include_str!("../../patches/verified-version").trim_end()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pep440Version {
    pub raw: String,
    pub epoch: u64,
    pub release: Vec<u64>,
    pub stage: u8, // 0=dev-only, 1=alpha, 2=beta, 3=rc, 4=final; post/dev are separate
    pub stage_num: u64,
    pub post_num: Option<u64>,
    pub dev_num: Option<u64>,
}

impl PartialOrd for Pep440Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pep440Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // 0. epoch 是 PEP 440 的最高位
        if self.epoch != other.epoch {
            return self.epoch.cmp(&other.epoch);
        }
        // 1. release 元组依次比对
        let max_len = self.release.len().max(other.release.len());
        for i in 0..max_len {
            let a = self.release.get(i).copied().unwrap_or(0);
            let b = other.release.get(i).copied().unwrap_or(0);
            if a != b {
                return a.cmp(&b);
            }
        }
        // 2. dev-only < alpha < beta < rc < final；组合后缀另行比较
        if self.stage != other.stage {
            return self.stage.cmp(&other.stage);
        }
        if self.stage_num != other.stage_num {
            return self.stage_num.cmp(&other.stage_num);
        }
        // 无 post < post；a1.post1 仍属于 alpha 阶段
        if self.post_num != other.post_num {
            return self.post_num.cmp(&other.post_num);
        }
        // 同阶段与 post 下，dev 早于非 dev
        match (self.dev_num, other.dev_num) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    }
}

impl Pep440Version {
    pub fn parse(raw: &str) -> Option<Self> {
        let clean = raw.trim().trim_start_matches(['v', 'V']);
        if clean.is_empty() {
            return None;
        }

        // epoch:`1!2.0`。PEP 440 规定 epoch 参与最高位比较,忽略会把 1!2.0 排到 2.0 之后
        let (epoch, clean) = match clean.split_once('!') {
            Some((e, rest)) => {
                let e: u64 = e.trim().parse().ok()?;
                (e, rest)
            }
            None => (0, clean),
        };

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
                current_num = Some(
                    current_num
                        .unwrap_or(0)
                        .checked_mul(10)?
                        .checked_add(digit)?,
                );
                idx += 1;
            } else if b == b'.' {
                // 点前必须是数字,否则是 .dev/.post 之类的后缀分隔符或非法输入
                let n = current_num.take()?;
                release.push(n);
                idx += 1;
                // 如果点后面不是数字，则可能是 .dev, .post, .b 等后缀
                if idx < bytes.len() && !bytes[idx].is_ascii_digit() {
                    break;
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

        let mut stage = 4u8;
        let mut stage_num = 0u64;
        let mut post_num = None;
        let mut dev_num = None;

        if !rest.is_empty() {
            let original_rest = rest.clone();
            let mut remainder = rest.as_str();
            let mut has_pre = false;
            for (prefix, pre_stage) in [
                ("alpha", 1u8),
                ("beta", 2),
                ("pre", 3),
                ("rc", 3),
                ("a", 1),
                ("b", 2),
                ("c", 3),
            ] {
                if let Some((tail, number)) = consume_prefixed_number(remainder, prefix) {
                    stage = pre_stage;
                    stage_num = number;
                    has_pre = true;
                    remainder = strip_suffix_separator(tail);
                    break;
                }
            }
            if let Some((tail, number)) = consume_prefixed_number(remainder, "post") {
                post_num = Some(number);
                remainder = strip_suffix_separator(tail);
            }
            if let Some((tail, number)) = consume_prefixed_number(remainder, "dev") {
                dev_num = Some(number);
                remainder = tail;
                if !has_pre && post_num.is_none() {
                    stage = 0;
                }
            }
            if !remainder.is_empty() {
                stage = 0;
                stage_num = extract_number(&original_rest);
                post_num = None;
                dev_num = None;
            }
        }

        Some(Self {
            raw: raw.to_string(),
            epoch,
            release,
            stage,
            stage_num,
            post_num,
            dev_num,
        })
    }

    pub fn is_beta(&self) -> bool {
        self.stage < 4 || self.dev_num.is_some()
    }
}

fn strip_suffix_separator(value: &str) -> &str {
    if value.starts_with(['.', '-', '_']) {
        &value[1..]
    } else {
        value
    }
}

fn consume_prefixed_number<'a>(value: &'a str, prefix: &str) -> Option<(&'a str, u64)> {
    let tail = value.strip_prefix(prefix)?;
    let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, remainder) = tail.split_at(digits);
    let number = digits.bytes().fold(0u64, |number, byte| {
        number
            .saturating_mul(10)
            .saturating_add((byte - b'0') as u64)
    });
    Some((remainder, number))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VersionListResponse {
    pub success: bool,
    pub versions: Vec<VersionItem>,
    pub latest_stable: String,
    pub latest_beta: String,
    pub recommended_version: Option<String>,
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
pub fn build_version_list(
    raw_versions: &crate::version_index::Releases,
    current_version: &str,
) -> VersionListResponse {
    let mut parsed: Vec<Pep440Version> = raw_versions
        .keys()
        .filter_map(|v| Pep440Version::parse(v))
        .collect();

    // 降序排序：最新版本排在最前面
    parsed.sort_by(|a, b| b.cmp(a));
    parsed.dedup_by(|a, b| a.raw == b.raw);

    let verified = verified_kernel_version();
    let recommended = parsed.iter().position(|v| v.raw == verified);

    let mut latest_stable = String::new();
    let mut latest_beta = String::new();

    for v in &parsed {
        if !v.is_beta() && latest_stable.is_empty() {
            latest_stable = v.raw.clone();
        } else if v.is_beta() && latest_beta.is_empty() {
            latest_beta = v.raw.clone();
        }
    }

    parsed.sort_by(|a, b| raw_versions[&b.raw].cmp(&raw_versions[&a.raw]));
    let items: Vec<VersionItem> = parsed
        .into_iter()
        .map(|v| {
            let action = determine_action(current_version, &v.raw).to_string();
            VersionItem {
                published_at: raw_versions[&v.raw].clone(),
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
        recommended_version: recommended.map(|_| verified.to_owned()),
        message: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(versions: &[String]) -> crate::version_index::Releases {
        versions.iter().cloned().map(|v| (v, None)).collect()
    }

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
    fn pep440_combined_pre_post_and_dev_segments_keep_their_order() {
        let ordered = [
            "2.3.0.dev1",
            "2.3.0a1.dev1",
            "2.3.0a1",
            "2.3.0a1.post1.dev1",
            "2.3.0a1.post1",
            "2.3.0b1",
            "2.3.0b1.post1",
            "2.3.0",
            "2.3.0.post1.dev1",
            "2.3.0.post1",
        ]
        .map(|raw| Pep440Version::parse(raw).unwrap());
        for pair in ordered.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{} must precede {}",
                pair[0].raw,
                pair[1].raw
            );
        }
        assert!(Pep440Version::parse("2.3.0b1.post1").unwrap().is_beta());
        assert!(Pep440Version::parse("2.3.0.post1.dev1").unwrap().is_beta());
        assert!(!Pep440Version::parse("2.3.0.post1").unwrap().is_beta());
    }

    #[test]
    fn epoch_outranks_release() {
        // PEP 440:1!2.0 > 2.0
        let a = Pep440Version::parse("1!2.0").unwrap();
        let b = Pep440Version::parse("2.0").unwrap();
        assert!(a > b, "epoch 必须优先于 release 比较");
        assert_eq!(a.epoch, 1);
        assert_eq!(b.epoch, 0);
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
    fn all_releases_and_the_recommendation_remain_selectable() {
        let mut mirror: Vec<String> = (0..20).map(|n| format!("98.{n}.0")).collect();
        let verified = "2.3.0.post1";
        mirror.push(verified.into());
        let list = build_version_list(&index(&mirror), "99.0.0");
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(json["recommended_version"], verified);
        assert_eq!(list.latest_stable, "98.19.0");
        assert_eq!(list.versions.len(), 21);
        assert!(list
            .versions
            .iter()
            .any(|v| v.version == verified && v.action == "downgrade"));
    }

    #[test]
    fn missing_verified_version_is_reported_as_null() {
        let list = build_version_list(&index(&["98.0.0".into()]), "");
        let json = serde_json::to_value(list).unwrap();
        assert_eq!(
            json.get("recommended_version"),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(json["latest_stable"], "98.0.0");
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
        let res = build_version_list(&index(&list), "2.1.0");
        assert!(res.success);
        assert_eq!(res.latest_stable, "2.10.0");
        assert_eq!(res.latest_beta, "2.1.0b2");
        assert_eq!(res.versions[0].version, "2.10.0");
        assert_eq!(res.versions[0].action, "upgrade");
        assert_eq!(
            res.versions
                .iter()
                .find(|v| v.version == "2.1.0")
                .unwrap()
                .action,
            "reinstall"
        );
        assert_eq!(
            res.versions
                .iter()
                .find(|v| v.version == "2.0.0")
                .unwrap()
                .action,
            "downgrade"
        );
    }
}
