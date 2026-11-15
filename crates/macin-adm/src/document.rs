use crate::{
    Error, Result,
    xml::{Attribute, Node, Tree},
};
use std::{collections::HashMap, sync::OnceLock};

pub(crate) const COMMON: u32 = 1 << 31;
static COMMON_DEFINITIONS: OnceLock<Result<Tree>> = OnceLock::new();

/// CHNA 提供的 ADM 轨道定义。`format_id` 为 AT ID 或去掉 `_00` 填充的 AC ID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackUidDefinition {
    pub uid: String,
    pub format_id: String,
    /// CHNA 的 packRef 为空时使用 `None`，保留 AXML 中已有的 pack 引用。
    pub pack_id: Option<String>,
}

#[derive(Debug)]
pub struct Document {
    pub(crate) xml: Tree,
    pub(crate) common: &'static Tree,
    pub(crate) ids: HashMap<String, u32>,
    pub(crate) elements: Vec<u32>,
    track_streams: HashMap<u32, u32>,
    pub(crate) track_uids: HashMap<u32, TrackUidDefinition>,
}

pub(crate) fn canonical_id(value: &str) -> String {
    let value = value.trim_end_matches([' ', '\0']).trim();
    value.split_once('_').map_or_else(
        || value.to_owned(),
        |(prefix, suffix)| {
            format!(
                "{}_{}",
                prefix.to_ascii_uppercase(),
                suffix.to_ascii_lowercase()
            )
        },
    )
}

fn id_attribute(kind: &str) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        "audioProgramme" => ("audioProgrammeID", "APR"),
        "audioContent" => ("audioContentID", "ACO"),
        "audioObject" => ("audioObjectID", "AO"),
        "audioPackFormat" => ("audioPackFormatID", "AP"),
        "audioChannelFormat" => ("audioChannelFormatID", "AC"),
        "audioStreamFormat" => ("audioStreamFormatID", "AS"),
        "audioTrackFormat" => ("audioTrackFormatID", "AT"),
        "audioTrackUID" => ("UID", "ATU"),
        _ => return None,
    })
}

fn validate_id(value: &str, prefix: &str) -> Result<()> {
    let parts: Vec<_> = value.split('_').collect();
    let widths: &[usize] = match prefix {
        "APR" | "ACO" | "AO" => &[4],
        "AT" => &[8, 2],
        _ => &[8],
    };
    if parts.first().copied() != Some(prefix)
        || parts.len() != widths.len() + 1
        || !parts[1..].iter().zip(widths).all(|(part, width)| {
            part.len() == *width && part.bytes().all(|c| c.is_ascii_hexdigit())
        })
    {
        return Err(Error::xml(format!("无效 ADM ID: {value}")));
    }
    Ok(())
}

fn afe(tree: &Tree, common: bool) -> Result<usize> {
    let root = &tree.nodes[tree.root];
    if root.local != "ebuCoreMain" && !(common && root.local == "ituADM") {
        return Err(Error::xml(
            "缺少 ebuCoreMain/coreMetadata/format/audioFormatExtended",
        ));
    }
    let mut index = tree.root;
    for name in ["coreMetadata", "format", "audioFormatExtended"] {
        let children: Vec<_> = tree.children(index, name).collect();
        if children.len() != 1 {
            return Err(Error::xml(format!("需要唯一的 {name} 元素")));
        }
        index = children[0];
    }
    Ok(index)
}

impl Document {
    /// 解析独立声道片段，供流式调用方逐块校验；命名空间声明须包含在片段中。
    /// 不解析跨元素引用，根元素必须是带有效 ID 和类型的 audioChannelFormat。
    pub fn parse_channel(source: &str) -> Result<Self> {
        let common = COMMON_DEFINITIONS
            .get_or_init(|| Tree::parse(include_str!("../assets/common_definitions.xml")))
            .as_ref()
            .map_err(Clone::clone)?;
        let xml = Tree::parse(source)?;
        let root = xml.root as u32;
        let mut doc = Self {
            xml,
            common,
            ids: HashMap::new(),
            elements: vec![root],
            track_streams: HashMap::new(),
            track_uids: HashMap::new(),
        };
        if doc.node(root).local != "audioChannelFormat" {
            return Err(Error::xml("声道片段需要 audioChannelFormat 根元素"));
        }
        let id = doc.id(root);
        validate_id(&id, "AC")?;
        doc.ids.insert(id, root);
        doc.validate_ranges(root)?;
        Ok(doc)
    }

