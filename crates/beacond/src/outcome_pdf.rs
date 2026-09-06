//! The Outcome Document's PDF derivative (ADR 0040, R30).
//!
//! `pdf_bytes = render_pdf(exact_markdown_bytes, pinned_profile)`. The Markdown bytes stay the
//! canonical export; this is a **deletable derivative** and never a source of truth. Delete it and
//! render it again and the bytes are the same, which is the property that makes it deletable.
//!
//! # What it must not depend on
//!
//! No network, no clock, no host path, no locale, no system font, no random identifier. Each of
//! those would make the same document render differently on two machines or on two days, and a
//! derivative that cannot be reproduced is a second source of truth by accident.
//!
//! Every one of them is closed in a specific place, named here so a reader does not have to find
//! them: the clock by `PdfOptions::timestamp: None` **and** [`BeaconWorld::today`] returning
//! `None`; the identifier by `ident` derived from the candidate Artifact digest; the fonts by
//! `include_bytes!` and a [`BeaconWorld`] whose `file()` never resolves anything; the locale by a
//! fixed `lang` in the profile; the engine version by exact `=0.15.1` pins in `Cargo.toml`, since
//! the bytes are a function of it.

use std::fmt;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration, Smart};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

use crate::outcome_typst::{self, TypstRefusal};

/// One embedded font face.
///
/// `bytes` is always `include_bytes!`. A face read from the host at run time would be a host path,
/// and the same document would render differently on two machines.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedFace {
    /// The family name the profile's Typst preamble asks for.
    pub family: &'static str,
    pub bytes: &'static [u8],
}

/// The faces `beacon-outcome-1` embeds.
///
/// **A font slot.** Swapping the face is a change to this one table and the `family` names in
/// [`PdfProfile`] beside it — nothing else in this module knows which font it is. The licences
/// ship next to the files in `assets/fonts/`.
const BEACON_OUTCOME_1_FACES: &[EmbeddedFace] = &[
    EmbeddedFace {
        family: "Libertinus Serif",
        bytes: include_bytes!("../assets/fonts/LibertinusSerif-Regular.otf"),
    },
    EmbeddedFace {
        family: "Libertinus Serif",
        bytes: include_bytes!("../assets/fonts/LibertinusSerif-Bold.otf"),
    },
    EmbeddedFace {
        family: "DejaVu Sans Mono",
        bytes: include_bytes!("../assets/fonts/DejaVuSansMono.ttf"),
    },
];

/// Everything the rendered bytes depend on besides the document itself.
///
/// A constant table in code, never a file on disk and never an environment read: a profile that
/// could be edited beside the binary would make the bytes a function of the host after all.
#[derive(Debug, Clone, Copy)]
pub struct PdfProfile {
    /// The name that rides in the PDF's `Creator`. This is where the profile is named — **not** in
    /// the document's front matter (R30.2): the profile belongs to the renderer, and putting it in
    /// front matter would change every byte of every stored document and need a new
    /// `renderer_version`.
    pub name: &'static str,
    pub faces: &'static [EmbeddedFace],
    pub body_family: &'static str,
    pub mono_family: &'static str,
    pub page: &'static str,
    pub margin_mm: u32,
    pub body_pt: u32,
    pub mono_pt: u32,
    /// Fixed, never the host locale.
    pub lang: &'static str,
}

/// The one profile this version ships.
pub const BEACON_OUTCOME_1: PdfProfile = PdfProfile {
    name: "beacon-outcome-1",
    faces: BEACON_OUTCOME_1_FACES,
    body_family: "Libertinus Serif",
    mono_family: "DejaVu Sans Mono",
    page: "a4",
    margin_mm: 20,
    body_pt: 10,
    mono_pt: 9,
    lang: "en",
};

