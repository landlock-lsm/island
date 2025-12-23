// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::{IslandError, Verbose};
use lddtree::{DependencyAnalyzer, DependencyTree};
use std::path::{Path, PathBuf};

fn lddtree_collect_extra_library_paths(tree: &DependencyTree) -> Vec<PathBuf> {
    let mut paths = std::collections::BTreeSet::<PathBuf>::new();

    // Binary-level RPATH/RUNPATH.
    for p in tree.runpath.iter().chain(tree.rpath.iter()) {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            paths.insert(pb);
        }
    }

    for lib in tree.libraries.values() {
        // Library-level RPATH/RUNPATH.
        for p in lib.runpath.iter().chain(lib.rpath.iter()) {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                paths.insert(pb);
            }
        }

        // Also add the directory of resolved libraries.
        if let Some(realpath) = lib.realpath.as_ref() {
            if let Some(parent) = realpath.parent() {
                paths.insert(parent.to_path_buf());
            }
        } else if lib.path.is_absolute() {
            if let Some(parent) = lib.path.parent() {
                paths.insert(parent.to_path_buf());
            }
        }
    }

    paths.into_iter().collect()
}

fn resolve_dependency_tree(path: &Path) -> Result<DependencyTree, IslandError> {
    // lddtree (crate) searches using the binary's RUNPATH/RPATH, but some ecosystems
    // (notably Nix) rely heavily on library-level RUNPATH to find second-order deps.
    // Work around this by iteratively re-analyzing with additional search paths
    // derived from already-resolved libraries.
    const MAX_PASSES: usize = 3;

    /* Root is set to / to resolve dependencies globally. */
    let mut tree = DependencyAnalyzer::new("/".into()).analyze(path)?;
    let mut extra_paths: Vec<PathBuf> = Vec::new();

    for _ in 0..MAX_PASSES {
        if !tree.libraries.values().any(|lib| {
            lib.realpath.is_none() && !lib.path.is_absolute() && !lib.path.as_os_str().is_empty()
        }) {
            break;
        }

        let newly_discovered = lddtree_collect_extra_library_paths(&tree);
        let mut combined = std::collections::BTreeSet::<PathBuf>::new();
        combined.extend(extra_paths.into_iter());
        combined.extend(newly_discovered.into_iter());
        extra_paths = combined.into_iter().collect();

        let next_tree = DependencyAnalyzer::new("/".into())
            .library_paths(extra_paths.clone())
            .analyze(path)?;
        // Stop early if we didn't make progress.
        if next_tree
            .libraries
            .values()
            .filter(|lib| lib.realpath.is_some())
            .count()
            <= tree
                .libraries
                .values()
                .filter(|lib| lib.realpath.is_some())
                .count()
        {
            tree = next_tree;
            break;
        }
        tree = next_tree;
    }

    Ok(tree)
}

pub fn resolve_command_dependency_paths(
    command_path: PathBuf,
    disable: bool,
    verbose: &Verbose,
) -> Result<Vec<PathBuf>, IslandError> {
    if disable {
        verbose.print(|| {
            "Skipping ELF dependency resolution (no_dependency includes \"elf\"); no automatic allow rules will be added".to_string()
        });
        return Ok(Vec::new());
    }

    let mut lddtree_paths: Vec<PathBuf> = vec![command_path.clone()];

    let dep_tree = resolve_dependency_tree(&command_path)?;
    for (library_name, library_object) in &dep_tree.libraries {
        // Use realpath if available (canonical resolved path), otherwise fall back to path.
        // When a library isn't found, lddtree sets realpath to None and path to just the
        // library name (not a full path).
        if let Some(realpath) = &library_object.realpath {
            lddtree_paths.push(realpath.clone());
        } else if library_object.path.is_absolute() {
            lddtree_paths.push(library_object.path.clone());
        } else {
            eprintln!(
                "Warning: could not resolve library path for {}: {}",
                library_name,
                library_object.path.display()
            );
        }
    }

    Ok(lddtree_paths)
}
