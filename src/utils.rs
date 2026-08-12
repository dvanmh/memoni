use egui::Key;
use xkeysym::Keysym;

pub fn keysym_to_egui_key(ks: Keysym) -> Option<Key> {
    Some(match ks {
        Keysym::Down => Key::ArrowDown,
        Keysym::Left => Key::ArrowLeft,
        Keysym::Right => Key::ArrowRight,
        Keysym::Up => Key::ArrowUp,

        Keysym::BackSpace => Key::Backspace,
        Keysym::Return => Key::Enter,
        Keysym::space => Key::Space,
        Keysym::Tab => Key::Tab,
        Keysym::Escape => Key::Escape,
        Keysym::Insert => Key::Insert,
        Keysym::Delete => Key::Delete,
        Keysym::Home => Key::Home,
        Keysym::End => Key::End,
        Keysym::Page_Up => Key::PageUp,
        Keysym::Page_Down => Key::PageDown,

        Keysym::a => Key::A,
        Keysym::b => Key::B,
        Keysym::c => Key::C,
        Keysym::d => Key::D,
        Keysym::e => Key::E,
        Keysym::f => Key::F,
        Keysym::g => Key::G,
        Keysym::h => Key::H,
        Keysym::i => Key::I,
        Keysym::j => Key::J,
        Keysym::k => Key::K,
        Keysym::l => Key::L,
        Keysym::m => Key::M,
        Keysym::n => Key::N,
        Keysym::o => Key::O,
        Keysym::p => Key::P,
        Keysym::q => Key::Q,
        Keysym::r => Key::R,
        Keysym::s => Key::S,
        Keysym::t => Key::T,
        Keysym::u => Key::U,
        Keysym::v => Key::V,
        Keysym::w => Key::W,
        Keysym::x => Key::X,
        Keysym::y => Key::Y,
        Keysym::z => Key::Z,

        Keysym::_1 => Key::Num1,
        Keysym::_2 => Key::Num2,
        Keysym::_3 => Key::Num3,
        Keysym::_4 => Key::Num4,
        Keysym::_5 => Key::Num5,
        Keysym::_6 => Key::Num6,
        Keysym::_7 => Key::Num7,
        Keysym::_8 => Key::Num8,
        Keysym::_9 => Key::Num9,
        Keysym::_0 => Key::Num0,

        Keysym::F1 => Key::F1,
        Keysym::F2 => Key::F2,
        Keysym::F3 => Key::F3,
        Keysym::F4 => Key::F4,
        Keysym::F5 => Key::F5,
        Keysym::F6 => Key::F6,
        Keysym::F7 => Key::F7,
        Keysym::F8 => Key::F8,
        Keysym::F9 => Key::F9,
        Keysym::F10 => Key::F10,
        Keysym::F11 => Key::F11,
        Keysym::F12 => Key::F12,
        Keysym::F13 => Key::F13,
        Keysym::F14 => Key::F14,
        Keysym::F15 => Key::F15,
        Keysym::F16 => Key::F16,
        Keysym::F17 => Key::F17,
        Keysym::F18 => Key::F18,
        Keysym::F19 => Key::F19,
        Keysym::F20 => Key::F20,
        Keysym::F21 => Key::F21,
        Keysym::F22 => Key::F22,
        Keysym::F23 => Key::F23,
        Keysym::F24 => Key::F24,
        Keysym::F25 => Key::F25,
        Keysym::F26 => Key::F26,
        Keysym::F27 => Key::F27,
        Keysym::F28 => Key::F28,
        Keysym::F29 => Key::F29,
        Keysym::F30 => Key::F30,
        Keysym::F31 => Key::F31,
        Keysym::F32 => Key::F32,
        Keysym::F33 => Key::F33,
        Keysym::F34 => Key::F34,
        Keysym::F35 => Key::F35,

        Keysym::colon => Key::Colon,
        Keysym::comma => Key::Comma,
        Keysym::minus => Key::Minus,
        Keysym::period => Key::Period,
        Keysym::plus => Key::Plus,
        Keysym::equal => Key::Equals,
        Keysym::semicolon => Key::Semicolon,
        Keysym::bracketleft => Key::OpenBracket,
        Keysym::bracketright => Key::CloseBracket,
        Keysym::backslash => Key::Backslash,
        Keysym::slash => Key::Slash,
        Keysym::apostrophe => Key::Quote,
        Keysym::grave => Key::Backtick,
        Keysym::bar => Key::Pipe,
        Keysym::question => Key::Questionmark,
        Keysym::exclam => Key::Exclamationmark,
        Keysym::braceleft => Key::OpenCurlyBracket,
        Keysym::braceright => Key::CloseCurlyBracket,

        _ => return None,
    })
}

pub fn plaintext_mime_score(mime: &str) -> Option<usize> {
    // low to high
    const TEXT_MIMES_ORDER: &[&str] = &[
        "",
        "text/plain;charset=us-ascii",
        "text/plain;charset=unicode",
        "text",
        "string",
        "text/plain",
        "text/plain;charset=utf-8",
        "utf8_string",
    ];

    TEXT_MIMES_ORDER
        .iter()
        .position(|b| mime.eq_ignore_ascii_case(b))
}

pub fn is_plaintext_mime(mime: &str) -> bool {
    plaintext_mime_score(mime).is_some()
}

pub fn image_mime_score(mime: &str) -> usize {
    // low to high
    const IMAGE_MIMES_ORDER: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/svg+xml"];

    IMAGE_MIMES_ORDER
        .iter()
        .position(|b| mime.eq_ignore_ascii_case(b))
        .map(|pos| pos + 1)
        .unwrap_or(0)
}

pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/")
}

pub fn utf16le_to_string(bytes: &[u8]) -> String {
    assert!(bytes.len().is_multiple_of(2));
    let u16_slice: &[u16] =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, bytes.len() / 2) };
    String::from_utf16_lossy(u16_slice)
}

pub fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut bytes = input.iter();
    while let Some(&b) = bytes.next() {
        if b == b'%' {
            let x = bytes.next();
            let y = bytes.next();
            if let (Some(&x), Some(&y)) = (x, y)
                && let Ok(v) = u8::from_str_radix(str::from_utf8(&[x, y]).unwrap(), 16)
            {
                out.push(v);
                continue;
            }

            out.push(b'%');
            if let Some(&x) = x {
                out.push(x);
            }
            if let Some(&y) = y {
                out.push(y);
            }
        } else {
            out.push(b);
        }
    }
    out
}

pub fn percent_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for b in input {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b'-'
            | b'.'
            | b':'
            | b'='
            | b'@'
            | b'_'
            | b'~' => out.push(*b),
            _ => {
                out.push(b'%');
                out.append(&mut format!("{:02X}", b).into_bytes());
            }
        }
    }
    out
}

pub fn to_hex_string(bytes: &[u8]) -> String {
    let hex_chars = b"0123456789abcdef";
    let mut hex_str = vec!['\0'; bytes.len() * 2];
    for (i, &b) in bytes.iter().enumerate() {
        hex_str[i * 2] = hex_chars[(b >> 4) as usize] as char;
        hex_str[i * 2 + 1] = hex_chars[(b & 0x0f) as usize] as char;
    }

    hex_str.into_iter().collect::<String>()
}
