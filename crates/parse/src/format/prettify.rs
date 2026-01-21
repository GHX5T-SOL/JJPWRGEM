use core::iter;

use crate::{
    Result,
    ast::Value,
    format::{DepthVisitor, Emitter, LineEnding},
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

#[allow(dead_code)]
mod layout {
    use super::FormatMode;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FrameKind {
        Object,
        Array,
        Scalar,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameStats {
        pub kind: FrameKind,
        pub inline_len: usize,
        pub key_count: usize,
        pub child_expanded: bool,
        pub is_empty: bool,
        pub available_bytes: usize,
    }

    pub fn decide_layout(stats: FrameStats, preferred_width: usize) -> FormatMode {
        match stats.kind {
            FrameKind::Object => {
                if stats.is_empty {
                    FormatMode::Compact
                } else if stats.key_count > 0 {
                    FormatMode::Expanded
                } else {
                    FormatMode::Compact
                }
            }
            FrameKind::Array => {
                if stats.is_empty {
                    FormatMode::Compact
                } else if stats.child_expanded
                    || stats.inline_len > preferred_width
                    || stats.available_bytes == 0
                {
                    FormatMode::Expanded
                } else {
                    FormatMode::Compact
                }
            }
            FrameKind::Scalar => {
                if stats.inline_len > preferred_width {
                    FormatMode::Expanded
                } else {
                    FormatMode::Compact
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LayoutDecision {
        Compact,
        Expanded,
    }

    impl From<FormatMode> for LayoutDecision {
        fn from(value: FormatMode) -> Self {
            match value {
                FormatMode::Compact => Self::Compact,
                FormatMode::Expanded => Self::Expanded,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LayoutFrame {
        stats: FrameStats,
    }

    #[derive(Debug, Default)]
    pub struct LayoutTracker {
        stack: Vec<LayoutFrame>,
        preferred_width: usize,
    }

    impl LayoutTracker {
        pub fn new(preferred_width: usize) -> Self {
            Self {
                stack: Vec::new(),
                preferred_width,
            }
        }

        pub fn open_frame(&mut self, kind: FrameKind) {
            let stats = FrameStats {
                kind,
                inline_len: 0,
                key_count: 0,
                child_expanded: false,
                is_empty: true,
                available_bytes: self.preferred_width,
            };
            self.stack.push(LayoutFrame { stats });
        }

        pub fn record_inline_len(&mut self, inline_len: usize) {
            if let Some(frame) = self.stack.last_mut() {
                frame.stats.inline_len = frame.stats.inline_len.saturating_add(inline_len);
            }
        }

        pub fn mark_non_empty(&mut self) {
            if let Some(frame) = self.stack.last_mut() {
                frame.stats.is_empty = false;
            }
        }

        pub fn increment_key_count(&mut self) {
            if let Some(frame) = self.stack.last_mut() {
                frame.stats.key_count = frame.stats.key_count.saturating_add(1);
                frame.stats.is_empty = false;
            }
        }

        pub fn mark_child_expanded(&mut self) {
            if let Some(frame) = self.stack.last_mut() {
                frame.stats.child_expanded = true;
            }
        }

        pub fn close_frame(&mut self) -> LayoutDecision {
            let stats = self
                .stack
                .pop()
                .map(|frame| frame.stats)
                .unwrap_or(FrameStats {
                    kind: FrameKind::Scalar,
                    inline_len: 0,
                    key_count: 0,
                    child_expanded: false,
                    is_empty: true,
                    available_bytes: self.preferred_width,
                });
            decide_layout(stats, self.preferred_width).into()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContainerState {
    id: usize,
    kind: layout::FrameKind,
    stats: layout::FrameStats,
    mode: Option<FormatMode>,
    line_overflow: bool,
}

struct PrettifyEmitVisitor<'a> {
    frames: VecDeque<Frame<'a>>,
    containers: Vec<ContainerState>,
    next_container_id: usize,
    format_buf: FormatBuf,
    depth: DepthVisitor,
}

impl<'a> PrettifyEmitVisitor<'a> {
    fn new(format_buf: FormatBuf) -> Self {
        Self {
            format_buf,
            frames: VecDeque::new(),
            containers: Vec::new(),
            next_container_id: 1,
            depth: DepthVisitor::default(),
        }
    }

    fn flush_buffer(&mut self) {
        // println!("buffer flushed");
        // println!();
        // self.format_buf.buf.reserve(self.buffered_bytes);
        // dbg!(&self.frames);
        // dbg!(&self.levels);

        // flush committed frames
        while let Some(mode) = self.frames.front().and_then(|frame| frame.mode) {
            // dbg!(mode);
            let frame = self.frames.pop_front().unwrap();
            // dbg!(frame.depth);

            let mut indent_depth =
                if matches!(mode, FormatMode::Compact) && self.format_buf.column() == 0 {
                    frame.events.first().and_then(|first| {
                        if first.is_array_value() {
                            Some(if first.is_scalar() {
                                frame.depth
                            } else {
                                frame.depth.saturating_sub(1)
                            })
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };

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
                        if let Some(depth) = indent_depth.take() {
                            self.format_buf.write_indent(depth);
                        }
                        CompactEmitter {
                            buf: &mut self.format_buf,
                        }
                        .emit_event(event);
                    }
                }
            }
        }
    }

    fn push_frame(&mut self, event: Event<'a>, depth: usize, owner_id: usize) {
        let mut frame = Frame {
            events: vec![],
            depth, // snapshot depth
            bytes_available: self.current_available_bytes(),
            owner_id,
            ..Frame::default()
        };

        let len = self.format_buf.event_len(&event);
        frame.push(event, len);

        self.frames.push_back(frame);
    }

    /// decides whether event should be in same frame or not
    fn push_event(&mut self, event: Event<'a>, depth: usize, owner_id: usize) {
        let Some(last_frame) = self.frames.back_mut() else {
            self.push_frame(event, depth, owner_id);
            return;
        };

        if last_frame.owner_id != owner_id {
            self.push_frame(event, depth, owner_id);
            return;
        }

        // TODO if statement
        match event.clone() {
            Event::ArrayOpen { .. } | Event::ObjectOpen { .. } | Event::ArrayClose => {
                self.push_frame(event, depth, owner_id);
            }
            _ if event.is_object() && last_frame.events.iter().any(|x| x.is_array()) => {
                self.push_frame(event, depth, owner_id);
            }
            _ if event.is_array() && last_frame.events.iter().any(|x| x.is_object()) => {
                self.push_frame(event, depth, owner_id);
            }
            _ => {
                let len = self.format_buf.event_len(&event);
                last_frame.push(event, len);
            }
        }
    }

    fn current_container_id(&self) -> usize {
        self.containers
            .last()
            .map(|container| container.id)
            .unwrap_or(0)
    }

    fn open_container(&mut self, kind: layout::FrameKind, available_bytes: usize) -> usize {
        let id = self.next_container_id;
        self.next_container_id = self.next_container_id.saturating_add(1);
        let stats = layout::FrameStats {
            kind,
            inline_len: 0,
            key_count: 0,
            child_expanded: false,
            is_empty: true,
            available_bytes,
        };
        self.containers.push(ContainerState {
            id,
            kind,
            stats,
            mode: None,
            line_overflow: false,
        });
        id
    }

    fn mark_object_line_overflow(&mut self, overflow: bool) {
        if let Some(container) = self
            .containers
            .last_mut()
            .filter(|container| container.kind == layout::FrameKind::Object)
        {
            container.line_overflow = overflow;
        }
    }

    fn parent_line_overflow(&self) -> bool {
        self.containers
            .last()
            .filter(|container| container.kind == layout::FrameKind::Object)
            .map(|container| container.line_overflow)
            .unwrap_or(false)
    }

    fn current_available_bytes(&self) -> usize {
        if self.containers.last().and_then(|container| container.mode) == Some(FormatMode::Expanded)
        {
            return self.format_buf.available_bytes();
        }

        self.frames
            .back()
            .map(|frame| frame.bytes_available)
            .unwrap_or_else(|| self.format_buf.available_bytes())
    }

    fn close_container(&mut self) -> Option<ContainerState> {
        self.containers.pop()
    }

    fn record_inline_len(&mut self, inline_len: usize) {
        if let Some(container) = self.containers.last_mut() {
            container.stats.inline_len = container.stats.inline_len.saturating_add(inline_len);
        }
    }

    fn mark_parent_array_non_empty(&mut self) {
        if let Some(container) = self
            .containers
            .last_mut()
            .filter(|container| container.kind == layout::FrameKind::Array)
        {
            container.stats.is_empty = false;
        }
    }

    fn mark_current_array_non_empty(&mut self) {
        if let Some(container) = self
            .containers
            .last_mut()
            .filter(|container| container.kind == layout::FrameKind::Array)
        {
            container.stats.is_empty = false;
        }
    }

    fn increment_key_count(&mut self) -> bool {
        if let Some(container) = self.containers.last_mut() {
            let was_empty = container.stats.key_count == 0;
            container.stats.key_count = container.stats.key_count.saturating_add(1);
            container.stats.is_empty = false;
            return was_empty;
        }

        false
    }

    fn mark_current_child_expanded(&mut self) {
        if let Some(container) = self.containers.last_mut() {
            container.stats.child_expanded = true;
        }
    }

    fn set_container_mode(&mut self, id: usize, mode: FormatMode) {
        for frame in &mut self.frames {
            if frame.owner_id == id {
                frame.mode = Some(mode);
            }
        }

        if let Some(container) = self
            .containers
            .iter_mut()
            .find(|container| container.id == id)
        {
            container.mode = Some(mode);
        }
    }

    fn apply_container_layout(&mut self, closed: ContainerState) {
        let mode = layout::decide_layout(closed.stats, self.format_buf.preferred_width);
        self.set_container_mode(closed.id, mode);

        if let Some(parent) = self.containers.last_mut() {
            match mode {
                FormatMode::Expanded => {
                    parent.stats.child_expanded = true;
                }
                FormatMode::Compact => {
                    parent.stats.inline_len = parent
                        .stats
                        .inline_len
                        .saturating_add(closed.stats.inline_len);
                }
            }
        }
    }

    fn update_depth_and_frame_depth(&mut self, event: &Event<'a>) -> (usize, usize) {
        match event {
            Event::ArrayOpen { is_array_value } => self.depth.on_array_open(*is_array_value),
            Event::ArrayClose => self.depth.on_array_close(),
            Event::ObjectOpen { is_array_value } => self.depth.on_object_open(*is_array_value),
            Event::ObjectClose => self.depth.on_object_close(),
            Event::ObjectKey { key } => self.depth.on_object_key(key),
            Event::KeyValDelim => self.depth.on_object_key_val_delim(),
            Event::ItemDelim => self.depth.on_item_delim(),
            Event::Null { is_array_value } => self.depth.on_null(*is_array_value),
            Event::String { s, is_array_value } => self.depth.on_string(s, *is_array_value),
            Event::Number { n, is_array_value } => self.depth.on_number(n.clone(), *is_array_value),
            Event::Boolean {
                value,
                is_array_value,
            } => self.depth.on_boolean(*value, *is_array_value),
        }

        let current_depth = self.depth.current_depth();
        let frame_depth = match event {
            Event::ArrayOpen { .. }
            | Event::ArrayClose
            | Event::ObjectOpen { .. }
            | Event::ObjectClose => current_depth,
            _ => current_depth.saturating_sub(1),
        };

        (current_depth, frame_depth)
    }

    /// Push an event into the current buffer and update remaining width and buffered byte count.
    /// assumes well formed events
    fn on_event(&mut self, event: Event<'a>) {
        let (_container_depth, frame_depth) = self.update_depth_and_frame_depth(&event);
        let event_len = self.format_buf.event_len(&event);

        match event {
            Event::ObjectOpen { .. } => {
                self.mark_parent_array_non_empty();
                let owner_id =
                    self.open_container(layout::FrameKind::Object, self.current_available_bytes());
                self.record_inline_len(event_len);
                self.push_event(event, frame_depth, owner_id);
            }
            Event::ArrayOpen { .. } => {
                self.mark_parent_array_non_empty();
                let available_bytes = if self.parent_line_overflow() {
                    0
                } else {
                    self.current_available_bytes()
                };
                let owner_id = self.open_container(layout::FrameKind::Array, available_bytes);
                self.record_inline_len(event_len);
                self.push_event(event, frame_depth, owner_id);
            }
            Event::ObjectKey { key } => {
                self.record_inline_len(event_len);
                let first_key = self.increment_key_count();
                let owner_id = self.current_container_id();
                if first_key {
                    self.set_container_mode(owner_id, FormatMode::Expanded);
                }
                let prefix_len = self
                    .format_buf
                    .indent_len(frame_depth)
                    .saturating_add(FormatBuf::quoted_len(key))
                    .saturating_add(self.format_buf.key_val_delim_len());
                self.mark_object_line_overflow(prefix_len >= self.format_buf.preferred_width);
                self.push_event(event, frame_depth, owner_id);
            }
            Event::ObjectClose => {
                let owner_id = self.current_container_id();
                self.record_inline_len(event_len);
                self.push_event(event, frame_depth, owner_id);
                if let Some(closed) = self.close_container() {
                    self.apply_container_layout(closed);
                }
            }
            Event::ArrayClose => {
                let owner_id = self.current_container_id();
                self.record_inline_len(event_len);
                self.push_event(event, frame_depth, owner_id);
                if let Some(closed) = self.close_container() {
                    self.apply_container_layout(closed);
                }
            }
            Event::Boolean { .. }
            | Event::Null { .. }
            | Event::Number { .. }
            | Event::String { .. } => {
                let owner_id = self.current_container_id();
                self.record_inline_len(event_len);
                self.mark_current_array_non_empty();
                if event_len > self.format_buf.preferred_width {
                    self.mark_current_child_expanded();
                }
                self.push_event(event, frame_depth, owner_id);

                if owner_id == 0 {
                    let stats = layout::FrameStats {
                        kind: layout::FrameKind::Scalar,
                        inline_len: event_len,
                        key_count: 0,
                        child_expanded: false,
                        is_empty: false,
                        available_bytes: self.current_available_bytes(),
                    };
                    let mode = layout::decide_layout(stats, self.format_buf.preferred_width);
                    self.set_container_mode(0, mode);
                }
            }
            Event::ItemDelim | Event::KeyValDelim => {
                let owner_id = self.current_container_id();
                self.record_inline_len(event_len);
                self.push_event(event, frame_depth, owner_id);
            }
        }

        self.flush_buffer();
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Frame<'a> {
    mode: Option<FormatMode>,
    events: Vec<Event<'a>>,
    depth: usize,
    bytes_available: usize,
    owner_id: usize,
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

#[allow(dead_code)]
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

    pub fn indent_len(&self, level: usize) -> usize {
        self.opts
            .indent
            .map(|(_, size)| size.saturating_mul(level))
            .unwrap_or(0)
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
    use super::layout::{FrameKind, FrameStats, LayoutDecision, LayoutTracker, decide_layout};
    use super::*;
    use crate::traverse::parse_tokens;

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

    #[test]
    fn layout_object_with_keys_expands() {
        let stats = FrameStats {
            kind: FrameKind::Object,
            inline_len: 10,
            key_count: 1,
            child_expanded: false,
            is_empty: false,
            available_bytes: 80,
        };

        assert_eq!(decide_layout(stats, 80), FormatMode::Expanded);
    }

    #[test]
    fn layout_empty_object_is_compact() {
        let stats = FrameStats {
            kind: FrameKind::Object,
            inline_len: 2,
            key_count: 0,
            child_expanded: false,
            is_empty: true,
            available_bytes: 80,
        };

        assert_eq!(decide_layout(stats, 0), FormatMode::Compact);
    }

    #[test]
    fn layout_array_expands_on_child_or_width() {
        let child_expanded = FrameStats {
            kind: FrameKind::Array,
            inline_len: 4,
            key_count: 0,
            child_expanded: true,
            is_empty: false,
            available_bytes: 80,
        };
        let width_overflow = FrameStats {
            kind: FrameKind::Array,
            inline_len: 10,
            key_count: 0,
            child_expanded: false,
            is_empty: false,
            available_bytes: 5,
        };

        assert_eq!(decide_layout(child_expanded, 80), FormatMode::Expanded);
        assert_eq!(decide_layout(width_overflow, 5), FormatMode::Expanded);
    }

    #[test]
    fn layout_empty_array_is_compact() {
        let stats = FrameStats {
            kind: FrameKind::Array,
            inline_len: 2,
            key_count: 0,
            child_expanded: false,
            is_empty: true,
            available_bytes: 80,
        };

        assert_eq!(decide_layout(stats, 0), FormatMode::Compact);
    }

    #[test]
    fn layout_scalar_expands_on_width_overflow() {
        let stats = FrameStats {
            kind: FrameKind::Scalar,
            inline_len: 10,
            key_count: 0,
            child_expanded: false,
            is_empty: false,
            available_bytes: 80,
        };

        assert_eq!(decide_layout(stats, 5), FormatMode::Expanded);
        assert_eq!(decide_layout(stats, 20), FormatMode::Compact);
    }

    #[test]
    fn layout_tracker_marks_object_keys_expanded() {
        let mut tracker = LayoutTracker::new(80);
        tracker.open_frame(FrameKind::Object);
        tracker.record_inline_len(1);
        tracker.mark_non_empty();
        tracker.increment_key_count();
        let decision = tracker.close_frame();

        assert_eq!(decision, LayoutDecision::Expanded);
    }

    #[test]
    fn layout_tracker_array_inherits_child_expansion() {
        let mut tracker = LayoutTracker::new(80);
        tracker.open_frame(FrameKind::Array);
        tracker.mark_child_expanded();
        let decision = tracker.close_frame();

        assert_eq!(decision, LayoutDecision::Expanded);
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum CaptureEventKind {
        ObjectOpen,
        ObjectKey,
        ObjectKeyValDelim,
        ObjectClose,
        ArrayOpen,
        ArrayClose,
        Null,
        String,
        Number,
        Boolean,
        ItemDelim,
    }

    #[derive(Debug)]
    struct CaptureVisitor<V, T, F> {
        inner: V,
        items: Vec<T>,
        map: F,
    }

    impl<V, T, F> CaptureVisitor<V, T, F>
    where
        F: FnMut(&V, CaptureEventKind) -> T,
    {
        fn new(inner: V, map: F) -> Self {
            Self {
                inner,
                items: Vec::new(),
                map,
            }
        }

        fn push(&mut self, kind: CaptureEventKind) {
            let item = (self.map)(&self.inner, kind);
            self.items.push(item);
        }
    }

    impl<'a, V, T, F> Visitor<'a> for CaptureVisitor<V, T, F>
    where
        V: Visitor<'a>,
        F: FnMut(&V, CaptureEventKind) -> T,
    {
        fn on_object_open(&mut self, is_array_value: bool) {
            self.inner.on_object_open(is_array_value);
            self.push(CaptureEventKind::ObjectOpen);
        }

        fn on_object_key(&mut self, key: &'a str) {
            self.inner.on_object_key(key);
            self.push(CaptureEventKind::ObjectKey);
        }

        fn on_object_key_val_delim(&mut self) {
            self.inner.on_object_key_val_delim();
            self.push(CaptureEventKind::ObjectKeyValDelim);
        }

        fn on_object_close(&mut self) {
            self.inner.on_object_close();
            self.push(CaptureEventKind::ObjectClose);
        }

        fn on_array_open(&mut self, is_array_value: bool) {
            self.inner.on_array_open(is_array_value);
            self.push(CaptureEventKind::ArrayOpen);
        }

        fn on_array_close(&mut self) {
            self.inner.on_array_close();
            self.push(CaptureEventKind::ArrayClose);
        }

        fn on_null(&mut self, is_array_value: bool) {
            self.inner.on_null(is_array_value);
            self.push(CaptureEventKind::Null);
        }

        fn on_string(&mut self, value: &'a str, is_array_value: bool) {
            self.inner.on_string(value, is_array_value);
            self.push(CaptureEventKind::String);
        }

        fn on_number(&mut self, value: Cow<'a, str>, is_array_value: bool) {
            self.inner.on_number(value, is_array_value);
            self.push(CaptureEventKind::Number);
        }

        fn on_boolean(&mut self, value: bool, is_array_value: bool) {
            self.inner.on_boolean(value, is_array_value);
            self.push(CaptureEventKind::Boolean);
        }

        fn on_item_delim(&mut self) {
            self.inner.on_item_delim();
            self.push(CaptureEventKind::ItemDelim);
        }
    }

    #[test]
    fn capture_visitor_records_event_kinds() {
        let json = r#"{"hi":[1]}"#;
        let mut visitor = CaptureVisitor::new(DepthVisitor::default(), |_, kind| kind);
        parse_tokens(json, &mut visitor).unwrap();

        assert_eq!(
            visitor.items,
            vec![
                CaptureEventKind::ObjectOpen,
                CaptureEventKind::ObjectKey,
                CaptureEventKind::ObjectKeyValDelim,
                CaptureEventKind::ArrayOpen,
                CaptureEventKind::Number,
                CaptureEventKind::ArrayClose,
                CaptureEventKind::ObjectClose,
            ]
        );
    }
}
