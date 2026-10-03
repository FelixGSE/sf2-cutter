//! Minimal RIFF reader/writer: sequential chunks, `LIST`/`RIFF` containers,
//! little-endian 32-bit sizes, and the even-byte padding rule.

use crate::error::Error;

/// Four-character chunk identifier.
pub type FourCc = [u8; 4];

/// The outer `RIFF` container id.
pub const RIFF: FourCc = *b"RIFF";
/// The `LIST` container id.
pub const LIST: FourCc = *b"LIST";

/// A chunk borrowed from a byte buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk<'a> {
    /// Four-character chunk id.
    pub id: FourCc,
    /// Chunk payload (without header or pad byte).
    pub data: &'a [u8],
}

/// Renders a four-character code for display in messages.
#[must_use]
pub fn fourcc(id: FourCc) -> String {
    String::from_utf8_lossy(&id).into_owned()
}

/// Splits a buffer into the sequence of chunks it contains.
///
/// # Errors
///
/// Returns [`Error::Truncated`] when a chunk header or payload extends past
/// the end of the buffer.
pub fn chunks(data: &[u8]) -> Result<Vec<Chunk<'_>>, Error> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let header = data
            .get(pos..pos + 8)
            .ok_or(Error::Truncated("chunk header"))?;
        let id: FourCc = [header[0], header[1], header[2], header[3]];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let payload = data
            .get(pos + 8..pos + 8 + size)
            .ok_or(Error::Truncated("chunk payload"))?;
        out.push(Chunk { id, data: payload });
        pos += 8 + size + (size & 1);
    }
    Ok(out)
}

/// Reads the single outer `RIFF` chunk and returns its form type and body.
///
/// # Errors
///
/// Returns [`Error::InvalidRiff`] when the buffer does not consist of exactly
/// one `RIFF` chunk with a form type, or [`Error::Truncated`] when it ends
/// prematurely.
pub fn root(bytes: &[u8]) -> Result<(FourCc, &[u8]), Error> {
    let top = chunks(bytes)?;
    match top.as_slice() {
        [chunk] if chunk.id == RIFF => split_form(chunk.data),
        [chunk] => Err(Error::InvalidRiff(format!(
            "outer chunk is `{}`, expected `RIFF`",
            fourcc(chunk.id)
        ))),
        other => Err(Error::InvalidRiff(format!(
            "expected exactly one outer chunk, found {}",
            other.len()
        ))),
    }
}

/// Interprets a chunk as a `LIST` container, returning its form type and body.
///
/// # Errors
///
/// Returns [`Error::InvalidRiff`] when the chunk is not a `LIST`, or
/// [`Error::Truncated`] when its payload is too short to hold a form type.
pub fn list_body<'a>(chunk: &Chunk<'a>) -> Result<(FourCc, &'a [u8]), Error> {
    if chunk.id != LIST {
        return Err(Error::InvalidRiff(format!(
            "chunk `{}` is not a LIST",
            fourcc(chunk.id)
        )));
    }
    split_form(chunk.data)
}

fn split_form(data: &[u8]) -> Result<(FourCc, &[u8]), Error> {
    let form = data.get(..4).ok_or(Error::Truncated("form type"))?;
    Ok(([form[0], form[1], form[2], form[3]], &data[4..]))
}

/// Appends a complete chunk: header, payload, and pad byte when the size is odd.
///
/// # Errors
///
/// Returns [`Error::TooLarge`] when the payload exceeds a 32-bit size field.
pub fn push_chunk(out: &mut Vec<u8>, id: FourCc, data: &[u8]) -> Result<(), Error> {
    let size = u32::try_from(data.len()).map_err(|_| Error::TooLarge("chunk"))?;
    out.extend_from_slice(&id);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    Ok(())
}

/// Begins a container chunk (`RIFF` or `LIST`) with the given form type.
///
/// Returns the position of the size field, to be patched by
/// [`end_container`] once the body is complete.
pub fn begin_container(out: &mut Vec<u8>, id: FourCc, form: FourCc) -> usize {
    out.extend_from_slice(&id);
    let size_at = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&form);
    size_at
}

