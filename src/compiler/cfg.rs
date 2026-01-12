//! Cfg attribute evaluation for conditional compilation.
//!
//! Evaluates `#[cfg(...)]` attributes to determine whether items should be
//! included in the compiled output based on compile options (test mode, features).

use crate::compiler::ast::{Attribute, AttributeArg, AttributeArgs};
use crate::config::CompileOptions;

/// Check if an item with the given attributes should be included in compilation.
/// Returns `true` if the item should be included, `false` if it should be excluded.
///
/// Items are included if:
/// - They have no cfg attributes, OR
/// - All cfg attributes evaluate to true
pub fn should_include(attrs: &[Attribute], options: &CompileOptions) -> bool {
    for attr in attrs {
        if attr.name == "cfg" {
            if !evaluate_cfg_attr(attr, options) {
                return false;
            }
        }
    }
    true
}

/// Check if an item has the `#[test]` attribute.
pub fn is_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.name == "test")
}

/// Check if an item has `#[cfg(test)]` attribute.
pub fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.name == "cfg" {
            if let AttributeArgs::Parenthesized(args) = &attr.args {
                if args.len() == 1 {
                    if let AttributeArg::Ident(ident) = &args[0] {
                        return ident == "test";
                    }
                }
            }
        }
        false
    })
}

/// Evaluate a single `#[cfg(...)]` attribute.
fn evaluate_cfg_attr(attr: &Attribute, options: &CompileOptions) -> bool {
    match &attr.args {
        AttributeArgs::Parenthesized(args) => {
            // Empty parens: #[cfg()] - always true (unusual but valid)
            if args.is_empty() {
                return true;
            }
            // Multiple top-level args are implicitly AND'd
            args.iter().all(|arg| evaluate_cfg_arg(arg, options))
        }
        // #[cfg] without args - not valid but we treat as true
        AttributeArgs::None => true,
        // #[cfg = "value"] - not standard cfg syntax, treat as true
        AttributeArgs::Eq(_) => true,
    }
}

