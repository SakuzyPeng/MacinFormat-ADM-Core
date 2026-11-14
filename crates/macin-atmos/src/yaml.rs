//! A small, strict YAML tree over `saphyr-parser` events, for DAMF reading.
//!
//! DAMF needs mappings, sequences and scalars and nothing else, so anchors, aliases, tags and
//! multiple documents are refused rather than half-supported. Large files are not built as one
//! tree: [`Stream`] hands out the members of one top-level sequence one at a time.
use crate::{Error, ErrorKind, Result};
use saphyr_parser::{BufferedInput, Event, Input, Parser, ScalarStyle, Span, StrInput};
use std::cell::RefCell;
use std::io::{BufRead, Read};
use std::rc::Rc;

/// Deep enough for any DAMF member, shallow enough that hostile input cannot exhaust the stack.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    Scalar {
        text: String,
        /// Plain scalars carry YAML's implicit types (numbers, booleans, `.inf`); quoted ones are
        /// always strings.
        plain: bool,
        line: usize,
    },
    Sequence(Vec<Node>, usize),
    Mapping(Vec<(String, Node)>, usize),
}
impl Node {
    pub(crate) fn line(&self) -> usize {
        match self {
            Self::Scalar { line, .. } | Self::Sequence(_, line) | Self::Mapping(_, line) => *line,
        }
    }
    pub(crate) fn as_mapping(&self, what: &str) -> Result<&[(String, Node)]> {
        match self {
            Self::Mapping(entries, _) => Ok(entries),
            _ => Err(invalid(self.line(), format!("{what} must be a mapping"))),
        }
    }
    pub(crate) fn as_sequence(&self, what: &str) -> Result<&[Node]> {
        match self {
            Self::Sequence(items, _) => Ok(items),
            _ => Err(invalid(self.line(), format!("{what} must be a sequence"))),
        }
    }
    /// The text of any scalar, plain or quoted.
    pub(crate) fn as_text(&self, what: &str) -> Result<&str> {
        match self {
            Self::Scalar { text, .. } => Ok(text),
            _ => Err(invalid(self.line(), format!("{what} must be a scalar"))),
        }
    }
    fn as_plain(&self, what: &str) -> Result<&str> {
        match self {
            Self::Scalar {
                text, plain: true, ..
            } => Ok(text),
            _ => Err(invalid(
                self.line(),
                format!("{what} must be a plain scalar"),
            )),
        }
    }
    pub(crate) fn as_bool(&self, what: &str) -> Result<bool> {
        match self.as_plain(what)? {
            "true" | "True" | "TRUE" => Ok(true),
            "false" | "False" | "FALSE" => Ok(false),
            _ => Err(invalid(
                self.line(),
                format!("{what} must be true or false"),
            )),
        }
    }
    pub(crate) fn as_u64(&self, what: &str) -> Result<u64> {
        let text = self.as_plain(what)?;
        text.strip_prefix('+')
            .unwrap_or(text)
            .parse()
            .map_err(|_| invalid(self.line(), format!("{what} must be a nonnegative integer")))
    }
    pub(crate) fn as_i64(&self, what: &str) -> Result<i64> {
        let text = self.as_plain(what)?;
        text.strip_prefix('+')
            .unwrap_or(text)
            .parse()
            .map_err(|_| invalid(self.line(), format!("{what} must be an integer")))
    }
    /// A YAML 1.2 core-schema float, including `.inf`, `-.inf` and `.nan`.
    pub(crate) fn as_f64(&self, what: &str) -> Result<f64> {
        let text = self.as_plain(what)?;
        let value = match text {
            ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => f64::INFINITY,
            "-.inf" | "-.Inf" | "-.INF" => f64::NEG_INFINITY,
            ".nan" | ".NaN" | ".NAN" => f64::NAN,
            _ if text.contains(['_', 'x', 'X', 'o']) => {
                return Err(invalid(
                    self.line(),
                    format!("{what} must be a decimal number"),
                ));
            }
            _ => text
                .parse()
                .map_err(|_| invalid(self.line(), format!("{what} must be a number")))?,
        };
        Ok(value)
    }
    pub(crate) fn as_vector(&self, what: &str) -> Result<[f64; 3]> {
        match self.as_sequence(what)? {
            [x, y, z] => Ok([x.as_f64(what)?, y.as_f64(what)?, z.as_f64(what)?]),
            _ => Err(invalid(
                self.line(),
                format!("{what} must have three elements"),
            )),
        }
    }
}

