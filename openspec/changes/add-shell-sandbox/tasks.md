## 1. Configuration Schema Extension
- [ ] 1.1 Add `SandboxConfig` struct to `config/schema.rs` with fields: `enabled`, `backend` (bwrap), `tmp_size_mb`
- [ ] 1.2 Add `workspace` (PathBuf) field to `ExecToolConfig`
- [ ] 1.3 Add `CommandPolicyConfig` struct with `allowlist` (Vec<String>) and `denylist` (Vec<String>)
- [ ] 1.4 Add `ResourceLimitsConfig` struct with `max_memory_mb`, `max_cpu_secs`, `max_output_bytes`
- [ ] 1.5 Integrate new structs into `ExecToolConfig` and update serde defaults
- [ ] 1.6 Write unit tests for config parsing with new fields (backward compatibility + new fields)

## 2. Command Policy Engine
- [ ] 2.1 Create `tools/command_policy.rs` with `CommandPolicy` struct
- [ ] 2.2 Implement allowlist matching (first token / binary name)
- [ ] 2.3 Implement denylist matching (substring/glob patterns)
- [ ] 2.4 Implement evaluation order: allowlist → denylist, with warn-level logging
- [ ] 2.5 Write unit tests: allowlist-only, denylist-only, both, empty, edge cases

## 3. Resource Limits Module
- [ ] 3.1 Create `tools/resource_limits.rs` with `ResourceLimits` struct
- [ ] 3.2 Implement `to_ulimit_prefix()` → generates `ulimit -v ... -t ...; ` prefix string for fallback mode
- [ ] 3.3 Implement `to_bwrap_args()` → generates `--rlimit-as`, `--rlimit-cpu` flags
- [ ] 3.4 Implement output truncation logic (read up to N bytes, append truncation marker)
- [ ] 3.5 Write unit tests for ulimit string generation and bwrap arg generation

## 4. Sandbox Provider Abstraction
- [ ] 4.1 Create `tools/sandbox.rs` with `SandboxProvider` trait: `fn build_command(&self, cmd: &str, working_dir: &Path, limits: &ResourceLimits) -> Command`
- [ ] 4.2 Implement `BwrapSandbox`: detect bwrap binary, build bwrap command with bind-ro /, bind-rw workspace, tmpfs /tmp, proc /proc, dev /dev
- [ ] 4.3 Implement `FallbackExecutor`: direct `bash -c` with ulimit prefix
- [ ] 4.4 Add startup detection: check `which bwrap`, log availability status
- [ ] 4.5 Write integration test: verify bwrap sandbox prevents writes outside workspace (Linux-only, skipped on CI without bwrap)

## 5. ExecTool Refactor
- [ ] 5.1 Update `ExecTool` struct to hold `CommandPolicy`, `SandboxProvider`, and `ResourceLimits`
- [ ] 5.2 Update `ExecTool::new()` to accept new config and initialize components
- [ ] 5.3 Refactor `execute()` flow: policy check → sandbox dispatch → timeout → output truncation
- [ ] 5.4 Update workspace directory resolution: use configured path or default `$HOME/workspace`, create if missing
- [ ] 5.5 Update tool description string to reflect sandbox capability
- [ ] 5.6 Preserve all existing tests, add new tests for sandboxed path

## 6. CLI Integration
- [ ] 6.1 Update `main.rs` ExecTool registration to pass new config fields
- [ ] 6.2 Update workspace directory resolution in both CLI and Gateway modes
- [ ] 6.3 Log sandbox status at startup (enabled/disabled, bwrap available/unavailable)

## 7. Documentation & Validation
- [ ] 7.1 Update `TOOLS.md` workspace template to document sandbox configuration options
- [ ] 7.2 Add example sandbox configuration to `config.yaml` comments or docs
- [ ] 7.3 Run `cargo test` to verify no regressions
- [ ] 7.4 Run `cargo clippy` to verify no new warnings
