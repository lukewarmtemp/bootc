use anyhow::Ok;
use anyhow::Result;

use ostree::gio;
use ostree_ext::ostree;
use ostree_ext::ostree::Deployment;
use crate::deploy::ImageState;
use ostree_ext::prelude::FileExt;
use ostree_ext::prelude::Cast;
use ostree_ext::prelude::FileEnumeratorExt;

use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    kargs: Vec<String>,
    arch: Option<Vec<String>>,
}

pub(crate) fn get_kargs(repo: &ostree::Repo, booted_deployment: &Deployment, fetched: &ImageState) -> Result<Vec<String>> {
    let cancellable = gio::Cancellable::NONE;
    let mut kargs: Vec<String> = vec![];

    // Get the running kargs of the booted system
    match ostree::Deployment::bootconfig(booted_deployment) {
        Some(bootconfig) => {
            match ostree::BootconfigParser::get(&bootconfig, "options") {
                Some(options) => {
                    let options: Vec<&str> = options.split_whitespace().collect();
                    let mut options: Vec<String> = options.into_iter().map(|s| s.to_string()).collect();
                    kargs.append(&mut options);
                },
                None => ()
            }
        },
        None => ()
    };

    // Get the kargs in kargs.d of the booted system
    let mut existing_kargs: Vec<String> = vec![];
    let fragments = liboverdrop::scan(&["/usr/lib"], "bootc/kargs.d", &["toml"], true);
    for (_name, path) in fragments {
        let s = std::fs::read_to_string(&path)?;
        let mut parsed_kargs = parse_file(s.clone())?;
        existing_kargs.append(&mut parsed_kargs);
    }
    
    // Get the kargs in kargs.d of the remote image
    let mut remote_kargs: Vec<String> = vec![];
    let (fetched_tree, _) = repo.read_commit(fetched.ostree_commit.as_str(), cancellable)?;
    let fetched_tree = fetched_tree.resolve_relative_path("/usr/lib/bootc/kargs.d");
    let fetched_tree = fetched_tree.downcast::<ostree::RepoFile>().expect("downcast");
    match fetched_tree.query_exists(cancellable) {
        true => {}
        false => {
            return Ok(vec![]);
        }
    }
    let queryattrs = "standard::name,standard::type";
    let queryflags = gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS;
    let fetched_iter = fetched_tree.enumerate_children(queryattrs, queryflags, cancellable)?;
    while let Some(fetched_info) = fetched_iter.next_file(cancellable)? {
        let fetched_child = fetched_iter.child(&fetched_info);
        let fetched_child = fetched_child.downcast::<ostree::RepoFile>().expect("downcast");
        fetched_child.ensure_resolved()?;
        let fetched_contents_checksum = fetched_child.checksum();
        let f = ostree::Repo::load_file(repo, fetched_contents_checksum.as_str(), cancellable)?;
        let file_content = f.0;
        let mut reader = ostree_ext::prelude::InputStreamExtManual::into_read(file_content.unwrap());
        let s = std::io::read_to_string(&mut reader)?;
        let mut parsed_kargs = parse_file(s.clone())?;
        remote_kargs.append(&mut parsed_kargs);
    }

    // get the diff between the existing and remote kargs
    let mut added_kargs: Vec<String> = remote_kargs.clone().into_iter().filter(|item| !existing_kargs.contains(item)).collect();
    let removed_kargs: Vec<String> = existing_kargs.clone().into_iter().filter(|item| !remote_kargs.contains(item)).collect();

    // apply the diff to the system kargs
    kargs.retain(|x| !removed_kargs.contains(x));
    kargs.append(&mut added_kargs);

    Ok(kargs)
}

pub fn parse_file(file_content: String) -> Result<Vec<String>> {
    let mut de: Config = toml::from_str(&file_content)?;
    let mut parsed_kargs: Vec<String> = vec![];
    // if arch specified, apply kargs only if the arch matches
    // if arch not specified, apply kargs unconditionally
    match de.arch {
        None => parsed_kargs = de.kargs,
        Some(supported_arch) => {
            for arch in supported_arch.iter() {
                if arch == std::env::consts::ARCH {
                    parsed_kargs.append(&mut de.kargs);
                }
            }
        }
    }
    return Ok(parsed_kargs);
}

#[test]
/// Verify that kargs are only applied to supported architectures
fn test_arch() {
    // no arch specified, kargs ensure that kargs are applied unconditionally
    std::env::set_var("ARCH", "x86_64");
    let file_content = r##"kargs = ["console=tty0", "nosmt"]"##.to_string();
    let parsed_kargs = parse_file(file_content.clone()).unwrap();
    assert_eq!(
        parsed_kargs,
        ["console=tty0", "nosmt"]
    );
    std::env::set_var("ARCH", "aarch64");
    let parsed_kargs = parse_file(file_content.clone()).unwrap();
    assert_eq!(
        parsed_kargs,
        ["console=tty0", "nosmt"]
    );

    // one arch matches and one doesn't, ensure that kargs are only applied for the matching arch
    std::env::set_var("ARCH", "x86_64");
    let file_content =
        r##"kargs = ["console=tty0", "nosmt"]
arch = ["x86_64"]
"##.to_string();
    let parsed_kargs = parse_file(file_content.clone()).unwrap();
    assert_eq!(
        parsed_kargs,
        ["console=tty0", "nosmt"]
    );
    let file_content =
        r##"kargs = ["console=tty0", "nosmt"]
arch = ["aarch64"]
"##.to_string();
    let parsed_kargs = parse_file(file_content.clone()).unwrap();
    assert_eq!(
        parsed_kargs,
        [] as [String; 0]
    );

    // multiple arch specified, ensure that kargs are applied to both archs
    std::env::set_var("ARCH", "x86_64");
    let file_content =
        r##"kargs = ["console=tty0", "nosmt"]
arch = ["x86_64", "aarch64"]
"##.to_string();
    let parsed_kargs = parse_file(file_content.clone()).unwrap();
    assert_eq!(
        parsed_kargs,
        ["console=tty0", "nosmt"]
    );
    std::env::set_var("ARCH", "aarch64");
    let parsed_kargs = parse_file(file_content.clone()).unwrap();
    assert_eq!(
        parsed_kargs,
        ["console=tty0", "nosmt"]
    );
}
