//! Gear identification: maps what a block knows about its capture to a gear family.
//!
//! A family names the rendered skin a block shows and its chain-strip icon. Hints are tried in
//! priority order: the `.nam` file's own make and model, the TONE3000 makes, then the tone title.
//! When none matches, the family is the default for the category, tinted by the tone type.

use serde_json::Value;

use crate::tone::{Tone, ToneModel};

/// Broad kind of gear a capture models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GearCategory {
    /// Stompbox.
    Pedal,
    /// Amplifier head or combo.
    Amp,
    /// Speaker cabinet impulse response.
    Cab,
    /// Amp, cab and microphone captured together.
    FullRig,
    /// Studio outboard such as compressors and preamps.
    Outboard,
    /// Room or reverb impulse response.
    Space,
    /// Not enough information.
    Unknown,
}

impl GearCategory {
    /// Parses a TONE3000 `gear` value or a `.nam` `gear_type` value.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match normalize_word(value).as_str() {
            "pedal" => Self::Pedal,
            "amp" | "preamp" | "pedal_amp" => Self::Amp,
            "cab" | "ir" => Self::Cab,
            "full_rig" | "rig" | "amp_cab" | "amp_pedal_cab" => Self::FullRig,
            "outboard" | "studio" => Self::Outboard,
            "space" | "room" | "reverb" => Self::Space,
            _ => Self::Unknown,
        }
    }
}

/// Tone character reported by the NAM trainer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToneType {
    /// Clean.
    Clean,
    /// Crunch or overdrive.
    Crunch,
    /// High gain.
    HiGain,
    /// Fuzz.
    Fuzz,
    /// Missing or unrecognised.
    Unknown,
}

impl ToneType {
    /// Parses the `tone_type` spelling written by the NAM trainer.
    #[must_use]
    pub fn from_nam(value: &str) -> Self {
        match normalize_word(value).as_str() {
            "clean" => Self::Clean,
            "crunch" | "overdrive" | "drive" => Self::Crunch,
            "hi_gain" | "high_gain" | "higain" | "distortion" => Self::HiGain,
            "fuzz" => Self::Fuzz,
            _ => Self::Unknown,
        }
    }
}

/// Gear family; each one names a skin and a chain icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GearFamily {
    /// Green overdrive with a treadle switch (TS808, TS9, SD-1).
    OdGreen,
    /// Transparent "gold" drive (Klon and its descendants).
    OdGold,
    /// Black distortion box (RAT, DS-1).
    Distortion,
    /// Wide fuzz box (Big Muff, Fuzz Face).
    Fuzz,
    /// Compact clean boost.
    Boost,
    /// Compact compressor pedal.
    Compressor,
    /// Unrecognised drive, distortion or fuzz pedal.
    PedalDefaultDark,
    /// Unrecognised clean or unknown pedal.
    PedalDefaultLight,
    /// High-gain head (5150, 6505, EVH).
    HeadHighGain,
    /// Modern US head (Rectifier family).
    HeadModernUs,
    /// British head (Plexi, JCM800, JVM).
    HeadBritish,
    /// Chime combo (AC30, AC15).
    ComboChime,
    /// Clean combo (Twin, Deluxe, JC series).
    ComboClean,
    /// Bass head (SVT, B-15).
    HeadBass,
    /// Unrecognised clean amp.
    AmpDefaultClean,
    /// Unrecognised crunch or overdrive amp.
    AmpDefaultCrunch,
    /// Unrecognised high-gain or unknown amp.
    AmpDefaultHiGain,
    /// Modern closed-back 4x12.
    Cab412Modern,
    /// Vintage British 4x12.
    Cab412Vintage,
    /// 2x12 cabinet.
    Cab212,
    /// 1x12 cabinet.
    Cab112,
    /// Bass cabinet (8x10, 4x10, 1x15).
    Cab810,
    /// Unrecognised cabinet.
    CabDefault,
    /// Amp, cab and microphone captured together.
    FullRig,
    /// Studio outboard.
    Outboard,
    /// Room or reverb impulse response.
    Space,
    /// Not enough information.
    Unknown,
}

