//! Host Zalo's `libzproto_wasm_bg.*.wasm` in-process via wasmtime so Sync V2
//! key generation and metadata/message decryption run without Node.js.
//!
//! The WASM module is wasm-bindgen output (raw core module, no JS glue at
//! runtime). This host re-implements the glue's semantics in Rust:
//!
//! - [`find_wasm_path`]: same locate order as `scripts/sync_v2_helper.js`
//!   (`HZ_ZPROTO_WASM` override → installed Zalo PC → `scripts/assets`);
//!   [`ZprotoHost::load`] then falls back to the copy packed inside Zalo PC's
//!   `app.asar` (Windows 26.11+ no longer unpacks it).
//!   The WASM blob is NEVER committed to the repo.
//! - wasm-bindgen object heap: slots 0..=3 are reserved (`undefined`, `null`,
//!   `true`, `false`); the rest hold host [`JsVal`]s. `c()` allocates,
//!   `a()` borrows, `p()` (take) frees back to the free list.
//! - The 39 `wbg.*` imports: `__wbindgen_*` heap/string/number/throw helpers,
//!   `Uint8Array`/buffer views over the exported `memory`, `__wbg_error_*`
//!   (string free), `__wbg_stack_*` (typed-array info), static accessors
//!   (`GLOBAL`/`GLOBAL_THIS`/`WINDOW`/`SELF` → one shared dummy object), and
//!   the `getrandom` shim path (`crypto` → `getRandomValues`, `process` →
//!   `versions`/`node` → `require("crypto").randomFillSync`) which funnels
//!   into OS randomness via `getrandom`.
//! - Memory helpers: pass bytes in (like glue's `passArray8ToWasm0`), read
//!   UTF-8 strings out, plus the `retptr` convention for calls returning
//!   `[ptr, len]` or `[ptr, len, err_ptr, err_flag]`.
//!
//! Public API mirrors the JS glue names used by `scripts/sync_v2_helper.js`:
//! `e` → [`ZprotoHost::generate_keypair`], `j` → [`ZprotoHost::decrypt_metadata`],
//! `i` → [`ZprotoHost::decrypt_message`], `O` → [`ZprotoHost::calculate_agreement`].

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Result, anyhow};
use wasmtime::{
    Caller, Engine, Extern, ExternType, Func, Instance, Linker, Memory, Module, Store, Val,
    ValType,
};

// ---------------------------------------------------------------------------
// WASM location (same order as scripts/sync_v2_helper.js findWasm)
// ---------------------------------------------------------------------------

/// Folder inside the data folder where a user can drop `libzproto_wasm_bg.*.wasm`.
pub const WASM_DATA_SUBDIR: &str = "zproto";

/// Locate `libzproto_wasm_bg.*.wasm` without bundling it in the repo.
/// Search order: `HZ_ZPROTO_WASM` absolute path, installed Zalo PC,
/// own app bundle `Contents/Resources`, dev copy under `scripts/assets`.
/// Returns `None` when not found (callers turn this into a clear error
/// telling the user to install Zalo PC or set the env var).
pub fn find_wasm_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HZ_ZPROTO_WASM") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    // Chính app bundle của mình: Contents/Resources/*.wasm (copy lúc cài đặt,
    // không commit vào repo theo chính sách HistoryCodec).
    if let Ok(exe) = std::env::current_exe() {
        // .../hz.app/Contents/MacOS/hz → .../Contents/Resources
        if let Some(macos) = exe.parent() {
            if let Some(contents) = macos.parent() {
                roots.push(contents.join("Resources"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Applications/Zalo.app/Contents/Resources/app.asar.unpacked"));
        if let Ok(home) = std::env::var("HOME") {
            roots.push(
                PathBuf::from(home).join("Applications/Zalo.app/Contents/Resources/app.asar.unpacked"),
            );
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Programs/Zalo"));
        }
        if let Ok(prog) = std::env::var("ProgramFiles") {
            roots.push(PathBuf::from(prog).join("Zalo"));
        }
    }
    // User-provided copy in the data folder (the process cwd, see `data_dir`). Linux has
    // no Zalo PC to borrow the module from, and the data folder is no longer the repo
    // checkout, so the relative `scripts/assets` below no longer finds the dev copy.
    roots.push(PathBuf::from(WASM_DATA_SUBDIR));
    #[cfg(target_os = "linux")]
    roots.push(PathBuf::from("/usr/share/hz"));
    roots.push(PathBuf::from("scripts/assets"));
    if let Ok(exe) = std::env::current_exe() {
        let mut d = exe.parent();
        for _ in 0..4 {
            if let Some(dir) = d {
                roots.push(dir.join("scripts/assets"));
                d = dir.parent();
            } else {
                break;
            }
        }
    }
    for root in roots {
        if let Some(hit) = glob_wasm(&root, 4) {
            return Some(hit);
        }
    }
    None
}

fn glob_wasm(dir: &Path, depth: u32) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if e.file_type().map(|t| t.is_file()).unwrap_or(false)
            && name.starts_with("libzproto_wasm_bg.")
            && name.ends_with(".wasm")
        {
            return Some(e.path());
        }
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) && name != "node_modules" {
            subdirs.push(e.path());
        }
    }
    if depth == 0 {
        return None;
    }
    for sub in subdirs {
        if let Some(hit) = glob_wasm(&sub, depth - 1) {
            return Some(hit);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// WASM inside Zalo PC's app.asar
// ---------------------------------------------------------------------------

/// Largest WASM accepted from an asar entry; the real module is ~0.3 MB.
const MAX_ASAR_WASM_BYTES: u64 = 16 * 1024 * 1024;

/// Largest asar header JSON accepted (Zalo PC 26.11's is ~0.8 MB).
const MAX_ASAR_HEADER_BYTES: u32 = 64 * 1024 * 1024;

/// `app.asar` archives of installed Zalo PC builds, newest version folder first.
/// Zalo PC 26.11+ on Windows packs `pc-dist/libs/libzproto_wasm_bg.*.wasm` inside
/// `resources/app.asar` (not `app.asar.unpacked`), so [`find_wasm_path`] cannot see it.
fn zalo_asar_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "windows")]
    {
        let mut installs: Vec<PathBuf> = Vec::new();
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            installs.push(PathBuf::from(local).join("Programs/Zalo"));
        }
        if let Ok(prog) = std::env::var("ProgramFiles") {
            installs.push(PathBuf::from(prog).join("Zalo"));
        }
        for install in installs {
            let mut versions: Vec<PathBuf> = std::fs::read_dir(&install)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("Zalo-"))
                })
                .collect();
            // "Zalo-26.11.11" style names; compare numerically so 26.10 < 26.9 cannot happen.
            versions.sort_by_key(|p| std::cmp::Reverse(zalo_version_key(p)));
            for v in versions {
                out.push(v.join("resources/app.asar"));
            }
            out.push(install.join("resources/app.asar"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        out.push(PathBuf::from("/Applications/Zalo.app/Contents/Resources/app.asar"));
        if let Ok(home) = std::env::var("HOME") {
            out.push(PathBuf::from(home).join("Applications/Zalo.app/Contents/Resources/app.asar"));
        }
    }
    out.retain(|p| p.is_file());
    out
}

/// Numeric version parts of a `Zalo-<a>.<b>.<c>` folder name, for newest-first ordering.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn zalo_version_key(path: &Path) -> Vec<u64> {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("Zalo-"))
        .map(|v| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect())
        .unwrap_or_default()
}

/// One file entry of an asar index: byte range relative to the data section.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AsarEntry {
    path: String,
    offset: u64,
    size: u64,
}

