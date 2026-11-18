//! 原生 ADM 扩展。静态元数据按所属层级保存，不改写为 Atmos 的相近字段。
use crate::{Error, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectDivergence {
    pub value: f64,
    pub azimuth_range: Option<f64>,
    pub position_range: Option<f64>,
}
impl ObjectDivergence {
    pub(crate) fn validate(self) -> Result<()> {
        if self.azimuth_range.is_some() && self.position_range.is_some() {
            return Err(Error::invalid(
                "objectDivergence cannot combine azimuthRange with positionRange",
            ));
        }
        range(self.value, 0., 1.)?;
        if let Some(value) = self.azimuth_range {
            range(value, 0., 180.)?;
        }
        if let Some(value) = self.position_range {
            range(value, 0., 1.)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenEdge {
    Left,
    Right,
    Top,
    Bottom,
}
impl ScreenEdge {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

/// 不包含 ID 引用的 ADM 静态子树。仅接受本库已识别的元素和属性，写入时统一转义。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdmElement {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<AdmElement>,
}

/// programme、content、object、pack、channel 分别拥有独立实例。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdmMetadata {
    pub attributes: Vec<(String, String)>,
    pub elements: Vec<AdmElement>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Level {
    Programme,
    Content,
    Object,
    Pack,
    Channel,
    BedGeometry,
}
impl Level {
    pub(crate) fn root(self) -> &'static str {
        match self {
            Self::Programme => "audioProgramme",
            Self::Content => "audioContent",
            Self::Object => "audioObject",
            Self::Pack => "audioPackFormat",
            Self::Channel => "audioChannelFormat",
            Self::BedGeometry => "DirectSpeakers geometry",
        }
    }
}

pub(crate) fn range(value: f64, min: f64, max: f64) -> Result<()> {
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(Error::invalid(
            "ADM numeric value outside its permitted range",
        ));
    }
    Ok(())
}

/// 每个已识别节点的扩展属性、子元素；结构引用由主序列化器生成，不能从扩展注入。
pub(crate) fn schema(name: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match name {
        "audioProgramme" => (
            &["audioProgrammeLanguage", "maxDuckingDepth"],
            &[
                "audioProgrammeLabel",
                "loudnessMetadata",
                "audioProgrammeReferenceScreen",
            ],
        ),
        "audioContent" => (
            &["audioContentLanguage"],
            &["audioContentLabel", "loudnessMetadata"],
        ),
        "audioObject" => (
            &["importance", "dialogue", "interact", "disableDucking"],
            &[
                "audioObjectLabel",
                "gain",
                "mute",
                "positionOffset",
                "audioObjectInteraction",
            ],
        ),
        "audioPackFormat" => (&["importance"], &[]),
        "audioChannelFormat" => (&[], &["frequency", "channelFrequency"]),
        "DirectSpeakers geometry" => (&[], &["position", "cartesian"]),
        "position" => (&["coordinate", "bound", "screenEdgeLock"], &[]),
        "audioProgrammeLabel" | "audioContentLabel" | "audioObjectLabel" => (&["language"], &[]),
        "loudnessMetadata" => (
            &[
                "loudnessMethod",
                "loudnessRecType",
                "loudnessCorrectionType",
            ],
            &[
                "integratedLoudness",
                "loudnessRange",
                "maxTruePeak",
                "maxMomentary",
                "maxShortTerm",
                "dialogueLoudness",
            ],
        ),
        "audioProgrammeReferenceScreen" => (
            &["aspectRatio"],
            &["aspectRatio", "screenCentrePosition", "screenWidth"],
        ),
        "screenCentrePosition" => (&["X", "Y", "Z", "azimuth", "elevation", "distance"], &[]),
        "screenWidth" => (&["X", "azimuth"], &[]),
        "positionOffset" => (&["coordinate"], &[]),
        "audioObjectInteraction" => (
            &["onOffInteract", "gainInteract", "positionInteract"],
            &["gainInteractionRange", "positionInteractionRange"],
        ),
        "gain" => (&["gainUnit"], &[]),
        "gainInteractionRange" => (&["bound", "gainUnit"], &[]),
        "positionInteractionRange" => (&["bound", "coordinate"], &[]),
        "frequency" | "channelFrequency" => (&["typeDefinition"], &[]),
        _ => (&[], &[]),
    }
}

impl AdmMetadata {
    /// 将对象层 gain / mute 应用于独立的 block 增益，供无此层级的目标使用。
    pub fn effective_gain(&self, block: crate::Gain) -> Result<crate::Gain> {
        if self
            .elements
            .iter()
            .any(|e| e.name == "mute" && matches!(e.text.trim(), "true" | "1"))
        {
            return Ok(crate::Gain::Silence);
        }
        let Some(gain) = self.elements.iter().find(|e| e.name == "gain") else {
            return Ok(block);
        };
        let v = numeric(&gain.text)?;
        let factor = if gain
            .attributes
            .iter()
            .any(|(k, v)| k == "gainUnit" && v == "dB")
        {
            libm::pow(10., v / 20.)
        } else {
            v
        };
        Ok(if factor == 1. {
            block
        } else {
            crate::Gain::Linear(factor * block.linear())
        })
    }
    fn bed_points(&self) -> Result<BedPoints<'_>> {
        let mut points = BTreeMap::new();
        for node in self.elements.iter().filter(|n| n.name == "position") {
            let attr = |key| {
                node.attributes
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str())
            };
            let axis =
                attr("coordinate").ok_or_else(|| Error::invalid("Missing bed coordinate"))?;
            points.insert(
                (axis, attr("bound").unwrap_or("")),
                (numeric(&node.text)?, attr("screenEdgeLock")),
            );
        }
        if points.contains_key(&("X", "")) {
            points.entry(("Z", "")).or_insert((0., None));
        } else if points.contains_key(&("azimuth", "")) {
            points.entry(("distance", "")).or_insert((1., None));
        }
        Ok(points)
    }
    pub(crate) fn is_canonical_bed(&self, expected: [f64; 3]) -> Result<bool> {
        if self.is_empty() {
            return Ok(true);
        }
        let points = self.bed_points()?;
        Ok(points.len() == 3
            && ["X", "Y", "Z"]
                .into_iter()
                .zip(expected)
                .all(|(axis, v)| points.get(&(axis, "")) == Some(&(v, None))))
    }
    pub fn is_empty(&self) -> bool {
        self.attributes.is_empty() && self.elements.is_empty()
    }
    pub(crate) fn validate(&self, level: Level) -> Result<()> {
        validate_parts(level.root(), &self.attributes, "", &self.elements, 0)?;
        if matches!(level, Level::BedGeometry) && !self.is_empty() {
            let mut axes = std::collections::BTreeSet::new();
            let mut cartesian = self
                .elements
                .iter()
                .find(|e| e.name == "cartesian")
                .map(|e| matches!(e.text.trim(), "1" | "true"));
            for node in &self.elements {
                if node.name == "cartesian" {
                    continue;
                }
                let coordinate = node
                    .attributes
                    .iter()
                    .find(|(k, _)| k == "coordinate")
                    .ok_or_else(|| Error::invalid("Bed position is missing its coordinate"))?;
                let uses_cartesian = matches!(coordinate.1.as_str(), "X" | "Y" | "Z");
                if cartesian.is_some_and(|v| v != uses_cartesian) {
                    return Err(Error::invalid(
                        "Bed position coordinate system is inconsistent",
                    ));
                }
                cartesian = Some(uses_cartesian);
                if !node.attributes.iter().any(|(k, _)| k == "bound") {
                    axes.insert(coordinate.1.as_str());
                }
            }
            let required = if cartesian == Some(true) {
                ["X", "Y"]
            } else {
                ["azimuth", "elevation"]
            };
            if !required.iter().all(|key| axes.contains(*key)) {
                return Err(Error::invalid(
                    "Bed geometry requires two nominal position axes",
                ));
            }
        }
        Ok(())
    }
    #[cfg(feature = "read")]
    pub(crate) fn has_loudness(&self) -> bool {
        self.elements.iter().any(|e| e.name == "loudnessMetadata")
    }
    #[cfg(feature = "read")]
    pub(crate) fn clear_loudness(&mut self) {
        self.elements.retain(|e| e.name != "loudnessMetadata");
    }
    pub(crate) fn fields(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.attributes
            .iter()
            .filter(|(name, value)| name != "importance" || value.trim().parse::<u8>() != Ok(10))
            .map(|(name, _)| field(name))
            .chain(
                self.elements
                    .iter()
                    .filter(|e| !matches!(e.name.as_str(), "gain" | "mute"))
                    .map(|e| field(&e.name)),
            )
    }
}
type BedPoints<'a> = BTreeMap<(&'a str, &'a str), (f64, Option<&'a str>)>;
pub(crate) fn field(name: &str) -> &'static str {
    match name {
        "audioProgrammeLanguage" | "audioContentLanguage" => "language",
        "audioProgrammeLabel" | "audioContentLabel" | "audioObjectLabel" => "labels",
        "loudnessMetadata" => "loudness",
        "audioProgrammeReferenceScreen" => "reference_screen",
        "positionOffset" => "position_offset",
        "importance" => "importance",
        "dialogue" => "dialog",
        "frequency" | "channelFrequency" => "frequency",
        "interact" | "audioObjectInteraction" => "interaction",
        "disableDucking" | "maxDuckingDepth" => "ducking",
        _ => "unknown_field",
    }
}
fn numeric(text: &str) -> Result<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::invalid("ADM metadata requires a finite number"))
}
fn choice(text: &str, choices: &[&str]) -> Result<()> {
    if !choices.contains(&text) {
        return Err(Error::invalid("Invalid ADM metadata enumeration"));
    }
    Ok(())
}
fn validate_parts(
    name: &str,
    attrs: &[(String, String)],
    text: &str,
    children: &[AdmElement],
    depth: usize,
) -> Result<()> {
    if depth > 4 {
        return Err(Error::invalid("ADM metadata nesting exceeds its schema"));
    }
    let (allowed_attrs, allowed_children) = schema(name);
    let mut keys = std::collections::BTreeSet::new();
    for (key, value) in attrs {
        if !allowed_attrs.contains(&key.as_str()) || !keys.insert(key) {
            return Err(Error::invalid(format!(
                "Unknown or duplicate ADM {name} attribute {key}"
            )));
        }
        crate::model::valid_text(value)?;
        match key.as_str() {
            "importance" | "dialogue" => {
                let max = if key == "importance" { 10 } else { 2 };
                value
                    .trim()
                    .parse::<u8>()
                    .ok()
                    .filter(|n| *n <= max)
                    .ok_or_else(|| {
                        Error::invalid(format!("ADM {key} requires an integer in 0..={max}"))
                    })?;
            }
            "interact" | "disableDucking" | "onOffInteract" | "gainInteract"
            | "positionInteract" => choice(value, &["0", "1", "false", "true"])?,
            "bound" => choice(value, &["min", "max"])?,
            "gainUnit" => choice(value, &["linear", "dB"])?,
            "coordinate" => choice(value, &["X", "Y", "Z", "azimuth", "elevation", "distance"])?,
            "typeDefinition" => choice(value, &["lowPass", "highPass"])?,
            "X" | "Y" | "Z" | "azimuth" | "elevation" | "distance" | "maxDuckingDepth" => {
                numeric(value)?;
            }
            _ => {}
        }
    }
    crate::model::valid_text(text)?;
    match name {
        "position"
        | "integratedLoudness"
        | "maxTruePeak"
        | "maxMomentary"
        | "maxShortTerm"
        | "dialogueLoudness"
        | "positionOffset"
        | "gainInteractionRange"
        | "positionInteractionRange" => {
            numeric(text)?;
        }
        "loudnessRange" | "frequency" | "channelFrequency" => {
            range(numeric(text)?, 0., f64::MAX)?;
        }
        "aspectRatio" => {
            if numeric(text)? <= 0. {
                return Err(Error::invalid("Screen aspectRatio must be positive"));
            }
        }
        "audioProgrammeLabel" | "audioContentLabel" | "audioObjectLabel" => {}
        "gain" => {
            let linear = !attrs.iter().any(|(k, v)| k == "gainUnit" && v == "dB");
            let n = numeric(text)?;
            if linear && n < 0. || !linear && !libm::pow(10., n / 20.).is_finite() {
                return Err(Error::invalid("Invalid ADM object gain"));
            }
        }
        "mute" | "cartesian" => choice(text.trim(), &["0", "1", "true", "false"])?,
        _ if !text.trim().is_empty() => return Err(Error::invalid("Unexpected ADM metadata text")),
        _ => {}
    }
    let attr = |key: &str| {
        attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    if matches!(
        name,
        "position" | "positionOffset" | "positionInteractionRange"
    ) && attr("coordinate").is_none()
        || matches!(name, "gainInteractionRange" | "positionInteractionRange")
            && attr("bound").is_none()
        || matches!(name, "frequency" | "channelFrequency") && attr("typeDefinition").is_none()
    {
        return Err(Error::invalid("Missing required ADM metadata attribute"));
    }
    if matches!(name, "position" | "positionOffset") {
        let value = numeric(text)?;
        let coordinate = attr("coordinate").unwrap();
        match (coordinate, name) {
            ("azimuth", "positionOffset") => range(value, -360., 360.)?,
            ("elevation", "positionOffset") => range(value, -180., 180.)?,
            ("azimuth", _) => range(value, -180., 180.)?,
            ("elevation", _) => range(value, -90., 90.)?,
            ("distance", "position") if attr("bound").is_some() => range(value, 0., 1.)?,
            ("distance", "position") => range(value, 0., f64::from(f32::MAX))?,
            _ => range(value, -f64::from(f32::MAX), f64::from(f32::MAX))?,
        }
        if let Some(edge) = attr("screenEdgeLock") {
            match coordinate {
                "X" | "azimuth" => choice(edge, &["left", "right"])?,
                "Z" | "elevation" => choice(edge, &["top", "bottom"])?,
                _ => {
                    return Err(Error::invalid(
                        "screenEdgeLock does not match its coordinate",
                    ));
                }
            }
        }
    }
    if matches!(name, "screenCentrePosition" | "screenWidth") {
        let cart = attrs
            .iter()
            .any(|(k, _)| ["X", "Y", "Z"].contains(&k.as_str()));
        let polar = attrs
            .iter()
            .any(|(k, _)| ["azimuth", "elevation", "distance"].contains(&k.as_str()));
        if cart && polar || attrs.is_empty() {
            return Err(Error::invalid(
                "Mixed or missing reference-screen coordinates",
            ));
        }
    }
    let mut singleton = std::collections::BTreeSet::new();
    for child in children {
        if !allowed_children.contains(&child.name.as_str()) {
            return Err(Error::invalid(format!(
                "Unknown ADM {name} child {}",
                child.name
            )));
        }
        let repeated = matches!(
            child.name.as_str(),
            "audioProgrammeLabel"
                | "audioContentLabel"
                | "audioObjectLabel"
                | "loudnessMetadata"
                | "positionOffset"
                | "position"
                | "gainInteractionRange"
                | "positionInteractionRange"
                | "frequency"
                | "channelFrequency"
        );
        let discriminator = if repeated {
            let mut keys: Vec<_> = child
                .attributes
                .iter()
                .filter(|(k, _)| ["coordinate", "bound", "typeDefinition"].contains(&k.as_str()))
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            keys.sort_unstable();
            keys.join(";")
        } else {
            String::new()
        };
        if (!repeated || !discriminator.is_empty())
            && !singleton.insert((
                if child.name == "channelFrequency" {
                    "frequency"
                } else {
                    child.name.as_str()
                },
                discriminator,
            ))
        {
            return Err(Error::invalid("Duplicate ADM metadata element"));
        }
        validate_parts(
            &child.name,
            &child.attributes,
            &child.text,
            &child.children,
            depth + 1,
        )?;
    }
    if name == "audioProgrammeReferenceScreen" {
        let attribute_ratio = attr("aspectRatio").map(numeric).transpose()?;
        let child_ratio = children
            .iter()
            .find(|e| e.name == "aspectRatio")
            .map(|e| numeric(&e.text))
            .transpose()?;
        if attribute_ratio.is_some_and(|v| v <= 0.)
            || attribute_ratio
                .zip(child_ratio)
                .is_some_and(|(a, b)| a != b)
        {
            return Err(Error::invalid(
                "Invalid or conflicting reference-screen aspectRatio",
            ));
        }
        let centre = children.iter().find(|e| e.name == "screenCentrePosition");
        let width = children.iter().find(|e| e.name == "screenWidth");
        let attrs = centre.into_iter().chain(width).flat_map(|e| &e.attributes);
        let cartesian = attrs
            .clone()
            .any(|(k, _)| ["X", "Y", "Z"].contains(&k.as_str()));
        let polar = attrs
            .clone()
            .any(|(k, _)| ["azimuth", "elevation", "distance"].contains(&k.as_str()));
        if cartesian && polar {
            return Err(Error::invalid("Mixed reference-screen coordinate systems"));
        }
        let get = |element: Option<&AdmElement>, key: &str| -> Result<Option<f64>> {
            element
                .and_then(|e| e.attributes.iter().find(|(k, _)| k == key))
                .map(|(_, v)| numeric(v))
                .transpose()
        };
        let width = get(width, if cartesian { "X" } else { "azimuth" })?;
        if cartesian && width.is_none() {
            return Err(Error::invalid(
                "Cartesian reference screen requires width X",
            ));
        }
        let width = width.unwrap_or(58.);
        let names = if cartesian {
            ["X", "Y", "Z"]
        } else {
            ["azimuth", "elevation", "distance"]
        };
        let defaults = if cartesian {
            [0., 1., 0.]
        } else {
            [0., 0., 1.]
        };
        let mut coordinates = defaults;
        for (i, key) in names.iter().enumerate() {
            coordinates[i] = get(centre, key)?.unwrap_or(defaults[i]);
        }
        if width <= 0.
            || cartesian && coordinates == [0.; 3]
            || !cartesian
                && (width >= 180.
                    || coordinates[2] <= 0.
                    || !(-180. ..=180.).contains(&coordinates[0])
                    || !(-90. ..90.).contains(&coordinates[1]))
        {
            return Err(Error::invalid("Invalid reference-screen geometry"));
        }
    }
    Ok(())
}
