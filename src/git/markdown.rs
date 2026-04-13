use gpui::*;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, CodeBlockKind};
use std::ops::Range;

use crate::editor::syntax;
use crate::theme::colors;

/// Render a markdown string into a list of GPUI elements.
///
/// Handles: paragraphs, bold/italic/code spans, code blocks (with syntax
/// highlighting), blockquotes, links, lists, and horizontal rules.
pub fn render_markdown(source: &str) -> Vec<AnyElement> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(source, opts);

    let mut ctx = RenderCtx::new();

    for event in parser {
        match event {
            Event::Start(tag) => ctx.open_tag(tag),
            Event::End(tag) => ctx.close_tag(tag),
            Event::Text(text) => ctx.push_text(&text),
            Event::Code(code) => ctx.push_inline_code(&code),
            Event::SoftBreak => ctx.push_text(" "),
            Event::HardBreak => ctx.flush_line(),
            Event::Rule => ctx.push_rule(),
            _ => {}
        }
    }

    ctx.finish()
}

// ── Inline span tracking ──

#[derive(Clone, Copy, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strikethrough: bool,
    link: bool,
}

impl InlineStyle {
    fn to_highlight(&self) -> HighlightStyle {
        let color = if self.link {
            Some(Hsla::from(colors::accent()))
        } else {
            None
        };

        let underline = if self.link {
            Some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(Hsla::from(colors::accent())),
                wavy: false,
            })
        } else {
            None
        };

        let strikethrough = if self.strikethrough {
            Some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            })
        } else {
            None
        };

        HighlightStyle {
            color,
            font_weight: if self.bold {
                Some(FontWeight::BOLD)
            } else {
                None
            },
            font_style: if self.italic {
                Some(FontStyle::Italic)
            } else {
                None
            },
            underline,
            strikethrough,
            background_color: None,
            fade_out: None,
        }
    }

    fn is_default(&self) -> bool {
        !self.bold && !self.italic && !self.strikethrough && !self.link
    }
}

// ── Block context stack ──

enum BlockKind {
    Paragraph,
    Heading(HeadingLevel),
    BlockQuote,
    CodeBlock(Option<String>), // language tag
    List(Option<u64>),         // None = unordered, Some(start) = ordered
    ListItem,
}

// ── Render context ──

struct RenderCtx {
    elements: Vec<AnyElement>,
    /// Current line being built (plain text accumulated here)
    line_buf: String,
    /// Style spans for the current line_buf
    line_spans: Vec<(Range<usize>, HighlightStyle)>,
    /// Active inline style
    style: InlineStyle,
    /// Block context stack
    block_stack: Vec<BlockKind>,
    /// For code blocks, accumulate all text before highlighting
    code_buf: String,
    /// List item counter for ordered lists
    list_counter: Vec<u64>,
}

impl RenderCtx {
    fn new() -> Self {
        Self {
            elements: Vec::new(),
            line_buf: String::new(),
            line_spans: Vec::new(),
            style: InlineStyle::default(),
            block_stack: Vec::new(),
            code_buf: String::new(),
            list_counter: Vec::new(),
        }
    }

    fn in_code_block(&self) -> bool {
        self.block_stack
            .iter()
            .any(|b| matches!(b, BlockKind::CodeBlock(_)))
    }

    fn in_block_quote(&self) -> bool {
        self.block_stack
            .iter()
            .any(|b| matches!(b, BlockKind::BlockQuote))
    }

    fn list_depth(&self) -> usize {
        self.block_stack
            .iter()
            .filter(|b| matches!(b, BlockKind::List(_)))
            .count()
    }

