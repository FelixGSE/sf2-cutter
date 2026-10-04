//! Export sample audio from a font as WAV files.

use crate::error::Error;
use crate::model::{SampleHeader, SoundFont};
use crate::riff;

/// A WAV file built from one sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wav {
    /// The complete RIFF/WAVE file.
    pub bytes: Vec<u8>,
    /// Number of audio frames (16-bit mono sample points) it contains; for
    /// SF3 samples this is the decoded length, not the compressed size.
    pub frames: usize,
}

/// Builds a 16-bit mono PCM WAV file of the sample at `index`.
///
/// SF3-compressed samples are decoded first (requires the `sf3` feature).
/// 24-bit `sm24` detail is not included; the WAV is always 16-bit.
///
/// # Errors
///
/// Returns [`Error::RomSample`] for ROM samples (they carry no audio data),
/// [`Error::InvalidSampleRate`] for a declared rate of 0,
/// [`Error::IndexOutOfBounds`] for a bad index or out-of-range sample
/// offsets, and [`Error::Sf3Unsupported`]/[`Error::Sf3Decode`] for
/// compressed samples without the `sf3` feature or with undecodable streams.
pub fn sample_wav(font: &SoundFont, index: usize) -> Result<Wav, Error> {
    let sample = font
        .samples
        .get(index)
        .ok_or_else(|| Error::IndexOutOfBounds {
            what: "sample index",
            index,
            max: font.samples.len().saturating_sub(1),
        })?;
    if sample.is_rom() {
        return Err(Error::RomSample(sample.name.to_display()));
    }
    if sample.sample_rate == 0 {
        return Err(Error::InvalidSampleRate(sample.name.to_display()));
    }
    let pcm = sample_pcm_bytes(font, sample)?;
    Ok(Wav {
        frames: pcm.len() / 2,
        bytes: wav_bytes(&pcm, sample.sample_rate)?,
    })
}

/// The sample's 16-bit little-endian PCM bytes (decoding SF3 streams).
fn sample_pcm_bytes(font: &SoundFont, sample: &SampleHeader) -> Result<Vec<u8>, Error> {
    if sample.is_compressed() {
        return compressed_pcm_bytes(font, sample);
    }
    let start = sample.start as usize;
    let end = sample.end as usize;
    font.sample_data
        .get(start * 2..end * 2)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Error::IndexOutOfBounds {
            what: "sample data range",
            index: end,
            max: font.sample_points(),
        })
}

