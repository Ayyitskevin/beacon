//! `beacon.outcome.md.v1` → Typst, for the PDF derivative (ADR 0040, R30).
//!
//! # Why this is not a Markdown parser
//!
//! ADR 0040 says `pdf_bytes = render_pdf(exact_markdown_bytes, pinned_profile)`: the PDF is a
//! function of the exported bytes, not of the manifest. So something has to read those bytes. What
//! it must NOT become is a second, laxer CommonMark implementation — the renderer owns the
//! document shape, and a reader that accepts more than the renderer emits is a second opinion
//! about what a document says.
//!
//! So this accepts exactly the shape `beacon.outcome.md.v1` emits — seven block constructs and one
//! inline one — and refuses everything else **by name**. Nothing is passed through, guessed at, or
//! silently dropped.
//!
//! # Why the escaping surface is two characters
//!
//! Text is never translated into Typst *markup*. Every run of human text leaves as a Typst STRING
//! LITERAL, where the only characters with meaning are `\` and `"`. Translating into markup would
//! mean a second escaping grammar that has to stay in step with [`crate::outcome`]'s `escape_v1`
//! forever, and the two would drift. A string literal has no such grammar.
//!
//! # How the shape is checked
//!
//! Not by listing the Markdown features that are out of shape — that list is never finished. `v1`
//! backslash-escapes every character that can begin a construct, so the check is the inverse of its
//! own escaping rule: outside a code span, an UNESCAPED construct character is text this renderer
//! did not produce. Bold, links, blockquotes, thematic breaks and raw HTML all fail that one rule
//! rather than four special cases.

use std::fmt;

/// The renderer version this translator reads. There is no default and no fallback: a document
/// whose renderer version this does not know is refused before a byte of it is interpreted,
/// because the escaping rule is exactly what the version names.
pub const READS_RENDERER_VERSION: &str = "beacon.outcome.md.v1";

/// Characters `escape_v1` escapes wherever they appear. Unescaped in a text run, each one is a
/// construct this document shape does not contain.
const CONSTRUCT_ANYWHERE: &[char] = &['\\', '`', '*', '_', '[', ']', '<', '>', '&', '|', '~', '!'];
/// Characters `escape_v1` escapes only in position zero.
const CONSTRUCT_AT_START: &[char] = &['#', '+', '-', '=', '.', ')'];

/// Why a document could not be translated. Every arm names the thing that was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypstRefusal {
    /// The document names a renderer version this translator does not read.
    UnknownRendererVersion(String),
    /// Front matter opened and never closed.
    UnclosedFrontMatter,
    /// A front-matter line is not `key: value`.
    FrontMatterNotKeyValue(String),
    /// A code fence opened and never closed.
    UnclosedCodeFence,
    /// A heading deeper than `###`. The document shape has three levels.
    HeadingTooDeep(String),
    /// An unescaped construct character in text. `escape_v1` escapes all of these, so an unescaped
    /// one is a construct outside the shape: bold, a link, a blockquote, a rule, raw HTML.
    UnescapedConstruct { character: char, line: String },
    /// A table row whose cell count differs from the row that opened the table.
    TableRowWidth { expected: usize, observed: usize },
}

impl fmt::Display for TypstRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRendererVersion(version) => write!(
                formatter,
                "this translator reads {READS_RENDERER_VERSION}, and the document says {version}"
            ),
            Self::UnclosedFrontMatter => {
                formatter.write_str("the front matter opened and never closed")
            }
            Self::FrontMatterNotKeyValue(line) => {
                write!(formatter, "a front-matter line is not `key: value`: {line}")
            }
            Self::UnclosedCodeFence => formatter.write_str("a code fence opened and never closed"),
            Self::HeadingTooDeep(line) => write!(
                formatter,
                "the document shape has three heading levels, and this is deeper: {line}"
            ),
            Self::UnescapedConstruct { character, line } => write!(
                formatter,
                "an unescaped `{character}` is a construct this document shape does not \
                 contain, on: {line}"
            ),
            Self::TableRowWidth { expected, observed } => write!(
                formatter,
                "a table row has {observed} cells where the table opened with {expected}"
            ),
        }
    }
}

