//! Frozen ordinary files and separately staged sensitive definitions.
//! Caller supplies immutable object projections and a quiescent read-only PVC.
use anyhow::{Context, Result, ensure};
use kimage_core::{
    build::{Build, DEFINITION_LIMIT, Definition},
    oci,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
const BYTE_LIMIT: u64 = 512 * 1024 * 1024;
const FILE_LIMIT: usize = 8192;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub ordinary_digest: String,
    pub definition_sensitive: bool,
    /// Private receipt only. Never write this field to logs/status/provenance.
    pub definition_digest: String,
    pub files: usize,
    pub bytes: u64,
}
fn physical(root: &Path, logical: &str) -> Result<PathBuf> {
    kimage_core::build::absolute(logical)?;
    Ok(root.join(
        logical
            .strip_prefix('/')
            .context("InputPathMustBeAbsolute")?,
    ))
}
fn logical(root: &Path, path: &Path) -> Result<String> {
    Ok(format!(
        "/{}",
        path.strip_prefix(root)?
            .to_str()
            .context("NonUtf8InputPath")?
    ))
}
fn resolve(build: &Build, root: &Path, path: &str) -> Result<PathBuf> {
    let (mount, volume) = build.inputs.mount_for(path)?;
    let base = physical(root, &mount.mount_path)?
        .canonicalize()
        .context("MissingInputMount")?;
    let candidate = physical(root, path)?;
    if volume.persistent_volume_claim.is_some() {
        let mut cursor = physical(root, &mount.mount_path)?;
        if path != mount.mount_path {
            for part in path
                .strip_prefix(&(mount.mount_path.clone() + "/"))
                .context("InputPathOutsideDeclaredMounts")?
                .split('/')
            {
                cursor.push(part);
                ensure!(
                    !fs::symlink_metadata(&cursor)?.file_type().is_symlink(),
                    "SourceSymlinkDenied"
                );
            }
        }
    }
    let resolved = candidate.canonicalize().context("MissingInputFile")?;
    ensure!(resolved.starts_with(base), "SourceSymlinkEscape");
    Ok(resolved)
}
fn declared_mode(build: &Build, path: &str, actual: u32) -> Result<u32> {
    let (mount, volume) = build.inputs.mount_for(path)?;
    let Some(object) = volume.config_map.as_ref().or(volume.secret.as_ref()) else {
        return Ok(actual & 0o777);
    };
    let suffix = path
        .strip_prefix(&mount.mount_path)
        .context("InputPathOutsideDeclaredMounts")?
        .trim_start_matches('/');
    let selected = match &mount.sub_path {
        Some(sub) if suffix.is_empty() => sub.clone(),
        Some(sub) => format!("{sub}/{suffix}"),
        None => suffix.into(),
    };
    let item_mode = if let Some(items) = &object.items {
        items
            .iter()
            .find(|item| item.path == selected)
            .context("UnexpectedProjectedInputFile")?
            .mode
    } else {
        None
    };
    Ok(item_mode.or(object.default_mode).unwrap_or(0o644))
}
struct Copy<'a> {
    build: &'a Build,
    root: &'a Path,
    context: &'a str,
    hasher: Sha256,
    files: usize,
    bytes: u64,
    members: usize,
}
impl Copy<'_> {
    fn header(&mut self, kind: u8, path: &str, mode: u32, size: u64) {
        self.hasher.update([kind]);
        self.hasher.update((path.len() as u64).to_be_bytes());
        self.hasher.update(path.as_bytes());
        self.hasher.update(mode.to_be_bytes());
        self.hasher.update(size.to_be_bytes());
    }
    fn walk(&mut self, source: &Path, destination: &Path, depth: usize) -> Result<()> {
        self.members += 1;
        ensure!(
            self.members <= FILE_LIMIT && depth <= 64,
            "TooManyInputFilesOrDepth"
        );
        let name = logical(self.root, source)?;
        let mapping = self.build.inputs.mount_for(&name).ok();
        let metadata = fs::symlink_metadata(source)?;
        let projected = mapping
            .as_ref()
            .is_some_and(|(_, v)| v.config_map.is_some() || v.secret.is_some());
        let resolved = if metadata.file_type().is_symlink() {
            ensure!(projected, "SourceSymlinkDenied");
            resolve(self.build, self.root, &name)?
        } else {
            source.to_owned()
        };
        let metadata = fs::metadata(&resolved)?;
        let relative = name
            .strip_prefix(self.context)
            .context("InputPathOutsideContext")?
            .trim_start_matches('/');
        if metadata.is_dir() {
            let mode = if projected || mapping.is_none() {
                0o755
            } else {
                metadata.permissions().mode() & 0o777
            };
            if !relative.is_empty() {
                self.header(b'd', relative, mode, 0);
            }
            fs::create_dir(destination)?;
            let mut members = fs::read_dir(&resolved)?
                .take(FILE_LIMIT + 1)
                .collect::<std::io::Result<Vec<_>>>()?;
            ensure!(members.len() <= FILE_LIMIT, "TooManyInputFiles");
            members.sort_by_key(|entry| entry.file_name());
            for entry in members {
                let file_name = entry.file_name();
                let text = file_name.to_str().context("NonUtf8InputPath")?;
                if projected && text.starts_with("..") {
                    continue;
                }
                kimage_core::build::relative(text)?;
                // Walk logical projection paths so canonical symlinks cannot
                // change COPY names or expose Kubernetes' internal directories.
                self.walk(&source.join(&file_name), &destination.join(text), depth + 1)?;
            }
            fs::set_permissions(destination, fs::Permissions::from_mode(mode))?;
        } else {
            ensure!(metadata.is_file(), "InputNotRegularFile");
            ensure!(mapping.is_some(), "InputFileOutsideDeclaredMounts");
            self.files += 1;
            ensure!(self.files <= FILE_LIMIT, "TooManyInputFiles");
            self.bytes = self
                .bytes
                .checked_add(metadata.len())
                .context("InputSizeOverflow")?;
            ensure!(self.bytes <= BYTE_LIMIT, "InputSnapshotTooLarge");
            let mode = declared_mode(self.build, &name, metadata.permissions().mode())?;
            self.header(b'f', relative, mode, metadata.len());
            let mut input = File::open(&resolved)?;
            let before = input.metadata()?;
            let mut output = File::options()
                .create_new(true)
                .write(true)
                .open(destination)?;
            let mut buffer = [0_u8; 65536];
            let mut count = 0_u64;
            loop {
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                ensure!(count <= before.len(), "SourceChangedDuringCapture");
                self.hasher.update(&buffer[..n]);
                output.write_all(&buffer[..n])?;
            }
            let after = input.metadata()?;
            ensure!(
                count == before.len()
                    && before.len() == after.len()
                    && before.modified()? == after.modified()?,
                "SourceChangedDuringCapture"
            );
            output.sync_all()?;
            fs::set_permissions(destination, fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    }
}
pub fn capture(build: &Build, root: &Path, output: &Path, sensitive: &Path) -> Result<Capture> {
    build.validate()?;
    fs::create_dir_all(output)?;
    fs::create_dir_all(sensitive)?;
    let context = physical(root, &build.inputs.context_path)?;
    let mut copier = Copy {
        build,
        root,
        context: &build.inputs.context_path,
        hasher: Sha256::new(),
        files: 0,
        bytes: 0,
        members: 0,
    };
    copier.hasher.update(b"kimage-ordinary-inputs-v1\0");
    if context.exists() {
        ensure!(context.is_dir(), "ContextNotDirectory");
        copier.walk(&context, &output.join("context"), 0)?;
    } else {
        ensure!(
            !build
                .inputs
                .volume_mounts
                .iter()
                .any(|m| kimage_core::build::beneath(&m.mount_path, &build.inputs.context_path)),
            "MissingInputContext"
        );
        fs::create_dir(output.join("context"))?;
    }
    let (definition, definition_sensitive) = match &build.definition {
        Definition::Inline(text) => (text.as_bytes().to_vec(), false),
        Definition::FromFile(file) => {
            let (_, volume) = build.inputs.mount_for(&file.path)?;
            let path = resolve(build, root, &file.path)?;
            let meta = fs::metadata(&path)?;
            ensure!(
                meta.is_file() && meta.len() > 0 && meta.len() <= DEFINITION_LIMIT as u64,
                "InvalidDefinitionFile"
            );
            let mut bytes = Vec::new();
            let mut input = File::open(path)?;
            let before = input.metadata()?;
            (&mut input)
                .take(DEFINITION_LIMIT as u64 + 1)
                .read_to_end(&mut bytes)?;
            let after = input.metadata()?;
            ensure!(
                bytes.len() as u64 == before.len()
                    && before.len() == after.len()
                    && before.modified()? == after.modified()?,
                "DefinitionChangedDuringCapture"
            );
            ensure!(bytes.len() <= DEFINITION_LIMIT, "DefinitionTooLarge");
            (bytes, volume.secret.is_some())
        }
    };
    ensure!(
        !definition.is_empty() && !definition.contains(&0) && definition.len() <= DEFINITION_LIMIT,
        "InvalidDefinitionText"
    );
    std::str::from_utf8(&definition).context("DefinitionMustBeUtf8")?;
    let definition_digest = oci::digest(&definition);
    if !definition_sensitive {
        copier.hasher.update(b"\0definition\0");
        copier.hasher.update(definition_digest.as_bytes());
    }
    let directory = if definition_sensitive {
        sensitive.join("definition")
    } else {
        output.join("definition")
    };
    fs::create_dir(&directory)?;
    let target = directory.join("Containerfile");
    let mut file = File::options().create_new(true).write(true).open(&target)?;
    file.write_all(&definition)?;
    file.sync_all()?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o400))?;
    for binding in &build.build_secrets {
        let source = resolve(build, root, &binding.source_file)?;
        ensure!(
            fs::metadata(&source)?.is_file() && fs::metadata(&source)?.len() <= 500 * 1024,
            "BuildSecretNotRegularFile"
        );
        let directory = sensitive.join("secrets");
        fs::create_dir_all(&directory)?;
        let target = directory.join(&binding.id);
        let mut input = File::open(source)?;
        let mut output = File::options().create_new(true).write(true).open(&target)?;
        oci::copy_hash(&mut input, &mut output, 500 * 1024)?;
        output.sync_all()?;
        fs::set_permissions(target, fs::Permissions::from_mode(0o400))?;
        // Fixed private filenames avoid author-controlled CSV paths in buildctl.
        // This directory must be an independent memory-backed volume.
    }
    Ok(Capture {
        ordinary_digest: format!("sha256:{}", hex::encode(copier.hasher.finalize())),
        definition_sensitive,
        definition_digest,
        files: copier.files,
        bytes: copier.bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn build(secret: bool) -> Build {
        serde_json::from_value(json!({"format":"Containerfile","definition":{"fromFile":{"path":if secret{"/run/kimage-build-secrets/recipe/Containerfile"}else{"/workspace/inputs/recipe/Containerfile"}}},"inputs":{"contextPath":"/workspace/context","volumes":[{"name":"src","persistentVolumeClaim":{"claimName":"src","readOnly":true}},{"name":"recipe",if secret{"secret"}else{"configMap"}:{"name":"recipe","defaultMode":256}}],"volumeMounts":[{"name":"src","mountPath":"/workspace/context/src","readOnly":true,"subPath":"project"},{"name":"recipe","mountPath":if secret{"/run/kimage-build-secrets/recipe"}else{"/workspace/inputs/recipe"},"readOnly":true}],"producerHandoff":true},"execution":{"strategy":"Direct","builderRef":"default"},"cache":{"mode":"Local"}})).unwrap()
    }
    #[test]
    fn mounted_definition_keeps_context_root_and_secret_fingerprint_private() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("workspace/context/src")).unwrap();
        fs::write(root.join("workspace/context/src/main"), b"\0binary\xff").unwrap();
        for secret in [false, true] {
            let build = build(secret);
            let Definition::FromFile(file) = &build.definition else {
                unreachable!()
            };
            let recipe = physical(&root, &file.path).unwrap();
            fs::create_dir_all(recipe.parent().unwrap()).unwrap();
            fs::write(&recipe, b"FROM scratch\nCOPY src/main /main\n").unwrap();
            let out = dir.path().join(format!("out-{secret}"));
            let private = dir.path().join(format!("private-{secret}"));
            let first = capture(&build, &root, &out, &private).unwrap();
            assert_eq!(
                fs::read(out.join("context/src/main")).unwrap(),
                b"\0binary\xff"
            );
            assert!(!out.join("context/Containerfile").exists());
            fs::write(&recipe, b"FROM scratch\nCOPY src/main /other\n").unwrap();
            let second = capture(
                &build,
                &root,
                &dir.path().join(format!("out2-{secret}")),
                &dir.path().join(format!("private2-{secret}")),
            )
            .unwrap();
            assert_ne!(first.definition_digest, second.definition_digest);
            assert_eq!(first.ordinary_digest == second.ordinary_digest, secret);
            assert_eq!(private.join("definition/Containerfile").exists(), secret);
        }
    }
    #[test]
    fn pvc_symlink_escape_is_refused_before_output_publication() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("workspace/context/src")).unwrap();
        fs::write(dir.path().join("outside"), b"outside").unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("outside"),
            root.join("workspace/context/src/link"),
        )
        .unwrap();
        let mut build = build(false);
        build.definition = Definition::Inline("FROM scratch\n".into());
        assert!(
            capture(
                &build,
                &root,
                &dir.path().join("output"),
                &dir.path().join("private")
            )
            .err()
            .unwrap()
            .to_string()
            .contains("SourceSymlinkDenied")
        );
    }
}
