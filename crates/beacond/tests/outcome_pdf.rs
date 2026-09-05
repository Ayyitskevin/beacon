//! Slice 4b — the PDF derivative (ADR 0040, R30).
//!
//! The Markdown bytes are the canonical export. This suite is about the derivative: that it is a
//! function of exactly those bytes and a pinned profile, and of nothing else — no clock, no
//! locale, no host path, no system font, no randomness.

use beacond::{BEACON_OUTCOME_1, PdfProfile, PdfRefusal, TypstRefusal, render_pdf, to_typst};

const V1: &str = "beacon.outcome.md.v1";
const DIGEST: &str = "e96fca3429bf3670b04e2e47ca0f5f025bec2247eda874876c304dc21ca8f831";
/// The environment variable that turns [`helper_child_render`] from a no-op into the child half of
/// the two-process bar.
const CHILD_OUT: &str = "BEACON_PDF_TWO_PROCESS_OUT";

fn render(markdown: &str, digest: &str, profile: &PdfProfile) -> Vec<u8> {
    render_pdf(markdown.as_bytes(), V1, digest, profile).expect("the golden document renders")
}

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

// ---------------------------------------------------------------------------------------------
// Item 2 — render_pdf
// ---------------------------------------------------------------------------------------------

/// The child half of [`two_renders_of_one_document_are_byte_identical`].
///
/// It is a `#[test]` because that is the only entry point a test binary has. Without the
/// environment variable it does nothing, because the parent is what asserts; with it, it renders
/// and writes, and the parent compares the two files it left behind.
#[test]
fn helper_child_render() {
    let Ok(path) = std::env::var(CHILD_OUT) else {
        return;
    };
    std::fs::write(path, render(&fixture(), DIGEST, &BEACON_OUTCOME_1)).unwrap();
}

/// Seconds since the epoch, for [`two_renders_of_one_document_are_byte_identical`]'s tick wait.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the host clock is after the Unix epoch")
        .as_secs()
}

