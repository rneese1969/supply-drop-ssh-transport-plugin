//! Line editing and wire encoding for interactive SSH sessions.
//!
//! Telnet clients perform local echo and line assembly by default, so the old
//! Telnet transport only had to strip IAC negotiation out of the byte stream.
//! SSH is different: once a PTY is allocated the client sends every keystroke
//! straight through in raw mode and performs no echo of its own. That makes the
//! server responsible for echoing input, honouring backspace, recognising
//! control keys, and assembling complete lines before handing them to Supply
//! Drop. This module implements that terminal-side line discipline.

/// Interrupt character (Ctrl-C).
pub const CTRL_C: u8 = 3;
/// End-of-transmission character (Ctrl-D).
pub const CTRL_D: u8 = 4;
/// Kill-line character (Ctrl-U).
pub const CTRL_U: u8 = 21;
/// ASCII backspace.
pub const BACKSPACE: u8 = 8;
/// ASCII delete, sent by most terminal emulators for the backspace key.
pub const DEL: u8 = 127;
/// Escape, the lead byte of ANSI/CSI control sequences.
pub const ESC: u8 = 27;

/// Byte sequence that erases the character left of the cursor.
const ERASE: &[u8] = b"\x08 \x08";

#[derive(Debug, PartialEq, Eq)]
pub enum LineEvent {
    /// A complete line of user input, ready to forward to Supply Drop.
    Line(String),
    /// Bytes that must be written back to the client so typing is visible.
    Echo(Vec<u8>),
    /// The pending line exceeded `max_line_bytes` and was discarded.
    LineTooLong,
    /// The user pressed Ctrl-C.
    Interrupt,
    /// The user pressed Ctrl-D on an empty line.
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    Data,
    Escape,
    ControlSequence,
}

/// Assembles raw SSH channel bytes into lines, with echo and editing support.
#[derive(Debug)]
pub struct LineEditor {
    state: ParserState,
    line: Vec<u8>,
    max_line_bytes: usize,
    ignore_lf_or_nul: bool,
    echo: bool,
    hide_input: bool,
}

impl LineEditor {
    pub fn new(max_line_bytes: usize, echo: bool) -> Self {
        Self {
            state: ParserState::Data,
            line: Vec::new(),
            max_line_bytes,
            ignore_lf_or_nul: false,
            echo,
            hide_input: false,
        }
    }

    /// Enable or disable server-side echo. Echo is on for PTY sessions and off
    /// for non-interactive ones such as `ssh host somecommand` or piped input.
    #[allow(dead_code)]
    pub fn set_echo(&mut self, echo: bool) {
        self.echo = echo;
    }

    /// Suppress echo for the line currently being typed. This is the SSH
    /// equivalent of the Telnet transport sending `IAC WILL ECHO`, and is what
    /// makes Supply Drop's `hide_input` password prompts safe.
    pub fn set_hide_input(&mut self, hide_input: bool) {
        self.hide_input = hide_input;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn hide_input(&self) -> bool {
        self.hide_input
    }

    /// Feed bytes received from the client and return the resulting events.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<LineEvent> {
        let mut events = Vec::new();

        for &byte in bytes {
            match self.state {
                ParserState::Data => self.handle_data_byte(byte, &mut events),
                ParserState::Escape => {
                    // `ESC [` (CSI) and `ESC O` (SS3) introduce a multi-byte
                    // sequence such as an arrow key. Anything else is a lone
                    // escape we simply drop.
                    self.state = match byte {
                        b'[' | b'O' => ParserState::ControlSequence,
                        _ => ParserState::Data,
                    };
                }
                ParserState::ControlSequence => {
                    // CSI sequences end with a byte in the range 0x40..=0x7E.
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = ParserState::Data;
                    }
                }
            }
        }

