//! # Configuration for `bootc install`
//!
//! This module handles the TOML configuration file for `bootc install`.

use anyhow::{Context, Result};
use fn_error_context::context;
use serde::{Deserialize, Serialize};

use super::baseline::BlockSetup;

/// The toplevel config entry for installation configs stored
/// in bootc/install (e.g. /etc/bootc/install/05-custom.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallConfigurationToplevel {
    pub(crate) install: Option<InstallConfiguration>,
}

/// Configuration for a filesystem
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RootFS {
    #[serde(rename = "type")]
    pub(crate) fstype: Option<super::baseline::Filesystem>,
}

/// This structure should only define "system" or "basic" filesystems; we are
/// not trying to generalize this into e.g. supporting `/var` or other ones.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct BasicFilesystems {
    pub(crate) root: Option<RootFS>,
    // TODO allow configuration of these other filesystems too
    // pub(crate) xbootldr: Option<FilesystemCustomization>,
    // pub(crate) esp: Option<FilesystemCustomization>,
}

/// The serialized [install] section
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename = "install", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct InstallConfiguration {
    /// Root filesystem type
    pub(crate) root_fs_type: Option<super::baseline::Filesystem>,
    /// Enabled block storage configurations
    pub(crate) block: Option<Vec<BlockSetup>>,
    pub(crate) filesystem: Option<BasicFilesystems>,
    /// Kernel arguments, applied at installation time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kargs: Option<Vec<String>>,
    pub(crate) arch: Option<Vec<String>>,
}

fn merge_basic<T>(s: &mut Option<T>, o: Option<T>) {
    if let Some(o) = o {
        *s = Some(o);
    }
}

trait Mergeable {
    fn merge(&mut self, other: Self)
    where
        Self: Sized;
}

impl<T> Mergeable for Option<T>
where
    T: Mergeable,
{
    fn merge(&mut self, other: Self)
    where
        Self: Sized,
    {
        if let Some(other) = other {
            if let Some(s) = self.as_mut() {
                s.merge(other)
            } else {
                *self = Some(other);
            }
        }
    }
}

impl Mergeable for RootFS {
    /// Apply any values in other, overriding any existing values in `self`.
    fn merge(&mut self, other: Self) {
        merge_basic(&mut self.fstype, other.fstype)
    }
}

impl Mergeable for BasicFilesystems {
    /// Apply any values in other, overriding any existing values in `self`.
    fn merge(&mut self, other: Self) {
        self.root.merge(other.root)
    }
}

impl Mergeable for InstallConfiguration {
    /// Apply any values in other, overriding any existing values in `self`.
    fn merge(&mut self, other: Self) {
        merge_basic(&mut self.root_fs_type, other.root_fs_type);
        merge_basic(&mut self.block, other.block);
        self.filesystem.merge(other.filesystem);
        if let Some(other_kargs) = other.kargs {
            // if arch is specified, only apply kargs if it matches the current arch
            // if arch is not specified, apply kargs unconditionally
            if let Some(other_arch) = other.arch {
                for arch in other_arch.iter() {
                    if arch == std::env::consts::ARCH {
                        self.kargs
                            .get_or_insert_with(Default::default)
                            .extend(other_kargs.clone())
                    }
                }
            } else {
                self.kargs
                    .get_or_insert_with(Default::default)
                    .extend(other_kargs)
            }
        }
    }
}

impl InstallConfiguration {
    /// Set defaults (e.g. `block`), and also handle fields that can be specified multiple ways
    /// by synchronizing the values of the fields to ensure they're the same.
    ///
    /// - install.root-fs-type is synchronized with install.filesystems.root.type; if
    ///   both are set, then the latter takes precedence
    pub(crate) fn canonicalize(&mut self) {
        // New canonical form wins.
        if let Some(rootfs_type) = self.filesystem_root().and_then(|f| f.fstype.as_ref()) {
            self.root_fs_type = Some(*rootfs_type)
        } else if let Some(rootfs) = self.root_fs_type.as_ref() {
            let fs = self.filesystem.get_or_insert_with(Default::default);
            let root = fs.root.get_or_insert_with(Default::default);
            root.fstype = Some(*rootfs);
        }

        if self.block.is_none() {
            self.block = Some(vec![BlockSetup::Direct]);
        }
    }

