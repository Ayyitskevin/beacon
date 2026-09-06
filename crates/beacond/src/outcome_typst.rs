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
    /// R42: a closed caller line. The document's own text -- the renderer version it named, the
    /// offending source line, the construct character -- stays in the typed refusal and in the
    /// derived `Debug`, which is where a document author's tooling should read it. `to_typst` is
    /// public, so this `Display` reaches callers who may have no business seeing the document.
    ///
    /// The row-width counts stay: two cell counts are the shape of the table, not its contents.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // R42: the version the document named is supplied text. Which version this
            // translator reads is not a secret, but a caller does not need the document's copy of
            // it to act, and it arrives from the document rather than from us.
            Self::UnknownRendererVersion(_) => formatter.write_str(
                "the document names an unsupported renderer version: UnknownRendererVersion",
            ),
            Self::UnclosedFrontMatter => {
                formatter.write_str("the front matter opened and never closed")
            }
            Self::FrontMatterNotKeyValue(_) => formatter
                .write_str("a front-matter line is not `key: value`: FrontMatterNotKeyValue"),
            Self::UnclosedCodeFence => formatter.write_str("a code fence opened and never closed"),
            Self::HeadingTooDeep(_) => formatter
                .write_str("a heading is deeper than this document shape permits: HeadingTooDeep"),
            // Both the character and the line it sat on are document content. The character is
            // as much a disclosure as the line: it is one the author typed.
            Self::UnescapedConstruct { .. } => formatter.write_str(
                "the document contains an unsupported unescaped construct: UnescapedConstruct",
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

#[cfg(test)]
mod refusal_text_witness {
    use super::*;

    /// Two fixed synthetic markers, each before a synthetic suffix and a CR/LF, so "is it in the
    /// line" is a substring question rather than an escaping question.
    const NEEDLE_A: &str = "R42S3_PRIVATE_A";
    const NEEDLE_B: &str = "R42S3_PRIVATE_B";

    /// Approved numeric context, chosen here before any refusal is built and pairwise distinct so
    /// a swapped or duplicated count cannot pass.
    const FIRST_EXPECTED: usize = 7;
    const FIRST_OBSERVED: usize = 11;
    const SECOND_EXPECTED: usize = 13;
    const SECOND_OBSERVED: usize = 17;

    /// Two independently selected construct characters. Neither may reach a line; both must stay
    /// in Debug. They are arbitrary public-constructor inputs, not parser-reachable ones.
    const FIRST_CHARACTER: char = '§';
    const SECOND_CHARACTER: char = '☃';

    fn payload_a() -> String {
        format!("{NEEDLE_A} /home/example/private.md column=private_column\r\nsecond-line")
    }

    fn payload_b() -> String {
        format!("{NEEDLE_B} /home/example/other.md column=other_column\r\nsecond-line")
    }

    /// One declarative row table, expanded into BOTH the exhaustive expected match and the
    /// constructed fixtures. Wildcard-free: a new translator arm fails to compile here until its
    /// row supplies a constructor, an independently written expected literal, and its markers.
    macro_rules! closed_table {
        (
            $expected_fn:ident / $rows_fn:ident : $ty:ty,
            $( $case:literal | $pattern:pat => $line:expr , $build:expr , $markers:expr ; )+
        ) => {
            fn $expected_fn(value: &$ty) -> (&'static str, String) {
                match value {
                    $( $pattern => ($case, String::from($line)), )+
                }
            }

            fn $rows_fn() -> Vec<(&'static str, $ty, &'static [&'static str])> {
                vec![$(($case, $build, $markers)),+]
            }
        };
    }

    closed_table! {
        expected_typst / typst_rows: TypstRefusal,
        "UnknownRendererVersion" | TypstRefusal::UnknownRendererVersion(_)
            => "the document names an unsupported renderer version: UnknownRendererVersion",
            TypstRefusal::UnknownRendererVersion(payload_a()), &[NEEDLE_A];
        "UnclosedFrontMatter" | TypstRefusal::UnclosedFrontMatter
            => "the front matter opened and never closed",
            TypstRefusal::UnclosedFrontMatter, &[];
        "FrontMatterNotKeyValue" | TypstRefusal::FrontMatterNotKeyValue(_)
            => "a front-matter line is not `key: value`: FrontMatterNotKeyValue",
            TypstRefusal::FrontMatterNotKeyValue(payload_a()), &[NEEDLE_A];
        "UnclosedCodeFence" | TypstRefusal::UnclosedCodeFence
            => "a code fence opened and never closed",
            TypstRefusal::UnclosedCodeFence, &[];
        "HeadingTooDeep" | TypstRefusal::HeadingTooDeep(_)
            => "a heading is deeper than this document shape permits: HeadingTooDeep",
            TypstRefusal::HeadingTooDeep(payload_a()), &[NEEDLE_A];
        "UnescapedConstruct" | TypstRefusal::UnescapedConstruct { .. }
            => "the document contains an unsupported unescaped construct: UnescapedConstruct",
            TypstRefusal::UnescapedConstruct {
                character: FIRST_CHARACTER,
                line: payload_a(),
            },
            &[NEEDLE_A];
        "TableRowWidth" | TypstRefusal::TableRowWidth { expected, observed }
            => format!("a table row has {observed} cells where the table opened with {expected}"),
            TypstRefusal::TableRowWidth {
                expected: FIRST_EXPECTED,
                observed: FIRST_OBSERVED,
            },
            &[];
    }

    /// The characters a case's fixture carries that must not reach its line. Declared, never
    /// inferred from what the output happens to contain.
    fn hidden_characters(case: &str) -> &'static [char] {
        match case {
            "UnescapedConstruct" => &[FIRST_CHARACTER],
            _ => &[],
        }
    }

    /// R42 slice 3: every `TypstRefusal` line is the closed caller projection, and every piece of
    /// document text it was built from is still in `Debug`.
    ///
    /// Four of the seven arms carried supplied text: the renderer version the document named, two
    /// source lines, and the construct character. `to_typst` is public, so this `Display` reaches
    /// callers with no claim on the document. The three already-closed arms are pinned as
    /// regressions -- closing four lines must not reword the three that were already right.
    #[test]
    fn r42_translator_projection_is_closed() {
        let rows = typst_rows();
        assert_eq!(rows.len(), 7, "R42S3.TYPST.CARDINALITY.ROOT");
        let ids: std::collections::BTreeSet<&str> = rows.iter().map(|(case, _, _)| *case).collect();
        assert_eq!(ids.len(), rows.len(), "R42S3.TYPST.CASE_IDS_UNIQUE");

        for (case, value, markers) in &rows {
            // 1. Fixture premise. A failure here is a broken fixture, not a mutant kill.
            let (matched, expected) = expected_typst(value);
            assert_eq!(matched, *case, "R42S3.FIXTURE.{case}.PATTERN");
            let debug = format!("{value:?}");
            for marker in *markers {
                assert!(debug.contains(marker), "R42S3.FIXTURE.{case}.NEEDLE");
            }

            // 2. Nothing hidden reaches the line, and the line is one line.
            let line = value.to_string();
            for marker in *markers {
                assert!(
                    !line.contains(marker),
                    "R42S3.TYPST.{case}.NO_NEEDLE: {line}"
                );
            }
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "R42S3.TYPST.{case}.ONE_LINE: {line:?}"
            );
            for marker in *markers {
                assert!(debug.contains(marker), "R42S3.TYPST.{case}.DEBUG_RETAINS");
            }
            // A construct character is document content too, and is checked at this rung -- after
            // the markers, before the projection -- so a line that appends it fails by name.
            for character in hidden_characters(case) {
                assert!(
                    !line.contains(*character),
                    "R42S3.TYPST.{case}.NO_CHARACTER: {line}"
                );
                assert!(
                    debug.contains(*character),
                    "R42S3.TYPST.{case}.DEBUG_CHARACTER"
                );
            }

            // 3. The whole line against the independent literal.
            assert_eq!(line, expected, "R42S3.TYPST.{case}.PROJECTION");
        }

        // The B seed: different hidden input, same independently fixed output.
        for (case, first, second, marker) in [
            (
                "UnknownRendererVersion",
                TypstRefusal::UnknownRendererVersion(payload_a()),
                TypstRefusal::UnknownRendererVersion(payload_b()),
                NEEDLE_B,
            ),
            (
                "FrontMatterNotKeyValue",
                TypstRefusal::FrontMatterNotKeyValue(payload_a()),
                TypstRefusal::FrontMatterNotKeyValue(payload_b()),
                NEEDLE_B,
            ),
            (
                "HeadingTooDeep",
                TypstRefusal::HeadingTooDeep(payload_a()),
                TypstRefusal::HeadingTooDeep(payload_b()),
                NEEDLE_B,
            ),
            (
                "UnescapedConstruct",
                TypstRefusal::UnescapedConstruct {
                    character: FIRST_CHARACTER,
                    line: payload_a(),
                },
                TypstRefusal::UnescapedConstruct {
                    character: SECOND_CHARACTER,
                    line: payload_b(),
                },
                NEEDLE_B,
            ),
        ] {
            let (_, expected) = expected_typst(&second);
            assert!(
                !second.to_string().contains(marker),
                "R42S3.TYPST.{case}.NO_NEEDLE"
            );
            assert_eq!(
                second.to_string(),
                expected,
                "R42S3.TYPST.{case}.PROJECTION"
            );
            assert_eq!(
                first.to_string(),
                second.to_string(),
                "R42S3.TYPST.{case}.HIDDEN_CHANGE_SAME_LINE"
            );
            assert!(
                format!("{second:?}").contains(marker),
                "R42S3.TYPST.{case}.DEBUG_RETAINS"
            );
        }
        assert!(
            !TypstRefusal::UnescapedConstruct {
                character: SECOND_CHARACTER,
                line: payload_b(),
            }
            .to_string()
            .contains(SECOND_CHARACTER),
            "R42S3.TYPST.UnescapedConstruct.NO_CHARACTER"
        );

        // 4. Approved context: both counts, both pairs, each pair distinct.
        for (expected_cells, observed_cells) in [
            (FIRST_EXPECTED, FIRST_OBSERVED),
            (SECOND_EXPECTED, SECOND_OBSERVED),
        ] {
            assert_eq!(
                TypstRefusal::TableRowWidth {
                    expected: expected_cells,
                    observed: observed_cells,
                }
                .to_string(),
                format!(
                    "a table row has {observed_cells} cells where the table opened with \
                     {expected_cells}"
                ),
                "R42S3.TYPST.TableRowWidth.CONTEXT"
            );
        }

        // The four call-through cases: the real parser, not a hand-built fixture.
        for (case, version, markdown) in [
            ("UnknownRendererVersion", NEEDLE_A, ""),
            // The supported version is written as the independently specified literal, not read
            // from `READS_RENDERER_VERSION`: an input pinned from the implementation is not an
            // independent input (astra's read).
            (
                "FrontMatterNotKeyValue",
                "beacon.outcome.md.v1",
                "---\nR42S3_PRIVATE_A\n---\n",
            ),
            (
                "HeadingTooDeep",
                "beacon.outcome.md.v1",
                "#### R42S3_PRIVATE_A\n",
            ),
            (
                "UnescapedConstruct",
                "beacon.outcome.md.v1",
                "Some * R42S3_PRIVATE_A\n",
            ),
        ] {
            let refusal =
                to_typst(version, markdown).expect_err("the translator refuses this document");
            let (matched, expected) = expected_typst(&refusal);
            // The variant first: a refusal from another parser branch is a false probe.
            assert_eq!(matched, case, "R42S3.TYPST.CALL.{case}.VARIANT");
            let line = refusal.to_string();
            assert!(
                !line.contains(NEEDLE_A),
                "R42S3.TYPST.CALL.{case}.NO_NEEDLE: {line}"
            );
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "R42S3.TYPST.CALL.{case}.ONE_LINE: {line:?}"
            );
            assert!(
                format!("{refusal:?}").contains(NEEDLE_A),
                "R42S3.TYPST.CALL.{case}.DEBUG_RETAINS"
            );
            assert_eq!(line, expected, "R42S3.TYPST.CALL.{case}.PROJECTION");
        }
    }
}
