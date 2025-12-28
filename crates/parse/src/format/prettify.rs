use core::iter;

use crate::{
    Result,
    ast::Value,
    format::{Emitter, LineEnding},
    tokens::{FALSE, NULL, TRUE},
    traverse::{Visitor, parse_tokens, parse_value},
};
use std::borrow::Cow;
use std::collections::VecDeque;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct FormatOptions {
    key_val_delimiter: Option<(char, usize)>,
    indent: Option<(char, usize)>,
    line_ending: LineEnding,
}

impl FormatOptions {
    pub fn new(
        key_val_delimiter: Option<(char, usize)>,
        indent: Option<(char, usize)>,
        line_ending: LineEnding,
    ) -> Self {
        Self {
            key_val_delimiter,
            indent,
            line_ending,
        }
    }

    pub fn prettify(line_ending: LineEnding) -> Self {
        Self {
            key_val_delimiter: Some((' ', 1)),
            indent: Some((' ', 2)),
            line_ending,
        }
    }
}

struct CompactEmitter<'a> {
    buf: &'a mut FormatBuf,
}
impl<'a> CompactEmitter<'a> {
    fn emit_event(&mut self, event: Event<'_>) {
        match event {
            Event::ArrayOpen { .. } => self.emit_array_open(),
            Event::ArrayClose => self.emit_array_close(),
            Event::ObjectOpen { .. } => self.emit_object_open(),
            Event::ObjectClose => self.emit_object_close(),
            Event::Null { .. } => self.emit_null(),
            Event::String { s, .. } => self.emit_string(s),
            Event::ObjectKey { key } => self.emit_string(key),
            Event::Number { n, .. } => self.emit_number(&n),
            Event::Boolean { value, .. } => self.emit_boolean(value),
            Event::ItemDelim => self.emit_item_delim(),
            Event::KeyValDelim => self.emit_key_val_delim(),
        }
    }
}
impl Emitter for CompactEmitter<'_> {
    fn push(&mut self, c: char) {
        self.buf.push(c);
    }

    fn push_str(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    fn emit_key_val_delim(&mut self) {
        self.buf.push(':');
        self.buf.write_key_val_delimiter();
    }

    fn emit_item_delim(&mut self) {
        self.buf.push(',');
        self.buf.push(' ');
    }
}
struct ExpandedEmitter<'a> {
    buf: &'a mut FormatBuf,
}
impl Emitter for ExpandedEmitter<'_> {
    fn push(&mut self, c: char) {
        self.buf.push(c);
    }

    fn push_str(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    fn emit_key_val_delim(&mut self) {
        self.buf.push(':');
        self.buf.write_key_val_delimiter();
    }
}
impl<'a> ExpandedEmitter<'a> {
    fn emit_event(&mut self, event: Event<'_>, depth: usize) {
        if event.is_array_value() {
            let depth = if event.is_scalar() {
                depth
            } else {
                depth.saturating_sub(1)
            };
            self.buf.write_indent(depth);
        }
        match event {
            Event::ArrayOpen { .. } => {
                self.emit_array_open();
                self.buf.write_eol();
            }
            Event::ObjectOpen { .. } => {
                self.emit_object_open();
                self.buf.write_eol();
            }
            Event::ArrayClose => {
                self.buf.write_eol();
                self.buf.write_indent(depth.saturating_sub(1));
                self.emit_array_close()
            }
            Event::ObjectClose => {
                // TODO name consistency, all should be emit
                self.buf.write_eol();
                self.buf.write_indent(depth.saturating_sub(1));
                self.emit_object_close()
            }
            Event::Null { .. } => self.emit_null(),
            Event::String { s, .. } => self.emit_string(s),
            Event::ObjectKey { key } => {
                self.buf.write_indent(depth);
                self.emit_string(key)
            }
            Event::Number { n, .. } => self.emit_number(&n),
            Event::Boolean { value, .. } => self.emit_boolean(value),
            Event::ItemDelim => {
                self.emit_item_delim();
                self.buf.write_eol();
            }
            Event::KeyValDelim => self.emit_key_val_delim(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Level {
    mode: Option<FormatMode>,
}
struct PrettifyEmitVisitor<'a> {
    frames: VecDeque<Frame<'a>>,
    levels: Vec<Level>,
    root_array_level: Option<usize>,
    format_buf: FormatBuf,
}

impl<'a> PrettifyEmitVisitor<'a> {
    fn new(format_buf: FormatBuf) -> Self {
        Self {
            format_buf,
            levels: vec![],
            frames: VecDeque::new(),
            root_array_level: None,
        }
    }

    fn get_depth(&self) -> usize {
        self.levels.len()
    }

    fn flush_buffer(&mut self) {
        // println!("buffer flushed");
        // println!();
        // self.format_buf.buf.reserve(self.buffered_bytes);
        // dbg!(&self.frames);
        // dbg!(&self.levels);

        // flush committed frames
        while let Some(mode) = self.frames.front().and_then(|frame| {
            combine_maybe_mode(
                frame.mode,
                self.levels
                    .get(frame.depth.saturating_sub(1))
                    .and_then(|&Level { mode, .. }| mode),
            )
        }) {
            // dbg!(mode);
            let frame = self.frames.pop_front().unwrap();
            // dbg!(frame.depth);

            for event in frame.events.into_iter() {
                // dbg!(&event);
                // TODO maybe better to have a general emitter that has two branches???
                // expansion stack should take precendence over frame
                match mode {
                    FormatMode::Expanded => {
                        ExpandedEmitter {
                            buf: &mut self.format_buf,
                        }
                        .emit_event(event, frame.depth);
                    }
                    FormatMode::Compact => {
                        // TODO new impl or fancy function
                        CompactEmitter {
                            buf: &mut self.format_buf,
                        }
                        .emit_event(event);
                    }
                }
            }
        }
    }

    fn push_frame(&mut self, event: Event<'a>) {
        let mut frame = Frame {
            events: vec![],
            depth: self.get_depth(), // snapshot depth
            bytes_available: self.format_buf.available_bytes(), // is this right here? can we assume everything has been flushed all the way for this line??????
            ..Frame::default()
        };

        let len = self.format_buf.event_len(&event);
        frame.push(event, len);

        self.frames.push_back(frame);
    }

    /// decides whether event should be in same frame or not
    fn push_event(&mut self, event: Event<'a>) {
        let Some(last_frame) = self.frames.back_mut() else {
            self.push_frame(event);
            return;
        };

        // TODO if statement
        match event.clone() {
            Event::ArrayOpen { .. } | Event::ObjectOpen { .. } | Event::ArrayClose => {
                self.push_frame(event);
            }
            _ if event.is_object() && last_frame.events.iter().any(|x| x.is_array()) => {
                self.push_frame(event);
            }
            _ if event.is_array() && last_frame.events.iter().any(|x| x.is_object()) => {
                self.push_frame(event);
            }
            _ => {
                let len = self.format_buf.event_len(&event);
                last_frame.push(event, len);
            }
        }
    }

    fn push_level(&mut self, is_arr_open: bool) {
        self.levels.push(Level { mode: None });
        if is_arr_open && self.root_array_level.is_none() {
            self.root_array_level = Some(self.get_depth());
        }
    }

    fn pop_level(&mut self) {
        if self.root_array_level == Some(self.get_depth()) {
            self.root_array_level.take();
        }
        self.levels.pop();
    }

    fn get_level_mode(&self) -> Option<FormatMode> {
        self.levels
            .get(self.get_depth().saturating_sub(1))
            .and_then(|x| x.mode)
    }

    /// Push an event into the current buffer and update remaining width and buffered byte count.
    /// assumes well formed events
    fn on_event(&mut self, event: Event<'a>) {
        match event {
            Event::ObjectOpen { .. } => {
                self.push_level(false);
                self.push_event(event);
            }
            Event::ArrayOpen { .. } => {
                self.push_level(true);
                self.push_event(event);
            }
            // if we see an open key, commit to formatting expanded all the way up
            Event::ObjectKey { .. } => {
                self.push_event(event);

                let frame = self.frames.back_mut().unwrap();
                frame.mode = Some(FormatMode::Expanded);

                for level in &mut self.levels.iter_mut().take(frame.depth - 1) {
                    level.mode = Some(FormatMode::Expanded)
                }

                self.flush_buffer();
            }
            Event::ObjectClose => {
                self.push_event(event);
                let frame = self.frames.back_mut().expect("just pushed an event");
                // if mode isn't committed, no keys are here and make it compact
                frame.mode = Some(frame.mode.unwrap_or(FormatMode::Compact));

                self.flush_buffer();

                self.pop_level();
            }
            Event::ArrayClose => {
                self.push_event(event);

                let is_root = Some(self.get_depth()) == self.root_array_level;
                if is_root {
                    let start = self.root_array_level.unwrap();
                    let mode = combine_maybe_mode(
                        Some(FormatMode::Compact),
                        self.levels.get(start).and_then(|x| x.mode),
                    );

                    for sub_frame in &mut self.frames.iter_mut().skip(start - 1) {
                        sub_frame.mode = mode;
                    }

                    self.flush_buffer();
                } else {
                    let level_mode = self.get_level_mode();
                    let frame = self.frames.back_mut().expect("just pushed");

                    frame.mode = level_mode;

                    if let Some(level) = self.levels.get_mut(frame.depth - 1) {
                        level.mode = frame.mode;
                    }
                }

                // TODO how can I track depth if I can't
                self.pop_level();

                // how to know when to flush?
                // if we go over of course
                // but also after rootmost array
            }
            Event::Boolean { .. }
            | Event::Null { .. }
            | Event::Number { .. }
            | Event::String { .. } => {
                let depth = self.get_depth();
                self.push_event(event);
                if depth == 0 {
                    let frame = self.frames.back_mut().unwrap();
                    frame.mode = Some(FormatMode::Compact);
                }
            }
            _ => {
                self.push_event(event);
            }
        };
        // if too wide, set all expansion stack to Expanded
        if self.frames.back().is_none_or(|x| x.bytes_available == 0) {
            for level in &mut self.levels {
                level.mode = Some(FormatMode::Expanded)
            }
            self.flush_buffer();
        }
    }

    pub fn finish(mut self) -> String {
        self.flush_buffer();
        // debug_assert!(self.frames.is_empty());
        self.format_buf.buf
    }
}

impl Emitter for PrettifyEmitVisitor<'_> {
    fn push(&mut self, c: char) {
        self.format_buf.push(c);
    }

    fn push_str(&mut self, s: &str) {
        self.format_buf.push_str(s);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event<'a> {
    ArrayOpen {
        is_array_value: bool,
    },
    ArrayClose,
    ObjectOpen {
        is_array_value: bool,
    },
    ObjectKey {
        key: &'a str,
    },
    ObjectClose,
    Null {
        is_array_value: bool,
    },
    String {
        s: &'a str,
        is_array_value: bool,
    },
    Number {
        n: Cow<'a, str>,
        is_array_value: bool,
    },
    Boolean {
        value: bool,
        is_array_value: bool,
    },
    ItemDelim,
    KeyValDelim,
}

impl<'a> Event<'a> {
    fn is_array_value(&self) -> bool {
        match self {
            Event::ArrayOpen { is_array_value }
            | Event::ObjectOpen { is_array_value }
            | Event::Null { is_array_value }
            | Event::String { is_array_value, .. }
            | Event::Number { is_array_value, .. }
            | Event::Boolean { is_array_value, .. } => *is_array_value,
            _ => false,
        }
    }

    fn is_scalar(&self) -> bool {
        matches!(
            self,
            Event::Null { .. }
                | Event::String { .. }
                | Event::Number { .. }
                | Event::Boolean { .. }
        )
    }

    fn is_array(&self) -> bool {
        matches!(self, Event::ArrayOpen { .. } | Event::ArrayClose)
    }

    fn is_object(&self) -> bool {
        matches!(
            self,
            Event::ObjectOpen { .. }
                | Event::ObjectClose
                | Event::ObjectKey { .. }
                | Event::KeyValDelim
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatMode {
    Compact,
    Expanded,
}

impl FormatMode {
    /// chains 2 modes
    /// returns `self` when `Expanded`, else returns `other`
    fn then(self, other: Self) -> Self {
        match self {
            FormatMode::Compact => other,
            FormatMode::Expanded => self,
        }
    }
}

fn combine_maybe_mode(x: Option<FormatMode>, y: Option<FormatMode>) -> Option<FormatMode> {
    x.zip(y).map(|(x, y)| x.then(y)).or(x).or(y)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Frame<'a> {
    mode: Option<FormatMode>,
    parent_mode: Option<FormatMode>,
    events: Vec<Event<'a>>,
    depth: usize,
    bytes_available: usize,
}

impl<'a> Frame<'a> {
    fn push(&mut self, event: Event<'a>, inline_width: usize) -> usize {
        self.events.push(event);
        self.bytes_available = self.bytes_available.saturating_sub(inline_width);
        self.bytes_available
    }
}

impl<'a> Visitor<'a> for PrettifyEmitVisitor<'a> {
    fn on_array_open(&mut self, is_array_value: bool) {
        self.on_event(Event::ArrayOpen { is_array_value });
    }

    fn on_array_close(&mut self) {
        self.on_event(Event::ArrayClose);
    }

    fn on_object_open(&mut self, is_array_value: bool) {
        self.on_event(Event::ObjectOpen { is_array_value });
    }

    fn on_object_close(&mut self) {
        self.on_event(Event::ObjectClose);
    }

    fn on_null(&mut self, is_array_value: bool) {
        self.on_event(Event::Null { is_array_value });
    }

    fn on_string(&mut self, s: &'a str, is_array_value: bool) {
        self.on_event(Event::String { s, is_array_value });
    }

    fn on_number(&mut self, n: Cow<'a, str>, is_array_value: bool) {
        self.on_event(Event::Number { n, is_array_value });
    }

    fn on_boolean(&mut self, b: bool, is_array_value: bool) {
        self.on_event(Event::Boolean {
            value: b,
            is_array_value,
        });
    }

    fn on_item_delim(&mut self) {
        self.on_event(Event::ItemDelim);
    }

    fn on_object_key_val_delim(&mut self) {
        self.on_event(Event::KeyValDelim);
    }

    fn on_object_key(&mut self, key: &'a str) {
        self.on_event(Event::ObjectKey { key });
    }
}

struct FormatBuf {
    opts: FormatOptions,
    buf: String,
    line_start: usize,
    preferred_width: usize,
}

impl FormatBuf {
    fn new(buf: String, opts: FormatOptions, preferred_width: usize) -> Self {
        Self {
            opts,
            buf,
            line_start: 0,
            preferred_width,
        }
    }

    fn push(&mut self, value: char) {
        self.buf.push(value);
    }
    fn push_str(&mut self, value: &str) {
        self.buf.push_str(value);
    }

    #[inline]
    fn push_repeat(&mut self, c: char, count: usize) {
        self.buf.extend(iter::repeat_n(c, count));
    }

    #[inline]
    fn write_spec(&mut self, spec: Option<(char, usize)>) {
        if let Some((c, size)) = spec {
            self.push_repeat(c, size);
        }
    }

    pub fn write_key_val_delimiter(&mut self) {
        self.write_spec(self.opts.key_val_delimiter);
    }

    pub fn write_eol(&mut self) {
        self.push_str(self.opts.line_ending.as_str());
        self.line_start = self.buf.len();
    }

    pub fn write_indent(&mut self, level: usize) {
        self.write_spec(self.opts.indent.map(|(c, size)| (c, size * level)));
    }

    #[inline]
    pub fn spec_len(&self, spec: Option<(char, usize)>) -> usize {
        spec.map(|(_, size)| size).unwrap_or(0)
    }

    #[inline]
    pub fn quoted_len(value: &str) -> usize {
        2 + value.len()
    }

    #[inline]
    pub fn indent_len(&self, level: usize) -> usize {
        self.opts.indent.map(|(_, size)| size * level).unwrap_or(0)
    }

    #[inline]
    pub fn key_val_delim_len(&self) -> usize {
        // colon + optional spec
        1 + self.spec_len(self.opts.key_val_delimiter)
    }

    #[inline]
    pub fn item_delim_len(&self) -> usize {
        // comma + optional spec
        1 + self.spec_len(self.opts.key_val_delimiter)
    }

    /// Returns the length in characters for a single `Event` (not including nested contents).
    #[inline]
    pub fn event_len(&self, event: &Event<'_>) -> usize {
        use Event::*;
        match event {
            ObjectClose | ArrayOpen { .. } | ArrayClose | ObjectOpen { .. } => 1,
            Null { .. } => NULL.len(),
            ObjectKey { key } => Self::quoted_len(key),
            String { s, .. } => Self::quoted_len(s),
            Number { n, .. } => n.len(),
            Boolean { value, .. } => (if *value { TRUE } else { FALSE }).len(),
            ItemDelim => self.item_delim_len(),
            KeyValDelim => self.key_val_delim_len(),
        }
    }

    fn into_inner(self) -> String {
        self.buf
    }

    pub fn column(&self) -> usize {
        self.buf.len() - self.line_start
    }

    fn available_bytes(&self) -> usize {
        self.preferred_width.saturating_sub(self.column())
    }
}

pub fn format_str<'a>(
    json: &'a str,
    options: FormatOptions,
    preferred_width: usize,
) -> Result<'a, String> {
    let buf = FormatBuf::new(String::with_capacity(json.len()), options, preferred_width);

    let mut visitor = PrettifyEmitVisitor::new(buf);
    parse_tokens(json, &mut visitor)?;

    Ok(visitor.finish())
}

pub fn format_value(val: &Value, options: &FormatOptions, preferred_width: usize) -> String {
    let buf = FormatBuf::new(String::new(), *options, preferred_width);
    let mut visitor = PrettifyEmitVisitor::new(buf);

    parse_value(val, &mut visitor, false);

    visitor.finish()
}

pub fn prettify_str(
    json: &str,
    preferred_width: usize,
    line_ending: LineEnding,
) -> Result<'_, String> {
    format_str(json, FormatOptions::prettify(line_ending), preferred_width)
}

pub fn prettify_value(val: &Value, preferred_width: usize, line_ending: LineEnding) -> String {
    format_value(val, &FormatOptions::prettify(line_ending), preferred_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_object() {
        let json = r#"{ "expanded": null, "secondExpanded": {"third": {}}}"#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
{
  "expanded": null,
  "secondExpanded": {
    "third": {}
  }
}
"#
            .trim()
        );
    }

    #[test]
    fn formats_arr_compact() {
        let json = r#"[ "not expanded"]"#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(res, r#"["not expanded"]"#.trim());
    }
    #[test]
    fn formats_arr_expanded_due_to_inner() {
        let json = r#"[ [{"key": "expanded"}]]"#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
[
  [
    {
      "key": "expanded"
    }
  ]
]
            "#
            .trim()
        );
    }

    #[test]
    fn formats_arr_expanded_due_to_inner_0_preferred() {
        let json = r#"[ [{"key": "expanded"}]]"#;

        let res = prettify_str(json, 0, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
[
  [
    {
      "key": "expanded"
    }
  ]
]
            "#
            .trim()
        );
    }
    #[test]
    fn formats_arr_expanded_due_to_length() {
        let json = r#"["hi"]"#;

        let res = prettify_str(json, 0, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
[
  "hi"
]
            "#
            .trim()
        );
    }
    #[test]
    fn formats_arr_expanded_due_to_length_long() {
        let json = r#"[0.4e00669999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999969999999006]"#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
[
    0.4e00669999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999969999999006
]
            "#
            .trim()
        );
    }

    #[test]
    fn formats_arr_expanded_due_to_length_long_non_scalar() {
        let json = r#"[[[0.4e00669999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999969999999006, {"hi": null},[],{}]]]"#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
[
  [
    [
      0.4e00669999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999969999999006,
      {
        "hi": null
      },
      [],
      {}
    ]
  ]
]
            "#
            .trim()
        );
    }

    #[test]
    fn obj() {
        let json = r#"{}"#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
        {}
            "#
            .trim()
        );
    }

    #[test]
    fn str() {
        let json = r#"" ""#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
       " " 
            "#
            .trim()
        );
    }
    #[test]
    fn obj2() {
        let json = r#"{"hi": [], "bye": {"hi":[]}}"#;

        let res = prettify_str(json, 80, LineEnding::Lf).unwrap();

        assert_eq!(
            res,
            r#"
{
  "hi": [],
  "bye": {
      "hi": []
    }
}
            "#
            .trim()
        );
    }
}