pub(crate) fn invalid(line: usize, message: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::InvalidInput, format!("line {line}: {message}"))
}
fn unsupported(line: usize, message: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::Unsupported, format!("line {line}: {message}"))
}

/// Streams UTF-8 text into the parser and keeps the first read or decoding error for later,
/// because the parser's input is an infallible `char` iterator.
pub(crate) struct Utf8Chars<R> {
    source: R,
    text: String,
    at: usize,
    /// Bytes of a character split across two reads.
    tail: Vec<u8>,
    done: bool,
    error: Rc<RefCell<Option<Error>>>,
}
impl<R: BufRead> Utf8Chars<R> {
    fn fail(&mut self, error: Error) -> Option<char> {
        *self.error.borrow_mut() = Some(error);
        self.done = true;
        None
    }
}
impl<R: BufRead> Iterator for Utf8Chars<R> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        loop {
            if let Some(c) = self.text[self.at..].chars().next() {
                self.at += c.len_utf8();
                return Some(c);
            }
            if self.done {
                return None;
            }
            self.text.clear();
            self.at = 0;
            let read = match self.source.fill_buf() {
                Ok(bytes) => {
                    self.tail.extend_from_slice(bytes);
                    bytes.len()
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return self.fail(e.into()),
            };
            self.source.consume(read);
            if read == 0 {
                self.done = true;
                if !self.tail.is_empty() {
                    return self.fail(Error::invalid("DAMF YAML ends inside a UTF-8 sequence"));
                }
                return None;
            }
            let valid = match std::str::from_utf8(&self.tail) {
                Ok(_) => self.tail.len(),
                Err(e) if e.error_len().is_some() => {
                    return self.fail(Error::invalid("DAMF YAML is not valid UTF-8"));
                }
                Err(e) => e.valid_up_to(),
            };
            let decoded = std::str::from_utf8(&self.tail[..valid]).unwrap_or_default();
            self.text.push_str(decoded);
            self.tail.drain(..valid);
        }
    }
}

pub(crate) struct Stream<'a, T: Input> {
    parser: Parser<'a, T>,
    error: Rc<RefCell<Option<Error>>>,
}

pub(crate) type FileStream<R> = Stream<'static, BufferedInput<Utf8Chars<R>>>;

pub(crate) fn from_reader<R: BufRead + 'static>(source: R) -> FileStream<R> {
    let error = Rc::new(RefCell::new(None));
    Stream {
        parser: Parser::new_from_iter(Utf8Chars {
            source,
            text: String::new(),
            at: 0,
            tail: Vec::new(),
            done: false,
            error: Rc::clone(&error),
        }),
        error,
    }
}

/// A whole small document, such as a manifest, as one tree.
pub(crate) fn parse_document(text: &str) -> Result<Node> {
    let mut stream = Stream {
        parser: Parser::new(StrInput::new(text)),
        error: Rc::new(RefCell::new(None)),
    };
    stream.begin()?;
    let (event, span) = stream.next()?;
    let node = stream.node(event, span, 0)?;
    stream.end()?;
    Ok(node)
}

