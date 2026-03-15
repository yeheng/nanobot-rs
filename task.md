### 第一层：数据结构分析

"Bad programmers worry about the code. Good programmers worry about data structures."

**关于 `nanobot-sandbox` 的 AuditLog**：
你们的 `AuditLog::write` 方法里，每次写日志都要调用 `OpenOptions::new().create(true).append(true).open(&self.path)`。
**你是在逗我吗？** 每记录一条操作你都要重新发起一次系统调用去打开文件、写入、落盘、关闭？如果你在一秒内执行了 100 个命令，你就要把磁盘 I/O 操死。正确的数据结构应该是一个常驻的、带缓冲的写入器（Buffered Writer），加上一个后台定时 flush 的机制。

**关于 `tantivy-cli` 的 Rebuild 操作**：
看一眼 `rebuild.rs` 里的 `rebuild_index`：
`let docs = manager.list_documents(index_name, usize::MAX, 0)?;`
你把 `usize::MAX` 传进去，直接把整个索引库里的所有文档全部加载到一个 `Vec<Document>` 里？如果我的索引里有 1000 万篇文档，你的 CLI 程序就会直接吃光系统内存然后 OOM 暴毙。数据流走向完全错了，这必须是流式（Stream）或者游标（Cursor）分页处理的！

### 第二层：特殊情况识别

"好代码没有特殊情况"

**关于 Sandbox 的命令执行防注入**：
在 `executor/command.rs` 里，你们写了一个 `check_dangerous_patterns`，试图用一个黑名单 `[";", "|", "&", ">", "<", "$", "\n", "\r"]` 来拦截命令注入。
同时，在真正的执行后端（比如 `linux.rs`），你们是这样构造命令的：
`command.arg("bash").arg("-c").arg(cmd);`
**这是彻头彻尾的垃圾安全设计。** 你费尽心机用 `bwrap` 和 `sandbox-exec` 做了 OS 级别的命名空间隔离，结果转手就把未经转义的原始字符串塞进 `bash -c` 里执行？黑名单永远防不住 shell 注入。不要用 shell！应该直接要求输入 `["/bin/ls", "-la", "/tmp"]` 这样的数组格式，通过 `execvp` 语义直接执行二进制文件，彻底干掉特殊字符转义这个伪命题！

### 第三层：复杂度审查

"如果实现需要超过3层缩进，重新设计它"

**关于 `tantivy-cli` 的背景任务队列**：
你的程序是一个 **CLI**（命令行工具），运行完就退出了。但是你在 `manager.rs` 里搞了一个 `JobRegistry`，用 `DashMap` 存状态，用 `tokio::spawn` 扔到后台去异步写入，然后你在 `main.rs` 里写了一个 `wait_for_job_completion` 循环，用 `sleep(100ms)` 去轮询这个 DashMap 看任务完没完成？
**这是何等愚蠢的过度设计！** 这是一个典型的把简单的同步函数调用（写入磁盘）强行扭曲成“微服务级异步任务队列”的炫技产物。如果 CLI 命令要添加文档，直接在当前线程获取 RwLock 写锁加进去然后返回就行了。为了不阻塞不存在的其他请求，你生生造了一整套状态机！

### 第四层：破坏性分析

"Never break userspace"

清理 `JobRegistry` 会破坏现有的 CLI 调用习惯吗？不会。用户只想敲回车然后看到结果，他们不在乎底层是不是通过一个幽灵队列绕了一圈。
修复 `rebuild_index` 会破坏接口吗？不会。只会救命，防止真实用户空间出现内存爆炸。
修复 `bash -c` 会破坏向后兼容吗？可能会，因为现有的 workflow 可能依赖了 shell 特性（比如 `ls *.md`）。如果你必须保留 shell，至少使用安全的 `sh -c -- "$@"` 传参模式，而不是直接字符串拼接！

### 第五层：实用性验证

问题在生产环境中真的存在吗？
1. **OOM 炸弹**：只要索引大一点，`tantivy-cli` 必死无疑。绝对致命。
2. **日志 I/O 瓶颈**：并发高的沙箱场景下，审计日志会让磁盘 IO 飙升。
3. **架构复杂度**：代码里一堆为了 Job 而存在的样板代码，纯属负担。

---

### 【决策输出模式】

【核心判断】
**必须重构**：代码中潜伏着极度危险的内存炸弹（OOM）和安全漏洞（Shell拼接），同时存在为了炫技而制造的并发伪需求。

【关键洞察】
- **数据结构**：`rebuild_index` 的全量加载必须改为流式（Stream/Iterator）按批次读取和写入。
- **复杂度**：`tantivy-cli` 里的 `JobRegistry` 完全是脱裤子放屁，CLI 应该直接做同步的阻塞等待操作。
- **风险点**：沙箱环境中的 `bash -c` 搭配黑名单拦截是安全界公认的反面教材。

---

### 【代码审查输出】