/// Why a document could not be rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfRefusal {
    /// The exported bytes are not UTF-8. The canonical export is bytes, so this is checked rather
    /// than assumed.
    NotUtf8,
    /// The document is not in the shape this renderer version emits.
    Translate(TypstRefusal),
    /// The profile embeds no font. Loud, and never a fallback: a renderer that finds a face on the
    /// host renders differently on two machines.
    EmbeddedFontSetEmpty,
    /// Typst refused the translated source.
    Compile(String),
    /// Typst refused to serialize the laid-out document.
    Serialize(String),
}

/// The translator refusal's NAME, for this caller's outward line (R42).
///
/// `TypstRefusal`'s `Display` is now a closed caller line. The original version, source line and
/// character remain in the typed refusal and Debug. This PDF selector keeps its own established
/// projection; it does not forward the translator's wording.
/// The exhaustive match is the point -- a new translator arm is a compile error here rather than a
/// new sentence nobody read.
///
/// The row-width counts are the one permitted piece of context (R42's allowed-context list): two
/// decimal cell counts, carrying no document text.
fn typst_tag(refusal: &TypstRefusal) -> String {
    match refusal {
        TypstRefusal::UnknownRendererVersion(_) => "UnknownRendererVersion".to_owned(),
        TypstRefusal::UnclosedFrontMatter => "UnclosedFrontMatter".to_owned(),
        TypstRefusal::FrontMatterNotKeyValue(_) => "FrontMatterNotKeyValue".to_owned(),
        TypstRefusal::UnclosedCodeFence => "UnclosedCodeFence".to_owned(),
        TypstRefusal::HeadingTooDeep(_) => "HeadingTooDeep".to_owned(),
        TypstRefusal::UnescapedConstruct { .. } => "UnescapedConstruct".to_owned(),
        TypstRefusal::TableRowWidth { expected, observed } => {
            format!("TableRowWidth(expected={expected}, observed={observed})")
        }
    }
}

impl fmt::Display for PdfRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 => formatter.write_str("the exported document is not UTF-8"),
            Self::Translate(refusal) => write!(formatter, "Translate.{}", typst_tag(refusal)),
            Self::EmbeddedFontSetEmpty => formatter.write_str(
                "the render profile embeds no font, and this renderer never looks for one on the \
                 host",
            ),
            Self::Compile(_) => formatter.write_str("Compile"),
            Self::Serialize(_) => formatter.write_str("Serialize"),
        }
    }
}

impl From<TypstRefusal> for PdfRefusal {
    fn from(refusal: TypstRefusal) -> Self {
        Self::Translate(refusal)
    }
}

/// The profile's Typst preamble. Every value comes from the table; none is written twice.
fn preamble(profile: &PdfProfile) -> String {
    let PdfProfile {
        page,
        margin_mm,
        body_family,
        body_pt,
        mono_family,
        mono_pt,
        lang,
        ..
    } = *profile;
    format!(
        "#set document(title: \"Outcome\", author: (), date: none)\n\
         #set page(paper: \"{page}\", margin: (x: {margin_mm}mm, y: {margin_mm}mm), \
         numbering: none)\n\
         #set text(font: \"{body_family}\", size: {body_pt}pt, lang: \"{lang}\")\n\
         #show raw: set text(font: \"{mono_family}\", size: {mono_pt}pt)\n\
         #set heading(numbering: none)\n"
    )
}

/// A [`World`] with exactly one source file, a fixed font set, and no clock.
struct BeaconWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: FileId,
    source: Source,
}

impl BeaconWorld {
    fn new(typst_source: String, faces: &[EmbeddedFace]) -> Result<Self, PdfRefusal> {
        let mut fonts = Vec::new();
        let mut book = FontBook::new();
        for face in faces {
            for font in Font::iter(Bytes::new(face.bytes.to_vec())) {
                book.push(font.info().clone());
                fonts.push(font);
            }
        }
        if fonts.is_empty() {
            return Err(PdfRefusal::EmbeddedFontSetEmpty);
        }
        let main = RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("outcome.typ").unwrap(),
        )
        .intern();
        let source = Source::new(main, typst_source);
        Ok(Self {
            library: LazyHash::new(Library::builder().build()),
            book: LazyHash::new(book),
            fonts,
            main,
            source,
        })
    }
}

