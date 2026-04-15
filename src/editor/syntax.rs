use gpui::Rgba;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::theme::colors;

const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "function",
    "function.builtin",
    "function.macro",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

fn highlight_color(index: usize) -> Rgba {
    match HIGHLIGHT_NAMES.get(index).copied().unwrap_or("") {
        "comment" => colors::syn_comment(),
        "keyword" => colors::syn_keyword(),
        "string" | "string.special" => colors::syn_string(),
        "number" | "constant" | "constant.builtin" => colors::syn_number(),
        "function" | "function.builtin" | "function.macro" => colors::syn_function(),
        "type" | "type.builtin" | "constructor" => colors::syn_type(),
        "variable" | "variable.parameter" => colors::syn_variable(),
        "variable.builtin" => colors::syn_variable_builtin(),
        "operator" => colors::syn_operator(),
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => colors::syn_punctuation(),
        "property" | "label" => colors::syn_property(),
        "attribute" | "tag" => colors::syn_attribute(),
        _ => colors::text(),
    }
}

#[derive(Clone)]
pub struct HighlightSpan {
    pub byte_range: Range<usize>,
    pub color: Rgba,
}

pub fn detect_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    match ext {
        "rs" => Some("rust"),
        "json" => Some("json"),
        "toml" => Some("rust"), // basic highlighting
        "py" | "pyi" => Some("python"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("jsx"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "md" | "mdx" | "markdown" => Some("markdown"),
        _ => None,
    }
}

fn make_config(
    language: tree_sitter::Language,
    highlights: &str,
) -> Option<HighlightConfiguration> {
    let mut config =
        HighlightConfiguration::new(language, "highlight", highlights, "", "").ok()?;
    config.configure(HIGHLIGHT_NAMES);
    Some(config)
}

// Cache compiled HighlightConfigurations per language. These are expensive to
// build (compiles tree-sitter highlight queries into internal state machines)
// but identical for every file of the same language.
thread_local! {
    static CONFIG_CACHE: RefCell<HashMap<&'static str, HighlightConfiguration>> =
        RefCell::new(HashMap::new());
}

fn with_cached_config<R>(lang: &'static str, f: impl FnOnce(&HighlightConfiguration) -> R) -> Option<R> {
    CONFIG_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(lang) {
            let config = build_config(lang)?;
            cache.insert(lang, config);
        }
        cache.get(lang).map(f)
    })
}

fn build_config(lang: &str) -> Option<HighlightConfiguration> {
    // TypeScript extends JavaScript — its highlight query only covers TS-specific
    // syntax, so we concatenate the JS base query with the TS additions.
    match lang {
        "rust" => make_config(
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
        ),
        "json" => make_config(
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
        ),
        "python" => make_config(
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
        ),
        "javascript" => make_config(
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY,
        ),
        "jsx" => make_config(
            tree_sitter_javascript::LANGUAGE.into(),
            &format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ),
        ),
        "typescript" => {
            let combined = format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            );
            make_config(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), &combined)
        }
        "tsx" => {
            let combined = format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            );
            make_config(tree_sitter_typescript::LANGUAGE_TSX.into(), &combined)
        }
        "markdown" => make_config(
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        ),
        _ => None,
    }
}

pub fn highlight_source(source: &str, lang: &str) -> Vec<HighlightSpan> {
    // SAFETY: lang values come from detect_language() which always returns
    // &'static str literals, so this transmute just restores the original lifetime.
    let lang_static: &'static str = unsafe { std::mem::transmute::<&str, &'static str>(lang) };

    with_cached_config(lang_static, |config| {
        let mut highlighter = Highlighter::new();
        let highlights = match highlighter.highlight(config, source.as_bytes(), None, |_| None) {
            Ok(h) => h,
            Err(_) => return vec![],
        };

        let mut spans = Vec::new();
        let mut current_color: Option<Rgba> = None;

        for event in highlights {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    // Only emit spans with actual syntax highlighting colors,
                    // skip default-colored (unhighlighted) text regions
                    if let Some(color) = current_color {
                        spans.push(HighlightSpan {
                            byte_range: start..end,
                            color,
                        });
                    }
                }
                Ok(HighlightEvent::HighlightStart(h)) => {
                    current_color = Some(highlight_color(h.0));
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    current_color = None;
                }
                Err(_) => break,
            }
        }

        spans
    })
    .unwrap_or_default()
}