/// Two renders in two SEPARATE PROCESSES, **on two different wall-clock seconds**.
///
/// Separate processes because two calls in one would miss anything seeded once per process -- a
/// hash seed, a per-process counter, an address that reaches the bytes. The child is this same
/// test binary (`current_exe`) run with `--exact helper_child_render` and an output path in the
/// environment, so the two renders share no memory at all.
///
/// # Why it waits for the second to tick
///
/// blue's probe (`D-blue-confirm-s4b.md`): with the clock deliberately let in
/// (`timestamp: Some(now)` and `date: auto`), this bar was GREEN 5 runs out of 5. The two children
/// run about 0.4 s apart, so they almost always fall inside the same wall-clock second and a
/// second-resolution clock leak produces identical bytes. A bar that only fails when the scheduler
/// happens to straddle a second is the false-green shape this suite exists to avoid, and the
/// report named this bar as *the* clock bar, which made the claim worse than the gap.
///
/// So the parent waits for the second to change between the two children. The wait is bounded by
/// construction -- the second always ticks, so it is under 1 s -- and it makes a second-resolution
/// leak RED on every run rather than on a lucky one.
///
/// A faked clock would be the other way to force disagreement, and it is not available: `faketime`
/// is a new tool, and injecting a time seam into `render_pdf` would add a knob to the product for
/// a test's benefit -- the exact shape of guard this fleet has been bitten by, where the thing
/// under test supplies the value that constrains it. Waiting costs one second and adds nothing to
/// the product.
#[test]
fn two_renders_of_one_document_are_byte_identical() {
    let directory = std::env::temp_dir().join(format!("beacon-pdf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut written = Vec::new();
    for run in ["a", "b"] {
        if run == "b" {
            let second = now_secs();
            while now_secs() == second {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        let path = directory.join(format!("{run}.pdf"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "helper_child_render", "--nocapture"])
            .env(CHILD_OUT, &path)
            .status()
            .expect("the child test binary runs");
        assert!(status.success(), "the child render failed");
        written.push(std::fs::read(&path).expect("the child wrote its PDF"));
    }
    assert!(
        !written[0].is_empty(),
        "an empty render would compare equal to another empty render"
    );
    assert_eq!(
        written[0], written[1],
        "two processes rendered the same document to different bytes"
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn one_changed_markdown_byte_changes_the_pdf() {
    let markdown = fixture();
    let changed = markdown.replacen("Outcome", "Outcomf", 1);
    assert_ne!(
        markdown, changed,
        "the fixture did not change, so this proves nothing"
    );
    assert_ne!(
        render(&markdown, DIGEST, &BEACON_OUTCOME_1),
        render(&changed, DIGEST, &BEACON_OUTCOME_1),
        "the renderer does not read its input"
    );
}

#[test]
fn a_changed_profile_changes_the_pdf() {
    let markdown = fixture();
    let larger = PdfProfile {
        body_pt: BEACON_OUTCOME_1.body_pt + 1,
        ..BEACON_OUTCOME_1
    };
    assert_ne!(
        render(&markdown, DIGEST, &BEACON_OUTCOME_1),
        render(&markdown, DIGEST, &larger),
        "the profile does not reach the bytes, so pinning it proves nothing"
    );
}

#[test]
fn the_pdf_identifier_is_derived_from_the_candidate_digest() {
    let markdown = fixture();
    let mine = render(&markdown, DIGEST, &BEACON_OUTCOME_1);
    let other = render(&markdown, &DIGEST.replace('e', "f"), &BEACON_OUTCOME_1);
    assert_ne!(
        mine, other,
        "two candidates share a PDF identifier, so deriving it is theatre"
    );
    assert_eq!(
        mine,
        render(&markdown, DIGEST, &BEACON_OUTCOME_1),
        "the same candidate rendered two different identifiers"
    );
    // The identifier is HASHED into the PDF, not written: the digest is an identity, and a
    // derivative should not restate one.
    assert!(
        !String::from_utf8_lossy(&mine).contains(DIGEST_HEAD),
        "the candidate digest was written into the PDF rather than hashed into its identifier"
    );
}

/// A prefix long enough to be unmistakable and short enough to survive any line wrapping.
const DIGEST_HEAD: &str = "e96fca3429bf3670";

#[test]
fn a_document_that_is_not_utf8_is_refused_by_name() {
    assert_eq!(
        render_pdf(&[0xff, 0xfe, 0x00], V1, DIGEST, &BEACON_OUTCOME_1).unwrap_err(),
        PdfRefusal::NotUtf8
    );
}

#[test]
fn a_document_outside_the_shape_is_refused_through_the_translator() {
    assert!(matches!(
        render_pdf(b"#### too deep\n", V1, DIGEST, &BEACON_OUTCOME_1).unwrap_err(),
        PdfRefusal::Translate(TypstRefusal::HeadingTooDeep(_))
    ));
}

/// No date survives as plain bytes.
///
/// Named for exactly what it proves, because the first draft of it claimed more. When the clock is
/// deliberately let in, typst writes the date somewhere this scan cannot see -- a compressed
/// stream -- and this test stays green while `two_renders_of_one_document_are_byte_identical`
/// goes red. So the two-process bar is the clock bar; this one catches the plain-text case and
/// documents where the date is not.
#[test]
fn the_pdf_carries_no_plaintext_date() {
    let pdf = render(&fixture(), DIGEST, &BEACON_OUTCOME_1);
    let text = String::from_utf8_lossy(&pdf);
    for needle in ["CreationDate", "ModDate", "D:20"] {
        assert!(!text.contains(needle), "the PDF carries a date: {needle}");
    }
}

/// R30.2: the profile belongs to the renderer, so it names itself in the PDF rather than in the
/// document's front matter. Front matter would change every byte of every stored document.
#[test]
fn the_profile_names_itself_in_the_pdf_creator() {
    let pdf = render(&fixture(), DIGEST, &BEACON_OUTCOME_1);
    assert!(
        String::from_utf8_lossy(&pdf).contains(BEACON_OUTCOME_1.name),
        "the profile does not name itself in the rendered bytes"
    );
    // And the document it rendered from does not name a profile, because that is the renderer's.
    assert!(
        !fixture().contains("pdf_profile"),
        "the Markdown document declares a PDF profile, which R30.2 says it must not"
    );
}

// ---------------------------------------------------------------------------------------------
// Item 3 — isolation
// ---------------------------------------------------------------------------------------------

#[test]
fn an_empty_embedded_font_set_refuses_by_name() {
    // Never a fallback. A renderer that finds a face on the host renders differently on two
    // machines, and the operator has no way to know which face they are looking at.
    let fontless = PdfProfile {
        faces: &[],
        ..BEACON_OUTCOME_1
    };
    assert_eq!(
        render_pdf(fixture().as_bytes(), V1, DIGEST, &fontless).unwrap_err(),
        PdfRefusal::EmbeddedFontSetEmpty
    );
}

/// The child of [`the_same_document_renders_identically_under_two_locales_and_timezones`].
#[test]
fn helper_child_render_env() {
    let Ok(path) = std::env::var(CHILD_OUT) else {
        return;
    };
    std::fs::write(path, render(&fixture(), DIGEST, &BEACON_OUTCOME_1)).unwrap();
}

/// Two processes under hostile environments. Separate processes because a locale is read once per
/// process, so setting the variables inside one would prove nothing about a renderer that had
/// already read them.
#[test]
fn the_same_document_renders_identically_under_two_locales_and_timezones() {
    let directory = std::env::temp_dir().join(format!("beacon-pdf-env-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let environments = [("Pacific/Kiritimati", "de_DE.UTF-8"), ("Etc/GMT+12", "C")];
    let mut written = Vec::new();
    for (index, (timezone, locale)) in environments.iter().enumerate() {
        let path = directory.join(format!("{index}.pdf"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "helper_child_render_env", "--nocapture"])
            .env(CHILD_OUT, &path)
            .env("TZ", timezone)
            .env("LANG", locale)
            .env("LC_ALL", locale)
            .status()
            .expect("the child test binary runs");
        assert!(
            status.success(),
            "the child render failed under {timezone}/{locale}"
        );
        written.push(std::fs::read(&path).unwrap());
    }
    assert!(!written[0].is_empty(), "an empty render proves nothing");
    assert_eq!(
        written[0], written[1],
        "the same document rendered differently under two locales and timezones"
    );
    std::fs::remove_dir_all(&directory).ok();
}

/// Nothing about this host reaches the bytes.
#[test]
fn the_pdf_names_nothing_about_the_host() {
    let pdf = render(&fixture(), DIGEST, &BEACON_OUTCOME_1);
    let text = String::from_utf8_lossy(&pdf);
    let mut needles: Vec<String> = vec![
        "/home".to_owned(),
        "LC_".to_owned(),
        "/usr/share/fonts".to_owned(),
        "TZ=".to_owned(),
    ];
    if let Ok(home) = std::env::var("HOME") {
        needles.push(home);
    }
    if let Ok(user) = std::env::var("USER") {
        needles.push(user);
    }
    if let Ok(host) = std::fs::read_to_string("/etc/hostname") {
        let host = host.trim().to_owned();
        if !host.is_empty() {
            needles.push(host);
        }
    }
    assert!(
        needles.len() > 4,
        "the scan found nothing host-specific to look for, so it proves nothing"
    );
    for needle in &needles {
        assert!(
            !text.contains(needle.as_str()),
            "the PDF names something about this host: {needle}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Fold — the quote
// ---------------------------------------------------------------------------------------------

/// The other half of the two-character escape surface.
///
/// `typst_string` escapes `\` and `"`. Only the backslash half was proven: `escape_v1` escapes
/// neither quote nor anything that produces one, `vector-v2.md` contains **zero** double quotes,
/// and no other text run in this suite carries one — so removing `|| character == '"'` left the
/// whole suite green (Fable's mutant at `565f149`).
///
/// The input is what `escape_v1` would emit for prose containing every character that matters
/// here: `"` bare, because `escape_v1` escapes neither `"` nor anything that yields one; and
/// `\ # [ * ` + backslash-escaped, because it escapes those. A quote sits at the very end of the
/// run, where an off-by-one in the escaper shows up first.
///
/// # Why this asserts the translated source and a differential render, not extracted PDF text
///
/// Typst compresses content streams, so pulling text back out means decompressing and parsing PDF
/// text operators — a second decoder, in a test, whose bugs would produce a confident wrong
/// answer. Instead: the source assertion kills the mutant deterministically, the render proves the
/// escaped form is valid Typst, and rendering the same document *without* the quotes and asserting
/// the bytes differ proves the quote reaches the output rather than only the source. Three cheap
/// facts beat one fragile decoder.
#[test]
fn a_text_run_carrying_quotes_and_backslashes_survives_translation_and_renders() {
    let quoted = "A quote \" and a backslash \\\\ and a hash \\# and a bracket \\[ \
                  and a star \\* and a tick \\` and trailing \"\n";
    let typst = to_typst(V1, quoted).expect("bare quotes are ordinary prose in v1");

    // The escaped forms, spelled out. With `"` unescaped the source reads `#"A quote " and ...`,
    // which is a different string and usually not a string at all.
    assert!(
        typst.contains(r#"A quote \" and"#),
        "the quote was not escaped for the Typst string literal: {typst}"
    );
    assert!(
        typst.contains(r#"and trailing \""#),
        "a quote at the end of a run was not escaped: {typst}"
    );
    assert!(
        typst.contains(r#"a backslash \\ and"#),
        "the backslash was not escaped: {typst}"
    );
    // The characters `escape_v1` escaped are text again, and are NOT re-escaped: they have no
    // meaning inside a string literal, which is the whole point of using one.
    assert!(
        typst.contains("a hash # and a bracket [ and a star * and a tick ` and"),
        "an escaped construct character did not come back as text: {typst}"
    );

    // The escaped form is valid Typst.
    let pdf = render(quoted, DIGEST, &BEACON_OUTCOME_1);

    // And the quotes reach the rendered bytes: the same document without them renders differently.
    let unquoted = quoted.replace('"', "");
    assert_ne!(
        quoted, unquoted,
        "the input had no quotes, so this proves nothing"
    );
    assert_ne!(
        pdf,
        render(&unquoted, DIGEST, &BEACON_OUTCOME_1),
        "the quotes did not reach the rendered document"
    );
}

/// The same blind spot, checked in the other direction and found absent.
///
/// `escape_v1` escapes [`ESCAPE_ANYWHERE`] and [`ESCAPE_AT_START`], and `"` is in neither, so it
/// **cannot emit `\"`** — the fixture's twelve backslash escapes are all inside those sets. So
/// `unescape_checked` has no quote-shaped blind spot to have. It is exercised here anyway, because
/// "cannot happen" is worth one assertion: a `\"` unescapes to a quote like any other escape, and
/// that quote is then escaped again on the way into the string literal.
#[test]
fn an_escaped_quote_unescapes_to_a_quote_and_is_escaped_again() {
    let typst = to_typst(V1, "He said \\\"hello\\\" once\n").unwrap();
    assert!(
        typst.contains(r#"He said \"hello\" once"#),
        "a backslash-escaped quote did not round-trip: {typst}"
    );
    render("He said \\\"hello\\\" once\n", DIGEST, &BEACON_OUTCOME_1);
}
