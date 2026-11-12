//! Using a chat server on another computer (see `config::Remote`). The app then starts neither
//! server: an always-on machine runs cedar, cedar-chat and the scheduled agents, and this app is a
//! window onto it, with its notifications, like the phone apps. Switching restarts the app, since
//! the window hands the chat page its token from a script fixed when the window is built.

use crate::app::AppState;
use crate::config::Remote;
use crate::i18n::{t, tf};
use anyhow::{anyhow, bail, Result};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// What the setup page sent: the `cedar://connect` link, or the address and token typed in.
pub fn from_input(link: &str, server: &str, token: &str) -> Result<Remote> {
    if !link.trim().is_empty() {
        let (server, token) = cedar_pairing::connect::parse(link)?;
        return Ok(Remote { server, token });
    }
    if server.trim().is_empty() || token.trim().is_empty() {
        bail!(t("remote.need_input"));
    }
    Ok(Remote {
        server: cedar_pairing::connect::server_origin(server)?,
        token: token.trim().to_string(),
    })
}

/// Asks the chat for something only its access token may read, so a wrong address or token is
/// caught before the app switches to it.
pub async fn check(remote: &Remote) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let server = remote.server.as_str();
    let resp = client
        .get(format!("{server}/api/notifications"))
        .bearer_auth(&remote.token)
        .send()
        .await
        .map_err(|e| anyhow!("{}: {e}", tf("remote.unreachable", &[("server", server)])))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!(tf("remote.bad_token", &[("server", server)]));
    }
    if !status.is_success() {
        bail!(tf(
            "remote.answered",
            &[("server", server), ("status", status.as_str())]
        ));
    }
    Ok(())
}

/// Switches to `remote` (`None`: back to the servers on this computer) and restarts into it. The
/// servers this app started and the tailnet sharing are stopped first, as for an update.
pub async fn switch(app: AppHandle, remote: Option<Remote>) -> Result<()> {
    let state = app.state::<AppState>();
    if let Some(r) = &remote {
        check(r).await?;
    }
    state.update_config(|c| c.remote = remote)?;
    state.supervisor.stop().await;
    state.stop_share().await;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_or_an_address_and_token() {
        let link = cedar_pairing::connect::link("https://spark.t.ts.net", "tok");
        assert_eq!(
            from_input(&link, "ignored", "").unwrap(),
            Remote { server: "https://spark.t.ts.net".into(), token: "tok".into() }
        );
        assert_eq!(
            from_input(" ", "https://spark.t.ts.net/", " tok ").unwrap(),
            Remote { server: "https://spark.t.ts.net".into(), token: "tok".into() }
        );
        assert!(from_input("", "https://spark.t.ts.net", "").is_err());
        assert!(from_input("", "", "tok").is_err());
        assert!(from_input("", "http://spark.local:3000", "tok").is_err());
    }

    /// A server on loopback that answers every request with `status`, and the address it is at.
    async fn answering(status: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let reply = format!("HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}");
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn the_check_says_what_is_wrong() {
        let remote = |server: String| Remote { server, token: "tok".into() };
        assert!(check(&remote(answering("200 OK").await)).await.is_ok());
        let e = check(&remote(answering("401 Unauthorized").await)).await.unwrap_err();
        assert!(e.to_string().contains("didn't accept the access token"), "{e}");
        let e = check(&remote(answering("404 Not Found").await)).await.unwrap_err();
        assert!(e.to_string().contains("answered 404"), "{e}");
        // Nothing listens there once the listener is dropped.
        let gone = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let e = check(&remote(gone)).await.unwrap_err();
        assert!(e.to_string().contains("can't reach"), "{e}");
    }
}
