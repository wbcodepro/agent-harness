use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use wasmtime::{Caller, Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use zcode_kernel::BoxFuture;
use zcode_session::ToolSpec;
use zcode_tools::{Effect, Tool, ToolContext, ToolOutput};

/// Instructions a plugin may execute per call before it is stopped.
const FUEL_PER_CALL: u64 = 2_000_000_000;
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Largest file handed to a plugin, and largest string taken from one.
const MAX_TRANSFER_BYTES: usize = 4 * 1024 * 1024;

/// A WebAssembly plugin the user installed, and what they let it reach.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConfig {
    /// The `.wasm` file.
    pub path: PathBuf,
    /// Directories the plugin may read files under. Relative paths are
    /// taken from the working directory. Empty means it can read nothing.
    #[serde(default)]
    pub read: Vec<PathBuf>,
}

/// A compiled plugin.
///
/// A plugin runs inside the WebAssembly sandbox: it has no filesystem,
/// network, clock or environment of its own. The single thing it can ask
/// the host for is the content of a file, and only under the directories
/// granted in its configuration. Every call gets a fresh instance, a
/// memory cap and an instruction budget.
pub struct WasmPlugin {
    name: String,
    engine: Engine,
    module: Module,
    /// Canonical directories the plugin may read under.
    grants: Vec<PathBuf>,
    cwd: PathBuf,
}

struct Host {
    grants: Vec<PathBuf>,
    cwd: PathBuf,
    limits: StoreLimits,
}

fn unpack(packed: i64) -> (usize, usize) {
    let packed = packed as u64;
    ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize)
}

impl Host {
    fn read(&self, path: &str) -> Result<String, String> {
        let requested = Path::new(path);
        let absolute = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.cwd.join(requested)
        };
        // Resolve symlinks and `..` before checking, so neither can lead
        // out of a granted directory.
        let real = absolute
            .canonicalize()
            .map_err(|e| format!("{path}: {e}"))?;
        if !self.grants.iter().any(|grant| real.starts_with(grant)) {
            return Err(format!(
                "{path}: this plugin has not been granted access to that location"
            ));
        }
        let size = std::fs::metadata(&real).map_err(|e| e.to_string())?.len();
        if size > MAX_TRANSFER_BYTES as u64 {
            return Err(format!("{path}: larger than {MAX_TRANSFER_BYTES} bytes"));
        }
        std::fs::read_to_string(&real).map_err(|e| format!("{path}: {e}"))
    }
}

/// Copies `text` into the guest through its own allocator and returns the
/// packed pointer and length.
fn give(caller: &mut Caller<'_, Host>, text: &str) -> wasmtime::Result<i64> {
    let alloc = caller
        .get_export("zcode_alloc")
        .and_then(|export| export.into_func())
        .ok_or_else(|| wasmtime::Error::msg("the plugin does not export zcode_alloc"))?
        .typed::<i32, i32>(&mut *caller)?;
    let pointer = alloc.call(&mut *caller, text.len() as i32)?;
    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("the plugin does not export its memory"))?;
    memory.write(&mut *caller, pointer as u32 as usize, text.as_bytes())?;
    Ok(((pointer as u32 as i64) << 32) | text.len() as i64)
}

impl WasmPlugin {
    pub fn load(name: &str, config: &PluginConfig, cwd: &Path) -> Result<Arc<Self>, String> {
        let mut engine_config = Config::new();
        engine_config.consume_fuel(true);
        let engine = Engine::new(&engine_config).map_err(|e| e.to_string())?;
        let path = if config.path.is_absolute() {
            config.path.clone()
        } else {
            cwd.join(&config.path)
        };
        let module = Module::from_file(&engine, &path)
            .map_err(|e| format!("cannot load plugin {name} from {}: {e}", path.display()))?;
        let grants = config
            .read
            .iter()
            .filter_map(|dir| {
                let dir = if dir.is_absolute() {
                    dir.clone()
                } else {
                    cwd.join(dir)
                };
                dir.canonicalize().ok()
            })
            .collect();
        Ok(Arc::new(Self {
            name: name.to_string(),
            engine,
            module,
            grants,
            cwd: cwd.to_path_buf(),
        }))
    }

