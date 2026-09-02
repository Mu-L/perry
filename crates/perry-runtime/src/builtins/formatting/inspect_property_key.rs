//! Node-style bare-vs-quoted property key rendering for `util.inspect`.

use super::escape_string;

pub(super) fn format_inspect_property_key(key: &str) -> String {
    let mut chars = key.chars();
    let is_bare = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if is_bare {
        return key.to_string();
    }

    // Node prefers the delimiter that avoids an escape when exactly one kind
    // of quote occurs in the key.
    if key.contains('\'') && !key.contains('"') {
        let escaped = key
            .chars()
            .flat_map(|c| match c {
                '\\' => "\\\\".chars().collect::<Vec<_>>(),
                '"' => "\\\"".chars().collect(),
                '\n' => "\\n".chars().collect(),
                '\r' => "\\r".chars().collect(),
                '\t' => "\\t".chars().collect(),
                _ => vec![c],
            })
            .collect::<String>();
        format!("\"{}\"", escaped)
    } else {
        format!("'{}'", escape_string(key))
    }
}
