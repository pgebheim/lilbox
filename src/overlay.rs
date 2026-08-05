//! Docker-free OCI image overlay builder.
//!
//! Pulls a base OCI image straight from a registry, appends a single new
//! layer, and writes a standard OCI image-layout tar that
//! [`microsandbox::Image::load`] can ingest directly. No local Docker daemon
//! is involved anywhere in this path.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow};
use flate2::{Compression, write::GzEncoder};
use oci_client::{
    Client, Reference,
    client::ClientConfig,
    manifest::{
        IMAGE_DOCKER_LAYER_GZIP_MEDIA_TYPE, IMAGE_DOCKER_LAYER_TAR_MEDIA_TYPE,
        IMAGE_LAYER_GZIP_MEDIA_TYPE, IMAGE_LAYER_MEDIA_TYPE,
        IMAGE_LAYER_NONDISTRIBUTABLE_GZIP_MEDIA_TYPE, IMAGE_LAYER_NONDISTRIBUTABLE_MEDIA_TYPE,
    },
    secrets::RegistryAuth,
};
use oci_spec::image::{
    Arch, Descriptor, DescriptorBuilder, Digest as OciDigest, HistoryBuilder, ImageConfiguration,
    ImageIndexBuilder, ImageManifest, MediaType, OciLayoutBuilder,
};
use sha2::{Digest as Sha2Digest, Sha256};

use crate::util::now;

const OCI_REF_NAME_ANNOTATION: &str = "org.opencontainers.image.ref.name";

/// Tailscale release to fetch for the `tailscalify` overlay layer.
const TAILSCALE_VERSION: &str = "1.102.2";

/// Pinned SHA-256 of each `TAILSCALE_VERSION` release tarball (from
/// `pkgs.tailscale.com/stable/tailscale_<ver>_<arch>.tgz.sha256`). The
/// download is verified against this before its binaries are unpacked into a
/// layer that runs as root in the guest — HTTPS authenticates the host, this
/// pins the exact artifact. Bump both when `TAILSCALE_VERSION` changes.
const TAILSCALE_SHA256_AMD64: &str =
    "ad2cde12f8de95f7b93a1e0401e652291c603d42b9d60a33fb1741eb38ab04d8";
const TAILSCALE_SHA256_ARM64: &str =
    "2b64e9ade7e73034b5ec9e9bcd537f5ddd14ae3abb435e57e929e7486ae42660";

/// The canonical lilbox-box first-boot bring-up hook, embedded so the
/// tailscalified image and the reference `images/lilbox-box` build share a
/// single source of truth.
const LILBOX_BOOT: &str = include_str!("../images/lilbox-box/lilbox-boot");

/// A base image pulled from a registry: its raw manifest/config JSON plus
/// every layer blob (bottom-to-top), keyed by their compressed digest hex.
pub(crate) struct BaseImage {
    pub(crate) config_bytes: Vec<u8>,
    pub(crate) manifest_bytes: Vec<u8>,
    /// `(digest hex, compressed layer bytes)`, bottom-to-top.
    pub(crate) layer_blobs: Vec<(String, Vec<u8>)>,
}

/// Pull `reference`'s manifest, config, and every layer blob for the host
/// platform. Anonymous access only -- sufficient for public base images in
/// this thin slice.
pub(crate) async fn pull_base(reference: &str) -> Result<BaseImage> {
    let oci_ref: Reference = reference
        .parse()
        .map_err(|e| anyhow!("invalid image reference '{reference}': {e}"))?;

    let client = Client::new(ClientConfig::default());
    let accepted_media_types = vec![
        IMAGE_LAYER_MEDIA_TYPE,
        IMAGE_LAYER_GZIP_MEDIA_TYPE,
        IMAGE_DOCKER_LAYER_TAR_MEDIA_TYPE,
        IMAGE_DOCKER_LAYER_GZIP_MEDIA_TYPE,
        IMAGE_LAYER_NONDISTRIBUTABLE_MEDIA_TYPE,
        IMAGE_LAYER_NONDISTRIBUTABLE_GZIP_MEDIA_TYPE,
    ];

    let image_data = client
        .pull(&oci_ref, &RegistryAuth::Anonymous, accepted_media_types)
        .await
        .with_context(|| format!("pulling base image '{reference}'"))?;

    let manifest = image_data
        .manifest
        .ok_or_else(|| anyhow!("registry returned no manifest for '{reference}'"))?;

    if manifest.layers.len() != image_data.layers.len() {
        return Err(anyhow!(
            "manifest for '{reference}' lists {} layers but {} were downloaded",
            manifest.layers.len(),
            image_data.layers.len()
        ));
    }

    let layer_blobs = manifest
        .layers
        .iter()
        .zip(image_data.layers.iter())
        .map(|(descriptor, layer)| {
            let hex = strip_sha256(&descriptor.digest)?.to_string();
            Ok((hex, layer.data.to_vec()))
        })
        .collect::<Result<Vec<_>>>()?;

    let manifest_bytes = serde_json::to_vec(&manifest).context("serializing pulled manifest")?;

    Ok(BaseImage {
        config_bytes: image_data.config.data.to_vec(),
        manifest_bytes,
        layer_blobs,
    })
}

