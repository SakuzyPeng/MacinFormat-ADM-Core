//! 只保留引用结构和声道范围；正常有序时间线不建立逐 block 索引。
use super::{adm_error, unsupported};
use crate::{Error, Result};
use macin_adm::{Document, Snapshot};
use macin_bwf::Chunk;
use quick_xml::{
    NsReader, Writer,
    events::{BytesStart, Event},
    name::{PrefixDeclaration, ResolveResult},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom},
    ops::Range,
    path::Path,
    sync::Arc,
};

fn xml_error(error: impl std::fmt::Display) -> Error {
    Error::invalid(format!("ADM XML: {error}"))
}

fn read_error(error: quick_xml::Error) -> Error {
    match error {
        quick_xml::Error::Io(error) => std::io::Error::new(error.kind(), error.to_string()).into(),
        other => xml_error(other),
    }
}

// Take 会把提前 EOF 当作正常结束；此处不能让拼接的结束标签掩盖源文件被截断。
struct Exact {
    file: File,
    remaining: u64,
}

impl Read for Exact {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let count = self.remaining.min(output.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let read = self.file.read(&mut output[..count])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "ADM 元数据被截断",
            ));
        }
        self.remaining -= read as u64;
        Ok(read)
    }
}

fn utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(xml_error)
}

fn namespace(ns: ResolveResult<'_>) -> Result<Vec<u8>> {
    match ns {
        ResolveResult::Bound(ns) => Ok(ns.as_ref().to_vec()),
        ResolveResult::Unbound => Ok(Vec::new()),
        ResolveResult::Unknown(_) => Err(Error::invalid("XML 命名空间前缀未定义")),
    }
}

// 补齐祖先声明，使从文件中截取的声道可独立解析。
fn opening<R>(reader: &NsReader<R>, start: &BytesStart<'_>) -> Result<Vec<u8>> {
    let mut element = start.to_owned();
    let attributes = start
        .attributes()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(xml_error)?;
    for (prefix, uri) in reader.resolver().bindings() {
        let name = match prefix {
            PrefixDeclaration::Default => "xmlns".to_owned(),
            PrefixDeclaration::Named(name) => format!("xmlns:{}", utf8(name)?),
        };
        if !attributes.iter().any(|a| a.key.as_ref() == name.as_bytes()) {
            element.push_attribute((name.as_str(), utf8(uri.as_ref())?));
        }
    }
    let mut out = Writer::new(Vec::new());
    out.write_event(Event::Start(element))?;
    Ok(out.into_inner())
}

fn subtree<R: BufRead>(reader: &mut NsReader<R>, first: Event<'_>) -> Result<Vec<u8>> {
    let mut depth = usize::from(matches!(first, Event::Start(_)));
    let mut out = Writer::new(Vec::new());
    out.write_event(first)?;
    let mut buffer = Vec::new();
    while depth != 0 {
        buffer.clear();
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(read_error)?;
        namespace(ns)?;
        match &event {
            Event::Start(_) => {
                depth += 1;
                if depth >= 1024 {
                    return Err(unsupported("XML 结构过大或嵌套过深"));
                }
            }
            Event::End(_) => depth -= 1,
            Event::Eof => return Err(Error::invalid("XML 文档被截断")),
            _ => {}
        }
        out.write_event(event)?;
    }
    Ok(out.into_inner())
}

#[derive(Debug, Clone)]
pub(super) struct Location {
    range: Range<u64>,
    sample: u64,
}

#[derive(Debug, Clone)]
pub(super) struct Channel {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    range: Range<u64>,
    namespace: Vec<u8>,
    inline: Option<Arc<[u8]>>,
    // 仅乱序声道需要紧凑的偏移索引，保留旧读取器按时间稳定排序的行为。
    order: Option<Arc<[Location]>>,
    pub last: (u64, u64),
}

