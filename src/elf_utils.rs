// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Utilities for parsing ELF binaries and resolving their dynamic library dependencies.

use elf::abi::{DT_NEEDED, DT_RPATH, DT_RUNPATH, PT_INTERP};
use elf::endian::AnyEndian;
use elf::ElfBytes;
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ElfError {
    #[error("Failed to read file: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to parse ELF: {0}")]
    Parse(#[from] elf::ParseError),

    #[error("Not an ELF file: {0}")]
    NotElf(PathBuf),

    #[error("Could not resolve library '{0}' required by '{1}'")]
    LibraryNotFound(String, PathBuf),
}

/// Information about an executable and its required libraries.
#[derive(Debug)]
pub struct ExecutableInfo {
    /// Absolute path to the executable itself.
    pub executable_path: PathBuf,
    /// Absolute paths to all required shared libraries.
    pub library_paths: BTreeSet<PathBuf>,
    /// Path to the dynamic linker/interpreter (e.g., /lib64/ld-linux-x86-64.so.2).
    pub interpreter: Option<PathBuf>,
}

/// Resolve an executable and all its dynamic library dependencies.
pub fn resolve_executable(path: &Path) -> Result<ExecutableInfo, ElfError> {
    resolve_executable_inner(
        path,
        &mut BTreeSet::new(),
        &mut Vec::new(),
    )
}

/// Inner function that tracks already-resolved interpreters and accumulated search paths.
fn resolve_executable_inner(
    path: &Path,
    known_interpreters: &mut BTreeSet<String>,
    inherited_search_paths: &mut Vec<PathBuf>,
) -> Result<ExecutableInfo, ElfError> {
    let executable_path = path.canonicalize()?;

    let file_data = std::fs::read(&executable_path)?;

    let elf = ElfBytes::<AnyEndian>::minimal_parse(&file_data)
        .map_err(|_| ElfError::NotElf(executable_path.clone()))?;

    let mut library_paths = BTreeSet::new();
    let mut search_paths = inherited_search_paths.clone();

    // Add the directory containing this binary/library to search paths
    if let Some(binary_dir) = executable_path.parent() {
        if !search_paths.contains(&binary_dir.to_path_buf()) {
            search_paths.push(binary_dir.to_path_buf());
        }
    }

    // Get the interpreter (dynamic linker) path from PT_INTERP
    let interpreter = get_interpreter(&elf, &file_data)?;
    if let Some(ref interp) = interpreter {
        if interp.exists() {
            library_paths.insert(interp.clone());
            // Add the interpreter's directory to search paths
            if let Some(interp_dir) = interp.parent() {
                if !search_paths.contains(&interp_dir.to_path_buf()) {
                    search_paths.push(interp_dir.to_path_buf());
                }
            }
            // Track the interpreter's filename so we can skip it in DT_NEEDED
            if let Some(interp_name) = interp.file_name() {
                known_interpreters.insert(interp_name.to_string_lossy().into_owned());
            }
        }
    }

    // Extract RPATH and RUNPATH from dynamic section
    if let Some((rpath, runpath)) = get_rpath_runpath(&elf)? {
        // RUNPATH takes precedence over RPATH in modern linkers
        if let Some(runpath) = runpath {
            for path in runpath.split(':') {
                let expanded = expand_origin(path, &executable_path);
                let expanded_path = PathBuf::from(expanded);
                if !search_paths.contains(&expanded_path) {
                    search_paths.push(expanded_path);
                }
            }
        }
        if let Some(rpath) = rpath {
            for path in rpath.split(':') {
                let expanded = expand_origin(path, &executable_path);
                let expanded_path = PathBuf::from(expanded);
                if !search_paths.contains(&expanded_path) {
                    search_paths.push(expanded_path);
                }
            }
        }
    }

    // Get the list of needed libraries
    let needed_libs = get_needed_libraries(&elf)?;

    // Resolve each library
    for lib_name in needed_libs {
        // Skip the dynamic linker if we've already found it via PT_INTERP
        if known_interpreters.contains(&lib_name) {
            continue;
        }

        if let Some(lib_path) = find_library(&lib_name, &search_paths) {
            // Recursively resolve the library's dependencies, passing accumulated search paths
            let lib_info = resolve_executable_inner(&lib_path, known_interpreters, &mut search_paths)?;
            library_paths.insert(lib_path);
            library_paths.extend(lib_info.library_paths);
        } else {
            return Err(ElfError::LibraryNotFound(
                lib_name,
                executable_path.clone(),
            ));
        }
    }

    Ok(ExecutableInfo {
        executable_path,
        library_paths,
        interpreter,
    })
}

/// Get the interpreter path from PT_INTERP program header.
fn get_interpreter(elf: &ElfBytes<AnyEndian>, file_data: &[u8]) -> Result<Option<PathBuf>, ElfError> {
    let segments = elf.segments().ok_or_else(|| {
        ElfError::Parse(elf::ParseError::BadMagic([0, 0, 0, 0]))
    })?;

    for segment in segments {
        if segment.p_type == PT_INTERP {
            let start = segment.p_offset as usize;
            let end = start + segment.p_filesz as usize;
            if end <= file_data.len() {
                let interp_bytes = &file_data[start..end];
                // Remove null terminator
                let interp_str = std::str::from_utf8(interp_bytes)
                    .ok()
                    .map(|s| s.trim_end_matches('\0'))
                    .unwrap_or("");
                if !interp_str.is_empty() {
                    return Ok(Some(PathBuf::from(interp_str)));
                }
            }
        }
    }
    Ok(None)
}

/// Get RPATH and RUNPATH from the dynamic section.
fn get_rpath_runpath(elf: &ElfBytes<AnyEndian>) -> Result<Option<(Option<String>, Option<String>)>, ElfError> {
    let common = elf.find_common_data()?;
    
    let (dynamic, strtab) = match (common.dynamic, common.dynsyms_strs) {
        (Some(d), Some(s)) => (d, s),
        _ => return Ok(None),
    };

    let mut rpath = None;
    let mut runpath = None;

    for dyn_entry in dynamic {
        match dyn_entry.d_tag {
            DT_RPATH => {
                if let Ok(s) = strtab.get(dyn_entry.d_val() as usize) {
                    rpath = Some(s.to_string());
                }
            }
            DT_RUNPATH => {
                if let Ok(s) = strtab.get(dyn_entry.d_val() as usize) {
                    runpath = Some(s.to_string());
                }
            }
            _ => {}
        }
    }

    Ok(Some((rpath, runpath)))
}

/// Get the list of DT_NEEDED library names.
fn get_needed_libraries(elf: &ElfBytes<AnyEndian>) -> Result<Vec<String>, ElfError> {
    let common = elf.find_common_data()?;
    
    let (dynamic, strtab) = match (common.dynamic, common.dynsyms_strs) {
        (Some(d), Some(s)) => (d, s),
        _ => return Ok(Vec::new()),
    };

    let mut needed = Vec::new();

    for dyn_entry in dynamic {
        if dyn_entry.d_tag == DT_NEEDED {
            if let Ok(name) = strtab.get(dyn_entry.d_val() as usize) {
                needed.push(name.to_string());
            }
        }
    }

    Ok(needed)
}

/// Expand $ORIGIN in a path to the directory containing the executable.
fn expand_origin(path: &str, executable: &Path) -> String {
    if let Some(parent) = executable.parent() {
        path.replace("$ORIGIN", &parent.to_string_lossy())
            .replace("${ORIGIN}", &parent.to_string_lossy())
    } else {
        path.to_string()
    }
}

/// Find a library by name in the given search paths.
fn find_library(name: &str, search_paths: &[PathBuf]) -> Option<PathBuf> {
    // If the name is an absolute path, use it directly
    if name.starts_with('/') {
        let path = PathBuf::from(name);
        if path.exists() {
            return path.canonicalize().ok();
        }
        return None;
    }

    // Search in the provided paths
    for search_path in search_paths {
        let candidate = search_path.join(name);
        if candidate.exists() {
            return candidate.canonicalize().ok();
        }
    }

    None
}

/// Base TOML template for executable rules.
const EXECUTABLE_BASE_TOML: &str = include_str!("../assets/landlock/executable-base.toml");

/// Generate a TOML configuration for a single executable's access rules.
///
/// Returns the TOML content as a string.
pub fn generate_executable_toml(exec_info: &ExecutableInfo) -> String {
    // Parse the base template
    let mut doc: toml::Table = toml::from_str(EXECUTABLE_BASE_TOML)
        .expect("Failed to parse executable-base.toml template");

    // Collect all paths (executable + libraries + interpreter)
    let mut all_paths: BTreeSet<&Path> = BTreeSet::new();
    all_paths.insert(&exec_info.executable_path);
    for lib_path in &exec_info.library_paths {
        all_paths.insert(lib_path);
    }
    if let Some(ref interp) = exec_info.interpreter {
        all_paths.insert(interp);
    }

    // Convert paths to TOML array
    let paths_array: Vec<toml::Value> = all_paths
        .iter()
        .map(|p| toml::Value::String(p.to_string_lossy().into_owned()))
        .collect();

    // Find the [[path_beneath]] array and update the parent field
    if let Some(toml::Value::Array(path_beneath_array)) = doc.get_mut("path_beneath") {
        if let Some(toml::Value::Table(first_entry)) = path_beneath_array.first_mut() {
            first_entry.insert("parent".to_string(), toml::Value::Array(paths_array));
        }
    }

    // Serialize back to TOML string
    toml::to_string_pretty(&doc).expect("Failed to serialize TOML")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_origin() {
        let executable = PathBuf::from("/usr/bin/myapp");
        assert_eq!(
            expand_origin("$ORIGIN/../lib", &executable),
            "/usr/bin/../lib"
        );
        assert_eq!(
            expand_origin("${ORIGIN}/plugins", &executable),
            "/usr/bin/plugins"
        );
        assert_eq!(expand_origin("/absolute/path", &executable), "/absolute/path");
    }
}
