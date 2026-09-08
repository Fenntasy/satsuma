//! The custom "Grouping" tag: `Type / Volume / Vibe`.
//!
//! Format rules:
//! - `A / B / C` when all three parts are set
//! - `A / / C` when the volume is empty
//! - `A /` when both volume and vibe are empty
//! - each part may also be empty, so `/ Loud / Happy` or `/ / Dark` are valid

use std::fmt;

use serde::{Deserialize, Serialize};

/// A value that is not one of the allowed words for its part.
struct Unknown;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Chant,
    Instru,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Volume {
    Soft,
    Loud,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vibe {
    Happy,
    Sad,
    Dark,
    Mix,
}

/// A parsed grouping tag. Every part is optional.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grouping {
    pub kind: Option<Kind>,
    pub volume: Option<Volume>,
    pub vibe: Option<Vibe>,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Chant => "Chant",
            Kind::Instru => "Instru",
        }
    }

    fn parse(raw: &str) -> Result<Option<Self>, Unknown> {
        match raw {
            "" => Ok(None),
            "Chant" => Ok(Some(Kind::Chant)),
            "Instru" => Ok(Some(Kind::Instru)),
            _ => Err(Unknown),
        }
    }
}

impl Volume {
    fn label(self) -> &'static str {
        match self {
            Volume::Soft => "Soft",
            Volume::Loud => "Loud",
        }
    }

    fn parse(raw: &str) -> Result<Option<Self>, Unknown> {
        match raw {
            "" => Ok(None),
            "Soft" => Ok(Some(Volume::Soft)),
            "Loud" => Ok(Some(Volume::Loud)),
            _ => Err(Unknown),
        }
    }
}

impl Vibe {
    fn label(self) -> &'static str {
        match self {
            Vibe::Happy => "Happy",
            Vibe::Sad => "Sad",
            Vibe::Dark => "Dark",
            Vibe::Mix => "Mix",
        }
    }

    fn parse(raw: &str) -> Result<Option<Self>, Unknown> {
        match raw {
            "" => Ok(None),
            "Happy" => Ok(Some(Vibe::Happy)),
            "Sad" => Ok(Some(Vibe::Sad)),
            "Dark" => Ok(Some(Vibe::Dark)),
            "Mix" => Ok(Some(Vibe::Mix)),
            _ => Err(Unknown),
        }
    }
}

impl Grouping {
    /// Parses a grouping tag. Returns `None` when the text does not follow
    /// the `A / B / C` format or uses unknown values. Surrounding whitespace
    /// around each part is ignored; a blank string is the empty grouping.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let parts: Vec<&str> = raw.split('/').map(str::trim).collect();
        if parts.len() > 3 {
            return None;
        }
        let part = |index: usize| parts.get(index).copied().unwrap_or("");
        Some(Grouping {
            kind: Kind::parse(part(0)).ok()?,
            volume: Volume::parse(part(1)).ok()?,
            vibe: Vibe::parse(part(2)).ok()?,
        })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kind.is_none() && self.volume.is_none() && self.vibe.is_none()
    }
}

impl fmt::Display for Grouping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = self.kind.map_or("", Kind::label);
        let volume = self.volume.map_or("", Volume::label);
        let vibe = self.vibe.map_or("", Vibe::label);
        match (self.volume, self.vibe) {
            (None, None) => write!(f, "{kind} /"),
            (None, Some(_)) => write!(f, "{kind} / / {vibe}"),
            (Some(_), None) => write!(f, "{kind} / {volume} /"),
            (Some(_), Some(_)) => write!(f, "{kind} / {volume} / {vibe}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Grouping, Kind, Vibe, Volume};

    fn grouping(kind: Option<Kind>, volume: Option<Volume>, vibe: Option<Vibe>) -> Grouping {
        Grouping { kind, volume, vibe }
    }

    #[test]
    fn parses_full_grouping() {
        assert_eq!(
            Grouping::parse("Chant / Loud / Happy"),
            Some(grouping(
                Some(Kind::Chant),
                Some(Volume::Loud),
                Some(Vibe::Happy)
            ))
        );
    }

    #[test]
    fn parses_missing_volume() {
        assert_eq!(
            Grouping::parse("Instru / / Dark"),
            Some(grouping(Some(Kind::Instru), None, Some(Vibe::Dark)))
        );
    }

    #[test]
    fn parses_type_only() {
        assert_eq!(
            Grouping::parse("Chant /"),
            Some(grouping(Some(Kind::Chant), None, None))
        );
        assert_eq!(
            Grouping::parse("Chant"),
            Some(grouping(Some(Kind::Chant), None, None))
        );
    }

    #[test]
    fn parses_blank_as_empty() {
        assert_eq!(Grouping::parse(""), Some(Grouping::default()));
        assert_eq!(Grouping::parse("  /  /  "), Some(Grouping::default()));
        assert!(Grouping::parse("").is_some_and(|g| g.is_empty()));
    }

    #[test]
    fn rejects_unknown_values_and_extra_parts() {
        assert_eq!(Grouping::parse("Vocal / Loud / Happy"), None);
        assert_eq!(Grouping::parse("Chant / Medium / Happy"), None);
        assert_eq!(Grouping::parse("Chant / Loud / Angry"), None);
        assert_eq!(Grouping::parse("Chant / Loud / Happy / Extra"), None);
    }

    #[test]
    fn formats_following_the_rules() {
        let cases = [
            (
                grouping(Some(Kind::Chant), Some(Volume::Soft), Some(Vibe::Sad)),
                "Chant / Soft / Sad",
            ),
            (
                grouping(Some(Kind::Instru), None, Some(Vibe::Mix)),
                "Instru / / Mix",
            ),
            (grouping(Some(Kind::Chant), None, None), "Chant /"),
            (
                grouping(Some(Kind::Chant), Some(Volume::Loud), None),
                "Chant / Loud /",
            ),
            (
                grouping(None, Some(Volume::Loud), Some(Vibe::Happy)),
                " / Loud / Happy",
            ),
            (grouping(None, None, None), " /"),
        ];
        for (value, expected) in cases {
            assert_eq!(value.to_string(), expected);
        }
    }

    #[test]
    fn display_round_trips_through_parse() {
        let kinds = [None, Some(Kind::Chant), Some(Kind::Instru)];
        let volumes = [None, Some(Volume::Soft), Some(Volume::Loud)];
        let vibes = [
            None,
            Some(Vibe::Happy),
            Some(Vibe::Sad),
            Some(Vibe::Dark),
            Some(Vibe::Mix),
        ];
        for kind in kinds {
            for volume in volumes {
                for vibe in vibes {
                    let value = grouping(kind, volume, vibe);
                    assert_eq!(Grouping::parse(&value.to_string()), Some(value));
                }
            }
        }
    }
}