impl Channel {
    pub(super) fn inline(xml: &str, rate: u32) -> Result<Self> {
        let doc = Document::parse_channel(xml).map_err(adm_error)?;
        let scene = Snapshot::extract_channel(&doc, 0, 0, rate).map_err(adm_error)?;
        let last = scene
            .object_blocks
            .iter()
            .map(|b| (b.start, b.end))
            .chain(scene.direct_blocks.iter().map(|b| (b.start, b.end)))
            .max_by_key(|b| b.0)
            .ok_or_else(|| Error::invalid("声道没有 block"))?;
        Ok(Self {
            prefix: Vec::new(),
            suffix: Vec::new(),
            range: 0..0,
            namespace: Vec::new(),
            inline: Some(Arc::from(xml.as_bytes())),
            order: None,
            last,
        })
    }

    fn document(&self, block: &[u8]) -> Result<Document> {
        let mut xml = Vec::with_capacity(self.prefix.len() + block.len() + self.suffix.len());
        xml.extend_from_slice(&self.prefix);
        xml.extend_from_slice(block);
        xml.extend_from_slice(&self.suffix);
        Document::parse_channel(utf8(&xml)?).map_err(adm_error)
    }

    fn cursor(&self, path: &Path) -> Result<BlockCursor> {
        let source: Box<dyn BufRead> = if let Some(xml) = &self.inline {
            Box::new(Cursor::new(xml.clone()))
        } else {
            let mut file = File::open(path)?;
            file.seek(SeekFrom::Start(self.range.start))?;
            Box::new(BufReader::with_capacity(
                64 << 10,
                Cursor::new(self.prefix.clone())
                    .chain(Exact {
                        file,
                        remaining: self.range.end - self.range.start,
                    })
                    .chain(Cursor::new(self.suffix.clone())),
            ))
        };
        let mut reader = NsReader::from_reader(source);
        reader.config_mut().check_comments = true;
        Ok(BlockCursor {
            reader,
            buffer: Vec::new(),
            channel: self.clone(),
            started: false,
        })
    }

    pub(super) fn blocks(&self, path: &Path) -> Result<Blocks> {
        Ok(if let Some(order) = &self.order {
            Blocks::Indexed {
                file: File::open(path)?,
                channel: self.clone(),
                order: order.clone(),
                next: 0,
            }
        } else {
            Blocks::Sequential(self.cursor(path)?)
        })
    }
}

fn bounds(doc: &Document, rate: u32) -> Result<(u64, u64)> {
    let block = doc
        .element_children(0)
        .into_iter()
        .find(|&n| doc.element_name(n) == Some("audioBlockFormat"))
        .ok_or_else(|| Error::invalid("声道片段缺少 block"))?;
    let (start, end) = macin_adm::parse_time_bounds(
        doc.element_attribute(block, "rtime"),
        doc.element_attribute(block, "duration"),
        rate,
    )
    .map_err(adm_error)?;
    Ok((start, end.unwrap_or(u64::MAX)))
}

pub(super) struct Index {
    pub xml: String,
    pub channels: BTreeMap<String, Channel>,
}

