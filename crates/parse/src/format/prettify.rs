use core::iter;

use crate::{
    Result,
    ast::{ObjectEntries, Value},
    format::{Emitter, LineEnding},
    tokens::{FALSE, NULL, TRUE, TokenStream},
    traverse::{Visitor, parse_tokens},
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
            Event::ArrayOpen => self.emit_array_open(),
            Event::ArrayClose => self.emit_array_close(),
            Event::ObjectOpen => self.emit_object_open(),
            Event::ObjectClose => self.emit_object_close(),
            Event::Null => self.emit_null(),
            Event::String(s) | Event::ObjectKey(s) => self.emit_string(s),
            Event::Number(cow) => self.emit_number(&cow),
            Event::Boolean(b) => self.emit_boolean(b),
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
        match event {
            Event::ArrayOpen => self.emit_array_open(),
            Event::ArrayClose => self.emit_array_close(),
            Event::ObjectOpen => {
                self.emit_object_open();
                self.buf.write_eol();
            }
            Event::ObjectClose => {
                // TODO name consistency, all should be emit
                self.buf.write_eol();
                self.buf.write_indent(depth.saturating_sub(1));
                self.emit_object_close()
            }
            Event::Null => self.emit_null(),
            Event::String(s) => self.emit_string(s),
            Event::ObjectKey(s) => {
                self.buf.write_indent(depth);
                self.emit_string(s)
            }
            Event::Number(cow) => self.emit_number(&cow),
            Event::Boolean(b) => self.emit_boolean(b),
            Event::ItemDelim => {
                self.emit_item_delim();
                self.buf.write_eol();
            }
            Event::KeyValDelim => self.emit_key_val_delim(),
        }
    }
}
struct PrettifyEmitVisitor<'a> {
    frames: VecDeque<Frame<'a>>,
    expansion: Vec<Option<FormatMode>>,
    format_buf: FormatBuf,
}

impl<'a> PrettifyEmitVisitor<'a> {
    fn new(format_buf: FormatBuf) -> Self {
        Self {
            format_buf,
            expansion: vec![],
            frames: VecDeque::new(),
        }
    }

    fn get_depth(&self) -> usize {
        self.expansion.len()
    }

