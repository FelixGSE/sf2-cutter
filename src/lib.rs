//! Cut `SoundFont` 2 (`.sf2`) files down to a chosen subset of presets.
//!
//! The library is organised as a pipeline: [`parse`] turns bytes into a
//! [`model::SoundFont`], [`select`] decides which presets to keep, [`extract`]
//! walks preset → instrument → sample references and rebuilds a minimal model,
//! [`validate`] checks integrity, and [`write`] turns a model back into bytes.
//!
//! Key invariant: `write(parse(x))` round-trips a well-formed file, and
//! extraction output always passes [`validate::validate`].

pub mod builder;
pub mod edit;
pub mod error;
pub mod extract;
pub mod merge;
pub mod model;
pub mod parse;
pub mod riff;
pub mod select;
pub mod validate;
pub mod write;

pub use error::Error;
