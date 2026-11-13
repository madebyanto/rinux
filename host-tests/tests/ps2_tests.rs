use hosttest::ps2::{translate, KeyBuffer, KeyBufferError, Keyboard, BUFFER_CAPACITY, F12};

#[test]
fn translates_letters_with_and_without_shift() {
    assert_eq!(translate(0x10, false), Some(b'q'));
    assert_eq!(translate(0x10, true), Some(b'Q'));
    assert_eq!(translate(0x1E, false), Some(b'a'));
    assert_eq!(translate(0x1E, true), Some(b'A'));
    assert_eq!(translate(0x2C, false), Some(b'z'));
    assert_eq!(translate(0x2C, true), Some(b'Z'));
    assert_eq!(translate(0x32, false), Some(b'm'));
    assert_eq!(translate(0x32, true), Some(b'M'));
}

#[test]
fn translates_full_alphabet() {
    let alphabet = [
        (0x10u8, b'q'),
        (0x11, b'w'),
        (0x12, b'e'),
        (0x13, b'r'),
        (0x14, b't'),
        (0x15, b'y'),
        (0x16, b'u'),
        (0x17, b'i'),
        (0x18, b'o'),
        (0x19, b'p'),
        (0x1E, b'a'),
        (0x1F, b's'),
        (0x20, b'd'),
        (0x21, b'f'),
        (0x22, b'g'),
        (0x23, b'h'),
        (0x24, b'j'),
        (0x25, b'k'),
        (0x26, b'l'),
        (0x2C, b'z'),
        (0x2D, b'x'),
        (0x2E, b'c'),
        (0x2F, b'v'),
        (0x30, b'b'),
        (0x31, b'n'),
        (0x32, b'm'),
    ];
    for (scancode, lower) in alphabet {
        assert_eq!(translate(scancode, false), Some(lower), "{scancode:#04x}");
        assert_eq!(
            translate(scancode, true),
            Some(lower.to_ascii_uppercase()),
            "{scancode:#04x}"
        );
    }
}

#[test]
fn translates_digits_symbols_and_punctuation() {
    let shifted = [
        (0x02u8, b'!'),
        (0x03, b'@'),
        (0x04, b'#'),
        (0x05, b'$'),
        (0x06, b'%'),
        (0x07, b'^'),
        (0x08, b'&'),
        (0x09, b'*'),
        (0x0A, b'('),
        (0x0B, b')'),
        (0x0C, b'_'),
        (0x0D, b'+'),
        (0x1A, b'{'),
        (0x1B, b'}'),
        (0x27, b':'),
        (0x28, b'"'),
        (0x29, b'~'),
        (0x2B, b'|'),
        (0x33, b'<'),
        (0x34, b'>'),
        (0x35, b'?'),
    ];
    for (scancode, expected) in shifted {
        assert_eq!(translate(scancode, true), Some(expected), "{scancode:#04x}");
    }
    let plain = [
        (0x02u8, b'1'),
        (0x0B, b'0'),
        (0x0C, b'-'),
        (0x0D, b'='),
        (0x1A, b'['),
        (0x1B, b']'),
        (0x27, b';'),
        (0x28, b'\''),
        (0x29, b'`'),
        (0x2B, b'\\'),
        (0x33, b','),
        (0x34, b'.'),
        (0x35, b'/'),
    ];
    for (scancode, expected) in plain {
        assert_eq!(translate(scancode, false), Some(expected), "{scancode:#04x}");
        assert_ne!(translate(scancode, true), Some(expected), "{scancode:#04x}");
    }
}

#[test]
fn translates_control_keys() {
    assert_eq!(translate(0x01, false), Some(0x1B));
    assert_eq!(translate(0x0E, false), Some(0x08));
    assert_eq!(translate(0x0F, false), Some(b'\t'));
    assert_eq!(translate(0x1C, false), Some(b'\n'));
    assert_eq!(translate(0x39, false), Some(b' '));
    assert_eq!(translate(0x39, true), Some(b' '));
}

