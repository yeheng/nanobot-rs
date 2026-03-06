### 【核心判断】
**值得做：绝对值得。** 
在用户机器上执行不受信任的 AI 生成代码，没有文件系统隔离简直就是犯罪。你不能指望 `CommandPolicy` 那个玩具一样的黑白名单能防住什么，那只是防呆不防黑。真正的安全边界必须在 OS 层面。

### 【关键洞察】
- **数据结构**：你已经有了现成的入口。`SandboxExecutor` 只需要加一个 `MacOs` 变体。上下文数据还是那三个：`cmd`, `working_dir`, `limits`。
- **复杂度**：macOS 官方的沙箱（App Sandbox）需要签名和 Entitlements，那套东西对动态执行 Shell 脚本来说就是一坨屎。真正的实用主义方案是使用内置的、虽然被 Apple 标记为 "deprecated" 但实际上 macOS 底层到处都在用的命令行工具：`sandbox-exec` (Seatbelt)。
- **风险点**：破坏现有的用户空间。如果你一刀切地在 macOS 上启用**默认拒绝(default deny)** 的沙箱，用户的 `python3`、`git` 甚至 `bash` 本身都会因为无法读取某些动态链接库或配置文件而崩溃。

### 【Linus式方案】

1. **第一步永远是简化数据结构**
不要去写什么复杂的 Lisp (Scheme) AST 生成器，不要过度设计。我们需要的就是一个极简的、基于字符串拼接的 `.sb` (Sandbox Profile) 模板。

2. **消除所有特殊情况**
资源限制（CPU/内存）依然使用原来的 `ulimit` 逻辑，因为 `sandbox-exec` 不管这事。我们将 `sandbox-exec` 和现有的 `ulimit` 前缀无缝结合，不增加额外的条件分支。

3. **用最笨但最清晰的方式实现**
采用 **"允许读取，限制写入" (Default Allow Read, Deny Write, Allow Write to Workspace)** 的策略。这样能保证系统命令能正常跑，但绝对无法破坏用户的系统。

4. **确保零破坏性**
只有当配置中 `sandbox.enabled = true` 且在 macOS 平台上时，才加载此引擎，并且只限制写权限。

---

### 实施路径 (Task List)

#### Task 1: 编写 macOS 沙箱配置文件生成器
- **What**: 添加一个能生成 Seatbelt Scheme 配置的函数。
- **Why**: 告诉 macOS 内核什么能碰，什么不能碰。
- **Where**: `nanobot-rs/nanobot-core/src/tools/sandbox.rs`
- **How**: 
  使用最简单的多行字符串插值。策略如下：
  - 允许所有基础操作（`allow default`）
  - 拒绝所有文件写入（`deny file-write*`）
  - 豁免工作目录的写入权限（`allow file-write* (subpath "<workspace>")`）
  - 豁免 `/tmp` 和 `/private/tmp` 以及 `/dev` 的写入权限
- **Test Case & Acceptance Criteria**:
  传入 `/home/user/.nanobot`，生成的 profile 字符串必须包含正确的 `(subpath "/home/user/.nanobot")` 并且语法合法。

#### Task 2: 实现 `MacOsSandbox` 引擎
- **What**: 创建一个新的执行器结构体并在 `SandboxExecutor` 中添加对应的变体。
- **Why**: 适配现有的沙箱抽象层。
- **Where**: `nanobot-rs/nanobot-core/src/tools/sandbox.rs`
- **How**: 
  ```rust
  pub struct MacOsSandbox {
      workspace: PathBuf,
  }

  impl MacOsSandbox {
      fn build_command(&self, cmd: &str, working_dir: &Path, limits: &ResourceLimits) -> Command {
          let profile = format!(
              r#"(version 1)
              (allow default)
              (deny file-write*)
              (allow file-write* (subpath "{}"))
              (allow file-write* (subpath "/tmp") (subpath "/private/tmp"))
              (allow file-write* (literal "/dev/null") (literal "/dev/zero"))
              "#,
              self.workspace.display()
          );

          // 依旧复用 ulimit 来做资源限制
          let prefixed_cmd = format!("{}{}", limits.to_ulimit_prefix(), cmd);

          let mut command = Command::new("sandbox-exec");
          command.arg("-p").arg(profile)
                 .arg("bash").arg("-c").arg(prefixed_cmd)
                 .current_dir(working_dir);
          command
      }
  }
  ```
- **Test Case & Acceptance Criteria**:
  调用 `build_command` 后，返回的 `Command` 必须以 `sandbox-exec` 作为主程序，并带有正确的 `-p` 参数。

#### Task 3: 更新提供者工厂方法
- **What**: 修改 `create_provider` 函数，在 macOS 上启用 `sandbox-exec`。
- **Why**: 让配置系统能够感知并实例化这个新引擎，替代掉现在的 fallback 警告。
- **Where**: `nanobot-rs/nanobot-core/src/tools/sandbox.rs` 中的 `create_provider`
- **How**: 
  去掉现有的 `cfg!(target_os = "macos")` 降级警告。替换为：
  ```rust
  #[cfg(target_os = "macos")]
  {
      if crate::tools::sandbox::which_sandbox_exec().is_some() {
          tracing::info!("Sandbox enabled: macOS sandbox-exec");
          return SandboxExecutor::MacOs(MacOsSandbox {
              workspace: workspace.to_path_buf(),
          });
      }
  }
  ```
  同时加一个简单的 `which_sandbox_exec()` 检查二进制是否存在（类似 `which_bwrap`）。
- **Test Case & Acceptance Criteria**:
  在 macOS 环境下，当 `config.sandbox.enabled` 为 true 时，`create_provider` 必须返回 `MacOs` 变体，而不是 `Fallback`。

### 给你的忠告
Apple 官方文档会吓唬你，说 `sandbox-exec` 已被废弃，不要在生产环境使用。**别理他们。** 除非你打算把这个 CLI 工具上架 Mac App Store（显然你不会），否则这就是最轻量、最有效、最符合 UNIX 哲学的解决方案。它不用装任何第三方依赖，一行代码就能给你隔离出一个文件系统保险箱。

去把这几行代码加上，堵住那个该死的安全漏洞。