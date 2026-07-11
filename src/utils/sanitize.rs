// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

const SENSITIVE_KEYS: [&str; 16] = [
    "token",
    "password",
    "accesskey",
    "access_key",
    "access-key",
    "secretkey",
    "secret_key",
    "secret-key",
    "clientsecret",
    "client_secret",
    "client-secret",
    "sessiontoken",
    "session_token",
    "session-token",
    "credential",
    "credentials",
];

pub(crate) fn redact_sensitive_pairs(message: &str) -> String {
    let message = redact_sensitive_xml_tags(message);
    redact_sensitive_key_value_pairs(&message)
}

fn is_sensitive_key(key: &str) -> bool {
    matches!(
        normalize_key(key).as_str(),
        "token"
            | "password"
            | "accesskey"
            | "secretkey"
            | "clientsecret"
            | "sessiontoken"
            | "credential"
            | "credentials"
    )
}

fn normalize_key(raw: &str) -> String {
    raw.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn redact_sensitive_xml_tags(message: &str) -> String {
    let mut output = String::with_capacity(message.len());
    let mut cursor = 0usize;

    while cursor < message.len() {
        let Some(ch) = message[cursor..].chars().next() else {
            break;
        };

        if ch == '<'
            && let Some(replacement) = redact_xml_tag_at(message, cursor)
        {
            output.push_str(&replacement.redacted);
            cursor = replacement.end;
            continue;
        }

        output.push(ch);
        cursor += ch.len_utf8();
    }

    output
}

struct XmlRedaction {
    redacted: String,
    end: usize,
}

fn redact_xml_tag_at(message: &str, cursor: usize) -> Option<XmlRedaction> {
    let tag_end = cursor + message[cursor..].find('>')?;
    let tag_content = &message[cursor + 1..tag_end];
    if tag_content.starts_with('/') || tag_content.starts_with('?') || tag_content.starts_with('!')
    {
        return None;
    }
    let tag_name_end = tag_content
        .find(|ch: char| ch.is_whitespace() || ch == '/')
        .unwrap_or(tag_content.len());
    let tag_name = &tag_content[..tag_name_end];
    if tag_name.is_empty() || !is_sensitive_key(tag_name) {
        return None;
    }

    let open_end = tag_end + 1;
    let close = format!("</{tag_name}>");
    let close_start = open_end + message[open_end..].find(&close)?;
    let close_end = close_start + close.len();

    Some(XmlRedaction {
        redacted: format!(
            "{}<redacted>{}",
            &message[cursor..open_end],
            &message[close_start..close_end]
        ),
        end: close_end,
    })
}

fn redact_sensitive_key_value_pairs(message: &str) -> String {
    let bytes = message.as_bytes();
    let mut output = String::with_capacity(message.len());
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        let mut matched = false;

        for key in SENSITIVE_KEYS {
            let key_len = key.len();

            let unquoted_match = matches_key_at(message, cursor, key);
            let quoted_match = cursor + key_len + 2 <= bytes.len()
                && matches!(bytes[cursor] as char, '"' | '\'')
                && bytes[cursor + key_len + 1] == bytes[cursor]
                && matches_key_at(message, cursor + 1, key);

            let (key_start, key_end, cursor_after_key) = if unquoted_match {
                if cursor > 0 {
                    let prev = bytes[cursor - 1] as char;
                    if prev.is_ascii_alphanumeric() || prev == '_' || prev == '-' {
                        continue;
                    }
                }
                (cursor, cursor + key_len, cursor + key_len)
            } else if quoted_match {
                let key_start = cursor + 1;
                (key_start, key_start + key_len, key_start + key_len + 1)
            } else {
                continue;
            };

            let candidate = &message[key_start..key_end];

            let sep_index = skip_whitespace(message, cursor_after_key);
            if sep_index >= bytes.len() || !matches!(bytes[sep_index] as char, '=' | ':') {
                continue;
            }

            let value_start = skip_whitespace(message, sep_index + 1);
            let value_end = parse_value_end(message, value_start);
            if value_end <= value_start || !is_sensitive_key(candidate) {
                continue;
            }

            output.push_str(&message[cursor..value_start]);
            output.push_str(&redacted_value(&message[value_start..value_end]));
            cursor = value_end;
            matched = true;
            break;
        }

        if !matched {
            let Some(ch) = message[cursor..].chars().next() else {
                break;
            };
            output.push(ch);
            cursor += ch.len_utf8();
        }
    }

    output
}

