//! Slice 4b — the PDF derivative (ADR 0040, R30).
//!
//! The Markdown bytes are the canonical export. This suite is about the derivative: that it is a
//! function of exactly those bytes and a pinned profile, and of nothing else — no clock, no
//! locale, no host path, no system font, no randomness.

use beacond::{TypstRefusal, to_typst};

const V1: &str = "beacon.outcome.md.v1";

fn fixture() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/outcome/vector-v2.md"),
    )
    .expect("the golden Outcome document reads")
}

#[test]
fn the_real_outcome_document_translates() {
    let markdown = fixture();
    let typst = to_typst(V1, &markdown).expect("the golden document is in the shape v1 emits");

    // Not a smoke test: the constructs the document actually contains must each appear, or a
    // translator that emitted an empty body would pass.
    assert!(typst.contains("#heading(level: 1)"), "no h1");
    assert!(typst.contains("#heading(level: 2)"), "no h2");
    assert!(typst.contains("#heading(level: 3)"), "no h3");
    assert!(
        typst.contains("#table(columns: 2,"),
        "no front-matter table"
    );
    assert!(typst.contains("#list.item["), "no list item");
    assert!(typst.contains("#raw(block: true,"), "no fenced code block");
    assert!(typst.contains("#raw(\""), "no inline code span");
    assert!(typst.contains("#par["), "no paragraph");
    assert!(
        typst.contains("e96fca3429bf3670b04e2e47ca0f5f025bec2247eda874876c304dc21ca8f831"),
        "the manifest digest did not survive translation"
    );

    // Deterministic: the same bytes translate to the same bytes.
    assert_eq!(typst, to_typst(V1, &markdown).unwrap());
}

#[test]
fn an_unknown_renderer_version_is_refused_before_translation() {
    // Row 8. The escaping rule IS the renderer version, so a document this translator cannot name
    // is refused before a byte of it is interpreted -- never read with whatever rule is newest.
    assert_eq!(
        to_typst("beacon.outcome.md.v2", "# Anything\n").unwrap_err(),
        TypstRefusal::UnknownRendererVersion("beacon.outcome.md.v2".to_owned())
    );
}

#[test]
fn a_heading_deeper_than_the_shape_is_refused_by_name() {
    assert_eq!(
        to_typst(V1, "#### Fourth level\n").unwrap_err(),
        TypstRefusal::HeadingTooDeep("#### Fourth level".to_owned())
    );
}

#[test]
fn an_unclosed_code_fence_is_refused_by_name() {
    assert_eq!(
        to_typst(V1, "```\nnever closed\n").unwrap_err(),
        TypstRefusal::UnclosedCodeFence
    );
}

#[test]
fn front_matter_that_is_not_key_value_is_refused_by_name() {
    assert_eq!(
        to_typst(V1, "---\nnot a pair\n---\n").unwrap_err(),
        TypstRefusal::FrontMatterNotKeyValue("not a pair".to_owned())
    );
}

#[test]
fn front_matter_that_never_closes_is_refused_by_name() {
    assert_eq!(
        to_typst(V1, "---\nkey: value\n").unwrap_err(),
        TypstRefusal::UnclosedFrontMatter
    );
}

/// Bold, links, blockquotes and thematic breaks are not four special cases: `escape_v1` escapes
/// every character that can begin a construct, so each of these fails the one inverse rule.
#[test]
fn constructs_outside_the_shape_are_refused_by_the_escaping_rule() {
    for (markdown, character) in [
        ("Some **bold** text\n", '*'),
        ("A [link](https://example.test)\n", '['),
        ("> a blockquote\n", '>'),
        ("***\n", '*'),
        ("<script>alert(1)</script>\n", '<'),
        ("a | pipe outside a table\n", '|'),
    ] {
        match to_typst(V1, markdown) {
            Err(TypstRefusal::UnescapedConstruct { character: got, .. }) => assert_eq!(
                got, character,
                "{markdown:?} was refused for the wrong character"
            ),
            other => panic!("{markdown:?} was not refused as a construct: {other:?}"),
        }
    }
}

#[test]
fn escaped_construct_characters_are_text_and_survive_as_text() {
    // The other half of the same rule: `v1` escapes these, so an ESCAPED one is ordinary prose and
    // must reach the document rather than being refused with it.
    let typst = to_typst(V1, "A star \\* and a pipe \\| and a bracket \\[\n").unwrap();
    assert!(
        typst.contains("A star * and a pipe | and a bracket ["),
        "{typst}"
    );
}

#[test]
fn a_table_row_of_the_wrong_width_is_refused_by_name() {
    assert_eq!(
        to_typst(V1, "| a | b |\n| --- | --- |\n| c |\n").unwrap_err(),
        TypstRefusal::TableRowWidth {
            expected: 2,
            observed: 1
        }
    );
}
