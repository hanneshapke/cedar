//! The QR code the iOS app scans to connect: `cedar://connect?server=<url>&token=<token>`, with
//! the chat's tailnet address and its access token. The iOS app parses the same link
//! (`ConnectionCode.swift`), so the two must agree on it. `scripts/cedar-stack.sh --pair` makes
//! the same link on Linux.

use anyhow::Result;
use qrcode::render::svg;
use qrcode::QrCode;
use url::Url;

/// The link the code carries.
pub fn link(server: &str, token: &str) -> String {
    let mut url = Url::parse("cedar://connect").expect("a valid base");
    url.query_pairs_mut()
        .append_pair("server", server)
        .append_pair("token", token);
    url.into()
}

/// The link as an SVG QR code, black on white whatever the page's theme, so cameras read it.
pub fn qr_svg(text: &str) -> Result<String> {
    let code = QrCode::new(text.as_bytes())?;
    let image = code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    // The page inserts it as markup, where an XML declaration has no place.
    Ok(match image.find("<svg") {
        Some(start) => image[start..].to_string(),
        None => image,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_carries_the_address_and_token_encoded() {
        let link = link("https://my-mac.tail1234.ts.net", "ab&c=d");
        let url = Url::parse(&link).unwrap();
        assert_eq!(url.scheme(), "cedar");
        assert_eq!(url.host_str(), Some("connect"));
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs,
            vec![
                ("server".into(), "https://my-mac.tail1234.ts.net".into()),
                ("token".into(), "ab&c=d".into())
            ]
        );
    }

    #[test]
    fn the_code_is_bare_svg() {
        let svg = qr_svg(&link("https://m.t.ts.net", &"a".repeat(48))).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("#000000"));
    }
}