/// Parse the asar header and return `(data_section_start, libzproto entry)`.
///
/// Layout: `u32 4 | u32 header_pickle_size | u32 | u32 json_len | json`, with file data
/// starting at `8 + header_pickle_size`. Entries flagged `unpacked` live on disk instead.
fn read_asar_index(archive: &mut impl std::io::Read) -> Result<(u64, Option<AsarEntry>)> {
    let mut head = [0u8; 16];
    archive.read_exact(&mut head)?;
    let word = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    let (pickle_size, json_len) = (word(4), word(12));
    anyhow::ensure!(json_len > 0 && json_len <= MAX_ASAR_HEADER_BYTES, "asar header size {json_len} out of range");
    anyhow::ensure!(u64::from(json_len) + 8 <= u64::from(pickle_size), "asar header sizes inconsistent");
    let mut json = vec![0u8; json_len as usize];
    archive.read_exact(&mut json)?;
    let index: serde_json::Value = serde_json::from_slice(&json)?;
    Ok((8 + u64::from(pickle_size), find_asar_wasm_entry(&index)))
}

/// Depth-first search of an asar index for the packed `libzproto_wasm_bg.*.wasm`.
fn find_asar_wasm_entry(index: &serde_json::Value) -> Option<AsarEntry> {
    fn walk(node: &serde_json::Value, prefix: &str) -> Option<AsarEntry> {
        let files = node.get("files")?.as_object()?;
        for (name, child) in files {
            let path = format!("{prefix}/{name}");
            if child.get("files").is_some() {
                if let Some(hit) = walk(child, &path) {
                    return Some(hit);
                }
                continue;
            }
            let is_target = name.starts_with("libzproto_wasm_bg.") && name.ends_with(".wasm");
            let packed = !child.get("unpacked").and_then(serde_json::Value::as_bool).unwrap_or(false);
            if !is_target || !packed {
                continue;
            }
            let offset = match child.get("offset")? {
                serde_json::Value::String(s) => s.parse().ok()?,
                v => v.as_u64()?,
            };
            let size = child.get("size")?.as_u64()?;
            return Some(AsarEntry { path, offset, size });
        }
        None
    }
    walk(index, "")
}

/// Read the packed libzproto WASM out of one `app.asar`. `Ok(None)` when it has none.
fn read_wasm_from_asar(archive_path: &Path) -> Result<Option<(String, Vec<u8>)>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(archive_path)?;
    let (data_start, entry) = read_asar_index(&mut file)?;
    let Some(entry) = entry else {
        return Ok(None);
    };
    anyhow::ensure!(
        entry.size > 8 && entry.size <= MAX_ASAR_WASM_BYTES,
        "asar entry {} has implausible size {}",
        entry.path,
        entry.size
    );
    file.seek(SeekFrom::Start(data_start + entry.offset))?;
    let mut bytes = vec![0u8; entry.size as usize];
    file.read_exact(&mut bytes)?;
    anyhow::ensure!(bytes.starts_with(b"\0asm"), "asar entry {} is not a WASM module", entry.path);
    Ok(Some((entry.path, bytes)))
}