/// Escapes for a Typst string literal. This is the whole escaping surface.
fn typst_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        if character == '\\' || character == '"' {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// Reverses `escape_v1` and refuses anything it would not have produced.
///
/// `at_line_start` is whether this run begins a line: [`CONSTRUCT_AT_START`] is only a construct
/// there, which is the same context rule `escape_v1` applies when it writes them.
fn unescape_checked(run: &str, at_line_start: bool, line: &str) -> Result<String, TypstRefusal> {
    let mut out = String::with_capacity(run.len());
    let mut characters = run.chars().peekable();
    let mut first = true;
    while let Some(character) = characters.next() {
        if character == '\\' {
            // A backslash escapes whatever follows it. A trailing backslash escapes nothing, which
            // `escape_v1` never emits, so it is itself out of shape.
            match characters.next() {
                Some(escaped) => out.push(escaped),
                None => {
                    return Err(TypstRefusal::UnescapedConstruct {
                        character: '\\',
                        line: line.to_owned(),
                    });
                }
            }
        } else if CONSTRUCT_ANYWHERE.contains(&character)
            || (first && at_line_start && CONSTRUCT_AT_START.contains(&character))
        {
            return Err(TypstRefusal::UnescapedConstruct {
                character,
                line: line.to_owned(),
            });
        } else {
            out.push(character);
        }
        first = false;
    }
    Ok(out)
}

/// Splits a line into alternating text and inline-code runs on unescaped backticks.
fn inline(text: &str, at_line_start: bool, line: &str) -> Result<String, TypstRefusal> {
    let mut out = String::new();
    let mut buffer = String::new();
    let mut in_code = false;
    let mut characters = text.chars().peekable();
    let mut starts_line = at_line_start;
    while let Some(character) = characters.next() {
        match character {
            '\\' if !in_code => {
                buffer.push(character);
                if let Some(next) = characters.next() {
                    buffer.push(next);
                }
            }
            '`' => {
                if in_code {
                    out.push_str(&format!("#raw({})", typst_string(&buffer)));
                } else if !buffer.is_empty() {
                    let text = unescape_checked(&buffer, starts_line, line)?;
                    out.push_str(&format!("#{}", typst_string(&text)));
                }
                starts_line = false;
                buffer.clear();
                in_code = !in_code;
            }
            _ => buffer.push(character),
        }
    }
    if in_code {
        // An unclosed span is an unescaped backtick by another name.
        return Err(TypstRefusal::UnescapedConstruct {
            character: '`',
            line: line.to_owned(),
        });
    }
    if !buffer.is_empty() {
        let text = unescape_checked(&buffer, starts_line, line)?;
        out.push_str(&format!("#{}", typst_string(&text)));
    }
    Ok(out)
}

/// Splits a table row on unescaped `|`. An escaped one is a pipe in a cell's text.
fn split_cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut buffer = String::new();
    let mut characters = row.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                buffer.push(character);
                if let Some(next) = characters.next() {
                    buffer.push(next);
                }
            }
            '|' => {
                cells.push(buffer.clone());
                buffer.clear();
            }
            _ => buffer.push(character),
        }
    }
    cells.push(buffer);
    // A row is `|a|b|`, so the split yields an empty run at each end.
    if cells.first().is_some_and(|cell| cell.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|cell| cell.trim().is_empty()) {
        cells.pop();
    }
    cells
}

/// Whether a table row is the header delimiter, which carries no content.
fn is_delimiter_row(row: &str) -> bool {
    let body = row.trim().trim_matches('|');
    !body.is_empty()
        && body
            .chars()
            .all(|character| matches!(character, '-' | ':' | '|' | ' '))
}

