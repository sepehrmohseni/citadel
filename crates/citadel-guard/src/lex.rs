#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Word {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub quoted: bool,
    pub bare_var: bool,
    pub subs: Vec<(String, usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Redir {
    pub op: String,
    pub target: Option<Word>,
    pub start: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cmd {
    pub words: Vec<Word>,
    pub redirs: Vec<Redir>,
    pub piped_from: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Word(Word),
    Op(String),
    Redir(String, usize),
}

const MAX_DEPTH: usize = 8;

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            chars: src.char_indices().collect(),
            pos: 0,
        }
    }

    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).map(|&(_, c)| c)
    }

    fn at(&self) -> usize {
        self.chars.get(self.pos).map_or(self.src.len(), |&(i, _)| i)
    }

    fn balanced(&mut self, open: char, close: char) -> (String, usize, usize) {
        let start = self.at();
        let mut depth = 1usize;
        let mut quote: Option<char> = None;
        while let Some(c) = self.peek(0) {
            if let Some(q) = quote {
                if c == '\\' && q == '"' {
                    self.pos += 1;
                } else if c == q {
                    quote = None;
                }
            } else if c == '\'' || c == '"' {
                quote = Some(c);
            } else if c == '\\' {
                self.pos += 1;
            } else if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    let end = self.at();
                    self.pos += 1;
                    return (self.src[start..end].to_owned(), start, end);
                }
            }
            self.pos += 1;
        }
        let end = self.src.len();
        (self.src[start..end].to_owned(), start, end)
    }

    fn until(&mut self, close: char) -> (String, usize, usize) {
        let start = self.at();
        while let Some(c) = self.peek(0) {
            if c == '\\' {
                self.pos += 2;
                continue;
            }
            if c == close {
                let end = self.at();
                self.pos += 1;
                return (self.src[start..end].to_owned(), start, end);
            }
            self.pos += 1;
        }
        let end = self.src.len();
        (self.src[start..end].to_owned(), start, end)
    }

    fn dollar(&mut self, w: &mut Word, in_double: bool) {
        self.pos += 1;
        match self.peek(0) {
            Some('(') => {
                self.pos += 1;
                if self.peek(0) == Some('(') {
                    self.pos += 1;
                    let (inner, _, _) = self.balanced('(', ')');
                    if self.peek(0) == Some(')') {
                        self.pos += 1;
                    }
                    w.text.push_str("$((");
                    w.text.push_str(&inner);
                    w.text.push_str("))");
                } else {
                    let (inner, s, e) = self.balanced('(', ')');
                    w.text.push_str("$(");
                    w.text.push_str(&inner);
                    w.text.push(')');
                    w.subs.push((inner, s, e));
                }
            }
            Some('{') => {
                self.pos += 1;
                let (inner, _, _) = self.until('}');
                w.text.push_str("${");
                w.text.push_str(&inner);
                w.text.push('}');
                if !in_double {
                    w.bare_var = true;
                }
            }
            Some(c) if c.is_ascii_alphanumeric() || c == '_' || "?#@*!$-".contains(c) => {
                w.text.push('$');
                let single = !(c.is_ascii_alphabetic() || c == '_');
                while let Some(c) = self.peek(0) {
                    if c.is_ascii_alphanumeric() || c == '_' || (single && w.text.ends_with('$')) {
                        w.text.push(c);
                        self.pos += 1;
                        if single {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if !in_double {
                    w.bare_var = true;
                }
            }
            _ => w.text.push('$'),
        }
    }

    fn tokens(mut self) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut cur: Option<Word> = None;
        let flush = |cur: &mut Option<Word>, out: &mut Vec<Tok>, end: usize| {
            if let Some(mut w) = cur.take() {
                w.end = end;
                out.push(Tok::Word(w));
            }
        };
        while let Some(c) = self.peek(0) {
            let here = self.at();
            match c {
                ' ' | '\t' | '\r' => {
                    flush(&mut cur, &mut out, here);
                    self.pos += 1;
                }
                '#' if cur.is_none() => {
                    while let Some(c) = self.peek(0) {
                        if c == '\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                '\\' => {
                    let w = cur.get_or_insert_with(|| Word { start: here, ..Word::default() });
                    self.pos += 1;
                    if let Some(n) = self.peek(0) {
                        if n != '\n' {
                            w.text.push(n);
                        }
                        self.pos += 1;
                    }
                }
                '\'' => {
                    let w = cur.get_or_insert_with(|| Word { start: here, ..Word::default() });
                    w.quoted = true;
                    self.pos += 1;
                    let (inner, _, _) = self.until('\'');
                    w.text.push_str(&inner);
                }
                '"' => {
                    let mut w = cur.take().unwrap_or(Word {
                        start: here,
                        ..Word::default()
                    });
                    w.quoted = true;
                    self.pos += 1;
                    while let Some(c) = self.peek(0) {
                        match c {
                            '"' => {
                                self.pos += 1;
                                break;
                            }
                            '\\' => {
                                self.pos += 1;
                                if let Some(n) = self.peek(0) {
                                    if !"$`\"\\\n".contains(n) {
                                        w.text.push('\\');
                                    }
                                    w.text.push(n);
                                    self.pos += 1;
                                }
                            }
                            '$' => self.dollar(&mut w, true),
                            '`' => {
                                self.pos += 1;
                                let (inner, s, e) = self.until('`');
                                w.text.push_str(&inner);
                                w.subs.push((inner, s, e));
                            }
                            _ => {
                                w.text.push(c);
                                self.pos += 1;
                            }
                        }
                    }
                    cur = Some(w);
                }
                '$' => {
                    let mut w = cur.take().unwrap_or(Word {
                        start: here,
                        ..Word::default()
                    });
                    self.dollar(&mut w, false);
                    cur = Some(w);
                }
                '`' => {
                    let w = cur.get_or_insert_with(|| Word { start: here, ..Word::default() });
                    self.pos += 1;
                    let (inner, s, e) = self.until('`');
                    w.text.push_str(&inner);
                    w.subs.push((inner, s, e));
                }
                ';' | '&' | '|' | '\n' | '(' | ')' | '{' | '}'
                    if !matches!(c, '{' | '}') || cur.is_none() =>
                {
                    if c == '&' && self.peek(1) == Some('>') {
                        flush(&mut cur, &mut out, here);
                        self.pos += 2;
                        let op = if self.peek(0) == Some('>') {
                            self.pos += 1;
                            "&>>"
                        } else {
                            "&>"
                        };
                        out.push(Tok::Redir(op.to_owned(), here));
                        continue;
                    }
                    flush(&mut cur, &mut out, here);
                    let two: String = [Some(c), self.peek(1)].iter().flatten().collect();
                    let op = if ["&&", "||", ";;", "|&"].contains(&two.as_str()) {
                        self.pos += 2;
                        two
                    } else {
                        self.pos += 1;
                        c.to_string()
                    };
                    out.push(Tok::Op(op));
                }
                '>' | '<' => {
                    let fd_prefix = cur
                        .as_ref()
                        .is_some_and(|w| !w.quoted && !w.text.is_empty() && w.text.chars().all(|d| d.is_ascii_digit()));
                    let start = if fd_prefix {
                        cur.take().map_or(here, |w| w.start)
                    } else {
                        flush(&mut cur, &mut out, here);
                        here
                    };
                    let mut op = c.to_string();
                    self.pos += 1;
                    while let Some(n) = self.peek(0) {
                        if (n == '>' && c == '>') || n == '&' || n == '|' || (n == '<' && c == '<') {
                            op.push(n);
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    out.push(Tok::Redir(op, start));
                }
                _ => {
                    let w = cur.get_or_insert_with(|| Word { start: here, ..Word::default() });
                    w.text.push(c);
                    self.pos += 1;
                }
            }
        }
        flush(&mut cur, &mut out, self.src.len());
        out
    }
}

pub(crate) fn parse(src: &str) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    let mut cmd = Cmd::default();
    let mut pending: Option<(String, usize)> = None;
    for tok in Lexer::new(src).tokens() {
        match tok {
            Tok::Word(w) => {
                if let Some((op, start)) = pending.take() {
                    cmd.redirs.push(Redir {
                        op,
                        target: Some(w),
                        start,
                    });
                } else {
                    cmd.words.push(w);
                }
            }
            Tok::Redir(op, start) => {
                if let Some((op, start)) = pending.take() {
                    cmd.redirs.push(Redir { op, target: None, start });
                }
                pending = Some((op, start));
            }
            Tok::Op(op) => {
                if let Some((op, start)) = pending.take() {
                    cmd.redirs.push(Redir { op, target: None, start });
                }
                let piped = op == "|" || op == "|&";
                if !cmd.words.is_empty() || !cmd.redirs.is_empty() {
                    cmds.push(std::mem::take(&mut cmd));
                }
                cmd.piped_from = piped;
            }
        }
    }
    if let Some((op, start)) = pending.take() {
        cmd.redirs.push(Redir { op, target: None, start });
    }
    if !cmd.words.is_empty() || !cmd.redirs.is_empty() {
        cmds.push(cmd);
    }
    cmds
}

pub(crate) fn parse_nested(src: &str, offset: usize, depth: usize, out: &mut Vec<Cmd>) {
    if depth > MAX_DEPTH {
        return;
    }
    for mut cmd in parse(src) {
        let mut inner = Vec::new();
        for w in &mut cmd.words {
            w.start += offset;
            w.end += offset;
            for (sub, s, _) in &w.subs {
                inner.push((sub.clone(), *s + offset));
            }
        }
        for r in &mut cmd.redirs {
            r.start += offset;
            if let Some(t) = r.target.as_mut() {
                t.start += offset;
                t.end += offset;
            }
        }
        out.push(cmd);
        for (sub, at) in inner {
            parse_nested(&sub, at, depth + 1, out);
        }
    }
}