impl Index {
    pub fn read(path: &Path, chunk: Chunk, rate: u32) -> Result<Self> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(chunk.offset))?;
        let mut reader = NsReader::from_reader(BufReader::with_capacity(
            64 << 10,
            Exact {
                file,
                remaining: chunk.size,
            },
        ));
        reader.config_mut().check_comments = true;
        let mut xml = Writer::new(Vec::new());
        let mut buffer = Vec::new();
        let mut stack: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        let mut active: Option<(String, Channel, Option<u64>, bool)> = None;
        let mut channels = BTreeMap::new();
        let mut unordered = Vec::new();
        loop {
            buffer.clear();
            let position = reader.buffer_position();
            let (ns, event) = reader
                .read_resolved_event_into(&mut buffer)
                .map_err(read_error)?;
            let ns = namespace(ns)?;
            if let Event::Start(start) | Event::Empty(start) = &event {
                if stack.len() == 5
                    && start.local_name().as_ref() == b"audioBlockFormat"
                    && let Some((_, channel, last, sorted)) = &mut active
                    && ns == channel.namespace
                {
                    let raw = subtree(&mut reader, event)?;
                    let doc = channel.document(&raw)?;
                    let (sample, end) = bounds(&doc, rate)?;
                    *sorted &= last.is_none_or(|previous| previous <= sample);
                    if last.is_none() || sample >= channel.last.0 {
                        channel.last = (sample, end);
                    }
                    if last.is_none() {
                        xml.get_mut().extend_from_slice(&raw);
                    }
                    *last = Some(sample);
                    continue;
                }
                let expected = [
                    b"ebuCoreMain".as_slice(),
                    b"coreMetadata",
                    b"format",
                    b"audioFormatExtended",
                ];
                if stack.len() == 4
                    && stack
                        .iter()
                        .zip(expected)
                        .all(|((local, parent_ns), expected)| local == expected && *parent_ns == ns)
                    && start.local_name().as_ref() == b"audioChannelFormat"
                    && matches!(event, Event::Start(_))
                {
                    let id = start
                        .try_get_attribute("audioChannelFormatID")
                        .map_err(xml_error)?
                        .ok_or_else(|| Error::invalid("audioChannelFormat 缺少 ID"))?
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(xml_error)?
                        .into_owned();
                    let channel = Channel {
                        prefix: opening(&reader, start)?,
                        suffix: format!("</{}>", utf8(start.name().as_ref())?).into_bytes(),
                        range: (chunk.offset + reader.buffer_position())..0,
                        namespace: ns.clone(),
                        inline: None,
                        order: None,
                        last: (0, u64::MAX),
                    };
                    active = Some((id.to_ascii_uppercase(), channel, None, true));
                }
            }
            // 不让被剥离 block 之间的缩进或注释重新累积为一份大字符串。
            if matches!(&event, Event::Comment(_) | Event::PI(_))
                || (active.is_some()
                    && stack.len() == 5
                    && matches!(&event, Event::Text(text) if text.as_ref().iter().all(u8::is_ascii_whitespace)))
            {
                continue;
            }
            match &event {
                Event::Start(start) => {
                    if stack.len() >= 1024 {
                        return Err(unsupported("XML 结构过大或嵌套过深"));
                    }
                    stack.push((start.local_name().as_ref().to_vec(), ns));
                }
                Event::End(_) => {
                    if stack.len() == 5
                        && let Some((id, mut channel, _, sorted)) = active.take()
                    {
                        channel.range.end = chunk.offset + position;
                        if !sorted {
                            unordered.push(id.clone());
                        }
                        if channels.insert(id, channel).is_some() {
                            return Err(Error::invalid("重复 audioChannelFormat ID"));
                        }
                    }
                    stack
                        .pop()
                        .ok_or_else(|| Error::invalid("XML 结束标签不匹配"))?;
                }
                Event::Eof => {
                    if !stack.is_empty() {
                        return Err(Error::invalid("XML 文档被截断"));
                    }
                    break;
                }
                _ => {}
            }
            xml.write_event(event)?;
        }
        for id in unordered {
            let channel = channels.get_mut(&id).expect("已收集的声道");
            let mut cursor = channel.cursor(path)?;
            let mut order = Vec::new();
            while let Some((doc, range)) = cursor.next()? {
                order.push(Location {
                    range,
                    sample: bounds(&doc, rate)?.0,
                });
            }
            order.sort_by_key(|item| item.sample);
            channel.order = Some(order.into());
        }
        Ok(Self {
            xml: String::from_utf8(xml.into_inner()).map_err(xml_error)?,
            channels,
        })
    }
}

pub(super) struct Block {
    pub doc: Document,
    pub scene: Snapshot,
}

impl Block {
    pub fn start(&self) -> u64 {
        self.scene
            .object_blocks
            .first()
            .map_or_else(|| self.scene.direct_blocks[0].start, |b| b.start)
    }
}

// 一个轨道可经 pack 引用多个声道；每个声道只预读一个 block，稳定合并时间顺序。
pub(super) struct ProjectedBlocks {
    inputs: Vec<(Blocks, Option<Block>)>,
    rate: u32,
    start: u64,
    locked: u32,
}

