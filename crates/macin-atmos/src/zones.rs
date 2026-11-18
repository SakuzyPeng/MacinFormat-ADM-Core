//! Conversion Tool 往返验证的区域预设；任意区域不猜测为最接近的预设。
use crate::Zone;

pub(crate) struct Mask {
    pub name: &'static str,
    /// minX, maxX, minY, maxY, minZ, maxZ。
    pub bounds: [f64; 6],
}
pub(crate) const ATTRIBUTES: [&str; 6] = ["minX", "maxX", "minY", "maxY", "minZ", "maxZ"];
const MASKS: [Mask; 11] = [
    Mask {
        name: "ZB",
        bounds: [-1., 1., -1., 1., -1., -0.4995],
    },
    Mask {
        name: "ZT",
        bounds: [-1., 1., -1., 1., 0.4995, 1.],
    },
    Mask {
        name: "ZM1",
        bounds: [-1., 1., -1., -0.41934, -0.499, 0.499],
    },
    Mask {
        name: "ZM2L",
        bounds: [-1., -0.75806, -0.41934, 0.83871, -0.499, 0.499],
    },
    Mask {
        name: "ZM2R",
        bounds: [0.75806, 1., -0.41934, 0.83871, -0.499, 0.499],
    },
    Mask {
        name: "ZM3L",
        bounds: [-1., -0.16129, 0.5, 1., -0.499, 0.499],
    },
    Mask {
        name: "ZM3R",
        bounds: [0.16129, 1., 0.5, 1., -0.499, 0.499],
    },
    Mask {
        name: "ZM3Lss",
        bounds: [-1., -0.51611, -0.707, 0.49999, -0.499, 0.499],
    },
    Mask {
        name: "ZM3Rss",
        bounds: [0.51611, 1., -0.707, 0.49999, -0.499, 0.499],
    },
    Mask {
        name: "ZM4",
        bounds: [-1., 1., -1., 0.83871, -0.499, 0.499],
    },
    Mask {
        name: "ZM5",
        bounds: [-1., 1., 0.5, 1., -0.499, 0.499],
    },
];
fn bits(zone: Zone, elevation: bool) -> u16 {
    let horizontal = match zone {
        Zone::All => 0,
        Zone::NoBack => 1 << 2,
        Zone::NoSides => (1 << 3) | (1 << 4),
        Zone::CenterBack => (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8),
        Zone::ScreenOnly => 1 << 9,
        Zone::SurroundOnly => 1 << 10,
    };
    horizontal | if elevation { 0 } else { 3 }
}
pub(crate) fn masks(zone: Zone, elevation: bool) -> impl Iterator<Item = &'static Mask> {
    let selected = bits(zone, elevation);
    MASKS
        .iter()
        .enumerate()
        .filter_map(move |(i, mask)| ((selected & (1 << i)) != 0).then_some(mask))
}
#[cfg(feature = "read")]
pub(crate) fn decode<'a>(
    values: impl IntoIterator<Item = (&'a str, [f64; 6])>,
) -> Option<(Zone, bool)> {
    let mut selected = 0;
    for (name, bounds) in values {
        let i = MASKS
            .iter()
            .position(|m| m.name == name && m.bounds == bounds)?;
        if selected & (1 << i) != 0 {
            return None;
        }
        selected |= 1 << i;
    }
    for zone in [
        Zone::All,
        Zone::NoBack,
        Zone::NoSides,
        Zone::CenterBack,
        Zone::ScreenOnly,
        Zone::SurroundOnly,
    ] {
        for elevation in [true, false] {
            if bits(zone, elevation) == selected {
                return Some((zone, elevation));
            }
        }
    }
    None
}