    /// Convenience helper to access the root filesystem
    pub(crate) fn filesystem_root(&self) -> Option<&RootFS> {
        self.filesystem.as_ref().and_then(|fs| fs.root.as_ref())
    }

    // Remove all configuration which is handled by `install to-filesystem`.
    pub(crate) fn filter_to_external(&mut self) {
        self.kargs.take();
    }

    pub(crate) fn get_block_setup(&self, default: Option<BlockSetup>) -> Result<BlockSetup> {
        let valid_block_setups = self.block.as_deref().unwrap_or_default();
        let default_block = valid_block_setups.iter().next().ok_or_else(|| {
            anyhow::anyhow!("Empty block storage configuration in install configuration")
        })?;
        let block_setup = default.as_ref().unwrap_or(default_block);
        if !valid_block_setups.contains(block_setup) {
            anyhow::bail!("Block setup {block_setup:?} is not enabled in installation config");
        }
        Ok(*block_setup)
    }
}

#[context("Loading configuration")]
/// Load the install configuration, merging all found configuration files.
pub(crate) fn load_config() -> Result<Option<InstallConfiguration>> {
    const SYSTEMD_CONVENTIONAL_BASES: &[&str] = &["/usr/lib", "/usr/local/lib", "/etc", "/run"];
    let fragments = liboverdrop::scan(SYSTEMD_CONVENTIONAL_BASES, "bootc/install", &["toml"], true);
    let mut config: Option<InstallConfiguration> = None;
    for (_name, path) in fragments {
        let buf = std::fs::read_to_string(&path)?;
        let mut unused = std::collections::HashSet::new();
        let de = toml::Deserializer::new(&buf);
        let c: InstallConfigurationToplevel = serde_ignored::deserialize(de, |path| {
            unused.insert(path.to_string());
        })
        .with_context(|| format!("Parsing {path:?}"))?;
        for key in unused {
            eprintln!("warning: {path:?}: Unknown key {key}");
        }
        if let Some(config) = config.as_mut() {
            if let Some(install) = c.install {
                tracing::debug!("Merging install config: {install:?}");
                config.merge(install);
            }
        } else {
            config = c.install;
        }
    }
    if let Some(config) = config.as_mut() {
        config.canonicalize();
    }
    Ok(config)
}

