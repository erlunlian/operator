use gpui::{rgb, Rgba};
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
        "comment" => rgb(0x6c7086),
        "keyword" => rgb(0xcba6f7),
        "string" | "string.special" => rgb(0xa6e3a1),
        "number" | "constant" | "constant.builtin" => rgb(0xfab387),
        "function" | "function.builtin" | "function.macro" => rgb(0x89b4fa),
        "type" | "type.builtin" | "constructor" => rgb(0xf9e2af),
        "variable" | "variable.parameter" => rgb(0xcdd6f4),
        "variable.builtin" => rgb(0xf38ba8),
        "operator" => rgb(0x89dceb),
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => rgb(0x9399b2),
        "property" | "label" => rgb(0x89b4fa),
        "attribute" | "tag" => rgb(0xf5c2e7),
        _ => colors::text(),
    }
}

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

pub fn highlight_source(source: &str, lang: &str) -> Vec<HighlightSpan> {
    let config = match lang {
        "rust" => make_config(
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
        ),
        "json" => make_config(
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
        ),
        _ => return vec![],
    };

    let Some(config) = config else {
        return vec![];
    };

    let mut highlighter = Highlighter::new();
    let highlights = match highlighter.highlight(&config, source.as_bytes(), None, |_| None) {
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
}