fn parse_value_end(input: &str, start: usize) -> usize {
    if start >= input.len() {
        return start;
    }

    let mut chars = input[start..].char_indices();
    let Some((_, first)) = chars.next() else {
        return start;
    };
    if first == '"' || first == '\'' {
        let mut previous = first;
        for (offset, ch) in chars {
            if ch == first && previous != '\\' {
                return start + offset + ch.len_utf8();
            }
            previous = ch;
        }
        return input.len();
    }

    for (offset, ch) in input[start..].char_indices() {
        if ch.is_whitespace() || matches!(ch, ',' | ';' | '}' | ']' | ')') {
            return start + offset;
        }
    }
    input.len()
}

fn skip_whitespace(input: &str, start: usize) -> usize {
    for (offset, ch) in input[start..].char_indices() {
        if !ch.is_whitespace() {
            return start + offset;
        }
    }
    input.len()
}

fn matches_key_at(message: &str, start: usize, key: &str) -> bool {
    let end = start + key.len();
    end <= message.len()
        && message.is_char_boundary(start)
        && message.is_char_boundary(end)
        && message[start..end].eq_ignore_ascii_case(key)
}

fn redacted_value(original: &str) -> String {
    if original.len() >= 2 {
        let bytes = original.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            let quote = first as char;
            return format!("{quote}<redacted>{quote}");
        }
    }
    "<redacted>".to_string()
}

#[cfg(test)]
mod tests {
    use super::redact_sensitive_pairs;

    #[test]
    fn preserves_required_key_names() {
        let message = "Vault backend requires kmsSecret referencing a Secret with key vault-token";

        assert_eq!(redact_sensitive_pairs(message), message);
    }

    #[test]
    fn redacts_colon_and_json_secret_values() {
        let message =
            "kms config token: tok_123 password: p@ss accesskey: AKIA_TEST secretkey: SK_TEST";

        let sanitized = redact_sensitive_pairs(message);

        assert!(sanitized.contains("token"));
        assert!(sanitized.contains("password"));
        assert!(sanitized.contains("accesskey"));
        assert!(sanitized.contains("secretkey"));
        assert!(!sanitized.contains("tok_123"));
        assert!(!sanitized.contains("p@ss"));
        assert!(!sanitized.contains("AKIA_TEST"));
        assert!(!sanitized.contains("SK_TEST"));
    }

    #[test]
    fn redacts_key_name_variants_and_xml_tags() {
        let message =
            r#"clientSecret: oidc-secret {"access_key":"AKIA_JSON"} <SecretKey>SK_XML</SecretKey>"#;

        let sanitized = redact_sensitive_pairs(message);

        assert!(sanitized.contains("clientSecret: <redacted>"));
        assert!(sanitized.contains(r#""access_key":"<redacted>""#));
        assert!(sanitized.contains("<SecretKey><redacted></SecretKey>"));
        assert!(!sanitized.contains("oidc-secret"));
        assert!(!sanitized.contains("AKIA_JSON"));
        assert!(!sanitized.contains("SK_XML"));
    }

    #[test]
    fn handles_unicode_without_panicking() {
        let message = "错误🔐 token: tok_123 用户=测试 secretkey: SK_TEST 完成";

        let sanitized = redact_sensitive_pairs(message);

        assert!(sanitized.contains("错误🔐"));
        assert!(sanitized.contains("用户=测试"));
        assert!(sanitized.contains("完成"));
        assert!(sanitized.contains("token: <redacted>"));
        assert!(sanitized.contains("secretkey: <redacted>"));
        assert!(!sanitized.contains("tok_123"));
        assert!(!sanitized.contains("SK_TEST"));
    }

    #[test]
    fn redacts_unicode_quoted_values() {
        let message = "{\"说明\":\"🔐\",\"secretkey\":\"秘密值\"}";

        let sanitized = redact_sensitive_pairs(message);

        assert!(sanitized.contains("\"说明\":\"🔐\""));
        assert!(sanitized.contains("\"secretkey\":\"<redacted>\""));
        assert!(!sanitized.contains("秘密值"));
    }

    #[test]
    fn redacts_after_unicode_whitespace() {
        let message = "token:\u{3000}tok_123 secretkey:\u{2003}SK_TEST";

        let sanitized = redact_sensitive_pairs(message);

        assert!(sanitized.contains("token:\u{3000}<redacted>"));
        assert!(sanitized.contains("secretkey:\u{2003}<redacted>"));
        assert!(!sanitized.contains("tok_123"));
        assert!(!sanitized.contains("SK_TEST"));
    }
}
