//! A movie's subtitle file, `<name>.sbt`, as the subtitle loader reads it (`0x78cb00`).

use super::super::console_exec::s_str_tokenize;

/// One caption: its start and end on the movie clock, in milliseconds, and its text, the 12-byte
/// record the loader appends (`0x78cceb`, `0x78cd44`, `0x78cd62`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Caption {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) text: Vec<u8>,
}

/// The line buffer the loader tokenizes into (`[ebp-0x408]`, `0x400` bytes).
const LINE_BYTES: usize = 0x400;

/// The milliseconds a caption time's frame field counts (`0x78ccdf imul eax,eax,0x29`).
const FRAME_MS: i32 = 41;

/// The captions of an `.sbt` file. A leading `0xef` skips three bytes, a UTF-8 mark
/// (`0x78cb4d`). The file is cut into lines at `\r` and `\n` (`0x64ae50`, delimiters `0x82edfc`),
/// empty ones skipped; a line that starts with `0` (`0x78cb8a`) is a time line,
/// `HH:MM:SS:FF - HH:MM:SS:FF`, and the line after it is the caption's text. Any other line is
/// passed over. The read ends at an empty token, a caption with an empty text included, or the
/// file's end.
pub(crate) fn parse(file: &[u8]) -> Vec<Caption> {
    // The loaded file ends at its terminator, and the cursor stops at the first NUL (`0x78cd72`).
    let end = file.iter().position(|&b| b == 0).unwrap_or(file.len());
    let mut cursor = &file[..end];
    if cursor.first() == Some(&0xef) {
        cursor = cursor.get(3..).unwrap_or(&[]);
    }
    let mut line = [0u8; LINE_BYTES];
    let next_line = |cursor: &mut &[u8], line: &mut [u8; LINE_BYTES]| {
        let token = s_str_tokenize(cursor, LINE_BYTES, b"\r\n");
        line[..token.len()].copy_from_slice(&token);
        line[token.len()] = 0;
        token
    };
    let mut out = Vec::new();
    loop {
        let token = next_line(&mut cursor, &mut line);
        let Some(&first) = token.first() else {
            break;
        };
        if first == b'0' {
            // Each field is `SStrToInt` at its fixed column of the buffer (`0x78cca4`-`0x78cd28`),
            // which reads past a short line into what the longer lines before it left there.
            let start = time_at(&line, 0);
            let end = time_at(&line, 14);
            let text = next_line(&mut cursor, &mut line);
            let empty = text.is_empty();
            out.push(Caption { start, end, text });
            if empty {
                break;
            }
        }
        if cursor.is_empty() {
            break;
        }
    }
    out
}

/// The time whose hours field starts at `at`: hours, minutes and seconds at `at`, `at + 3` and
/// `at + 6` as milliseconds, plus the frames at `at + 9` times [`FRAME_MS`], in the 32-bit
/// register's wrapping arithmetic.
fn time_at(line: &[u8; LINE_BYTES], at: usize) -> u32 {
    let field = |off: usize| int_at(line, at + off);
    let hms = field(0)
        .wrapping_mul(60)
        .wrapping_add(field(3))
        .wrapping_mul(60)
        .wrapping_add(field(6))
        .wrapping_mul(1000);
    hms.wrapping_add(field(9).wrapping_mul(FRAME_MS)) as u32
}

/// `SStrToInt` (`0x64ac60`) of the NUL-terminated bytes at `at`.
fn int_at(line: &[u8; LINE_BYTES], at: usize) -> i32 {
    let rest = &line[at.min(LINE_BYTES)..];
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    super::super::binding_abi::s_str_to_int(&String::from_utf8_lossy(&rest[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caption(start: u32, end: u32, text: &str) -> Caption {
        Caption {
            start,
            end,
            text: text.as_bytes().to_vec(),
        }
    }

    /// The time lines' fields at their columns, the frames at 41 ms; the mark skipped, `\r\n`
    /// and blank lines alike as one break, and a line that is no time line passed over.
    #[test]
    fn an_sbt_file_reads_as_its_captions() {
        let file = b"\xef\xbb\xbfMovie captions\r\n\r\n\
            00:00:01:12 - 00:00:04:00\r\nFirst line\r\n\
            \n00:01:02:03 - 01:00:00:24\nSecond\n";
        assert_eq!(
            parse(file),
            [
                caption(1000 + 12 * 41, 4000, "First line"),
                caption(62_000 + 3 * 41, 3_600_000 + 24 * 41, "Second"),
            ]
        );
    }

    /// The fields are read at fixed columns of the line buffer, so a short time line reads the
    /// bytes a longer line left past its end; a caption with no text ends the file, kept.
    #[test]
    fn a_short_time_line_reads_what_the_buffer_holds_and_an_empty_text_ends_the_read() {
        // `0:5` puts its terminator at column 3, so the minutes read 0 and the seconds at column
        // 6 read the first line's `01`, and the end's fields read its `00:00:09:00`. A blank line
        // is no token, so the line after it is the third caption's text, and `C`, no time line,
        // is passed over.
        let file = b"00:00:01:00 - 00:00:09:00\nA\n0:5\nB\n00:00:20:00 - 00:00:21:00\n\n\
            00:00:30:00 - 00:00:31:00\nC\n";
        assert_eq!(
            parse(file),
            [
                caption(1000, 9000, "A"),
                caption(1000, 9000, "B"),
                caption(20_000, 21_000, "00:00:30:00 - 00:00:31:00"),
            ]
        );
        // The text line is tokenized whatever it holds; only an empty token stops the read.
        assert_eq!(
            parse(b"00:00:01:00 - 00:00:02:00\n"),
            [caption(1000, 2000, "")]
        );
        assert!(parse(b"").is_empty());
        assert!(parse(b"\xef\xbb").is_empty());
    }
}
