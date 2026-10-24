//! Which parts are metal and which are stones, and what a stone looks like
//! in a viewer that only knows glTF PBR.

use crate::parts::PartTable;

/// glTF look of a stone part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gem {
    /// Table key (`"sapphire"`, `"diamond"`, …).
    pub name: &'static str,
    /// Linear RGB tint (`baseColorFactor`).
    pub color: [f32; 3],
    /// `KHR_materials_ior`.
    pub ior: f32,
    /// `KHR_materials_transmission` (0 = opaque stone such as onyx or pearl).
    pub transmission: f32,
    pub roughness: f32,
}

/// What a part is exported as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PartKind {
    Metal,
    Gem(Gem),
}

const fn gem(name: &'static str, color: [f32; 3], ior: f32, transmission: f32, roughness: f32) -> Gem {
    Gem { name, color, ior, transmission, roughness }
}

const DIAMOND: Gem = gem("diamond", [1.0, 1.0, 1.0], 2.42, 1.0, 0.0);

/// Species words. Words of four letters or more match anywhere in the text
/// (`"emerald_dew"`, `"Rhinestone"`); shorter ones must be a whole word.
/// The longest match wins (`"moonstone"` over `"stone"`).
const GEMS: &[(&str, Gem)] = &[
    ("diamond", DIAMOND),
    ("moissanite", gem("moissanite", [1.0, 1.0, 0.98], 2.65, 1.0, 0.0)),
    ("zirconia", gem("cubic_zirconia", [1.0, 1.0, 1.0], 2.15, 1.0, 0.0)),
    ("cz", gem("cubic_zirconia", [1.0, 1.0, 1.0], 2.15, 1.0, 0.0)),
    ("zircon", gem("zircon", [1.0, 1.0, 1.0], 1.95, 1.0, 0.0)),
    ("sapphire", gem("sapphire", [0.03, 0.10, 0.55], 1.77, 1.0, 0.0)),
    ("ruby", gem("ruby", [0.60, 0.01, 0.04], 1.77, 1.0, 0.0)),
    ("emerald", gem("emerald", [0.03, 0.45, 0.15], 1.58, 1.0, 0.0)),
    ("amethyst", gem("amethyst", [0.35, 0.10, 0.55], 1.54, 1.0, 0.0)),
    ("garnet", gem("garnet", [0.30, 0.01, 0.02], 1.79, 1.0, 0.0)),
    ("topaz", gem("topaz", [0.45, 0.70, 0.90], 1.62, 1.0, 0.0)),
    ("citrine", gem("citrine", [0.90, 0.50, 0.05], 1.54, 1.0, 0.0)),
    ("peridot", gem("peridot", [0.40, 0.65, 0.05], 1.67, 1.0, 0.0)),
    ("aquamarine", gem("aquamarine", [0.45, 0.75, 0.85], 1.58, 1.0, 0.0)),
    ("tanzanite", gem("tanzanite", [0.20, 0.18, 0.60], 1.70, 1.0, 0.0)),
    ("tourmaline", gem("tourmaline", [0.75, 0.20, 0.40], 1.62, 1.0, 0.0)),
    ("spinel", gem("spinel", [0.55, 0.03, 0.08], 1.72, 1.0, 0.0)),
    ("morganite", gem("morganite", [0.95, 0.65, 0.65], 1.58, 1.0, 0.0)),
    ("alexandrite", gem("alexandrite", [0.15, 0.40, 0.30], 1.75, 1.0, 0.0)),
    ("quartz", gem("quartz", [1.0, 1.0, 1.0], 1.54, 1.0, 0.0)),
    ("crystal", gem("crystal", [1.0, 1.0, 1.0], 1.54, 1.0, 0.0)),
    ("glass", gem("glass", [1.0, 1.0, 1.0], 1.50, 1.0, 0.0)),
    ("opal", gem("opal", [0.85, 0.87, 0.90], 1.45, 0.4, 0.1)),
    ("moonstone", gem("moonstone", [0.85, 0.88, 0.95], 1.52, 0.3, 0.15)),
    ("onyx", gem("onyx", [0.01, 0.01, 0.01], 1.54, 0.0, 0.03)),
    ("obsidian", gem("obsidian", [0.01, 0.01, 0.012], 1.49, 0.0, 0.02)),
    ("jet", gem("jet", [0.01, 0.01, 0.01], 1.66, 0.0, 0.05)),
    ("pearl", gem("pearl", [0.90, 0.87, 0.82], 1.53, 0.0, 0.25)),
    ("turquoise", gem("turquoise", [0.10, 0.55, 0.55], 1.61, 0.0, 0.35)),
    ("jade", gem("jade", [0.15, 0.45, 0.20], 1.66, 0.1, 0.25)),
    ("lapis", gem("lapis", [0.05, 0.10, 0.45], 1.50, 0.0, 0.30)),
    ("agate", gem("agate", [0.55, 0.35, 0.25], 1.54, 0.0, 0.10)),
];