impl GearFamily {
    /// Rendered skin identity, when this family has one.
    #[must_use]
    pub const fn skin_id(self) -> Option<&'static str> {
        Some(match self {
            Self::OdGreen => "od-green",
            Self::OdGold => "od-gold",
            Self::Distortion => "dist-black",
            Self::Fuzz => "fuzz",
            Self::Boost => "boost",
            Self::Compressor => "comp",
            Self::PedalDefaultDark => "default-dark",
            Self::PedalDefaultLight => "default-light",
            Self::HeadHighGain => "highgain",
            Self::HeadModernUs => "modern-us",
            Self::HeadBritish => "british",
            Self::ComboChime => "chime",
            Self::ComboClean => "clean",
            Self::HeadBass => "bass",
            Self::AmpDefaultClean => "default-clean",
            Self::AmpDefaultCrunch => "default-crunch",
            Self::AmpDefaultHiGain => "default-hi-gain",
            Self::Cab412Modern => "cab-412-modern",
            Self::Cab412Vintage => "cab-412-vintage",
            Self::Cab212 => "cab-212-chime",
            Self::Cab112 => "cab-112-clean",
            Self::Cab810 => "cab-810-bass",
            Self::CabDefault => "cab-default",
            Self::FullRig | Self::Outboard | Self::Space | Self::Unknown => return None,
        })
    }

    /// Chain-strip icon identity.
    #[must_use]
    pub const fn icon_id(self) -> &'static str {
        match self {
            Self::OdGreen => "od-green",
            Self::OdGold => "od-gold",
            Self::Distortion => "dist-black",
            Self::Fuzz => "fuzz",
            Self::Boost => "boost",
            Self::Compressor => "comp-pedal",
            Self::PedalDefaultDark | Self::PedalDefaultLight => "pedal-generic",
            Self::HeadHighGain => "head-highgain",
            Self::HeadModernUs => "head-recto",
            Self::HeadBritish => "head-brit",
            Self::ComboChime => "combo-chime",
            Self::ComboClean => "combo-clean",
            Self::HeadBass => "head-bass",
            Self::AmpDefaultClean | Self::AmpDefaultCrunch | Self::AmpDefaultHiGain => {
                "head-generic"
            }
            Self::Cab412Modern | Self::Cab412Vintage => "cab-412",
            Self::Cab212 | Self::CabDefault => "cab-212",
            Self::Cab112 => "cab-112",
            Self::Cab810 => "cab-810",
            Self::FullRig => "rig",
            Self::Outboard => "rack",
            Self::Space => "room",
            Self::Unknown => "unknown",
        }
    }

    /// Category this family belongs to.
    #[must_use]
    pub const fn category(self) -> GearCategory {
        match self {
            Self::OdGreen
            | Self::OdGold
            | Self::Distortion
            | Self::Fuzz
            | Self::Boost
            | Self::Compressor
            | Self::PedalDefaultDark
            | Self::PedalDefaultLight => GearCategory::Pedal,
            Self::HeadHighGain
            | Self::HeadModernUs
            | Self::HeadBritish
            | Self::ComboChime
            | Self::ComboClean
            | Self::HeadBass
            | Self::AmpDefaultClean
            | Self::AmpDefaultCrunch
            | Self::AmpDefaultHiGain => GearCategory::Amp,
            Self::Cab412Modern
            | Self::Cab412Vintage
            | Self::Cab212
            | Self::Cab112
            | Self::Cab810
            | Self::CabDefault => GearCategory::Cab,
            Self::FullRig => GearCategory::FullRig,
            Self::Outboard => GearCategory::Outboard,
            Self::Space => GearCategory::Space,
            Self::Unknown => GearCategory::Unknown,
        }
    }
}

/// Which hint decided the family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchSource {
    /// The `.nam` file's own make and model.
    NamMetadata,
    /// The TONE3000 makes.
    Catalog,
    /// Keywords in the tone title.
    Title,
    /// Only the category was known; the family is its default.
    Category,
    /// Nothing was known.
    None,
}

/// Everything a block knows about what its capture is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GearHints {
    /// TONE3000 or local gear type (`amp`, `pedal`, `cab`, `full-rig`, `outboard`, `space`).
    pub gear: Option<String>,
    /// TONE3000 makes, trimmed and non-empty.
    pub makes: Vec<String>,
    /// Tone title.
    pub title: String,
    /// `.nam` metadata `gear_make`.
    pub nam_make: Option<String>,
    /// `.nam` metadata `gear_model`.
    pub nam_model: Option<String>,
    /// `.nam` metadata `gear_type`.
    pub nam_gear_type: Option<String>,
    /// `.nam` metadata `tone_type`.
    pub tone_type: Option<String>,
}

