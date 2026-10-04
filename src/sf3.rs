//! SF3 (MuseScore-style compressed `SoundFont`) support: decodes the
//! Ogg-Vorbis sample streams to PCM so extraction can emit plain sf2 output.
//! Compiled only with the `sf3` cargo feature.

use crate::error::Error;
use crate::model::{SampleHeader, SoundFont};

/// Decodes one compressed sample of `font` to PCM. Its `start`/`end` are
/// BYTE offsets of the Ogg stream in `smpl` (the SF3 convention); `clamp`
/// clips them to the available data instead of failing (salvage mode).
///
/// # Errors
///
/// Returns [`Error::IndexOutOfBounds`] for out-of-range offsets (unless
/// clamping) and [`Error::Sf3Decode`] for undecodable streams.
pub(crate) fn decode_sample(
    font: &SoundFont,
    sample: &SampleHeader,
    clamp: bool,
) -> Result<Vec<i16>, Error> {
    let total = font.sample_data.len();
    let (start, end) = if clamp {
        let start = (sample.start as usize).min(total);
        (start, (sample.end as usize).clamp(start, total))
    } else {
        (sample.start as usize, sample.end as usize)
    };
    let stream = font
        .sample_data
        .get(start..end)
        .ok_or(Error::IndexOutOfBounds {
            what: "compressed sample data range",
            index: end,
            max: total,
        })?;
    decode_ogg(&sample.name.to_display(), stream)
}

/// Decodes an Ogg-Vorbis stream to 16-bit PCM. Multi-channel streams yield
/// their first channel (SF3 stores stereo as two linked mono streams, so
/// anything else is defensive).
pub fn decode_ogg(name: &str, bytes: &[u8]) -> Result<Vec<i16>, Error> {
    let fail = |detail: String| Error::Sf3Decode {
        name: name.to_string(),
        detail,
    };
    let cursor = std::io::Cursor::new(bytes);
    let mut reader =
        lewton::inside_ogg::OggStreamReader::new(cursor).map_err(|e| fail(e.to_string()))?;
    let channels = usize::from(reader.ident_hdr.audio_channels);
    if channels == 0 {
        return Err(fail("stream declares zero channels".into()));
    }
    let mut pcm = Vec::new();
    while let Some(packet) = reader
        .read_dec_packet_itl()
        .map_err(|e| fail(e.to_string()))?
    {
        if channels == 1 {
            pcm.extend_from_slice(&packet);
        } else {
            pcm.extend(packet.iter().step_by(channels).copied());
        }
    }
    // The final Vorbis block may carry padding past the stream's true length;
    // the last page's granule position is the authoritative sample count.
    if let Some(total) = reader.get_last_absgp() {
        // truncate is a no-op when total >= len, so no guard is needed
        pcm.truncate(usize::try_from(total).unwrap_or(usize::MAX));
    }
    Ok(pcm)
}

/// Encodes mono 16-bit PCM to an Ogg-Vorbis stream (quality 0.0..=1.0).
#[cfg(feature = "sf3-write")]
pub(crate) fn encode_ogg(
    name: &str,
    pcm: &[i16],
    sample_rate: u32,
    quality: f32,
) -> Result<Vec<u8>, Error> {
    let fail = |detail: String| Error::Sf3Encode {
        name: name.to_string(),
        detail,
    };
    // aoTuV clamps out-of-range quality silently; reject it instead.
    if !(0.0..=1.0).contains(&quality) {
        return Err(fail(format!("quality {quality} outside 0.0..=1.0")));
    }
    let rate =
        std::num::NonZeroU32::new(sample_rate).ok_or_else(|| fail("sample rate is zero".into()))?;
    let mut out = Vec::new();
    let mut builder =
        vorbis_rs::VorbisEncoderBuilder::new(rate, std::num::NonZeroU8::MIN, &mut out)
            .map_err(|e| fail(e.to_string()))?;
    builder.bitrate_management_strategy(vorbis_rs::VorbisBitrateManagementStrategy::QualityVbr {
        target_quality: quality,
    });
    let mut encoder = builder.build().map_err(|e| fail(e.to_string()))?;
    let block: Vec<f32> = pcm.iter().map(|&v| f32::from(v) / 32_768.0).collect();
    encoder
        .encode_audio_block([&block])
        .map_err(|e| fail(e.to_string()))?;
    encoder.finish().map_err(|e| fail(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "sf3-write")]
    #[test]
    fn encode_then_decode_should_preserve_sample_count_when_stream_is_valid() {
        // given: a second of a quiet 440-ish tone
        #[allow(clippy::cast_possible_truncation)] // sine stays within +-8000
        let pcm: Vec<i16> = (0..8000i32)
            .map(|i| ((f64::from(i) * 0.3).sin() * 8000.0) as i16)
            .collect();

        // when
        let stream = encode_ogg("tone", &pcm, 8000, 0.5).unwrap();
        let decoded = decode_ogg("tone", &stream).unwrap();

        // then: lossy but sample-exact in length, and clearly not silence
        assert_eq!(decoded.len(), pcm.len());
        assert!(decoded.iter().any(|&v| v.abs() > 1000));
        assert!(stream.len() < pcm.len() * 2);
    }

    #[cfg(feature = "sf3-write")]
    #[test]
    fn encode_should_fail_when_quality_is_out_of_range() {
        // given / when
        let result = encode_ogg("bad", &[0; 100], 8000, 9.0);

        // then
        assert!(matches!(result, Err(Error::Sf3Encode { .. })));
    }

    #[test]
    fn decode_should_fail_when_bytes_are_not_ogg() {
        // given
        let garbage = [0x42u8; 64];

        // when
        let result = decode_ogg("junk", &garbage);

        // then
        assert!(matches!(result, Err(Error::Sf3Decode { .. })));
    }

    #[test]
    fn decode_should_fail_when_stream_is_truncated() {
        // given: a valid Ogg capture pattern followed by nothing useful
        let mut bytes = b"OggS".to_vec();
        bytes.extend_from_slice(&[0; 30]);

        // when
        let result = decode_ogg("truncated", &bytes);

        // then
        assert!(matches!(result, Err(Error::Sf3Decode { .. })));
    }
}
