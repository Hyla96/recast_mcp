//! Shared `{param_name}` placeholder parsing utilities.
//!
//! Both the upstream request builder ([`crate::upstream`]) and the MCP tool
//! schema generator use `{param_name}` syntax to declare path parameters in
//! URL templates. This module provides the shared extraction logic so that
//! parameter metadata is derived consistently in both places.
//!
//! # Placeholder syntax
//!
//! Placeholders are `{identifier}` where `identifier` is one or more
//! non-brace characters. Nested braces are not supported. Duplicate
//! placeholder names in a single template are returned once each.
//!
//! ```ignore
//! let names = extract_placeholders("/users/{user_id}/posts/{post_id}");
//! assert_eq!(names, vec!["user_id", "post_id"]);
//! ```

// ── Public API ────────────────────────────────────────────────────────────────

/// Extract all `{param_name}` placeholder names from a URL template.
///
/// Returns names in the order they first appear. Duplicate names are only
/// returned once. Does **not** allocate if `template` contains no placeholders.
#[must_use]
pub fn extract_placeholders(template: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_placeholder = false;
    let mut current = String::new();

    for ch in template.chars() {
        match ch {
            '{' => {
                in_placeholder = true;
                current.clear();
            }
            '}' if in_placeholder => {
                if !current.is_empty() {
                    let name = current.clone();
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                in_placeholder = false;
                current.clear();
            }
            c if in_placeholder => {
                current.push(c);
            }
            _ => {}
        }
    }

    names
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo
)]
mod tests {
    use super::*;

    #[test]
    fn extracts_single_placeholder() {
        assert_eq!(extract_placeholders("/users/{id}"), vec!["id"]);
    }

    #[test]
    fn extracts_multiple_placeholders() {
        assert_eq!(
            extract_placeholders("/users/{user_id}/posts/{post_id}"),
            vec!["user_id", "post_id"]
        );
    }

    #[test]
    fn returns_empty_for_no_placeholders() {
        let result: Vec<String> = extract_placeholders("/users/list");
        assert!(result.is_empty());
    }

    #[test]
    fn deduplicates_repeated_placeholders() {
        assert_eq!(extract_placeholders("/{x}/and/{x}"), vec!["x"]);
    }

    #[test]
    fn handles_placeholder_in_base_url() {
        assert_eq!(
            extract_placeholders("https://api.{region}.example.com/data"),
            vec!["region"]
        );
    }

    #[test]
    fn empty_template_returns_empty() {
        let result: Vec<String> = extract_placeholders("");
        assert!(result.is_empty());
    }

    #[test]
    fn unclosed_brace_is_ignored() {
        // An unclosed `{` at end of template is not a valid placeholder.
        let result: Vec<String> = extract_placeholders("/path/{unclosed");
        assert!(result.is_empty());
    }
}