impl GearHints {
    /// Collects hints from a tone and, optionally, its active model.
    ///
    /// The tone supplies the TONE3000 `gear`, `makes` and title. The model supplies the `.nam`
    /// metadata fields that local imports keep (`gear_make`, `gear_model`, `gear_type`,
    /// `tone_type`) and its own `gear` when the tone has none.
    #[must_use]
    pub fn from_tone(tone: &Tone, model: Option<&ToneModel>) -> Self {
        let model_raw = model.map(|model| &model.raw);
        let model_field = |key: &str| model_raw.and_then(|raw| text_field(raw.get(key)));
        Self {
            gear: text_field(tone.raw.get("gear")).or_else(|| model_field("gear")),
            makes: make_names(tone.raw.get("makes")),
            title: tone.title.clone(),
            nam_make: model_field("gear_make"),
            nam_model: model_field("gear_model"),
            nam_gear_type: model_field("gear_type"),
            tone_type: model_field("tone_type"),
        }
    }
}

/// The identified gear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GearIdentity {
    /// Broad category.
    pub category: GearCategory,
    /// Family, which names the skin and icon.
    pub family: GearFamily,
    /// Tone character.
    pub tone_type: ToneType,
    /// Which hint decided the family.
    pub source: MatchSource,
}

/// Keyword table, checked in order. Each keyword is matched as whole words.
///
/// Cab layouts (8x10, 2x12, 1x12) come before brand hints so a "Greenback 2x12" stays a 2x12.
const KEYWORDS: &[(GearFamily, &[&str])] = &[
    // Pedals
    (GearFamily::OdGold, &["klon", "centaur", "ktr", "soul food"]),
    (
        GearFamily::OdGreen,
        &[
            "ts808",
            "ts 808",
            "ts9",
            "ts 9",
            "tube screamer",
            "tubescreamer",
            "od808",
            "od 808",
            "sd 1",
            "sd1",
            "maxon",
        ],
    ),
    (
        GearFamily::Fuzz,
        &[
            "big muff",
            "muff",
            "fuzz face",
            "tone bender",
            "fuzz factory",
            "fuzz",
        ],
    ),
    (
        GearFamily::Distortion,
        &[
            "rat",
            "rat2",
            "rat 2",
            "ds 1",
            "ds1",
            "rodent",
            "distortion",
        ],
    ),
    (
        GearFamily::Compressor,
        &["dyna comp", "dynacomp", "cali76", "compressor", "comp"],
    ),
    (
        GearFamily::Boost,
        &["ep booster", "booster", "boost", "sho", "treble booster"],
    ),
    // Amps: specific models before brand names
    (
        GearFamily::HeadHighGain,
        &["5150", "5153", "6505", "6534", "evh", "invective", "peavey"],
    ),
    (
        GearFamily::HeadModernUs,
        &[
            "rectifier",
            "recto",
            "dual rec",
            "triple rec",
            "mesa",
            "boogie",
        ],
    ),
    (
        GearFamily::HeadBass,
        &["svt", "b 15", "b15", "ampeg", "bass"],
    ),
    (
        GearFamily::ComboChime,
        &["ac30", "ac 30", "ac15", "ac 15", "top boost", "vox"],
    ),
    (
        GearFamily::ComboClean,
        &[
            "twin",
            "deluxe",
            "princeton",
            "blackface",
            "jc 120",
            "jc120",
            "jc 40",
            "jc40",
            "jc 22",
            "jazz chorus",
            "fender",
        ],
    ),
    (
        GearFamily::HeadBritish,
        &[
            "plexi",
            "jcm800",
            "jcm 800",
            "jcm900",
            "jcm",
            "2203",
            "1959",
            "1987",
            "jvm",
            "dsl",
            "jtm",
            "super lead",
            "marshall",
        ],
    ),
    // Cabs: layout first, then vintage British, then modern
    (
        GearFamily::Cab810,
        &["8x10", "810", "4x10", "410", "1x15", "115", "bass cab"],
    ),
    (GearFamily::Cab212, &["2x12", "212"]),
    (GearFamily::Cab112, &["1x12", "112"]),
    (
        GearFamily::Cab412Vintage,
        &[
            "greenback",
            "g12m",
            "g12h",
            "creamback",
            "1960",
            "1960a",
            "1960b",
            "1960ax",
            "1960bx",
            "1960tv",
            "marshall",
        ],
    ),
    (
        GearFamily::Cab412Modern,
        &[
            "v30",
            "4x12",
            "412",
            "oversized",
            "rectifier",
            "recto",
            "mesa",
            "5150",
            "6505",
            "evh",
        ],
    ),
    // Studio gear
    (
        GearFamily::Outboard,
        &[
            "1176",
            "la 2a",
            "la2a",
            "1073",
            "neve",
            "ssl",
            "api 2500",
            "distressor",
        ],
    ),
];