/// Translate one `beacon.outcome.md.v1` document into a Typst body.
///
/// The body carries no page setup: the profile owns that, and a document that could set its own
/// geometry would be a second place the rendered bytes are decided.
pub fn to_typst(renderer_version: &str, markdown: &str) -> Result<String, TypstRefusal> {
    if renderer_version != READS_RENDERER_VERSION {
        return Err(TypstRefusal::UnknownRendererVersion(
            renderer_version.to_owned(),
        ));
    }

    let mut out = String::new();
    let mut lines = markdown.lines().peekable();

    // Front matter: `key: value` lines between two `---` delimiters, rendered as a two-column
    // table of raw text. Both halves are machine tokens, so neither is unescaped.
    let mut front: Vec<(String, String)> = Vec::new();
    if lines.peek() == Some(&"---") {
        lines.next();
        loop {
            let line = lines.next().ok_or(TypstRefusal::UnclosedFrontMatter)?;
            if line == "---" {
                break;
            }
            let (key, value) = line
                .split_once(": ")
                .ok_or_else(|| TypstRefusal::FrontMatterNotKeyValue(line.to_owned()))?;
            front.push((key.to_owned(), value.to_owned()));
        }
    }
    if !front.is_empty() {
        out.push_str("#table(columns: 2,\n");
        for (key, value) in &front {
            out.push_str(&format!(
                "  [#raw({})], [#raw({})],\n",
                typst_string(key),
                typst_string(value)
            ));
        }
        out.push_str(")\n\n");
    }

    let mut table: Vec<Vec<String>> = Vec::new();
    let mut fence: Option<Vec<String>> = None;

    for line in lines {
        if let Some(buffer) = fence.as_mut() {
            if line.starts_with("```") {
                out.push_str(&format!(
                    "#raw(block: true, {})\n\n",
                    typst_string(&buffer.join("\n"))
                ));
                fence = None;
            } else {
                buffer.push(line.to_owned());
            }
            continue;
        }
        if line.starts_with("```") {
            flush_table(&mut table, &mut out);
            fence = Some(Vec::new());
            continue;
        }
        if line.starts_with('|') {
            if is_delimiter_row(line) {
                continue;
            }
            let mut row = Vec::new();
            for cell in split_cells(line) {
                row.push(format!("[{}]", inline(cell.trim(), false, line)?));
            }
            if let Some(first) = table.first()
                && first.len() != row.len()
            {
                return Err(TypstRefusal::TableRowWidth {
                    expected: first.len(),
                    observed: row.len(),
                });
            }
            table.push(row);
            continue;
        }
        flush_table(&mut table, &mut out);

        if line.starts_with("#### ") {
            return Err(TypstRefusal::HeadingTooDeep(line.to_owned()));
        } else if let Some(rest) = line.strip_prefix("### ") {
            out.push_str(&format!(
                "#heading(level: 3)[{}]\n\n",
                inline(rest, true, line)?
            ));
        } else if let Some(rest) = line.strip_prefix("## ") {
            out.push_str(&format!(
                "#heading(level: 2)[{}]\n\n",
                inline(rest, true, line)?
            ));
        } else if let Some(rest) = line.strip_prefix("# ") {
            out.push_str(&format!(
                "#heading(level: 1)[{}]\n\n",
                inline(rest, true, line)?
            ));
        } else if let Some(rest) = line.strip_prefix("- ") {
            out.push_str(&format!("#list.item[{}]\n", inline(rest, false, line)?));
        } else if line.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("#par[{}]\n\n", inline(line, true, line)?));
        }
    }
    flush_table(&mut table, &mut out);
    if fence.is_some() {
        return Err(TypstRefusal::UnclosedCodeFence);
    }
    Ok(out)
}

fn flush_table(table: &mut Vec<Vec<String>>, out: &mut String) {
    if table.is_empty() {
        return;
    }
    let columns = table[0].len();
    out.push_str(&format!("#table(columns: {columns},\n"));
    for row in table.iter() {
        out.push_str("  ");
        out.push_str(&row.join(", "));
        out.push_str(",\n");
    }
    out.push_str(")\n\n");
    table.clear();
}
