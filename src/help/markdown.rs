//! A small Markdown parser for the help pages.
//!
//! The help content is authored in this repo (`help/*.md`), so the parser only
//! needs the subset the content actually uses: ATX headings, paragraphs,
//! ordered/unordered lists, GFM tables, fenced code blocks, block quotes with
//! GitHub-style `[!TYPE]` callouts, standalone images, thematic breaks, and
//! inline bold/italic/code/link spans. Keeping it in-tree avoids a dependency
//! and keeps the supported syntax predictable.

/// Table column alignment from the GFM separator row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// One styled run of inline text. Flags compose, e.g. bold code.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

impl Span {
    fn plain(text: impl Into<String>) -> Self {
        Span {
            text: text.into(),
            bold: false,
            italic: false,
            code: false,
            link: None,
        }
    }
}

/// A single list entry. `level` is 0-based nesting.
#[derive(Debug, Clone)]
pub struct ListItem {
    pub level: usize,
    pub ordered: bool,
    pub index: usize,
    pub spans: Vec<Span>,
}

/// Callout flavour parsed from a leading `[!TYPE]` inside a block quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callout {
    Note,
    Tip,
    Important,
    Warning,
}

/// A rendered block of the document.
#[derive(Debug, Clone)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Paragraph(Vec<Span>),
    List(Vec<ListItem>),
    Table {
        aligns: Vec<Align>,
        /// Row 0 is the header row.
        rows: Vec<Vec<Vec<Span>>>,
    },
    Code {
        lang: String,
        text: String,
    },
    Quote {
        callout: Option<Callout>,
        blocks: Vec<Block>,
    },
    Image {
        /// Path relative to the help content root, e.g. `images/xx.png`.
        path: String,
        alt: String,
    },
    Hr,
}