/// Identifies the gear behind a capture.
#[must_use]
pub fn identify(hints: &GearHints) -> GearIdentity {
    let declared = hints
        .gear
        .as_deref()
        .map(GearCategory::parse)
        .filter(|category| *category != GearCategory::Unknown)
        .or_else(|| {
            hints
                .nam_gear_type
                .as_deref()
                .map(GearCategory::parse)
                .filter(|category| *category != GearCategory::Unknown)
        })
        .unwrap_or(GearCategory::Unknown);
    let tone_type = hints
        .tone_type
        .as_deref()
        .map_or(ToneType::Unknown, ToneType::from_nam);

    let nam_text = [hints.nam_make.as_deref(), hints.nam_model.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let sources = [
        (MatchSource::NamMetadata, nam_text),
        (MatchSource::Catalog, hints.makes.join(" ")),
        (MatchSource::Title, hints.title.clone()),
    ];
    for (source, text) in sources {
        if let Some(family) = match_keywords(&text, declared) {
            let category = if declared == GearCategory::Unknown {
                family.category()
            } else {
                declared
            };
            return GearIdentity {
                category,
                family,
                tone_type,
                source,
            };
        }
    }

    let family = default_family(declared, tone_type);
    GearIdentity {
        category: declared,
        family,
        tone_type,
        source: if family == GearFamily::Unknown {
            MatchSource::None
        } else {
            MatchSource::Category
        },
    }
}

fn match_keywords(text: &str, declared: GearCategory) -> Option<GearFamily> {
    let haystack = words(text);
    if haystack.trim().is_empty() {
        return None;
    }
    KEYWORDS
        .iter()
        .filter(|(family, _)| declared == GearCategory::Unknown || family.category() == declared)
        .find(|(_, keywords)| {
            keywords
                .iter()
                .any(|keyword| haystack.contains(&words(keyword)))
        })
        .map(|(family, _)| *family)
}

const fn default_family(category: GearCategory, tone_type: ToneType) -> GearFamily {
    match category {
        GearCategory::Pedal => match tone_type {
            ToneType::Crunch | ToneType::HiGain | ToneType::Fuzz => GearFamily::PedalDefaultDark,
            ToneType::Clean | ToneType::Unknown => GearFamily::PedalDefaultLight,
        },
        GearCategory::Amp => match tone_type {
            ToneType::Clean => GearFamily::AmpDefaultClean,
            ToneType::Crunch => GearFamily::AmpDefaultCrunch,
            ToneType::HiGain | ToneType::Fuzz | ToneType::Unknown => GearFamily::AmpDefaultHiGain,
        },
        GearCategory::Cab => GearFamily::CabDefault,
        GearCategory::FullRig => GearFamily::FullRig,
        GearCategory::Outboard => GearFamily::Outboard,
        GearCategory::Space => GearFamily::Space,
        GearCategory::Unknown => GearFamily::Unknown,
    }
}

/// Lowercases, turns every non-alphanumeric run into one space, and pads with spaces so that
/// `contains(" kw ")` matches whole words only.
fn words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    let mut gap = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if gap && !out.ends_with(' ') {
                out.push(' ');
            }
            gap = false;
            out.extend(ch.to_lowercase());
        } else {
            gap = true;
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// Lowercases and joins words with underscores (`" Hi-Gain "` becomes `hi_gain`).
fn normalize_word(value: &str) -> String {
    words(value).trim().replace(' ', "_")
}

fn text_field(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn make_names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| match item {
            Value::String(_) => text_field(Some(item)),
            Value::Object(object) => text_field(object.get("name")),
            _ => None,
        })
        .collect()
}
