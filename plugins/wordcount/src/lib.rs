//! An example zcode plugin.
//!
//! A plugin is a WebAssembly module with three exports:
//!
//! - `zcode_alloc(len) -> ptr`: reserves `len` bytes the host can write into.
//! - `zcode_tools() -> packed`: JSON array of `{name, description, input_schema}`.
//! - `zcode_call(ptr, len) -> packed`: takes JSON `{tool, input}` and returns
//!   JSON `{text, is_error}`.
//!
//! `packed` is `(ptr << 32) | len` of a UTF-8 string in the module's memory.
//!
//! The only thing a plugin can ask of the host is `zcode.read_file`, and the
//! host answers only for paths the user granted to this plugin.

use serde_json::{Value, json};

#[link(wasm_import_module = "zcode")]
unsafe extern "C" {
    /// Returns packed JSON: `{"ok": "<contents>"}` or `{"error": "<why>"}`.
    fn read_file(path: *const u8, len: usize) -> u64;
}

#[unsafe(no_mangle)]
pub extern "C" fn zcode_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len.max(1));
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer
}

fn pack(text: String) -> u64 {
    let bytes = text.into_bytes().into_boxed_slice();
    let len = bytes.len() as u64;
    let pointer = Box::into_raw(bytes) as *mut u8 as u64;
    (pointer << 32) | len
}

/// Reads a string the host placed in our memory, or returned to us packed.
unsafe fn take(pointer: u64, len: u64) -> String {
    let bytes = unsafe { std::slice::from_raw_parts(pointer as *const u8, len as usize) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn host_read(path: &str) -> Result<String, String> {
    let packed = unsafe { read_file(path.as_ptr(), path.len()) };
    let reply = unsafe { take(packed >> 32, packed & 0xffff_ffff) };
    let reply: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    match reply["ok"].as_str() {
        Some(text) => Ok(text.to_string()),
        None => Err(reply["error"].as_str().unwrap_or("read failed").to_string()),
    }
}

fn stats(text: &str) -> String {
    format!(
        "{} lines, {} words, {} characters",
        text.lines().count(),
        text.split_whitespace().count(),
        text.chars().count()
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn zcode_tools() -> u64 {
    pack(
        json!([
            {
                "name": "text_stats",
                "description": "Count the lines, words and characters of a piece of text.",
                "input_schema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"]
                }
            },
            {
                "name": "file_stats",
                "description": "Count the lines, words and characters of a file.",
                "input_schema": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }
            }
        ])
        .to_string(),
    )
}

/// # Safety
/// `pointer` and `len` must describe bytes the host wrote into a buffer
/// obtained from `zcode_alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zcode_call(pointer: *const u8, len: usize) -> u64 {
    let request = unsafe { take(pointer as u64, len as u64) };
    let reply = |text: String, is_error: bool| {
        pack(json!({"text": text, "is_error": is_error}).to_string())
    };
    let Ok(request) = serde_json::from_str::<Value>(&request) else {
        return reply("invalid request".into(), true);
    };
    let input = &request["input"];
    match request["tool"].as_str() {
        Some("text_stats") => match input["text"].as_str() {
            Some(text) => reply(stats(text), false),
            None => reply("text is required".into(), true),
        },
        Some("file_stats") => match input["path"].as_str().map(host_read) {
            Some(Ok(text)) => reply(stats(&text), false),
            Some(Err(why)) => reply(format!("cannot read the file: {why}"), true),
            None => reply("path is required".into(), true),
        },
        _ => reply("unknown tool".into(), true),
    }
}
