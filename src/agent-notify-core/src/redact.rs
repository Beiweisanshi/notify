use sha2::{Digest, Sha256};

const DETAIL_LIMIT: usize = 160;

pub fn sanitize_summary(input: impl AsRef<str>) -> String {
    let mut value = input.as_ref().replace(['\r', '\n', '\t'], " ");
    for marker in ["token=", "api_key=", "apikey=", "authorization:", "bearer "] {
        value = redact_after_marker(&value, marker);
    }
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&compact, DETAIL_LIMIT)
}

pub fn safe_detail_for_tool(tool_name: Option<&str>, fallback: &str) -> String {
    match tool_name.map(str::to_ascii_lowercase).as_deref() {
        Some("bash") | Some("shell") | Some("powershell") | Some("cmd") => {
            "请求执行 shell 命令，参数已隐藏".to_string()
        }
        Some(name) if !name.trim().is_empty() => {
            truncate_chars(&format!("{name} 工具请求处理，参数已隐藏"), DETAIL_LIMIT)
        }
        _ => sanitize_summary(fallback),
    }
}

pub fn summary_hash(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

pub fn stable_event_id(parts: &[&str]) -> String {
    summary_hash(parts)
}

fn redact_after_marker(input: &str, marker: &str) -> String {
    let lower = input.to_ascii_lowercase();
    if let Some(index) = lower.find(marker) {
        let value_start = index + marker.len();
        let end = input[value_start..]
            .find(char::is_whitespace)
            .map(|offset| value_start + offset)
            .unwrap_or(input.len());
        format!("{}[REDACTED]{}", &input[..index], &input[end..])
    } else {
        input.to_string()
    }
}

pub fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut output = input
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token_like_values() {
        let redacted = sanitize_summary("run token=secret123 and Authorization: Bearer abc");
        assert!(!redacted.contains("secret123"));
        assert!(!redacted.contains("abc"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn truncates_detail_to_policy_limit() {
        let long = "a".repeat(200);
        assert_eq!(sanitize_summary(long).chars().count(), 160);
    }
}
