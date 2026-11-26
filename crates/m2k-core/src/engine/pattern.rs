//! Wildcard matching: SysEx patterns (`F0 7F ?? 06 02 F7`, `**` = any run of bytes) and
//! foreground-application rules (`*` / `?`, case-insensitive, substring when there is no
//! wildcard, empty = anything).

use m2k_output::platform::ForegroundApp;

use crate::model::AppMatcher;

/// One element of a wildcard pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok<T> {
    Lit(T),
    /// Any single element.
    One,
    /// Any run of elements, including none.
    Run,
}

/// Iterative wildcard match with backtracking to the last `Run`: linear in practice,
/// `O(pattern * text)` worst case, no allocation, no recursion.
pub fn glob<T: PartialEq>(pattern: &[Tok<T>], text: &[T]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut resume: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(Tok::Run) => {
                resume = Some((p + 1, t));
                p += 1;
                continue;
            }
            Some(Tok::One) => {
                p += 1;
                t += 1;
                continue;
            }
            Some(Tok::Lit(c)) if *c == text[t] => {
                p += 1;
                t += 1;
                continue;
            }
            _ => {}
        }
        match resume {
            Some((rp, rt)) => {
                p = rp;
                t = rt + 1;
                resume = Some((rp, rt + 1));
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|tok| matches!(tok, Tok::Run))
}

// ---------------------------------------------------------------------------------------------
// SysEx
// ---------------------------------------------------------------------------------------------

/// A parsed [`crate::model::SourceEvent::SysEx`] pattern, matched against the message body
/// (the bytes between `F0` and `F7`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysExPattern(Vec<Tok<u8>>);

impl SysExPattern {
    /// Parses space (or comma) separated hex bytes, `??` and `**`. The leading `F0` and trailing
    /// `F7` are optional. Errors are phrased for the user.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut tokens = Vec::new();
        for raw in text.split(|c: char| c.is_whitespace() || c == ',') {
            if raw.is_empty() {
                continue;
            }
            let token = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")).unwrap_or(raw);
            match token {
                "??" | "?" => tokens.push(Tok::One),
                "**" | "*" => tokens.push(Tok::Run),
                _ => {
                    let byte = (token.len() <= 2)
                        .then(|| u8::from_str_radix(token, 16).ok())
                        .flatten()
                        .ok_or_else(|| {
                            format!("\"{raw}\" is not a byte (write two hex digits, ?? or **)")
                        })?;
                    tokens.push(Tok::Lit(byte));
                }
            }
        }
        if tokens.first() == Some(&Tok::Lit(0xF0)) {
            tokens.remove(0);
        }
        if tokens.last() == Some(&Tok::Lit(0xF7)) {
            tokens.pop();
        }
        if tokens.is_empty() {
            return Err("the pattern is empty".into());
        }
        if let Some(byte) = tokens.iter().find_map(|t| match t {
            Tok::Lit(b) if *b > 0x7F => Some(*b),
            _ => None,
        }) {
            return Err(format!(
                "{byte:02X} cannot appear inside a SysEx message (data bytes are 00-7F)"
            ));
        }
        Ok(SysExPattern(tokens))
    }

    /// `data` is a complete SysEx message as delivered by the MIDI layer (with `F0`/`F7`).
    pub fn matches(&self, data: &[u8]) -> bool {
        let body = data.strip_prefix(&[0xF0]).unwrap_or(data);
        let body = body.strip_suffix(&[0xF7]).unwrap_or(body);
        glob(&self.0, body)
    }
}

// ---------------------------------------------------------------------------------------------
// Foreground application rules
// ---------------------------------------------------------------------------------------------

/// A lowercased string prepared for matching.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folded {
    text: String,
    chars: Vec<char>,
}

impl Folded {
    pub fn new(text: &str) -> Self {
        let text = text.to_lowercase();
        let chars = text.chars().collect();
        Folded { text, chars }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TextPattern {
    Any,
    Contains(String),
    Glob(Vec<Tok<char>>),
}

impl TextPattern {
    fn new(pattern: &str) -> Self {
        let pattern = pattern.trim().to_lowercase();
        if pattern.is_empty() {
            TextPattern::Any
        } else if pattern.contains(['*', '?']) {
            TextPattern::Glob(
                pattern
                    .chars()
                    .map(|c| match c {
                        '*' => Tok::Run,
                        '?' => Tok::One,
                        c => Tok::Lit(c),
                    })
                    .collect(),
            )
        } else {
            TextPattern::Contains(pattern)
        }
    }

    fn is_any(&self) -> bool {
        matches!(self, TextPattern::Any)
    }

    fn matches(&self, text: &Folded) -> bool {
        match self {
            TextPattern::Any => true,
            TextPattern::Contains(needle) => text.text.contains(needle.as_str()),
            TextPattern::Glob(tokens) => glob(tokens, &text.chars),
        }
    }
}

/// A compiled [`AppMatcher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRule {
    process: TextPattern,
    title: TextPattern,
}

impl AppRule {
    pub fn new(matcher: &AppMatcher) -> Self {
        AppRule {
            process: TextPattern::new(&matcher.process),
            title: TextPattern::new(&matcher.title),
        }
    }

    pub fn matches(&self, app: &Foreground) -> bool {
        let process = self.process.is_any()
            || self.process.matches(&app.process)
            || (!app.app_id.text.is_empty() && self.process.matches(&app.app_id));
        process && self.title.matches(&app.title)
    }
}

/// The foreground application as last observed, folded once for matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    pub app: ForegroundApp,
    process: Folded,
    title: Folded,
    app_id: Folded,
}

impl Foreground {
    pub fn new(app: ForegroundApp) -> Self {
        Foreground {
            process: Folded::new(&app.process),
            title: Folded::new(&app.title),
            app_id: Folded::new(&app.app_id),
            app,
        }
    }
}