/// Tar `files` and gzip the result.
///
/// Returns `(gzip_bytes, layer_digest, diff_id)`: `layer_digest` is
/// `sha256:<hex of the gzip bytes>` (the compressed blob identity recorded in
/// a manifest layer descriptor), and `diff_id` is `sha256:<hex of the
/// uncompressed tar>` (the identity recorded in the image config's
/// `rootfs.diff_ids`). The two are intentionally different digests over
/// different content.
pub(crate) fn build_layer(files: &[(String, Vec<u8>, u32)]) -> Result<(Vec<u8>, String, String)> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for (path, data, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, path, data.as_slice())
                .with_context(|| format!("tarring '{path}'"))?;
        }
        builder.finish().context("finishing layer tar")?;
    }
    let diff_id = format!("sha256:{}", sha256_hex(&tar_bytes));

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&tar_bytes)
        .context("gzipping layer tar")?;
    let gzip_bytes = encoder.finish().context("finishing layer gzip")?;
    let layer_digest = format!("sha256:{}", sha256_hex(&gzip_bytes));

    Ok((gzip_bytes, layer_digest, diff_id))
}

/// Push `diff_id` onto the base config's `rootfs.diff_ids` and record a
/// history entry for the new layer. Returns `(new_config_json, config_digest)`.
pub(crate) fn append_layer_to_config(
    base_config_json: &[u8],
    diff_id: &str,
) -> Result<(Vec<u8>, String)> {
    let mut config: ImageConfiguration =
        serde_json::from_slice(base_config_json).context("parsing base image config")?;

    config.rootfs_mut().diff_ids_mut().push(diff_id.to_string());

    let entry = HistoryBuilder::default()
        .created_by("lilbox overlay: append marker layer".to_string())
        .build()
        .map_err(|e| anyhow!("building history entry: {e}"))?;
    config
        .history_mut()
        .get_or_insert_with(Vec::new)
        .push(entry);

    let new_config_json = serde_json::to_vec(&config).context("serializing new image config")?;
    let config_digest = format!("sha256:{}", sha256_hex(&new_config_json));
    Ok((new_config_json, config_digest))
}

/// Append `layer_descriptor` to the base manifest's layer list and point its
/// config descriptor at `new_config_descriptor`. Returns
/// `(new_manifest_json, manifest_digest)`.
pub(crate) fn append_layer_to_manifest(
    base_manifest_json: &[u8],
    layer_descriptor: Descriptor,
    new_config_descriptor: Descriptor,
) -> Result<(Vec<u8>, String)> {
    let mut manifest: ImageManifest =
        serde_json::from_slice(base_manifest_json).context("parsing base image manifest")?;

    manifest.layers_mut().push(layer_descriptor);
    manifest.set_config(new_config_descriptor);

    let new_manifest_json =
        serde_json::to_vec(&manifest).context("serializing new image manifest")?;
    let manifest_digest = format!("sha256:{}", sha256_hex(&new_manifest_json));
    Ok((new_manifest_json, manifest_digest))
}

/// Write a standard OCI image-layout tar to `output`: `oci-layout`,
/// `index.json` (pointing at `manifest_bytes`, tagged `tag` via the
/// `org.opencontainers.image.ref.name` annotation), and every blob under
/// `blobs/sha256/<hex>` -- the exact layout `microsandbox::Image::load`
/// verifies and ingests.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_oci_archive(
    output: &Path,
    config_bytes: &[u8],
    config_digest: &str,
    manifest_bytes: &[u8],
    manifest_digest: &str,
    layer_blobs: &[(String, Vec<u8>)],
    tag: &str,
) -> Result<()> {
    let file = std::fs::File::create(output)
        .with_context(|| format!("creating '{}'", output.display()))?;
    let mut archive = tar::Builder::new(std::io::BufWriter::new(file));

    let oci_layout = OciLayoutBuilder::default()
        .image_layout_version("1.0.0".to_string())
        .build()
        .map_err(|e| anyhow!("building oci-layout: {e}"))?;
    append_bytes(
        &mut archive,
        "oci-layout",
        &serde_json::to_vec(&oci_layout)?,
    )?;

    let manifest_digest_parsed: OciDigest = manifest_digest
        .parse()
        .map_err(|e| anyhow!("parsing manifest digest '{manifest_digest}': {e}"))?;
    let mut annotations = HashMap::new();
    annotations.insert(OCI_REF_NAME_ANNOTATION.to_string(), tag.to_string());
    let manifest_descriptor = DescriptorBuilder::default()
        .media_type(MediaType::ImageManifest)
        .digest(manifest_digest_parsed)
        .size(manifest_bytes.len() as u64)
        .annotations(annotations)
        .build()
        .map_err(|e| anyhow!("building manifest descriptor: {e}"))?;
    let index = ImageIndexBuilder::default()
        .schema_version(2u32)
        .manifests(vec![manifest_descriptor])
        .build()
        .map_err(|e| anyhow!("building image index: {e}"))?;
    append_bytes(
        &mut archive,
        "index.json",
        &serde_json::to_vec_pretty(&index)?,
    )?;

    append_directory(&mut archive, "blobs")?;
    append_directory(&mut archive, "blobs/sha256")?;

    append_bytes(
        &mut archive,
        &format!("blobs/sha256/{}", strip_sha256(config_digest)?),
        config_bytes,
    )?;
    append_bytes(
        &mut archive,
        &format!("blobs/sha256/{}", strip_sha256(manifest_digest)?),
        manifest_bytes,
    )?;
    for (hex, bytes) in layer_blobs {
        append_bytes(&mut archive, &format!("blobs/sha256/{hex}"), bytes)?;
    }

    // into_inner() finishes the tar AND hands back the BufWriter so we can
    // flush it explicitly — a flush error on drop would otherwise be swallowed,
    // leaving a truncated archive that Image::load would later reject.
    let mut writer = archive.into_inner().context("finishing OCI archive tar")?;
    writer.flush().context("flushing OCI archive")?;
    Ok(())
}