#[test]
fn rejects_break_codes_and_unmapped_keys() {
    for scancode in 0u8..0x80 {
        assert_eq!(translate(scancode | 0x80, false), None, "{scancode:#04x}");
        assert_eq!(translate(scancode | 0x80, true), None, "{scancode:#04x}");
    }
    assert_eq!(translate(0x00, false), None);
    assert_eq!(translate(F12, false), None);
    assert_eq!(translate(0x46, false), None);
}

#[test]
fn key_buffer_is_fifo_and_wraps() {
    let mut buffer = KeyBuffer::new();
    assert!(buffer.is_empty());
    assert_eq!(buffer.pop(), None);

    buffer.push(b'a').unwrap();
    buffer.push(b'b').unwrap();
    buffer.push(b'c').unwrap();
    assert_eq!(buffer.len(), 3);
    assert_eq!(buffer.pop(), Some(b'a'));
    assert_eq!(buffer.pop(), Some(b'b'));

    while buffer.len() < BUFFER_CAPACITY {
        buffer.push(b'x').unwrap();
    }
    assert_eq!(buffer.len(), BUFFER_CAPACITY);
    assert_eq!(buffer.push(b'x'), Err(KeyBufferError::Full));
    assert_eq!(buffer.pop(), Some(b'c'));
    buffer.push(b'y').unwrap();
    assert_eq!(buffer.len(), BUFFER_CAPACITY);
    assert_eq!(buffer.push(b'z'), Err(KeyBufferError::Full));
}

#[test]
fn key_buffer_preserves_order_across_wrap() {
    let mut buffer = KeyBuffer::new();
    for index in 0..BUFFER_CAPACITY as u8 {
        buffer.push(index).unwrap();
    }
    for index in 0..BUFFER_CAPACITY / 2 {
        assert_eq!(buffer.pop(), Some(index as u8));
    }
    for index in 0..BUFFER_CAPACITY / 2 {
        buffer.push(0x80 | index as u8).unwrap();
    }
    for index in BUFFER_CAPACITY / 2..BUFFER_CAPACITY {
        assert_eq!(buffer.pop(), Some(index as u8));
    }
    for index in 0..BUFFER_CAPACITY / 2 {
        assert_eq!(buffer.pop(), Some(0x80 | index as u8));
    }
    assert!(buffer.is_empty());
}

#[test]
fn keyboard_tracks_shift_state() {
    let mut keyboard = Keyboard::new();
    assert!(!keyboard.shift_pressed());
    assert_eq!(keyboard.handle_scancode(0x1E), Some(b'a'));

    assert_eq!(keyboard.handle_scancode(0x2A), None);
    assert!(keyboard.shift_pressed());
    assert_eq!(keyboard.handle_scancode(0x1E), Some(b'A'));

    assert_eq!(keyboard.handle_scancode(0xAA), None);
    assert!(!keyboard.shift_pressed());
    assert_eq!(keyboard.handle_scancode(0x1E), Some(b'a'));

    assert_eq!(keyboard.handle_scancode(0x36), None);
    assert!(keyboard.shift_pressed());
    assert_eq!(keyboard.handle_scancode(0x1E), Some(b'A'));
    assert_eq!(keyboard.handle_scancode(0xB6), None);
    assert!(!keyboard.shift_pressed());
}

#[test]
fn keyboard_buffers_pressed_keys_and_ignores_releases() {
    let mut keyboard = Keyboard::new();
    keyboard.handle_scancode(0x1E);
    keyboard.handle_scancode(0x9E);
    keyboard.handle_scancode(0x1F);
    keyboard.handle_scancode(0x20);
    assert_eq!(keyboard.pending(), 3);
    assert_eq!(keyboard.read_key(), Some(b'a'));
    assert_eq!(keyboard.read_key(), Some(b's'));
    assert_eq!(keyboard.read_key(), Some(b'd'));
    assert_eq!(keyboard.read_key(), None);
    assert!(!keyboard.take_magic());
}

#[test]
fn keyboard_reports_f12_once() {
    let mut keyboard = Keyboard::new();
    assert_eq!(keyboard.handle_scancode(F12), None);
    assert!(keyboard.take_magic());
    assert!(!keyboard.take_magic());
    assert_eq!(keyboard.handle_scancode(F12 | 0x80), None);
    assert!(!keyboard.take_magic());
}