    /// Instantiates the plugin and calls one of its exports with an
    /// optional JSON argument, returning the JSON string it produced.
    fn invoke(&self, export: &str, argument: Option<&str>) -> Result<String, String> {
        let fail = |e: wasmtime::Error| {
            let text = format!("{e:#}");
            if text.contains("fuel") {
                format!(
                    "plugin {} used up its computation budget and was stopped",
                    self.name
                )
            } else {
                format!("plugin {} failed: {text}", self.name)
            }
        };
        let mut store = Store::new(
            &self.engine,
            Host {
                grants: self.grants.clone(),
                cwd: self.cwd.clone(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(MAX_MEMORY_BYTES)
                    .build(),
            },
        );
        store.limiter(|host| &mut host.limits);
        store.set_fuel(FUEL_PER_CALL).map_err(fail)?;

        let mut linker = Linker::<Host>::new(&self.engine);
        linker
            .func_wrap(
                "zcode",
                "read_file",
                |mut caller: Caller<'_, Host>, pointer: i32, len: i32| -> wasmtime::Result<i64> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|export| export.into_memory())
                        .ok_or_else(|| {
                            wasmtime::Error::msg("the plugin does not export its memory")
                        })?;
                    let len = (len as u32 as usize).min(4096);
                    let mut raw = vec![0u8; len];
                    memory.read(&caller, pointer as u32 as usize, &mut raw)?;
                    let path = String::from_utf8_lossy(&raw).into_owned();
                    let reply = match caller.data().read(&path) {
                        Ok(text) => json!({ "ok": text }),
                        Err(why) => json!({ "error": why }),
                    };
                    give(&mut caller, &reply.to_string())
                },
            )
            .map_err(fail)?;
        let instance = linker.instantiate(&mut store, &self.module).map_err(fail)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| format!("plugin {} does not export its memory", self.name))?;

        let packed = match argument {
            None => instance
                .get_typed_func::<(), i64>(&mut store, export)
                .and_then(|func| func.call(&mut store, ()))
                .map_err(fail)?,
            Some(argument) => {
                let alloc = instance
                    .get_typed_func::<i32, i32>(&mut store, "zcode_alloc")
                    .map_err(fail)?;
                let pointer = alloc
                    .call(&mut store, argument.len() as i32)
                    .map_err(fail)?;
                memory
                    .write(&mut store, pointer as u32 as usize, argument.as_bytes())
                    .map_err(|e| format!("plugin {} failed: {e}", self.name))?;
                instance
                    .get_typed_func::<(i32, i32), i64>(&mut store, export)
                    .and_then(|func| func.call(&mut store, (pointer, argument.len() as i32)))
                    .map_err(fail)?
            }
        };
        let (pointer, len) = unpack(packed);
        if len > MAX_TRANSFER_BYTES {
            return Err(format!("plugin {} returned too much data", self.name));
        }
        let mut raw = vec![0u8; len];
        memory
            .read(&store, pointer, &mut raw)
            .map_err(|e| format!("plugin {} returned an invalid result: {e}", self.name))?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    /// The tools the plugin declares, named `<plugin>__<tool>`.
    pub fn tools(self: &Arc<Self>) -> Result<Vec<Arc<dyn Tool>>, String> {
        let declared: Value = serde_json::from_str(&self.invoke("zcode_tools", None)?)
            .map_err(|e| format!("plugin {} declared its tools badly: {e}", self.name))?;
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for tool in declared.as_array().into_iter().flatten() {
            let Some(remote) = tool["name"].as_str() else {
                continue;
            };
            let name: String = format!("{}__{remote}", self.name)
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(64)
                .collect();
            tools.push(Arc::new(PluginTool {
                plugin: Arc::clone(self),
                remote: remote.to_string(),
                spec: ToolSpec {
                    name,
                    description: tool["description"].as_str().unwrap_or("").to_string(),
                    input_schema: match &tool["input_schema"] {
                        Value::Object(schema) => Value::Object(schema.clone()),
                        _ => json!({"type": "object"}),
                    },
                },
            }));
        }
        Ok(tools)
    }
}

struct PluginTool {
    plugin: Arc<WasmPlugin>,
    remote: String,
    spec: ToolSpec,
}