impl World for BeaconWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main {
            Ok(self.source.clone())
        } else {
            Err(FileError::NotFound(std::path::PathBuf::from(format!(
                "{id:?}"
            ))))
        }
    }

    /// Nothing on disk is reachable. There is one source and no packages, so every other file id
    /// is a document trying to reach the host.
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(std::path::PathBuf::from(format!(
            "{id:?}"
        ))))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    /// No clock. Typst's `datetime` errors rather than reading one, which is the honest answer for
    /// a renderer whose output must not change between two days.
    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

/// Render one exported Outcome Document to PDF.
///
/// `candidate_artifact_digest` becomes the PDF's document identifier (R29.2): the same document
/// always carries the same identifier, and two different documents never share one. Typst hashes
/// it rather than writing it, so the digest itself does not appear in the bytes.
pub fn render_pdf(
    markdown_bytes: &[u8],
    renderer_version: &str,
    candidate_artifact_digest: &str,
    profile: &PdfProfile,
) -> Result<Vec<u8>, PdfRefusal> {
    let markdown = std::str::from_utf8(markdown_bytes).map_err(|_| PdfRefusal::NotUtf8)?;
    let body = outcome_typst::to_typst(renderer_version, markdown)?;
    let world = BeaconWorld::new(format!("{}{body}", preamble(profile)), profile.faces)?;

    let compiled = typst::compile::<PagedDocument>(&world);
    let document = compiled
        .output
        .map_err(|errors| PdfRefusal::Compile(format!("{errors:?}")))?;

    let options = typst_pdf::PdfOptions {
        // Derived, never random and never `Auto`: `Auto` hashes the title and author, which are
        // the same for every Outcome Document ever rendered.
        ident: Smart::Custom(candidate_artifact_digest.to_owned()),
        // The profile's name, not the engine's. `Auto` writes `Typst $version`, which would put
        // the engine version in the bytes a second time and change it on a patch bump.
        creator: Smart::Custom(Some(profile.name.to_owned())),
        // The clock, closed.
        timestamp: None,
        page_ranges: None,
        standards: typst_pdf::PdfStandards::default(),
        tagged: false,
        pretty: false,
    };
    typst_pdf::pdf(&document, &options)
        .map_err(|errors| PdfRefusal::Serialize(format!("{errors:?}")))
}

#[cfg(test)]
mod refusal_text_witness {
    use super::*;

    /// Two fixed synthetic markers, one per seed. Every diagnostic payload a case carries holds
    /// one, so "did this line disclose the payload" is a substring search rather than an eyeball.
    const NEEDLE_A: &str = "R42S1_PRIVATE_A";
    const NEEDLE_B: &str = "R42S1_PRIVATE_B";

    fn payload_a() -> String {
        format!("{NEEDLE_A}|column=private_column|path=/home/example-user/private.txt\nsecond-line")
    }

    fn payload_b() -> String {
        format!(
            "{NEEDLE_B}|column=other_column|path=/home/example-user/other-note.txt\nsecond-line"
        )
    }

    /// One declarative row table, expanded into BOTH the exhaustive expected match and the
    /// constructed fixtures. The match is wildcard-free, so a new translator or PDF arm fails to
    /// compile here until its row supplies a constructor, an independently written expected
    /// literal, and the markers its payload carries. Nothing is read from `typst_tag`.
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

    /// The row-width counts, chosen here before any refusal is built. They are approved outward
    /// context, so they are asserted PRESENT.
    const ROW_WIDTH_EXPECTED: usize = 3;
    const ROW_WIDTH_OBSERVED: usize = 5;

