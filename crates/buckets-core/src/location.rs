use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Location {
    Local {
        path: PathBuf,
    },
    Plugin {
        plugin: String,
        path: String,
    },
    Ssh {
        host: String,
        path: String,
    },
    S3 {
        bucket: String,
        prefix: String,
        endpoint: Option<String>,
    },
    Git {
        repo: PathBuf,
        reference: String,
        path: String,
    },
}
impl Location {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local { path: path.into() }
    }
    pub fn parse(input: &str) -> Result<Self> {
        ensure!(!input.is_empty(), "Enter a path or location URL");
        if input.starts_with("plugin://") {
            let u = Url::parse(input)?;
            let plugin = u
                .host_str()
                .context("Plugin location requires a plugin id")?
                .to_owned();
            ensure!(
                plugin
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                "Invalid plugin id"
            );
            return Ok(Self::Plugin {
                plugin,
                path: decode_path(u.path())?,
            });
        }
        if input.starts_with("ssh://") {
            let u = Url::parse(input)?;
            ensure!(
                u.password().is_none(),
                "Use SSH keys or your SSH agent, not a password in the URL"
            );
            ensure!(
                u.port().is_none(),
                "Configure custom SSH ports in ~/.ssh/config"
            );
            let hostname = u.host_str().context("SSH location requires a host")?;
            let host = if u.username().is_empty() {
                hostname.to_owned()
            } else {
                format!("{}@{}", u.username(), hostname)
            };
            validate_host(&host)?;
            return Ok(Self::Ssh {
                host,
                path: decode_path(u.path())?,
            });
        }
        if input.starts_with("s3://") {
            let u = Url::parse(input)?;
            let bucket = u
                .host_str()
                .context("S3 location requires a bucket")?
                .to_owned();
            ensure!(
                !bucket.starts_with('-')
                    && bucket
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)),
                "Invalid bucket name"
            );
            let endpoint = u
                .query_pairs()
                .find(|(k, _)| k == "endpoint")
                .map(|(_, v)| v.into_owned());
            if let Some(e) = &endpoint {
                let e = Url::parse(e)?;
                ensure!(
                    matches!(e.scheme(), "http" | "https"),
                    "Endpoint must use http or https"
                );
            }
            return Ok(Self::S3 {
                bucket,
                prefix: decode_path(u.path())?.trim_start_matches('/').to_string(),
                endpoint,
            });
        }
        if input.starts_with("git://") {
            let u = Url::parse(input)?;
            ensure!(
                u.host_str().is_none(),
                "Use git:///absolute/repository?ref=HEAD&path=subdirectory"
            );
            let repo = PathBuf::from(decode_path(u.path())?);
            ensure!(repo.is_absolute(), "Git repository path must be absolute");
            let reference = u
                .query_pairs()
                .find(|(k, _)| k == "ref")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_else(|| "HEAD".into());
            ensure!(
                !reference.starts_with('-')
                    && !reference.contains(':')
                    && !reference.contains('\0'),
                "Invalid Git reference"
            );
            let path = u
                .query_pairs()
                .find(|(k, _)| k == "path")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            validate_relative(&path)?;
            return Ok(Self::Git {
                repo,
                reference,
                path,
            });
        }
        if input.starts_with("file://") {
            return Ok(Self::local(
                Url::parse(input)?
                    .to_file_path()
                    .map_err(|_| anyhow::anyhow!("Invalid local file URL"))?,
            ));
        }
        ensure!(!input.contains("://"), "Unsupported location scheme");
        let path = if input == "~" {
            home()?
        } else if let Some(p) = input.strip_prefix("~/") {
            home()?.join(p)
        } else {
            PathBuf::from(input)
        };
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };
        Ok(Self::local(path))
    }
    pub fn display(&self) -> String {
        match self {
            Self::Local { path } => path.to_string_lossy().into_owned(),
            Self::Plugin { plugin, path } => format!("plugin://{plugin}{}", encode_path(path)),
            Self::Ssh { host, path } => format!("ssh://{host}{}", encode_path(path)),
            Self::S3 {
                bucket,
                prefix,
                endpoint,
            } => {
                let mut s = format!("s3://{bucket}/{}", encode_path(prefix));
                if let Some(e) = endpoint {
                    s.push_str("?endpoint=");
                    s.extend(url::form_urlencoded::byte_serialize(e.as_bytes()));
                }
                s
            }
            Self::Git {
                repo,
                reference,
                path,
            } => {
                let mut u = Url::parse("git:///").expect("constant URL");
                u.set_path(&encode_path(&repo.to_string_lossy()));
                u.query_pairs_mut()
                    .append_pair("ref", reference)
                    .append_pair("path", path);
                u.to_string()
            }
        }
    }
    pub fn parent(&self) -> Option<Self> {
        match self {
            Self::Local { path } => path.parent().map(Self::local),
            Self::Plugin { plugin, path } => {
                if path == "/" {
                    None
                } else {
                    Some(Self::Plugin {
                        plugin: plugin.clone(),
                        path: remote_parent(path, true),
                    })
                }
            }
            Self::Ssh { host, path } => {
                if path == "/" {
                    None
                } else {
                    Some(Self::Ssh {
                        host: host.clone(),
                        path: remote_parent(path, true),
                    })
                }
            }
            Self::S3 {
                bucket,
                prefix,
                endpoint,
            } => {
                if prefix.is_empty() {
                    None
                } else {
                    Some(Self::S3 {
                        bucket: bucket.clone(),
                        prefix: {
                            let parent = remote_parent(prefix, false);
                            if parent.is_empty() {
                                parent
                            } else {
                                format!("{parent}/")
                            }
                        },
                        endpoint: endpoint.clone(),
                    })
                }
            }
            Self::Git {
                repo,
                reference,
                path,
            } => {
                if path.is_empty() {
                    None
                } else {
                    Some(Self::Git {
                        repo: repo.clone(),
                        reference: reference.clone(),
                        path: remote_parent(path, false),
                    })
                }
            }
        }
    }
    pub fn join(&self, name: &str) -> Result<Self> {
        validate_name(name)?;
        Ok(match self {
            Self::Local { path } => Self::local(path.join(name)),
            Self::Plugin { plugin, path } => Self::Plugin {
                plugin: plugin.clone(),
                path: format!("{}/{}", path.trim_end_matches('/'), name),
            },
            Self::Ssh { host, path } => Self::Ssh {
                host: host.clone(),
                path: format!("{}/{}", path.trim_end_matches('/'), name),
            },
            Self::S3 {
                bucket,
                prefix,
                endpoint,
            } => Self::S3 {
                bucket: bucket.clone(),
                prefix: join_remote(prefix, name),
                endpoint: endpoint.clone(),
            },
            Self::Git {
                repo,
                reference,
                path,
            } => Self::Git {
                repo: repo.clone(),
                reference: reference.clone(),
                path: join_remote(path, name),
            },
        })
    }
    pub fn name(&self) -> String {
        match self {
            Self::Local { path } => path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            Self::Plugin { path, .. } | Self::Ssh { path, .. } => {
                path.rsplit('/').next().unwrap_or(path).into()
            }
            Self::S3 { prefix, bucket, .. } => prefix
                .rsplit('/')
                .find(|p| !p.is_empty())
                .unwrap_or(bucket)
                .into(),
            Self::Git { path, repo, .. } => {
                if path.is_empty() {
                    repo.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    path.rsplit('/').next().unwrap_or(path).into()
                }
            }
        }
    }
    pub fn local_path(&self) -> Option<&Path> {
        if let Self::Local { path } = self {
            Some(path)
        } else {
            None
        }
    }
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::Plugin { .. } => "plugin",
            Self::Ssh { .. } => "ssh",
            Self::S3 { .. } => "s3",
            Self::Git { .. } => "git",
        }
    }
}
impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}
pub(crate) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains('/')
            && !name.contains('\0'),
        "Name must be a single nonempty path component"
    );
    Ok(())
}
pub(crate) fn validate_host(host: &str) -> Result<()> {
    ensure!(
        !host.starts_with('-')
            && !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-_@[]:".contains(c)),
        "Invalid SSH host"
    );
    Ok(())
}
fn validate_relative(path: &str) -> Result<()> {
    ensure!(
        !path.starts_with('/') && !path.split('/').any(|p| p == "..") && !path.contains('\0'),
        "Path must stay inside the Git tree"
    );
    Ok(())
}
fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("Home directory unavailable")
}
fn remote_parent(path: &str, absolute: bool) -> String {
    let p = path
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(p, _)| p)
        .unwrap_or("");
    if p.is_empty() && absolute {
        "/".into()
    } else {
        p.into()
    }
}
fn join_remote(p: &str, n: &str) -> String {
    if p.is_empty() {
        n.into()
    } else {
        format!("{}/{n}", p.trim_end_matches('/'))
    }
}
fn decode_path(path: &str) -> Result<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            ensure!(i + 2 < bytes.len(), "Invalid URL escape");
            let h = std::str::from_utf8(&bytes[i + 1..i + 3])?;
            out.push(u8::from_str_radix(h, 16)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out)?;
    if s.contains('\0') {
        bail!("Paths cannot contain NUL")
    };
    Ok(if s.is_empty() { "/".into() } else { s })
}
fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            encoded.push(byte as char)
        } else {
            use std::fmt::Write;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}
