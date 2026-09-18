pub mod backends;
pub mod client;
pub mod paths;
pub mod plugins;
pub mod store;
pub mod worker;
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn redact(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = ["https://", "http://", "socks5://"]
        .iter()
        .filter_map(|prefix| remaining.find(prefix))
        .min()
    {
        output.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let end = remaining
            .find(|c: char| c.is_whitespace() || matches!(c, '\"' | '\'' | '<' | '>'))
            .unwrap_or(remaining.len());
        let candidate = remaining[..end].trim_end_matches([')', ',', ';']);
        match url::Url::parse(candidate) {
            Ok(mut url) => {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                output.push_str(url.as_str());
            }
            Err(_) => output.push_str("[redacted URL]"),
        }
        output.push_str(&remaining[candidate.len()..end]);
        remaining = &remaining[end..];
    }
    output.push_str(remaining);
    output
}
pub fn safe_filename(value: &str) -> String {
    let mut s: String = value
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(160)
        .collect();
    // Filename limits are byte-based on Unix; leave room for suffixes on every OS.
    let mut boundary = s.len().min(160);
    while !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    s.truncate(boundary);
    let s = s.trim().trim_matches('.').trim();
    let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if s.is_empty() {
        "download".into()
    } else if matches!(
        base.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "COM¹"
            | "COM²"
            | "COM³"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
            | "LPT¹"
            | "LPT²"
            | "LPT³"
    ) {
        format!("_{s}")
    } else {
        s.to_string()
    }
}
