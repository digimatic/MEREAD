//! Percent-encoding for the small part of it that matters here: file names with spaces and other
//! url-significant characters have to survive the round trip from a generated listing link back
//! into a path below the root.

/// Percent-encode the characters that would otherwise change the meaning of a url path segment.
/// Non-ascii is left alone; browsers send it back as-is and it round-trips fine.
pub(crate) fn encode_path_segment(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '%' => encoded.push_str("%25"),
            ' ' => encoded.push_str("%20"),
            '#' => encoded.push_str("%23"),
            '?' => encoded.push_str("%3F"),
            _ => encoded.push(character),
        }
    }
    encoded
}

/// Decode `%XX` escapes in a request path. Invalid escapes are left as written, so a file actually
/// named `100%` still resolves.
pub(crate) fn decode_percent(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(high) = hex_value(bytes[i + 1])
            && let Some(low) = hex_value(bytes[i + 2])
        {
            decoded.push(high * 16 + low);
            i += 3;
            continue;
        }

        decoded.push(bytes[i]);
        i += 1;
    }

    String::from_utf8(decoded).unwrap_or_else(|_| path.to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_only_url_significant_characters() {
        assert_eq!(encode_path_segment("ideas.md"), "ideas.md");
        assert_eq!(encode_path_segment("My Notes.md"), "My%20Notes.md");
        assert_eq!(encode_path_segment("a#b?c%d"), "a%23b%3Fc%25d");
        assert_eq!(encode_path_segment("räksmörgås.md"), "räksmörgås.md");
    }

    #[test]
    fn decodes_escapes() {
        assert_eq!(decode_percent("My%20Notes.md"), "My Notes.md");
        assert_eq!(decode_percent("a%23b%3fc"), "a#b?c");
        assert_eq!(decode_percent("plain/path.md"), "plain/path.md");
    }

    #[test]
    fn leaves_invalid_escapes_alone() {
        assert_eq!(decode_percent("100%"), "100%");
        assert_eq!(decode_percent("100%25"), "100%");
        assert_eq!(decode_percent("%zz"), "%zz");
        assert_eq!(decode_percent("%2"), "%2");
    }

    #[test]
    fn round_trips() {
        for name in ["ideas.md", "My Notes.md", "a#b?c%d", "räksmörgås.md"] {
            assert_eq!(decode_percent(&encode_path_segment(name)), name);
        }
    }
}
