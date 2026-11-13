//! UTF-8 decoding for a byte stream that arrives in arbitrary pieces.

/// Decodes a byte stream to text one read at a time.
///
/// A read can end part-way through a multi-byte character. Decoding each read
/// on its own turns both halves into replacement characters, so the unfinished
/// tail of one read is held back and decoded together with the next.
#[derive(Default)]
pub(crate) struct Utf8StreamDecoder {
    /// The start of a character whose remaining bytes have not arrived yet.
    /// At most three bytes.
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    /// Decode the next read. Invalid bytes become U+FFFD, as with
    /// `String::from_utf8_lossy`. The result is empty when the read only
    /// continued a character that is still incomplete.
    pub(crate) fn decode(&mut self, bytes: &[u8]) -> String {
        let joined;
        let mut input = if self.pending.is_empty() {
            bytes
        } else {
            self.pending.extend_from_slice(bytes);
            joined = std::mem::take(&mut self.pending);
            joined.as_slice()
        };

        let mut text = String::with_capacity(input.len());
        loop {
            let error = match std::str::from_utf8(input) {
                Ok(valid) => {
                    text.push_str(valid);
                    break;
                }
                Err(error) => error,
            };
            let (valid, rest) = input.split_at(error.valid_up_to());
            text.push_str(&String::from_utf8_lossy(valid));
            match error.error_len() {
                // Bytes that can never start or continue a character.
                Some(invalid_len) => {
                    text.push(char::REPLACEMENT_CHARACTER);
                    input = &rest[invalid_len..];
                }
                // The input ended inside a character: wait for the rest.
                None => {
                    self.pending = rest.to_vec();
                    break;
                }
            }
        }
        text
    }

    /// Call at end of stream. A character that never completed becomes one
    /// U+FFFD; otherwise the result is empty.
    pub(crate) fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        self.pending.clear();
        char::REPLACEMENT_CHARACTER.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode `bytes` split at every possible position and check that the
    /// result always matches decoding it in one piece.
    fn assert_split_invariant(bytes: &[u8]) {
        let expected = String::from_utf8_lossy(bytes).to_string();
        for split in 0..=bytes.len() {
            let mut decoder = Utf8StreamDecoder::default();
            let mut text = decoder.decode(&bytes[..split]);
            text.push_str(&decoder.decode(&bytes[split..]));
            text.push_str(&decoder.finish());
            assert_eq!(text, expected, "split at byte {split}");
        }
    }

    #[test]
    fn characters_split_across_reads_decode_whole() {
        // Two, three and four byte characters.
        assert_split_invariant("aé€😀z".as_bytes());
    }

    #[test]
    fn a_character_fed_one_byte_at_a_time_decodes_whole() {
        let mut decoder = Utf8StreamDecoder::default();
        let mut text = String::new();
        for byte in "😀".as_bytes() {
            text.push_str(&decoder.decode(std::slice::from_ref(byte)));
        }
        assert_eq!(text, "😀");
        assert_eq!(decoder.finish(), "");
    }

    #[test]
    fn invalid_bytes_match_lossy_decoding_at_any_split() {
        // A stray continuation byte, an overlong lead byte, and a three-byte
        // character cut short by ASCII.
        assert_split_invariant(b"ok\x80mid\xc0\xafx\xe2\x82yz");
    }

    #[test]
    fn a_character_cut_off_by_end_of_stream_becomes_one_replacement() {
        let mut decoder = Utf8StreamDecoder::default();
        assert_eq!(decoder.decode(b"tail\xe2\x82"), "tail");
        assert_eq!(decoder.finish(), "\u{fffd}");
        assert_eq!(decoder.finish(), "");
    }
}
