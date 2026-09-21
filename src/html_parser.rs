use std::borrow::Cow;

use log::trace;

macro_rules! letter {
    () => { b'a'..=b'z' | b'A'..=b'Z' };
}

macro_rules! ws {
    () => {
        b' ' | b'\t' | b'\n' | b'\x0C' | b'\r'
    };
}

/// Source and alt text of the first `img` tag of an HTML fragment.
pub fn image_metadata<'a>(html: &'a str) -> (Option<Cow<'a, str>>, Option<Cow<'a, str>>) {
    let html_bytes = html.as_bytes();
    let mut html_iter = html_bytes.iter().enumerate().peekable();

    let mut is_in_img_tag = false;
    let mut src_value: Option<&'a str> = None;
    let mut alt_value: Option<&'a str> = None;

    let mut state = State::Data;
    let mut is_start_tag = false;
    let mut tag_name: &[u8] = b"";
    let mut attr_name: &[u8] = b"";

    while let Some(&(i, &b)) = html_iter.peek() {
        let mut skip_next = false;

        match state {
            State::Data => {
                if b == b'<' {
                    state = State::TagOpen
                }
            }
            State::TagOpen => match b {
                b'/' => state = State::EndTagOpen,
                letter!() => {
                    is_start_tag = true;
                    state = State::TagName { start: i };
                    skip_next = true;
                }
                b'!' => state = State::MarkupDeclarationOpen(0),
                _ => state = State::Data,
            },
            State::EndTagOpen => match b {
                letter!() => {
                    is_start_tag = false;
                    state = State::TagName { start: i };
                    skip_next = true;
                }
                _ => state = State::Data,
            },
            State::TagName { start } => {
                tag_name = &html_bytes[start..i];

                let new_state = match b {
                    ws!() => State::BeforeAttributeName,
                    b'/' => State::SelfClosingStartTag,
                    b'>' => {
                        if is_start_tag && is_raw_text_tag(tag_name) {
                            State::RawText([0; _])
                        } else {
                            State::Data
                        }
                    }
                    _ => State::TagName { start },
                };

                if is_start_tag && new_state != state && tag_name.eq_ignore_ascii_case(b"img") {
                    is_in_img_tag = true;
                }

                if new_state != state {
                    trace!(
                        "{} tag {:?}",
                        if is_start_tag { "start" } else { "end" },
                        str::from_utf8(tag_name)
                    );
                }

                state = new_state;
            }

            State::MarkupDeclarationOpen(comment_mark_count) => match b {
                b'-' if comment_mark_count == 0 => state = State::MarkupDeclarationOpen(1),
                b'-' if comment_mark_count == 1 => state = State::Comment([0; _]),
                // e.g. DOCTYPE
                _ => state = State::MarkupDeclarationContent,
            },
            State::MarkupDeclarationContent => {
                if b == b'>' {
                    state = State::Data
                }
            }
            State::Comment(comment_end_buf) => match b {
                b'>' if comment_end_buf == *b"--" => state = State::Data,
                b => state = State::Comment([comment_end_buf[1], b]),
            },

            State::BeforeAttributeName => match b {
                ws!() => {}
                b'/' | b'>' => {
                    state = State::AfterAttributeName { start_name: None };
                    skip_next = true;
                }
                _ => state = State::AttributeName { start: i },
            },
            State::AttributeName { start } => match b {
                b'=' => {
                    attr_name = html_bytes[start..i].trim_ascii();
                    state = State::BeforeAttributeValue;
                }
                ws!() | b'/' | b'>' => {
                    state = State::AfterAttributeName {
                        start_name: Some(start),
                    };
                    skip_next = true;
                }
                _ => {}
            },
            State::AfterAttributeName { start_name } => match b {
                ws!() => {}
                b'=' => {
                    attr_name = start_name
                        .map(|start| html_bytes[start..i].trim_ascii())
                        .unwrap_or(b"");
                    state = State::BeforeAttributeValue;
                }
                b'/' => state = State::SelfClosingStartTag,
                b'>' => {
                    state = if is_start_tag && is_raw_text_tag(tag_name) {
                        State::RawText([0; _])
                    } else {
                        State::Data
                    }
                }
                _ => state = State::AttributeName { start: i },
            },

            State::BeforeAttributeValue => match b {
                b'\'' => {
                    state = State::AttributeValue {
                        quote_type: QuoteType::Single,
                        start: i + 1,
                    }
                }
                b'"' => {
                    state = State::AttributeValue {
                        quote_type: QuoteType::Double,
                        start: i + 1,
                    }
                }
                _ => {
                    state = State::AttributeValue {
                        quote_type: QuoteType::Unquoted,
                        start: i,
                    };
                    skip_next = true;
                }
            },
            State::AttributeValue { quote_type, start } => {
                let next_state = match b {
                    b'\'' if quote_type == QuoteType::Single => State::BeforeAttributeName,
                    b'"' if quote_type == QuoteType::Double => State::BeforeAttributeName,
                    ws!() if quote_type == QuoteType::Unquoted => State::BeforeAttributeName,
                    b'>' if quote_type == QuoteType::Unquoted => {
                        if is_start_tag && is_raw_text_tag(tag_name) {
                            State::RawText([0; _])
                        } else {
                            State::Data
                        }
                    }
                    _ => State::AttributeValue { quote_type, start },
                };

                if next_state == State::BeforeAttributeName && is_in_img_tag {
                    let attr_value = &html_bytes[start..i];
                    if src_value.is_none() && attr_name.eq_ignore_ascii_case(b"src") {
                        src_value = Some(str::from_utf8(attr_value).unwrap());
                    }
                    if alt_value.is_none() && attr_name.eq_ignore_ascii_case(b"alt") {
                        alt_value = Some(str::from_utf8(attr_value).unwrap());
                    }
                }

                if next_state == State::BeforeAttributeName {
                    let attr_value = &html_bytes[start..i];
                    trace!(
                        "attr {:?} with value {:?}",
                        str::from_utf8(attr_name),
                        str::from_utf8(attr_value)
                    );
                }

                state = next_state;
            }
            State::SelfClosingStartTag => match b {
                b'>' => state = State::Data,
                _ => state = State::BeforeAttributeName,
            },
            State::RawText(raw_end_buf) => match b {
                b'>' if raw_end_buf[8 - (tag_name.len() + 2)..8] == *tag_name => {
                    state = State::Data
                }
                _ => {}
            },
        }

        if is_in_img_tag && matches!(state, State::Data) {
            break;
        }

        if !skip_next {
            html_iter.next();
        }
    }

    (src_value.map(unescape), alt_value.map(unescape))
}

