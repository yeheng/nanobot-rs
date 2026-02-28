## 1. Configuration Schema Extension
- [x] 1.1 Add `SandboxConfig` struct to `config/schema.rs` with fields: `enabled`, `backend` (bwrap), `tmp_size_mb`
- [x] 1.2 Add `workspace` (PathBuf) field to `ExecToolConfig`
- [x] 1.3 Add `CommandPolicyConfig` struct with `allowlist` (Vec<String>) and `denylist` (Vec<String>)
- [x] 1.4 Add `ResourceLimitsConfig` struct with `max_memory_mb`, `max_cpu_secs`, `max_output_bytes`
- [x] 1.5 Integrate new structs into `ExecToolConfig` and update serde defaults
- [x] 1.6 Write unit tests for config parsing with new fields (backward compatibility + new fields)

## 2. Command Policy Engine
- [x] 2.1 Create `tools/command_policy.rs` with `CommandPolicy` struct
- [x] 2.2 Implement allowlist matching (first token / binary name)
- [x] 2.3 Implement denylist matching (substring/glob patterns)
- [x] 2.4 Implement evaluation order: denylist → allowlist, with warn-level logging
- [x] 2.5 Write unit tests: allowlist-only, denylist-only, both, empty, edge cases

## 3. Resource Limits Module
- [x] 3.1 Create `tools/resource_limits.rs` with `ResourceLimits` struct
- [x] 3.2 Implement `to_ulimit_prefix()` → generates `ulimit -v ... -t ...; ` prefix string for fallback mode
- [x] 3.3 Implement `to_bwrap_args()` → generates `--rlimit-as`, `--rlimit-cpu` flags
- [x] 3.4 Implement output truncation logic (read up to N bytes, append truncation marker)
- [x] 3.5 Write unit tests for ulimit string generation and bwrap arg generation

## 4. Sandbox Provider Abstraction
- [x] 4.1 Create `tools/sandbox.rs` with `SandboxProvider` trait: `fn build_command(&self, cmd: &str, working_dir: &Path, limits: &ResourceLimits) -> Command`
- [x] 4.2 Implement `BwrapSandbox`: detect bwrap binary, build bwrap command with bind-ro /, bind-rw workspace, tmpfs /tmp, proc /proc, dev /dev
- [x] 4.3 Implement `FallbackExecutor`: direct `bash -c` with ulimit prefix
- [x] 4.4 Add startup detection: check `which bwrap`, log availability status
- [x] 4.5 Write integration test: verify bwrap sandbox prevents writes outside workspace (Linux-only, skipped on CI without bwrap)

## 5. ExecTool Refactor
- [x] 5.1 Update `ExecTool` struct to hold `CommandPolicy`, `SandboxProvider`, and `ResourceLimits`
- [x] 5.2 Update `ExecTool::new()` to accept new config and initialize components (via `from_config`)
- [x] 5.3 Refactor `execute()` flow: policy check → sandbox dispatch → timeout → output truncation
- [x] 5.4 Update workspace directory resolution: use configured path or default `$HOME/.nanobot`, create if missing
- [x] 5.5 Update tool description string to reflect sandbox capability
- [x] 5.6 Preserve all existing tests, add new tests for sandboxed path

## 6. CLI Integration
- [x] 6.1 Update `main.rs` ExecTool registration to pass new config fields
- [x] 6.2 Update workspace directory resolution in both CLI and Gateway modes
- [x] 6.3 Log sandbox status at startup (enabled/disabled, bwrap available/unavailable)

## 7. Documentation & Validation
- [x] 7.1 Update `TOOLS.md` workspace template to document sandbox configuration options
- [x] 7.2 Add example sandbox configuration to `config.yaml` comments or docs
- [x] 7.3 Run `cargo test` to verify no regressions (198 unit + 68 e2e tests pass)
- [x] 7.4 Run `cargo clippy` to verify no new warnings (clean)
