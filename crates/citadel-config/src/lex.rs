#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Directive {
    pub key: String,
    pub args: Vec<String>,
    pub indent: usize,
    pub key_end: usize,
    pub val_start: usize,
    pub val_end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Blank,
    Comment,
    Other,
    Directive(Directive),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub text: String,
    pub eol: String,
    pub kind: Kind,
}

pub(crate) fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\x0c')
}

impl Line {
    pub fn new(text: String, eol: &str) -> Self {
        let kind = classify(&text);
        Self {
            text,
            eol: eol.to_owned(),
            kind,
        }
    }

    pub fn directive(&self) -> Option<&Directive> {
        match &self.kind {
            Kind::Directive(d) => Some(d),
            _ => None,
        }
    }

    pub fn is_blank(&self) -> bool {
        matches!(self.kind, Kind::Blank)
    }

    pub fn is_comment(&self) -> bool {
        matches!(self.kind, Kind::Comment)
    }

    pub fn is_header(&self) -> bool {
        self.directive()
            .is_some_and(|d| d.key == "host" || d.key == "match")
    }

    pub fn indent(&self) -> &str {
        self.directive().map_or("", |d| &self.text[..d.indent])
    }
}

pub(crate) fn split_lines(src: &str) -> Vec<Line> {
    src.split_inclusive('\n')
        .map(|seg| {
            let (text, eol) = match seg.strip_suffix("\r\n") {
                Some(t) => (t, "\r\n"),
                None => match seg.strip_suffix('\n') {
                    Some(t) => (t, "\n"),
                    None => (seg, ""),
                },
            };
            Line::new(text.to_owned(), eol)
        })
        .collect()
}

fn classify(text: &str) -> Kind {
    let body = text.trim_start_matches(is_ws);
    let indent = text.len() - body.len();
    if body.trim_end_matches(is_ws).is_empty() {
        return Kind::Blank;
    }
    if body.starts_with('#') {
        return Kind::Comment;
    }
    let key_len = body
        .find(|c: char| is_ws(c) || c == '=')
        .unwrap_or(body.len());
    if key_len == 0 {
        return Kind::Other;
    }
    let key_end = indent + key_len;
    let rest = &text[key_end..];
    let after_ws = rest.trim_start_matches(is_ws);
    let after_eq = after_ws.strip_prefix('=').unwrap_or(after_ws);
    let value = after_eq.trim_start_matches(is_ws);
    let val_start = text.len() - value.len();
    let (args, consumed) = split_args(value);
    Kind::Directive(Directive {
        key: text[indent..key_end].to_ascii_lowercase(),
        args,
        indent,
        key_end,
        val_start,
        val_end: val_start + consumed,
    })
}

pub(crate) fn split_args(s: &str) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut end = 0;
    let mut it = s.char_indices().peekable();
    loop {
        while it.next_if(|&(_, c)| is_ws(c)).is_some() {}
        match it.peek() {
            None | Some(&(_, '#')) => break,
            Some(_) => {}
        }
        let mut arg = String::new();
        let mut quote: Option<char> = None;
        while let Some(&(i, c)) = it.peek() {
            if c == '\\' {
                it.next();
                end = i + 1;
                match it.peek() {
                    Some(&(j, n))
                        if n == '\'' || n == '"' || n == '\\' || (quote.is_none() && n == ' ') =>
                    {
                        arg.push(n);
                        it.next();
                        end = j + n.len_utf8();
                    }
                    _ => arg.push('\\'),
                }
                continue;
            }
            if quote.is_none() && is_ws(c) {
                break;
            }
            it.next();
            end = i + c.len_utf8();
            match quote {
                None if c == '"' || c == '\'' => quote = Some(c),
                Some(q) if c == q => quote = None,
                _ => arg.push(c),
            }
        }
        out.push(arg);
    }
    (out, end)
}

pub(crate) fn quote(v: &str) -> String {
    let cleaned: String = v
        .chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect();
    let v = cleaned.as_str();
    let plain = !v.is_empty()
        && !v.starts_with('#')
        && !v
            .chars()
            .any(|c| is_ws(c) || c == '"' || c == '\'' || c == '\\');
    if plain {
        return v.to_owned();
    }
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

pub(crate) fn glob(pattern: &str, text: &str, fold: bool) -> bool {
    let norm = |s: &str| -> Vec<char> {
        if fold {
            s.chars().map(|c| c.to_ascii_lowercase()).collect()
        } else {
            s.chars().collect()
        }
    };
    let p = norm(pattern);
    let t = norm(text);
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(&c) if c == '?' || c == t[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_follows_openssh_argv_rules() {
        let cases: &[(&str, &[&str])] = &[
            ("a b  c", &["a", "b", "c"]),
            ("\"a b\" c", &["a b", "c"]),
            ("'a b'c\"d e\"", &["a bcd e"]),
            ("a\\ b", &["a b"]),
            ("\"a\\\"b\"", &["a\"b"]),
            ("C:\\Users\\x", &["C:\\Users\\x"]),
            ("a # trailing", &["a"]),
            ("a#b", &["a#b"]),
            ("", &[]),
            ("   ", &[]),
            ("\"unterminated value", &["unterminated value"]),
        ];
        for (src, want) in cases {
            let (got, _) = split_args(src);
            assert_eq!(&got, want, "{src:?}");
        }
    }

    #[test]
    fn quote_roundtrips_through_tokenizer() {
        let values = [
            "plain",
            "with space",
            "tab\there",
            "quo\"te",
            "single'q",
            "back\\slash",
            "C:\\Users\\me\\.ssh\\id",
            "#hash",
            "",
            "trailing\\",
            "ünïcödé path",
            "\\\"",
        ];
        for v in values {
            let (got, consumed) = split_args(&quote(v));
            assert_eq!(got, vec![v.to_owned()], "{v:?}");
            assert_eq!(consumed, quote(v).len());
        }
    }

    #[test]
    fn glob_matches_like_openssh() {
        assert!(glob("*", "anything", true));
        assert!(glob("web-*", "WEB-01", true));
        assert!(!glob("web-*", "WEB-01", false));
        assert!(glob("db?", "db1", true));
        assert!(!glob("db?", "db12", true));
        assert!(glob("*.corp.*", "a.corp.net", true));
        assert!(glob("a*b*c", "aXXbYYc", true));
        assert!(!glob("a*b*c", "aXXbYY", true));
        assert!(glob("", "", true));
        assert!(!glob("", "x", true));
    }

    #[test]
    fn classify_spans() {
        let l = Line::new("  Port = 2222  ".into(), "\n");
        let d = l.directive().unwrap();
        assert_eq!(d.key, "port");
        assert_eq!(d.args, vec!["2222"]);
        assert_eq!(&l.text[..d.indent], "  ");
        assert_eq!(&l.text[d.val_start..d.val_end], "2222");
        assert!(Line::new("=x".into(), "").kind == Kind::Other);
        assert!(Line::new("\t\r".into(), "").is_blank());
        assert!(Line::new("  # c".into(), "").is_comment());
    }
}
