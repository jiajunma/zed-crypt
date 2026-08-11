//! Zed extension shell for zed-crypt: registers the "Encrypted Armor" language
//! (*.asc, *.age) and points its language server at the zed-crypt-lsp binary.
//! All actual crypto lives in that binary; see ../lsp/.

use zed_extension_api::{self as zed, Result};

struct ZedCryptExtension;

impl zed::Extension for ZedCryptExtension {
    fn new() -> Self {
        ZedCryptExtension
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // PATH as the user's shell sees it; fall back to the install.sh target
        // for GUI launches with a minimal environment.
        let command = worktree.which("zed-crypt-lsp").unwrap_or_else(|| {
            let home = worktree
                .shell_env()
                .into_iter()
                .find(|(k, _)| k == "HOME")
                .map(|(_, v)| v)
                .unwrap_or_default();
            format!("{home}/.local/bin/zed-crypt-lsp")
        });
        Ok(zed::Command { command, args: vec![], env: Default::default() })
    }
}

zed::register_extension!(ZedCryptExtension);