    pub fn parse(source: &str) -> Result<Self> {
        Self::parse_with_track_uids(source, &[])
    }

    /// 在引用校验前合并 CHNA 的轨道定义，补全缺失的 UID、format 和 pack 引用。
    /// 与 AXML 或其他定义冲突时返回错误。原始 XML 和既有节点编号保留用于回写。
    pub fn parse_with_track_uids(source: &str, definitions: &[TrackUidDefinition]) -> Result<Self> {
        let common = COMMON_DEFINITIONS
            .get_or_init(|| Tree::parse(include_str!("../assets/common_definitions.xml")))
            .as_ref()
            .map_err(Clone::clone)?;
        let xml = Tree::parse(source)?;
        let local_afe = afe(&xml, false)?;
        let common_afe = afe(common, true)?;
        let mut doc = Self {
            xml,
            common,
            ids: HashMap::new(),
            elements: Vec::new(),
            track_streams: HashMap::new(),
            track_uids: HashMap::new(),
        };
        for (tree, parent, flag) in [(common, common_afe, COMMON), (&doc.xml, local_afe, 0)] {
            for &index in &tree.nodes[parent].children {
                let node = &tree.nodes[index];
                if node.namespace != tree.nodes[parent].namespace {
                    continue;
                }
                if let Some((attr, prefix)) = id_attribute(&node.local) {
                    let raw_id = node
                        .attr(attr)
                        .ok_or_else(|| Error::xml(format!("{} 缺少 {attr}", node.local)))?;
                    let id = canonical_id(raw_id);
                    validate_id(&id, prefix)?;
                    let key = index as u32 | flag;
                    if doc.ids.insert(id.clone(), key).is_some() {
                        return Err(Error::xml(format!("重复 ADM ID: {id}")));
                    }
                    if flag == 0 {
                        doc.elements.push(key);
                    }
                }
            }
        }
        doc.merge_track_uids(local_afe, definitions)?;
        // Resolve every modeled relationship, including those not used by a renderer.
        for &key in doc.ids.values() {
            let node = doc.node(key);
            for child in doc.all_children(key) {
                let reference = doc.node(child);
                let prefix = match reference.local.as_str() {
                    "audioContentIDRef" => "ACO",
                    "audioObjectIDRef" => "AO",
                    "audioPackFormatIDRef" => "AP",
                    "audioChannelFormatIDRef" => "AC",
                    "audioStreamFormatIDRef" => "AS",
                    "audioTrackFormatIDRef" => "AT",
                    "audioTrackUIDRef" => "ATU",
                    _ => continue,
                };
                let id = canonical_id(&reference.text);
                validate_id(&id, prefix)?;
                if id != "ATU_00000000" && !doc.ids.contains_key(&id) {
                    return Err(Error::xml(format!("{} 引用了不存在的 {id}", node.local)));
                }
            }
            if matches!(
                node.local.as_str(),
                "audioPackFormat" | "audioChannelFormat"
            ) {
                doc.type_of(key)?;
            }
        }
        doc.resolve_track_streams()?;
        for &key in doc.ids.values() {
            doc.validate_ranges(key)?;
        }
        doc.check_cycles("audioObject", "audioObjectIDRef")?;
        doc.check_cycles("audioPackFormat", "audioPackFormatIDRef")?;
        Ok(doc)
    }