fn is_raw_text_tag(tag_name: &[u8]) -> bool {
    tag_name.eq_ignore_ascii_case(b"script") || tag_name.eq_ignore_ascii_case(b"style")
}

fn unescape<'a>(s: &'a str) -> Cow<'a, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }

    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp + 1..];
        match rest
            .find(';')
            .and_then(|semi| decode_entity(&rest[..semi]).map(|c| (c, semi)))
        {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => out.push('&'),
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity.strip_prefix('#') {
        Some("") => None,
        Some(digits) => {
            let (digits, radix) = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => (hex, 16),
                None => (digits, 10),
            };
            u32::from_str_radix(digits, radix)
                .ok()
                .and_then(char::from_u32)
        }
        None => NAMED_ENTITIES
            .iter()
            .find(|(name, _)| *name == entity)
            .map(|(_, c)| *c),
    }
}

#[derive(Eq, PartialEq)]
enum State {
    Data,
    TagOpen,
    EndTagOpen,
    TagName { start: usize },

    MarkupDeclarationOpen(usize),
    MarkupDeclarationContent,
    Comment([u8; 2]),

    BeforeAttributeName,
    AttributeName { start: usize },
    AfterAttributeName { start_name: Option<usize> },

    BeforeAttributeValue,
    AttributeValue { quote_type: QuoteType, start: usize },

    SelfClosingStartTag,
    RawText([u8; 8]),
}