#[test]
/// Verify that we can parse our default config file
fn test_parse_config() {
    use super::baseline::Filesystem;

    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
root-fs-type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    assert_eq!(install.root_fs_type.unwrap(), Filesystem::Xfs);
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            root_fs_type: Some(Filesystem::Ext4),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    assert_eq!(
        install.root_fs_type.as_ref().copied().unwrap(),
        Filesystem::Ext4
    );
    // This one shouldn't have been set
    assert!(install.filesystem_root().is_none());
    install.canonicalize();
    assert_eq!(install.root_fs_type.as_ref().unwrap(), &Filesystem::Ext4);
    assert_eq!(
        install.filesystem_root().unwrap().fstype.unwrap(),
        Filesystem::Ext4
    );

    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
root-fs-type = "ext4"
kargs = ["console=ttyS0", "foo=bar"]
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    assert_eq!(install.root_fs_type.unwrap(), Filesystem::Ext4);
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            kargs: Some(
                ["console=tty0", "nosmt"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    assert_eq!(install.root_fs_type.unwrap(), Filesystem::Ext4);
    assert_eq!(
        install.kargs,
        Some(
            ["console=ttyS0", "foo=bar", "console=tty0", "nosmt"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect()
        )
    )
}

#[test]
fn test_parse_filesystems() {
    use super::baseline::Filesystem;
    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install.filesystem.root]
type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    assert_eq!(
        install.filesystem_root().unwrap().fstype.unwrap(),
        Filesystem::Xfs
    );
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            filesystem: Some(BasicFilesystems {
                root: Some(RootFS {
                    fstype: Some(Filesystem::Ext4),
                }),
            }),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    assert_eq!(
        install.filesystem_root().unwrap().fstype.unwrap(),
        Filesystem::Ext4
    );
}

#[test]
fn test_parse_block() {
    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install.filesystem.root]
type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    // Verify the default (but note canonicalization mutates)
    {
        let mut install = install.clone();
        install.canonicalize();
        assert_eq!(install.get_block_setup(None).unwrap(), BlockSetup::Direct);
    }
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            block: Some(vec![]),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    // Should be set, but zero length
    assert_eq!(install.block.as_ref().unwrap().len(), 0);
    assert!(install.get_block_setup(None).is_err());

    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
block = ["tpm2-luks"]"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    install.canonicalize();
    assert_eq!(install.block.as_ref().unwrap().len(), 1);
    assert_eq!(install.get_block_setup(None).unwrap(), BlockSetup::Tpm2Luks);

    // And verify passing a disallowed config is an error
    assert!(install.get_block_setup(Some(BlockSetup::Direct)).is_err());
}

#[test]
/// Verify that kargs are only applied to supported architectures
fn test_arch() {
    // no arch specified, kargs ensure that kargs are applied unconditionally
    std::env::set_var("ARCH", "x86_64");
    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
root-fs-type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            kargs: Some(
                ["console=tty0", "nosmt"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    assert_eq!(
        install.kargs,
        Some(
            ["console=tty0", "nosmt"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect()
        )
    );
    std::env::set_var("ARCH", "aarch64");
    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
root-fs-type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            kargs: Some(
                ["console=tty0", "nosmt"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    assert_eq!(
        install.kargs,
        Some(
            ["console=tty0", "nosmt"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect()
        )
    );

    // one arch matches and one doesn't, ensure that kargs are only applied for the matching arch
    std::env::set_var("ARCH", "x86_64");
    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
root-fs-type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            kargs: Some(
                ["console=ttyS0", "foo=bar"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            arch: Some(
                ["x86_64"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    assert_eq!(
        install.kargs,
        Some(
            ["console=ttyS0", "foo=bar"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect()
        )
    );
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            kargs: Some(
                ["console=tty0", "nosmt"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            arch: Some(
                ["aarch64"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            ..Default::default()
        }),
    };
    install.merge(other.install.unwrap());
    assert_eq!(
        install.kargs,
        Some(
            ["console=ttyS0", "foo=bar"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect()
        )
    );

    // multiple arch specified, ensure that kargs are applied to both archs
    std::env::set_var("ARCH", "x86_64");
    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
root-fs-type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            kargs: Some(
                ["console=tty0", "nosmt"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            arch: Some(
                ["x86_64", "aarch64"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            ..Default::default()
        }),
    };
    std::env::set_var("ARCH", "x86_64");
    install.merge(other.install.unwrap());
    assert_eq!(
        install.kargs,
        Some(
            ["console=tty0", "nosmt"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect()
        )
    );
    std::env::set_var("ARCH", "aarch64");
    let c: InstallConfigurationToplevel = toml::from_str(
        r##"[install]
root-fs-type = "xfs"
"##,
    )
    .unwrap();
    let mut install = c.install.unwrap();
    let other = InstallConfigurationToplevel {
        install: Some(InstallConfiguration {
            kargs: Some(
                ["console=tty0", "nosmt"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            arch: Some(
                ["x86_64", "aarch64"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            ..Default::default()
        }),
    };
    std::env::set_var("ARCH", "x86_64");
    install.merge(other.install.unwrap());
    assert_eq!(
        install.kargs,
        Some(
            ["console=tty0", "nosmt"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect()
        )
    );
}
