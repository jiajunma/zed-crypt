# zed-crypt

Edit encrypted files from [Zed](https://zed.dev) with **gpg**, **age**, and
**sops** backends — two complementary modes:

1. **Extension mode** (`extension/` + `lsp/`): open an armored `.asc`/`.age`
   file in Zed and it just decrypts; saving re-encrypts. Plaintext exists
   **only in memory** — vim-gnupg-grade. Armored files only.
2. **Watcher mode** (`zed-crypt` script): works with *any* file format
   (including binary `.gpg`) and any editor with a blocking CLI. Plaintext
   lives on a RAM disk, never on persistent storage.

Successor to [zed-gpg](https://github.com/jiajunma/zed-gpg).

```bash
# Watcher mode
zed-crypt edit ~/notes/diary.tex.gpg   # decrypt, open in Zed; every save
                                       # re-encrypts; closing the tab seals
zed-crypt status                       # what's open right now
zed-crypt backends                     # which backends this machine can use
zed-crypt close FILE                   # manual seal (normally not needed)

# Extension mode: no commands — just open the .asc/.age file in Zed.
```

## Extension mode

Zed's extension API has no file I/O hooks, but two mechanisms compose into
exactly the pair vim-gnupg gets from `BufReadPost`/`BufWritePre`:

- **Read side**: the extension registers an "Encrypted Armor" language for
  `.asc`/`.age` and starts `zed-crypt-lsp`. On `didOpen` the server decrypts
  the file from disk and swaps the buffer to plaintext via
  `workspace/applyEdit`.
- **Write side**: the language's *external formatter* is
  `zed-crypt-lsp --format {buffer_path}` — on save Zed pipes the plaintext
  buffer through it and writes only the returned ciphertext. After the save,
  `didSave` triggers another `applyEdit` swapping the buffer back to
  plaintext.

**Failure-safety** (verified in Zed's source, `crates/editor/src/items.rs`):
`Editor::save` awaits the format task with `?` *before* `save_buffers`, and an
external formatter's non-zero exit is a hard error — so if encryption fails,
**the save is aborted and plaintext never reaches disk**. The formatter also
passes armored input through unchanged, so undoing past the decrypt and
saving cannot double-encrypt.

### Extension setup

```bash
cd lsp && cargo build --release
install -m 755 target/release/zed-crypt-lsp ~/.local/bin/
```

Then in Zed: `zed: install dev extension` → pick the `extension/` directory,
and add to `settings.json`:

```json
{
  "languages": {
    "Encrypted Armor": {
      "formatter": {
        "external": {
          "command": "/absolute/path/to/.local/bin/zed-crypt-lsp",
          "arguments": ["--format", "{buffer_path}"]
        }
      },
      "format_on_save": "on",
      "remove_trailing_whitespace_on_save": false,
      "ensure_final_newline_on_save": false
    }
  },
  "session": { "restore_unsaved_buffers": false }
}
```

`restore_unsaved_buffers: false` matters: the decrypted buffer is permanently
dirty (the plaintext swap is an unsaved edit by design), and without this
setting Zed would persist dirty buffers — i.e. your **plaintext** — into its
local database on quit.

### Extension-mode trade-offs

- **Armored files only** (`gpg --armor` / `age --armor`); binary ciphertext
  cannot round-trip through a UTF-8 text buffer. Opening a binary `.gpg`
  shows an error pointing at `zed-crypt edit`, and the formatter refuses to
  save such buffers (which would otherwise corrupt the ciphertext). Convert
  once with `gpg -d f.gpg | gpg --armor -r <you> -e -o f.asc` for transparent
  editing.
- **A passphrase-protected key needs a GUI pinentry** — the LSP has no
  terminal, so a curses pinentry cannot appear. On macOS:
  `brew install pinentry-mac`, then put
  `pinentry-program /opt/homebrew/bin/pinentry-mac` in
  `~/.gnupg/gpg-agent.conf` and run `gpgconf --kill gpg-agent`.
- The buffer is always dirty, so closing the tab prompts to save. "Save" is
  safe (encrypts); "Don't save" is also safe (disk already has ciphertext).
- Don't enable autosave for this language — each save produces fresh
  ciphertext and re-dirties the buffer, which loops.
- New files must be created with `gpg`/`age` once before editing (the
  formatter reads recipients from the existing ciphertext on disk).
- No syntax highlighting for the inner content (the language is "Encrypted
  Armor", not LaTeX/Markdown).

## How it works

1. **Backend detection** by extension and content: `.gpg/.pgp/.asc` → gpg,
   `.age` → age, yaml/json/env/ini containing sops metadata → sops.
2. **Scratch on a RAM disk.** macOS: a 64 MB HFS+ RAM disk is created on
   demand (`hdiutil attach ram://`) and detached when the last file is sealed;
   Linux: `/dev/shm`. The plaintext never lands on SSD/APFS. macOS encrypts
   swap by default, so paged-out RAM-disk memory is covered. If no RAM disk
   can be created it falls back to `$TMPDIR` **with a warning** — that
   fallback has the plaintext-on-disk weakness the RAM disk exists to avoid.
   The crypto extension is stripped from the scratch name, so `slides.tex.gpg`
   opens with LaTeX support.
3. **Re-encrypt on save.** A watcher polls the scratch file's mtime once a
   second; on change it encrypts to a temp file next to the target and `mv`s
   it into place, so an interrupted write can never truncate your ciphertext.
4. **Seal on close.** The session runs `zed --wait` (configurable), which
   blocks until the buffer is closed; then a final encrypt runs, the plaintext
   is shredded (real overwrite — the RAM disk is HFS+, not copy-on-write
   APFS), and the RAM disk is detached if nothing else is open. If the final
   encrypt fails the plaintext is deliberately **kept**, the failure is
   logged, and a macOS notification is posted.

## Per-backend notes

| Backend | Recipients for re-encryption | Notes |
|---|---|---|
| **gpg** | Read from the ciphertext itself (`--list-packets`), so shared files keep their recipients | Armored (`.asc`) files stay armored. Symmetric (`gpg -c`) files are **refused** — they can't be re-encrypted faithfully. |
| **age** | **Not recoverable from the file** (age headers hold one-shot stanzas). Taken from `.age-recipients` next to the file, else `~/.config/age/recipients.txt`, else the file named by `$AGE_RECIPIENTS` | Decryption identity: `$AGE_IDENTITY`, else `~/.config/age/keys.txt`, else `~/.config/sops/age/keys.txt`. |
| **sops** | Stored in the file's own sops metadata | Delegated entirely to `sops edit` with `EDITOR` set to the blocking editor — sops manages its own tmpfile and re-encryption. |

## Install

```bash
git clone https://github.com/jiajunma/zed-crypt.git
cd zed-crypt && ./install.sh
```

Installs to `~/.local/bin/zed-crypt`. Hard requirement: at least one of
`gpg`, `age`, `sops`. Optional: `gshred` (`brew install coreutils`) for
overwrite-on-delete in the `$TMPDIR` fallback path.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `ZED_CRYPT_EDITOR` | `zed --wait` | Editor command; must **block** until the file is closed (`code --wait`, `subl --wait`, `vim` all qualify) |
| `ZED_CRYPT_RAMDISK_MB` | `64` | RAM disk size on macOS |
| `ZED_CRYPT_NO_RAMDISK` | unset | Set to force the `$TMPDIR` fallback |
| `AGE_RECIPIENTS` | unset | Path to an age recipients file |
| `AGE_IDENTITY` | unset | Path to an age identity file |

## Security model, honestly

| | Plaintext location | Failure mode on encrypt error |
|---|---|---|
| Extension mode | Editor buffer + LSP process + pipes only | Save aborted; nothing written |
| Watcher mode | File on RAM disk (never SSD) | Plaintext kept on RAM disk, user notified |
| vim-gnupg / Emacs epa | Editor memory + pipes | Write aborted |

- Plaintext exists **in RAM** while a file is open (RAM disk + editor buffer).
  Anything running as your user can read it during that window. This equals
  what Emacs `epa` / vim-gnupg offer; the improvement over zed-gpg is that
  nothing is written to persistent storage.
- The RAM disk volume is a normal mount; the per-user directory under it is
  `0700` and scratch files are `0600`.
- If re-encryption ever fails, keeping your edits wins over secrecy: the
  plaintext stays (in RAM) and you are notified.
- If your threat model includes forensic recovery of RAM or swap on a
  compromised machine, no editor-side tool helps; use full-disk measures.

## Limitations

- Opening is explicit (`zed-crypt edit`), not transparent — see "Why not a
  Zed extension".
- Save-to-reencrypt latency is up to 1 s (mtime polling).
- A reboot or RAM-disk detach while a file is open loses **unsaved** edits
  (saved edits were already re-encrypted within a second of saving).
- gpg symmetric files are unsupported.

## License

MIT