        events
    }

    fn handle_data_byte(&mut self, byte: u8, events: &mut Vec<LineEvent>) {
        // A CR may be followed by LF or NUL, which must not open a second line.
        if self.ignore_lf_or_nul {
            self.ignore_lf_or_nul = false;
            if byte == b'\n' || byte == 0 {
                return;
            }
        }

        match byte {
            b'\r' => {
                self.emit_line(events);
                self.ignore_lf_or_nul = true;
            }
            b'\n' => self.emit_line(events),
            BACKSPACE | DEL => {
                if self.pop_char() && self.visible_echo() {
                    events.push(LineEvent::Echo(ERASE.to_vec()));
                }
            }
            CTRL_C => events.push(LineEvent::Interrupt),
            CTRL_D => {
                if self.line.is_empty() {
                    events.push(LineEvent::Eof);
                }
            }
            CTRL_U => {
                let erase_count = self.char_count();
                self.line.clear();
                if erase_count > 0 && self.visible_echo() {
                    events.push(LineEvent::Echo(ERASE.repeat(erase_count)));
                }
            }
            ESC => self.state = ParserState::Escape,
            // Drop remaining control characters so they never reach the BBS.
            0..=31 => {}
            _ => {
                if self.line.len() >= self.max_line_bytes {
                    self.line.clear();
                    events.push(LineEvent::LineTooLong);
                } else {
                    self.line.push(byte);
                    if self.visible_echo() {
                        events.push(LineEvent::Echo(vec![byte]));
                    }
                }
            }
        }
    }

    /// True when typed characters should be mirrored back to the client.
    fn visible_echo(&self) -> bool {
        self.echo && !self.hide_input
    }

    /// Remove the last character, treating UTF-8 sequences as a single unit.
    fn pop_char(&mut self) -> bool {
        if self.line.is_empty() {
            return false;
        }
        while let Some(&byte) = self.line.last() {
            self.line.pop();
            // Continuation bytes are 10xxxxxx; stop once we drop a lead byte.
            if byte & 0xC0 != 0x80 {
                break;
            }
        }
        true
    }

    /// Number of characters (not bytes) buffered in the pending line.
    fn char_count(&self) -> usize {
        self.line.iter().filter(|b| *b & 0xC0 != 0x80).count()
    }

    fn emit_line(&mut self, events: &mut Vec<LineEvent>) {
        // Move the cursor to a fresh line even when input was hidden, so BBS
        // output does not land on top of the prompt the user just answered.
        if self.echo {
            events.push(LineEvent::Echo(b"\r\n".to_vec()));
        }

        let raw = std::mem::take(&mut self.line);
        let line = String::from_utf8_lossy(&raw).into_owned();
        self.hide_input = false;
        events.push(LineEvent::Line(line));
    }
}

