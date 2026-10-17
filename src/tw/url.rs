//! Just enough of the WHATWG URL parser for `looksLikeUri`: does `new URL(s)` succeed with a
//! non-empty `host`?

const SPECIAL: &[&str] = &["http", "https", "ws", "wss", "ftp", "file"];

fn forbidden_host_char(c: char) -> bool {
    matches!(c, '\0' | '\t' | '\n' | '\r' | ' ' | '#' | '/' | ':' | '<' | '>' | '?' | '@' | '[' | '\\' | ']' | '^' | '|' | '%')
}

pub fn is_valid_url_with_host(s: &str) -> bool {
    let s = s.trim_matches(|c: char| c <= ' ');
    let s: String = s.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let Some(colon) = s.find(':') else { return false };
    let scheme = &s[..colon];
    let mut chars = scheme.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return false;
    }
    let scheme = scheme.to_ascii_lowercase();
    let rest = &s[colon + 1..];
    let special = SPECIAL.contains(&scheme.as_str());
    let authority = if special {
        if scheme == "file" {
            return false;
        }
        rest.trim_start_matches(['/', '\\'])
    } else if let Some(r) = rest.strip_prefix("//") {
        r
    } else {
        return false;
    };
    let end = authority.find(|c: char| c == '/' || c == '?' || c == '#' || (special && c == '\\')).unwrap_or(authority.len());
    let auth = &authority[..end];
    let host_port = match auth.rfind('@') {
        Some(i) => &auth[i + 1..],
        None => auth,
    };
    let host = if host_port.starts_with('[') {
        match host_port.find(']') {
            Some(i) => &host_port[..=i],
            None => return false,
        }
    } else {
        match host_port.rfind(':') {
            Some(i) => {
                let port = &host_port[i + 1..];
                if !port.chars().all(|c| c.is_ascii_digit()) || port.parse::<u64>().map(|p| p > 65535).unwrap_or(false) {
                    return false;
                }
                &host_port[..i]
            }
            None => host_port,
        }
    };
    if host.is_empty() {
        return false;
    }
    if host.starts_with('[') {
        return true;
    }
    !host.chars().any(forbidden_host_char)
}