/// A single new layer ready to be appended onto a [`BaseImage`]: its gzip
/// blob plus the compressed (`digest`) and uncompressed (`diff_id`) digests
/// produced by [`build_layer`].
pub(crate) struct BuiltLayer {
    pub(crate) gzip: Vec<u8>,
    pub(crate) digest: String,
    pub(crate) diff_id: String,
}

/// Pull `base`, append a single new layer containing `extra_files` (each
/// written with mode `0o644`), write a standard OCI image-layout tar, and
/// load it into microsandbox as `tag`.
pub(crate) async fn overlay_image(
    base: &str,
    extra_files: &[(String, Vec<u8>)],
    tag: &str,
) -> Result<()> {
    let base_image = pull_base(base).await?;

    let files: Vec<(String, Vec<u8>, u32)> = extra_files
        .iter()
        .map(|(path, data)| (path.clone(), data.clone(), 0o644))
        .collect();
    let (gzip, digest, diff_id) = build_layer(&files)?;

    assemble_and_load(
        base_image,
        vec![BuiltLayer {
            gzip,
            digest,
            diff_id,
        }],
        tag,
    )
    .await
}

/// Given a pulled `base` and one or more `new_layers` built by
/// [`build_layer`], rewrite the base's config and manifest to append every
/// new layer, write the resulting OCI image-layout tar, and load it into
/// microsandbox as `tag`. Shared assembly tail for [`overlay_image`] and
/// [`tailscalify_image`].
async fn assemble_and_load(base: BaseImage, new_layers: Vec<BuiltLayer>, tag: &str) -> Result<()> {
    let mut config_bytes = base.config_bytes;
    let mut config_digest = String::new();
    for layer in &new_layers {
        let (new_config_bytes, digest) = append_layer_to_config(&config_bytes, &layer.diff_id)?;
        config_bytes = new_config_bytes;
        config_digest = digest;
    }

    let config_descriptor = DescriptorBuilder::default()
        .media_type(MediaType::ImageConfig)
        .digest(
            config_digest
                .parse::<OciDigest>()
                .map_err(|e| anyhow!("parsing config digest '{config_digest}': {e}"))?,
        )
        .size(config_bytes.len() as u64)
        .build()
        .map_err(|e| anyhow!("building config descriptor: {e}"))?;

    let mut manifest_bytes = base.manifest_bytes;
    let mut manifest_digest = String::new();
    for layer in &new_layers {
        let layer_descriptor = DescriptorBuilder::default()
            .media_type(MediaType::ImageLayerGzip)
            .digest(
                layer
                    .digest
                    .parse::<OciDigest>()
                    .map_err(|e| anyhow!("parsing layer digest '{}': {e}", layer.digest))?,
            )
            .size(layer.gzip.len() as u64)
            .build()
            .map_err(|e| anyhow!("building layer descriptor: {e}"))?;

        let (new_manifest_bytes, digest) =
            append_layer_to_manifest(&manifest_bytes, layer_descriptor, config_descriptor.clone())?;
        manifest_bytes = new_manifest_bytes;
        manifest_digest = digest;
    }

    let mut layer_blobs = base.layer_blobs;
    for layer in new_layers {
        layer_blobs.push((strip_sha256(&layer.digest)?.to_string(), layer.gzip));
    }

    let tmp_path = std::env::temp_dir().join(format!(
        "lilbox-overlay-{}-{}.tar",
        std::process::id(),
        now()
    ));

    if let Err(err) = write_oci_archive(
        &tmp_path,
        &config_bytes,
        &config_digest,
        &manifest_bytes,
        &manifest_digest,
        &layer_blobs,
        tag,
    ) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    let loaded = microsandbox::Image::load(&tmp_path, vec![tag.to_string()]).await;
    let _ = std::fs::remove_file(&tmp_path);
    loaded?;
    Ok(())
}