impl Tool for PluginTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    /// A plugin has no way to change anything outside its own memory.
    fn effect(&self) -> Effect {
        Effect::ReadOnly
    }

    fn host_side(&self) -> bool {
        true
    }

    fn call<'a>(&'a self, input: &'a Value, _ctx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let request = json!({"tool": self.remote, "input": input}).to_string();
            let plugin = Arc::clone(&self.plugin);
            let outcome =
                tokio::task::spawn_blocking(move || plugin.invoke("zcode_call", Some(&request)))
                    .await;
            let reply = match outcome {
                Ok(Ok(reply)) => reply,
                Ok(Err(why)) => return ToolOutput::error(why),
                Err(e) => return ToolOutput::error(format!("the plugin crashed: {e}")),
            };
            match serde_json::from_str::<Value>(&reply) {
                Ok(reply) => ToolOutput {
                    text: reply["text"].as_str().unwrap_or("").to_string(),
                    is_error: reply["is_error"].as_bool().unwrap_or(false),
                },
                Err(e) => ToolOutput::error(format!("the plugin returned an invalid result: {e}")),
            }
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::OnceLock;

    /// Builds the example plugin once per test run.
    fn example_plugin() -> &'static Path {
        static BUILT: OnceLock<PathBuf> = OnceLock::new();
        BUILT.get_or_init(|| {
            let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wordcount");
            let status = Command::new(env!("CARGO"))
                .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
                .current_dir(&source)
                .env_remove("CARGO_TARGET_DIR")
                .status()
                .expect("cargo runs");
            assert!(
                status.success(),
                "building the example plugin needs the wasm32-unknown-unknown target"
            );
            source.join("target/wasm32-unknown-unknown/release/zcode_plugin_wordcount.wasm")
        })
    }

    #[tokio::test]
    async fn a_plugin_declares_tools_and_reads_only_what_it_was_granted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("private")).unwrap();
        std::fs::write(root.join("docs/a.txt"), "one two\nthree\n").unwrap();
        std::fs::write(root.join("private/key"), "secret").unwrap();
        std::os::unix::fs::symlink(root.join("private/key"), root.join("docs/link")).unwrap();

        let config = PluginConfig {
            path: example_plugin().to_path_buf(),
            read: vec!["docs".into()],
        };
        let plugin = WasmPlugin::load("wc", &config, &root).unwrap();
        let tools = plugin.tools().unwrap();
        let names: Vec<String> = tools.iter().map(|tool| tool.spec().name).collect();
        assert_eq!(names, ["wc__text_stats", "wc__file_stats"]);
        assert!(
            tools
                .iter()
                .all(|tool| tool.effect() == Effect::ReadOnly && tool.host_side())
        );

        let ctx = ToolContext::new(&root);
        let pure = tools[0]
            .call(&json!({"text": "xin chào thế giới"}), &ctx)
            .await;
        assert_eq!(pure, ToolOutput::ok("1 lines, 4 words, 17 characters"));

        let granted = tools[1].call(&json!({"path": "docs/a.txt"}), &ctx).await;
        assert_eq!(granted, ToolOutput::ok("2 lines, 3 words, 14 characters"));

        // Outside the grant: directly, by traversal, and through a symlink
        // that sits inside the granted directory.
        for path in [
            "private/key",
            "docs/../private/key",
            "docs/link",
            "/etc/hosts",
        ] {
            let denied = tools[1].call(&json!({ "path": path }), &ctx).await;
            assert!(
                denied.is_error && denied.text.contains("not been granted"),
                "{path}: {}",
                denied.text
            );
        }

        // With no grant at all the plugin can read nothing.
        let none = PluginConfig {
            read: vec![],
            ..config
        };
        let tools = WasmPlugin::load("wc", &none, &root)
            .unwrap()
            .tools()
            .unwrap();
        let denied = tools[1].call(&json!({"path": "docs/a.txt"}), &ctx).await;
        assert!(denied.is_error);
    }

    #[tokio::test]
    async fn runaway_and_broken_plugins_are_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let spinning = dir.path().join("spin.wat");
        std::fs::write(
            &spinning,
            r#"(module
                (memory (export "memory") 1)
                (data (i32.const 0) "[{\"name\":\"spin\"}]")
                (func (export "zcode_alloc") (param i32) (result i32) (i32.const 1024))
                (func (export "zcode_tools") (result i64) (i64.const 17))
                (func (export "zcode_call") (param i32 i32) (result i64)
                    (loop $forever (br $forever))
                    (i64.const 0)))"#,
        )
        .unwrap();
        let config = PluginConfig {
            path: spinning,
            read: vec![],
        };
        let plugin = WasmPlugin::load("bad", &config, dir.path()).unwrap();
        let tools = plugin.tools().unwrap();
        assert_eq!(tools[0].spec().name, "bad__spin");
        let stopped = tools[0]
            .call(&json!({}), &ToolContext::new(dir.path()))
            .await;
        assert!(
            stopped.is_error && stopped.text.contains("computation budget"),
            "{}",
            stopped.text
        );

        let missing = PluginConfig {
            path: dir.path().join("none.wasm"),
            read: vec![],
        };
        assert!(WasmPlugin::load("none", &missing, dir.path()).is_err());
    }
}