/// First libzproto WASM packed in an installed Zalo PC `app.asar`, with its origin label.
fn find_wasm_in_zalo_asar() -> Option<(String, Vec<u8>)> {
    for archive in zalo_asar_candidates() {
        match read_wasm_from_asar(&archive) {
            Ok(Some((entry, bytes))) => return Some((format!("{}:{entry}", archive.display()), bytes)),
            Ok(None) => {}
            Err(e) => crate::logger::log_sync(&format!("app.asar {}: {e}", archive.display())),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// wasm-bindgen object heap
// ---------------------------------------------------------------------------

/// Which host callable a [`JsVal::Func`] slot represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FuncKind {
    GetRandomValues,
    RandomFillSync,
    Require,
    Dead,
}

/// Host-side JS value. Only the shapes the glue touches are modeled;
/// everything exotic collapses to [`JsVal::Dummy`] (a shared plain object).
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // Bool/Dummy payloads giữ shape enum cho glue sau này.
enum JsVal {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    /// Uint8Array: owned bytes plus optional `(ptr, len)` view into WASM
    /// memory (`new with byteoffset/length`, `subarray`).
    U8 { bytes: Vec<u8>, view: Option<(u32, u32)> },
    /// Shared dummy for GLOBAL / process / versions / crypto / require etc.
    Dummy(&'static str),
    /// Callable marker for the getrandom shim dispatch.
    Func(FuncKind),
    Err(String),
}

/// Per-instance host state: the bindgen heap plus the `require("crypto")`
/// handle the `call`/`apply` dispatch returns.
struct HostState {
    heap: Vec<Option<JsVal>>,
    free: Vec<u32>,
    require_crypto_obj: u32,
}

impl HostState {
    fn new() -> Self {
        // Slots 0..=3 reserved: undefined / null / true / false.
        let mut heap: Vec<Option<JsVal>> = vec![
            Some(JsVal::Undefined),
            Some(JsVal::Null),
            Some(JsVal::Bool(true)),
            Some(JsVal::Bool(false)),
        ];
        heap.push(Some(JsVal::Dummy("global"))); // 4
        heap.push(Some(JsVal::Dummy("process"))); // 5
        heap.push(Some(JsVal::Dummy("versions"))); // 6
        heap.push(Some(JsVal::Dummy("node"))); // 7: truthy `process.versions.node`
        heap.push(Some(JsVal::Dummy("crypto"))); // 8
        heap.push(Some(JsVal::Dummy("require_crypto"))); // 9: require("crypto")
        heap.push(Some(JsVal::Func(FuncKind::GetRandomValues))); // 10
        heap.push(Some(JsVal::Func(FuncKind::RandomFillSync))); // 11
        heap.push(Some(JsVal::Func(FuncKind::Require))); // 12
        Self { heap, free: Vec::new(), require_crypto_obj: 9 }
    }
    fn alloc(&mut self, v: JsVal) -> u32 {
        if let Some(i) = self.free.pop() {
            self.heap[i as usize] = Some(v);
            return i;
        }
        let i = self.heap.len() as u32;
        self.heap.push(Some(v));
        i
    }
    fn get(&self, idx: u32) -> &JsVal {
        static UNDEF: JsVal = JsVal::Undefined;
        self.heap.get(idx as usize).and_then(|o| o.as_ref()).unwrap_or(&UNDEF)
    }
    fn take(&mut self, idx: u32) -> JsVal {
        if idx < 132 {
            return self.get(idx).clone();
        }
        if let Some(slot) = self.heap.get_mut(idx as usize) {
            if let Some(v) = slot.take() {
                self.free.push(idx);
                return v;
            }
        }
        JsVal::Undefined
    }
    fn clone_ref(&mut self, idx: u32) -> u32 {
        let v = self.get(idx).clone();
        self.alloc(v)
    }
}

// ---------------------------------------------------------------------------
// Import helpers (operate on a Caller)
// ---------------------------------------------------------------------------

fn mem_of(caller: &mut Caller<'_, HostState>) -> Memory {
    caller.get_export("memory").and_then(Extern::into_memory).expect("wasm exports memory")
}

fn read_bytes(caller: &Caller<'_, HostState>, memory: Memory, ptr: u32, len: u32) -> Vec<u8> {
    let data = memory.data(caller);
    let s = (ptr as usize).min(data.len());
    let e = s.saturating_add(len as usize).min(data.len());
    data[s..e].to_vec()
}

fn write_bytes(caller: &mut Caller<'_, HostState>, memory: Memory, ptr: u32, bytes: &[u8]) {
    let data = memory.data_mut(caller);
    let s = (ptr as usize).min(data.len());
    let e = (s + bytes.len()).min(data.len());
    data[s..e].copy_from_slice(&bytes[..e - s]);
}

/// Resolve a heap `U8` to concrete bytes (views read back from WASM memory).
fn u8_bytes(caller: &Caller<'_, HostState>, memory: Memory, idx: u32) -> Vec<u8> {
    match caller.data().get(idx) {
        JsVal::U8 { bytes, view: Some((ptr, len)) } if bytes.is_empty() => {
            read_bytes(caller, memory, *ptr, *len)
        }
        JsVal::U8 { bytes, .. } => bytes.clone(),
        _ => Vec::new(),
    }
}

/// Fill `dest` with OS randomness (the `getRandomValues`/`randomFillSync`
/// funnel). Never mocked, always real entropy.
fn fill_random(dest: &mut [u8]) -> anyhow::Result<()> {
    getrandom::fill(dest).map_err(|e| anyhow!("getrandom failed: {e}"))
}

/// `new Function(source)` in the getrandom shim: the source string decides
/// which branch the shim takes. The Zalo glue's `require` path needs the
/// exact prefix `"return require"` to select node; anything else is dead.
fn classify_new_function(src: &str) -> FuncKind {
    if src.starts_with("return require") { FuncKind::Require } else { FuncKind::Dead }
}

// ---------------------------------------------------------------------------
// Linker: dynamic wbg import resolver (resilient to wasm-bindgen hash changes)
// ---------------------------------------------------------------------------

/// Strip a trailing `_<16 hex digits>` `wasm-bindgen` symbol hash if present.
fn strip_wasm_bindgen_hash(name: &str) -> &str {
    if let Some((prefix, suffix)) = name.rsplit_once('_') {
        if suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return prefix;
        }
    }
    name
}

fn new_u8_array_from_src(caller: &mut Caller<'_, HostState>, src: u32) -> u32 {
    match caller.data().get(src).clone() {
        JsVal::U8 { view: Some(v), .. } => {
            caller.data_mut().alloc(JsVal::U8 { bytes: Vec::new(), view: Some(v) })
        }
        JsVal::Dummy("arraybuffer") => {
            let m = mem_of(caller);
            let view = (0u32, m.data_size(&*caller) as u32);
            caller.data_mut().alloc(JsVal::U8 { bytes: Vec::new(), view: Some(view) })
        }
        _ => {
            let m = mem_of(caller);
            let b = u8_bytes(caller, m, src);
            caller.data_mut().alloc(JsVal::U8 { bytes: b, view: None })
        }
    }
}

fn wbg_set_u8_or_err(caller: &mut Caller<'_, HostState>, obj: u32, _k_or_src: u32, v_or_off: u32) {
    match caller.data().get(obj).clone() {
        JsVal::U8 { view: Some((ptr, _)), .. } => {
            let m = mem_of(caller);
            let src_bytes = u8_bytes(caller, m, _k_or_src);
            write_bytes(caller, m, ptr + v_or_off, &src_bytes);
        }
        JsVal::U8 { view: None, .. } => {
            let m = mem_of(caller);
            let src_bytes = u8_bytes(caller, m, _k_or_src);
            if let Some(slot) = caller.data_mut().heap.get_mut(obj as usize) {
                if let Some(JsVal::U8 { bytes, .. }) = slot.as_mut() {
                    let need = v_or_off as usize + src_bytes.len();
                    if bytes.len() < need {
                        bytes.resize(need, 0);
                    }
                    bytes[v_or_off as usize..v_or_off as usize + src_bytes.len()]
                        .copy_from_slice(&src_bytes);
                }
            }
        }
        JsVal::Err(_) => {
            if let JsVal::Str(s) = caller.data().get(v_or_off).clone() {
                if let Some(JsVal::Err(e)) = caller
                    .data_mut()
                    .heap
                    .get_mut(obj as usize)
                    .and_then(|o| o.as_mut())
                {
                    *e = s;
                }
            }
        }
        _ => {}
    }
}

fn add_imports_for_module(linker: &mut Linker<HostState>, module: &Module) -> anyhow::Result<()> {
    use ValType::{F64, I32};

    let mut resolved = 0usize;
    for imp in module.imports() {
        if imp.module() != "wbg" {
            continue;
        }
        let ExternType::Func(fty) = imp.ty() else {
            anyhow::bail!("unsupported non-function wbg import: {}", imp.name());
        };
        let raw_name = imp.name();
        let base = strip_wasm_bindgen_hash(raw_name).to_ascii_lowercase();
        let params: Vec<ValType> = fty.params().collect();
        let results: Vec<ValType> = fty.results().collect();

        match (base.as_str(), params.as_slice(), results.as_slice()) {
            // -- heap & core wasm-bindgen helpers ----------------------------
            ("__wbindgen_object_drop_ref", [I32], []) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, idx: u32| {
                    caller.data_mut().take(idx);
                })?;
            }
            ("__wbindgen_object_clone_ref", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, idx: u32| -> u32 {
                    caller.data_mut().clone_ref(idx)
                })?;
            }
            ("__wbindgen_string_new", [I32, I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
                    let m = mem_of(&mut caller);
                    let s = String::from_utf8_lossy(&read_bytes(&caller, m, ptr, len)).into_owned();
                    caller.data_mut().alloc(JsVal::Str(s))
                })?;
            }
            ("__wbindgen_number_new", [F64], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, v: f64| -> u32 {
                    caller.data_mut().alloc(JsVal::Num(v))
                })?;
            }
            ("__wbindgen_memory", [], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>| -> u32 {
                    let m = mem_of(&mut caller);
                    let view = (0u32, m.data_size(&caller) as u32);
                    caller.data_mut().alloc(JsVal::U8 { bytes: Vec::new(), view: Some(view) })
                })?;
            }
            ("__wbindgen_throw", [I32, I32], []) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> Result<(), wasmtime::Error> {
                    let m = caller.get_export("memory").and_then(Extern::into_memory).expect("memory");
                    let s = String::from_utf8_lossy(&read_bytes(&caller, m, ptr, len)).into_owned();
                    Err(wasmtime::Error::msg(format!("wasm throw: {s}")))
                })?;
            }
            ("__wbindgen_is_object", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |caller: Caller<'_, HostState>, idx: u32| -> i32 {
                    matches!(caller.data().get(idx), JsVal::Dummy(_) | JsVal::U8 { .. } | JsVal::Func(_) | JsVal::Err(_)) as i32
                })?;
            }
            ("__wbindgen_is_string", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |caller: Caller<'_, HostState>, idx: u32| -> i32 {
                    matches!(caller.data().get(idx), JsVal::Str(_)) as i32
                })?;
            }
            ("__wbindgen_is_function", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |caller: Caller<'_, HostState>, idx: u32| -> i32 {
                    matches!(caller.data().get(idx), JsVal::Func(_)) as i32
                })?;
            }
            ("__wbindgen_is_undefined", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |caller: Caller<'_, HostState>, idx: u32| -> i32 {
                    matches!(caller.data().get(idx), JsVal::Undefined) as i32
                })?;
            }

            // -- Array / Object / Error / Uint8Array -------------------------
            ("__wbg_new", [], [I32]) => {
                let kind = raw_name.to_string();
                linker.func_wrap("wbg", raw_name, move |mut caller: Caller<'_, HostState>| -> u32 {
                    let val = if kind == "__wbg_new_78feb108b6472713" {
                        JsVal::Dummy("array")
                    } else if kind == "__wbg_new_405e22f390576ce2" {
                        JsVal::Dummy("object")
                    } else {
                        JsVal::Err(String::new())
                    };
                    caller.data_mut().alloc(val)
                })?;
            }
            ("__wbg_new", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, src: u32| -> u32 {
                    new_u8_array_from_src(&mut caller, src)
                })?;
            }
            ("__wbg_push", [I32, I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |_caller: Caller<'_, HostState>, _arr: u32, _v: u32| -> u32 { 0 })?;
            }
            ("__wbg_set", [I32, I32, I32], []) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, obj: u32, k: u32, v: u32| {
                    wbg_set_u8_or_err(&mut caller, obj, k, v);
                })?;
            }
            ("__wbg_error", [I32, I32], []) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _ptr: u32, _len: u32| {
                    let _ = mem_of(&mut caller);
                })?;
            }
            ("__wbg_stack", [I32, I32], []) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, retptr: u32, _err_idx: u32| {
                    let m = mem_of(&mut caller);
                    write_bytes(&mut caller, m, retptr, &[0u8; 8]);
                })?;
            }
            ("__wbg_newwithlength", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, len: u32| -> u32 {
                    caller.data_mut().alloc(JsVal::U8 { bytes: vec![0; len as usize], view: None })
                })?;
            }
            ("__wbg_newwithbyteoffsetandlength", [I32, I32, I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _buf: u32, off: u32, len: u32| -> u32 {
                    caller.data_mut().alloc(JsVal::U8 { bytes: Vec::new(), view: Some((off, len)) })
                })?;
            }
            ("__wbg_subarray", [I32, I32, I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, arr: u32, s: u32, e: u32| -> u32 {
                    let owned = caller.data().get(arr).clone();
                    match owned {
                        JsVal::U8 { view: Some((ptr, _)), .. } => {
                            caller.data_mut().alloc(JsVal::U8 { bytes: Vec::new(), view: Some((ptr + s, e.saturating_sub(s))) })
                        }
                        JsVal::U8 { bytes, view: None } => {
                            let sub = bytes.get(s as usize..e as usize).unwrap_or(&[]).to_vec();
                            caller.data_mut().alloc(JsVal::U8 { bytes: sub, view: None })
                        }
                        _ => caller.data_mut().alloc(JsVal::U8 { bytes: Vec::new(), view: None }),
                    }
                })?;
            }
            ("__wbg_buffer", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _arr: u32| -> u32 {
                    caller.data_mut().alloc(JsVal::Dummy("arraybuffer"))
                })?;
            }

            // -- getrandom shim ----------------------------------------------
            ("__wbg_static_accessor_global"
            | "__wbg_static_accessor_global_this"
            | "__wbg_static_accessor_window"
            | "__wbg_static_accessor_self", [], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>| -> u32 {
                    caller.data_mut().alloc(JsVal::Dummy("global"))
                })?;
            }
            ("__wbg_crypto", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _o: u32| -> u32 {
                    caller.data_mut().alloc(JsVal::Dummy("crypto"))
                })?;
            }
            ("__wbg_mscrypto", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |_caller: Caller<'_, HostState>, _o: u32| -> u32 { 0 })?;
            }
            ("__wbg_process", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _o: u32| -> u32 {
                    caller.data_mut().alloc(JsVal::Dummy("process"))
                })?;
            }
            ("__wbg_versions", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _o: u32| -> u32 {
                    caller.data_mut().alloc(JsVal::Dummy("versions"))
                })?;
            }
            ("__wbg_node", [I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _o: u32| -> u32 {
                    caller.data_mut().alloc(JsVal::Dummy("node"))
                })?;
            }
            ("__wbg_require", [], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>| -> u32 {
                    caller.data_mut().alloc(JsVal::Func(FuncKind::Require))
                })?;
            }
            ("__wbg_newnoargs", [I32, I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| -> u32 {
                    let m = mem_of(&mut caller);
                    let src = String::from_utf8_lossy(&read_bytes(&caller, m, ptr, len)).into_owned();
                    let kind = classify_new_function(&src);
                    caller.data_mut().alloc(JsVal::Func(kind))
                })?;
            }
            ("__wbg_call", [I32, I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, f: u32, this: u32| -> Result<u32, wasmtime::Error> {
                    call0(&mut caller, f, this).map_err(|e| wasmtime::Error::msg(e.to_string()))
                })?;
            }
            ("__wbg_call" | "__wbg_apply", [I32, I32, I32], [I32]) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, f: u32, this: u32, arg: u32| -> Result<u32, wasmtime::Error> {
                    call1(&mut caller, f, this, arg).map_err(|e| wasmtime::Error::msg(e.to_string()))
                })?;
            }
            ("__wbg_getrandomvalues" | "__wbg_randomfillsync", [I32, I32], []) => {
                linker.func_wrap("wbg", raw_name, |mut caller: Caller<'_, HostState>, _crypto: u32, arr: u32| -> Result<(), wasmtime::Error> {
                    fill_u8(&mut caller, arr).map_err(|e| wasmtime::Error::msg(e.to_string()))
                })?;
            }
            _ => {
                let msg = format!(
                    "unsupported wbg import {raw_name} (base={base}, params={params:?}, results={results:?})"
                );
                crate::logger::log_sync_event("zproto::import", &msg, true);
                anyhow::bail!("{msg}");
            }
        }
        resolved += 1;
    }

    crate::logger::log_sync_event(
        "zproto::load",
        &format!("dynamically linked {resolved} wbg imports"),
        false,
    );
    Ok(())
}