#[derive(Eq, PartialEq)]
enum QuoteType {
    Unquoted,
    Single,
    Double,
}

const NAMED_ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{a0}'),
];

#[cfg(test)]
mod tests {
    use crate::html_parser::image_metadata;

    #[test]
    fn firefox_fragment() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<meta http-equiv="content-type" content="text/html; charset=utf-8"><img src="https://example.com/image.jpg" alt="example image" width="200">"#;
        assert_eq!(
            image_metadata(html),
            (
                Some("https://example.com/image.jpg".into()),
                Some("example image".into()),
            )
        );
    }

    #[test]
    fn chromium_fragment() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<img src="https://example.com/image.jpg" alt="example image"/>"#;
        assert_eq!(
            image_metadata(html),
            (
                Some("https://example.com/image.jpg".into()),
                Some("example image".into()),
            )
        );
    }

    #[test]
    fn only_source_or_alt() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<img src="example.png"/>"#;
        assert_eq!(image_metadata(html), (Some("example.png".into()), None));

        let html = r#"<img src alt="example image"/>"#;
        assert_eq!(image_metadata(html), (None, Some("example image".into())));
    }

    #[test]
    fn inlined_image() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<img src="data:image/png;base64,iVBORw0KGgo=" alt="inline">"#;
        assert_eq!(
            image_metadata(html),
            (
                Some("data:image/png;base64,iVBORw0KGgo=".into()),
                Some("inline".into())
            )
        );
    }

    #[test]
    fn quoted_and_bare_attribute_values() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<img ALT=a&#32;b SRC='example.png' title="not the alt">"#;
        assert_eq!(
            image_metadata(html),
            (Some("example.png".into()), Some("a b".into()))
        );
    }

    #[test]
    fn decode_amp_encoding() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<img src="example.png?x=1&amp;y=2" alt="a &#x26; b &lt;3&gt; &#65;&nbsp;">"#;
        assert_eq!(
            image_metadata(html),
            (
                Some("example.png?x=1&y=2".into()),
                Some("a & b <3> A\u{a0}".into()),
            )
        );

        let html = r#"<img alt="&unknown; &amp;&nbsp;&gt; &ampx;">"#;
        assert_eq!(
            image_metadata(html),
            (None, Some("&unknown; &\u{a0}> &ampx;".into())),
        );
    }

    #[test]
    fn only_the_first_image_is_read() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<img src="a.png" alt="a"><img src="b.png" alt="b">"#;
        assert_eq!(
            image_metadata(html),
            (Some("a.png".into()), Some("a".into()))
        );

        let html = r#"<p><img src="a.png" alt="a"></p><img src="b.png" alt="b">"#;
        assert_eq!(
            image_metadata(html),
            (Some("a.png".into()), Some("a".into()))
        );
    }

    #[test]
    fn empty_or_blank_image_attribute_values() {
        let _ = env_logger::builder().is_test(true).try_init();

        let html = r#"<img src="" alt="">"#;
        assert_eq!(image_metadata(html), (Some("".into()), Some("".into())));

        let html = r#"<img src="  " alt="   ">"#;
        assert_eq!(
            image_metadata(html),
            (Some("  ".into()), Some("   ".into()))
        );
    }

    #[test]
    fn empty_or_missing_image_tag() {
        let _ = env_logger::builder().is_test(true).try_init();

        for html in [
            "<img>",
            "",
            "no markup at all",
            "a < b",
            "<p>text &lt;img src=a.png alt=a&gt;</p>",
            "<image src=a.png alt=a>",
        ] {
            assert_eq!(image_metadata(html), (None, None), "{html:?}");
        }
    }

    #[test]
    fn broken_markup() {
        let _ = env_logger::builder().is_test(true).try_init();

        assert_eq!(
            image_metadata(r#"<img src="unclosed alt="quote">"#),
            (Some("unclosed alt=".into()), None)
        );

        assert_eq!(
            image_metadata(r#"<p><img src="a.png" alt="ok""#),
            (Some("a.png".into()), Some("ok".into()))
        );
    }
}
