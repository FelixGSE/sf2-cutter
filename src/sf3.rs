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
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;

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
