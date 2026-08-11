//! zed-crypt-lsp — makes armored gpg/age files transparently editable in Zed.
//!
//! Two modes in one binary:
//!
//! LSP mode (default, spawned by the zed-crypt Zed extension):
//!   didOpen with armored ciphertext → decrypt from disk → workspace/applyEdit
//!   swaps the buffer to plaintext. didSave (after the formatter has encrypted
//!   the buffer and Zed wrote it) → applyEdit swaps back to plaintext.
//!
//! Formatter mode (`zed-crypt-lsp --format <buffer_path>`, wired as Zed's
//! external formatter for the "Encrypted Armor" language):
//!   plaintext on stdin → armored ciphertext on stdout. Recipients are read
//!   from the ciphertext still on disk (gpg) or from recipients files (age).
//!   A non-zero exit makes Zed ABORT the save (verified in Zed's source:
//!   Editor::save runs `format_task.await?` before `save_buffers`), so a
//!   failure here can never leak plaintext to disk.
//!
//! Plaintext only ever exists in the editor buffer, this process, and pipes —
//! never as a file. The trade-offs (armor-only, permanently-dirty buffer,
//! `session.restore_unsaved_buffers` must be off) are documented in the README.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

const PGP_HEADER: &str = "-----BEGIN PGP MESSAGE-----";
const AGE_HEADER: &str = "-----BEGIN AGE ENCRYPTED FILE-----";

fn is_armor(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with(PGP_HEADER) || t.starts_with(AGE_HEADER)
}

fn fail(msg: &str) -> ! {
    eprintln!("zed-crypt-lsp: {msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--format") => {
            let path = args.get(2).unwrap_or_else(|| fail("--format requires the buffer path"));
            format_mode(path);
        }
        Some("--version") => println!("zed-crypt-lsp {}", env!("CARGO_PKG_VERSION")),
        _ => lsp_mode(),
    }
}

// ------------------------------------------------------------ crypto helpers

fn run(cmd: &str, args: &[&str], stdin_data: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(if stdin_data.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    if let Some(data) = stdin_data {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(data)
            .map_err(|e| format!("{cmd}: stdin write failed: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!("{cmd} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn gpg_recipients(path: &str) -> Result<Vec<String>, String> {
    let out = run("gpg", &["--batch", "--list-packets", path], None)?;
    let mut keys: Vec<String> = String::from_utf8_lossy(&out)
        .lines()
        .filter(|l| l.starts_with(":pubkey enc packet"))
        .filter_map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            toks.iter().position(|t| *t == "keyid").map(|i| toks[i + 1].to_string())
        })
        .collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        Err("no public-key recipients (symmetric gpg files are not supported)".into())
    } else {
        Ok(keys)
    }
}

fn age_identity() -> Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        std::env::var("AGE_IDENTITY").unwrap_or_default(),
        format!("{home}/.config/age/keys.txt"),
        format!("{home}/.config/sops/age/keys.txt"),
    ];
    candidates
        .iter()
        .find(|p| !p.is_empty() && std::path::Path::new(p).is_file())
        .cloned()
        .ok_or_else(|| "no age identity (set $AGE_IDENTITY or create ~/.config/age/keys.txt)".into())
}

fn age_recipients_file(target: &str) -> Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let beside = std::path::Path::new(target)
        .parent()
        .map(|d| d.join(".age-recipients").to_string_lossy().into_owned())
        .unwrap_or_default();
    let candidates = [
        std::env::var("AGE_RECIPIENTS").unwrap_or_default(),
        beside,
        format!("{home}/.config/age/recipients.txt"),
    ];
    candidates
        .iter()
        .find(|p| !p.is_empty() && std::path::Path::new(p).is_file())
        .cloned()
        .ok_or_else(|| {
            "no age recipients: age files do not record recipients; put them in \
             .age-recipients next to the file (or ~/.config/age/recipients.txt)"
                .into()
        })
}

fn decrypt_file(path: &str) -> Result<String, String> {
    let head = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let head_str = String::from_utf8_lossy(&head[..head.len().min(64)]).to_string();
    let plain = if head_str.trim_start().starts_with(PGP_HEADER) {
        run("gpg", &["--batch", "--quiet", "--decrypt", "--", path], None)?
    } else if head_str.trim_start().starts_with(AGE_HEADER) {
        let id = age_identity()?;
        run("age", &["--decrypt", "-i", &id, "--", path], None)?
    } else {
        return Err("not an armored gpg/age file".into());
    };
    String::from_utf8(plain).map_err(|_| "decrypted content is not UTF-8 text".into())
}

fn encrypt_for_target(plain: &[u8], target: &str) -> Result<Vec<u8>, String> {
    let head = std::fs::read(target)
        .map_err(|_| format!("{target} does not exist on disk yet; create it with gpg/age first"))?;
    let head_str = String::from_utf8_lossy(&head[..head.len().min(64)]).to_string();
    if head_str.trim_start().starts_with(PGP_HEADER) {
        let keys = gpg_recipients(target)?;
        let mut args: Vec<&str> = vec!["--batch", "--quiet", "--armor", "--output", "-"];
        for k in &keys {
            args.push("--recipient");
            args.push(k);
        }
        args.push("--encrypt");
        run("gpg", &args, Some(plain))
    } else if head_str.trim_start().starts_with(AGE_HEADER) {
        let rcpt = age_recipients_file(target)?;
        run("age", &["--encrypt", "--armor", "-R", &rcpt], Some(plain))
    } else {
        Err(format!("{target} is not an armored gpg/age file"))
    }
}

