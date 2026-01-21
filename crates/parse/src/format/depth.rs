use crate::traverse::Visitor;
use std::borrow::Cow;

#[derive(Debug, Default)]
pub struct DepthVisitor {
    depth: usize,
    current_depth: usize,
}

impl DepthVisitor {
    pub fn current_depth(&self) -> usize {
        self.current_depth
    }

    pub(crate) fn on_scalar(&mut self, _is_array_value: bool) {
        self.set_current_depth(self.nested_depth());
    }

    fn set_current_depth(&mut self, depth: usize) {
        self.current_depth = depth;
    }

    fn nested_depth(&self) -> usize {
        self.depth.saturating_add(1)
    }
}

impl<'a> Visitor<'a> for DepthVisitor {
    fn on_object_open(&mut self, _is_array_value: bool) {
        self.depth = self.depth.saturating_add(1);
        self.set_current_depth(self.depth);
    }

    fn on_object_key(&mut self, _key: &'a str) {
        self.set_current_depth(self.nested_depth());
    }

    fn on_object_key_val_delim(&mut self) {
        self.set_current_depth(self.nested_depth());
    }

    fn on_object_close(&mut self) {
        self.set_current_depth(self.depth);
        self.depth = self.depth.saturating_sub(1);
    }

    fn on_array_open(&mut self, _is_array_value: bool) {
        self.depth = self.depth.saturating_add(1);
        self.set_current_depth(self.depth);
    }

    fn on_array_close(&mut self) {
        self.set_current_depth(self.depth);
        self.depth = self.depth.saturating_sub(1);
    }

    fn on_null(&mut self, _is_array_value: bool) {
        self.set_current_depth(self.nested_depth());
    }

    fn on_string(&mut self, _value: &'a str, _is_array_value: bool) {
        self.set_current_depth(self.nested_depth());
    }

    fn on_number(&mut self, _value: Cow<'a, str>, _is_array_value: bool) {
        self.set_current_depth(self.nested_depth());
    }

    fn on_boolean(&mut self, _value: bool, _is_array_value: bool) {
        self.set_current_depth(self.nested_depth());
    }

    fn on_item_delim(&mut self) {
        self.set_current_depth(self.nested_depth());
    }
}

#[cfg(test)]
mod tests {
    use super::DepthVisitor;
    use crate::traverse::Visitor;
    use crate::traverse::parse_tokens;
    use std::borrow::Cow;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum EventKind {
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
        events: Vec<T>,
        map: F,
    }

    impl<V, T, F> CaptureVisitor<V, T, F>
    where
        F: FnMut(&V, EventKind) -> T,
    {
        fn new(inner: V, map: F) -> Self {
            Self {
                inner,
                events: Vec::new(),
                map,
            }
        }

        fn push(&mut self, kind: EventKind) {
            let item = (self.map)(&self.inner, kind);
            self.events.push(item);
        }
    }

    impl<'a, V, T, F> Visitor<'a> for CaptureVisitor<V, T, F>
    where
        V: Visitor<'a>,
        F: FnMut(&V, EventKind) -> T,
    {
        fn on_object_open(&mut self, is_array_value: bool) {
            self.inner.on_object_open(is_array_value);
            self.push(EventKind::ObjectOpen);
        }

        fn on_object_key(&mut self, key: &'a str) {
            self.inner.on_object_key(key);
            self.push(EventKind::ObjectKey);
        }

        fn on_object_key_val_delim(&mut self) {
            self.inner.on_object_key_val_delim();
            self.push(EventKind::ObjectKeyValDelim);
        }

        fn on_object_close(&mut self) {
            self.inner.on_object_close();
            self.push(EventKind::ObjectClose);
        }

        fn on_array_open(&mut self, is_array_value: bool) {
            self.inner.on_array_open(is_array_value);
            self.push(EventKind::ArrayOpen);
        }

        fn on_array_close(&mut self) {
            self.inner.on_array_close();
            self.push(EventKind::ArrayClose);
        }

        fn on_null(&mut self, is_array_value: bool) {
            self.inner.on_null(is_array_value);
            self.push(EventKind::Null);
        }

        fn on_string(&mut self, value: &'a str, is_array_value: bool) {
            self.inner.on_string(value, is_array_value);
            self.push(EventKind::String);
        }

        fn on_number(&mut self, value: Cow<'a, str>, is_array_value: bool) {
            self.inner.on_number(value, is_array_value);
            self.push(EventKind::Number);
        }

        fn on_boolean(&mut self, value: bool, is_array_value: bool) {
            self.inner.on_boolean(value, is_array_value);
            self.push(EventKind::Boolean);
        }

        fn on_item_delim(&mut self) {
            self.inner.on_item_delim();
            self.push(EventKind::ItemDelim);
        }
    }

    #[test]
    fn depth_tracks_object_and_array() {
        let json = r#"{"hi":"bye","hello":["hi"]}"#;

        let mut visitor = CaptureVisitor::new(DepthVisitor::default(), |visitor, kind| {
            (kind, visitor.current_depth())
        });
        parse_tokens(json, &mut visitor).unwrap();

        let expected = vec![
            (EventKind::ObjectOpen, 1),
            (EventKind::ObjectKey, 2),
            (EventKind::ObjectKeyValDelim, 2),
            (EventKind::String, 2),
            (EventKind::ItemDelim, 2),
            (EventKind::ObjectKey, 2),
            (EventKind::ObjectKeyValDelim, 2),
            (EventKind::ArrayOpen, 2),
            (EventKind::String, 3),
            (EventKind::ArrayClose, 2),
            (EventKind::ObjectClose, 1),
        ];

        assert_eq!(visitor.events, expected);
    }
}