    fn flush_buffer(&mut self) {
        dbg!("buffer flushed");
        // self.format_buf.buf.reserve(self.buffered_bytes);
        dbg!(&self.frames);

        // flush committed frames
        while let Some(mode) = self.frames.front().and_then(|frame| {
            combine_maybe_mode(
                frame.mode,
                self.expansion
                    .get(frame.depth.saturating_sub(1))
                    .copied()
                    .flatten(),
            )
        }) {
            let frame = self.frames.pop_front().unwrap();

            for event in frame.events.into_iter() {
                dbg!(&event);
                dbg!(&self.expansion);
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

    /// Push an event into the current buffer and update remaining width and buffered byte count.
    /// assumes well formed events
    fn on_event(&mut self, event: Event<'a>) {
        match event {
            Event::ObjectOpen => {
                self.expansion.push(None);
                self.push_frame(event);
            }
            // if we see an open key, commit to formatting expanded
            Event::ObjectKey(_) => {
                let frame = self.frames.back_mut().expect("should have an open key");
                frame.mode = Some(FormatMode::Expanded);
                self.expansion[frame.depth.saturating_sub(1)] = Some(FormatMode::Expanded);
                let len = self.format_buf.event_len(&event);
                frame.push(event, len);

                self.flush_buffer();
            }
            Event::ObjectClose => {
                let frame = if let Some(frame) = self.frames.back_mut() {
                    let last_event = frame.events.last();
                    frame.mode = Some(if last_event == Some(&Event::ObjectOpen) {
                        FormatMode::Compact
                    } else {
                        FormatMode::Expanded
                    });
                    // TODO helper
                    let len = self.format_buf.event_len(&event);
                    frame.push(event, len);
                    frame
                } else {
                    self.push_frame(event.clone());
                    self.frames.back_mut().unwrap()
                };
                frame.mode = Some(frame.mode.unwrap_or(FormatMode::Compact));

                self.flush_buffer();

                self.expansion.pop();
            }
            Event::ArrayClose | Event::ArrayOpen => unimplemented!(),
            _ => {
                if let Some(frame) = self.frames.back_mut() {
                    let len = self.format_buf.event_len(&event);
                    frame.push(event, len);
                } else {
                    self.push_frame(event);
                }
            }
        }
    }

    pub fn finish(mut self) -> String {
        dbg!("finished");
        self.flush_buffer();
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
    ArrayOpen,
    ArrayClose,
    ObjectOpen,
    ObjectKey(&'a str),
    ObjectClose,
    Null,
    String(&'a str),
    Number(Cow<'a, str>),
    Boolean(bool),
    ItemDelim,
    KeyValDelim,
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
    fn root(event: Event<'a>, depth: usize) -> Self {
        Frame {
            events: vec![event], // TODO sub the len :(
            depth,
            ..Default::default()
        }
    }

    fn nested(&self, event: Event<'a>) -> Self {
        Frame {
            depth: self.depth + 1,
            bytes_available: self.bytes_available,
            events: vec![event],
            ..Default::default()
        }
    }

    fn push(&mut self, event: Event<'a>, inline_width: usize) -> usize {
        self.events.push(event);
        self.bytes_available = self.bytes_available.saturating_sub(inline_width);
        self.bytes_available
    }
}

// hmmmmm
// if buffering and found a non empty object, then flush buffer
// if buffering arr, sub remaining width, if over, then sub width
// how to reset frame's remaining width???????
// maybe store a smallvec/stack of the inline width per item
/*
{
    "hi": [1,2,"pretend200long"]
}
[Indent(4), KeyPlusPadding(6), Arr(200)] // when hits too long, expand output for all

{
    "hi": [1,2,[1]]
}
[Indent(4), KeyPlusPadding(6), Arr(4)] // when hits end and below preferred width, emit all collapsed
*/
impl<'a> Visitor<'a> for PrettifyEmitVisitor<'a> {
    fn on_array_open(&mut self) {
        self.on_event(Event::ArrayOpen);
    }

    fn on_array_close(&mut self) {
        self.on_event(Event::ArrayClose);
    }

    fn on_object_open(&mut self) {
        self.on_event(Event::ObjectOpen);
    }

    fn on_object_close(&mut self) {
        self.on_event(Event::ObjectClose);
    }

    fn on_null(&mut self) {
        self.on_event(Event::Null);
    }

    fn on_string(&mut self, s: &'a str) {
        self.on_event(Event::String(s));
    }

    fn on_number(&mut self, n: Cow<'a, str>) {
        self.on_event(Event::Number(n));
    }

    fn on_boolean(&mut self, b: bool) {
        self.on_event(Event::Boolean(b));
    }

    fn on_item_delim(&mut self) {
        self.on_event(Event::ItemDelim);
    }

    fn on_object_key_val_delim(&mut self) {
        self.on_event(Event::KeyValDelim);
    }

    fn on_object_key(&mut self, key: &'a str) {
        self.on_event(Event::ObjectKey(key));
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
    fn push_quoted(&mut self, value: &str) {
        self.push('"');
        self.push_str(value);
        self.push('"');
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
            ObjectClose | ArrayOpen | ArrayClose | ObjectOpen => 1,
            Null => NULL.len(),
            ObjectKey(s) | String(s) => Self::quoted_len(s),
            Number(n) => n.len(),
            Boolean(b) => (if *b { TRUE } else { FALSE }).len(),
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
    parse_tokens(&mut TokenStream::new(json), json, true, &mut visitor)?;

    Ok(visitor.finish())
}

/// writes formatted delimiters between formatted items
///
/// avoids allocating intermediate `String`s declaratively
/// # Examples
/// ```
/// # use jjpwrgem_parse::format::join_into;
/// # use std::fmt::Write as _;
///
/// let mut buf = String::new();
/// join_into(&mut buf, [1,2,3,4],
///     |buf, x| write!(buf, "{}", x * 2).unwrap(),
///     |buf, _| write!(buf, ",").unwrap(),
/// );
/// assert_eq!(buf, "2,4,6,8");
/// ```
pub fn join_into<T, B>(
    buf: &mut B,
    items: impl IntoIterator<Item = T>,
    mut item_fmt: impl FnMut(&mut B, &T),
    mut delim_fmt: impl FnMut(&mut B, &T),
) {
    let mut iter = items.into_iter();
    if let Some(first) = iter.next() {
        item_fmt(buf, &first);
        for item in iter {
            delim_fmt(buf, &item);
            item_fmt(buf, &item);
        }
    }
}

fn format_value_into(buf: &mut FormatBuf, val: &Value, depth: usize) {
    match val {
        Value::Null => buf.push_str(NULL),
        Value::String(s) => buf.push_quoted(s),
        Value::Number(s) => buf.push_str(s.as_ref()),
        Value::Object(entries) if entries.0.is_empty() => buf.push_str("{}"),
        Value::Object(entries) => {
            expanded_format_object_into(buf, entries, depth);
        }
        Value::Array(items) if items.is_empty() => buf.push_str("[]"),
        Value::Array(items) => {
            if len::should_expand(val, buf.available_bytes()) {
                expanded_format_arr_into(buf, items, depth)
            } else {
                compact_format_arr_into(buf, items, depth);
            }
        }
        Value::Boolean(b) => buf.push_str(if *b { TRUE } else { FALSE }),
    }
}

fn expanded_format_object_into(buf: &mut FormatBuf, entries: &ObjectEntries, depth: usize) {
    buf.push('{');
    buf.write_eol();
    join_into(
        buf,
        entries.0.iter(),
        |buf, (key, val)| {
            buf.write_indent(depth + 1);
            buf.push_quoted(key);
            buf.push(':');
            buf.write_key_val_delimiter();
            format_value_into(buf, val, depth + 1);
        },
        |buf, _| {
            buf.push(',');
            buf.write_eol();
        },
    );
    buf.write_eol();
    buf.write_indent(depth);
    buf.push('}');
}

fn expanded_format_arr_into(buf: &mut FormatBuf, items: &[Value], depth: usize) {
    buf.push('[');
    buf.write_eol();
    join_into(
        buf,
        items,
        |buf, val| {
            buf.write_indent(depth + 1);
            format_value_into(buf, val, depth + 1)
        },
        |buf, _| {
            buf.push(',');
            buf.write_eol();
        },
    );
    buf.write_eol();
    buf.write_indent(depth);
    buf.push(']');
}

fn compact_format_arr_into(buf: &mut FormatBuf, items: &[Value], depth: usize) {
    buf.push('[');
    join_into(
        buf,
        items,
        |buf, val| format_value_into(buf, val, depth + 1),
        |buf, _| {
            buf.push(',');
            buf.write_key_val_delimiter();
        },
    );
    buf.push(']');
}

pub fn format_value(val: &Value, options: &FormatOptions, preferred_width: usize) -> String {
    let mut buf = FormatBuf::new(String::new(), *options, preferred_width);
    format_value_into(&mut buf, val, 0);
    buf.into_inner()
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

mod len {
    use crate::{
        ast::Value,
        tokens::{FALSE, NULL, TRUE},
    };

    /// returns if the inline length of the value > limit or it finds a newline
    pub fn should_expand(val: &Value, limit: usize) -> bool {
        try_get_value_len(val, limit).is_none()
    }

    fn try_get_value_len(val: &Value<'_>, limit: usize) -> Option<usize> {
        fn within_limit(len: usize, limit: usize) -> bool {
            len <= limit
        }

        let len = match val {
            Value::Null => NULL.len(),
            Value::String(s) => s.len(),
            Value::Number(s) => s.len(),
            Value::Object(entries) => {
                if entries.is_empty() {
                    2
                } else {
                    return None; // will have a newline
                }
            }
            Value::Array(values) => {
                let brackets_len = 2;
                let mut sum = brackets_len;
                let mut values = values.iter();
                while let Some(value) = values.next()
                    && within_limit(sum, limit)
                {
                    let remaining = limit.saturating_sub(sum);
                    let len = try_get_value_len(value, remaining)?;
                    sum += len;
                }

                sum
            }
            Value::Boolean(b) => {
                if *b {
                    TRUE.len()
                } else {
                    FALSE.len()
                }
            }
        };

        within_limit(len, limit).then_some(len)
    }
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
}