impl<'a, T: Input> Stream<'a, T> {
    pub(crate) fn next(&mut self) -> Result<(Event<'a>, Span)> {
        let item = self.parser.next_event();
        if let Some(error) = self.error.borrow_mut().take() {
            return Err(error);
        }
        match item {
            None => Err(Error::invalid("DAMF YAML ends unexpectedly")),
            Some(Err(e)) => Err(invalid(e.marker().line(), e.info())),
            Some(Ok(item)) => Ok(item),
        }
    }
    /// Consume the stream and document start of a single-document stream.
    pub(crate) fn begin(&mut self) -> Result<()> {
        let (event, span) = self.next()?;
        if !matches!(event, Event::StreamStart) {
            return Err(invalid(
                span.start.line(),
                "expected the start of a YAML stream",
            ));
        }
        let (event, span) = self.next()?;
        if !matches!(event, Event::DocumentStart(_)) {
            return Err(invalid(span.start.line(), "expected a YAML document"));
        }
        Ok(())
    }
    /// Consume the document and stream end; a second document is refused.
    pub(crate) fn end(&mut self) -> Result<()> {
        let (event, span) = self.next()?;
        if !matches!(event, Event::DocumentEnd) {
            return Err(invalid(
                span.start.line(),
                "expected the end of the YAML document",
            ));
        }
        let (event, span) = self.next()?;
        if !matches!(event, Event::StreamEnd) {
            return Err(unsupported(
                span.start.line(),
                "DAMF files hold a single YAML document",
            ));
        }
        Ok(())
    }
    /// Build the node that starts with `event`.
    pub(crate) fn node(&mut self, event: Event<'a>, span: Span, depth: usize) -> Result<Node> {
        let line = span.start.line();
        if depth > MAX_DEPTH {
            return Err(unsupported(line, "YAML nesting is too deep"));
        }
        match event {
            Event::Scalar(text, style, anchor, tag) => {
                refuse_extras(line, anchor, tag.is_some())?;
                Ok(Node::Scalar {
                    text: text.into_owned(),
                    plain: style == ScalarStyle::Plain,
                    line,
                })
            }
            Event::SequenceStart(anchor, tag) => {
                refuse_extras(line, anchor, tag.is_some())?;
                let mut items = Vec::new();
                loop {
                    let (event, span) = self.next()?;
                    if matches!(event, Event::SequenceEnd) {
                        return Ok(Node::Sequence(items, line));
                    }
                    items.push(self.node(event, span, depth + 1)?);
                }
            }
            Event::MappingStart(anchor, tag) => {
                refuse_extras(line, anchor, tag.is_some())?;
                let mut entries: Vec<(String, Node)> = Vec::new();
                while let Some((key, key_line)) = self.key()? {
                    if entries.iter().any(|(k, _)| *k == key) {
                        return Err(invalid(key_line, format!("duplicate key {key:?}")));
                    }
                    let (event, span) = self.next()?;
                    let value = self.node(event, span, depth + 1)?;
                    entries.push((key, value));
                }
                Ok(Node::Mapping(entries, line))
            }
            Event::Alias(_) => Err(unsupported(line, "YAML aliases are not used by DAMF")),
            _ => Err(invalid(line, "unexpected YAML structure")),
        }
    }
    /// The next key of the mapping being read, or `None` at its end.
    pub(crate) fn key(&mut self) -> Result<Option<(String, usize)>> {
        let (event, span) = self.next()?;
        let line = span.start.line();
        match event {
            Event::MappingEnd => Ok(None),
            Event::Scalar(text, _, anchor, tag) => {
                refuse_extras(line, anchor, tag.is_some())?;
                Ok(Some((text.into_owned(), line)))
            }
            _ => Err(unsupported(line, "DAMF mapping keys must be scalars")),
        }
    }
}

fn refuse_extras(line: usize, anchor: usize, tagged: bool) -> Result<()> {
    if anchor != 0 {
        return Err(unsupported(line, "YAML anchors are not used by DAMF"));
    }
    if tagged {
        return Err(unsupported(line, "YAML tags are not used by DAMF"));
    }
    Ok(())
}

/// Read a whole small file as text, refusing anything past `limit` bytes.
pub(crate) fn read_text(source: impl Read, limit: u64, what: &str) -> Result<String> {
    let mut text = String::new();
    let read = source
        .take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                Error::invalid(format!("{what} is not valid UTF-8"))
            } else {
                e.into()
            }
        })?;
    if read as u64 > limit {
        return Err(Error::new(
            ErrorKind::Unsupported,
            format!("{what} is larger than {limit} bytes"),
        ));
    }
    Ok(text)
}