/// Zero-arg call dispatch for the getrandom shim.
fn call0(caller: &mut Caller<'_, HostState>, f: u32, _this: u32) -> anyhow::Result<u32> {
    match caller.data().get(f) {
        JsVal::Func(FuncKind::Require) => Ok(caller.data().require_crypto_obj),
        _ => Ok(0),
    }
}

/// One-arg call dispatch for the getrandom shim.
fn call1(caller: &mut Caller<'_, HostState>, f: u32, _this: u32, arg: u32) -> anyhow::Result<u32> {
    match caller.data().get(f) {
        JsVal::Func(FuncKind::Require) => Ok(caller.data().require_crypto_obj),
        // `randomFillSync(buf)`: fill through, return undefined.
        JsVal::Func(FuncKind::RandomFillSync) => {
            fill_u8(caller, arg)?;
            Ok(0)
        }
        // `getRandomValues(view)`: fill through, return the view.
        JsVal::Func(FuncKind::GetRandomValues) => {
            fill_u8(caller, arg)?;
            Ok(arg)
        }
        _ => Ok(0),
    }
}

/// Fill the heap `U8` at `idx` with OS randomness, writing through views.
fn fill_u8(caller: &mut Caller<'_, HostState>, idx: u32) -> anyhow::Result<()> {
    let m = mem_of(caller);
    let owned = caller.data().get(idx).clone();
    match owned {
        JsVal::U8 { view: Some((ptr, len)), .. } => {
            let mut buf = vec![0u8; len as usize];
            fill_random(&mut buf).map_err(|e| wasmtime::Error::msg(e.to_string()))?;
            write_bytes(caller, m, ptr, &buf);
        }
        JsVal::U8 { view: None, .. } => {
            let mut buf = u8_bytes(caller, m, idx);
            fill_random(&mut buf).map_err(|e| wasmtime::Error::msg(e.to_string()))?;
            if let Some(slot) = caller.data_mut().heap.get_mut(idx as usize) {
                if let Some(JsVal::U8 { bytes, .. }) = slot.as_mut() {
                    *bytes = buf;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Instance + memory-call helpers
// ---------------------------------------------------------------------------

static ENGINE: LazyLock<Engine> = LazyLock::new(Engine::default);

struct Inner {
    store: Store<HostState>,
    instance: Instance,
    memory: Memory,
    malloc: Func,
    free: Func,
}

pub struct ZprotoHost {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for ZprotoHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZprotoHost").finish_non_exhaustive()
    }
}

impl ZprotoHost {
    /// Load the WASM from disk ([`find_wasm_path`]) or, failing that, from inside an
    /// installed Zalo PC's `app.asar`.
    ///
    /// # Errors
    /// Returns a user-facing error when no Zalo PC / WASM is available, or when the
    /// module cannot be instantiated.
    pub fn load() -> Result<Self> {
        if let Some(path) = find_wasm_path() {
            return Self::load_from(&path);
        }
        if let Some((origin, bytes)) = find_wasm_in_zalo_asar() {
            crate::logger::log_sync(&format!("libzproto WASM từ {origin}"));
            return Self::load_from_bytes(&bytes);
        }
        let data_hint = std::env::current_dir()
            .map(|d| d.join(WASM_DATA_SUBDIR).display().to_string())
            .unwrap_or_else(|_| WASM_DATA_SUBDIR.to_string());
        Err(anyhow!(
            "Không tìm thấy Zalo PC trên máy này (cần file libzproto_wasm_bg.*.wasm của Zalo PC). \
             Cài Zalo PC, chép file WASM vào {data_hint}, hoặc đặt HZ_ZPROTO_WASM trỏ tới file WASM."
        ))
    }

    /// Load the WASM module from a file path.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read or instantiated.
    pub fn load_from(path: &Path) -> Result<Self> {
        Self::load_from_bytes(&std::fs::read(path)?)
    }

    /// Instantiate the WASM module from raw bytes.
    ///
    /// # Errors
    /// Returns an error when the bytes are not a compatible libzproto module.
    pub fn load_from_bytes(bytes: &[u8]) -> Result<Self> {
        let module = Module::new(&ENGINE, bytes)?;
        let mut linker = Linker::<HostState>::new(&ENGINE);
        add_imports_for_module(&mut linker, &module)?;
        let mut store = Store::new(&ENGINE, HostState::new());
        let instance = linker.instantiate(&mut store, &module)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow!("wasm has no memory export"))?;
        let need_any = |names: &[&str], store: &mut Store<HostState>| -> Result<Func> {
            for n in names {
                if let Some(f) = instance.get_func(&mut *store, n) {
                    return Ok(f);
                }
            }
            anyhow::bail!("wasm missing export {}", names.join(" / "))
        };
        let malloc = need_any(&["__wbindgen_export_2", "__wbindgen_malloc"], &mut store)?;
        let free = need_any(&["__wbindgen_export_1", "__wbindgen_free"], &mut store)?;
        Ok(Self { inner: Mutex::new(Inner { store, instance, memory, malloc, free }) })
    }

    fn with<T>(&self, f: impl FnOnce(&mut Store<HostState>, &Instance, Memory, &Func, &Func) -> Result<T>) -> Result<T> {
        let mut inner = self.inner.lock().map_err(|_| anyhow!("zproto host lock poisoned"))?;
        let Inner { store, instance, memory, malloc, free } = &mut *inner;
        f(store, instance, *memory, malloc, free)
    }

    /// Copy bytes into WASM memory, return `(ptr, len)` (glue's `I()`).
    fn pass_bytes(
        store: &mut Store<HostState>,
        memory: Memory,
        malloc: &Func,
        bytes: &[u8],
    ) -> Result<(u32, u32)> {
        let mut out = [Val::I32(0)];
        malloc.call(&mut *store, &[Val::I32(bytes.len() as i32), Val::I32(1)], &mut out)?;
        let ptr = match out[0] {
            Val::I32(p) => p as u32,
            _ => anyhow::bail!("malloc returned non-i32"),
        };
        let data = memory.data_mut(store);
        let s = ptr as usize;
        anyhow::ensure!(s + bytes.len() <= data.len(), "wasm memory too small");
        data[s..s + bytes.len()].copy_from_slice(bytes);
        Ok((ptr, bytes.len() as u32))
    }

    fn read_str(store: &Store<HostState>, memory: Memory, ptr: u32, len: u32) -> String {
        let data = memory.data(store);
        let s = (ptr as usize).min(data.len());
        let e = s.saturating_add(len as usize).min(data.len());
        String::from_utf8_lossy(&data[s..e]).into_owned()
    }

    fn read_mem(store: &Store<HostState>, memory: Memory, ptr: u32, len: u32) -> Vec<u8> {
        let data = memory.data(store);
        let s = (ptr as usize).min(data.len());
        let e = s.saturating_add(len as usize).min(data.len());
        data[s..e].to_vec()
    }

    fn read_u32(store: &Store<HostState>, memory: Memory, addr: u32) -> anyhow::Result<u32> {
        let b = Self::read_mem(store, memory, addr, 4);
        anyhow::ensure!(b.len() == 4, "retptr out of bounds");
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn free_mem(store: &mut Store<HostState>, free: &Func, ptr: u32, len: u32) -> anyhow::Result<()> {
        free.call(store, &[Val::I32(ptr as i32), Val::I32(len as i32), Val::I32(1)], &mut [])?;
        Ok(())
    }

    /// Take heap object `idx` (a `U8` view/copy or `Str`) into bytes.
    fn take_bytes(store: &mut Store<HostState>, memory: Memory, idx: u32) -> Vec<u8> {
        let owned: JsVal = store.data_mut().take(idx);
        match owned {
            JsVal::U8 { bytes, view: Some((ptr, len)) } if bytes.is_empty() => {
                Self::read_mem(store, memory, ptr, len)
            }
            JsVal::U8 { bytes, .. } => bytes,
            JsVal::Str(s) => s.into_bytes(),
            _ => Vec::new(),
        }
    }

    fn call_raw(&self, name: &str, args: &[Val]) -> anyhow::Result<()> {
        self.with(|store, instance, _, _, _| {
            let f = instance.get_func(&mut *store, name).ok_or_else(|| anyhow!("wasm missing export {name}"))?;
            f.call(&mut *store, args, &mut [])?;
            Ok(())
        })
    }

    /// Reserve 16 scratch bytes via malloc, run `f(retptr)`, free after.
    /// Equivalent to the glue's `__wbindgen_add_to_stack_pointer(-16)` retptr
    /// slot for reading `[ptr, len]` / `[ptr, len, err]` returns.
    fn with_retptr<T>(&self, f: impl FnOnce(&Self, u32) -> Result<T>) -> Result<T> {
        let rp = self.with(|store, _, memory, malloc, _| Self::pass_bytes(store, memory, malloc, &[0u8; 16]))?;
        let rp = rp.0;
        let out = f(self, rp);
        let _ = self.with(|store, _, _, _, free| Self::free_mem(store, free, rp, 16));
        out
    }

    fn free_keypair(&self, ptr: u32) -> anyhow::Result<()> {
        self.call_raw("__wbg_zprotoed25519keypair_free", &[Val::I32(ptr as i32), Val::I32(0)])
    }

    fn free_meta(&self, ptr: u32) -> anyhow::Result<()> {
        self.call_raw("__wbg_zprotosyncmetadatadecrypted_free", &[Val::I32(ptr as i32), Val::I32(0)])
    }

    fn free_msg(&self, ptr: u32) -> anyhow::Result<()> {
        self.call_raw("__wbg_zprotosyncmessagedecrypted_free", &[Val::I32(ptr as i32), Val::I32(0)])
    }

    /// `e()` → `(public_key[33], private_key[32])`. Mirrors glue `k()`.
    pub fn generate_keypair(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let ptr: u32 = self.with_retptr(|h, rp| {
            h.call_raw("zprotoEd25519GenerateKeyPair", &[Val::I32(rp as i32)])?;
            h.with(|store, _, memory, _, free| {
                let obj = Self::read_u32(store, memory, rp)?;
                let err_len = Self::read_u32(store, memory, rp + 4)?;
                let err_flag = Self::read_u32(store, memory, rp + 8)?;
                if err_flag != 0 {
                    let msg = Self::read_str(store, memory, obj, err_len);
                    let _ = Self::free_mem(store, free, obj, err_len);
                    anyhow::bail!("zprotoEd25519GenerateKeyPair: {msg}");
                }
                Ok(obj)
            })
        })?;
        // Getters return heap indices of Uint8Array views.
        let get_u8 = |this: &Self, name: &str| -> Result<Vec<u8>> {
            let idx: u32 = this.with(|store, instance, _, _, _| {
                let f = instance.get_func(&mut *store, name).ok_or_else(|| anyhow!("wasm missing export {name}"))?;
                let mut out = [Val::I32(0)];
                f.call(&mut *store, &[Val::I32(ptr as i32)], &mut out)?;
                match out[0] {
                    Val::I32(i) => Ok(i as u32),
                    _ => anyhow::bail!("{name} returned non-i32"),
                }
            })?;
            this.with(|store, _, memory, _, _| Ok(Self::take_bytes(store, memory, idx)))
        };
        let pubk = get_u8(self, "zprotoed25519keypair_publicKey")?;
        let privk = get_u8(self, "zprotoed25519keypair_privateKey")?;
        self.free_keypair(ptr)?;
        Ok((pubk, privk))
    }

    /// `O(a, b)` → 32-byte agreement. Mirrors glue `O()`.
    pub fn calculate_agreement(&self, sk: &[u8], pk: &[u8]) -> Result<Vec<u8>> {
        self.with_retptr(|h, rp| {
            let (aptr, alen, bptr, blen) = h.with(|store, _, memory, malloc, _| {
                let (a, al) = Self::pass_bytes(store, memory, malloc, sk)?;
                let (b, bl) = Self::pass_bytes(store, memory, malloc, pk)?;
                Ok((a, al, b, bl))
            })?;
            h.call_raw(
                "zprotoEd25519CalculateAgreement",
                &[Val::I32(rp as i32), Val::I32(aptr as i32), Val::I32(alen as i32), Val::I32(bptr as i32), Val::I32(blen as i32)],
            )?;
            h.with(|store, _, memory, _, free| {
                let ptr = Self::read_u32(store, memory, rp)?;
                let len = Self::read_u32(store, memory, rp + 4)?;
                let _err_ptr = Self::read_u32(store, memory, rp + 8)?;
                let err_flag = Self::read_u32(store, memory, rp + 12)?;
                if err_flag != 0 {
                    let msg = Self::read_str(store, memory, ptr, len);
                    let _ = Self::free_mem(store, free, ptr, len);
                    anyhow::bail!("zprotoEd25519CalculateAgreement: {msg}");
                }
                let out = Self::read_mem(store, memory, ptr, len);
                Self::free_mem(store, free, ptr, len)?;
                Ok(out)
            })
        })
    }

    /// `j(chunk, ik_pub, ik_priv, ek_priv, session_record?)` → decrypted
    /// metadata. Mirrors glue `T()`.
    pub fn decrypt_metadata(
        &self,
        chunk: &[u8],
        ik_pub: &[u8],
        ik_priv: &[u8],
        ek_priv: &[u8],
        session_record: Option<&[u8]>,
    ) -> Result<DecryptedMeta> {
        let obj: u32 = self.with_retptr(|h, rp| {
            let args = h.with(|store, _, memory, malloc, _| {
                let (c, cl) = Self::pass_bytes(store, memory, malloc, chunk)?;
                let (a, al) = Self::pass_bytes(store, memory, malloc, ik_pub)?;
                let (b, bl) = Self::pass_bytes(store, memory, malloc, ik_priv)?;
                let (d, dl) = Self::pass_bytes(store, memory, malloc, ek_priv)?;
                let (e, el) = match session_record {
                    Some(sr) => {
                        let (p, l) = Self::pass_bytes(store, memory, malloc, sr)?;
                        (p as i32, l as i32)
                    }
                    None => (0, 0),
                };
                Ok(vec![
                    Val::I32(rp as i32),
                    Val::I32(c as i32), Val::I32(cl as i32),
                    Val::I32(a as i32), Val::I32(al as i32),
                    Val::I32(b as i32), Val::I32(bl as i32),
                    Val::I32(d as i32), Val::I32(dl as i32),
                    Val::I32(e), Val::I32(el),
                ])
            })?;
            h.call_raw("zprotoSync2DecryptMetadata", &args)?;
            h.with(|store, _, memory, _, free| {
                let o = Self::read_u32(store, memory, rp)?;
                let err_len = Self::read_u32(store, memory, rp + 4)?;
                let err_flag = Self::read_u32(store, memory, rp + 8)?;
                if err_flag != 0 {
                    let msg = Self::read_str(store, memory, o, err_len);
                    let _ = Self::free_mem(store, free, o, err_len);
                    anyhow::bail!("zprotoSync2DecryptMetadata: {msg}");
                }
                Ok(o)
            })
        })?;
        let get_str = |name: &str| -> Result<String> {
            let idx: u32 = self.with(|store, instance, _, _, _| {
                let f = instance.get_func(&mut *store, name).ok_or_else(|| anyhow!("wasm missing export {name}"))?;
                let mut out = [Val::I32(0)];
                f.call(&mut *store, &[Val::I32(obj as i32)], &mut out)?;
                match out[0] {
                    Val::I32(i) => Ok(i as u32),
                    _ => anyhow::bail!("{name} returned non-i32"),
                }
            })?;
            self.with(|store, _, _, _, _| {
                let owned: JsVal = store.data_mut().take(idx);
                Ok(match owned {
                    JsVal::Str(s) => s,
                    JsVal::Num(n) => n.to_string(),
                    _ => String::new(),
                })
            })
        };
        let get_bytes = |name: &str| -> Result<Vec<u8>> {
            let idx: u32 = self.with(|store, instance, _, _, _| {
                let f = instance.get_func(&mut *store, name).ok_or_else(|| anyhow!("wasm missing export {name}"))?;
                let mut out = [Val::I32(0)];
                f.call(&mut *store, &[Val::I32(obj as i32)], &mut out)?;
                match out[0] {
                    Val::I32(i) => Ok(i as u32),
                    _ => anyhow::bail!("{name} returned non-i32"),
                }
            })?;
            self.with(|store, _, memory, _, _| Ok(Self::take_bytes(store, memory, idx)))
        };
        let out = DecryptedMeta {
            session_id: get_str("zprotosyncmetadatadecrypted_sessionId")?,
            plaintext: get_bytes("zprotosyncmetadatadecrypted_plaintext")?,
            sender_identity: get_str("zprotosyncmetadatadecrypted_senderIdentity")?,
            session_record: get_bytes("zprotosyncmetadatadecrypted_sessionRecord")?,
        };
        self.free_meta(obj)?;
        Ok(out)
    }

    /// `i(chunk, session_record)` → `(session_id, plaintext)`. Mirrors glue `R()`.
    pub fn decrypt_message(&self, chunk: &[u8], session_record: &[u8]) -> Result<DecryptedMsg> {
        let obj: u32 = self.with_retptr(|h, rp| {
            let args = h.with(|store, _, memory, malloc, _| {
                let (c, cl) = Self::pass_bytes(store, memory, malloc, chunk)?;
                let (s, sl) = Self::pass_bytes(store, memory, malloc, session_record)?;
                Ok(vec![
                    Val::I32(rp as i32),
                    Val::I32(c as i32), Val::I32(cl as i32),
                    Val::I32(s as i32), Val::I32(sl as i32),
                ])
            })?;
            h.call_raw("zprotoSync2DecryptMessage", &args)?;
            h.with(|store, _, memory, _, free| {
                let o = Self::read_u32(store, memory, rp)?;
                let err_len = Self::read_u32(store, memory, rp + 4)?;
                let err_flag = Self::read_u32(store, memory, rp + 8)?;
                if err_flag != 0 {
                    let msg = Self::read_str(store, memory, o, err_len);
                    let _ = Self::free_mem(store, free, o, err_len);
                    anyhow::bail!("zprotoSync2DecryptMessage: {msg}");
                }
                Ok(o)
            })
        })?;
        let session_id: String = self.with(|store, instance, _, _, _| {
            let f = instance
                .get_func(&mut *store, "zprotosyncmessagedecrypted_sessionId")
                .ok_or_else(|| anyhow!("missing sessionId"))?;
            let mut out = [Val::I32(0)];
            f.call(&mut *store, &[Val::I32(obj as i32)], &mut out)?;
            let idx = match out[0] {
                Val::I32(i) => i as u32,
                _ => anyhow::bail!("sessionId returned non-i32"),
            };
            let owned: JsVal = store.data_mut().take(idx);
            Ok(match owned {
                JsVal::Str(s) => s,
                JsVal::Num(n) => n.to_string(),
                _ => String::new(),
            })
        })?;
        let plaintext: Vec<u8> = self.with(|store, instance, memory, _, _| {
            let f = instance
                .get_func(&mut *store, "zprotosyncmessagedecrypted_plaintext")
                .ok_or_else(|| anyhow!("missing plaintext"))?;
            let mut out = [Val::I32(0)];
            f.call(&mut *store, &[Val::I32(obj as i32)], &mut out)?;
            let idx = match out[0] {
                Val::I32(i) => i as u32,
                _ => anyhow::bail!("plaintext returned non-i32"),
            };
            Ok(Self::take_bytes(store, memory, idx))
        })?;
        self.free_msg(obj)?;
        Ok(DecryptedMsg { session_id, plaintext })
    }
}

// ---------------------------------------------------------------------------
// Results + framing
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct DecryptedMeta {
    pub session_id: String,
    pub plaintext: Vec<u8>,
    pub sender_identity: String,
    pub session_record: Vec<u8>,
}

#[derive(Debug)]
pub struct DecryptedMsg {
    pub session_id: String,
    pub plaintext: Vec<u8>,
}

/// Split `[u32 LE len][chunk]...` framing (HistoryCodec `_split`).
/// Pure Rust port of `splitFrames` in `scripts/sync_v2_helper.js`.
pub fn split_frames(blob: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut chunks = Vec::new();
    let mut pos = 0;
    while pos < blob.len() {
        if blob.len() - pos < 4 {
            anyhow::bail!("Truncated frame header");
        }
        let len =
            u32::from_le_bytes([blob[pos], blob[pos + 1], blob[pos + 2], blob[pos + 3]]) as usize;
        pos += 4;
        if len == 0 || len > blob.len() - pos {
            anyhow::bail!("Invalid frame length");
        }
        chunks.push(blob[pos..pos + len].to_vec());
        pos += len;
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_load() -> Option<ZprotoHost> {
        match find_wasm_path() {
            None => match find_wasm_in_zalo_asar() {
                Some((origin, bytes)) => match ZprotoHost::load_from_bytes(&bytes) {
                    Ok(h) => Some(h),
                    Err(e) => panic!("failed to load WASM from {origin}: {e:#}"),
                },
                None => {
                    eprintln!("SKIP: no libzproto WASM found (install Zalo PC or set HZ_ZPROTO_WASM)");
                    None
                }
            },
            Some(p) => match ZprotoHost::load_from(&p) {
                Ok(h) => Some(h),
                Err(e) => {
                    panic!("failed to load WASM {}: {e:#}", p.display());
                }
            },
        }
    }

    #[test]
    fn wasm_missing_gives_clear_error() {
        if find_wasm_path().is_some() || find_wasm_in_zalo_asar().is_some() {
            eprintln!("WASM present; missing-path branch not exercised");
            return;
        }
        let err = ZprotoHost::load().unwrap_err().to_string();
        assert!(err.contains("libzproto_wasm_bg"), "clear WASM error: {err}");
        assert!(err.contains("Zalo PC"), "error names the requirement: {err}");
    }

    /// Write a minimal asar archive holding `files` (`(path, bytes, unpacked)`) and return it.
    fn write_fake_asar(name: &str, files: &[(&str, &[u8], bool)]) -> PathBuf {
        let mut root = serde_json::json!({ "files": {} });
        let mut data: Vec<u8> = Vec::new();
        for (path, bytes, unpacked) in files {
            let mut node = &mut root;
            let parts: Vec<&str> = path.split('/').collect();
            for dir in &parts[..parts.len() - 1] {
                node = node["files"]
                    .as_object_mut()
                    .unwrap()
                    .entry(dir.to_string())
                    .or_insert_with(|| serde_json::json!({ "files": {} }));
            }
            let entry = if *unpacked {
                serde_json::json!({ "size": bytes.len(), "unpacked": true })
            } else {
                serde_json::json!({ "size": bytes.len(), "offset": data.len().to_string() })
            };
            node["files"][parts[parts.len() - 1]] = entry;
            if !*unpacked {
                data.extend_from_slice(bytes);
            }
        }
        let json = serde_json::to_vec(&root).unwrap();
        let aligned = (json.len() + 3) & !3;
        let mut out = Vec::new();
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&((8 + aligned) as u32).to_le_bytes());
        out.extend_from_slice(&((4 + aligned) as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        out.resize(16 + aligned, 0);
        out.extend_from_slice(&data);
        let path = std::env::temp_dir().join(format!("hz_{name}_{}.asar", std::process::id()));
        std::fs::write(&path, out).unwrap();
        path
    }

    #[test]
    fn asar_reader_extracts_packed_libzproto_wasm() {
        let wasm: &[u8] = b"\0asm\x01\x00\x00\x00fake-module";
        let archive = write_fake_asar(
            "packed",
            &[
                ("pc-dist/index.js", b"console.log(1)", false),
                ("pc-dist/libs/libzproto_backup_wasm_bg.111.wasm", b"\0asm-backup", false),
                ("pc-dist/libs/libzproto_wasm_bg.5ea0.wasm", wasm, false),
            ],
        );
        let got = read_wasm_from_asar(&archive);
        let _ = std::fs::remove_file(&archive);
        let (entry, bytes) = got.expect("readable").expect("entry found");
        assert_eq!(entry, "/pc-dist/libs/libzproto_wasm_bg.5ea0.wasm");
        assert_eq!(bytes, wasm);
    }

    #[test]
    fn asar_reader_skips_unpacked_and_missing_entries() {
        let unpacked = write_fake_asar("unpacked", &[("libs/libzproto_wasm_bg.1.wasm", b"\0asm1234", true)]);
        let none = write_fake_asar("none", &[("libs/other.wasm", b"\0asm1234", false)]);
        let (a, b) = (read_wasm_from_asar(&unpacked), read_wasm_from_asar(&none));
        let _ = std::fs::remove_file(&unpacked);
        let _ = std::fs::remove_file(&none);
        assert!(a.expect("readable").is_none());
        assert!(b.expect("readable").is_none());
    }

    #[test]
    fn asar_reader_rejects_non_wasm_and_garbage() {
        let bogus = write_fake_asar("bogus", &[("libs/libzproto_wasm_bg.1.wasm", b"not a wasm module", false)]);
        let res = read_wasm_from_asar(&bogus);
        let _ = std::fs::remove_file(&bogus);
        assert!(res.unwrap_err().to_string().contains("not a WASM"));

        let garbage = std::env::temp_dir().join(format!("hz_garbage_{}.asar", std::process::id()));
        std::fs::write(&garbage, [0xffu8; 32]).unwrap();
        let res = read_wasm_from_asar(&garbage);
        let _ = std::fs::remove_file(&garbage);
        assert!(res.is_err());
    }

    #[test]
    fn zalo_version_folders_sort_numerically() {
        let mut dirs = [
            PathBuf::from("Zalo-26.9.3"),
            PathBuf::from("Zalo-26.11.11"),
            PathBuf::from("Zalo-26.10.2"),
        ];
        dirs.sort_by_key(|p| std::cmp::Reverse(zalo_version_key(p)));
        assert_eq!(dirs[0], PathBuf::from("Zalo-26.11.11"));
        assert_eq!(dirs[2], PathBuf::from("Zalo-26.9.3"));
    }

    /// Opt-in check against a real Zalo PC archive: set `HZ_TEST_ASAR` to its path.
    #[test]
    fn real_zalo_asar_wasm_instantiates() {
        let Ok(path) = std::env::var("HZ_TEST_ASAR") else {
            eprintln!("SKIP: HZ_TEST_ASAR not set");
            return;
        };
        let (entry, bytes) = read_wasm_from_asar(Path::new(&path))
            .expect("asar readable")
            .expect("libzproto entry present");
        let host = ZprotoHost::load_from_bytes(&bytes).expect("module instantiates");
        let (pk, sk) = host.generate_keypair().expect("keypair");
        eprintln!("{entry}: pk={} sk={}", pk.len(), sk.len());
        assert!(!pk.is_empty() && !sk.is_empty());
    }

    #[test]
    fn gen_keys_returns_ed25519_pair() {
        let Some(h) = try_load() else { return };
        let (pubk, privk) = h.generate_keypair().expect("gen-keys via wasmtime");
        assert_eq!(pubk.len(), 33, "pub must be 33 bytes");
        assert_eq!(pubk[0], 0x05, "pub must have 0x05 prefix");
        assert_eq!(privk.len(), 32, "priv must be 32 bytes");
    }

    #[test]
    fn gen_keys_are_unique_per_call() {
        // RNG shim phải cấp entropy thật mỗi lần gọi: key trùng nhau khiến
        // điện thoại văng app khi bấm vào thông báo đồng bộ (ek == ik).
        let Some(h) = try_load() else { return };
        let (p1, _) = h.generate_keypair().expect("keypair 1");
        let (p2, _) = h.generate_keypair().expect("keypair 2");
        assert_ne!(p1, p2, "hai lần gen key liên tiếp phải khác nhau");
    }

    #[test]
    fn agreement_is_symmetric() {
        let Some(h) = try_load() else { return };
        let (pub1, priv1) = h.generate_keypair().expect("keypair 1");
        let (pub2, priv2) = h.generate_keypair().expect("keypair 2");
        let a = h.calculate_agreement(&priv1, &pub2).expect("agreement 1-2");
        let b = h.calculate_agreement(&priv2, &pub1).expect("agreement 2-1");
        assert_eq!(a.len(), 32);
        assert_eq!(a, b, "agreement must be symmetric");
    }

    #[test]
    fn split_frames_roundtrip() {
        let blob = [3u32.to_le_bytes().as_slice(), b"abc", 2u32.to_le_bytes().as_slice(), b"de"].concat();
        let chunks = split_frames(&blob).unwrap();
        assert_eq!(chunks, vec![b"abc".to_vec(), b"de".to_vec()]);
        assert!(split_frames(b"ab").is_err());
    }

    #[test]
    fn strip_wasm_bindgen_hash_handles_hashed_and_unhashed_names() {
        assert_eq!(
            strip_wasm_bindgen_hash("__wbg_new_8a6f238a6ece86ea"),
            "__wbg_new"
        );
        assert_eq!(
            strip_wasm_bindgen_hash("__wbg_getRandomValues_bcb4912f16000dc4"),
            "__wbg_getRandomValues"
        );
        assert_eq!(
            strip_wasm_bindgen_hash("__wbindgen_object_drop_ref"),
            "__wbindgen_object_drop_ref"
        );
        assert_eq!(
            strip_wasm_bindgen_hash("__wbg_static_accessor_GLOBAL_THIS_56578be7e9f832b0"),
            "__wbg_static_accessor_GLOBAL_THIS"
        );
    }

    #[test]
    fn dynamic_import_resolver_links_rotated_wasm_bindgen_hashes() {
        fn push_u32_leb(out: &mut Vec<u8>, mut val: u32) {
            loop {
                let mut byte = (val & 0x7f) as u8;
                val >>= 7;
                if val != 0 {
                    byte |= 0x80;
                }
                out.push(byte);
                if val == 0 {
                    break;
                }
            }
        }
        fn push_section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
            out.push(id);
            push_u32_leb(out, body.len() as u32);
            out.extend_from_slice(body);
        }
        fn push_name(out: &mut Vec<u8>, s: &str) {
            push_u32_leb(out, s.len() as u32);
            out.extend_from_slice(s.as_bytes());
        }

        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

        // Type section:
        // 0: () -> i32
        // 1: (i32, i32, i32) -> ()
        // 2: (i32, i32) -> i32
        // 3: (i32, i32) -> ()
        let types = vec![
            4, // count
            0x60, 0, 1, 0x7f, // 0
            0x60, 3, 0x7f, 0x7f, 0x7f, 0, // 1
            0x60, 2, 0x7f, 0x7f, 1, 0x7f, // 2
            0x60, 2, 0x7f, 0x7f, 0, // 3
        ];
        push_section(&mut wasm, 1, &types);

        // Import section: 4 rotated wbg imports
        let imports = [
            ("__wbg_new_deadbeefcafebabe", 0u32),
            ("__wbg_set_1122334455667788", 1u32),
            ("__wbg_call_abcdef0123456789", 2u32),
            ("__wbg_getRandomValues_0011223344556677", 3u32),
        ];
        let mut imp_sec = Vec::new();
        push_u32_leb(&mut imp_sec, imports.len() as u32);
        for (name, ty_idx) in imports {
            push_name(&mut imp_sec, "wbg");
            push_name(&mut imp_sec, name);
            imp_sec.push(0x00); // func import
            push_u32_leb(&mut imp_sec, ty_idx);
        }
        push_section(&mut wasm, 2, &imp_sec);

        // Function section: 2 local functions (func 4 = malloc type 2, func 5 = free type 1)
        push_section(&mut wasm, 3, &[2, 2, 1]);

        // Memory section: 1 memory, min 1 page
        push_section(&mut wasm, 5, &[1, 0x00, 1]);

        // Export section: memory, __wbindgen_export_2, __wbindgen_export_1
        let mut exp_sec = Vec::new();
        push_u32_leb(&mut exp_sec, 3);
        push_name(&mut exp_sec, "memory");
        exp_sec.push(0x02);
        exp_sec.push(0);
        push_name(&mut exp_sec, "__wbindgen_export_2");
        exp_sec.push(0x00);
        exp_sec.push(4);
        push_name(&mut exp_sec, "__wbindgen_export_1");
        exp_sec.push(0x00);
        exp_sec.push(5);
        push_section(&mut wasm, 7, &exp_sec);

        // Code section: 2 minimal bodies
        let code_sec = vec![
            2, // count
            4, 0, 0x41, 16, 0x0b, // malloc: i32.const 16; end
            2, 0, 0x0b, // free: end
        ];
        push_section(&mut wasm, 10, &code_sec);

        let host = ZprotoHost::load_from_bytes(&wasm)
            .expect("rotated wasm-bindgen hashes should link dynamically");
        let _ = host;
    }
}