    closed_table! {
        expected_typst / typst_rows: TypstRefusal,
        "UNKNOWN_RENDERER_VERSION" | TypstRefusal::UnknownRendererVersion(_)
            => "UnknownRendererVersion",
            TypstRefusal::UnknownRendererVersion(payload_a()), &[NEEDLE_A];
        "UNCLOSED_FRONT_MATTER" | TypstRefusal::UnclosedFrontMatter
            => "UnclosedFrontMatter", TypstRefusal::UnclosedFrontMatter, &[];
        "FRONT_MATTER_NOT_KEY_VALUE" | TypstRefusal::FrontMatterNotKeyValue(_)
            => "FrontMatterNotKeyValue",
            TypstRefusal::FrontMatterNotKeyValue(payload_a()), &[NEEDLE_A];
        "UNCLOSED_CODE_FENCE" | TypstRefusal::UnclosedCodeFence
            => "UnclosedCodeFence", TypstRefusal::UnclosedCodeFence, &[];
        "HEADING_TOO_DEEP" | TypstRefusal::HeadingTooDeep(_)
            => "HeadingTooDeep", TypstRefusal::HeadingTooDeep(payload_a()), &[NEEDLE_A];
        "UNESCAPED_CONSTRUCT" | TypstRefusal::UnescapedConstruct { .. }
            => "UnescapedConstruct",
            TypstRefusal::UnescapedConstruct { character: '#', line: payload_a() }, &[NEEDLE_A];
        "TABLE_ROW_WIDTH" | TypstRefusal::TableRowWidth { expected, observed }
            => format!("TableRowWidth(expected={expected}, observed={observed})"),
            TypstRefusal::TableRowWidth {
                expected: ROW_WIDTH_EXPECTED,
                observed: ROW_WIDTH_OBSERVED,
            },
            &[];
    }

    closed_table! {
        expected_pdf / pdf_rows: PdfRefusal,
        "R42S1.PDF.NOT_UTF8" | PdfRefusal::NotUtf8
            => "the exported document is not UTF-8", PdfRefusal::NotUtf8, &[];
        "R42S1.PDF.TRANSLATE" | PdfRefusal::Translate(refusal)
            => format!("Translate.{}", expected_typst(refusal).1),
            PdfRefusal::Translate(TypstRefusal::UnknownRendererVersion(payload_a())),
            &[NEEDLE_A];
        "R42S1.PDF.EMBEDDED_FONT_SET_EMPTY" | PdfRefusal::EmbeddedFontSetEmpty
            => "the render profile embeds no font, and this renderer never looks for one on the \
                host",
            PdfRefusal::EmbeddedFontSetEmpty, &[];
        "R42S1.PDF.COMPILE" | PdfRefusal::Compile(_)
            => "Compile", PdfRefusal::Compile(payload_a()), &[NEEDLE_A];
        "R42S1.PDF.SERIALIZE" | PdfRefusal::Serialize(_)
            => "Serialize", PdfRefusal::Serialize(payload_b()), &[NEEDLE_B];
    }