/// Evaluate a single cfg argument.
fn evaluate_cfg_arg(arg: &AttributeArg, options: &CompileOptions) -> bool {
    match arg {
        AttributeArg::Ident(ident) => {
            // `test` - checks if we're in test mode
            if ident == "test" {
                return options.test_mode;
            }
            // Unknown identifier - treat as false
            false
        }
        AttributeArg::KeyValue(key, value) => {
            // `feature = "name"` - checks if feature is enabled
            if key == "feature" {
                return options.has_feature(value);
            }
            // Unknown key - treat as false
            false
        }
        AttributeArg::Nested(name, inner_args) => {
            match name.as_str() {
                "not" => {
                    // `not(...)` - negates the inner condition
                    // Should have exactly one argument
                    if inner_args.len() == 1 {
                        !evaluate_cfg_arg(&inner_args[0], options)
                    } else {
                        // Multiple args in not() - treat as false
                        false
                    }
                }
                "all" => {
                    // `all(...)` - all inner conditions must be true
                    inner_args.iter().all(|a| evaluate_cfg_arg(a, options))
                }
                "any" => {
                    // `any(...)` - at least one inner condition must be true
                    inner_args.iter().any(|a| evaluate_cfg_arg(a, options))
                }
                _ => {
                    // Unknown nested function - treat as false
                    false
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::lexer::Span;
    use std::collections::HashSet;

    fn make_attr(name: &str, args: AttributeArgs) -> Attribute {
        Attribute {
            name: name.to_string(),
            args,
            span: Span { start: 0, end: 0 },
        }
    }

    #[test]
    fn test_no_attrs_included() {
        let options = CompileOptions::new();
        assert!(should_include(&[], &options));
    }

    #[test]
    fn test_non_cfg_attrs_included() {
        let options = CompileOptions::new();
        let attrs = vec![make_attr("test", AttributeArgs::None)];
        assert!(should_include(&attrs, &options));
    }

    #[test]
    fn test_cfg_test_in_test_mode() {
        let options = CompileOptions::for_testing();
        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::Ident("test".to_string())]),
        )];
        assert!(should_include(&attrs, &options));
    }

    #[test]
    fn test_cfg_test_not_in_test_mode() {
        let options = CompileOptions::new();
        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::Ident("test".to_string())]),
        )];
        assert!(!should_include(&attrs, &options));
    }

    #[test]
    fn test_cfg_feature_enabled() {
        let mut features = HashSet::new();
        features.insert("json".to_string());
        let options = CompileOptions::with_features(features);

        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::KeyValue(
                "feature".to_string(),
                "json".to_string(),
            )]),
        )];
        assert!(should_include(&attrs, &options));
    }

    #[test]
    fn test_cfg_feature_disabled() {
        let options = CompileOptions::new();
        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::KeyValue(
                "feature".to_string(),
                "json".to_string(),
            )]),
        )];
        assert!(!should_include(&attrs, &options));
    }

    #[test]
    fn test_cfg_not() {
        let options = CompileOptions::new(); // not in test mode
        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::Nested(
                "not".to_string(),
                vec![AttributeArg::Ident("test".to_string())],
            )]),
        )];
        assert!(should_include(&attrs, &options));

        // In test mode, not(test) should be false
        let options = CompileOptions::for_testing();
        assert!(!should_include(&attrs, &options));
    }

    #[test]
    fn test_cfg_all() {
        let mut features = HashSet::new();
        features.insert("json".to_string());
        features.insert("async".to_string());
        let options = CompileOptions::with_features(features);

        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::Nested(
                "all".to_string(),
                vec![
                    AttributeArg::KeyValue("feature".to_string(), "json".to_string()),
                    AttributeArg::KeyValue("feature".to_string(), "async".to_string()),
                ],
            )]),
        )];
        assert!(should_include(&attrs, &options));

        // Missing one feature
        let mut features = HashSet::new();
        features.insert("json".to_string());
        let options = CompileOptions::with_features(features);
        assert!(!should_include(&attrs, &options));
    }

    #[test]
    fn test_cfg_any() {
        let mut features = HashSet::new();
        features.insert("json".to_string());
        let options = CompileOptions::with_features(features);

        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::Nested(
                "any".to_string(),
                vec![
                    AttributeArg::KeyValue("feature".to_string(), "json".to_string()),
                    AttributeArg::KeyValue("feature".to_string(), "yaml".to_string()),
                ],
            )]),
        )];
        assert!(should_include(&attrs, &options));

        // No matching features
        let options = CompileOptions::new();
        assert!(!should_include(&attrs, &options));
    }

    #[test]
    fn test_is_test_attr() {
        let attrs = vec![make_attr("test", AttributeArgs::None)];
        assert!(is_test(&attrs));

        let attrs = vec![make_attr("cfg", AttributeArgs::None)];
        assert!(!is_test(&attrs));

        let attrs: Vec<Attribute> = vec![];
        assert!(!is_test(&attrs));
    }

    #[test]
    fn test_is_cfg_test() {
        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::Ident("test".to_string())]),
        )];
        assert!(is_cfg_test(&attrs));

        let attrs = vec![make_attr(
            "cfg",
            AttributeArgs::Parenthesized(vec![AttributeArg::KeyValue(
                "feature".to_string(),
                "json".to_string(),
            )]),
        )];
        assert!(!is_cfg_test(&attrs));
    }

    #[test]
    fn test_multiple_cfg_attrs() {
        // Both conditions must be true
        let mut features = HashSet::new();
        features.insert("json".to_string());
        let options = CompileOptions::for_testing_with_features(features);

        let attrs = vec![
            make_attr(
                "cfg",
                AttributeArgs::Parenthesized(vec![AttributeArg::Ident("test".to_string())]),
            ),
            make_attr(
                "cfg",
                AttributeArgs::Parenthesized(vec![AttributeArg::KeyValue(
                    "feature".to_string(),
                    "json".to_string(),
                )]),
            ),
        ];
        assert!(should_include(&attrs, &options));

        // Missing feature
        let options = CompileOptions::for_testing();
        assert!(!should_include(&attrs, &options));
    }
}