impl ProjectedBlocks {
    pub fn new(
        channels: &[Channel],
        path: &Path,
        rate: u32,
        start: u64,
        locked: u32,
    ) -> Result<Self> {
        let mut out = Self {
            inputs: Vec::new(),
            rate,
            start,
            locked,
        };
        for channel in channels {
            let mut blocks = channel.blocks(path)?;
            let first = out.project(blocks.next()?)?;
            out.inputs.push((blocks, first));
        }
        Ok(out)
    }
    fn project(&self, doc: Option<Document>) -> Result<Option<Block>> {
        doc.map(|doc| {
            let scene = Snapshot::extract_channel(&doc, self.start, self.locked, self.rate)
                .map_err(adm_error)?;
            if scene.object_blocks.len() + scene.direct_blocks.len() != 1 {
                return Err(Error::invalid("声道片段需要恰好一个 block"));
            }
            Ok(Block { doc, scene })
        })
        .transpose()
    }
    pub fn next(&mut self) -> Result<Option<Block>> {
        let Some(index) = self
            .inputs
            .iter()
            .enumerate()
            .filter_map(|(i, (_, b))| b.as_ref().map(|b| (i, b.start())))
            .min_by_key(|&(i, start)| (start, i))
            .map(|(i, _)| i)
        else {
            return Ok(None);
        };
        let block = self.inputs[index].1.take();
        let next = self.inputs[index].0.next()?;
        self.inputs[index].1 = self.project(next)?;
        Ok(block)
    }
}

pub(super) struct BlockCursor {
    reader: NsReader<Box<dyn BufRead>>,
    buffer: Vec<u8>,
    channel: Channel,
    started: bool,
}

impl BlockCursor {
    fn next(&mut self) -> Result<Option<(Document, Range<u64>)>> {
        loop {
            self.buffer.clear();
            let position = self.reader.buffer_position();
            let (ns, event) = self
                .reader
                .read_resolved_event_into(&mut self.buffer)
                .map_err(read_error)?;
            let ns = namespace(ns)?;
            match &event {
                Event::Start(start) | Event::Empty(start) if !self.started => {
                    self.channel.prefix = opening(&self.reader, start)?;
                    self.channel.suffix =
                        format!("</{}>", utf8(start.name().as_ref())?).into_bytes();
                    self.channel.namespace = ns;
                    self.started = true;
                }
                Event::Start(start) | Event::Empty(start) => {
                    let block = start.local_name().as_ref() == b"audioBlockFormat"
                        && ns == self.channel.namespace;
                    let raw = subtree(&mut self.reader, event)?;
                    if block {
                        let base = self.channel.range.start;
                        let prefix = self.channel.prefix.len() as u64;
                        let range = (base + position.saturating_sub(prefix))
                            ..(base + self.reader.buffer_position().saturating_sub(prefix));
                        return Ok(Some((self.channel.document(&raw)?, range)));
                    }
                }
                Event::Eof => return Ok(None),
                _ => {}
            }
        }
    }
}

pub(super) enum Blocks {
    Sequential(BlockCursor),
    Indexed {
        file: File,
        channel: Channel,
        order: Arc<[Location]>,
        next: usize,
    },
}

impl Blocks {
    pub fn next(&mut self) -> Result<Option<Document>> {
        match self {
            Self::Sequential(cursor) => cursor.next().map(|item| item.map(|(doc, _)| doc)),
            Self::Indexed {
                file,
                channel,
                order,
                next,
            } => {
                let Some(location) = order.get(*next) else {
                    return Ok(None);
                };
                *next += 1;
                file.seek(SeekFrom::Start(location.range.start))?;
                let len = usize::try_from(location.range.end - location.range.start)
                    .map_err(|_| Error::invalid("block 过大"))?;
                let mut bytes = vec![0; len];
                file.read_exact(&mut bytes)?;
                channel.document(&bytes).map(Some)
            }
        }
    }
}
