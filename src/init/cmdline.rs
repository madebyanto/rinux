//! Early kernel command-line storage and parsing.
//!
//! The Multiboot2 boot information is only valid while the boot region is
//! reserved. The command line is therefore copied into a static buffer before
//! the memory subsystem starts managing frames.
//!
//! Supported syntax: whitespace-separated `key=value` tokens, double quotes
//! around values, backslash escapes inside quotes, and a `--` terminator that
//! ends kernel parameter processing. Unknown keys and invalid values are
//! counted and ignored; they never prevent the kernel from booting.

use crate::sync::SpinLock;
use core::fmt;

/// Maximum retained command-line length, including the terminating NUL.
pub const COMMAND_LINE_SIZE: usize = 4096;

/// Maximum length of a decoded parameter value.
pub const MAX_VALUE: usize = 256;

const MAX_TOKEN: usize = 512;
const DEFAULT_LOGLEVEL: u8 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Console {
    Serial,
    Vga,
    Both,
}

/// A NUL-terminated, fixed-capacity parameter value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLineValue {
    bytes: [u8; MAX_VALUE],
    len: u16,
}

impl CommandLineValue {
    pub const fn new() -> Self {
        Self {
            bytes: [0; MAX_VALUE],
            len: 0,
        }
    }

    pub fn from_bytes(value: &[u8]) -> Option<Self> {
        if value.len() >= MAX_VALUE {
            return None;
        }
        let mut stored = Self::new();
        stored.bytes[..value.len()].copy_from_slice(value);
        stored.len = value.len() as u16;
        Some(stored)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

impl fmt::Display for CommandLineValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match core::str::from_utf8(self.as_bytes()) {
            Ok(text) => formatter.write_str(text),
            Err(_) => formatter.write_str("<invalid utf-8>"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootParams {
    pub init: Option<CommandLineValue>,
    pub root: Option<CommandLineValue>,
    pub console: Console,
    pub loglevel: u8,
    pub selftest: bool,
}

impl BootParams {
    pub const fn new() -> Self {
        Self {
            init: None,
            root: None,
            console: Console::Both,
            loglevel: DEFAULT_LOGLEVEL,
            selftest: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLineReport {
    pub params: BootParams,
    pub stored_len: usize,
    pub truncated: bool,
    pub unknown: u16,
    pub invalid: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Param {
    Init,
    Root,
    Console,
    LogLevel,
    SelfTest,
}

/// Static parameter table. Adding a parameter means adding an entry here and
/// one arm in `apply`.
const PARAMETERS: &[(&str, Param)] = &[
    ("init", Param::Init),
    ("root", Param::Root),
    ("console", Param::Console),
    ("loglevel", Param::LogLevel),
    ("selftest", Param::SelfTest),
];

struct State {
    bytes: [u8; COMMAND_LINE_SIZE],
    params: BootParams,
}

impl State {
    const fn new() -> Self {
        Self {
            bytes: [0; COMMAND_LINE_SIZE],
            params: BootParams::new(),
        }
    }
}

static COMMAND_LINE: SpinLock<State> = SpinLock::new(State::new());

/// Copies, truncates, and parses the Multiboot2 command line.
///
/// Returns a snapshot of the parsed parameters and diagnostics. The stored
/// copy remains available through [`params`].
pub fn init(input: &[u8]) -> CommandLineReport {
    let mut state = COMMAND_LINE.lock();
    let copy_len = input.len().min(COMMAND_LINE_SIZE - 1);
    state.bytes[..copy_len].copy_from_slice(&input[..copy_len]);
    state.bytes[copy_len] = 0;

    let report = parse(&state.bytes[..copy_len]);
    state.params = report.params;
    CommandLineReport {
        params: report.params,
        stored_len: copy_len,
        truncated: report.truncated || input.len() > copy_len,
        unknown: report.unknown,
        invalid: report.invalid,
    }
}

/// Returns the parameters parsed by [`init`].
pub fn params() -> BootParams {
    COMMAND_LINE.lock().params
}

/// Parses a command line without touching the stored copy.
pub fn parse(input: &[u8]) -> CommandLineReport {
    let mut report = CommandLineReport {
        params: BootParams::new(),
        stored_len: input.len(),
        truncated: false,
        unknown: 0,
        invalid: 0,
    };
    let mut token = [0u8; MAX_TOKEN];
    let mut cursor = 0;

    while cursor < input.len() {
        while cursor < input.len() && is_space(input[cursor]) {
            cursor += 1;
        }
        if cursor == input.len() {
            break;
        }

        let mut len = 0;
        let mut overflow = false;
        let mut quoted = false;
        while cursor < input.len() {
            let byte = input[cursor];
            if byte == b'\\' && cursor + 1 < input.len() {
                push_byte(&mut token, &mut len, &mut overflow, input[cursor + 1]);
                cursor += 2;
            } else if byte == b'"' {
                quoted = !quoted;
                cursor += 1;
            } else if !quoted && is_space(byte) {
                break;
            } else {
                push_byte(&mut token, &mut len, &mut overflow, byte);
                cursor += 1;
            }
        }

        if overflow {
            report.truncated = true;
            continue;
        }
        if len == 0 {
            continue;
        }
        let token = &token[..len];
        if token == b"--" {
            break;
        }
        apply(token, &mut report);
    }

    report
}

fn apply(token: &[u8], report: &mut CommandLineReport) {
    let Some(separator) = token.iter().position(|byte| *byte == b'=') else {
        report.unknown = report.unknown.saturating_add(1);
        return;
    };
    let key = &token[..separator];
    let value = &token[separator + 1..];
    let Some((_, parameter)) = PARAMETERS.iter().find(|(name, _)| name.as_bytes() == key) else {
        report.unknown = report.unknown.saturating_add(1);
        return;
    };

    match *parameter {
        Param::Init => match CommandLineValue::from_bytes(value) {
            Some(path) if value.first() == Some(&b'/') => report.params.init = Some(path),
            _ => report.invalid = report.invalid.saturating_add(1),
        },
        Param::Root => match CommandLineValue::from_bytes(value) {
            Some(root) if !value.is_empty() => report.params.root = Some(root),
            _ => report.invalid = report.invalid.saturating_add(1),
        },
        Param::Console => match value {
            b"serial" => report.params.console = Console::Serial,
            b"vga" => report.params.console = Console::Vga,
            b"both" => report.params.console = Console::Both,
            _ => report.invalid = report.invalid.saturating_add(1),
        },
        Param::LogLevel => match parse_decimal(value) {
            Some(level) if level <= 7 => report.params.loglevel = level,
            _ => report.invalid = report.invalid.saturating_add(1),
        },
        Param::SelfTest => match value {
            b"1" => report.params.selftest = true,
            b"0" => report.params.selftest = false,
            _ => report.invalid = report.invalid.saturating_add(1),
        },
    }
}

fn push_byte(buffer: &mut [u8], len: &mut usize, overflow: &mut bool, byte: u8) {
    if *len < buffer.len() {
        buffer[*len] = byte;
        *len += 1;
    } else {
        *overflow = true;
    }
}

fn parse_decimal(bytes: &[u8]) -> Option<u8> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: u16 = 0;
    for byte in bytes {
        if !(b'0'..=b'9').contains(byte) {
            return None;
        }
        value = value.checked_mul(10)?;
        value = value.checked_add((*byte - b'0') as u16)?;
        if value > u8::MAX as u16 {
            return None;
        }
    }
    Some(value as u8)
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}
