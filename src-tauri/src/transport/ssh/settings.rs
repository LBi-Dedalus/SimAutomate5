//! The user's SSH setup: `~/.ssh/config`, private key resolution and host key checks.

use std::path::{Path, PathBuf};

use russh::keys::known_hosts::check_known_hosts_path;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};

use crate::models::User;

pub(super) const DEFAULT_KEY_FILES: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

/// Where the user's SSH files live; injectable so that the tests never touch the real home.
#[derive(Debug, Clone)]
pub(super) struct SshPaths {
    pub(super) home: PathBuf,
}

impl SshPaths {
    pub(super) fn from_env() -> Result<Self, String> {
        ["USERPROFILE", "HOME"]
            .iter()
            .filter_map(std::env::var_os)
            .find(|value| !value.is_empty())
            .map(|home| Self { home: home.into() })
            .ok_or_else(|| "Cannot find the home directory (USERPROFILE / HOME)".to_string())
    }

    fn ssh_dir(&self) -> PathBuf {
        self.home.join(".ssh")
    }

    pub(super) fn config(&self) -> PathBuf {
        self.ssh_dir().join("config")
    }

    pub(super) fn known_hosts(&self) -> PathBuf {
        self.ssh_dir().join("known_hosts")
    }
}

/// What the matching `Host` blocks of `~/.ssh/config` say about the SSH host.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct HostSettings {
    pub(super) hostname: Option<String>,
    pub(super) user: Option<String>,
    pub(super) identity_files: Vec<PathBuf>,
}

/// The settings of `alias`, plus a warning when the file exists but cannot be used (the
/// connection then goes on with the defaults). A missing file is not a problem.
pub(super) fn load_host_settings(
    config_path: &Path,
    alias: &str,
) -> (HostSettings, Option<String>) {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (HostSettings::default(), None)
        }
        Err(err) => {
            let warning = format!("Cannot read {}: {err}; ignoring it", config_path.display());
            return (HostSettings::default(), Some(warning));
        }
    };
    match russh_config::parse(&contents, alias) {
        Ok(config) => {
            let host_config = config.host_config;
            let settings = HostSettings {
                hostname: host_config.hostname,
                user: host_config.user,
                identity_files: host_config.identity_file.unwrap_or_default(),
            };
            (settings, None)
        }
        Err(err) => {
            let warning = format!("Cannot parse {}: {err}; ignoring it", config_path.display());
            (HostSettings::default(), Some(warning))
        }
    }
}

/// Expands a leading `~` or `%USERPROFILE%` (the SSH config crate only handles `~/`).
pub(super) fn expand_path(path: &Path, home: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    let rest = if raw == "~" {
        Some("")
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        Some(rest)
    } else {
        let prefix = "%USERPROFILE%";
        match raw.get(..prefix.len()) {
            Some(head) if head.eq_ignore_ascii_case(prefix) => {
                Some(raw[prefix.len()..].trim_start_matches(['/', '\\']))
            }
            _ => None,
        }
    };
    match rest {
        Some("") => home.to_path_buf(),
        Some(rest) => home.join(rest),
        None => path.to_path_buf(),
    }
}

/// The first existing private key: the `IdentityFile`s of the SSH config, then the default
/// files. The error names every file that was tried.
pub(super) fn resolve_key(settings: &HostSettings, paths: &SshPaths) -> Result<PathBuf, String> {
    let candidates: Vec<PathBuf> = settings
        .identity_files
        .iter()
        .map(|file| expand_path(file, &paths.home))
        .chain(
            DEFAULT_KEY_FILES
                .iter()
                .map(|name| paths.ssh_dir().join(name)),
        )
        .collect();
    if let Some(found) = candidates.iter().find(|candidate| candidate.is_file()) {
        return Ok(found.clone());
    }
    let tried: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect();
    Err(format!(
        "No private key found for the SSH connection; tried: {}",
        tried.join(", ")
    ))
}

/// The explicit user, else the `User` of the SSH config, else the local user name.
pub(super) fn resolve_user(
    explicit: Option<&User>,
    settings: &HostSettings,
    local_user: Option<String>,
) -> Result<String, String> {
    explicit
        .map(|user| user.as_str().to_string())
        .or_else(|| settings.user.clone().filter(|user| !user.trim().is_empty()))
        .or_else(|| local_user.filter(|user| !user.trim().is_empty()))
        .ok_or_else(|| {
            "Cannot determine the SSH user: fill the SSH user field, or set User in ~/.ssh/config"
                .to_string()
        })
}

pub(super) fn local_user_from_env() -> Option<String> {
    ["USERNAME", "USER"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
}

pub(super) fn load_key(path: &Path) -> Result<PrivateKey, String> {
    russh::keys::load_secret_key(path, None).map_err(|err| match err {
        russh::keys::Error::KeyIsEncrypted => format!(
            "The private key {} requires a passphrase, passphrases are not supported",
            path.display()
        ),
        err => format!("Cannot load the private key {}: {err}", path.display()),
    })
}

/// `ssh` command that makes the user trust the host key.
fn trust_hint(alias: &str, port: u16) -> String {
    if port == 22 {
        format!("ssh {alias}")
    } else {
        format!("ssh -p {port} {alias}")
    }
}

/// Accepts the key only if the user's known_hosts already lists it for this host, using russh's
/// own (simpler than OpenSSH's) check; every other outcome is an error. `host` is the name
/// actually connected to, `alias` what the user typed (used in the hint).
pub(super) fn verify_host_key(
    known_hosts: &Path,
    host: &str,
    alias: &str,
    port: u16,
    key: &PublicKey,
) -> Result<(), String> {
    match check_known_hosts_path(host, port, key, known_hosts) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "The host key of {host}:{port} is not in {}. Run `{}` once to check and trust it, then retry",
            known_hosts.display(),
            trust_hint(alias, port)
        )),
        Err(russh::keys::Error::KeyChanged { line }) => Err(format!(
            "WARNING: the host key of {host}:{port} does not match line {line} of {}. \
             This can be a man-in-the-middle attack, or the server was reinstalled. \
             If you trust it, remove that line and run `{}` once",
            known_hosts.display(),
            trust_hint(alias, port)
        )),
        Err(err) => Err(format!(
            "Cannot check the host key of {host}:{port} in {}: {err}",
            known_hosts.display()
        )),
    }
}

/// Host key algorithms, those already recorded in known_hosts first (like OpenSSH), so that the
/// server presents a key that can be checked.
pub(super) fn preferred_host_key_algorithms(
    recorded: &[Algorithm],
    defaults: &[Algorithm],
) -> Vec<Algorithm> {
    let mut preferred: Vec<Algorithm> = Vec::new();
    let mut push = |algorithm: Algorithm| {
        if !preferred.contains(&algorithm) {
            preferred.push(algorithm);
        }
    };
    for algorithm in recorded {
        match algorithm {
            // The recorded RSA key is valid for every signature hash.
            Algorithm::Rsa { .. } => {
                push(Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512),
                });
                push(Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256),
                });
                push(Algorithm::Rsa { hash: None });
            }
            other => push(other.clone()),
        }
    }
    for algorithm in defaults {
        push(algorithm.clone());
    }
    preferred
}
