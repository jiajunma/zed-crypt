# zed-crypt

Edit encrypted files from [Zed](https://zed.dev) — or any editor with a
blocking CLI — with **gpg**, **age**, and **sops** backends.

Successor to [zed-gpg](https://github.com/jiajunma/zed-gpg), fixing its two
real flaws: the plaintext no longer touches persistent storage (RAM-disk
scratch), and sealing is automatic (`zed --wait` detects the buffer closing).

```bash
zed-crypt edit ~/notes/diary.tex.gpg   # decrypt, open in Zed; every save
                                       # re-encrypts; closing the tab seals
zed-crypt status                       # what's open right now
zed-crypt backends                     # which backends this machine can use
zed-crypt close FILE                   # manual seal (normally not needed)
```

## Why not a Zed extension?

Zed's extension API (`zed_extension_api`) exposes language servers, slash
commands, MCP servers, debug adapters and docs indexing — **no file I/O hooks,
no virtual filesystem**. Zed reads raw bytes into the buffer before any
extension code could run, so "decrypt on open" cannot be implemented inside
the editor. (An LSP `workspace/applyEdit` trick can fake the open half, but on
save the plaintext would be written to the *original* file path before
re-encryption — strictly worse than doing this outside the editor.)

So the logic lives outside, where it also happens to work with any editor.

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
