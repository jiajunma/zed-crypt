//! Zed extension shell for zed-crypt: registers the "Encrypted Armor" language
//! (*.asc, *.age, *.gpg, *.pgp) and points its language server at the
//! zed-crypt-lsp binary. All actual crypto lives in that binary; see ../lsp/.
//!
//! Binary resolution order:
//!   1. `zed-crypt-lsp` on the user's shell PATH (a local build always wins)
//!   2. a previously downloaded copy in the extension's work directory
//!   3. freshly downloaded from this repo's GitHub releases

use zed_extension_api::{self as zed, LanguageServerId, Result};

struct ZedCryptExtension {
    cached_binary_path: Option<String>,
}

impl ZedCryptExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        if let Some(path) = worktree.which("zed-crypt-lsp") {
            return Ok(path);
        }

        if let Some(path) = &self.cached_binary_path {
            if std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
                return Ok(path.clone());
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = zed::latest_github_release(
            "jiajunma/zed-crypt",
            zed::GithubReleaseOptions { require_assets: true, pre_release: false },
        )?;

        // Must match the archive names produced by .github/workflows/release.yml.
        let (os, arch) = zed::current_platform();
        let target = match (os, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "aarch64-apple-darwin",
            (zed::Os::Mac, zed::Architecture::X8664) => "x86_64-apple-darwin",
            (zed::Os::Linux, zed::Architecture::Aarch64) => "aarch64-unknown-linux-musl",
            (zed::Os::Linux, zed::Architecture::X8664) => "x86_64-unknown-linux-musl",
            _ => {
                return Err(
                    "no prebuilt zed-crypt-lsp for this platform; build it from source \
                     (cd lsp && cargo build --release) and put it on PATH"
                        .into(),
                )
            }
        };
        let asset_name = format!("zed-crypt-lsp-{target}.tar.gz");
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| format!("release {} has no asset {asset_name}", release.version))?;

        let version_dir = format!("zed-crypt-lsp-{}", release.version);
        let binary_path = format!("{version_dir}/zed-crypt-lsp");

        if !std::fs::metadata(&binary_path).is_ok_and(|m| m.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(&asset.download_url, &version_dir, zed::DownloadedFileType::GzipTar)?;
            zed::make_file_executable(&binary_path)?;

            // Drop stale versions so the work dir doesn't accumulate binaries.
            if let Ok(entries) = std::fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with("zed-crypt-lsp-") && name != version_dir {
                        std::fs::remove_dir_all(entry.path()).ok();
                    }
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl zed::Extension for ZedCryptExtension {
    fn new() -> Self {
        ZedCryptExtension { cached_binary_path: None }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = self.language_server_binary_path(language_server_id, worktree)?;
        Ok(zed::Command { command, args: vec![], env: Default::default() })
    }
}

zed::register_extension!(ZedCryptExtension);