pub fn parse(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source
        .lines()
        .map(|line| line.trim_end_matches(['\r', '\n']))
        .collect();
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();

    let flush_paragraph = |paragraph: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if paragraph.is_empty() {
            return;
        }
        let joined = paragraph.join("\n");
        paragraph.clear();
        let spans = parse_inline(&joined);
        // A paragraph that is exactly one image reference renders as a figure.
        if let [span] = spans.as_slice()
            && span.text.is_empty()
            && let Some(encoded) = &span.link
            && let Some((path, alt)) = encoded.split_once('\u{1}')
        {
            blocks.push(Block::Image {
                path: path.to_owned(),
                alt: alt.to_owned(),
            });
            return;
        }
        blocks.push(Block::Paragraph(spans));
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // Blank line ends the current paragraph.
        if trimmed.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            i += 1;
            continue;
        }

        // Fenced code block.
        if let Some(fence) = trimmed.strip_prefix("```") {
            flush_paragraph(&mut paragraph, &mut blocks);
            let lang = fence.trim().to_owned();
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                body.push(lines[i]);
                i += 1;
            }
            // Skip the closing fence (or run to EOF on an unterminated block).
            i += 1;
            blocks.push(Block::Code {
                lang,
                text: body.join("\n"),
            });
            continue;
        }

        // ATX heading.
        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|&c| c == '#').count();
            if let Some(text) = trimmed[hashes..].strip_prefix(' ') {
                flush_paragraph(&mut paragraph, &mut blocks);
                blocks.push(Block::Heading {
                    level: hashes.min(6) as u8,
                    spans: parse_inline(text.trim()),
                });
                i += 1;
                continue;
            }
        }

        // Thematic break.
        if (trimmed.starts_with("---") || trimmed.starts_with("***"))
            && trimmed.chars().all(|c| c == '-' || c == '*' || c.is_whitespace())
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Hr);
            i += 1;
            continue;
        }

        // Block quote (with optional `[!TYPE]` callout marker).
        if trimmed.starts_with('>') {
            flush_paragraph(&mut paragraph, &mut blocks);
            let mut inner = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if !t.starts_with('>') {
                    break;
                }
                let content = t.strip_prefix('>').unwrap_or(t);
                inner.push(content.strip_prefix(' ').unwrap_or(content).to_owned());
                i += 1;
            }
            let callout = inner
                .first()
                .and_then(|first| parse_callout_marker(first))
                .map(|(kind, _)| kind);
            if callout.is_some() {
                inner.remove(0);
            }
            blocks.push(Block::Quote {
                callout,
                blocks: parse(&inner.join("\n")),
            });
            continue;
        }

        // Table: a header row followed by a GFM separator row.
        if trimmed.starts_with('|')
            && i + 1 < lines.len()
            && is_separator_row(lines[i + 1].trim())
        {
            flush_paragraph(&mut paragraph, &mut blocks);
            let aligns = parse_separator_row(lines[i + 1].trim());
            let mut rows = vec![split_table_row(trimmed)];
            i += 2;
            while i < lines.len() {
                let t = lines[i].trim();
                if !t.starts_with('|') {
                    break;
                }
                rows.push(split_table_row(t));
                i += 1;
            }
            // Pad or trim every row to the header width.
            let width = aligns.len();
            for row in &mut rows {
                row.resize(width, Vec::new());
            }
            blocks.push(Block::Table { aligns, rows });
            continue;
        }

        // List item.
        if list_marker(line).is_some() {
            flush_paragraph(&mut paragraph, &mut blocks);
            let mut items = Vec::new();
            let mut ordered_index = 0usize;
            while i < lines.len() {
                let raw = lines[i];
                if let Some((lead, (ordered, marker_len))) = list_marker(raw) {
                    let level = (lead / 2).min(3);
                    if ordered {
                        ordered_index += 1;
                    } else {
                        ordered_index = 0;
                    }
                    let text = &raw[lead + marker_len..];
                    items.push(ListItem {
                        level,
                        ordered,
                        index: ordered_index,
                        spans: parse_inline(text.trim_start()),
                    });
                    i += 1;
                } else if !items.is_empty() && is_continuation(raw) {
                    // An unindented plain-text line extends the last item.
                    if let Some(last) = items.last_mut() {
                        let mut extra = parse_inline(raw.trim());
                        if let Some(first) = extra.first_mut() {
                            first.text = format!(" {}", first.text);
                        }
                        last.spans.extend(extra);
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(Block::List(items));
            continue;
        }

        // Regular paragraph line. Blank-line-free consecutive lines join.
        paragraph.push(line);
        i += 1;
    }

    flush_paragraph(&mut paragraph, &mut blocks);
    blocks
}

/// `(indent-width, (ordered?, marker-length))` when the line starts a list item.
fn list_marker(line: &str) -> Option<(usize, (bool, usize))> {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    let bytes = t.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if (bytes[0] == b'-' || bytes[0] == b'+' || bytes[0] == b'*') && bytes.get(1) == Some(&b' ') {
        return Some((indent, (false, 2)));
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let after = &t[digits..];
        let (sep, sep_len) = if let Some(rest) = after.strip_prefix(". ") {
            (rest, 2)
        } else if let Some(rest) = after.strip_prefix("、") {
            (rest, "、".len())
        } else if let Some(rest) = after.strip_prefix(") ") {
            (rest, 2)
        } else if let Some(rest) = after.strip_prefix(".") {
            (rest, 1)
        } else {
            return None;
        };
        // `sep` points at the item text; trailing content must be non-empty.
        if sep.is_empty() {
            return None;
        }
        return Some((indent, (true, digits + sep_len)));
    }
    None
}

/// A continuation line belongs to the previous item when it is plain text.
fn is_continuation(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && !t.starts_with('#')
        && !t.starts_with('>')
        && !t.starts_with('|')
        && !t.starts_with("```")
        && list_marker(t).is_none()
}

fn is_separator_row(line: &str) -> bool {
    // Cells are still joined by `|` here, so the delimiter is allowed alongside
    // the alignment markers (`-`, `:`) and padding.
    let inner = line.trim_start_matches('|').trim_end_matches('|');
    !inner.is_empty()
        && inner
            .chars()
            .all(|c| matches!(c, '-' | ':' | '|' | ' ' | '\t'))
        && inner.contains('-')
}

fn parse_separator_row(line: &str) -> Vec<Align> {
    split_table_row_raw(line)
        .iter()
        .map(|cell| {
            let leading = cell.starts_with(':');
            let trailing = cell.ends_with(':');
            match (leading, trailing) {
                (true, true) => Align::Center,
                (true, false) => Align::Left,
                (false, true) => Align::Right,
                (false, false) => Align::None,
            }
        })
        .collect()
}

fn split_table_row(line: &str) -> Vec<Vec<Span>> {
    split_table_row_raw(line)
        .iter()
        .map(|cell| parse_inline(cell.trim()))
        .collect()
}

fn split_table_row_raw(line: &str) -> Vec<String> {
    let inner = line.trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|c| c.trim().to_owned()).collect()
}

fn parse_callout_marker(line: &str) -> Option<(Callout, &str)> {
    let rest = line.trim().strip_prefix("[!")?;
    let end = rest.find(']')?;
    let kind = match rest[..end].to_ascii_lowercase().as_str() {
        "note" => Callout::Note,
        "tip" => Callout::Tip,
        "important" => Callout::Important,
        "warning" | "caution" => Callout::Warning,
        _ => return None,
    };
    Some((kind, rest[end + 1..].trim()))
}