    fn merge_track_uids(
        &mut self,
        parent: usize,
        definitions: &[TrackUidDefinition],
    ) -> Result<()> {
        for definition in definitions {
            let uid = canonical_id(&definition.uid);
            validate_id(&uid, "ATU")?;
            if uid == "ATU_00000000" {
                return Err(Error::xml("CHNA 不能定义保留的静音 UID"));
            }
            let format_id = canonical_id(&definition.format_id);
            let (prefix, reference) = if format_id.starts_with("AC_") {
                ("AC", "audioChannelFormatIDRef")
            } else {
                ("AT", "audioTrackFormatIDRef")
            };
            validate_id(&format_id, prefix)?;
            let pack_id = definition.pack_id.as_deref().map(canonical_id);
            if let Some(pack) = &pack_id {
                validate_id(pack, "AP")?;
            }
            for id in std::iter::once(&format_id).chain(pack_id.iter()) {
                if !self.ids.contains_key(id) {
                    return Err(Error::xml(format!("CHNA {uid} 引用了不存在的 {id}")));
                }
            }
            let key = if let Some(&key) = self.ids.get(&uid) {
                key
            } else {
                if self.xml.nodes.len() >= COMMON as usize {
                    return Err(Error::unsupported("ADM 节点表过大"));
                }
                let key = self.xml.nodes.len() as u32;
                let parent = &self.xml.nodes[parent];
                let node = Node {
                    name: parent.qualified("audioTrackUID"),
                    local: "audioTrackUID".to_owned(),
                    namespace: parent.namespace.clone(),
                    attributes: vec![Attribute {
                        name: "UID".to_owned(),
                        value: uid.clone(),
                    }],
                    text: String::new(),
                    children: Vec::new(),
                    start: 0,
                    open_end: 0,
                    close_start: 0,
                    end: 0,
                    empty: true,
                };
                self.xml.nodes.push(node);
                self.ids.insert(uid.clone(), key);
                self.elements.push(key);
                key
            };
            for name in ["audioTrackFormatIDRef", "audioChannelFormatIDRef"] {
                for child in self.children(key, name) {
                    if name != reference || canonical_id(&self.node(child).text) != format_id {
                        return Err(Error::xml(format!(
                            "CHNA 与 AXML 的 {uid} format 引用不一致"
                        )));
                    }
                }
            }
            if let Some(pack) = &pack_id {
                for child in self.children(key, "audioPackFormatIDRef") {
                    if canonical_id(&self.node(child).text) != *pack {
                        return Err(Error::xml(format!("CHNA 与 AXML 的 {uid} pack 引用不一致")));
                    }
                }
            }
            let previous = self.track_uids.get(&key);
            if previous.is_some_and(|p| {
                p.format_id != format_id
                    || p.pack_id
                        .as_ref()
                        .zip(pack_id.as_ref())
                        .is_some_and(|(a, b)| a != b)
            }) {
                return Err(Error::xml(format!("CHNA 包含相互矛盾的 {uid} 定义")));
            }
            let pack_id = pack_id.or_else(|| previous.and_then(|p| p.pack_id.clone()));
            self.track_uids.insert(
                key,
                TrackUidDefinition {
                    uid,
                    format_id,
                    pack_id,
                },
            );
        }
        Ok(())
    }
    /// Read-only element access by the `node` numbers a [`Snapshot`](crate::Snapshot) records,
    /// for callers that need an element or literal value the projection does not carry. A number
    /// that names no element gives `None` (or no children) rather than panicking.
    pub fn element_name(&self, node: u32) -> Option<&str> {
        self.checked(node).map(|n| n.local.as_str())
    }
    /// The value of attribute `name` on element `node`.
    pub fn element_attribute(&self, node: u32, name: &str) -> Option<&str> {
        self.checked(node).and_then(|n| n.attr(name))
    }
    /// The text content of element `node`, as written in the document.
    pub fn element_text(&self, node: u32) -> Option<&str> {
        self.checked(node).map(|n| n.text.as_str())
    }
    /// 原文中的完整元素片段；不补充祖先元素的命名空间声明。
    pub fn element_xml(&self, node: u32) -> Option<&str> {
        let element = self.checked(node)?;
        self.tree(node).source.get(element.start..element.end)
    }
    /// The child elements of `node` in its own namespace, in document order.
    pub fn element_children(&self, node: u32) -> Vec<u32> {
        if self.checked(node).is_none() {
            return Vec::new();
        }
        self.all_children(node)
    }
    /// The element that carries ADM ID `id` (for example `AP_00011001`), in the document or the
    /// common definitions.
    pub fn element_by_id(&self, id: &str) -> Option<u32> {
        self.ids.get(&canonical_id(id)).copied()
    }
    /// 返回轨道 UID 对应的声道元素，沿用 CHNA 合并与 TrackFormat / StreamFormat 引用解析。
    /// 未指定独立声道时返回其 pack 的声道，未知 UID 返回空列表。
    pub fn track_channels(&self, id: &str) -> Vec<u32> {
        let Some(uid) = self.element_by_id(id) else {
            return Vec::new();
        };
        if self.node(uid).local != "audioTrackUID" {
            return Vec::new();
        }
        self.channel(uid).map_or_else(
            || {
                self.reference(uid, "audioPackFormatIDRef")
                    .map_or_else(Vec::new, |pack| self.refs(pack, "audioChannelFormatIDRef"))
            },
            |channel| vec![channel],
        )
    }
    fn checked(&self, key: u32) -> Option<&Node> {
        self.tree(key).nodes.get((key & !COMMON) as usize)
    }
    pub(crate) fn tree(&self, key: u32) -> &Tree {
        if key & COMMON == 0 {
            &self.xml
        } else {
            self.common
        }
    }
    pub(crate) fn node(&self, key: u32) -> &Node {
        &self.tree(key).nodes[(key & !COMMON) as usize]
    }
    pub(crate) fn all_children(&self, key: u32) -> Vec<u32> {
        let node = self.node(key);
        let tree = self.tree(key);
        node.children
            .iter()
            .copied()
            .filter(|&n| tree.nodes[n].namespace == node.namespace)
            .map(|n| n as u32 | (key & COMMON))
            .collect()
    }
    pub(crate) fn children(&self, key: u32, name: &str) -> Vec<u32> {
        self.tree(key)
            .children((key & !COMMON) as usize, name)
            .map(|n| n as u32 | (key & COMMON))
            .collect()
    }
    pub(crate) fn child(&self, key: u32, name: &str) -> Option<u32> {
        self.tree(key)
            .child((key & !COMMON) as usize, name)
            .map(|n| n as u32 | (key & COMMON))
    }
    pub(crate) fn text(&self, key: u32, name: &str) -> Option<&str> {
        self.child(key, name).map(|k| self.node(k).text.as_str())
    }
    pub(crate) fn refs(&self, key: u32, name: &str) -> Vec<u32> {
        self.children(key, name)
            .iter()
            .filter_map(|k| self.ids.get(&canonical_id(&self.node(*k).text)).copied())
            .collect()
    }
    pub(crate) fn reference(&self, key: u32, name: &str) -> Option<u32> {
        if let Some(definition) = self.track_uids.get(&key) {
            let id = match name {
                "audioTrackFormatIDRef" if definition.format_id.starts_with("AT_") => {
                    Some(definition.format_id.as_str())
                }
                "audioChannelFormatIDRef" if definition.format_id.starts_with("AC_") => {
                    Some(definition.format_id.as_str())
                }
                "audioPackFormatIDRef" => definition.pack_id.as_deref(),
                _ => None,
            };
            if let Some(id) = id {
                return self.ids.get(id).copied();
            }
        }
        self.refs(key, name).first().copied()
    }
    pub(crate) fn id(&self, key: u32) -> String {
        let n = self.node(key);
        canonical_id(
            n.attr(id_attribute(&n.local).map_or("", |(a, _)| a))
                .unwrap_or(""),
        )
    }
    pub(crate) fn of_type(&self, name: &str) -> Vec<u32> {
        self.elements
            .iter()
            .copied()
            .filter(|k| self.node(*k).local == name)
            .collect()
    }
    pub(crate) fn type_of(&self, key: u32) -> Result<u32> {
        let node = self.node(key);
        let id = self.id(key);
        let descriptor = id
            .split_once('_')
            .and_then(|(_, suffix)| suffix.get(..4))
            .and_then(|value| u32::from_str_radix(value, 16).ok())
            .filter(|value| (1..=5).contains(value))
            .ok_or_else(|| Error::xml("ADM ID 缺少有效类型"))?;
        let label = node
            .attr("typeLabel")
            .map(|v| u32::from_str_radix(v, 16).map_err(|_| Error::xml("无效 typeLabel")))
            .transpose()?;
        let definition = node
            .attr("typeDefinition")
            .map(|v| match v {
                "DirectSpeakers" => Ok(1),
                "Matrix" => Ok(2),
                "Objects" => Ok(3),
                "HOA" => Ok(4),
                "Binaural" => Ok(5),
                _ => Err(Error::xml("无效 typeDefinition")),
            })
            .transpose()?;
        if label.is_some() && definition.is_some() && label != definition {
            return Err(Error::xml("typeLabel 与 typeDefinition 不一致"));
        }
        if label.is_some_and(|value| value != descriptor)
            || definition.is_some_and(|value| value != descriptor)
        {
            return Err(Error::xml("typeLabel 或 typeDefinition 与 ADM ID 不一致"));
        }
        Ok(descriptor)
    }
    // libadm synchronizes TrackFormat <-> StreamFormat even when the XML only
    // declares one direction. Keep that relation separate from the lossless XML.
    fn resolve_track_streams(&mut self) -> Result<()> {
        let mut links = HashMap::new();
        for &key in self.ids.values() {
            let (reference, reverse) = match self.node(key).local.as_str() {
                "audioTrackFormat" => ("audioStreamFormatIDRef", false),
                "audioStreamFormat" => ("audioTrackFormatIDRef", true),
                _ => continue,
            };
            for other in self.refs(key, reference) {
                let (track, stream) = if reverse { (other, key) } else { (key, other) };
                if links.insert(track, stream).is_some_and(|old| old != stream) {
                    return Err(Error::xml(format!(
                        "{} 引用了相互矛盾的 audioStreamFormat",
                        self.id(track)
                    )));
                }
            }
        }
        self.track_streams = links;
        Ok(())
    }
    /// PCM 可直接引用 channel，其余情况沿 TrackFormat -> StreamFormat 解析。
    pub(crate) fn channel(&self, uid: u32) -> Option<u32> {
        if let Some(channel) = self.reference(uid, "audioChannelFormatIDRef") {
            return Some(channel);
        }
        let tf = self.reference(uid, "audioTrackFormatIDRef")?;
        let sf = *self.track_streams.get(&tf)?;
        self.reference(sf, "audioChannelFormatIDRef")
    }
    // Match the range-checked parameters consumed from libadm. In particular,
    // accepting a huge finite azimuth could stall downstream angle wrapping.
    fn validate_ranges(&self, key: u32) -> Result<()> {
        let node = self.node(key);
        if node.local == "audioObject" {
            for (name, maximum) in [("importance", 10.0), ("dialogue", 2.0)] {
                if let Some(value) = node.attr(name) {
                    validate_float_range(value, 0.0, maximum, name)?;
                }
            }
            for child in self.children(key, "positionOffset") {
                self.validate_position(child, true)?;
            }
        }
        if node.local == "audioChannelFormat" {
            let channel_type = self.type_of(key)?;
            for block in self.children(key, "audioBlockFormat") {
                for child in self.all_children(block) {
                    let value = self.node(child);
                    match value.local.as_str() {
                        "position" if matches!(channel_type, 1 | 3) => {
                            self.validate_position(child, false)?
                        }
                        "diffuse" | "objectDivergence" if channel_type == 3 => {
                            validate_float_range(&value.text, 0.0, 1.0, &value.local)?
                        }
                        "importance" => validate_float_range(&value.text, 0.0, 10.0, "importance")?,
                        _ => {}
                    }
                    let attributes: &[(&str, f32)] = match value.local.as_str() {
                        "objectDivergence" if channel_type == 3 => {
                            &[("azimuthRange", 180.0), ("positionRange", 1.0)]
                        }
                        "channelLock" if channel_type == 3 => &[("maxDistance", 2.0)],
                        _ => &[],
                    };
                    for &(name, maximum) in attributes {
                        if let Some(text) = value.attr(name) {
                            validate_float_range(text, 0.0, maximum, name)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn validate_position(&self, key: u32, offset: bool) -> Result<()> {
        let node = self.node(key);
        let coordinate = node
            .attr("coordinate")
            .ok_or_else(|| Error::xml("position 缺少 coordinate"))?;
        let (minimum, maximum) = match coordinate {
            "azimuth" => {
                if offset {
                    (-360.0, 360.0)
                } else {
                    (-180.0, 180.0)
                }
            }
            "elevation" => {
                if offset {
                    (-180.0, 180.0)
                } else {
                    (-90.0, 90.0)
                }
            }
            "distance" if !offset => (
                0.0,
                if node.attr("bound").is_some() {
                    1.0
                } else {
                    f32::MAX
                },
            ),
            "distance" | "X" | "Y" | "Z" => (-f32::MAX, f32::MAX),
            _ => return Err(Error::xml("无效 position coordinate")),
        };
        validate_float_range(&node.text, minimum, maximum, coordinate)
    }
    fn check_cycles(&self, kind: &str, edge: &str) -> Result<()> {
        let mut marks = HashMap::<u32, u8>::new();
        for root in self.of_type(kind) {
            let mut work = vec![(root, false)];
            while let Some((key, exiting)) = work.pop() {
                if exiting {
                    marks.insert(key, 2);
                    continue;
                }
                match marks.get(&key) {
                    Some(1) => return Err(Error::xml(format!("{kind} 引用包含循环"))),
                    Some(2) => continue,
                    _ => {}
                }
                marks.insert(key, 1);
                work.push((key, true));
                for next in self.refs(key, edge) {
                    work.push((next, false));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn number(text: &str) -> Result<f64> {
    let value = text
        .trim()
        .parse::<f64>()
        .map_err(|_| Error::xml(format!("无效数值: {text}")))?;
    if !value.is_finite() {
        return Err(Error::xml("ADM 数值必须有限"));
    }
    Ok(value)
}
/// 解析 ADM 布尔文本，接受 `0` / `false` 与 `1` / `true`，返回记录使用的整数标志。
pub fn boolean(text: &str) -> Result<u32> {
    match text.trim() {
        "0" | "false" => Ok(0),
        "1" | "true" => Ok(1),
        _ => Err(Error::xml(format!("无效布尔值: {text}"))),
    }
}
/// 将 ADM 小数或分数时间换算为整数采样，采用与文档投影相同的舍入方式。
pub fn samples(time: &str, rate: u32) -> Result<u64> {
    let (clock, fraction) = time
        .split_once('.')
        .ok_or_else(|| Error::xml("无效 ADM 时间"))?;
    let parts = clock
        .split(':')
        .map(|p| p.parse::<u128>().map_err(|_| Error::xml("无效 ADM 时间")))
        .collect::<Result<Vec<_>>>()?;
    if parts.len() != 3 {
        return Err(Error::xml("无效 ADM 时间"));
    }
    let whole = parts[0]
        .checked_mul(3600)
        .and_then(|n| parts[1].checked_mul(60).and_then(|m| n.checked_add(m)))
        .and_then(|n| n.checked_add(parts[2]))
        .ok_or_else(|| Error::xml("ADM 时间溢出"))?;
    let ns = if let Some((n, d)) = fraction.split_once('S') {
        let numerator = n
            .parse::<u128>()
            .map_err(|_| Error::xml("无效 ADM 分数时间"))?;
        let denominator = d
            .parse::<u128>()
            .map_err(|_| Error::xml("无效 ADM 分数时间"))?;
        if denominator == 0 {
            return Err(Error::xml("ADM 时间分母为零"));
        }
        numerator
            .checked_mul(1_000_000_000)
            .ok_or_else(|| Error::xml("ADM 时间溢出"))?
            / denominator
    } else {
        if fraction.is_empty()
            || fraction.len() > 9
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Error::xml("无效 ADM 小数时间"));
        }
        fraction
            .parse::<u128>()
            .map_err(|_| Error::xml("无效 ADM 时间"))?
            * 10u128.pow(9 - fraction.len() as u32)
    };
    let ns = whole
        .checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(ns))
        .ok_or_else(|| Error::xml("ADM 时间溢出"))?;
    let value = ns
        .checked_mul(u128::from(rate))
        .and_then(|v| v.checked_add(500_000_000))
        .ok_or_else(|| Error::xml("ADM 时间溢出"))?
        / 1_000_000_000;
    Ok(value.min(u128::from(u64::MAX)) as u64)
}

pub(crate) fn validate_float_range(
    text: &str,
    minimum: f32,
    maximum: f32,
    name: &str,
) -> Result<()> {
    let value = number(text)? as f32;
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(Error::xml(format!(
            "ADM {name} 超出范围 [{minimum}, {maximum}]"
        )));
    }
    Ok(())
}
