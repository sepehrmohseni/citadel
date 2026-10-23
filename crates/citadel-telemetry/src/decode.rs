use std::collections::HashMap;

use crate::script::MARK;

const MAX_PENDING: usize = 1 << 20;
const MAX_SECTION_LINES: usize = 16_384;
const MAX_SECTIONS: usize = 32;
const MAX_FRAME_BYTES: usize = 8 << 20;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RawFrame {
    pub full: bool,
    pub docker_mode: Option<String>,
    sections: HashMap<String, Vec<String>>,
}

impl RawFrame {
    pub fn section(&self, name: &str) -> &[String] {
        self.sections.get(name).map_or(&[], Vec::as_slice)
    }

    pub fn first(&self, name: &str) -> Option<&str> {
        self.section(name)
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
    }
}

#[derive(Debug, Default)]
pub(crate) struct Decoder {
    pending: Vec<u8>,
    cur: Option<RawFrame>,
    section: Option<String>,
    frame_bytes: usize,
}

impl Decoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<RawFrame> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(pos) = self.pending[start..].iter().position(|&b| b == b'\n') {
            let end = start + pos;
            let line = String::from_utf8_lossy(&self.pending[start..end]).into_owned();
            start = end + 1;
            if let Some(frame) = self.line(line.trim_end_matches('\r')) {
                out.push(frame);
            }
        }
        self.pending.drain(..start);
        if self.pending.len() > MAX_PENDING {
            self.pending.clear();
        }
        out
    }

    fn line(&mut self, line: &str) -> Option<RawFrame> {
        let Some(rest) = line.strip_prefix(MARK) else {
            if let (Some(frame), Some(name)) = (self.cur.as_mut(), self.section.as_ref()) {
                let lines = frame.sections.entry(name.clone()).or_default();
                if lines.len() < MAX_SECTION_LINES
                    && self.frame_bytes + line.len() <= MAX_FRAME_BYTES
                {
                    self.frame_bytes += line.len();
                    lines.push(line.to_owned());
                }
            }
            return None;
        };
        let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        match name {
            "frame" => {
                self.cur = Some(RawFrame {
                    full: arg == "full",
                    ..RawFrame::default()
                });
                self.section = None;
                self.frame_bytes = 0;
                None
            }
            "end" => {
                self.section = None;
                self.cur.take()
            }
            _ => {
                let frame = self.cur.as_mut()?;
                if frame.sections.len() >= MAX_SECTIONS && !frame.sections.contains_key(name) {
                    self.section = None;
                    return None;
                }
                if name == "docker" {
                    frame.docker_mode = Some(arg.to_owned());
                }
                frame.sections.entry(name.to_owned()).or_default();
                self.section = Some(name.to_owned());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: &str = "motd noise\n@@citadel:frame full\n@@citadel:hostname\nweb-01\n@@citadel:stat\ncpu  1 2 3 4\ncpu0 1 2 3 4\n@@citadel:docker absent\n@@citadel:end\ntrailing junk\n";

    #[test]
    fn whole_frame_decodes() {
        let mut dec = Decoder::default();
        let frames = dec.push(FRAME.as_bytes());
        assert_eq!(frames.len(), 1);
        let f = &frames[0];
        assert!(f.full);
        assert_eq!(f.first("hostname"), Some("web-01"));
        assert_eq!(f.section("stat").len(), 2);
        assert_eq!(f.docker_mode.as_deref(), Some("absent"));
        assert!(f.section("docker").is_empty());
        assert!(f.section("missing").is_empty());
    }

    #[test]
    fn byte_at_a_time_matches_whole() {
        let mut whole = Decoder::default();
        let expect = whole.push(FRAME.as_bytes());
        let mut dec = Decoder::default();
        let mut got = Vec::new();
        for b in FRAME.as_bytes() {
            got.extend(dec.push(std::slice::from_ref(b)));
        }
        assert_eq!(got, expect);
    }

    #[test]
    fn split_utf8_and_crlf_survive() {
        let text = "@@citadel:frame full\r\n@@citadel:hostname\r\nh\u{f6}st-\u{1f680}\r\n@@citadel:end\r\n";
        let bytes = text.as_bytes();
        let cut = text.find('\u{1f680}').unwrap() + 2;
        let mut dec = Decoder::default();
        assert!(dec.push(&bytes[..cut]).is_empty());
        let frames = dec.push(&bytes[cut..]);
        assert_eq!(frames[0].first("hostname"), Some("h\u{f6}st-\u{1f680}"));
    }

    #[test]
    fn unterminated_frame_is_discarded_by_next_frame() {
        let mut dec = Decoder::default();
        let frames = dec.push(b"@@citadel:frame full\n@@citadel:hostname\nstale\n@@citadel:frame prime\n@@citadel:stat\ncpu 1 1 1 1\n@@citadel:end\n");
        assert_eq!(frames.len(), 1);
        assert!(!frames[0].full);
        assert!(frames[0].section("hostname").is_empty());
    }

    #[test]
    fn lines_outside_frames_and_sections_are_ignored() {
        let mut dec = Decoder::default();
        let frames = dec.push(
            b"@@citadel:stat\ncpu 9\n@@citadel:end\n@@citadel:frame full\norphan\n@@citadel:end\n",
        );
        assert_eq!(frames.len(), 1);
        assert!(frames[0].section("stat").is_empty());
    }

    #[test]
    fn runaway_line_is_dropped() {
        let mut dec = Decoder::default();
        assert!(dec.push(&vec![b'x'; MAX_PENDING + 1]).is_empty());
        assert!(dec.pending.is_empty());
        let frames = dec.push(b"\n@@citadel:frame full\n@@citadel:end\n");
        assert_eq!(frames.len(), 1);
    }
}
