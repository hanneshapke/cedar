//! The QR code the iOS app scans to connect: `cedar://connect?server=<url>&token=<token>`, with
//! the chat's HTTPS address and its access token. The iOS app parses the same link
//! (`ConnectionCode.swift`), so the two must agree on it. `scripts/cedar-stack.sh --pair` makes
//! the same link in a terminal. The desktop app reads it too ([`parse`]), to use a chat server
//! that runs on another computer instead of starting its own.

use anyhow::{anyhow, bail, Result};
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

/// The server and token a link carries, the server as [`server_origin`] gives it. Takes what the
/// iOS app takes (`ConnectionCode.swift`), and also a plain `http://` address on this computer.
pub fn parse(link: &str) -> Result<(String, String)> {
    let url = Url::parse(link.trim()).map_err(|_| anyhow!("not a Cedar connection link"))?;
    if !url.scheme().eq_ignore_ascii_case("cedar") || url.host_str() != Some("connect") {
        bail!("not a Cedar connection link (it starts with cedar://connect)");
    }
    let value = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default()
    };
    let server = server_origin(&value("server"))?;
    let token = value("token");
    if token.is_empty() {
        bail!("the link has no access token");
    }
    Ok((server, token))
}

/// A chat server's address as the origin the window loads and compares pages against
/// (`https://host[:port]`, no path). HTTPS, as the phone apps require, since the access token
/// goes with every request; plain HTTP only to this computer itself.
pub fn server_origin(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("no server address");
    }
    let url = Url::parse(raw).map_err(|_| anyhow!("not a web address: {raw}"))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => bail!("no host in {raw}"),
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => bail!("{raw} is plain HTTP; use the server's https:// address (on a tailnet, the one `tailscale serve` gives it)"),
        _ => bail!("not a web address: {raw}"),
    }
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        bail!("use the server's address without a path: {}", url.origin().ascii_serialization());
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("the address can't carry a user name or password");
    }
    Ok(url.origin().ascii_serialization())
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
    fn a_link_reads_back() {
        let made = link("https://my-mac.tail1234.ts.net", "ab&c=d");
        assert_eq!(
            parse(&made).unwrap(),
            ("https://my-mac.tail1234.ts.net".into(), "ab&c=d".into())
        );
        assert_eq!(
            parse("  cedar://connect?token=t&server=https%3A%2F%2Fh.ts.net%3A8443%2F ").unwrap(),
            ("https://h.ts.net:8443".into(), "t".into())
        );
        for bad in [
            "https://h.ts.net",
            "cedar://other?server=https%3A%2F%2Fh&token=t",
            "cedar://connect?server=https%3A%2F%2Fh",
            "cedar://connect?server=http%3A%2F%2Fh.ts.net&token=t",
            "cedar://connect?token=t",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn server_addresses_are_origins() {
        assert_eq!(server_origin("https://H.ts.net/").unwrap(), "https://h.ts.net");
        assert_eq!(server_origin("https://h.ts.net:443").unwrap(), "https://h.ts.net");
        assert_eq!(server_origin("http://127.0.0.1:3000").unwrap(), "http://127.0.0.1:3000");
        assert_eq!(server_origin("http://localhost:3000/").unwrap(), "http://localhost:3000");
        assert_eq!(server_origin("http://[::1]:3000").unwrap(), "http://[::1]:3000");
        for bad in [
            "",
            "h.ts.net",
            "http://192.168.1.5:3000",
            "http://100.64.0.1",
            "https://h.ts.net/chat",
            "https://h.ts.net/?x=1",
            "https://u:p@h.ts.net",
            "ftp://h.ts.net",
            "file:///etc/passwd",
        ] {
            assert!(server_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_code_is_bare_svg() {
        let svg = qr_svg(&link("https://m.t.ts.net", &"a".repeat(48))).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("#000000"));
    }
}