    struct Case {
        id: String,
        value: PdfRefusal,
        markers: &'static [&'static str],
    }

    /// The needle-bearing cases lead, so a mutant's FIRST failing label is predictable.
    const LEAD_CASES: [&str; 3] = [
        "R42S1.PDF.TRANSLATE.UNKNOWN_RENDERER_VERSION",
        "R42S1.PDF.COMPILE",
        "R42S1.PDF.SERIALIZE",
    ];

    fn pdf_cases() -> Vec<Case> {
        let mut cases = Vec::new();
        for (stem, value, markers) in pdf_rows() {
            if stem == "R42S1.PDF.TRANSLATE" {
                for (child, refusal, child_markers) in typst_rows() {
                    cases.push(Case {
                        id: format!("{stem}.{child}"),
                        value: PdfRefusal::Translate(refusal),
                        markers: child_markers,
                    });
                }
            } else {
                cases.push(Case {
                    id: stem.to_owned(),
                    value,
                    markers,
                });
            }
        }
        cases.sort_by_key(|case| {
            LEAD_CASES
                .iter()
                .position(|lead| *lead == case.id)
                .unwrap_or(usize::MAX)
        });
        cases
    }

    /// R42 slice 1: every `PdfRefusal` line is the closed projection, and every translator
    /// diagnostic it was built from is still in `Debug`.
    ///
    /// The three census sites are `Translate` (which forwarded the translator's whole diagnostic,
    /// including the document line and the renderer version the document named), `Compile` and
    /// `Serialize` (which forwarded Typst's compiler text). `NotUtf8` and `EmbeddedFontSetEmpty`
    /// were already closed and are pinned here as regressions.
    #[test]
    fn r42_pdf_projection_is_closed() {
        let cases = pdf_cases();

        assert_eq!(typst_rows().len(), 7, "R42S1.PDF.CARDINALITY.TRANSLATOR");
        assert_eq!(pdf_rows().len(), 5, "R42S1.PDF.CARDINALITY.ROOT");
        assert_eq!(cases.len(), 11, "R42S1.PDF.CARDINALITY.CASES");
        let ids: std::collections::BTreeSet<&str> =
            cases.iter().map(|case| case.id.as_str()).collect();
        assert_eq!(ids.len(), cases.len(), "R42S1.PDF.CASE_IDS_UNIQUE");
        for lead in LEAD_CASES {
            assert!(ids.contains(lead), "R42S1.PDF.LEAD_CASE_PRESENT: {lead}");
        }

        for case in &cases {
            let Case { id, value, markers } = case;
            let (stem, expected) = expected_pdf(value);
            assert!(
                id == stem || id.starts_with(&format!("{stem}.")),
                "R42S1.PDF.PATTERN_ALIGNS: {id} was built as {stem}"
            );

            let line = value.to_string();
            for marker in *markers {
                assert!(!line.contains(marker), "{id}.NO_NEEDLE: {line}");
            }
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "{id}.ONE_LINE: {line:?}"
            );

            let debug = format!("{value:?}");
            for marker in *markers {
                assert!(debug.contains(marker), "{id}.DEBUG_RETAINS: {debug}");
            }

            assert_eq!(line, expected, "{id}.PROJECTION");
        }

        // Approved context: the two row-width counts are deliberately IN the line, and they are
        // different from each other so a swapped pair cannot pass.
        let widths = PdfRefusal::Translate(TypstRefusal::TableRowWidth {
            expected: ROW_WIDTH_EXPECTED,
            observed: ROW_WIDTH_OBSERVED,
        });
        assert_eq!(
            widths.to_string(),
            format!(
                "Translate.TableRowWidth(expected={ROW_WIDTH_EXPECTED}, \
                 observed={ROW_WIDTH_OBSERVED})"
            ),
            "R42S1.PDF.TRANSLATE.TABLE_ROW_WIDTH.CONTEXT"
        );
        let second_widths = PdfRefusal::Translate(TypstRefusal::TableRowWidth {
            expected: 9,
            observed: 2,
        });
        assert_eq!(
            second_widths.to_string(),
            "Translate.TableRowWidth(expected=9, observed=2)",
            "R42S1.PDF.TRANSLATE.TABLE_ROW_WIDTH.SECOND_SEED"
        );

        // A second payload proves the projection is independent of the text it was handed.
        let second = PdfRefusal::Compile(payload_b());
        assert_eq!(
            second.to_string(),
            "Compile",
            "R42S1.PDF.COMPILE.SECOND_SEED"
        );
        assert!(
            format!("{second:?}").contains(NEEDLE_B),
            "R42S1.PDF.COMPILE.SECOND_SEED_DEBUG_RETAINS"
        );
    }
}
