//! Jewelry alloys: the metals the studio actually casts and plates.
//!
//! PLACEHOLDER: every alloy maps to its dominant pure metal. To be replaced
//! by optics derived from measured alloy reflectance (blend of pure-metal
//! normal-incidence reflectance spectra by composition, inverted to n,k with
//! [`crate::fresnel::gulbrandsen`]), documented per alloy. Keep the API.

use crate::metals::{self, ComplexIor};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Alloy {
    FineSilver,
    /// .925 sterling: 92.5% Ag, 7.5% Cu.
    Sterling,
    Gold24k,
    Gold18kYellow,
    /// 14k yellow, also the look of 14/20 gold-filled.
    Gold14kYellow,
    Gold10kYellow,
    Gold14kRose,
    /// Rhodium-plated white gold reads as rhodium.
    Gold14kWhite,
    Platinum,
    Palladium,
    Rhodium,
    Copper,
    /// Jewelers' bronze, ~88% Cu 12% Sn.
    Bronze,
    /// Yellow brass, ~70% Cu 30% Zn.
    Brass,
}

impl Alloy {
    pub const ALL: &'static [Alloy] = &[
        Alloy::FineSilver,
        Alloy::Sterling,
        Alloy::Gold24k,
        Alloy::Gold18kYellow,
        Alloy::Gold14kYellow,
        Alloy::Gold10kYellow,
        Alloy::Gold14kRose,
        Alloy::Gold14kWhite,
        Alloy::Platinum,
        Alloy::Palladium,
        Alloy::Rhodium,
        Alloy::Copper,
        Alloy::Bronze,
        Alloy::Brass,
    ];

    /// Optical constants for rendering.
    pub fn ior(self) -> ComplexIor {
        let m = match self {
            Alloy::FineSilver | Alloy::Sterling => "Ag",
            Alloy::Gold24k | Alloy::Gold18kYellow | Alloy::Gold14kYellow | Alloy::Gold10kYellow => "Au",
            Alloy::Gold14kRose | Alloy::Copper | Alloy::Bronze | Alloy::Brass => "Cu",
            Alloy::Gold14kWhite | Alloy::Platinum | Alloy::Palladium | Alloy::Rhodium => "Ag",
        };
        metals::get(m).expect("placeholder metal")
    }

    /// Density in g/cm³, for weight estimates from mesh volume.
    pub fn density(self) -> f32 {
        match self {
            Alloy::FineSilver => 10.49,
            Alloy::Sterling => 10.36,
            Alloy::Gold24k => 19.32,
            Alloy::Gold18kYellow => 15.58,
            Alloy::Gold14kYellow => 13.07,
            Alloy::Gold10kYellow => 11.57,
            Alloy::Gold14kRose => 13.26,
            Alloy::Gold14kWhite => 12.61,
            Alloy::Platinum => 21.45,
            Alloy::Palladium => 12.02,
            Alloy::Rhodium => 12.41,
            Alloy::Copper => 8.96,
            Alloy::Bronze => 8.8,
            Alloy::Brass => 8.53,
        }
    }

    /// Stable machine name (`"sterling"`, `"gold14k"`, …).
    pub fn key(self) -> &'static str {
        match self {
            Alloy::FineSilver => "fine-silver",
            Alloy::Sterling => "sterling",
            Alloy::Gold24k => "gold24k",
            Alloy::Gold18kYellow => "gold18k",
            Alloy::Gold14kYellow => "gold14k",
            Alloy::Gold10kYellow => "gold10k",
            Alloy::Gold14kRose => "rose14k",
            Alloy::Gold14kWhite => "white14k",
            Alloy::Platinum => "platinum",
            Alloy::Palladium => "palladium",
            Alloy::Rhodium => "rhodium",
            Alloy::Copper => "copper",
            Alloy::Bronze => "bronze",
            Alloy::Brass => "brass",
        }
    }

    /// Display name (`".925 sterling silver"`).
    pub fn display(self) -> &'static str {
        match self {
            Alloy::FineSilver => "Fine silver",
            Alloy::Sterling => ".925 sterling silver",
            Alloy::Gold24k => "24k gold",
            Alloy::Gold18kYellow => "18k yellow gold",
            Alloy::Gold14kYellow => "14k yellow gold",
            Alloy::Gold10kYellow => "10k yellow gold",
            Alloy::Gold14kRose => "14k rose gold",
            Alloy::Gold14kWhite => "14k white gold",
            Alloy::Platinum => "Platinum",
            Alloy::Palladium => "Palladium",
            Alloy::Rhodium => "Rhodium",
            Alloy::Copper => "Copper",
            Alloy::Bronze => "Bronze",
            Alloy::Brass => "Brass",
        }
    }

    /// Parse a key or a common alias (`silver`, `gold`, `gold-filled`, `rose`).
    pub fn from_name(s: &str) -> Option<Alloy> {
        let s = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        let a = match s.as_str() {
            "silver" | "sterling" | "925" | "sterling-silver" => Alloy::Sterling,
            "fine-silver" | "999" => Alloy::FineSilver,
            "gold" | "yellow" | "gold14k" | "14k" | "gold-filled" | "gf" | "yellow-gold" => Alloy::Gold14kYellow,
            "gold24k" | "24k" => Alloy::Gold24k,
            "gold18k" | "18k" => Alloy::Gold18kYellow,
            "gold10k" | "10k" => Alloy::Gold10kYellow,
            "rose" | "rose-gold" | "rose14k" => Alloy::Gold14kRose,
            "white" | "white-gold" | "white14k" => Alloy::Gold14kWhite,
            "platinum" | "pt" => Alloy::Platinum,
            "palladium" | "pd" => Alloy::Palladium,
            "rhodium" | "rh" => Alloy::Rhodium,
            "copper" | "cu" => Alloy::Copper,
            "bronze" => Alloy::Bronze,
            "brass" => Alloy::Brass,
            _ => return None,
        };
        Some(a)
    }
}