    fn open_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.block_stack.push(BlockKind::Paragraph);
            }
            Tag::Heading { level, .. } => {
                self.block_stack.push(BlockKind::Heading(level));
            }
            Tag::BlockQuote(_) => {
                self.flush_paragraph();
                self.block_stack.push(BlockKind::BlockQuote);
            }
            Tag::CodeBlock(kind) => {
                self.flush_paragraph();
                let lang = match &kind {
                    CodeBlockKind::Fenced(lang) => {
                        let l = lang.split_whitespace().next().unwrap_or("");
                        if l.is_empty() {
                            None
                        } else {
                            Some(l.to_string())
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code_buf.clear();
                self.block_stack.push(BlockKind::CodeBlock(lang));
            }
            Tag::List(start) => {
                self.flush_paragraph();
                if let Some(n) = start {
                    self.list_counter.push(n);
                }
                self.block_stack.push(BlockKind::List(start));
            }
            Tag::Item => {
                self.block_stack.push(BlockKind::ListItem);
            }
            Tag::Emphasis => self.style.italic = true,
            Tag::Strong => self.style.bold = true,
            Tag::Strikethrough => self.style.strikethrough = true,
            Tag::Link { .. } => self.style.link = true,
            _ => {}
        }
    }

    fn close_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_paragraph();
                self.pop_block(|b| matches!(b, BlockKind::Paragraph));
            }
            TagEnd::Heading(_level) => {
                self.flush_heading();
                self.pop_block(|b| matches!(b, BlockKind::Heading(_)));
            }
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                self.pop_block(|b| matches!(b, BlockKind::BlockQuote));
            }
            TagEnd::CodeBlock => {
                self.flush_code_block();
                self.pop_block(|b| matches!(b, BlockKind::CodeBlock(_)));
            }
            TagEnd::List(ordered) => {
                if ordered {
                    self.list_counter.pop();
                }
                self.pop_block(|b| matches!(b, BlockKind::List(_)));
            }
            TagEnd::Item => {
                self.flush_list_item();
                self.pop_block(|b| matches!(b, BlockKind::ListItem));
            }
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strikethrough = false,
            TagEnd::Link => self.style.link = false,
            _ => {}
        }
    }

    fn pop_block(&mut self, pred: impl Fn(&BlockKind) -> bool) {
        if let Some(pos) = self.block_stack.iter().rposition(|b| pred(b)) {
            self.block_stack.remove(pos);
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.in_code_block() {
            self.code_buf.push_str(text);
            return;
        }

        let start = self.line_buf.len();
        self.line_buf.push_str(text);
        let end = self.line_buf.len();

        if !self.style.is_default() && start < end {
            self.line_spans
                .push((start..end, self.style.to_highlight()));
        }
    }

    fn push_inline_code(&mut self, code: &str) {
        let start = self.line_buf.len();
        self.line_buf.push_str(code);
        let end = self.line_buf.len();
        if start < end {
            self.line_spans.push((
                start..end,
                HighlightStyle {
                    color: Some(Hsla::from(colors::accent())),
                    background_color: Some(Hsla::from(rgba(0xffffff0a))),
                    ..Default::default()
                },
            ));
        }
    }

    fn push_rule(&mut self) {
        self.flush_paragraph();
        self.elements.push(
            div()
                .w_full()
                .h(px(1.0))
                .my_1()
                .bg(colors::border())
                .into_any_element(),
        );
    }

    fn flush_line(&mut self) {
        // Emit current line_buf as a styled line, then clear
        if self.line_buf.is_empty() {
            return;
        }

        let text = SharedString::from(std::mem::take(&mut self.line_buf));
        let spans = std::mem::take(&mut self.line_spans);

        let styled = StyledText::new(text).with_highlights(spans);
        self.elements.push(
            div()
                .text_xs()
                .text_color(colors::text_muted())
                .child(styled)
                .into_any_element(),
        );
    }

    fn flush_paragraph(&mut self) {
        if self.line_buf.is_empty() {
            return;
        }

        let text = SharedString::from(std::mem::take(&mut self.line_buf));
        let spans = std::mem::take(&mut self.line_spans);

        let styled = StyledText::new(text).with_highlights(spans);

        let mut el = div().text_xs().text_color(colors::text_muted());

        if self.in_block_quote() {
            el = el
                .pl_2()
                .border_l_2()
                .border_color(colors::border())
                .text_color(colors::text_muted());
        }

        el = el.child(styled);
        self.elements.push(el.into_any_element());
    }

    fn flush_heading(&mut self) {
        if self.line_buf.is_empty() {
            return;
        }

        let text = SharedString::from(std::mem::take(&mut self.line_buf));
        let spans = std::mem::take(&mut self.line_spans);
        let styled = StyledText::new(text).with_highlights(spans);

        let level = self.block_stack.iter().find_map(|b| match b {
            BlockKind::Heading(l) => Some(*l),
            _ => None,
        });

        let size = match level {
            Some(HeadingLevel::H1) => px(16.0),
            Some(HeadingLevel::H2) => px(14.0),
            _ => px(13.0),
        };

        self.elements.push(
            div()
                .text_size(size)
                .font_weight(FontWeight::BOLD)
                .text_color(colors::text())
                .mt_1()
                .child(styled)
                .into_any_element(),
        );
    }

    fn flush_code_block(&mut self) {
        let code = std::mem::take(&mut self.code_buf);
        let code = code.trim_end_matches('\n');
        if code.is_empty() {
            return;
        }

        // Detect language from block stack
        let lang = self
            .block_stack
            .iter()
            .find_map(|b| match b {
                BlockKind::CodeBlock(l) => l.clone(),
                _ => None,
            });

        // Map common fence language tags to the names highlight_source expects
        let highlight_lang = lang.as_deref().and_then(|l| match l {
            "rust" | "rs" => Some("rust"),
            "python" | "py" => Some("python"),
            "javascript" | "js" => Some("javascript"),
            "jsx" => Some("jsx"),
            "typescript" | "ts" => Some("typescript"),
            "tsx" => Some("tsx"),
            "json" => Some("json"),
            "markdown" | "md" => Some("markdown"),
            _ => None,
        });

        let highlights = highlight_lang
            .map(|l| syntax::highlight_source(code, l))
            .unwrap_or_default();

        let mut block = div()
            .w_full()
            .rounded(px(4.0))
            .bg(rgba(0xffffff08))
            .border_1()
            .border_color(rgba(0xffffff10))
            .px_2()
            .py_1()
            .flex()
            .flex_col();

        let mut byte_offset = 0usize;
        for line_text in code.split('\n') {
            let line_shared = SharedString::from(line_text.to_string());

            let line_start = byte_offset;
            let line_end = line_start + line_text.len();
            byte_offset = line_end + 1; // +1 for the '\n'

            let hl: Vec<(Range<usize>, HighlightStyle)> = highlights
                .iter()
                .filter_map(|span| {
                    let s = span.byte_range.start.max(line_start);
                    let e = span.byte_range.end.min(line_end);
                    if s < e {
                        Some((
                            (s - line_start)..(e - line_start),
                            HighlightStyle {
                                color: Some(Hsla::from(span.color)),
                                ..Default::default()
                            },
                        ))
                    } else {
                        None
                    }
                })
                .collect();

            let styled = StyledText::new(line_shared).with_highlights(hl);
            block = block.child(
                div()
                    .text_size(px(11.0))
                    .font_family("monospace")
                    .text_color(colors::text())
                    .child(styled),
            );
        }

        self.elements.push(block.into_any_element());
    }

    fn flush_list_item(&mut self) {
        if self.line_buf.is_empty() {
            return;
        }

        let text = SharedString::from(std::mem::take(&mut self.line_buf));
        let spans = std::mem::take(&mut self.line_spans);
        let styled = StyledText::new(text).with_highlights(spans);

        // Determine bullet/number
        let depth = self.list_depth();
        let indent = (depth.saturating_sub(1) as f32) * 12.0;

        // Check if inside an ordered list
        let is_ordered = self.block_stack.iter().rev().any(|b| {
            matches!(b, BlockKind::List(Some(_)))
        });

        let marker = if is_ordered {
            let counter = self.list_counter.last_mut();
            if let Some(c) = counter {
                let n = *c;
                *c += 1;
                format!("{n}.")
            } else {
                "\u{2022}".to_string()
            }
        } else {
            "\u{2022}".to_string()
        };

        self.elements.push(
            div()
                .flex()
                .flex_row()
                .gap_1()
                .pl(px(indent))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors::text_muted())
                        .flex_shrink_0()
                        .w(px(16.0))
                        .child(marker),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors::text_muted())
                        .flex_1()
                        .child(styled),
                )
                .into_any_element(),
        );
    }

    fn finish(mut self) -> Vec<AnyElement> {
        // Flush any remaining inline content
        self.flush_paragraph();
        self.elements
    }
}

use pulldown_cmark::HeadingLevel;
