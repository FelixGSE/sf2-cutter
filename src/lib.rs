//! Cut `SoundFont` 2 (`.sf2`) files down to a chosen subset of presets.
//!
//! The library is organised as a pipeline: [`parse`] turns bytes into a
//! [`SoundFont`], [`select`] decides which presets to keep, [`extract`]
//! walks preset → instrument → sample references and rebuilds a minimal model,
//! [`validate`] checks integrity, and [`mod@write`] turns a model
//! back into bytes.
//!
//! Key invariant: `write(parse(x))` round-trips a well-formed file, and
//! extraction output always passes [`validate::validate`].
//!
//! # Example
//!
//! ```
//! use sf2_cutter::builder::{SoundFontBuilder, instrument_zone, sample_zone};
//! use sf2_cutter::extract::{Options, extract};
//! use sf2_cutter::select::Selection;
//! use sf2_cutter::SoundFont;
//!
//! # fn main() -> Result<(), sf2_cutter::Error> {
//! // Assemble a one-preset font (normally you would read a file instead).
//! let mut builder = SoundFontBuilder::new("Demo");
//! let sample = builder.add_sample("sine", &[0i16; 64], 44_100, 60)?;
//! let instrument = builder.add_instrument("Inst", vec![sample_zone(sample)])?;
//! builder.add_preset("Demo Piano", 0, 0, vec![instrument_zone(instrument)]);
//! let bytes = builder.build().to_bytes()?;
//!
//! // Parse it back and extract everything matching "piano".
//! let font = SoundFont::from_bytes(&bytes)?;
//! let mut selection = Selection::new();
//! selection.add_pattern("piano");
//! let cut = extract(&font, &selection, &Options::default())?;
//! assert_eq!(cut.presets.len(), 1);
//! # Ok(()) }
//! ```
#![forbid(unsafe_code)]

pub mod builder;
#[cfg(feature = "sf3")]
pub mod convert;
pub mod edit;
pub mod error;
pub mod export;
pub mod extract;
pub mod merge;
pub mod model;
pub mod parse;
pub mod riff;
pub mod select;
#[cfg(feature = "sf3")]
mod sf3;
pub mod validate;
pub mod write;

pub use error::Error;
pub use model::SoundFont;
pub use select::Selection;

impl SoundFont {
    /// Parses a complete `.sf2` file; convenience for [`parse::parse`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when the bytes are not a structurally valid
    /// `SoundFont` 2 file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        parse::parse(bytes)
    }

    /// Serialises the font; convenience for [`write::write`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] when record counts exceed 16-bit SF2 indices or a
    /// chunk exceeds a 32-bit RIFF size field.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        write::write(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundfont_should_round_trip_when_using_convenience_methods() {
        // given
        let font = builder::test_font();

        // when
        let bytes = font.to_bytes().unwrap();
        let reparsed = SoundFont::from_bytes(&bytes).unwrap();

        // then
        assert_eq!(reparsed, font);
    }
}