/// Patches the size field of a container begun at `size_at`.
///
/// # Errors
///
/// Returns [`Error::TooLarge`] when the container body exceeds a 32-bit size.
pub fn end_container(out: &mut [u8], size_at: usize) -> Result<(), Error> {
    let size = u32::try_from(out.len() - size_at - 4).map_err(|_| Error::TooLarge("container"))?;
    out[size_at..size_at + 4].copy_from_slice(&size.to_le_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_bytes(id: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        push_chunk(&mut out, id, payload).unwrap();
        out
    }

    #[test]
    fn chunks_should_return_all_chunks_when_buffer_is_well_formed() {
        // given
        let mut data = chunk_bytes(*b"aaaa", &[1, 2]);
        data.extend_from_slice(&chunk_bytes(*b"bbbb", &[3, 4, 5, 6]));

        // when
        let parsed = chunks(&data).unwrap();

        // then
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, *b"aaaa");
        assert_eq!(parsed[0].data, &[1, 2]);
        assert_eq!(parsed[1].id, *b"bbbb");
        assert_eq!(parsed[1].data, &[3, 4, 5, 6]);
    }

    #[test]
    fn chunks_should_skip_pad_byte_when_chunk_size_is_odd() {
        // given
        let mut data = chunk_bytes(*b"oddc", &[9, 9, 9]);
        data.extend_from_slice(&chunk_bytes(*b"next", &[1]));

        // when
        let parsed = chunks(&data).unwrap();

        // then
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].data, &[9, 9, 9]);
        assert_eq!(parsed[1].id, *b"next");
    }

    #[test]
    fn chunks_should_reject_buffer_when_payload_is_truncated() {
        // given
        let mut data = Vec::new();
        data.extend_from_slice(b"aaaa");
        data.extend_from_slice(&100u32.to_le_bytes());
        data.extend_from_slice(&[0; 10]);

        // when
        let result = chunks(&data);

        // then
        assert!(matches!(result, Err(Error::Truncated("chunk payload"))));
    }

    #[test]
    fn chunks_should_reject_buffer_when_header_is_truncated() {
        // given
        let data = [0u8; 5];

        // when
        let result = chunks(&data);

        // then
        assert!(matches!(result, Err(Error::Truncated("chunk header"))));
    }

    #[test]
    fn root_should_return_form_and_body_when_outer_chunk_is_riff() {
        // given
        let mut payload = b"sfbk".to_vec();
        payload.extend_from_slice(&[7, 7]);
        let data = chunk_bytes(*b"RIFF", &payload);

        // when
        let (form, body) = root(&data).unwrap();

        // then
        assert_eq!(form, *b"sfbk");
        assert_eq!(body, &[7, 7]);
    }

    #[test]
    fn root_should_reject_buffer_when_outer_chunk_is_not_riff() {
        // given
        let data = chunk_bytes(*b"LIST", b"sfbk");

        // when
        let result = root(&data);

        // then
        assert!(matches!(result, Err(Error::InvalidRiff(_))));
    }

    #[test]
    fn root_should_reject_buffer_when_multiple_outer_chunks_exist() {
        // given
        let mut data = chunk_bytes(*b"RIFF", b"sfbk");
        data.extend_from_slice(&chunk_bytes(*b"RIFF", b"sfbk"));

        // when
        let result = root(&data);

        // then
        assert!(matches!(result, Err(Error::InvalidRiff(_))));
    }

    #[test]
    fn list_body_should_return_form_and_body_when_chunk_is_list() {
        // given
        let mut payload = b"INFO".to_vec();
        payload.extend_from_slice(&[1]);
        let chunk = Chunk {
            id: LIST,
            data: &payload,
        };

        // when
        let (form, body) = list_body(&chunk).unwrap();

        // then
        assert_eq!(form, *b"INFO");
        assert_eq!(body, &[1]);
    }

    #[test]
    fn list_body_should_reject_chunk_when_id_is_not_list() {
        // given
        let chunk = Chunk {
            id: *b"smpl",
            data: &[],
        };

        // when
        let result = list_body(&chunk);

        // then
        assert!(matches!(result, Err(Error::InvalidRiff(_))));
    }

    #[test]
    fn fourcc_should_render_ascii_when_id_is_printable() {
        // given / when / then
        assert_eq!(fourcc(*b"LIST"), "LIST");
    }

    #[test]
    fn containers_should_round_trip_when_nested() {
        // given
        let mut out = Vec::new();

        // when
        let riff_at = begin_container(&mut out, RIFF, *b"sfbk");
        let list_at = begin_container(&mut out, LIST, *b"INFO");
        push_chunk(&mut out, *b"INAM", b"hi\0\0").unwrap();
        end_container(&mut out, list_at).unwrap();
        end_container(&mut out, riff_at).unwrap();

        // then
        let (form, body) = root(&out).unwrap();
        assert_eq!(form, *b"sfbk");
        let inner = chunks(body).unwrap();
        let (list_form, list_data) = list_body(&inner[0]).unwrap();
        assert_eq!(list_form, *b"INFO");
        assert_eq!(chunks(list_data).unwrap()[0].data, b"hi\0\0");
    }
}
