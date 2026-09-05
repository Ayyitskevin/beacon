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

impl fmt::Display for PdfRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 => formatter.write_str("the exported document is not UTF-8"),
            Self::Translate(refusal) => write!(formatter, "{refusal}"),
            Self::EmbeddedFontSetEmpty => formatter.write_str(
                "the render profile embeds no font, and this renderer never looks for one on the \
                 host",
            ),
            Self::Compile(detail) => write!(formatter, "the document did not compile: {detail}"),
            Self::Serialize(detail) => {
                write!(formatter, "the document did not serialize: {detail}")
            }
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