/// Generic stone words: diamond-like, as argent's renderer defaults them.
const GENERIC: &[&str] = &["stone", "gem", "jewel", "cabochon", "brilliant"];

/// Colour words that override a stone's tint (whole words only).
const TINTS: &[(&str, [f32; 3])] = &[
    ("black", [0.01, 0.01, 0.01]),
    ("white", [1.0, 1.0, 1.0]),
    ("clear", [1.0, 1.0, 1.0]),
    ("pink", [0.90, 0.40, 0.55]),
    ("red", [0.60, 0.02, 0.04]),
    ("blue", [0.05, 0.15, 0.60]),
    ("green", [0.05, 0.45, 0.15]),
    ("yellow", [0.90, 0.70, 0.10]),
    ("purple", [0.35, 0.10, 0.55]),
];

const METALS: &[&str] = &[
    "metal",
    "silver",
    "sterling",
    "gold",
    "bronze",
    "brass",
    "copper",
    "platinum",
    "palladium",
    "titanium",
    "steel",
    "rhodium",
    "alloy",
    "argentium",
];

fn tokens(t: &str) -> Vec<&str> {
    t.split(|c: char| !c.is_ascii_alphabetic()).filter(|s| !s.is_empty()).collect()
}

fn word_in(word: &str, text: &str, toks: &[&str]) -> bool {
    if word.len() >= 4 {
        text.contains(word)
    } else {
        toks.iter().any(|t| *t == word || (t.len() == word.len() + 1 && t.starts_with(word) && t.ends_with('s')))
    }
}

/// A stone named in free text (`"Emerald dew"`, `"stone_03"`, `"gem"`),
/// with a colour word applied (`"black_spinel"` is opaque black).
pub fn gem_from_text(text: &str) -> Option<Gem> {
    let t = text.to_ascii_lowercase();
    let toks = tokens(&t);
    let mut best: Option<(usize, Gem)> = None;
    for (w, g) in GEMS {
        if word_in(w, &t, &toks) && best.is_none_or(|(l, _)| w.len() > l) {
            best = Some((w.len(), *g));
        }
    }
    let mut g = match best {
        Some((_, g)) => g,
        None if GENERIC.iter().any(|w| word_in(w, &t, &toks)) => DIAMOND,
        None => return None,
    };
    for (w, c) in TINTS {
        if toks.contains(w) {
            g.color = *c;
            if *w == "black" {
                g.transmission = 0.0;
            }
        }
    }
    Some(g)
}

fn is_metal_text(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    METALS.iter().any(|m| t.contains(m))
}

/// Classify a part from its name and material text alone: a stone word in
/// the material decides; a metal word in the material makes it metal;
/// otherwise a stone word in the name makes it a gem.
pub fn classify_part(name: &str, material: Option<&str>) -> PartKind {
    if let Some(m) = material {
        if let Some(g) = gem_from_text(m) {
            return PartKind::Gem(g);
        }
        if is_metal_text(m) {
            return PartKind::Metal;
        }
    }
    gem_from_text(name).map_or(PartKind::Metal, PartKind::Gem)
}

/// [`classify_part`] for every part, except that a part holding more than
/// half of all triangles is metal unless its *material* names a stone:
/// casting files are often named after their stone
/// (`ophidian-amethyst-serpent.stl`), and the bulk of a jewel is metal.
pub(crate) fn classify_parts(table: &PartTable) -> Vec<PartKind> {
    let total: usize = table.counts.iter().sum();
    (0..table.len())
        .map(|p| {
            let material = table.materials[p].as_deref();
            let kind = classify_part(&table.names[p], material);
            let named_only = material.and_then(gem_from_text).is_none();
            if matches!(kind, PartKind::Gem(_)) && named_only && table.counts[p] * 2 > total {
                PartKind::Metal
            } else {
                kind
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(k: PartKind) -> &'static str {
        match k {
            PartKind::Metal => "metal",
            PartKind::Gem(g) => g.name,
        }
    }

    #[test]
    fn words() {
        assert_eq!(name(classify_part("stone_03", Some("gem"))), "diamond");
        assert_eq!(name(classify_part("Emerald dew", None)), "emerald");
        assert_eq!(name(classify_part("eye_left", Some("Sapphire"))), "sapphire");
        assert_eq!(name(classify_part("moonstone_cab", None)), "moonstone");
        assert_eq!(name(classify_part("arachne-the-weaver_size7", None)), "metal");
        assert_eq!(name(classify_part("stone_seat", Some("sterling"))), "metal");
        assert_eq!(name(classify_part("gems", None)), "diamond");
        assert_eq!(name(classify_part("gemini_bail", None)), "metal");
        assert_eq!(name(classify_part("cz_pave", None)), "cubic_zirconia");
        assert_eq!(name(classify_part("cubic zirconia", None)), "cubic_zirconia");
        let PartKind::Gem(g) = classify_part("black_spinel_03", None) else { panic!() };
        assert_eq!(g.name, "spinel");
        assert_eq!(g.transmission, 0.0);
        assert!(g.color[0] < 0.05);
    }
}