/// Inline scanner: `**bold**`, `*italic*`, `` `code` ``, `[text](url)`,
/// `![alt](src)`. Images surface as a span with empty text and `src \u{1} alt`
/// in `link` so a lone-image paragraph can be hoisted into `Block::Image`.
fn parse_inline(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut buffer = String::new();

    let flush = |buffer: &mut String, spans: &mut Vec<Span>| {
        if !buffer.is_empty() {
            spans.push(Span::plain(std::mem::take(buffer)));
        }
    };

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Image: ![alt](src)
        if c == '!' && chars.get(i + 1) == Some(&'[') {
            if let Some((alt, src, next)) = parse_link_target(&chars, i + 1) {
                flush(&mut buffer, &mut spans);
                spans.push(Span {
                    text: String::new(),
                    bold: false,
                    italic: false,
                    code: false,
                    link: Some(format!("{src}\u{1}{alt}")),
                });
                i = next;
                continue;
            }
        }
        // Link: [text](url)
        if c == '[' {
            if let Some((label, url, next)) = parse_link_target(&chars, i) {
                flush(&mut buffer, &mut spans);
                spans.push(Span {
                    text: label,
                    bold: false,
                    italic: false,
                    code: false,
                    link: Some(url),
                });
                i = next;
                continue;
            }
        }
        // Inline code: `...`
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                flush(&mut buffer, &mut spans);
                spans.push(Span {
                    text: chars[i + 1..i + 1 + end].iter().collect(),
                    bold: false,
                    italic: false,
                    code: true,
                    link: None,
                });
                i += end + 2;
                continue;
            }
        }
        // Bold: **...**
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = find_double(&chars, i + 2, '*') {
                flush(&mut buffer, &mut spans);
                let inner: String = chars[i + 2..end].iter().collect();
                let mut nested = parse_inline(&inner);
                for span in &mut nested {
                    span.bold = true;
                }
                spans.extend(nested);
                i = end + 2;
                continue;
            }
        }
        // Italic: *...*
        if c == '*' && chars.get(i + 1) != Some(&'*') {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '*') {
                flush(&mut buffer, &mut spans);
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                let mut nested = parse_inline(&inner);
                for span in &mut nested {
                    span.italic = true;
                }
                spans.extend(nested);
                i += end + 2;
                continue;
            }
        }
        buffer.push(c);
        i += 1;
    }
    flush(&mut buffer, &mut spans);
    spans
}

/// Parses `[label](target)` starting at `open` (which must point at `[`).
/// Returns `(label, target, index-after-close)`.
fn parse_link_target(chars: &[char], open: usize) -> Option<(String, String, usize)> {
    let mut depth = 0usize;
    let mut close = None;
    for (offset, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().position(|&c| c == ')')? + close + 2;
    let label: String = chars[open + 1..close].iter().collect();
    let target: String = chars[close + 2..end].iter().collect();
    Some((label, target, end + 1))
}

fn find_double(chars: &[char], from: usize, needle: char) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == needle && chars[i + 1] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_paragraphs_and_callouts() {
        let blocks = parse("# 标题\n\n正文**加粗**与`代码`。\n\n> [!TIP]\n> 内容\n");
        assert_eq!(blocks.len(), 3);
        assert!(matches!(&blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(&blocks[1], Block::Paragraph(_)));
        match &blocks[2] {
            Block::Quote {
                callout: Some(Callout::Tip),
                blocks,
            } => {
                assert_eq!(blocks.len(), 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_tables() {
        let blocks = parse("| A | B |\n|---|:-:|\n| 1 | 2 |\n");
        match &blocks[0] {
            Block::Table { aligns, rows } => {
                assert_eq!(aligns, &vec![Align::None, Align::Center]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn hoists_standalone_images() {
        let blocks = parse("![总览页](images/desktop-dashboard.png)\n");
        match &blocks[0] {
            Block::Image { path, alt } => {
                assert_eq!(path, "images/desktop-dashboard.png");
                assert_eq!(alt, "总览页");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_nested_lists() {
        let blocks = parse("- a\n  - b\n- c\n");
        match &blocks[0] {
            Block::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[1].level, 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_inline_styles() {
        let spans = parse_inline("a **b** *c* `d` [e](https://x)");
        // Whitespace between runs stays as its own plain span.
        assert_eq!(spans.len(), 8);
        assert!(spans[1].bold);
        assert!(spans[3].italic);
        assert!(spans[5].code);
        assert_eq!(spans[7].link.as_deref(), Some("https://x"));
    }
}