#[cfg(feature = "sf3")]
fn compressed_pcm_bytes(font: &SoundFont, sample: &SampleHeader) -> Result<Vec<u8>, Error> {
    let pcm = crate::sf3::decode_sample(font, sample, false)?;
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for value in pcm {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(not(feature = "sf3"))]
fn compressed_pcm_bytes(_font: &SoundFont, _sample: &SampleHeader) -> Result<Vec<u8>, Error> {
    Err(Error::Sf3Unsupported)
}

/// Wraps PCM bytes in a canonical RIFF/WAVE container (mono, 16-bit).
fn wav_bytes(pcm: &[u8], sample_rate: u32) -> Result<Vec<u8>, Error> {
    let mut fmt = Vec::with_capacity(16);
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&sample_rate.to_le_bytes());
    fmt.extend_from_slice(&(sample_rate.saturating_mul(2)).to_le_bytes()); // byte rate
    fmt.extend_from_slice(&2u16.to_le_bytes()); // block align
    fmt.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    let mut out = Vec::with_capacity(44 + pcm.len());
    let riff_at = riff::begin_container(&mut out, riff::RIFF, *b"WAVE");
    riff::push_chunk(&mut out, *b"fmt ", &fmt)?;
    riff::push_chunk(&mut out, *b"data", pcm)?;
    riff::end_container(&mut out, riff_at)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::test_font;
    use crate::model::SAMPLE_TYPE_ROM;

    #[test]
    fn sample_wav_should_wrap_pcm_in_valid_wave_when_sample_is_plain() {
        // given: fixture sample 0 is the 200-point piano
        let font = test_font();

        // when
        let wav = sample_wav(&font, 0).unwrap();
        let frames = wav.frames;
        let wav = wav.bytes;

        // then: RIFF/WAVE with a correct fmt chunk and the exact PCM bytes
        let (form, body) = crate::riff::root(&wav).unwrap();
        assert_eq!(form, *b"WAVE");
        let chunks = crate::riff::chunks(body).unwrap();
        assert_eq!(chunks[0].id, *b"fmt ");
        let fmt = chunks[0].data;
        assert_eq!(u16::from_le_bytes([fmt[0], fmt[1]]), 1); // PCM
        assert_eq!(u16::from_le_bytes([fmt[2], fmt[3]]), 1); // mono
        assert_eq!(u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]), 44_100);
        assert_eq!(
            u32::from_le_bytes([fmt[8], fmt[9], fmt[10], fmt[11]]),
            88_200
        ); // byte rate
        assert_eq!(u16::from_le_bytes([fmt[12], fmt[13]]), 2); // block align
        assert_eq!(u16::from_le_bytes([fmt[14], fmt[15]]), 16); // bits
        assert_eq!(chunks[1].id, *b"data");
        assert_eq!(chunks[1].data.len(), 200 * 2);
        assert_eq!(chunks[1].data, &font.sample_data[..400]);
        assert_eq!(frames, 200);
    }

    #[test]
    fn sample_wav_should_fail_when_sample_offsets_exceed_data() {
        // given
        let mut font = test_font();
        font.samples[0].end = u32::MAX / 4;

        // when
        let result = sample_wav(&font, 0);

        // then
        assert!(matches!(
            result,
            Err(Error::IndexOutOfBounds {
                what: "sample data range",
                ..
            })
        ));
    }

    #[test]
    fn sample_wav_should_fail_when_sample_rate_is_zero() {
        // given
        let mut font = test_font();
        font.samples[0].sample_rate = 0;

        // when
        let result = sample_wav(&font, 0);

        // then
        assert!(matches!(result, Err(Error::InvalidSampleRate(_))));
    }

    #[test]
    fn sample_wav_should_slice_correct_region_when_sample_start_is_nonzero() {
        // given: fixture sample 1 (strings-l) sits after the piano sample
        let font = test_font();
        let header = &font.samples[1];
        assert_ne!(header.start, 0);
        let expected = &font.sample_data[header.start as usize * 2..header.end as usize * 2];

        // when
        let wav = sample_wav(&font, 1).unwrap().bytes;

        // then
        let (_, body) = crate::riff::root(&wav).unwrap();
        let chunks = crate::riff::chunks(body).unwrap();
        assert_eq!(chunks[1].data, expected);
    }

    #[test]
    fn sample_wav_should_fail_when_sample_is_in_rom() {
        // given
        let mut font = test_font();
        font.samples[0].sample_type = SAMPLE_TYPE_ROM | 1;

        // when
        let result = sample_wav(&font, 0);

        // then
        assert!(matches!(result, Err(Error::RomSample(_))));
    }

    #[test]
    fn sample_wav_should_fail_when_index_is_out_of_range() {
        // given
        let font = test_font();

        // when
        let result = sample_wav(&font, 99);

        // then
        assert!(matches!(
            result,
            Err(Error::IndexOutOfBounds {
                what: "sample index",
                ..
            })
        ));
    }

    #[cfg(feature = "sf3")]
    #[test]
    fn sample_wav_should_fail_when_compressed_stream_is_garbage() {
        // given: a sample flagged compressed whose bytes are not Ogg
        let mut font = test_font();
        font.samples[0].sample_type |= crate::model::SAMPLE_TYPE_COMPRESSED;
        font.samples[0].start = 0;
        font.samples[0].end = 100;

        // when
        let result = sample_wav(&font, 0);

        // then
        assert!(matches!(result, Err(Error::Sf3Decode { .. })));
    }
}
