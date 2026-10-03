//! Crate-wide error type.

use thiserror::Error;

/// Errors produced while parsing, writing, or extracting `SoundFont` data.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The file ends inside a structure that must be complete.
    #[error("truncated file: {0}")]
    Truncated(&'static str),
    /// The byte stream is not structured as a RIFF file.
    #[error("invalid RIFF structure: {0}")]
    InvalidRiff(String),
    /// The outer RIFF form type is not `sfbk`.
    #[error("not a SoundFont file: {0}")]
    NotSoundFont(String),
    /// A required chunk is missing.
    #[error("missing required chunk `{0}`")]
    MissingChunk(&'static str),
    /// A chunk's size is not a multiple of its fixed record size.
    #[error("chunk `{chunk}` has size {size}, not a multiple of {record_size}-byte records")]
    BadRecordSize {
        /// Offending chunk id.
        chunk: &'static str,
        /// Actual chunk size in bytes.
        size: usize,
        /// Expected record size in bytes.
        record_size: usize,
    },
    /// A `pdta` sub-chunk lacks its terminal (`EOP`/`EOI`/`EOS`/zero) record.
    #[error("chunk `{0}` is missing its terminal record")]
    MissingTerminal(&'static str),
    /// Record indices that must be monotonically non-decreasing are not.
    #[error("indices in `{0}` are not monotonically non-decreasing")]
    IndexOrder(&'static str),
    /// An index points past the end of the structure it refers to.
    #[error("{what}: index {index} out of bounds (max {max})")]
    IndexOutOfBounds {
        /// What kind of reference is broken.
        what: &'static str,
        /// The offending index.
        index: usize,
        /// The largest permissible index.
        max: usize,
    },
    /// More records than a 16-bit SF2 index can address.
    #[error("too many {0} for the SF2 format (max 65535)")]
    TooManyRecords(&'static str),
    /// A chunk grew past what a 32-bit RIFF size field can describe.
    #[error("{0} too large for a RIFF chunk (max 4 GiB)")]
    TooLarge(&'static str),
    /// The selection matched no presets.
    #[error("selection matched no presets")]
    EmptySelection,
    /// Renumbering would push a bank's program numbers past the MIDI limit.
    #[error("bank {bank} has more than 128 presets; cannot renumber into 0..=127")]
    RenumberOverflow {
        /// The overflowing bank.
        bank: u16,
    },
    /// A `BANK:PROG` preset spec could not be parsed.
    #[error("invalid preset spec `{0}` (expected BANK:PROG, e.g. 0:4)")]
    InvalidPresetSpec(String),
    /// A recipe (config file) could not be parsed.
    #[error("invalid recipe: {0}")]
    InvalidRecipe(String),
}