/// Determine the Tailscale arch string (`amd64`/`arm64`) for the given OCI
/// [`Arch`]. Bails on architectures Tailscale packages don't ship or that
/// this overlay doesn't support yet.
fn tailscale_arch_string(arch: &Arch) -> Result<&'static str> {
    match arch {
        Arch::Amd64 => Ok("amd64"),
        Arch::ARM64 => Ok("arm64"),
        other => Err(anyhow!(
            "unsupported base image architecture '{other}' for tailscale overlay (only amd64/arm64 are supported)"
        )),
    }
}

/// Extract `tailscale`/`tailscaled` from a Tailscale release `.tgz` and pair
/// them with the embedded `lilbox-boot` hook. Pure: no I/O beyond decoding
/// `tarball_gz` in memory. Returns `(path, bytes, mode)` triples, all mode
/// `0o755`.
fn tailscale_layer_files(
    tarball_gz: &[u8],
    version: &str,
    arch: &str,
) -> Result<Vec<(String, Vec<u8>, u32)>> {
    let mut decoder = flate2::read::GzDecoder::new(tarball_gz);
    let mut tar_bytes = Vec::new();
    decoder
        .read_to_end(&mut tar_bytes)
        .context("gunzipping tailscale release tarball")?;

    let prefix = format!("tailscale_{version}_{arch}/");
    let tailscale_path = format!("{prefix}tailscale");
    let tailscaled_path = format!("{prefix}tailscaled");

    let mut tailscale_bin: Option<Vec<u8>> = None;
    let mut tailscaled_bin: Option<Vec<u8>> = None;

    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    for entry in archive
        .entries()
        .context("reading tailscale release tarball entries")?
    {
        let mut entry = entry.context("reading a tailscale release tarball entry")?;
        let entry_path = entry.path()?.to_string_lossy().into_owned();
        if entry_path == tailscale_path {
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            tailscale_bin = Some(data);
        } else if entry_path == tailscaled_path {
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            tailscaled_bin = Some(data);
        }
    }

    let tailscaled_bin = tailscaled_bin.ok_or_else(|| {
        anyhow!("tailscale release tarball is missing expected binary '{tailscaled_path}'")
    })?;
    let tailscale_bin = tailscale_bin.ok_or_else(|| {
        anyhow!("tailscale release tarball is missing expected binary '{tailscale_path}'")
    })?;

    Ok(vec![
        (
            "usr/local/bin/tailscaled".to_string(),
            tailscaled_bin,
            0o755,
        ),
        ("usr/local/bin/tailscale".to_string(), tailscale_bin, 0o755),
        (
            "usr/local/bin/lilbox-boot".to_string(),
            LILBOX_BOOT.as_bytes().to_vec(),
            0o755,
        ),
    ])
}

/// Download a Tailscale release tarball for `arch` from the stable channel.
async fn download_tailscale(arch: &str, version: &str) -> Result<Vec<u8>> {
    let url = format!("https://pkgs.tailscale.com/stable/tailscale_{version}_{arch}.tgz");
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("downloading tailscale from '{url}'"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "downloading tailscale from '{url}' failed: {status}"
        ));
    }
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("reading tailscale download body from '{url}'"))?
        .to_vec();
    // Verify the download against the pinned digest before we unpack a binary
    // that will run as root in the guest.
    let expected = match arch {
        "amd64" => TAILSCALE_SHA256_AMD64,
        "arm64" => TAILSCALE_SHA256_ARM64,
        other => return Err(anyhow!("no pinned tailscale checksum for arch '{other}'")),
    };
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(anyhow!(
            "tailscale download from '{url}' failed checksum verification (expected {expected}, got {actual})"
        ));
    }
    Ok(bytes)
}

/// Derive a deterministic, valid microsandbox image tag for the
/// tailscalified variant of `base`, e.g.
/// `docker.io/library/alpine:latest` -> `lilbox/tailnet/alpine-latest-ts1.102.2`.
///
/// The base reference is sanitized: any leading registry host (and, for
/// Docker Hub, the `library/` official-image prefix) is stripped, then every
/// remaining character outside `[a-z0-9._-]` (including `/` and `:`) is
/// replaced with `-` and the result is lowercased.
pub(crate) fn tailnet_image_tag(base: &str) -> String {
    format!(
        "lilbox/tailnet/{}-ts{TAILSCALE_VERSION}",
        sanitize_base_ref(base)
    )
}

/// Cache-aware wrapper around [`tailscalify_image`]: returns the cached tag
/// instantly if a tailscalified variant of `base` was already built (unless
/// `force`), otherwise builds it and returns the newly cached tag.
pub(crate) async fn ensure_tailnet_image(base: &str, force: bool) -> Result<String> {
    let tag = tailnet_image_tag(base);
    if !force && microsandbox::Image::get(&tag).await.is_ok() {
        return Ok(tag);
    }
    println!("building tailnet-capable {base} (first run; cached after)...");
    tailscalify_image(base, &tag).await?;
    Ok(tag)
}

