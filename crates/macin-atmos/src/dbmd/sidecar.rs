/// EBU DD+ 第 7 段的 RF（compr1）与 Line（dynrng1）压缩配置原码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DdPlusCompression {
    pub compr1: u8,
    pub dynrng1: u8,
}

const PROFILES: [&str; 6] = [
    "none",
    "film_standard",
    "film_light",
    "music_standard",
    "music_light",
    "speech",
];

impl Default for DdPlusCompression {
    fn default() -> Self {
        Self {
            compr1: 2,
            dynrng1: 2,
        }
    }
}

impl DdPlusCompression {
    pub(crate) fn portable(self) -> bool {
        [self.compr1, self.dynrng1]
            .iter()
            .all(|&x| usize::from(x) < PROFILES.len())
    }

    pub(crate) fn use_portable_defaults(&mut self) {
        for x in [&mut self.compr1, &mut self.dynrng1] {
            if usize::from(*x) >= PROFILES.len() {
                *x = 2;
            }
        }
    }

    pub(crate) fn yaml(self) -> crate::Result<String> {
        let name = |x: u8| {
            PROFILES.get(usize::from(x)).ok_or_else(|| {
                crate::Error::invalid("No DAMF spelling for this DD+ compression profile")
            })
        };
        Ok(format!(
            "ddplus:\n  compr1: {}\n  dynrng1: {}\n",
            name(self.compr1)?,
            name(self.dynrng1)?
        ))
    }
}

/// 旧 DAMF 的同名 YAML 伴随文件。原文包含独立编码参数。
#[derive(Debug, Clone)]
pub struct DbmdSidecar {
    text: String,
    pub(crate) compression: DdPlusCompression,
    pub(crate) unmapped: bool,
    pub(crate) origin: Option<crate::ancillary::Origin>,
}

impl DbmdSidecar {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[cfg(feature = "read")]
    pub(crate) fn parse(text: String) -> crate::Result<Self> {
        let document = crate::yaml::parse_document(&text)?;
        let mut compression = DdPlusCompression::default();
        let mut unmapped = false;
        for (section, values) in document.as_mapping("DBMD sidecar")? {
            if section != "ddplus" {
                unmapped = true;
                continue;
            }
            for (name, value) in values.as_mapping("ddplus")? {
                let slot = match name.as_str() {
                    "compr1" => &mut compression.compr1,
                    "dynrng1" => &mut compression.dynrng1,
                    _ => {
                        unmapped = true;
                        continue;
                    }
                };
                let value = value.as_text(name)?;
                if let Some(code) = PROFILES.iter().position(|s| *s == value) {
                    *slot = code as u8;
                } else {
                    unmapped = true;
                }
            }
        }
        Ok(Self {
            text,
            compression,
            unmapped,
            origin: None,
        })
    }
}