【品味评分】
**垃圾** (存在严重的过度设计和常识性性能/安全错误)

【致命问题】
1. **OOM 炸弹**：`tantivy-cli/src/maintenance/rebuild.rs:60` 盲目使用 `usize::MAX` 加载所有文档到内存中。
2. **虚假的沙箱安全**：`nanobot-sandbox/src/backend/platform/linux.rs:77` 竟然使用 `bash -c <cmd_string>`，且完全依赖外部软弱的字符串过滤。
3. **荒谬的 CLI 异步轮询**：`tantivy-cli/src/main.rs:242` 里写了一个包含 `sleep` 的 `while` 轮询循环，只是为了等待本可以直接同步执行的本地写盘操作。

【改进方向】
"把 `usize::MAX` 改成按批次加载，或者提供游标。"
"把沙箱命令的输入拆分成二进制和参数数组，彻底干掉 `bash -c`。"
"砍掉 CLI 里的 Job 注册表，KISS 原则懂不懂？"

---

### 【Task List】

- [ ] **Task 1: 修复 `tantivy-cli` 的 Rebuild 内存爆炸 (OOM) 漏洞**
  - **What**: 重构 `rebuild_index` 函数。
  - **Why**: 现在的实现试图把千万级别的文档全塞进一个 `Vec` 里，绝对会崩溃。
  - **Where**: `tantivy-cli/src/maintenance/rebuild.rs` -> `rebuild_index`。
  - **How**: 不要调用带 `usize::MAX` 的 `list_documents`。应该利用 tantivy 的 API 分批（Batch）读取文档，或者在持有新旧两个 Index 的同时，按 Segment 去流式重建。最简单的修复是：分页加载（利用 offset 和 limit 循环读取），读取一批 -> 写入新 index 一批。
  - **Test Case**: 创建一个包含 100 万条最小记录的测试索引，触发 rebuild。
  - **Acceptance Criteria**: 内存占用保持在低位且平稳，不随文档数量线性增长。

- [ ] **Task 2: 拆除 `tantivy-cli` 中过度设计的 `JobRegistry`**
  - **What**: 移除 `JobRegistry`，将 `add_document`、`commit`、`delete` 等 CLI 写入操作改为直接在当前异步上下文同步等待的直接调用。
  - **Why**: 这是个 CLI 工具。用户执行 `tantivy-cli doc add`，程序应该在当前 task 直接加写锁、执行、返回。用一套微服务级别的 job 状态机去配合一个 CLI 是愚蠢的。
  - **Where**: `tantivy-cli/src/index/manager.rs` 和 `tantivy-cli/src/main.rs`。
  - **How**: 去掉 `manager.add_document` 中 `rt_handle.spawn` 和 `job_registry` 的依赖，改成拿了写锁直接写入并 await 返回。删除 `wait_for_job_completion`，不再使用 `sleep` 轮询。
  - **Acceptance Criteria**: `tantivy-cli` 的源码减少不必要的样板代码，单次执行延迟降低（无需轮询间隔）。

- [ ] **Task 3: 堵住 Sandbox 中 `bash -c` 造成的注入漏洞**
  - **What**: 改变 `SandboxBackend::build_command` 的签名和实现，支持传递严格的参数数组。
  - **Why**: 现有的 `check_dangerous_patterns` 基于黑名单的防御是不可能完美的（比如它甚至没防御 backticks `` ` `` 或者 `$()` 的各种变体）。使用 `bash -c "string"` 极大削弱了隔离性。
  - **Where**: `nanobot-sandbox/src/backend/platform/*.rs` 和 `nanobot-sandbox/src/executor/command.rs`。
  - **How**: 将传入的 `command: &str` 改为 `program: &str, args: &[&str]`。如果你真的非得提供便捷的字符串执行，至少在传给 bash 时这样做：`command.arg("bash").arg("-c").arg("$@").arg("--").arg(cmd)`，确保 shell 把原始输入当成一个单一实体（或完全不要用 shell）。
  - **Acceptance Criteria**: 可以安全执行任意包含特殊符号的命令串而不触发意外的 Shell expansion。

- [ ] **Task 4: 优化 `AuditLog` 文件 I/O 性能**
  - **What**: 修复每次写入都要 reopen 文件的性能问题。
  - **Why**: `AuditLog::write` 每次都会走一整遍文件系统 open/close，在频繁写操作时性能极差。
  - **Where**: `nanobot-sandbox/src/audit/log.rs`。
  - **How**: 在 `AuditLog` struct 内部持有一个异步的带互斥锁的 `BufWriter<tokio::fs::File>` (或通过专门的 logger 线程 channel 传递)。每次 `write` 只进行 buffer 写入。注意处理文件滚动（rotation）时重新获取文件句柄。
  - **Acceptance Criteria**: 沙箱高频执行短命令时，审计日志模块不成为系统 I/O 的瓶颈。