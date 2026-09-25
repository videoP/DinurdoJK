//! Text-command helpers shared by the live session and CGame.
//! References: OpenJK `qcommon/cmd.cpp` (Cmd_TokenizeString) and
//! `client/cl_cgame.cpp` (CL_GetServerCommand's bcs0/bcs1/bcs2 handling).

/// BIG_INFO_STRING: the assembled `cs` command must stay below this.
pub const BIG_INFO_STRING: usize = 8192;

/// Cmd_TokenizeString semantics: whitespace separated, `"quoted"` groups,
/// `//` and `/* */` comments. Bytes are preserved (legacy text is not UTF-8).
pub fn tokenize(mut text: &[u8]) -> Vec<Vec<u8>> {
    text = &text[..text.iter().position(|&b| b == 0).unwrap_or(text.len())];
    let mut tokens = Vec::new();
    loop {
        while text.first().is_some_and(|&b| b <= b' ') {
            text = &text[1..];
        }
        if text.is_empty() || text.starts_with(b"//") {
            return tokens;
        }
        if text.starts_with(b"/*") {
            match text.windows(2).position(|pair| pair == b"*/") {
                Some(end) => {
                    text = &text[end + 2..];
                    continue;
                }
                None => return tokens,
            }
        }
        if text[0] == b'"' {
            let body = &text[1..];
            let end = body.iter().position(|&b| b == b'"').unwrap_or(body.len());
            tokens.push(body[..end].to_vec());
            text = &body[(end + 1).min(body.len())..];
            continue;
        }
        let end = (0..text.len())
            .find(|&i| {
                text[i] <= b' '
                    || text[i] == b'"'
                    || text[i..].starts_with(b"//")
                    || text[i..].starts_with(b"/*")
            })
            .unwrap_or(text.len());
        tokens.push(text[..end].to_vec());
        text = &text[end..];
    }
}

/// Reassembles configstrings the server split with `bcs0`/`bcs1`/`bcs2`.
#[derive(Debug, Default, Clone)]
pub struct BigConfigString {
    buffer: Vec<u8>,
}

/// Result of feeding one server command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BigConfigOutcome {
    /// Not a bcs command: process the original text.
    PassThrough,
    /// Part of a split configstring; nothing to execute yet.
    Pending,
    /// The complete `cs <index> "<text>"` command to execute instead.
    Complete(Vec<u8>),
    /// bcs exceeded BIG_INFO_STRING (ERR_DROP in OpenJK).
    Overflow,
}

impl BigConfigString {
    pub fn feed(&mut self, command: &[u8]) -> BigConfigOutcome {
        let args = tokenize(command);
        let name = args.first().map(Vec::as_slice).unwrap_or_default();
        let arg = |index: usize| args.get(index).map(Vec::as_slice).unwrap_or_default();
        match name {
            b"bcs0" => {
                // Com_sprintf( bigConfigString, BIG_INFO_STRING, "cs %s \"%s", ... )
                self.buffer = [b"cs ".as_slice(), arg(1), b" \"", arg(2)].concat();
                self.buffer.truncate(BIG_INFO_STRING - 1);
                BigConfigOutcome::Pending
            }
            b"bcs1" => {
                if self.buffer.len() + arg(2).len() >= BIG_INFO_STRING {
                    return BigConfigOutcome::Overflow;
                }
                self.buffer.extend_from_slice(arg(2));
                BigConfigOutcome::Pending
            }
            b"bcs2" => {
                if self.buffer.len() + arg(2).len() + 1 >= BIG_INFO_STRING {
                    return BigConfigOutcome::Overflow;
                }
                self.buffer.extend_from_slice(arg(2));
                self.buffer.push(b'"');
                BigConfigOutcome::Complete(std::mem::take(&mut self.buffer))
            }
            _ => BigConfigOutcome::PassThrough,
        }
    }
}

/// Info_ValueForKey over a backslash-separated info string (case-insensitive).
pub fn info_value<'a>(info: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut fields = info.split(|&b| b == b'\\');
    if info.first() == Some(&b'\\') {
        let _ = fields.next();
    }
    loop {
        let candidate = fields.next()?;
        let value = fields.next()?;
        if candidate.eq_ignore_ascii_case(key) {
            return Some(value);
        }
    }
}

/// C `atoi`: optional sign and leading digits; anything else yields 0.
pub fn atoi(text: &[u8]) -> i32 {
    let text = &text[text.iter().take_while(|&&b| b == b' ' || b == b'\t').count()..];
    let (negative, digits) = match text.first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let mut value: i64 = 0;
    for &b in digits.iter().take_while(|b| b.is_ascii_digit()) {
        value = (value * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let value = if negative { -value } else { value };
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_quotes_and_comments() {
        assert_eq!(
            tokenize(b"cs 1 \"\\sv_serverid\\77\" // tail"),
            vec![b"cs".to_vec(), b"1".to_vec(), b"\\sv_serverid\\77".to_vec()]
        );
        assert_eq!(tokenize(b"  a/*x*/b "), vec![b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn assembles_split_configstrings() {
        let mut big = BigConfigString::default();
        assert_eq!(big.feed(b"print \"hi\""), BigConfigOutcome::PassThrough);
        assert_eq!(big.feed(b"bcs0 1 \"\\sv_serverid\\"), BigConfigOutcome::Pending);
        assert_eq!(big.feed(b"bcs1 1 \"12\""), BigConfigOutcome::Pending);
        let BigConfigOutcome::Complete(text) = big.feed(b"bcs2 1 \"34\\sv_pure\\0\"") else { panic!() };
        assert_eq!(text, b"cs 1 \"\\sv_serverid\\1234\\sv_pure\\0\"");
        let args = tokenize(&text);
        assert_eq!(info_value(&args[2], b"SV_SERVERID"), Some(b"1234".as_slice()));
    }

    #[test]
    fn atoi_matches_c() {
        assert_eq!(atoi(b"-123abc"), -123);
        assert_eq!(atoi(b"x"), 0);
        assert_eq!(atoi(b" 42"), 42);
        assert_eq!(atoi(b"99999999999"), i32::MAX);
    }
}
