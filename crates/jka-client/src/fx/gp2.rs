//! Raven GenericParser2 (codemp/qcommon/GenericParser2.cpp) subset used by
//! `.efx` files: named groups, `key value` pairs whose value runs to end of
//! line, and `key [ ... ]` lists with one value per line.

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Group {
    pub name: String,
    pub pairs: Vec<Pair>,
    pub groups: Vec<Group>,
}

/// A pair holds one value, or several when written as a `[ ]` list.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    pub name: String,
    pub values: Vec<String>,
}

impl Pair {
    /// CGPValue::GetTopValue.
    pub fn top(&self) -> &str {
        self.values.first().map_or("", String::as_str)
    }
}

struct Tokens<'a> {
    text: &'a [u8],
    at: usize,
}

impl<'a> Tokens<'a> {
    /// GetToken(text, allowLineBreaks = true, readUntilEOL).
    fn next(&mut self, read_until_eol: bool) -> Option<String> {
        loop {
            while self.at < self.text.len() && self.text[self.at] <= b' ' {
                self.at += 1;
            }
            if self.at >= self.text.len() {
                return None;
            }
            let rest = &self.text[self.at..];
            if rest.starts_with(b"//") {
                while self.at < self.text.len() && self.text[self.at] != b'\n' {
                    self.at += 1;
                }
            } else if rest.starts_with(b"/*") {
                self.at += 2;
                while self.at + 1 < self.text.len() && !(self.text[self.at] == b'*' && self.text[self.at + 1] == b'/') {
                    self.at += 1;
                }
                self.at = (self.at + 2).min(self.text.len());
            } else {
                break;
            }
        }
        let start = self.at;
        let token = if self.text[start] == b'"' {
            self.at += 1;
            let begin = self.at;
            while self.at < self.text.len() && self.text[self.at] != b'"' {
                self.at += 1;
            }
            let token = &self.text[begin..self.at];
            self.at = (self.at + 1).min(self.text.len());
            token
        } else if read_until_eol {
            while self.at < self.text.len() {
                let c = self.text[self.at];
                if c == b'\n' || c == b'\r' {
                    break;
                }
                if c == b'/' && matches!(self.text.get(self.at + 1), Some(b'/' | b'*')) {
                    break;
                }
                self.at += 1;
            }
            let mut end = self.at;
            while end > start && self.text[end - 1] <= b' ' {
                end -= 1;
            }
            &self.text[start..end]
        } else {
            while self.at < self.text.len() && self.text[self.at] > b' ' {
                self.at += 1;
            }
            &self.text[start..self.at]
        };
        Some(String::from_utf8_lossy(token).into_owned())
    }
}

/// Parse a whole file into its root group (CGenericParser2::Parse).
pub fn parse(text: &[u8]) -> Group {
    let mut tokens = Tokens { text, at: 0 };
    let mut root = Group::default();
    parse_group(&mut tokens, &mut root, true);
    root
}

/// CGPGroup::Parse. Returns false on premature end of a nested group.
fn parse_group(tokens: &mut Tokens, group: &mut Group, is_root: bool) -> bool {
    loop {
        let Some(name) = tokens.next(false) else { return is_root };
        if name == "}" {
            return true;
        }
        let Some(lookahead) = tokens.next(true) else { return is_root };
        if lookahead == "{" {
            let mut sub = Group { name, ..Group::default() };
            let complete = parse_group(tokens, &mut sub, false);
            group.groups.push(sub);
            if !complete {
                return false;
            }
        } else if lookahead == "[" {
            let mut values = Vec::new();
            loop {
                let Some(value) = tokens.next(true) else {
                    group.pairs.push(Pair { name, values });
                    return false;
                };
                if value == "]" {
                    break;
                }
                values.push(value);
            }
            group.pairs.push(Pair { name, values });
        } else {
            group.pairs.push(Pair { name, values: vec![lookahead] });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_efx_groups_eol_values_lists_and_comments() {
        let root = parse(
            b"// Simple blaster effect\nrepeatDelay 300\nLine\n{\n\torigin\t8 0 0 // trailing\n\tflags\tuseAlpha\n\n\twidth\n\t{\n\t\tstart \t1.4 \t1.6\n\t}\n\tshader\n\t[\n\t\tgfx/effects/blaster_blob\n\t\t\"gfx/two words\"\n\t]\n}\n/* block\n comment */ particle { size { start 1.6 1.8 } }\n",
        );
        // A group opened on the same line is read to EOL as a pair value,
        // exactly like CGPGroup::Parse's read-ahead token.
        assert_eq!(root.pairs[0], Pair { name: "repeatDelay".into(), values: vec!["300".into()] });
        assert_eq!(root.pairs[1].top(), "{ size { start 1.6 1.8 } }");
        assert_eq!(root.groups.len(), 1);
        let line = &root.groups[0];
        assert_eq!(line.name, "Line");
        assert_eq!(line.pairs[0].top(), "8 0 0");
        assert_eq!(line.pairs[1].top(), "useAlpha");
        assert_eq!(line.pairs[2].values, ["gfx/effects/blaster_blob", "gfx/two words"]);
        assert_eq!(line.groups[0].name, "width");
        assert_eq!(line.groups[0].pairs[0].top(), "1.4 \t1.6");    }
}
