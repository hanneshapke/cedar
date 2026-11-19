//! The looks a deck can have: a palette and a pair of fonts each. Every colour a slide uses comes
//! from here, so a deck stays consistent whatever its slides hold.

use anyhow::{bail, Result};
use serde_json::{json, Value};

/// An sRGB colour, `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const WHITE: Color = Color(0xFFFFFF);
    pub const INK: Color = Color(0x111827);

    /// `#2563EB`, `2563eb` or the short `#26e`.
    pub fn parse(s: &str) -> Result<Self> {
        let hex = s.trim().trim_start_matches('#');
        let full = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 => hex.to_string(),
            _ => bail!("{s:?} is not a colour; write it like #2563EB"),
        };
        match u32::from_str_radix(&full, 16) {
            Ok(v) if full.chars().all(|c| c.is_ascii_hexdigit()) => Ok(Color(v)),
            _ => bail!("{s:?} is not a colour; write it like #2563EB"),
        }
    }

    fn channels(self) -> [f64; 3] {
        [16, 8, 0].map(|shift| f64::from((self.0 >> shift) & 0xFF) / 255.0)
    }

    /// The Slides API's `rgbColor`.
    pub fn rgb(self) -> Value {
        let [r, g, b] = self.channels().map(|c| (c * 1000.0).round() / 1000.0);
        json!({ "red": r, "green": g, "blue": b })
    }

    /// WCAG relative luminance, 0 (black) to 1 (white).
    pub fn luminance(self) -> f64 {
        let [r, g, b] = self.channels().map(|c| {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// WCAG contrast ratio between two colours, 1 to 21.
    pub fn contrast(self, other: Color) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// White or near-black, whichever reads better on this colour.
    pub fn on(self) -> Color {
        if self.contrast(Color::WHITE) >= self.contrast(Color::INK) {
            Color::WHITE
        } else {
            Color::INK
        }
    }
}

/// One look. The "cover" colours are for the title, section and closing slides, which are set
/// apart from the content slides.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    /// Cards and zebra rows: a step away from `bg`.
    pub surface: Color,
    pub text: Color,
    pub muted: Color,
    /// Hairlines.
    pub rule: Color,
    pub accent: Color,
    pub cover_bg: Color,
    pub cover_text: Color,
    pub cover_muted: Color,
    pub cover_accent: Color,
    /// The cover slides are filled with the accent itself, so a custom accent recolours them.
    cover_is_accent: bool,
    pub heading_font: &'static str,
    /// Display faces that come in one weight are not set bold.
    pub heading_bold: bool,
    pub body_font: &'static str,
}

pub const NAMES: &[&str] = &["minimal", "midnight", "editorial", "forest", "bold"];
pub const DEFAULT: &str = "minimal";

impl Theme {
    pub fn named(name: &str) -> Result<Self> {
        let c = Color;
        Ok(match name.trim().to_ascii_lowercase().as_str() {
            "minimal" => Theme {
                name: "minimal",
                bg: c(0xFFFFFF),
                surface: c(0xF3F4F6),
                text: c(0x111827),
                muted: c(0x5B6472),
                rule: c(0xE5E7EB),
                accent: c(0x2563EB),
                cover_bg: c(0x0F172A),
                cover_text: c(0xFFFFFF),
                cover_muted: c(0xA3AEC2),
                cover_accent: c(0x60A5FA),
                cover_is_accent: false,
                heading_font: "Inter",
                heading_bold: true,
                body_font: "Inter",
            },
            "midnight" => Theme {
                name: "midnight",
                bg: c(0x0B1220),
                surface: c(0x162036),
                text: c(0xF8FAFC),
                muted: c(0x9AA8BF),
                rule: c(0x2A3650),
                accent: c(0x38BDF8),
                cover_bg: c(0x060B16),
                cover_text: c(0xFFFFFF),
                cover_muted: c(0x9AA8BF),
                cover_accent: c(0x38BDF8),
                cover_is_accent: false,
                heading_font: "Poppins",
                heading_bold: true,
                body_font: "Inter",
            },
            "editorial" => Theme {
                name: "editorial",
                bg: c(0xFAF7F2),
                surface: c(0xF0EAE0),
                text: c(0x1F1B16),
                muted: c(0x6B6258),
                rule: c(0xDED5C7),
                accent: c(0xA8481F),
                cover_bg: c(0x1F1B16),
                cover_text: c(0xFAF7F2),
                cover_muted: c(0xBDB3A5),
                cover_accent: c(0xE0895F),
                cover_is_accent: false,
                heading_font: "Playfair Display",
                heading_bold: true,
                body_font: "Lato",
            },
            "forest" => Theme {
                name: "forest",
                bg: c(0xF7F8F4),
                surface: c(0xE9EDE3),
                text: c(0x1C2B24),
                muted: c(0x56665D),
                rule: c(0xD5DCCD),
                accent: c(0x2F6B4F),
                cover_bg: c(0x173D2E),
                cover_text: c(0xF7F8F4),
                cover_muted: c(0xAAC4B6),
                cover_accent: c(0x8FCFAB),
                cover_is_accent: false,
                heading_font: "DM Serif Display",
                heading_bold: false,
                body_font: "DM Sans",
            },
            "bold" => Theme {
                name: "bold",
                bg: c(0xFFFFFF),
                surface: c(0xF4F1FE),
                text: c(0x18181B),
                muted: c(0x63636E),
                rule: c(0xE4E4E7),
                accent: c(0x6D28D9),
                cover_bg: c(0x6D28D9),
                cover_text: c(0xFFFFFF),
                cover_muted: c(0xDDD0FB),
                cover_accent: c(0xFACC15),
                cover_is_accent: true,
                heading_font: "Montserrat",
                heading_bold: true,
                body_font: "Open Sans",
            },
            other => bail!(
                "unknown theme {other:?}; the themes are {}",
                NAMES.join(", ")
            ),
        })
    }

    /// The theme with the deck's own accent colour (a brand colour) in place of its own. Where
    /// the colour would be too faint to read on a slide it is refused rather than used.
    pub fn with_accent(mut self, accent: Color) -> Result<Self> {
        if accent.contrast(self.bg) < 2.0 {
            bail!(
                "the accent colour is too close to the {} theme's background to be seen; pick a \
                 stronger one or another theme",
                self.name
            );
        }
        self.accent = accent;
        if self.cover_is_accent {
            self.cover_bg = accent;
            self.cover_text = accent.on();
            self.cover_muted = accent.on();
            self.cover_accent = accent.on();
        } else if accent.contrast(self.cover_bg) >= 3.0 {
            self.cover_accent = accent;
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_parsed_and_rendered() {
        assert_eq!(Color::parse("#2563EB").unwrap(), Color(0x2563EB));
        assert_eq!(Color::parse("2563eb").unwrap(), Color(0x2563EB));
        assert_eq!(Color::parse("#26e").unwrap(), Color(0x2266EE));
        assert!(Color::parse("blue").is_err());
        assert!(Color::parse("#12345").is_err());
        assert!(Color::parse("+12345").is_err());
        assert_eq!(
            Color(0xFF0080).rgb(),
            json!({ "red": 1.0, "green": 0.0, "blue": 0.502 })
        );
    }

    #[test]
    fn text_on_a_colour_is_the_readable_one() {
        assert_eq!(Color(0x0F172A).on(), Color::WHITE);
        assert_eq!(Color(0xFACC15).on(), Color::INK);
        assert!((Color::WHITE.contrast(Color(0x000000)) - 21.0).abs() < 0.01);
    }

    /// Every theme keeps body text at WCAG AA and its accents visible, on content and cover
    /// slides alike.
    #[test]
    fn every_theme_is_readable() {
        for name in NAMES {
            let t = Theme::named(name).unwrap();
            assert_eq!(t.name, *name);
            assert!(t.text.contrast(t.bg) >= 7.0, "{name} text");
            assert!(t.muted.contrast(t.bg) >= 4.5, "{name} muted");
            assert!(t.muted.contrast(t.surface) >= 4.5, "{name} muted on surface");
            assert!(t.accent.contrast(t.bg) >= 4.5, "{name} accent");
            assert!(t.accent.contrast(t.surface) >= 4.0, "{name} accent on surface");
            assert!(t.cover_text.contrast(t.cover_bg) >= 7.0, "{name} cover text");
            assert!(t.cover_muted.contrast(t.cover_bg) >= 4.5, "{name} cover muted");
            assert!(t.cover_accent.contrast(t.cover_bg) >= 4.5, "{name} cover accent");
        }
        assert!(Theme::named("neon").is_err());
        assert!(NAMES.contains(&DEFAULT));
    }

    #[test]
    fn a_custom_accent_recolours_the_deck() {
        let t = Theme::named("minimal").unwrap().with_accent(Color(0xE11D48)).unwrap();
        assert_eq!(t.accent, Color(0xE11D48));
        assert_eq!(t.cover_accent, Color(0xE11D48));
        // Too dark to show on the dark cover: the cover keeps the theme's own accent.
        let t = Theme::named("minimal").unwrap().with_accent(Color(0x1E293B)).unwrap();
        assert_eq!(t.cover_accent, Color(0x60A5FA));
        // A theme whose covers are the accent takes the new one there, with readable text.
        let t = Theme::named("bold").unwrap().with_accent(Color(0xFACC15)).unwrap_err();
        assert!(t.to_string().contains("too close"), "{t}");
        let t = Theme::named("bold").unwrap().with_accent(Color(0x0F766E)).unwrap();
        assert_eq!(t.cover_bg, Color(0x0F766E));
        assert_eq!(t.cover_text, Color::WHITE);
    }
}