fn sanitize_base_ref(base: &str) -> String {
    let (host, rest) = match base.split_once('/') {
        Some((host, rest)) if host.contains('.') || host.contains(':') || host == "localhost" => {
            (Some(host), rest)
        }
        _ => (None, base),
    };
    let rest = if host == Some("docker.io") {
        rest.strip_prefix("library/").unwrap_or(rest)
    } else {
        rest
    };
    rest.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Pull `base`, append a Tailscale layer (`tailscaled`, `tailscale`, and the
/// `lilbox-boot` bring-up hook), write a standard OCI image-layout tar, and
/// load it into microsandbox as `tag` -- producing a tailnet-capable image
/// from any base.
pub(crate) async fn tailscalify_image(base: &str, tag: &str) -> Result<()> {
    let base_image = pull_base(base).await?;

    let arch = serde_json::from_slice::<ImageConfiguration>(&base_image.config_bytes)
        .context("parsing base image config to read its architecture")?
        .architecture()
        .clone();
    let tailscale_arch = tailscale_arch_string(&arch)?;

    println!("downloading tailscale {TAILSCALE_VERSION} ({tailscale_arch})...");
    let tarball = download_tailscale(tailscale_arch, TAILSCALE_VERSION).await?;
    let files = tailscale_layer_files(&tarball, TAILSCALE_VERSION, tailscale_arch)?;
    let (gzip, digest, diff_id) = build_layer(&files)?;

    assemble_and_load(
        base_image,
        vec![BuiltLayer {
            gzip,
            digest,
            diff_id,
        }],
        tag,
    )
    .await
}

//--------------------------------------------------------------------------------------------------
// Helpers
//--------------------------------------------------------------------------------------------------

fn strip_sha256(digest: &str) -> Result<&str> {
    digest
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow!("digest '{digest}' is not a sha256 digest"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn append_directory<W: Write>(archive: &mut tar::Builder<W>, path: &str) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o755);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(0);
    header.set_cksum();
    archive
        .append_data(&mut header, path, std::io::empty())
        .with_context(|| format!("appending directory '{path}'"))
}

fn append_bytes<W: Write>(archive: &mut tar::Builder<W>, path: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    archive
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("appending '{path}'"))
}

//--------------------------------------------------------------------------------------------------
// Tests
//--------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::str::FromStr;

    use oci_spec::image::{
        Arch, ImageConfigurationBuilder, ImageManifestBuilder, Os, RootFsBuilder,
    };

    use super::*;

    fn fake_digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn read_all_entries(path: &Path) -> HashMap<String, Vec<u8>> {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = tar::Archive::new(file);
        let mut out = HashMap::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let entry_path = entry.path().unwrap().to_string_lossy().into_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            out.insert(entry_path, data);
        }
        out
    }

    #[test]
    fn build_layer_computes_distinct_deterministic_digests() {
        let files = vec![("etc/lilbox-overlay".to_string(), b"v1\n".to_vec(), 0o644u32)];

        let (gzip_a, layer_digest_a, diff_id_a) = build_layer(&files).unwrap();
        let (gzip_b, layer_digest_b, diff_id_b) = build_layer(&files).unwrap();

        assert!(layer_digest_a.starts_with("sha256:"));
        assert!(diff_id_a.starts_with("sha256:"));
        assert_ne!(
            layer_digest_a, diff_id_a,
            "layer digest and diff_id cover different content"
        );

        // Deterministic: identical inputs produce identical outputs.
        assert_eq!(layer_digest_a, layer_digest_b);
        assert_eq!(diff_id_a, diff_id_b);
        assert_eq!(gzip_a, gzip_b);

        // diff_id must equal the sha256 of the UNCOMPRESSED tar.
        let mut decoder = flate2::read::GzDecoder::new(gzip_a.as_slice());
        let mut tar_bytes = Vec::new();
        decoder.read_to_end(&mut tar_bytes).unwrap();
        assert_eq!(diff_id_a, format!("sha256:{}", sha256_hex(&tar_bytes)));

        // layer_digest must equal the sha256 of the COMPRESSED (gzip) bytes.
        assert_eq!(layer_digest_a, format!("sha256:{}", sha256_hex(&gzip_a)));

        // The per-file mode passed to build_layer is honored in the tar header.
        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        let entry = archive.entries().unwrap().next().unwrap().unwrap();
        assert_eq!(entry.header().mode().unwrap(), 0o644);
    }

    #[test]
    fn build_layer_honors_executable_mode() {
        let files = vec![(
            "usr/local/bin/tailscale".to_string(),
            b"bin".to_vec(),
            0o755u32,
        )];

        let (gzip, _, _) = build_layer(&files).unwrap();
        let mut decoder = flate2::read::GzDecoder::new(gzip.as_slice());
        let mut tar_bytes = Vec::new();
        decoder.read_to_end(&mut tar_bytes).unwrap();

        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        let entry = archive.entries().unwrap().next().unwrap().unwrap();
        assert_eq!(entry.header().mode().unwrap(), 0o755);
    }

    fn synthetic_tailscale_tarball(version: &str, arch: &str) -> Vec<u8> {
        let prefix = format!("tailscale_{version}_{arch}");
        let files = vec![
            (
                format!("{prefix}/tailscale"),
                b"fake tailscale binary".to_vec(),
            ),
            (
                format!("{prefix}/tailscaled"),
                b"fake tailscaled binary".to_vec(),
            ),
            (format!("{prefix}/README"), b"not a binary".to_vec()),
        ];

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (path, data) in &files {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, data.as_slice())
                    .unwrap();
            }
            builder.finish().unwrap();
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn tailscale_layer_files_extracts_binaries_and_boot_hook() {
        let version = "1.102.2";
        let arch = "amd64";
        let tarball_gz = synthetic_tailscale_tarball(version, arch);

        let files = tailscale_layer_files(&tarball_gz, version, arch).unwrap();
        let by_path: HashMap<String, (Vec<u8>, u32)> = files
            .into_iter()
            .map(|(path, data, mode)| (path, (data, mode)))
            .collect();

        let (tailscaled_data, tailscaled_mode) = by_path.get("usr/local/bin/tailscaled").unwrap();
        assert_eq!(tailscaled_data, b"fake tailscaled binary");
        assert_eq!(*tailscaled_mode, 0o755);

        let (tailscale_data, tailscale_mode) = by_path.get("usr/local/bin/tailscale").unwrap();
        assert_eq!(tailscale_data, b"fake tailscale binary");
        assert_eq!(*tailscale_mode, 0o755);

        let (boot_data, boot_mode) = by_path.get("usr/local/bin/lilbox-boot").unwrap();
        assert_eq!(std::str::from_utf8(boot_data).unwrap(), LILBOX_BOOT);
        assert_eq!(*boot_mode, 0o755);
    }

    #[test]
    fn tailscale_layer_files_errors_when_binary_missing() {
        let version = "1.102.2";
        let arch = "amd64";
        let prefix = format!("tailscale_{version}_{arch}");

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let data = b"fake tailscale binary".to_vec();
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{prefix}/tailscale"), data.as_slice())
                .unwrap();
            builder.finish().unwrap();
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let tarball_gz = encoder.finish().unwrap();

        let err = tailscale_layer_files(&tarball_gz, version, arch).unwrap_err();
        assert!(err.to_string().contains("tailscaled"));
    }

    #[test]
    fn tailnet_image_tag_is_deterministic() {
        let base = "docker.io/library/alpine:latest";
        assert_eq!(tailnet_image_tag(base), tailnet_image_tag(base));
    }

    #[test]
    fn tailnet_image_tag_sanitizes_docker_hub_official_image() {
        assert_eq!(
            tailnet_image_tag("docker.io/library/alpine:latest"),
            format!("lilbox/tailnet/alpine-latest-ts{TAILSCALE_VERSION}")
        );
    }

    #[test]
    fn tailnet_image_tag_sanitizes_ghcr_ref_to_valid_chars() {
        let tag = tailnet_image_tag("ghcr.io/foo/bar:v1.0");
        assert_eq!(
            tag,
            format!("lilbox/tailnet/foo-bar-v1.0-ts{TAILSCALE_VERSION}")
        );
        let sanitized = tag.strip_prefix("lilbox/tailnet/").unwrap();
        assert!(
            sanitized.chars().all(|c| c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, '.' | '_' | '-')),
            "sanitized portion '{sanitized}' contains an invalid tag character"
        );
    }

    #[test]
    fn tailnet_image_tag_contains_tailscale_version() {
        assert!(tailnet_image_tag("alpine:latest").ends_with(&format!("-ts{TAILSCALE_VERSION}")));
    }

    #[test]
    fn append_layer_to_config_pushes_diff_id_and_records_history() {
        let base_config = ImageConfigurationBuilder::default()
            .architecture(Arch::Amd64)
            .os(Os::Linux)
            .rootfs(
                RootFsBuilder::default()
                    .typ("layers".to_string())
                    .diff_ids(vec![fake_digest('a')])
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let base_json = serde_json::to_vec(&base_config).unwrap();

        let new_diff_id = fake_digest('b');
        let (new_json, digest) = append_layer_to_config(&base_json, &new_diff_id).unwrap();

        let parsed: ImageConfiguration = serde_json::from_slice(&new_json).unwrap();
        assert_eq!(
            parsed.rootfs().diff_ids(),
            &vec![fake_digest('a'), new_diff_id]
        );
        assert_eq!(parsed.history().as_ref().map(Vec::len), Some(1));
        assert_eq!(digest, format!("sha256:{}", sha256_hex(&new_json)));
    }

    #[test]
    fn append_layer_to_manifest_appends_layer_and_swaps_config() {
        let base_layer = DescriptorBuilder::default()
            .media_type(MediaType::ImageLayerGzip)
            .digest(OciDigest::from_str(&fake_digest('1')).unwrap())
            .size(10u64)
            .build()
            .unwrap();
        let base_config_descriptor = DescriptorBuilder::default()
            .media_type(MediaType::ImageConfig)
            .digest(OciDigest::from_str(&fake_digest('2')).unwrap())
            .size(20u64)
            .build()
            .unwrap();
        let base_manifest = ImageManifestBuilder::default()
            .schema_version(2u32)
            .config(base_config_descriptor)
            .layers(vec![base_layer.clone()])
            .build()
            .unwrap();
        let base_manifest_json = serde_json::to_vec(&base_manifest).unwrap();

        let new_layer = DescriptorBuilder::default()
            .media_type(MediaType::ImageLayerGzip)
            .digest(OciDigest::from_str(&fake_digest('3')).unwrap())
            .size(30u64)
            .build()
            .unwrap();
        let new_config_descriptor = DescriptorBuilder::default()
            .media_type(MediaType::ImageConfig)
            .digest(OciDigest::from_str(&fake_digest('4')).unwrap())
            .size(40u64)
            .build()
            .unwrap();

        let (new_manifest_json, digest) = append_layer_to_manifest(
            &base_manifest_json,
            new_layer.clone(),
            new_config_descriptor.clone(),
        )
        .unwrap();

        let parsed: ImageManifest = serde_json::from_slice(&new_manifest_json).unwrap();
        assert_eq!(parsed.layers().len(), 2);
        assert_eq!(parsed.layers()[0], base_layer);
        assert_eq!(parsed.layers()[1], new_layer);
        assert_eq!(parsed.config(), &new_config_descriptor);
        assert_eq!(digest, format!("sha256:{}", sha256_hex(&new_manifest_json)));
    }

    #[test]
    fn write_oci_archive_round_trips_verifiable_digests() {
        // A minimal, hand-built synthetic base: one tiny layer, a config
        // whose rootfs references it, and a manifest tying both together.
        let layer_bytes = b"hello layer".to_vec();
        let layer_hex = sha256_hex(&layer_bytes);
        let layer_digest = format!("sha256:{layer_hex}");

        let config = ImageConfigurationBuilder::default()
            .architecture(Arch::Amd64)
            .os(Os::Linux)
            .rootfs(
                RootFsBuilder::default()
                    .typ("layers".to_string())
                    .diff_ids(vec![layer_digest.clone()])
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let config_bytes = serde_json::to_vec(&config).unwrap();
        let config_digest = format!("sha256:{}", sha256_hex(&config_bytes));

        let layer_descriptor = DescriptorBuilder::default()
            .media_type(MediaType::ImageLayerGzip)
            .digest(OciDigest::from_str(&layer_digest).unwrap())
            .size(layer_bytes.len() as u64)
            .build()
            .unwrap();
        let config_descriptor = DescriptorBuilder::default()
            .media_type(MediaType::ImageConfig)
            .digest(OciDigest::from_str(&config_digest).unwrap())
            .size(config_bytes.len() as u64)
            .build()
            .unwrap();
        let manifest = ImageManifestBuilder::default()
            .schema_version(2u32)
            .config(config_descriptor)
            .layers(vec![layer_descriptor])
            .build()
            .unwrap();
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_digest = format!("sha256:{}", sha256_hex(&manifest_bytes));

        let dir = std::env::temp_dir().join(format!(
            "lilbox-overlay-write-test-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let archive_path = dir.join("image.tar");

        write_oci_archive(
            &archive_path,
            &config_bytes,
            &config_digest,
            &manifest_bytes,
            &manifest_digest,
            &[(layer_hex.clone(), layer_bytes.clone())],
            "lilbox-test:overlay",
        )
        .unwrap();

        let entries = read_all_entries(&archive_path);
        let _ = std::fs::remove_dir_all(&dir);

        let layout: oci_spec::image::OciLayout =
            serde_json::from_slice(entries.get("oci-layout").expect("oci-layout present"))
                .expect("oci-layout parses");
        assert_eq!(layout.image_layout_version(), "1.0.0");

        let index: oci_spec::image::ImageIndex =
            serde_json::from_slice(entries.get("index.json").expect("index.json present"))
                .expect("index.json parses");
        assert_eq!(index.manifests().len(), 1);
        let manifest_entry = &index.manifests()[0];
        assert_eq!(manifest_entry.digest().to_string(), manifest_digest);
        assert_eq!(manifest_entry.size(), manifest_bytes.len() as u64);
        assert_eq!(
            manifest_entry
                .annotations()
                .as_ref()
                .and_then(|a| a.get(OCI_REF_NAME_ANNOTATION)),
            Some(&"lilbox-test:overlay".to_string())
        );

        let manifest_blob = entries
            .get(&format!(
                "blobs/sha256/{}",
                strip_sha256(&manifest_digest).unwrap()
            ))
            .expect("manifest blob present");
        assert_eq!(
            sha256_hex(manifest_blob),
            strip_sha256(&manifest_digest).unwrap()
        );
        let parsed_manifest: ImageManifest =
            serde_json::from_slice(manifest_blob).expect("manifest blob parses");

        let config_blob = entries
            .get(&format!(
                "blobs/sha256/{}",
                parsed_manifest.config().digest().digest()
            ))
            .expect("config blob present");
        assert_eq!(
            sha256_hex(config_blob),
            parsed_manifest.config().digest().digest()
        );
        assert_eq!(config_blob.len() as u64, parsed_manifest.config().size());

        assert_eq!(parsed_manifest.layers().len(), 1);
        for layer in parsed_manifest.layers() {
            let blob = entries
                .get(&format!("blobs/sha256/{}", layer.digest().digest()))
                .expect("layer blob present for every manifest layer descriptor");
            assert_eq!(sha256_hex(blob), layer.digest().digest());
            assert_eq!(blob.len() as u64, layer.size());
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Pulls a real base image, appends the marker layer, writes the OCI
    /// archive, structurally verifies it, and (if the environment allows)
    /// loads it into microsandbox. Ignored by default: needs network access
    /// and, for the final load, a working microsandbox runtime.
    #[tokio::test]
    #[ignore = "network + microsandbox runtime required"]
    async fn overlays_a_real_base() {
        let base_image = pull_base("docker.io/library/alpine:latest")
            .await
            .expect("pull docker.io/library/alpine:latest");

        let extra_files = vec![("etc/lilbox-overlay".to_string(), b"v1\n".to_vec(), 0o644u32)];
        let (layer_gzip, layer_digest, diff_id) = build_layer(&extra_files).unwrap();
        let (new_config_bytes, config_digest) =
            append_layer_to_config(&base_image.config_bytes, &diff_id).unwrap();

        let layer_descriptor = DescriptorBuilder::default()
            .media_type(MediaType::ImageLayerGzip)
            .digest(layer_digest.parse::<OciDigest>().unwrap())
            .size(layer_gzip.len() as u64)
            .build()
            .unwrap();
        let config_descriptor = DescriptorBuilder::default()
            .media_type(MediaType::ImageConfig)
            .digest(config_digest.parse::<OciDigest>().unwrap())
            .size(new_config_bytes.len() as u64)
            .build()
            .unwrap();

        let (new_manifest_bytes, manifest_digest) = append_layer_to_manifest(
            &base_image.manifest_bytes,
            layer_descriptor,
            config_descriptor,
        )
        .unwrap();

        let layer_hex = strip_sha256(&layer_digest).unwrap().to_string();
        let mut layer_blobs = base_image.layer_blobs;
        layer_blobs.push((layer_hex, layer_gzip));

        let tag = "lilbox-test/alpine-overlay:latest";
        let tmp = std::env::temp_dir().join(format!(
            "lilbox-overlay-real-test-{}-{}.tar",
            std::process::id(),
            now()
        ));

        write_oci_archive(
            &tmp,
            &new_config_bytes,
            &config_digest,
            &new_manifest_bytes,
            &manifest_digest,
            &layer_blobs,
            tag,
        )
        .expect("write OCI archive for real base");

        // Structural verification: every blob's sha256 matches its digest --
        // exactly what `Image::load` checks before it trusts the archive.
        {
            let file = std::fs::File::open(&tmp).unwrap();
            let mut archive = tar::Archive::new(file);
            let mut seen = HashMap::new();
            for entry in archive.entries().unwrap() {
                use std::io::Read;
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let mut data = Vec::new();
                entry.read_to_end(&mut data).unwrap();
                seen.insert(path, data);
            }
            for (hex, _) in &layer_blobs {
                let blob = seen
                    .get(&format!("blobs/sha256/{hex}"))
                    .expect("layer blob present in archive");
                assert_eq!(&sha256_hex(blob), hex);
            }
        }

        let load_result = microsandbox::Image::load(&tmp, vec![tag.to_string()]).await;
        let _ = std::fs::remove_file(&tmp);
        load_result.expect("microsandbox::Image::load should ingest the overlaid archive");
    }

    /// Builds a real tailnet-capable image from a real base: pulls alpine,
    /// downloads a real Tailscale release, and structurally verifies the
    /// extracted layer contains a `usr/local/bin/tailscaled` entry before
    /// exercising the full `tailscalify_image` pipeline (which itself calls
    /// `Image::load`). Ignored by default: needs network access for both the
    /// registry pull and the Tailscale download, and a working microsandbox
    /// runtime for the final load.
    #[tokio::test]
    #[ignore = "network + microsandbox runtime required"]
    async fn tailscalifies_alpine() {
        let host_arch = tailscale_arch_string(&Arch::default())
            .expect("host architecture should be amd64 or arm64");
        let tarball = download_tailscale(host_arch, TAILSCALE_VERSION)
            .await
            .expect("downloading a real tailscale release");
        let files = tailscale_layer_files(&tarball, TAILSCALE_VERSION, host_arch)
            .expect("extracting tailscale/tailscaled from the real release tarball");
        assert!(
            files
                .iter()
                .any(|(path, _, mode)| path == "usr/local/bin/tailscaled" && *mode == 0o755),
            "extracted layer should contain an executable usr/local/bin/tailscaled entry"
        );

        let tag = "lilbox/tailnet-test";
        tailscalify_image("docker.io/library/alpine:latest", tag)
            .await
            .expect("tailscalify_image should succeed against a real alpine base");
    }
}
