//! Text actions: placeholder templates and short human labels.

use std::borrow::Cow;
use std::fmt::Write as _;

use m2k_midi::names::note_name;

use super::cx::Ctx;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Lit(String),
    Value,
    Note,
    NoteName,
    Velocity,
    Channel,
    Controller,
}

/// Text with the placeholders `{value}`, `{note}`, `{note_name}`, `{velocity}`, `{channel}` and
/// `{controller}`, parsed once when the config is compiled. Unknown `{...}` stay literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextTemplate {
    parts: Vec<Part>,
}

impl TextTemplate {
    pub fn parse(text: &str) -> Self {
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            literal.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let placeholder = after.find('}').and_then(|close| {
                let part = match &after[..close] {
                    "value" => Part::Value,
                    "note" => Part::Note,
                    "note_name" => Part::NoteName,
                    "velocity" => Part::Velocity,
                    "channel" => Part::Channel,
                    "controller" => Part::Controller,
                    _ => return None,
                };
                Some((part, close))
            });
            match placeholder {
                Some((part, close)) => {
                    if !literal.is_empty() {
                        parts.push(Part::Lit(std::mem::take(&mut literal)));
                    }
                    parts.push(part);
                    rest = &after[close + 1..];
                }
                None => {
                    literal.push('{');
                    rest = after;
                }
            }
        }
        literal.push_str(rest);
        if !literal.is_empty() {
            parts.push(Part::Lit(literal));
        }
        TextTemplate { parts }
    }

    /// Characters of the literal parts plus a few per placeholder (for time estimates).
    pub fn approx_chars(&self) -> usize {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Lit(text) => text.chars().count(),
                _ => 3,
            })
            .sum()
    }

    /// `true` when the text contains no placeholder.
    pub fn is_literal(&self) -> bool {
        self.parts.iter().all(|p| matches!(p, Part::Lit(_)))
    }

    pub fn render(&self, ctx: &Ctx, middle_c_octave: i8) -> Cow<'_, str> {
        match self.parts.as_slice() {
            [] => return Cow::Borrowed(""),
            [Part::Lit(text)] => return Cow::Borrowed(text),
            _ => {}
        }
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Lit(text) => out.push_str(text),
                Part::Value => {
                    let _ = write!(out, "{}", ctx.value);
                }
                Part::Note => {
                    if let Some(note) = ctx.note {
                        let _ = write!(out, "{note}");
                    }
                }
                Part::NoteName => {
                    if let Some(note) = ctx.note {
                        out.push_str(&note_name(note, middle_c_octave));
                    }
                }
                Part::Velocity => {
                    if let Some(velocity) = ctx.velocity {
                        let _ = write!(out, "{velocity}");
                    }
                }
                Part::Channel => {
                    if ctx.channel > 0 {
                        let _ = write!(out, "{}", ctx.channel);
                    }
                }
                Part::Controller => {
                    if let Some(controller) = ctx.controller {
                        let _ = write!(out, "{controller}");
                    }
                }
            }
        }
        Cow::Owned(out)
    }
}

/// `type "hello"` (shortened, control characters escaped).
pub fn type_label(text: &str) -> String {
    format!("type {:?}", shorten(text, 40))
}

/// At most `max` characters, with an ellipsis when cut.
pub fn shorten(text: &str, max: usize) -> Cow<'_, str> {
    match text.char_indices().nth(max) {
        None => Cow::Borrowed(text),
        Some((cut, _)) => Cow::Owned(format!("{}…", &text[..cut])),
    }
}
