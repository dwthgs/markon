//! Guards: web fetches never reach this Mac or its network, and secret files are never read.
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

const METADATA_HOSTS: &[&str] = &[
    "metadata.google.internal",
    "metadata.google.com",
    "metadata",
    "instance-data",
];

fn blocked_v4(a: Ipv4Addr) -> bool {
    let [x, y, ..] = a.octets();
    a.is_unspecified()
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_multicast()
        || a.is_broadcast()
        || a.is_documentation()
        || x == 0
        || (x == 100 && (64..128).contains(&y)) // carrier-grade NAT, also Alibaba's metadata address
        || (x == 198 && (y == 18 || y == 19)) // benchmarking
        || x >= 240
}

/// Loopback, private, link-local, multicast and other addresses a web page must never reach.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => blocked_v4(a),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return blocked_v4(v4);
            }
            let s = a.segments();
            a.is_unspecified()
                || a.is_loopback()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] == 0x64 && s[1] == 0xff9b) // NAT64 wraps an IPv4 address
                || a == Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)
        }
    }
}

/// Why this URL's host may not be fetched, if it may not. Literal IP hosts never reach the resolver,
/// so they are checked here.
pub fn blocked_url(url: &reqwest::Url) -> Option<String> {
    match url.host() {
        None => Some("the link has no host".into()),
        Some(url::Host::Ipv4(a)) if is_blocked_ip(a.into()) => {
            Some(format!("{a} is a private address"))
        }
        Some(url::Host::Ipv6(a)) if is_blocked_ip(a.into()) => {
            Some(format!("{a} is a private address"))
        }
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            (d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || METADATA_HOSTS.contains(&d.as_str()))
            .then(|| format!("{d} is on this Mac or its network"))
        }
        _ => None,
    }
}

/// System DNS, with every private answer dropped. Applies to redirects and DNS rebinding too,
/// because every connection the client opens resolves through it.
pub struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::task::spawn_blocking(move || {
                (host.as_str(), 0)
                    .to_socket_addrs()
                    .map(|a| a.collect::<Vec<_>>())
            })
            .await??;
            let public: Vec<SocketAddr> = addrs
                .into_iter()
                .filter(|a| !is_blocked_ip(a.ip()))
                .collect();
            if public.is_empty() {
                return Err(
                    "that site resolves to a private address, which Markon does not fetch".into(),
                );
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

pub fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        let scheme_ok = matches!(attempt.url().scheme(), "http" | "https");
        match blocked_url(attempt.url()) {
            Some(why) => attempt.error(why),
            None if !scheme_ok => attempt.error("redirect to a non-web link"),
            None => attempt.follow(),
        }
    })
}

/// File names that hold secrets (including Mac keychains).
fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let exact = [
        ".env",
        ".secret",
        "credentials",
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        ".htpasswd",
        ".pgpass",
        ".netrc",
        ".git-credentials",
        "shadow",
        "gshadow",
        "pass.txt",
        "password.txt",
        "passwords.txt",
        ".npmrc",
        ".pypirc",
        "known_hosts",
        "authorized_keys",
    ];
    let ext = [
        ".pem",
        ".key",
        ".p12",
        ".pfx",
        ".jks",
        ".keychain",
        ".keychain-db",
        ".env",
        ".secrets",
        ".kdbx",
    ];
    exact.contains(&n.as_str())
        || n.starts_with(".env.")
        || n.starts_with("credentials.")
        || ext.iter().any(|e| n.ends_with(e))
}

/// Folders whose whole content is secret.
const SECRET_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    "Keychains",
    ".password-store",
];

fn secret_path(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| secret_name(&n.to_string_lossy()))
        || p.components()
            .any(|c| SECRET_DIRS.iter().any(|d| c.as_os_str() == *d))
}

/// True if `path`, any symlink on the way to its target, or the resolved target is a secret file.
pub fn is_secret_file(path: &Path) -> bool {
    let mut candidates: Vec<PathBuf> = vec![path.to_path_buf()];
    let mut cur = path.to_path_buf();
    for _ in 0..40 {
        let Ok(t) = std::fs::read_link(&cur) else {
            break;
        };
        cur = if t.is_absolute() {
            t
        } else {
            cur.parent().unwrap_or(Path::new("/")).join(t)
        };
        candidates.push(cur.clone());
    }
    if let Ok(c) = path.canonicalize() {
        candidates.push(c);
    }
    candidates.iter().any(|c| secret_path(c))
}

/// Words that suggest the model is being asked to enter a secret into a field (a web page, or
/// any app), or a 13-or-more-digit number anywhere.
pub fn looks_secret(field_line: &str, text: &str) -> bool {
    let f = field_line.to_lowercase();
    [
        "password",
        "passcode",
        "card number",
        "cvc",
        "cvv",
        "security code",
        "one-time",
        "otp",
    ]
    .iter()
    .any(|w| f.contains(w))
        || (text.len() >= 13 && text.chars().filter(char::is_ascii_digit).count() >= 13)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses_are_blocked() {
        for a in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.10",
            "169.254.169.254",
            "100.100.100.200",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.1",
        ] {
            assert!(is_blocked_ip(a.parse().unwrap()), "{a}");
        }
        for a in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(!is_blocked_ip(a.parse().unwrap()), "{a}");
        }
    }

    #[test]
    fn local_hosts_are_blocked_in_urls() {
        let b = |u: &str| blocked_url(&reqwest::Url::parse(u).unwrap()).is_some();
        assert!(b("http://localhost:8080/"));
        assert!(b("http://127.0.0.1/"));
        assert!(b("http://[::1]/"));
        assert!(b("http://169.254.169.254/latest/meta-data"));
        assert!(b("http://metadata.google.internal/"));
        assert!(b("http://printer.local/"));
        assert!(b("http://2130706433/")); // decimal 127.0.0.1, normalised by the URL parser
        assert!(!b("https://example.com/"));
    }

    #[test]
    fn secret_files_are_recognised() {
        for p in [
            "/u/p/.env",
            "/u/p/.env.local",
            "/u/.ssh/config",
            "/u/.ssh/id_ed25519",
            "/u/p/server.pem",
            "/u/p/tls.key",
            "/u/.aws/credentials",
            "/u/.netrc",
            "/u/Library/Keychains/login.keychain-db",
            "/u/p/cert.p12",
            "/u/p/id_rsa",
        ] {
            assert!(is_secret_file(Path::new(p)), "{p}");
        }
        for p in [
            "/u/p/notes.md",
            "/u/p/env.rs",
            "/u/p/keyboard.txt",
            "/u/p/README",
        ] {
            assert!(!is_secret_file(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn a_symlink_cannot_hide_a_secret() {
        let d = std::env::temp_dir().join(format!("markon-guard-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".env"), "KEY=1").unwrap();
        let link = d.join("innocent.txt");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(d.join(".env"), &link).unwrap();
        assert!(is_secret_file(&link));
        let _ = std::fs::remove_dir_all(d);
    }
}