// ------------------------------------------------------------ formatter mode

fn format_mode(buffer_path: &str) -> ! {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap_or_else(|e| fail(&format!("stdin: {e}")));

    // Undo-guard: if the buffer already holds ciphertext (user undid past the
    // decrypt, or the open-time decrypt failed), pass it through unchanged
    // instead of encrypting ciphertext a second time.
    if is_armor(&String::from_utf8_lossy(&input)) {
        std::io::stdout().write_all(&input).ok();
        std::process::exit(0);
    }

    match encrypt_for_target(&input, buffer_path) {
        Ok(cipher) => {
            std::io::stdout().write_all(&cipher).ok();
            std::process::exit(0);
        }
        // Non-zero exit → Zed aborts the save; plaintext never reaches disk.
        Err(e) => fail(&e),
    }
}

// ------------------------------------------------------------------ LSP mode

struct Doc {
    current: String,
    plain: Option<String>,
}

struct Server {
    docs: HashMap<String, Doc>,
    next_id: i64,
    stdout: std::io::Stdout,
}

fn uri_to_path(uri: &str) -> String {
    let p = uri.strip_prefix("file://").unwrap_or(uri);
    // Minimal percent-decoding (spaces and non-ASCII in paths).
    let bytes = p.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&p[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// End position of `text` in LSP terms (UTF-16 code units on the last line).
fn end_position(text: &str) -> (usize, usize) {
    let mut line = 0usize;
    let mut last = text;
    for (i, l) in text.split('\n').enumerate() {
        line = i;
        last = l;
    }
    (line, last.encode_utf16().count())
}

impl Server {
    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        let out = self.stdout.lock();
        let mut w = std::io::BufWriter::new(out);
        write!(w, "Content-Length: {}\r\n\r\n{}", body.len(), body).ok();
        w.flush().ok();
    }

    fn respond(&mut self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn show_error(&mut self, msg: &str) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "window/showMessage",
            "params": {"type": 1, "message": format!("zed-crypt: {msg}")}
        }));
    }

    /// Replace the whole document with `new_text` via workspace/applyEdit.
    fn swap_buffer(&mut self, uri: &str, new_text: &str) {
        let (end_line, end_char) = {
            let doc = match self.docs.get(uri) {
                Some(d) => d,
                None => return,
            };
            end_position(&doc.current)
        };
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "workspace/applyEdit",
            "params": {"edit": {"changes": {uri: [{
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": end_line, "character": end_char}
                },
                "newText": new_text
            }]}}}
        }));
    }

    fn handle(&mut self, msg: Value) {
        let method = msg["method"].as_str().unwrap_or("").to_string();
        let id = msg["id"].clone();
        let params = &msg["params"];

        match method.as_str() {
            "initialize" => {
                let caps = json!({
                    "capabilities": {
                        "textDocumentSync": {
                            "openClose": true,
                            "change": 1,          // full-document sync
                            "save": {"includeText": false}
                        }
                    },
                    "serverInfo": {"name": "zed-crypt-lsp", "version": env!("CARGO_PKG_VERSION")}
                });
                self.respond(id, caps);
            }
            "shutdown" => self.respond(id, Value::Null),
            "exit" => std::process::exit(0),

            "textDocument/didOpen" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                let text = params["textDocument"]["text"].as_str().unwrap_or("").to_string();
                let armored = is_armor(&text);
                self.docs.insert(
                    uri.clone(),
                    Doc { current: text.clone(), plain: if armored { None } else { Some(text) } },
                );
                if armored {
                    match decrypt_file(&uri_to_path(&uri)) {
                        Ok(plain) => {
                            if let Some(d) = self.docs.get_mut(&uri) {
                                d.plain = Some(plain.clone());
                            }
                            self.swap_buffer(&uri, &plain);
                        }
                        Err(e) => self.show_error(&format!("decrypt failed: {e}")),
                    }
                }
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                if let Some(text) =
                    params["contentChanges"][0]["text"].as_str().map(str::to_string)
                {
                    if let Some(d) = self.docs.get_mut(&uri) {
                        d.current = text.clone();
                        if !is_armor(&text) {
                            d.plain = Some(text);
                        }
                    }
                }
            }
            "textDocument/didSave" => {
                // By now the formatter has turned the buffer into ciphertext and
                // Zed has written it. Swap the buffer back to plaintext. If the
                // buffer is not ciphertext (formatter skipped/failed → save was
                // aborted), do nothing.
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                let plain = self
                    .docs
                    .get(&uri)
                    .filter(|d| is_armor(&d.current))
                    .and_then(|d| d.plain.clone());
                if let Some(p) = plain {
                    self.swap_buffer(&uri, &p);
                }
            }
            "textDocument/didClose" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                self.docs.remove(uri);
            }

            _ => {
                // Unknown *requests* need an answer or the client hangs.
                if !id.is_null() && !method.is_empty() {
                    self.send(json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": format!("unhandled: {method}")}
                    }));
                }
                // Responses to our applyEdit requests (no method) are ignored.
            }
        }
    }
}

fn lsp_mode() -> ! {
    let mut server =
        Server { docs: HashMap::new(), next_id: 1000, stdout: std::io::stdout() };
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());

    loop {
        // Read headers.
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                std::process::exit(0); // client hung up
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        if content_length == 0 {
            continue;
        }
        let mut body = vec![0u8; content_length];
        if reader.read_exact(&mut body).is_err() {
            std::process::exit(0);
        }
        if let Ok(msg) = serde_json::from_slice::<Value>(&body) {
            server.handle(msg);
        }
    }
}
