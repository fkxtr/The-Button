//! Restricted HTTP client for public archive metadata and file transfers.
use anyhow::{bail, Context, Result};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{header, redirect, Client, Response};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

const MAX_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Clone)]
pub struct SafeClient {
    client: Client,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Family {
    Archive,
    Wikimedia,
}

fn archive_storage_host(host: &str) -> bool {
    [".us.archive.org", ".eu.archive.org", ".archive.org"]
        .iter()
        .any(|suffix| {
            host.strip_suffix(suffix)
                .and_then(|label| label.strip_prefix("ia"))
                .is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                })
        })
}

fn family(url: &Url) -> Result<Family> {
    match url.host_str() {
        Some("archive.org") => Ok(Family::Archive),
        Some("commons.wikimedia.org" | "upload.wikimedia.org") => Ok(Family::Wikimedia),
        Some(host) if archive_storage_host(host) => Ok(Family::Archive),
        _ => bail!("The URL host is outside the supported public archives"),
    }
}

/// URLs are limited to the exact public hosts used by the adapters. IP literals,
/// credentials, fragments, unusual ports, and non-HTTPS schemes are rejected.
pub fn validate_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("Invalid download URL")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || url.fragment().is_some()
        || url
            .host_str()
            .is_some_and(|host| host.parse::<IpAddr>().is_ok())
    {
        bail!("Only public archive HTTPS URLs without credentials are accepted");
    }
    family(&url)?;
    Ok(url)
}

fn redirect_target(current: &Url, location: &str) -> Result<Url> {
    let next = validate_url(current.join(location)?.as_str())?;
    if family(current)? != family(&next)? {
        bail!("The archive redirected to a different content provider");
    }
    Ok(next)
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && a != 0
        && a < 224
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 192 && b == 0 && c == 0)
        && !(a == 192 && b == 88 && c == 99)
        && !(a == 198 && (b == 18 || b == 19))
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let segments = ip.segments();
            // Current global unicast allocation, excluding documentation and tunnels.
            segments[0] & 0xe000 == 0x2000
                && !(segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8))
                && segments[0] != 0x2002
                && !(segments[0] == 0x3fff && segments[1] < 0x1000)
        }
    }
}

#[derive(Debug)]
struct PublicResolver {
    proxy_hosts: HashSet<String>,
}

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let hostname = name.as_str().to_owned();
        // The managed environment's explicit proxy gateway may itself be private.
        // This exception is only for connecting to that gateway, never a URL target.
        let is_proxy = self.proxy_hosts.contains(&hostname);
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((hostname.as_str(), 0))
                .await?
                .collect();
            if addresses.is_empty()
                || (!is_proxy && addresses.iter().any(|address| !public_ip(address.ip())))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Archive DNS resolved to a non-public address",
                )
                .into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

impl SafeClient {
    pub fn new() -> Result<Self> {
        let proxy_hosts = [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .filter_map(|value| Url::parse(&value).ok())
        .filter_map(|url| url.host_str().map(str::to_owned))
        .collect();
        let client = Client::builder()
            .user_agent(concat!(
                "BOTON/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/fkxtr/The-Button)"
            ))
            .https_only(true)
            .redirect(redirect::Policy::none())
            .connect_timeout(Duration::from_secs(12))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(30 * 60))
            .dns_resolver(Arc::new(PublicResolver { proxy_hosts }))
            .build()?;
        Ok(Self { client })
    }

    /// Perform a GET with status checking and at most five validated redirects.
    /// The body is deliberately left streaming for the download engine.
    pub async fn get(&self, raw: &str) -> Result<Response> {
        let mut current = validate_url(raw)?;
        for hop in 0..=MAX_REDIRECTS {
            let response = self
                .client
                .get(current.clone())
                .header(header::ACCEPT_ENCODING, "identity")
                .send()
                .await
                .context("The public archive could not be reached")?;
            if response.status().is_redirection() {
                if hop == MAX_REDIRECTS
                    || !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308)
                {
                    bail!("The archive returned too many or unsupported redirects");
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .context("The archive redirect had no destination")?
                    .to_str()?;
                current = redirect_target(&current, location)?;
                continue;
            }
            let response = response
                .error_for_status()
                .context("The public archive rejected the request")?;
            // We never issue Range requests. An unsolicited partial response
            // must not become a complete download when size metadata is absent.
            if response.status() != reqwest::StatusCode::OK {
                bail!("The archive did not return a complete file response");
            }
            return Ok(response);
        }
        unreachable!("bounded redirect loop always returns")
    }

    pub async fn json(&self, url: &str) -> Result<serde_json::Value> {
        tokio::time::timeout(Duration::from_secs(40), async {
            let mut response = self.get(url).await?;
            if response
                .content_length()
                .is_some_and(|size| size > MAX_METADATA_BYTES as u64)
            {
                bail!("Archive metadata exceeds the size limit");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
                    bail!("Archive metadata exceeds the size limit");
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).context("The archive returned invalid JSON metadata")
        })
        .await
        .context("Archive metadata request timed out")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_archive_urls_only() {
        for good in [
            "https://archive.org/download/item/file.pdf",
            "https://ia801234.us.archive.org/0/items/a/b",
            "https://upload.wikimedia.org/wikipedia/commons/a/a0/photo.jpg",
        ] {
            assert!(validate_url(good).is_ok(), "{good}");
        }
        for bad in [
            "http://archive.org/a",
            "https://127.0.0.1/a",
            "https://[::1]/a",
            "https://archive.org:8080/a",
            "https://user@archive.org/a",
            "https://archive.org.attacker.test/a",
            "https://evil.archive.org/a",
            "https://ia123.us.archive.org.evil.test/a",
            "https://archive.org/a#secret",
            "file:///tmp/a",
        ] {
            assert!(validate_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn redirects_cannot_change_security_boundary() {
        let initial = validate_url("https://archive.org/download/item/file.pdf").unwrap();
        assert!(redirect_target(
            &initial,
            "https://ia801234.us.archive.org/0/items/item/file.pdf"
        )
        .is_ok());
        assert!(redirect_target(&initial, "https://upload.wikimedia.org/a").is_err());
        assert!(redirect_target(&initial, "//localhost/a").is_err());
        assert!(redirect_target(&initial, "http://archive.org/a").is_err());
    }

    #[test]
    fn special_networks_are_rejected() {
        for bad in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.19.255.254",
            "198.51.100.2",
            "203.0.113.2",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "0.0.0.0",
            "::",
            "::1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "2001:2::1",
            "2001:10::1",
            "2002::1",
            "3fff::1",
        ] {
            assert!(!public_ip(bad.parse().unwrap()), "{bad}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