/// Normalise outbound BBS text for a terminal: bare LF becomes CRLF, and a
/// trailing newline is appended when requested. Identical to the Telnet
/// transport's encoding, since an SSH PTY expects CRLF line endings too.
pub fn text_to_wire(text: &str, append_newline: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 2);
    let mut prev_was_cr = false;

    for byte in text.bytes() {
        match byte {
            b'\n' if !prev_was_cr => {
                out.extend_from_slice(b"\r\n");
                prev_was_cr = false;
            }
            b'\n' => {
                out.push(b'\n');
                prev_was_cr = false;
            }
            b'\r' => {
                out.push(b'\r');
                prev_was_cr = true;
            }
            _ => {
                out.push(byte);
                prev_was_cr = false;
            }
        }
    }

    if append_newline && !out.ends_with(b"\n") && !out.ends_with(b"\r") {
        out.extend_from_slice(b"\r\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(events: &[LineEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                LineEvent::Line(line) => Some(line.clone()),
                _ => None,
            })
            .collect()
    }

    fn echoes(events: &[LineEvent]) -> Vec<u8> {
        events
            .iter()
            .filter_map(|event| match event {
                LineEvent::Echo(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn decodes_crlf_line() {
        let mut editor = LineEditor::new(1024, false);
        let events = editor.push(b"help\r\n");
        assert_eq!(events, vec![LineEvent::Line("help".to_string())]);
    }

    #[test]
    fn decodes_lf_only_line() {
        let mut editor = LineEditor::new(1024, false);
        assert_eq!(lines(&editor.push(b"help\n")), vec!["help".to_string()]);
    }

    #[test]
    fn splits_multiple_lines_in_one_packet() {
        let mut editor = LineEditor::new(1024, false);
        let events = editor.push(b"one\r\ntwo\r\n");
        assert_eq!(lines(&events), vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn echoes_typed_characters_when_enabled() {
        let mut editor = LineEditor::new(1024, true);
        let events = editor.push(b"hi\r\n");
        assert_eq!(echoes(&events), b"hi\r\n".to_vec());
        assert_eq!(lines(&events), vec!["hi".to_string()]);
    }

    #[test]
    fn does_not_echo_when_disabled() {
        let mut editor = LineEditor::new(1024, false);
        let events = editor.push(b"hi\r\n");
        assert!(echoes(&events).is_empty());
    }

    #[test]
    fn hides_input_but_still_reports_line() {
        let mut editor = LineEditor::new(1024, true);
        editor.set_hide_input(true);
        let events = editor.push(b"secret\r\n");

        // The password itself is never echoed, only the closing newline.
        assert_eq!(echoes(&events), b"\r\n".to_vec());
        assert_eq!(lines(&events), vec!["secret".to_string()]);
    }

    #[test]
    fn hide_input_resets_after_one_line() {
        let mut editor = LineEditor::new(1024, true);
        editor.set_hide_input(true);
        editor.push(b"secret\r\n");
        assert!(!editor.hide_input());

        let events = editor.push(b"ok\r\n");
        assert_eq!(echoes(&events), b"ok\r\n".to_vec());
    }

    #[test]
    fn handles_backspace() {
        let mut editor = LineEditor::new(1024, false);
        let events = editor.push(b"helpo\x08\r");
        assert_eq!(lines(&events), vec!["help".to_string()]);
    }

    #[test]
    fn handles_del_as_backspace_with_echo() {
        let mut editor = LineEditor::new(1024, true);
        let events = editor.push(b"ab\x7f");
        assert_eq!(echoes(&events), b"ab\x08 \x08".to_vec());
    }

    #[test]
    fn backspace_on_empty_line_is_ignored() {
        let mut editor = LineEditor::new(1024, true);
        let events = editor.push(b"\x08");
        assert!(echoes(&events).is_empty());
    }

    #[test]
    fn backspace_removes_whole_utf8_character() {
        let mut editor = LineEditor::new(1024, false);
        let events = editor.push("é\x08x\r".as_bytes());
        assert_eq!(lines(&events), vec!["x".to_string()]);
    }

    #[test]
    fn ctrl_u_clears_pending_line() {
        let mut editor = LineEditor::new(1024, false);
        let events = editor.push(b"junk\x15kept\r");
        assert_eq!(lines(&events), vec!["kept".to_string()]);
    }

    #[test]
    fn reports_ctrl_c_and_ctrl_d() {
        let mut editor = LineEditor::new(1024, false);
        assert_eq!(editor.push(&[CTRL_C]), vec![LineEvent::Interrupt]);
        assert_eq!(editor.push(&[CTRL_D]), vec![LineEvent::Eof]);
    }

    #[test]
    fn ctrl_d_mid_line_is_ignored() {
        let mut editor = LineEditor::new(1024, false);
        assert!(editor.push(b"ab\x04").is_empty());
    }

    #[test]
    fn strips_arrow_key_escape_sequences() {
        let mut editor = LineEditor::new(1024, false);
        let events = editor.push(b"a\x1b[Ab\r");
        assert_eq!(lines(&events), vec!["ab".to_string()]);
    }

    #[test]
    fn ignores_lone_lf_after_cr_split_across_packets() {
        let mut editor = LineEditor::new(1024, false);
        assert_eq!(lines(&editor.push(b"hi\r")), vec!["hi".to_string()]);
        assert!(lines(&editor.push(b"\n")).is_empty());
    }

    #[test]
    fn reports_overlong_lines() {
        let mut editor = LineEditor::new(4, false);
        let events = editor.push(b"toolong");
        assert!(events.contains(&LineEvent::LineTooLong));
    }

    #[test]
    fn normalizes_outbound_newlines() {
        assert_eq!(text_to_wire("a\nb", true), b"a\r\nb\r\n");
        assert_eq!(text_to_wire("a\r\n", true), b"a\r\n");
        assert_eq!(text_to_wire("Password: ", false), b"Password: ");
    }
}
