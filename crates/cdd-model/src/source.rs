//! Turns the bytes of a CDD file into text. The encoding is read from the XML declaration;
//! decoding never guesses and never replaces a bad byte.

use crate::error::Error;

/// Text of a document and the encoding it was decoded from.
#[derive(Clone, Debug)]
pub struct DecodedSource {
    pub text: String,
    /// The label of the encoding, for the report (`utf-8`, `iso-8859-1`, ...).
    pub encoding: String,
}

/// The `encoding` of the XML declaration, when the file starts with one.
fn declared_encoding(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(256)];
    // The declaration is ASCII in every encoding this tool accepts.
    let head: String = head
        .iter()
        .map(|b| if b.is_ascii() { *b as char } else { '?' })
        .collect();
    let decl_end = head.find("?>")?;
    let decl = &head[..decl_end];
    if !decl.starts_with("<?xml") {
        return None;
    }
    let at = decl.find("encoding")?;
    let rest = decl[at + "encoding".len()..]
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let quote = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let rest = &rest[1..];
    Some(rest[..rest.find(quote)?].to_string())
}

pub fn decode(bytes: &[u8]) -> Result<DecodedSource, Error> {
    // A byte order mark decides first.
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let text = encoding
            .decode_without_bom_handling_and_without_replacement(&bytes[bom_len..])
            .ok_or_else(|| {
                Error::Encoding(format!(
                    "invalid {} bytes after the byte order mark",
                    encoding.name()
                ))
            })?;
        return Ok(DecodedSource {
            text: text.into_owned(),
            encoding: encoding.name().to_ascii_lowercase(),
        });
    }
    let label = declared_encoding(bytes).unwrap_or_else(|| "utf-8".to_string());
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| {
        Error::Encoding(format!("unknown encoding `{label}` in the XML declaration"))
    })?;
    let text = encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .ok_or_else(|| {
            Error::Encoding(format!(
                "the file declares `{label}` but contains bytes that are not valid in it"
            ))
        })?;
    Ok(DecodedSource {
        text: text.into_owned(),
        encoding: label.to_ascii_lowercase(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_latin1_is_decoded() {
        let bytes = b"<?xml version='1.0' encoding='iso-8859-1'?><a>f\xFCr</a>";
        let d = decode(bytes).unwrap();
        assert!(d.text.contains("für"));
        assert_eq!(d.encoding, "iso-8859-1");
    }

    #[test]
    fn utf8_is_the_default_and_bad_bytes_are_an_error() {
        assert_eq!(decode(b"<a>x</a>").unwrap().encoding, "utf-8");
        let bad = b"<?xml version='1.0' encoding='utf-8'?><a>\xFF</a>";
        assert!(matches!(decode(bad), Err(Error::Encoding(_))));
    }

    #[test]
    fn unknown_encoding_is_reported() {
        let bytes = b"<?xml version='1.0' encoding='klingon'?><a/>";
        assert!(matches!(decode(bytes), Err(Error::Encoding(_))));
    }

    #[test]
    fn utf8_bom_is_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"<a/>");
        assert_eq!(decode(&bytes).unwrap().text, "<a/>");
    }
}
