//! Small helpers over `roxmltree` for the shapes CDD files use.

use roxmltree::Node;

/// Element children only.
pub fn kids<'a, 'i>(n: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(Node::is_element)
}

pub fn kid<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    kids(n).find(|c| c.tag_name().name() == tag)
}

pub fn kids_named<'a, 'i, 't>(
    n: Node<'a, 'i>,
    tag: &'t str,
) -> impl Iterator<Item = Node<'a, 'i>> + 't
where
    'a: 't,
    'i: 't,
{
    kids(n).filter(move |c| c.tag_name().name() == tag)
}

pub fn tag<'a>(n: Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

/// All text below the node, joined and trimmed (rich text keeps its words).
pub fn text_of(n: Node<'_, '_>) -> String {
    let mut s = String::new();
    for d in n.descendants().filter(Node::is_text) {
        s.push_str(d.text().unwrap_or(""));
    }
    s.trim().to_string()
}

/// The `QUAL` child: the identifier CANdela generates names from.
pub fn qual(n: Node<'_, '_>) -> Option<String> {
    kid(n, "QUAL").map(text_of).filter(|s| !s.is_empty())
}

/// Texts by language, in document order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalText(pub Vec<(String, String)>);

impl LocalText {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text of the first language in `preferred` that exists, else the first text.
    pub fn pick<'s>(&'s self, preferred: &[String]) -> Option<&'s str> {
        for lang in preferred {
            if let Some((_, t)) = self.0.iter().find(|(l, _)| l.eq_ignore_ascii_case(lang)) {
                return Some(t);
            }
        }
        self.0.first().map(|(_, t)| t.as_str())
    }
}

fn lang_of(tuv: Node<'_, '_>) -> String {
    tuv.attribute((roxmltree::NS_XML_URI, "lang"))
        .or_else(|| tuv.attribute("lang"))
        .or_else(|| tuv.attribute("xml:lang"))
        .unwrap_or("")
        .to_string()
}

/// `<NAME><TUV xml:lang=..>text</TUV>..</NAME>` (or `DESC`, `TEXT`).
pub fn local_text(n: Node<'_, '_>, tag_name: &str) -> LocalText {
    let Some(holder) = kid(n, tag_name) else {
        return LocalText::default();
    };
    LocalText(
        kids_named(holder, "TUV")
            .map(|t| (lang_of(t), text_of(t)))
            .collect(),
    )
}

/// A TUV list directly below `holder` (for `TEXT` inside `TEXTMAP`).
pub fn tuv_list(holder: Node<'_, '_>) -> LocalText {
    LocalText(
        kids_named(holder, "TUV")
            .map(|t| (lang_of(t), text_of(t)))
            .collect(),
    )
}

/// Decimal, `0x` hexadecimal or negative integers.
pub fn parse_int(s: &str) -> Option<i128> {
    let s = s.trim();
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let v = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i128::from_str_radix(hex, 16).ok()?
    } else {
        body.parse::<i128>().ok()?
    };
    Some(if neg { -v } else { v })
}

pub fn attr_int(n: Node<'_, '_>, name: &str) -> Option<i128> {
    n.attribute(name).and_then(parse_int)
}

pub fn attr_f64(n: Node<'_, '_>, name: &str) -> Option<f64> {
    n.attribute(name).and_then(|s| s.trim().parse::<f64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_in_all_spellings() {
        assert_eq!(parse_int("129"), Some(129));
        assert_eq!(parse_int(" 0x81 "), Some(129));
        assert_eq!(parse_int("-9"), Some(-9));
        assert_eq!(parse_int("4294967295"), Some(4_294_967_295));
        assert_eq!(parse_int("1.5"), None);
        assert_eq!(parse_int(""), None);
    }

    #[test]
    fn local_text_prefers_the_requested_language() {
        let doc = roxmltree::Document::parse(
            "<X><NAME><TUV xml:lang='en-US'>Door</TUV><TUV xml:lang='de-DE'>Tür</TUV></NAME></X>",
        )
        .unwrap();
        let t = local_text(doc.root_element(), "NAME");
        assert_eq!(t.pick(&["de-DE".to_string()]), Some("Tür"));
        assert_eq!(t.pick(&["fr-FR".to_string()]), Some("Door"));
        assert_eq!(t.pick(&[]), Some("Door"));
    }
}
